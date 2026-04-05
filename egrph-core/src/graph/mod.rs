pub mod storage;
pub mod types;

use self::storage::GraphStorage;
use self::types::*;
use crate::error::CypherError;
use crate::executor::result::QueryResult;
use std::collections::HashMap;

pub struct Graph {
    pub(crate) storage: GraphStorage,
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            storage: GraphStorage::new(),
        }
    }

    /// New primary entry point: parse, plan, and execute a Cypher query.
    pub fn execute(&mut self, q: &str) -> Result<QueryResult, CypherError> {
        let stmt = crate::parser::parse_with_return_extraction(q)?;
        let plan = crate::planner::plan(&stmt)?;
        crate::executor::execute(&plan, &mut self.storage)
    }

    /// Legacy query method - preserved for backward compatibility.
    /// Returns node IDs from the result.
    #[deprecated(note = "Use execute() instead")]
    pub fn query(&mut self, q: &str) -> Result<Vec<NodeId>, String> {
        let result = self.execute(q).map_err(|e| e.to_string())?;
        Ok(result
            .rows
            .iter()
            .filter_map(|r| match r.values.first() {
                Some(CypherValue::Node(n)) => Some(n.id),
                Some(CypherValue::Integer(id)) => Some(*id as NodeId),
                _ => None,
            })
            .collect())
    }

    // Direct API methods (preserved from original)

    pub fn create_node(
        &mut self,
        labels: Vec<String>,
        properties: HashMap<String, PropertyValue>,
    ) -> NodeId {
        self.storage.create_node(labels, properties)
    }

    pub fn create_edge(
        &mut self,
        label: String,
        src: NodeId,
        dst: NodeId,
        properties: HashMap<String, PropertyValue>,
    ) -> Result<EdgeId, String> {
        self.storage.create_edge(label, src, dst, properties)
    }

    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.storage.get_node(id)
    }

    pub fn get_edge(&self, id: EdgeId) -> Option<&Edge> {
        self.storage.get_edge(id)
    }

    pub fn match_nodes(&self, label: Option<&str>) -> Vec<&Node> {
        self.storage.match_nodes(label)
    }

    pub fn node_count(&self) -> usize {
        self.storage.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.storage.edge_count()
    }
}
