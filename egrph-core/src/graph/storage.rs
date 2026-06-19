use super::backend::StorageBackend;
use super::types::{Edge, EdgeId, Node, NodeId, PropertyValue};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Default)]
pub struct MemoryStorage {
    pub(crate) nodes: HashMap<NodeId, Arc<Node>>,
    pub(crate) edges: HashMap<EdgeId, Arc<Edge>>,
    pub(crate) outgoing: HashMap<NodeId, Vec<EdgeId>>,
    pub(crate) incoming: HashMap<NodeId, Vec<EdgeId>>,
    /// Label index: label -> set of NodeIds that have this label.
    label_index: HashMap<String, HashSet<NodeId>>,
    next_node_id: NodeId,
    next_edge_id: EdgeId,
    /// Unique constraints: label -> set of property names.
    unique_constraints: HashMap<String, HashSet<String>>,
    /// NOT NULL constraints: label -> set of property names that must be non-null.
    not_null_constraints: HashMap<String, HashSet<String>>,
    /// NODE KEY constraints: label -> list of property-key-tuples (each tuple must be unique and non-null).
    node_key_constraints: HashMap<String, Vec<Vec<String>>>,
    /// PROPERTY TYPE constraints: label -> property name -> type name ("BOOLEAN"|"STRING"|"INTEGER"|"FLOAT").
    property_type_constraints: HashMap<String, HashMap<String, String>>,
    /// Property index: prop_name -> prop_key -> set of NodeIds.
    /// Enables O(1) property lookup instead of O(N) full scan.
    property_index: HashMap<String, HashMap<PropKey, HashSet<NodeId>>>,
    /// Edge adjacency index: (src, label, dst) -> edge ids. Enables O(1)
    /// relationship lookup by endpoints+type (e.g. MERGE of an edge between two
    /// already-bound nodes) instead of scanning the source's whole adjacency.
    edge_adjacency: HashMap<(NodeId, String, NodeId), Vec<EdgeId>>,
}

/// Zero-copy key for a `PropertyValue`, used as the inner key of the
/// property index.  Each variant is distinct so there is no collision
/// between e.g. `Int(1)` and `String("1")`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PropKey {
    Str(String),
    Int(i64),
    FloatBits(u64), // f64::to_bits() for Hash+Eq
    Bool(bool),
}

fn prop_key(val: &PropertyValue) -> PropKey {
    match val {
        PropertyValue::String(s) => PropKey::Str(s.clone()),
        PropertyValue::Int(i) => PropKey::Int(*i),
        PropertyValue::Float(f) => PropKey::FloatBits(f.to_bits()),
        PropertyValue::Bool(b) => PropKey::Bool(*b),
    }
}

/// Backward-compatible alias used within this crate.
pub type GraphStorage = MemoryStorage;

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop one edge from the (src, label, dst) adjacency index.
    fn remove_edge_adjacency(&mut self, edge: &Edge) {
        let key = (edge.src, edge.label.clone(), edge.dst);
        if let Some(ids) = self.edge_adjacency.get_mut(&key) {
            ids.retain(|e| *e != edge.id);
            if ids.is_empty() {
                self.edge_adjacency.remove(&key);
            }
        }
    }
}

impl StorageBackend for MemoryStorage {
    fn create_node(
        &mut self,
        labels: Vec<String>,
        properties: HashMap<String, PropertyValue>,
    ) -> NodeId {
        // Unique constraint check is done via check_unique_constraint before calling this.
        let id = self.next_node_id;
        for (key, val) in &properties {
            let val_map = match self.property_index.get_mut(key.as_str()) {
                Some(m) => m,
                None => self.property_index.entry(key.clone()).or_default(),
            };
            val_map.entry(prop_key(val)).or_default().insert(id);
        }
        let node = Node {
            id,
            labels: labels.clone(),
            properties,
        };
        self.nodes.insert(id, Arc::new(node));
        for label in &labels {
            self.label_index
                .entry(label.clone())
                .or_default()
                .insert(id);
        }
        self.next_node_id += 1;
        id
    }

