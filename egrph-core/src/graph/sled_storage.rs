//! Persistent graph storage backed by [sled](https://docs.rs/sled).
//!
//! Enable with the `sled-storage` Cargo feature:
//! ```toml
//! egrph-core = { version = "...", features = ["sled-storage"] }
//! ```
//!
//! # Data Layout
//!
//! All data is stored across six named sled Trees:
//!
//! | Tree | Key | Value |
//! |------|-----|-------|
//! | `nodes` | `NodeId` (u64 big-endian) | `Node` (bincode) |
//! | `edges` | `EdgeId` (u64 big-endian) | `Edge` (bincode) |
//! | `label_idx` | `"{label}\x00{node_id_be}"` | empty |
//! | `outgoing` | `NodeId` (u64 BE) | `Vec<EdgeId>` (bincode) |
//! | `incoming` | `NodeId` (u64 BE) | `Vec<EdgeId>` (bincode) |
//! | `meta` | ASCII key | u64 (big-endian 8 bytes) |
//!
//! Writes are immediately flushed to disk via sled's ACID transaction log.

use super::backend::StorageBackend;
use super::types::{Edge, EdgeId, Node, NodeId, PropertyValue};
use sled::Db;
use std::collections::HashMap;
use std::path::Path;

/// Persistent graph storage backed by sled.
pub struct SledStorage {
    _db: Db,
    nodes: sled::Tree,
    edges: sled::Tree,
    label_idx: sled::Tree,
    outgoing: sled::Tree,
    incoming: sled::Tree,
    meta: sled::Tree,
    /// Property index: `{prop_name}\x00{type_byte}{value_bytes}{node_id_be}` → empty.
    prop_idx: sled::Tree,
    /// Unique constraints: `{label}\x00{property}` → empty.
    constraints: sled::Tree,
}

// ── Key helpers ──────────────────────────────────────────────────────────────

fn u64_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

fn label_key(label: &str, node_id: NodeId) -> Vec<u8> {
    let mut key = label.as_bytes().to_vec();
    key.push(0x00);
    key.extend_from_slice(&node_id.to_be_bytes());
    key
}

fn label_prefix(label: &str) -> Vec<u8> {
    let mut p = label.as_bytes().to_vec();
    p.push(0x00);
    p
}

fn prop_idx_value_bytes(val: &PropertyValue) -> Vec<u8> {
    // Format: type_byte + fixed-length or length-prefixed value bytes.
    // Using a length prefix for strings ensures no ambiguity when the key is
    // used as a scan prefix (prefix = prop_name + \x00 + value_bytes, the last
    // 8 bytes of the full key are always the node_id).
    match val {
        PropertyValue::String(s) => {
            let bytes = s.as_bytes();
            let mut v = vec![b's'];
            v.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            v.extend_from_slice(bytes);
            v
        }
        PropertyValue::Int(i) => {
            let mut v = vec![b'i'];
            v.extend_from_slice(&i.to_be_bytes());
            v
        }
        PropertyValue::Float(f) => {
            let mut v = vec![b'f'];
            v.extend_from_slice(&f.to_bits().to_be_bytes());
            v
        }
        PropertyValue::Bool(b) => vec![b'b', *b as u8],
    }
}

fn prop_idx_prefix(prop_name: &str, val: &PropertyValue) -> Vec<u8> {
    let mut key = prop_name.as_bytes().to_vec();
    key.push(0x00);
    key.extend_from_slice(&prop_idx_value_bytes(val));
    key
}

fn prop_idx_key(prop_name: &str, val: &PropertyValue, node_id: NodeId) -> Vec<u8> {
    let mut key = prop_idx_prefix(prop_name, val);
    key.extend_from_slice(&node_id.to_be_bytes());
    key
}

fn constraint_key(label: &str, property: &str) -> Vec<u8> {
    let mut key = label.as_bytes().to_vec();
    key.push(0x00);
    key.extend_from_slice(property.as_bytes());
    key
}

