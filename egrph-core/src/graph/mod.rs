pub mod backend;
#[cfg(feature = "sled-storage")]
pub mod sled_storage;
pub mod storage;
pub mod types;

use self::backend::StorageBackend;
use self::storage::MemoryStorage;
use self::types::*;
use crate::error::CypherError;
use crate::executor::result::QueryResult;
use std::collections::HashMap;

/// Upper bound on cached query plans. Workloads reuse a small fixed set of
/// query strings (ingest, rules, reads), so this caps memory against a caller
/// that issues unboundedly many distinct query texts; once full, further new
/// queries are planned per-call without being cached.
const PLAN_CACHE_MAX: usize = 1024;

pub struct Graph<S: StorageBackend = MemoryStorage> {
    pub(crate) storage: S,
    /// Parse+plan cache keyed by the verbatim query string. A `LogicalPlan` is
    /// a pure function of the query text (parameters bind at execution), so the
    /// same string always yields the same plan. Avoids re-parsing/re-planning a
    /// hot query (e.g. the ingest MERGE) on every call.
    plan_cache: HashMap<String, crate::planner::plan::LogicalPlan>,
}

impl Default for Graph<MemoryStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl Graph<MemoryStorage> {
    pub fn new() -> Self {
        Graph {
            storage: MemoryStorage::new(),
            plan_cache: HashMap::new(),
        }
    }
}

impl<S: StorageBackend> Graph<S> {
    /// Construct a `Graph` backed by an arbitrary `StorageBackend`.
    pub fn new_with_storage(storage: S) -> Self {
        Graph {
            storage,
            plan_cache: HashMap::new(),
        }
    }

    /// New primary entry point: parse, plan, and execute a Cypher query.
    pub fn execute(&mut self, q: &str) -> Result<QueryResult, CypherError> {
        self.execute_with_params(q, HashMap::new())
    }

    /// Execute a Cypher query with named parameters ($param syntax).
    pub fn execute_with_params(
        &mut self,
        q: &str,
        params: HashMap<String, CypherValue>,
    ) -> Result<QueryResult, CypherError> {
        if !self.plan_cache.contains_key(q) {
            let stmt = crate::parser::parse_with_return_extraction(q)?;
            let plan = crate::planner::plan(&stmt)?;
            if self.plan_cache.len() >= PLAN_CACHE_MAX {
                // Cache saturated: run this plan without caching to bound memory.
                return crate::executor::execute_with_params(&plan, &mut self.storage, params);
            }
            self.plan_cache.insert(q.to_string(), plan);
        }
        // Disjoint field borrows: `&self.plan_cache` for the plan, `&mut
        // self.storage` for execution.
        let plan = self.plan_cache.get(q).expect("plan present after insert");
        crate::executor::execute_with_params(plan, &mut self.storage, params)
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

    pub fn get_node(&self, id: NodeId) -> Option<Node> {
        self.storage.get_node(id)
    }

    pub fn get_edge(&self, id: EdgeId) -> Option<Edge> {
        self.storage.get_edge(id)
    }

    pub fn match_nodes(&self, label: Option<&str>) -> Vec<Node> {
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
        let node_ids = self.storage.all_node_ids();
        if node_ids.is_empty() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();

        for id in &node_ids {
            if let Some(node) = self.storage.get_node(*id) {
                let var = format!("_{}", id);
                let labels = if node.labels.is_empty() {
                    String::new()
                } else {
                    format!(":{}", node.labels.join(":"))
                };
                let props = format_properties(&node.properties);
                parts.push(format!("({var}{labels}{props})"));
            }
        }

        let edge_ids = self.storage.all_edge_ids();
        for id in &edge_ids {
            if let Some(edge) = self.storage.get_edge(*id) {
                let src_var = format!("_{}", edge.src);
                let dst_var = format!("_{}", edge.dst);
                let props = format_properties(&edge.properties);
                parts.push(format!(
                    "({src_var})-[:{label}{props}]->({dst_var})",
                    label = edge.label
                ));
            }
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
                    format!(
                        "\"{}\"",
                        s.replace('\\', "\\\\")
                            .replace('"', "\\\"")
                            .replace('\n', "\\n")
                            .replace('\r', "\\r")
                            .replace('\t', "\\t")
                    )
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
