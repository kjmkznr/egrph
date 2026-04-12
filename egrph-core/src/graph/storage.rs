use super::backend::StorageBackend;
use super::types::{Edge, EdgeId, Node, NodeId, PropertyValue};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct MemoryStorage {
    pub(crate) nodes: HashMap<NodeId, Node>,
    pub(crate) edges: HashMap<EdgeId, Edge>,
    pub(crate) outgoing: HashMap<NodeId, Vec<EdgeId>>,
    pub(crate) incoming: HashMap<NodeId, Vec<EdgeId>>,
    /// Label index: label -> set of NodeIds that have this label.
    label_index: HashMap<String, HashSet<NodeId>>,
    next_node_id: NodeId,
    next_edge_id: EdgeId,
    /// Unique constraints: label -> set of property names.
    unique_constraints: HashMap<String, HashSet<String>>,
}

/// Backward-compatible alias used within this crate.
pub type GraphStorage = MemoryStorage;

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
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
        let node = Node {
            id,
            labels: labels.clone(),
            properties,
        };
        self.nodes.insert(id, node);
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
        let edge = Edge {
            id,
            label,
            src,
            dst,
            properties,
        };
        self.edges.insert(id, edge);
        self.outgoing.entry(src).or_default().push(id);
        self.incoming.entry(dst).or_default().push(id);
        self.next_edge_id += 1;
        Ok(id)
    }

    fn get_node(&self, id: NodeId) -> Option<Node> {
        self.nodes.get(&id).cloned()
    }

    fn get_edge(&self, id: EdgeId) -> Option<Edge> {
        self.edges.get(&id).cloned()
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn edge_count(&self) -> usize {
        self.edges.len()
    }

    fn outgoing_edges(&self, node_id: NodeId) -> Vec<Edge> {
        self.outgoing
            .get(&node_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.edges.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn incoming_edges(&self, node_id: NodeId) -> Vec<Edge> {
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

    fn match_nodes(&self, label: Option<&str>) -> Vec<Node> {
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
        let candidates: Vec<NodeId> = if let Some(first_label) = labels.first() {
            match self.label_index.get(first_label) {
                Some(ids) => ids.iter().copied().collect(),
                None => return Vec::new(),
            }
        } else {
            self.nodes.keys().copied().collect()
        };

        candidates
            .into_iter()
            .filter(|id| {
                let node = match self.nodes.get(id) {
                    Some(n) => n,
                    None => return false,
                };
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
            .collect()
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
        if let Some(node) = self.nodes.get_mut(&id) {
            node.properties.insert(key, value);
        }
    }

    fn set_edge_property(&mut self, id: EdgeId, key: String, value: PropertyValue) {
        if let Some(edge) = self.edges.get_mut(&id) {
            edge.properties.insert(key, value);
        }
    }

    fn set_node_all_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.properties = properties;
        }
    }

    fn set_edge_all_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(edge) = self.edges.get_mut(&id) {
            edge.properties = properties;
        }
    }

    fn merge_node_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            for (k, v) in properties {
                node.properties.insert(k, v);
            }
        }
    }

    fn merge_edge_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(edge) = self.edges.get_mut(&id) {
            for (k, v) in properties {
                edge.properties.insert(k, v);
            }
        }
    }

    fn add_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(node) = self.nodes.get_mut(&id) {
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
        if let Some(node) = self.nodes.get_mut(&id) {
            node.properties.remove(key);
        }
    }

    fn remove_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(node) = self.nodes.get_mut(&id) {
            for label in labels {
                node.labels.retain(|l| l != label);
                if let Some(set) = self.label_index.get_mut(label) {
                    set.remove(&id);
                }
            }
        }
    }

    fn remove_edge_property(&mut self, id: EdgeId, key: &str) {
        if let Some(edge) = self.edges.get_mut(&id) {
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
                }
            }
        }

        self.outgoing.remove(&id);
        self.incoming.remove(&id);

        let labels: Vec<String> = self
            .nodes
            .get(&id)
            .map(|n| n.labels.clone())
            .unwrap_or_default();
        for label in &labels {
            if let Some(set) = self.label_index.get_mut(label) {
                set.remove(&id);
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
            && let Some(node_ids) = self.label_index.get(label)
        {
            for &nid in node_ids {
                if let Some(node) = self.nodes.get(&nid)
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

    fn delete_edge(&mut self, id: EdgeId) -> Result<(), String> {
        if let Some(edge) = self.edges.remove(&id) {
            if let Some(out) = self.outgoing.get_mut(&edge.src) {
                out.retain(|e| *e != id);
            }
            if let Some(inc) = self.incoming.get_mut(&edge.dst) {
                inc.retain(|e| *e != id);
            }
            Ok(())
        } else {
            Err(format!("Edge {} not found", id))
        }
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
