use std::collections::{HashMap, HashSet};
use super::types::{NodeId, EdgeId, Node, Edge, PropertyValue};

#[derive(Default)]
pub struct GraphStorage {
    pub(crate) nodes: HashMap<NodeId, Node>,
    pub(crate) edges: HashMap<EdgeId, Edge>,
    pub(crate) outgoing: HashMap<NodeId, Vec<EdgeId>>,
    pub(crate) incoming: HashMap<NodeId, Vec<EdgeId>>,
    /// Label index: label -> set of NodeIds that have this label.
    label_index: HashMap<String, HashSet<NodeId>>,
    next_node_id: NodeId,
    next_edge_id: EdgeId,
}

impl GraphStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_node(&mut self, labels: Vec<String>, properties: HashMap<String, PropertyValue>) -> NodeId {
        let id = self.next_node_id;
        let node = Node {
            id,
            labels: labels.clone(),
            properties,
        };
        self.nodes.insert(id, node);
        for label in &labels {
            self.label_index.entry(label.clone()).or_default().insert(id);
        }
        self.next_node_id += 1;
        id
    }

    pub fn create_edge(
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

    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(&id)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn outgoing_edges(&self, node_id: NodeId) -> Vec<&Edge> {
        self.outgoing
            .get(&node_id)
            .map(|ids| ids.iter().filter_map(|id| self.edges.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn incoming_edges(&self, node_id: NodeId) -> Vec<&Edge> {
        self.incoming
            .get(&node_id)
            .map(|ids| ids.iter().filter_map(|id| self.edges.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn match_nodes(&self, label: Option<&str>) -> Vec<&Node> {
        match label {
            None => self.nodes.values().collect(),
            Some(l) => {
                self.label_index
                    .get(l)
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| self.nodes.get(id))
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
    }

    // --- Mutation methods for SET/REMOVE/DELETE/MERGE ---

    pub fn set_node_property(&mut self, id: NodeId, key: String, value: PropertyValue) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.properties.insert(key, value);
        }
    }

    pub fn set_edge_property(&mut self, id: EdgeId, key: String, value: PropertyValue) {
        if let Some(edge) = self.edges.get_mut(&id) {
            edge.properties.insert(key, value);
        }
    }

    pub fn set_node_all_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.properties = properties;
        }
    }

    pub fn merge_node_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            for (k, v) in properties {
                node.properties.insert(k, v);
            }
        }
    }

    pub fn add_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(node) = self.nodes.get_mut(&id) {
            for label in labels {
                if !node.labels.contains(label) {
                    node.labels.push(label.clone());
                    self.label_index.entry(label.clone()).or_default().insert(id);
                }
            }
        }
    }

    pub fn remove_node_property(&mut self, id: NodeId, key: &str) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.properties.remove(key);
        }
    }

    pub fn remove_node_labels(&mut self, id: NodeId, labels: &[String]) {
        if let Some(node) = self.nodes.get_mut(&id) {
            for label in labels {
                node.labels.retain(|l| l != label);
                if let Some(set) = self.label_index.get_mut(label) {
                    set.remove(&id);
                }
            }
        }
    }

    pub fn delete_node(&mut self, id: NodeId, detach: bool) -> Result<(), String> {
        if !self.nodes.contains_key(&id) {
            return Err(format!("Node {} not found", id));
        }

        // Check only edge IDs that actually exist in self.edges (adjacency lists may
        // contain stale IDs left over from earlier deletes).
        let has_outgoing = self.outgoing.get(&id)
            .map(|v| v.iter().any(|eid| self.edges.contains_key(eid)))
            .unwrap_or(false);
        let has_incoming = self.incoming.get(&id)
            .map(|v| v.iter().any(|eid| self.edges.contains_key(eid)))
            .unwrap_or(false);

        if !detach && (has_outgoing || has_incoming) {
            return Err(format!(
                "Cannot delete node {} because it still has relationships. Use DETACH DELETE.",
                id
            ));
        }

        if detach {
            // Collect all edge IDs incident to this node (outgoing + incoming),
            // deduplicating to handle self-loops correctly.
            let mut edge_ids: Vec<EdgeId> = Vec::new();
            if let Some(out) = self.outgoing.get(&id) {
                edge_ids.extend(out.iter().copied());
            }
            if let Some(inc) = self.incoming.get(&id) {
                for eid in inc {
                    if !edge_ids.contains(eid) {
                        edge_ids.push(*eid);
                    }
                }
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

            self.outgoing.remove(&id);
            self.incoming.remove(&id);
        }

        // Remove adjacency list entries for this node regardless of detach mode.
        // (If detach=false we already confirmed there are no live edges above.)
        self.outgoing.remove(&id);
        self.incoming.remove(&id);

        // Remove node from label index
        if let Some(node) = self.nodes.get(&id) {
            for label in &node.labels.clone() {
                if let Some(set) = self.label_index.get_mut(label) {
                    set.remove(&id);
                }
            }
        }

        self.nodes.remove(&id);
        Ok(())
    }

    pub fn delete_edge(&mut self, id: EdgeId) -> Result<(), String> {
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

    /// Find a node that matches all given labels and properties.
    ///
    /// When `labels` is non-empty the label index is used to narrow the candidate
    /// set to O(|matching nodes|). When `labels` is empty **all** nodes are
    /// scanned in O(|nodes|) — avoid calling with an empty label list on large graphs.
    pub fn find_node(&self, labels: &[String], properties: &HashMap<String, PropertyValue>) -> Option<NodeId> {
        // Use the label index to narrow candidates when labels are present
        let candidates: Box<dyn Iterator<Item = &NodeId>> = if let Some(first_label) = labels.first() {
            match self.label_index.get(first_label) {
                Some(ids) => Box::new(ids.iter()),
                None => return None,
            }
        } else {
            Box::new(self.nodes.keys())
        };

        for id in candidates {
            let node = match self.nodes.get(id) {
                Some(n) => n,
                None => continue,
            };
            let labels_match = labels.iter().all(|l| node.labels.contains(l));
            if !labels_match {
                continue;
            }
            let props_match = properties.iter().all(|(key, val)| {
                node.properties.get(key).map(|v| property_values_equal(v, val)).unwrap_or(false)
            });
            if props_match {
                return Some(*id);
            }
        }
        None
    }

    /// Replace all properties on an edge.
    pub fn set_edge_all_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(edge) = self.edges.get_mut(&id) {
            edge.properties = properties;
        }
    }

    /// Merge properties into an edge.
    pub fn merge_edge_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>) {
        if let Some(edge) = self.edges.get_mut(&id) {
            for (k, v) in properties {
                edge.properties.insert(k, v);
            }
        }
    }

    /// Remove a property from an edge.
    pub fn remove_edge_property(&mut self, id: EdgeId, key: &str) {
        if let Some(edge) = self.edges.get_mut(&id) {
            edge.properties.remove(key);
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
