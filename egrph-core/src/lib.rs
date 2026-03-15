use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use pest::Parser;
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "cypher.pest"]
pub struct CypherParser;

pub type NodeId = u64;
pub type EdgeId = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropertyValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub label: String,
    pub src: NodeId,
    pub dst: NodeId,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: HashMap<NodeId, Node>,
    pub edges: HashMap<EdgeId, Edge>,
    next_node_id: NodeId,
    next_edge_id: EdgeId,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_node(&mut self, labels: Vec<String>, properties: HashMap<String, PropertyValue>) -> NodeId {
        let id = self.next_node_id;
        let node = Node {
            id,
            labels,
            properties,
        };
        self.nodes.insert(id, node);
        self.next_node_id += 1;
        id
    }

    pub fn create_edge(&mut self, label: String, src: NodeId, dst: NodeId, properties: HashMap<String, PropertyValue>) -> Result<EdgeId, String> {
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
        self.next_edge_id += 1;
        Ok(id)
    }

    pub fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(&id)
    }

    pub fn match_nodes(&self, label: Option<&str>) -> Vec<&Node> {
        self.nodes.values()
            .filter(|n| {
                if let Some(l) = label {
                    n.labels.iter().any(|node_label| node_label == l)
                } else {
                    true
                }
            })
            .collect()
    }

    pub fn query(&mut self, q: &str) -> Result<Vec<NodeId>, String> {
        let pairs = CypherParser::parse(Rule::query, q)
            .map_err(|e| format!("Parse error: {}", e))?;

        for pair in pairs {
            match pair.as_rule() {
                Rule::query => {
                    for inner in pair.into_inner() {
                        match inner.as_rule() {
                            Rule::create_clause => {
                                let mut labels = Vec::new();
                                let mut properties = HashMap::new();
                                
                                for p in inner.into_inner() {
                                    if p.as_rule() == Rule::node_pattern {
                                        for np in p.into_inner() {
                                            match np.as_rule() {
                                                Rule::labels => {
                                                    for label in np.into_inner() {
                                                        labels.push(label.as_str().to_string());
                                                    }
                                                }
                                                Rule::properties => {
                                                    if let Some(pl) = np.into_inner().next() {
                                                        for prop in pl.into_inner() {
                                                            let mut inner_prop = prop.into_inner();
                                                            let key = inner_prop.next().unwrap().as_str().to_string();
                                                            let val_pair = inner_prop.next().unwrap();
                                                            let val_rule = val_pair.clone().into_inner().next().unwrap().as_rule();
                                                            let val = match val_rule {
                                                                Rule::string => {
                                                                    let s = val_pair.as_str();
                                                                    PropertyValue::String(s[1..s.len()-1].to_string())
                                                                }
                                                                Rule::integer => PropertyValue::Int(val_pair.as_str().parse().unwrap()),
                                                                Rule::boolean => PropertyValue::Bool(val_pair.as_str().to_lowercase() == "true"),
                                                                _ => unreachable!(),
                                                            };
                                                            properties.insert(key, val);
                                                        }
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                let id = self.create_node(labels, properties);
                                return Ok(vec![id]);
                            }
                            Rule::match_clause => {
                                let mut target_label = None;
                                for p in inner.into_inner() {
                                    if p.as_rule() == Rule::node_pattern {
                                        for np in p.into_inner() {
                                            if np.as_rule() == Rule::labels {
                                                target_label = Some(np.into_inner().next().unwrap().as_str());
                                            }
                                        }
                                    }
                                }
                                return Ok(self.match_nodes(target_label).into_iter().map(|n| n.id).collect());
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cypher_query() {
        let mut g = Graph::new();
        g.query("CREATE (:Person {name: \"Alice\", age: 30})").unwrap();
        g.query("CREATE (:Person {name: \"Bob\", age: 25})").unwrap();

        let results = g.query("MATCH (:Person) RETURN p").unwrap();
        assert_eq!(results.len(), 2);

        let n1 = g.get_node(results[0]).unwrap();
        assert!(n1.labels.contains(&"Person".to_string()));
    }

    #[test]
    fn test_graph_ops() {
        let mut g = Graph::new();
        let n1 = g.create_node(vec!["Person".to_string()], HashMap::new());
        let n2 = g.create_node(vec!["Person".to_string()], HashMap::new());
        let e1 = g.create_edge("KNOWS".to_string(), n1, n2, HashMap::new()).unwrap();

        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.get_edge(e1).unwrap().src, n1);
    }
}
