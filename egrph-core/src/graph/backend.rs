use super::types::{Edge, EdgeId, Node, NodeId, PropertyValue};
use std::collections::HashMap;
use std::sync::Arc;

/// Trait abstracting the graph storage layer.
///
/// All read methods return **owned** values so that both in-memory and
/// persistent (e.g. sled) backends can implement this trait without lifetime
/// complexity.  For `MemoryStorage` this means cloning on read; for
/// `SledStorage` it means deserialising from disk.
pub trait StorageBackend {
    // ── Read ──────────────────────────────────────────────────────────────

    fn get_node(&self, id: NodeId) -> Option<Arc<Node>>;
    fn get_edge(&self, id: EdgeId) -> Option<Arc<Edge>>;

    fn node_count(&self) -> usize;
    fn edge_count(&self) -> usize;

    fn outgoing_edges(&self, node_id: NodeId) -> Vec<Arc<Edge>>;
    fn incoming_edges(&self, node_id: NodeId) -> Vec<Arc<Edge>>;

    /// Return edges from `src` to `dst` with relationship type `rel_type`.
    /// Used to match a relationship between two already-bound nodes (e.g. the
    /// edge MERGE in `(a)-[:R]->(b)`) without scanning `src`'s whole adjacency.
    /// Implementations should override with an endpoint index for O(1) lookup;
    /// the default falls back to scanning `outgoing_edges`.
    fn edges_between(&self, src: NodeId, rel_type: &str, dst: NodeId) -> Vec<Arc<Edge>> {
        self.outgoing_edges(src)
            .into_iter()
            .filter(|e| e.label == rel_type && e.dst == dst)
            .collect()
    }

    /// Return edge IDs for outgoing edges of `node_id`.
    fn outgoing_edge_ids(&self, node_id: NodeId) -> Vec<EdgeId>;

    /// Return edge IDs for incoming edges of `node_id`.
    fn incoming_edge_ids(&self, node_id: NodeId) -> Vec<EdgeId>;

    /// Return all nodes optionally filtered by label.
    fn match_nodes(&self, label: Option<&str>) -> Vec<Arc<Node>>;

    /// Return all nodes matching `label` (if given) and ALL of `props`.
    /// Implementations should override this to use a property index for O(1)
    /// lookup.  The default falls back to a full scan + filter.
    fn match_nodes_with_props(
        &self,
        label: Option<&str>,
        props: &HashMap<String, PropertyValue>,
    ) -> Vec<Arc<Node>> {
        self.match_nodes(label)
            .into_iter()
            .filter(|node| {
                props
                    .iter()
                    .all(|(key, val)| node.properties.get(key).map(|v| v == val).unwrap_or(false))
            })
            .collect()
    }

    /// Return the first node ID that matches `labels` and `properties`.
    fn find_node(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Option<NodeId>;

    /// Return all node IDs that match `labels` and `properties`.
    fn find_nodes(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Vec<NodeId>;

    /// Return all node IDs (used for `export_cypher`).
    fn all_node_ids(&self) -> Vec<NodeId>;

    /// Return all edge IDs (used for `export_cypher`).
    fn all_edge_ids(&self) -> Vec<EdgeId>;

    // ── Write ─────────────────────────────────────────────────────────────

    fn create_node(
        &mut self,
        labels: Vec<String>,
        properties: HashMap<String, PropertyValue>,
    ) -> NodeId;

    fn create_edge(
        &mut self,
        label: String,
        src: NodeId,
        dst: NodeId,
        properties: HashMap<String, PropertyValue>,
    ) -> Result<EdgeId, String>;

    fn set_node_property(&mut self, id: NodeId, key: &str, value: PropertyValue);
    fn set_edge_property(&mut self, id: EdgeId, key: &str, value: PropertyValue);

    fn set_node_all_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>);
    fn set_edge_all_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>);

    fn merge_node_properties(&mut self, id: NodeId, properties: HashMap<String, PropertyValue>);
    fn merge_edge_properties(&mut self, id: EdgeId, properties: HashMap<String, PropertyValue>);

    fn add_node_labels(&mut self, id: NodeId, labels: &[String]);

    fn remove_node_property(&mut self, id: NodeId, key: &str);
    fn remove_node_labels(&mut self, id: NodeId, labels: &[String]);
    fn remove_edge_property(&mut self, id: EdgeId, key: &str);

    fn delete_node(&mut self, id: NodeId, detach: bool) -> Result<(), String>;
    fn delete_edge(&mut self, id: EdgeId) -> Result<(), String>;

    // ── Constraints ───────────────────────────────────────────────────────

    /// Register a unique constraint for `property` on nodes with `label`.
    /// Returns an error if existing data already violates the constraint.
    fn add_unique_constraint(&mut self, label: &str, property: &str) -> Result<(), String>;

    /// Check whether adding a node with `label` and `value` for `property`
    /// would violate any registered unique constraint.
    fn check_unique_constraint(
        &self,
        label: &str,
        property: &str,
        value: &PropertyValue,
    ) -> Result<(), String>;

    /// List all registered unique constraints as `(label, property)` pairs.
    fn list_unique_constraints(&self) -> Vec<(String, String)>;

    /// Register a NOT NULL constraint for `property` on nodes with `label`.
    /// Returns an error if existing nodes with `label` are missing `property`.
    fn add_not_null_constraint(&mut self, label: &str, property: &str) -> Result<(), String>;

    /// Check whether the given `properties` satisfy all NOT NULL constraints
    /// for any of the given `labels`.
    fn check_not_null_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String>;

    /// Register a NODE KEY constraint for `properties` on nodes with `label`.
    /// NODE KEY = composite uniqueness + all properties must be non-null.
    /// Returns an error if existing data already violates the constraint.
    fn add_node_key_constraint(&mut self, label: &str, properties: &[String])
    -> Result<(), String>;

    /// Check whether the given `node_properties` satisfy all NODE KEY constraints
    /// for any of the given `labels`.
    fn check_node_key_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String>;

    /// Register a PROPERTY TYPE constraint: `property` on nodes with `label`
    /// must have type `type_name` ("BOOLEAN", "STRING", "INTEGER", "FLOAT")
    /// when present.
    /// Returns an error if existing nodes violate the type constraint.
    fn add_property_type_constraint(
        &mut self,
        label: &str,
        property: &str,
        type_name: &str,
    ) -> Result<(), String>;

    /// Check whether the given `properties` satisfy all PROPERTY TYPE constraints
    /// for any of the given `labels`.
    fn check_property_type_constraints(
        &self,
        labels: &[String],
        properties: &HashMap<String, PropertyValue>,
    ) -> Result<(), String>;
}