// ── Codec helpers ─────────────────────────────────────────────────────────────

fn encode<T: serde::Serialize>(v: &T) -> Vec<u8> {
    bincode::serialize(v).expect("bincode encode")
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    bincode::deserialize(bytes).ok()
}

fn read_u64(bytes: &[u8]) -> Option<u64> {
    bytes.try_into().ok().map(u64::from_be_bytes)
}

fn write_u64(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

// ── SledStorage impl ─────────────────────────────────────────────────────────

impl SledStorage {
    /// Open (or create) a persistent graph database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, sled::Error> {
        let db = sled::open(path)?;
        let nodes = db.open_tree("nodes")?;
        let edges = db.open_tree("edges")?;
        let label_idx = db.open_tree("label_idx")?;
        let outgoing = db.open_tree("outgoing")?;
        let incoming = db.open_tree("incoming")?;
        let meta = db.open_tree("meta")?;
        let prop_idx = db.open_tree("prop_idx")?;
        let constraints = db.open_tree("constraints")?;
        Ok(Self {
            _db: db,
            nodes,
            edges,
            label_idx,
            outgoing,
            incoming,
            meta,
            prop_idx,
            constraints,
        })
    }

    fn next_node_id(&self) -> NodeId {
        self.meta
            .get(b"next_node_id")
            .ok()
            .flatten()
            .and_then(|v| read_u64(&v))
            .unwrap_or(0)
    }

    fn set_next_node_id(&self, id: NodeId) {
        self.meta
            .insert(b"next_node_id", &write_u64(id))
            .expect("meta insert");
    }

    fn next_edge_id(&self) -> EdgeId {
        self.meta
            .get(b"next_edge_id")
            .ok()
            .flatten()
            .and_then(|v| read_u64(&v))
            .unwrap_or(0)
    }

    fn set_next_edge_id(&self, id: EdgeId) {
        self.meta
            .insert(b"next_edge_id", &write_u64(id))
            .expect("meta insert");
    }

    fn load_adj(&self, tree: &sled::Tree, node_id: NodeId) -> Vec<EdgeId> {
        tree.get(u64_key(node_id))
            .ok()
            .flatten()
            .and_then(|v| decode::<Vec<EdgeId>>(&v))
            .unwrap_or_default()
    }

    fn save_adj(&self, tree: &sled::Tree, node_id: NodeId, ids: &[EdgeId]) {
        let ids_vec: Vec<EdgeId> = ids.to_vec();
        tree.insert(u64_key(node_id), encode(&ids_vec))
            .expect("adj insert");
    }

    fn append_adj(&self, tree: &sled::Tree, node_id: NodeId, edge_id: EdgeId) {
        let mut ids = self.load_adj(tree, node_id);
        ids.push(edge_id);
        self.save_adj(tree, node_id, &ids);
    }

    fn remove_from_adj(&self, tree: &sled::Tree, node_id: NodeId, edge_id: EdgeId) {
        let mut ids = self.load_adj(tree, node_id);
        ids.retain(|&e| e != edge_id);
        if ids.is_empty() {
            tree.remove(u64_key(node_id)).ok();
        } else {
            self.save_adj(tree, node_id, &ids);
        }
    }

    fn node_exists(&self, id: NodeId) -> bool {
        self.nodes.contains_key(u64_key(id)).unwrap_or(false)
    }
}

// ── StorageBackend ─────────────────────────────────────────────────────────────

impl StorageBackend for SledStorage {
    fn create_node(
        &mut self,
        labels: Vec<String>,
        properties: HashMap<String, PropertyValue>,
    ) -> NodeId {
        let id = self.next_node_id();
        for (key, val) in &properties {
            self.prop_idx
                .insert(prop_idx_key(key, val, id), b"")
                .expect("prop idx insert");
        }
        let node = Node {
            id,
            labels: labels.clone(),
            properties,
        };
        self.nodes
            .insert(u64_key(id), encode(&node))
            .expect("node insert");
        for label in &labels {
            self.label_idx
                .insert(label_key(label, id), b"")
                .expect("label idx insert");
        }
        self.set_next_node_id(id + 1);
        id
    }

