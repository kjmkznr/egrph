/// Query result types for the Cypher executor.

use crate::graph::types::CypherValue;

/// A single row of results.
#[derive(Debug, Clone)]
pub struct ResultRow {
    pub values: Vec<CypherValue>,
}

/// The result of executing a Cypher query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<ResultRow>,
}

impl QueryResult {
    pub fn empty() -> Self {
        QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }
}
