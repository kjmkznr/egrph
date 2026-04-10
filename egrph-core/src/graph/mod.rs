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

    /// Execute a Cypher query with named parameters ($param syntax).
    pub fn execute_with_params(
        &mut self,
        q: &str,
        params: HashMap<String, CypherValue>,
    ) -> Result<QueryResult, CypherError> {
        let stmt = crate::parser::parse_with_return_extraction(q)?;
        let plan = crate::planner::plan(&stmt)?;
        crate::executor::execute_with_params(&plan, &mut self.storage, params)
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

    /// Export the entire graph as a Cypher CREATE statement that can be used to recreate it.
    pub fn export_cypher(&self) -> String {
        if self.storage.nodes.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();

        let mut node_ids: Vec<NodeId> = self.storage.nodes.keys().copied().collect();
        node_ids.sort();

        for id in &node_ids {
            let node = &self.storage.nodes[id];
            let var = format!("_{}", id);
            let labels = if node.labels.is_empty() {
                String::new()
            } else {
                format!(":{}", node.labels.join(":"))
            };
            let props = format_properties(&node.properties);
            parts.push(format!("({var}{labels}{props})"));
        }

        let mut edge_ids: Vec<EdgeId> = self.storage.edges.keys().copied().collect();
        edge_ids.sort();

        for id in &edge_ids {
            let edge = &self.storage.edges[id];
            let src_var = format!("_{}", edge.src);
            let dst_var = format!("_{}", edge.dst);
            let props = format_properties(&edge.properties);
            parts.push(format!(
                "({src_var})-[:{label}{props}]->({dst_var})",
                label = edge.label
            ));
        }

        format!("CREATE\n  {}", parts.join(",\n  "))
    }
}

fn format_properties(props: &HashMap<String, PropertyValue>) -> String {
    if props.is_empty() {
        return String::new();
    }
    let mut keys: Vec<&String> = props.keys().collect();
    keys.sort();
    let entries: Vec<String> = keys
        .iter()
        .map(|k| {
            let v = match &props[*k] {
                PropertyValue::String(s) => {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                }
                PropertyValue::Int(i) => i.to_string(),
                PropertyValue::Float(f) => {
                    if f.fract() == 0.0 {
                        format!("{f:.1}")
                    } else {
                        f.to_string()
                    }
                }
                PropertyValue::Bool(b) => b.to_string(),
            };
            format!("{k}: {v}")
        })
        .collect();
    format!(" {{{}}}", entries.join(", "))
}