    fn create_edge(
        &mut self,
        label: String,
        src: NodeId,
        dst: NodeId,
        properties: HashMap<String, PropertyValue>,
    ) -> Result<EdgeId, String> {
        if !self.node_exists(src) {
            return Err(format!("Source node {} not found", src));
        }
        if !self.node_exists(dst) {
            return Err(format!("Destination node {} not found", dst));
        }
        let id = self.next_edge_id();
        let edge = Edge {
            id,
            label,
            src,
            dst,
            properties,
        };
        self.edges
            .insert(u64_key(id), encode(&edge))
            .expect("edge insert");
        self.append_adj(&self.outgoing.clone(), src, id);
        self.append_adj(&self.incoming.clone(), dst, id);
        self.set_next_edge_id(id + 1);
        Ok(id)
    }

    fn get_node(&self, id: NodeId) -> Option<Node> {
        self.nodes.get(u64_key(id)).ok()?.and_then(|v| decode(&v))
    }

    fn get_edge(&self, id: EdgeId) -> Option<Edge> {
        self.edges.get(u64_key(id)).ok()?.and_then(|v| decode(&v))
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn edge_count(&self) -> usize {
        self.edges.len()
    }

    fn outgoing_edges(&self, node_id: NodeId) -> Vec<Edge> {
        self.outgoing_edge_ids(node_id)
            .into_iter()
            .filter_map(|id| self.get_edge(id))
            .collect()
    }

    fn incoming_edges(&self, node_id: NodeId) -> Vec<Edge> {
        self.incoming_edge_ids(node_id)
            .into_iter()
            .filter_map(|id| self.get_edge(id))
            .collect()
    }

    fn outgoing_edge_ids(&self, node_id: NodeId) -> Vec<EdgeId> {
        self.load_adj(&self.outgoing, node_id)
    }

    fn incoming_edge_ids(&self, node_id: NodeId) -> Vec<EdgeId> {
        self.load_adj(&self.incoming, node_id)
    }

    fn match_nodes(&self, label: Option<&str>) -> Vec<Node> {
        match label {
            None => self
                .nodes
                .iter()
                .filter_map(|r| r.ok())
                .filter_map(|(_, v)| decode(&v))
                .collect(),
            Some(l) => {
                let prefix = label_prefix(l);
                self.label_idx
                    .scan_prefix(&prefix)
                    .filter_map(|r| r.ok())
                    .filter_map(|(k, _)| {
                        let id_bytes = k.get(prefix.len()..)?;
                        if id_bytes.len() != 8 {
                            return None;
                        }
                        let id = u64::from_be_bytes(id_bytes.try_into().ok()?);
                        self.get_node(id)
                    })
                    .collect()
            }
        }
    }

    fn find_node(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Option<NodeId> {
        self.find_nodes(labels, properties).into_iter().next()
    }

    fn find_nodes(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Vec<NodeId> {
        let candidates: Vec<Node> = if let Some(first) = labels.first() {
            let prefix = label_prefix(first);
            self.label_idx
                .scan_prefix(&prefix)
                .filter_map(|r| r.ok())
                .filter_map(|(k, _)| {
                    let id_bytes = k.get(prefix.len()..)?;
                    if id_bytes.len() != 8 {
                        return None;
                    }
                    let id = u64::from_be_bytes(id_bytes.try_into().ok()?);
                    self.get_node(id)
                })
                .collect()
        } else {
            self.nodes
                .iter()
                .filter_map(|r| r.ok())
                .filter_map(|(_, v)| decode(&v))
                .collect()
        };

        candidates
            .into_iter()
            .filter(|node| {
                let labels_match = labels.iter().all(|l| node.labels.contains(l));
                if !labels_match {
                    return false;
                }
                properties.iter().all(|(key, val)| {
                    node.properties
                        .get(key)
                        .map(|v| property_values_equal(v, val))
                        .unwrap_or(false)
                })
            })
            .map(|n| n.id)
            .collect()
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .nodes
            .iter()
            .filter_map(|r| r.ok())
            .map(|(k, _)| u64::from_be_bytes(k.as_ref().try_into().unwrap_or([0u8; 8])))
            .collect();
        ids.sort();
        ids
    }

    fn all_edge_ids(&self) -> Vec<EdgeId> {
        let mut ids: Vec<EdgeId> = self
            .edges
            .iter()
            .filter_map(|r| r.ok())
            .map(|(k, _)| u64::from_be_bytes(k.as_ref().try_into().unwrap_or([0u8; 8])))
            .collect();
        ids.sort();
        ids
    }

    fn set_node_property(&mut self, id: NodeId, key: String, value: PropertyValue) {
        if let Some(mut node) = self.get_node(id) {
            if let Some(old_val) = node.properties.get(&key) {
                self.prop_idx.remove(prop_idx_key(&key, old_val, id)).ok();
            }
            self.prop_idx
                .insert(prop_idx_key(&key, &value, id), b"")
                .ok();
            node.properties.insert(key, value);
            self.nodes.insert(u64_key(id), encode(&node)).ok();
        }
    }

    fn set_edge_property(&mut self, id: EdgeId, key: String, value: PropertyValue) {
        if let Some(mut edge) = self.get_edge(id) {
            edge.properties.insert(key, value);
            self.edges.insert(u64_key(id), encode(&edge)).ok();
        }
    }

    fn set_node_all_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(mut node) = self.get_node(id) {
            for (key, val) in &node.properties {
                self.prop_idx.remove(prop_idx_key(key, val, id)).ok();
            }
            for (key, val) in &properties {
                self.prop_idx.insert(prop_idx_key(key, val, id), b"").ok();
            }
            node.properties = properties;
            self.nodes.insert(u64_key(id), encode(&node)).ok();
        }
    }

    fn set_edge_all_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(mut edge) = self.get_edge(id) {
            edge.properties = properties;
            self.edges.insert(u64_key(id), encode(&edge)).ok();
        }
    }

    fn merge_node_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(mut node) = self.get_node(id) {
            for (k, v) in properties {
                if let Some(old_val) = node.properties.get(&k) {
                    self.prop_idx.remove(prop_idx_key(&k, old_val, id)).ok();
                }
                self.prop_idx.insert(prop_idx_key(&k, &v, id), b"").ok();
                node.properties.insert(k, v);
            }
            self.nodes.insert(u64_key(id), encode(&node)).ok();
        }
    }

