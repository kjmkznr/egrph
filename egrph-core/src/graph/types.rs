use std::collections::HashMap;

pub type NodeId = u64;
pub type EdgeId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub id: EdgeId,
    pub label: String,
    pub src: NodeId,
    pub dst: NodeId,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq)]
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
    Node(Node),
    Relationship(Edge),
    Path(Path),
}
