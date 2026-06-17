use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub type NodeId = u64;
pub type EdgeId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub label: String,
    pub src: NodeId,
    pub dst: NodeId,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub nodes: Vec<Node>,
    pub relationships: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CypherValue {
    Null,
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Vec<CypherValue>),
    Map(HashMap<String, CypherValue>),
    /// Nodes are reference-counted so the executor can thread a scanned node
    /// through records, filters and expands without deep-cloning its property
    /// map on every step.
    Node(Arc<Node>),
    /// Edges are reference-counted for the same reason as nodes: the executor
    /// threads them through records, filters and expands without deep-cloning
    /// the property map on every step.
    Relationship(Arc<Edge>),
    Path(Path),
    Date(NaiveDate),
    Timestamp(DateTime<Utc>),
}