    fn merge_edge_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(mut edge) = self.get_edge(id) {
            for (k, v) in properties {
                edge.properties.insert(k, v);
            }
            self.edges.insert(u64_key(id), encode(&edge)).ok();
        }
    }

    fn add_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(mut node) = self.get_node(id) {
            for label in labels {
                if !node.labels.contains(label) {
                    node.labels.push(label.clone());
                    self.label_idx
                        .insert(label_key(label, id), b"")
                        .expect("label idx insert");
                }
            }
            self.nodes.insert(u64_key(id), encode(&node)).ok();
        }
    }

    fn remove_node_property(&mut self, id: NodeId, key: &str) {
        if let Some(mut node) = self.get_node(id) {
            if let Some(val) = node.properties.remove(key) {
                self.prop_idx.remove(prop_idx_key(key, &val, id)).ok();
            }
            self.nodes.insert(u64_key(id), encode(&node)).ok();
        }
    }

    fn remove_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(mut node) = self.get_node(id) {
            for label in labels {
                node.labels.retain(|l| l != label);
                self.label_idx.remove(label_key(label, id)).ok();
            }
            self.nodes.insert(u64_key(id), encode(&node)).ok();
        }
    }

    fn remove_edge_property(&mut self, id: EdgeId, key: &str) {
        if let Some(mut edge) = self.get_edge(id) {
            edge.properties.remove(key);
            self.edges.insert(u64_key(id), encode(&edge)).ok();
        }
    }

    fn delete_node(&mut self, id: NodeId, detach: bool) -> Result<(), String> {
        if !self.node_exists(id) {
            return Err(format!("Node {} not found", id));
        }

        let out_ids = self.outgoing_edge_ids(id);
        let inc_ids = self.incoming_edge_ids(id);

        let has_edges = out_ids.iter().any(|&eid| self.get_edge(eid).is_some())
            || inc_ids.iter().any(|&eid| self.get_edge(eid).is_some());

        if !detach && has_edges {
            return Err(format!(
                "Cannot delete node {} because it still has relationships. Use DETACH DELETE.",
                id
            ));
        }

        if detach {
            let mut all_eids: std::collections::HashSet<EdgeId> = std::collections::HashSet::new();
            all_eids.extend(out_ids.iter().copied());
            all_eids.extend(inc_ids.iter().copied());

            for eid in all_eids {
                if let Some(edge) = self.get_edge(eid) {
                    self.edges.remove(u64_key(eid)).ok();
                    if edge.src != id {
                        self.remove_from_adj(&self.outgoing.clone(), edge.src, eid);
                    }
                    if edge.dst != id {
                        self.remove_from_adj(&self.incoming.clone(), edge.dst, eid);
                    }
                }
            }
        }

        self.outgoing.remove(u64_key(id)).ok();
        self.incoming.remove(u64_key(id)).ok();

        if let Some(node) = self.get_node(id) {
            for label in &node.labels {
                self.label_idx.remove(label_key(label, id)).ok();
            }
            for (key, val) in &node.properties {
                self.prop_idx.remove(prop_idx_key(key, val, id)).ok();
            }
        }

        self.nodes.remove(u64_key(id)).ok();
        Ok(())
    }

    fn delete_edge(&mut self, id: EdgeId) -> Result<(), String> {
        if let Some(edge) = self.get_edge(id) {
            self.edges.remove(u64_key(id)).ok();
            self.remove_from_adj(&self.outgoing.clone(), edge.src, id);
            self.remove_from_adj(&self.incoming.clone(), edge.dst, id);
            Ok(())
        } else {
            Err(format!("Edge {} not found", id))
        }
    }

    fn match_nodes_with_props(
        &self,
        label: Option<&str>,
        props: &HashMap<String, PropertyValue>,
    ) -> Vec<Node> {
        if props.is_empty() {
            return self.match_nodes(label);
        }

        // Use the first property to get an initial candidate set via the index,
        // then intersect with remaining properties.
        let mut candidate_ids: Option<std::collections::HashSet<NodeId>> = None;
        for (key, val) in props {
            let prefix = prop_idx_prefix(key, val);
            let ids: std::collections::HashSet<NodeId> = self
                .prop_idx
                .scan_prefix(&prefix)
                .filter_map(|r| r.ok())
                .filter_map(|(k, _)| {
                    let id_bytes = k.get(k.len().saturating_sub(8)..)?;
                    if id_bytes.len() != 8 {
                        return None;
                    }
                    Some(u64::from_be_bytes(id_bytes.try_into().ok()?))
                })
                .collect();
            candidate_ids = Some(match candidate_ids {
                None => ids,
                Some(existing) => existing.intersection(&ids).copied().collect(),
            });
        }

        let candidate_ids = match candidate_ids {
            Some(ids) => ids,
            None => return Vec::new(),
        };

        candidate_ids
            .into_iter()
            .filter_map(|id| {
                let node = self.get_node(id)?;
                if let Some(l) = label
                    && !node.labels.contains(&l.to_string())
                {
                    return None;
                }
                Some(node)
            })
            .collect()
    }

    fn add_unique_constraint(&mut self, label: &str, property: &str) -> Result<(), String> {
        // Check existing nodes for violations.
        if let Some(node_ids) = {
            let prefix = label_prefix(label);
            let ids: Vec<NodeId> = self
                .label_idx
                .scan_prefix(&prefix)
                .filter_map(|r| r.ok())
                .filter_map(|(k, _)| {
                    let id_bytes = k.get(prefix.len()..)?;
                    if id_bytes.len() != 8 {
                        return None;
                    }
                    Some(u64::from_be_bytes(id_bytes.try_into().ok()?))
                })
                .collect();
            if ids.is_empty() { None } else { Some(ids) }
        } {
            let mut seen: HashMap<String, NodeId> = HashMap::new();
            for nid in node_ids {
                if let Some(node) = self.get_node(nid)
                    && let Some(val) = node.properties.get(property)
                {
                    let key = format!("{:?}", val);
                    if let Some(&existing) = seen.get(&key) {
                        return Err(format!(
                            "Unique constraint violation: nodes {} and {} both have {}:{} = {:?}",
                            existing, nid, label, property, val
                        ));
                    }
                    seen.insert(key, nid);
                }
            }
        }
        self.constraints
            .insert(constraint_key(label, property), b"")
            .expect("constraint insert");
        Ok(())
    }

    fn check_unique_constraint(
        &self,
        label: &str,
        property: &str,
        value: &PropertyValue,
    ) -> Result<(), String> {
        if self
            .constraints
            .contains_key(constraint_key(label, property))
            .unwrap_or(false)
        {
            // Find nodes that have this property value.
            let prefix = prop_idx_prefix(property, value);
            for (k, _) in self.prop_idx.scan_prefix(&prefix).flatten() {
                if let Some(id_bytes) = k.get(k.len().saturating_sub(8)..)
                    && id_bytes.len() == 8
                {
                    let nid = u64::from_be_bytes(id_bytes.try_into().unwrap());
                    if let Some(node) = self.get_node(nid)
                        && node.labels.contains(&label.to_string())
                    {
                        return Err(format!(
                            "Unique constraint violation on {}:{}: value {:?} already exists",
                            label, property, value
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn list_unique_constraints(&self) -> Vec<(String, String)> {
        self.constraints
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(k, _)| {
                // Skip non-unique constraint entries (they start with 0x01)
                if k.first() == Some(&0x01) {
                    return None;
                }
                let s = std::str::from_utf8(&k).ok()?;
                let mut parts = s.splitn(2, '\x00');
                let label = parts.next()?.to_string();
                let property = parts.next()?.to_string();
                Some((label, property))
            })
            .collect()
    }

    fn add_not_null_constraint(&mut self, label: &str, property: &str) -> Result<(), String> {
        // Check existing nodes for violations.
        let prefix = label_prefix(label);
        let node_ids: Vec<NodeId> = self
            .label_idx
            .scan_prefix(&prefix)
            .filter_map(|r| r.ok())
            .filter_map(|(k, _)| {
                let id_bytes = k.get(prefix.len()..)?;
                if id_bytes.len() != 8 {
                    return None;
                }
                Some(u64::from_be_bytes(id_bytes.try_into().ok()?))
            })
            .collect();
        for nid in node_ids {
            if let Some(node) = self.get_node(nid) {
                if !node.properties.contains_key(property) {
                    return Err(format!(
                        "NOT NULL constraint violation: node {} with label {} is missing property {}",
                        nid, label, property
                    ));
                }
            }
        }
        // Key format: 0x01 NN 0x00 {label} 0x00 {property}
        let mut key = vec![0x01, b'N', b'N', 0x00];
        key.extend_from_slice(label.as_bytes());
        key.push(0x00);
        key.extend_from_slice(property.as_bytes());
        self.constraints
            .insert(key, b"")
            .expect("constraint insert");
        Ok(())
    }

    fn check_not_null_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String> {
        for label in labels {
            // Scan for NOT NULL constraints for this label
            let mut prefix = vec![0x01, b'N', b'N', 0x00];
            prefix.extend_from_slice(label.as_bytes());
            prefix.push(0x00);
            for entry in self.constraints.scan_prefix(&prefix).flatten() {
                let (k, _) = entry;
                if let Some(prop_bytes) = k.get(prefix.len()..) {
                    if let Ok(prop) = std::str::from_utf8(prop_bytes) {
                        if !properties.contains_key(prop) {
                            return Err(format!(
                                "NOT NULL constraint violation on {}:{}: property is required",
                                label, prop
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn add_node_key_constraint(
        &mut self,
        label: &str,
        properties: &[String],
    ) -> Result<(), String> {
        // Check existing nodes for violations.
        let prefix = label_prefix(label);
        let node_ids: Vec<NodeId> = self
            .label_idx
            .scan_prefix(&prefix)
            .filter_map(|r| r.ok())
            .filter_map(|(k, _)| {
                let id_bytes = k.get(prefix.len()..)?;
                if id_bytes.len() != 8 {
                    return None;
                }
                Some(u64::from_be_bytes(id_bytes.try_into().ok()?))
            })
            .collect();
        let mut seen: HashMap<Vec<String>, NodeId> = HashMap::new();
        for nid in node_ids {
            if let Some(node) = self.get_node(nid) {
                for prop in properties {
                    if !node.properties.contains_key(prop) {
                        return Err(format!(
                            "NODE KEY constraint violation: node {} with label {} is missing property {}",
                            nid, label, prop
                        ));
                    }
                }
                let key: Vec<String> = properties
                    .iter()
                    .map(|p| format!("{:?}", node.properties.get(p)))
                    .collect();
                if let Some(&existing) = seen.get(&key) {
                    return Err(format!(
                        "NODE KEY constraint violation: nodes {} and {} both have the same key for label {}",
                        existing, nid, label
                    ));
                }
                seen.insert(key, nid);
            }
        }
        // Key format: 0x01 NK 0x00 {label} 0x00 {prop1,prop2,...}
        let mut key = vec![0x01, b'N', b'K', 0x00];
        key.extend_from_slice(label.as_bytes());
        key.push(0x00);
        key.extend_from_slice(properties.join(",").as_bytes());
        self.constraints
            .insert(key, b"")
            .expect("constraint insert");
        Ok(())
    }

    fn check_node_key_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String> {
        for label in labels {
            let mut prefix = vec![0x01, b'N', b'K', 0x00];
            prefix.extend_from_slice(label.as_bytes());
            prefix.push(0x00);
            for entry in self.constraints.scan_prefix(&prefix).flatten() {
                let (k, _) = entry;
                if let Some(props_bytes) = k.get(prefix.len()..) {
                    if let Ok(props_str) = std::str::from_utf8(props_bytes) {
                        let key_props: Vec<&str> = props_str.split(',').collect();
                        // All key properties must be present
                        for prop in &key_props {
                            if !properties.contains_key(*prop) {
                                return Err(format!(
                                    "NODE KEY constraint violation on {}:{}: property is required",
                                    label, prop
                                ));
                            }
                        }
                        // Tuple must be unique among existing nodes
                        let new_key: Vec<String> = key_props
                            .iter()
                            .map(|p| format!("{:?}", properties.get(*p)))
                            .collect();
                        let label_prefix_bytes = label_prefix(label);
                        let node_ids: Vec<NodeId> = self
                            .label_idx
                            .scan_prefix(&label_prefix_bytes)
                            .filter_map(|r| r.ok())
                            .filter_map(|(nk, _)| {
                                let id_bytes = nk.get(label_prefix_bytes.len()..)?;
                                if id_bytes.len() != 8 {
                                    return None;
                                }
                                Some(u64::from_be_bytes(id_bytes.try_into().ok()?))
                            })
                            .collect();
                        for nid in node_ids {
                            if let Some(node) = self.get_node(nid) {
                                let existing_key: Vec<String> = key_props
                                    .iter()
                                    .map(|p| format!("{:?}", node.properties.get(*p)))
                                    .collect();
                                if new_key == existing_key {
                                    return Err(format!(
                                        "NODE KEY constraint violation on {}: duplicate key {:?}",
                                        label, new_key
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn add_property_type_constraint(
        &mut self,
        label: &str,
        property: &str,
        type_name: &str,
    ) -> Result<(), String> {
        // Check existing nodes for type violations.
        let prefix = label_prefix(label);
        let node_ids: Vec<NodeId> = self
            .label_idx
            .scan_prefix(&prefix)
            .filter_map(|r| r.ok())
            .filter_map(|(k, _)| {
                let id_bytes = k.get(prefix.len()..)?;
                if id_bytes.len() != 8 {
                    return None;
                }
                Some(u64::from_be_bytes(id_bytes.try_into().ok()?))
            })
            .collect();
        for nid in node_ids {
            if let Some(node) = self.get_node(nid) {
                if let Some(val) = node.properties.get(property) {
                    if !property_value_matches_type(val, type_name) {
                        return Err(format!(
                            "PROPERTY TYPE constraint violation: node {} property {}:{} has wrong type (expected {})",
                            nid, label, property, type_name
                        ));
                    }
                }
            }
        }
        // Key format: 0x01 PT 0x00 {label} 0x00 {property} 0x00 {type_name}
        let mut key = vec![0x01, b'P', b'T', 0x00];
        key.extend_from_slice(label.as_bytes());
        key.push(0x00);
        key.extend_from_slice(property.as_bytes());
        key.push(0x00);
        key.extend_from_slice(type_name.as_bytes());
        self.constraints
            .insert(key, b"")
            .expect("constraint insert");
        Ok(())
    }

    fn check_property_type_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String> {
        for label in labels {
            let mut prefix = vec![0x01, b'P', b'T', 0x00];
            prefix.extend_from_slice(label.as_bytes());
            prefix.push(0x00);
            for entry in self.constraints.scan_prefix(&prefix).flatten() {
                let (k, _) = entry;
                if let Some(rest) = k.get(prefix.len()..) {
                    if let Ok(rest_str) = std::str::from_utf8(rest) {
                        let mut parts = rest_str.splitn(2, '\x00');
                        if let (Some(prop), Some(type_name)) = (parts.next(), parts.next()) {
                            if let Some(val) = properties.get(prop) {
                                if !property_value_matches_type(val, type_name) {
                                    return Err(format!(
                                        "PROPERTY TYPE constraint violation on {}:{}: expected type {}",
                                        label, prop, type_name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn property_values_equal(a: &PropertyValue, b: &PropertyValue) -> bool {
    match (a, b) {
        (PropertyValue::String(a), PropertyValue::String(b)) => a == b,
        (PropertyValue::Int(a), PropertyValue::Int(b)) => a == b,
        (PropertyValue::Float(a), PropertyValue::Float(b)) => a == b,
        (PropertyValue::Bool(a), PropertyValue::Bool(b)) => a == b,
        _ => false,
    }
}

fn property_value_matches_type(val: &PropertyValue, type_name: &str) -> bool {
    match type_name {
        "BOOLEAN" => matches!(val, PropertyValue::Bool(_)),
        "STRING" => matches!(val, PropertyValue::String(_)),
        "INTEGER" => matches!(val, PropertyValue::Int(_)),
        "FLOAT" => matches!(val, PropertyValue::Float(_)),
        _ => false,
    }
}