    fn create_edge(
        &mut self,
        label: String,
        src: NodeId,
        dst: NodeId,
        properties: HashMap<String, PropertyValue>,
    ) -> Result<EdgeId, String> {
        if !self.nodes.contains_key(&src) {
            return Err(format!("Source node {} not found", src));
        }
        if !self.nodes.contains_key(&dst) {
            return Err(format!("Destination node {} not found", dst));
        }

        let id = self.next_edge_id;
        self.edge_adjacency
            .entry((src, label.clone(), dst))
            .or_default()
            .push(id);
        let edge = Edge {
            id,
            label,
            src,
            dst,
            properties,
        };
        self.edges.insert(id, Arc::new(edge));
        self.outgoing.entry(src).or_default().push(id);
        self.incoming.entry(dst).or_default().push(id);
        self.next_edge_id += 1;
        Ok(id)
    }

    fn get_node(&self, id: NodeId) -> Option<Arc<Node>> {
        self.nodes.get(&id).cloned()
    }

    fn get_edge(&self, id: EdgeId) -> Option<Arc<Edge>> {
        self.edges.get(&id).cloned()
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn edge_count(&self) -> usize {
        self.edges.len()
    }

    fn outgoing_edges(&self, node_id: NodeId) -> Vec<Arc<Edge>> {
        self.outgoing
            .get(&node_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.edges.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn incoming_edges(&self, node_id: NodeId) -> Vec<Arc<Edge>> {
        self.incoming
            .get(&node_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.edges.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn outgoing_edge_ids(&self, node_id: NodeId) -> Vec<EdgeId> {
        self.outgoing.get(&node_id).cloned().unwrap_or_default()
    }

    fn incoming_edge_ids(&self, node_id: NodeId) -> Vec<EdgeId> {
        self.incoming.get(&node_id).cloned().unwrap_or_default()
    }

    fn match_nodes(&self, label: Option<&str>) -> Vec<Arc<Node>> {
        match label {
            None => self.nodes.values().cloned().collect(),
            Some(l) => self
                .label_index
                .get(l)
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| self.nodes.get(id).cloned())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn match_nodes_with_props(
        &self,
        label: Option<&str>,
        props: &HashMap<String, PropertyValue>,
    ) -> Vec<Arc<Node>> {
        if props.is_empty() {
            return self.match_nodes(label);
        }

        // Build the candidate set as the intersection of all per-property index
        // results.  This is typically a single-element set for unique properties.
        let mut candidates: Option<HashSet<NodeId>> = None;
        for (key, val) in props {
            let vkey = prop_key(val);
            let ids: HashSet<NodeId> = self
                .property_index
                .get(key)
                .and_then(|val_map| val_map.get(&vkey))
                .cloned()
                .unwrap_or_default();
            candidates = Some(match candidates {
                None => ids,
                Some(existing) => existing.intersection(&ids).copied().collect(),
            });
        }

        let candidates = match candidates {
            Some(ids) => ids,
            None => return Vec::new(),
        };

        candidates
            .into_iter()
            .filter_map(|id| {
                let node = self.nodes.get(&id)?;
                if let Some(l) = label
                    && !node.labels.contains(&l.to_string())
                {
                    return None;
                }
                Some(node.clone())
            })
            .collect()
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
        // Seed from the SMALLEST available index set, then filter exactly.
        // Each property value's posting list and the first label's id-set are
        // candidate seeds; we materialize whichever is smallest (HashSet::len
        // is O(1)) and scan only that. This makes MERGE-by-key O(1) for unique
        // keys AND stays O(label-set) when a property value is shared across
        // many nodes of other labels — e.g. `MERGE (ip:IpAddress {ip})` where
        // every LogEvent also carries that `ip`: the IpAddress label-set is the
        // small seed, not the huge `ip` posting list. Any absent label or
        // value means no match.
        let mut best: Option<&HashSet<NodeId>> = None;
        if let Some(first_label) = labels.first() {
            match self.label_index.get(first_label) {
                Some(ids) => best = Some(ids),
                None => return Vec::new(),
            }
        }
        for (key, val) in properties {
            let vkey = prop_key(val);
            match self
                .property_index
                .get(key)
                .and_then(|val_map| val_map.get(&vkey))
            {
                Some(ids) => {
                    if best.is_none_or(|b| ids.len() < b.len()) {
                        best = Some(ids);
                    }
                }
                None => return Vec::new(),
            }
        }
        // Re-filter exactly: the index is keyed by `prop_value_key`, so confirm
        // every label and property with the precise comparison. The candidate
        // set is tiny (typically one node for a unique key), so this is cheap.
        let filter = |id: &NodeId| -> bool {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => return false,
            };
            if !labels.iter().all(|l| node.labels.contains(l)) {
                return false;
            }
            properties.iter().all(|(key, val)| {
                node.properties
                    .get(key)
                    .map(|v| property_values_equal(v, val))
                    .unwrap_or(false)
            })
        };
        match best {
            Some(ids) => ids.iter().copied().filter(filter).collect(),
            // No labels and no properties: every node is a candidate.
            None => self.nodes.keys().copied().filter(filter).collect(),
        }
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        ids.sort();
        ids
    }

    fn all_edge_ids(&self) -> Vec<EdgeId> {
        let mut ids: Vec<EdgeId> = self.edges.keys().copied().collect();
        ids.sort();
        ids
    }

    fn set_node_property(&mut self, id: NodeId, key: String, value: PropertyValue) {
        if let Some(node) = self.nodes.get_mut(&id).map(Arc::make_mut) {
            // Remove old index entry.
            if let Some(old_val) = node.properties.get(&key) {
                let old_vkey = prop_key(old_val);
                if let Some(val_map) = self.property_index.get_mut(&key)
                    && let Some(id_set) = val_map.get_mut(&old_vkey)
                {
                    id_set.remove(&id);
                }
            }
            node.properties.insert(key.clone(), value.clone());
            // Add new index entry.
            self.property_index
                .entry(key)
                .or_default()
                .entry(prop_key(&value))
                .or_default()
                .insert(id);
        }
    }

    fn set_edge_property(&mut self, id: EdgeId, key: String, value: PropertyValue) {
        if let Some(edge) = self.edges.get_mut(&id).map(Arc::make_mut) {
            edge.properties.insert(key, value);
        }
    }

    fn set_node_all_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(node) = self.nodes.get_mut(&id).map(Arc::make_mut) {
            // Remove old index entries.
            for (key, val) in &node.properties {
                let old_vkey = prop_key(val);
                if let Some(val_map) = self.property_index.get_mut(key)
                    && let Some(id_set) = val_map.get_mut(&old_vkey)
                {
                    id_set.remove(&id);
                }
            }
            // Add new index entries.
            for (key, val) in &properties {
                self.property_index
                    .entry(key.clone())
                    .or_default()
                    .entry(prop_key(val))
                    .or_default()
                    .insert(id);
            }
            node.properties = properties;
        }
    }

    fn set_edge_all_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(edge) = self.edges.get_mut(&id).map(Arc::make_mut) {
            edge.properties = properties;
        }
    }

    fn merge_node_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(node) = self.nodes.get_mut(&id).map(Arc::make_mut) {
            for (k, v) in properties {
                // Remove old index entry for this key if it exists.
                if let Some(old_val) = node.properties.get(&k) {
                    let old_vkey = prop_key(old_val);
                    if let Some(val_map) = self.property_index.get_mut(&k)
                        && let Some(id_set) = val_map.get_mut(&old_vkey)
                    {
                        id_set.remove(&id);
                    }
                }
                // Add new index entry.
                self.property_index
                    .entry(k.clone())
                    .or_default()
                    .entry(prop_key(&v))
                    .or_default()
                    .insert(id);
                node.properties.insert(k, v);
            }
        }
    }

    fn merge_edge_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(edge) = self.edges.get_mut(&id).map(Arc::make_mut) {
            for (k, v) in properties {
                edge.properties.insert(k, v);
            }
        }
    }

    fn add_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(node) = self.nodes.get_mut(&id).map(Arc::make_mut) {
            for label in labels {
                if !node.labels.contains(label) {
                    node.labels.push(label.clone());
                    self.label_index
                        .entry(label.clone())
                        .or_default()
                        .insert(id);
                }
            }
        }
    }

    fn remove_node_property(&mut self, id: NodeId, key: &str) {
        if let Some(node) = self.nodes.get_mut(&id).map(Arc::make_mut)
            && let Some(val) = node.properties.remove(key)
        {
            let vkey = prop_key(&val);
            if let Some(val_map) = self.property_index.get_mut(key)
                && let Some(id_set) = val_map.get_mut(&vkey)
            {
                id_set.remove(&id);
            }
        }
    }

    fn remove_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(node) = self.nodes.get_mut(&id).map(Arc::make_mut) {
            for label in labels {
                node.labels.retain(|l| l != label);
                if let Some(set) = self.label_index.get_mut(label) {
                    set.remove(&id);
                }
            }
        }
    }

    fn remove_edge_property(&mut self, id: EdgeId, key: &str) {
        if let Some(edge) = self.edges.get_mut(&id).map(Arc::make_mut) {
            edge.properties.remove(key);
        }
    }

    fn delete_node(&mut self, id: NodeId, detach: bool) -> Result<(), String> {
        if !self.nodes.contains_key(&id) {
            return Err(format!("Node {} not found", id));
        }

        let has_outgoing = self
            .outgoing
            .get(&id)
            .map(|v| v.iter().any(|eid| self.edges.contains_key(eid)))
            .unwrap_or(false);
        let has_incoming = self
            .incoming
            .get(&id)
            .map(|v| v.iter().any(|eid| self.edges.contains_key(eid)))
            .unwrap_or(false);

        if !detach && (has_outgoing || has_incoming) {
            return Err(format!(
                "Cannot delete node {} because it still has relationships. Use DETACH DELETE.",
                id
            ));
        }

        if detach {
            let mut edge_ids: HashSet<EdgeId> = HashSet::new();
            if let Some(out) = self.outgoing.get(&id) {
                edge_ids.extend(out.iter().copied());
            }
            if let Some(inc) = self.incoming.get(&id) {
                edge_ids.extend(inc.iter().copied());
            }

            for eid in edge_ids {
                if let Some(edge) = self.edges.remove(&eid) {
                    if edge.src != id
                        && let Some(out) = self.outgoing.get_mut(&edge.src)
                    {
                        out.retain(|e| *e != eid);
                    }
                    if edge.dst != id
                        && let Some(inc) = self.incoming.get_mut(&edge.dst)
                    {
                        inc.retain(|e| *e != eid);
                    }
                    self.remove_edge_adjacency(&edge);
                }
            }
        }

        self.outgoing.remove(&id);
        self.incoming.remove(&id);

        if let Some(node) = self.nodes.get(&id) {
            for label in &node.labels {
                if let Some(set) = self.label_index.get_mut(label) {
                    set.remove(&id);
                }
            }
            for (key, val) in &node.properties {
                let vkey = prop_key(val);
                if let Some(val_map) = self.property_index.get_mut(key)
                    && let Some(id_set) = val_map.get_mut(&vkey)
                {
                    id_set.remove(&id);
                }
            }
        }

        self.nodes.remove(&id);
        Ok(())
    }

    fn add_unique_constraint(&mut self, label: &str, property: &str) -> Result<(), String> {
        // Check existing nodes for violations.
        if let Some(node_ids) = self.label_index.get(label) {
            let mut seen: HashMap<String, NodeId> = HashMap::new();
            for &nid in node_ids {
                if let Some(node) = self.nodes.get(&nid)
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
        self.unique_constraints
            .entry(label.to_string())
            .or_default()
            .insert(property.to_string());
        Ok(())
    }

    fn check_unique_constraint(
        &self,
        label: &str,
        property: &str,
        value: &PropertyValue,
    ) -> Result<(), String> {
        if let Some(props) = self.unique_constraints.get(label)
            && props.contains(property)
        {
            // Probe the property index for this exact value instead of scanning
            // the whole label set, then confirm the label and value precisely.
            let vkey = prop_key(value);
            if let Some(node_ids) = self
                .property_index
                .get(property)
                .and_then(|val_map| val_map.get(&vkey))
            {
                for &nid in node_ids {
                    if let Some(node) = self.nodes.get(&nid)
                        && node.labels.iter().any(|l| l == label)
                        && let Some(existing_val) = node.properties.get(property)
                        && property_values_equal(existing_val, value)
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
        let mut result = Vec::new();
        for (label, props) in &self.unique_constraints {
            for prop in props {
                result.push((label.clone(), prop.clone()));
            }
        }
        result
    }

    fn add_not_null_constraint(&mut self, label: &str, property: &str) -> Result<(), String> {
        // Check existing nodes for violations.
        if let Some(node_ids) = self.label_index.get(label) {
            for &nid in node_ids {
                if let Some(node) = self.nodes.get(&nid)
                    && !node.properties.contains_key(property)
                {
                    return Err(format!(
                        "NOT NULL constraint violation: node {} with label {} is missing property {}",
                        nid, label, property
                    ));
                }
            }
        }
        self.not_null_constraints
            .entry(label.to_string())
            .or_default()
            .insert(property.to_string());
        Ok(())
    }

    fn check_not_null_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String> {
        for label in labels {
            if let Some(required_props) = self.not_null_constraints.get(label) {
                for prop in required_props {
                    if !properties.contains_key(prop) {
                        return Err(format!(
                            "NOT NULL constraint violation on {}:{}: property is required",
                            label, prop
                        ));
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
        // Check existing nodes for violations: all props must be present and tuple must be unique.
        if let Some(node_ids) = self.label_index.get(label) {
            let node_ids: Vec<NodeId> = node_ids.iter().copied().collect();
            let mut seen: HashMap<Vec<String>, NodeId> = HashMap::new();
            for nid in node_ids {
                if let Some(node) = self.nodes.get(&nid) {
                    // Check all properties are non-null
                    for prop in properties {
                        if !node.properties.contains_key(prop) {
                            return Err(format!(
                                "NODE KEY constraint violation: node {} with label {} is missing property {}",
                                nid, label, prop
                            ));
                        }
                    }
                    // Check tuple uniqueness
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
        }
        self.node_key_constraints
            .entry(label.to_string())
            .or_default()
            .push(properties.to_vec());
        Ok(())
    }

    fn check_node_key_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String> {
        for label in labels {
            if let Some(key_tuples) = self.node_key_constraints.get(label) {
                for key_props in key_tuples {
                    // All key properties must be present (non-null)
                    for prop in key_props {
                        if !properties.contains_key(prop) {
                            return Err(format!(
                                "NODE KEY constraint violation on {}:{}: property is required",
                                label, prop
                            ));
                        }
                    }
                    // Tuple must be unique
                    if let Some(node_ids) = self.label_index.get(label) {
                        let new_key: Vec<String> = key_props
                            .iter()
                            .map(|p| format!("{:?}", properties.get(p)))
                            .collect();
                        for &nid in node_ids {
                            if let Some(node) = self.nodes.get(&nid) {
                                let existing_key: Vec<String> = key_props
                                    .iter()
                                    .map(|p| format!("{:?}", node.properties.get(p)))
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
        // Check existing nodes for type violations (only if property is present).
        if let Some(node_ids) = self.label_index.get(label) {
            for &nid in node_ids {
                if let Some(node) = self.nodes.get(&nid)
                    && let Some(val) = node.properties.get(property)
                    && !property_value_matches_type(val, type_name)
                {
                    return Err(format!(
                        "PROPERTY TYPE constraint violation: node {} property {}:{} has wrong type (expected {})",
                        nid, label, property, type_name
                    ));
                }
            }
        }
        self.property_type_constraints
            .entry(label.to_string())
            .or_default()
            .insert(property.to_string(), type_name.to_string());
        Ok(())
    }

    fn check_property_type_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String> {
        for label in labels {
            if let Some(type_map) = self.property_type_constraints.get(label) {
                for (prop, type_name) in type_map {
                    if let Some(val) = properties.get(prop)
                        && !property_value_matches_type(val, type_name)
                    {
                        return Err(format!(
                            "PROPERTY TYPE constraint violation on {}:{}: expected type {}",
                            label, prop, type_name
                        ));
                    }
                    // If property is absent, that's OK for PROPERTY TYPE constraints.
                }
            }
        }
        Ok(())
    }

    fn delete_edge(&mut self, id: EdgeId) -> Result<(), String> {
        if let Some(edge) = self.edges.remove(&id) {
            if let Some(out) = self.outgoing.get_mut(&edge.src) {
                out.retain(|e| *e != id);
            }
            if let Some(inc) = self.incoming.get_mut(&edge.dst) {
                inc.retain(|e| *e != id);
            }
            self.remove_edge_adjacency(&edge);
            Ok(())
        } else {
            Err(format!("Edge {} not found", id))
        }
    }

    fn edges_between(&self, src: NodeId, rel_type: &str, dst: NodeId) -> Vec<Arc<Edge>> {
        self.edge_adjacency
            .get(&(src, rel_type.to_string(), dst))
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.edges.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
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
