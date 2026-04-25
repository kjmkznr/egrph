use egrph_core::{CypherValue, Graph, PropertyValue};
use wasm_bindgen::prelude::*;

/// In-memory graph database exposed to JavaScript via WebAssembly.
///
/// ```javascript
/// import init, { WasmGraph } from './pkg/egrph_wasm.js';
/// await init();
/// const g = new WasmGraph();
/// g.execute('CREATE (:Person {name: "Alice", age: 30})');
/// const rows = JSON.parse(g.execute('MATCH (p:Person) RETURN p.name'));
/// g.free();
/// ```
///
/// ## Node and relationship IDs
///
/// `_id`, `_src`, and `_dst` fields in the JSON output are serialized as
/// **strings** (not numbers). JavaScript's `JSON.parse` silently loses
/// precision for integers above 2^53, and `NodeId` is a `u64`, so string
/// serialization is used to preserve correctness in all cases.
#[wasm_bindgen]
pub struct WasmGraph {
    inner: Graph,
}

impl Default for WasmGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmGraph {
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmGraph {
        // Redirect Rust panics to browser/Node.js console with a readable message.
        console_error_panic_hook::set_once();
        WasmGraph {
            inner: Graph::new(),
        }
    }

    /// Execute a Cypher query and return results as a JSON string.
    ///
    /// Returns a JSON array of objects, one per result row.
    /// Throws a JavaScript exception on query error or unsupported result type.
    pub fn execute(&mut self, query: &str) -> Result<String, JsValue> {
        let result = self
            .inner
            .execute(query)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let mut rows_json = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let mut obj = serde_json::Map::new();
            for (i, col) in result.columns.iter().enumerate() {
                let val = match row.values.get(i) {
                    Some(v) => cypher_value_to_json(v)?,
                    None => {
                        debug_assert!(false, "column '{col}' has no value in row — executor bug");
                        serde_json::Value::Null
                    }
                };
                obj.insert(col.clone(), val);
            }
            rows_json.push(serde_json::Value::Object(obj));
        }

        serde_json::to_string(&rows_json)
            .map_err(|e| JsValue::from_str(&format!("JSON serialization error: {e}")))
    }

    /// Returns the number of nodes as `u32`.
    ///
    /// Saturates at `u32::MAX` if the count exceeds ~4 billion, which is not
    /// reachable in practice for an in-memory WASM graph.
    #[wasm_bindgen(js_name = nodeCount)]
    pub fn node_count(&self) -> u32 {
        self.inner.node_count().min(u32::MAX as usize) as u32
    }

    /// Returns the number of edges as `u32`.
    ///
    /// Saturates at `u32::MAX` if the count exceeds ~4 billion, which is not
    /// reachable in practice for an in-memory WASM graph.
    #[wasm_bindgen(js_name = edgeCount)]
    pub fn edge_count(&self) -> u32 {
        self.inner.edge_count().min(u32::MAX as usize) as u32
    }

    /// Export the entire graph as a Cypher CREATE statement string.
    ///
    /// The returned string can be passed back to `execute()` to recreate the graph.
    /// Returns an empty string if the graph has no nodes.
    #[wasm_bindgen(js_name = exportCypher)]
    pub fn export_cypher(&self) -> String {
        self.inner.export_cypher()
    }
}

fn cypher_value_to_json(val: &CypherValue) -> Result<serde_json::Value, JsValue> {
    Ok(match val {
        CypherValue::Null => serde_json::Value::Null,
        CypherValue::Boolean(b) => serde_json::Value::Bool(*b),
        CypherValue::Integer(i) => serde_json::Value::Number((*i).into()),
        CypherValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| JsValue::from_str(&format!("Non-finite float value: {f}")))?,
        CypherValue::String(s) => serde_json::Value::String(s.clone()),
        CypherValue::List(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(cypher_value_to_json).collect();
            serde_json::Value::Array(arr?)
        }
        CypherValue::Map(map) => {
            let obj: Result<serde_json::Map<_, _>, _> = map
                .iter()
                .map(|(k, v)| cypher_value_to_json(v).map(|v| (k.clone(), v)))
                .collect();
            serde_json::Value::Object(obj?)
        }
        CypherValue::Node(n) => {
            let mut obj = serde_json::Map::new();
            // Serialize as string: JS JSON.parse loses precision for u64 > 2^53.
            obj.insert(
                "_id".to_string(),
                serde_json::Value::String(n.id.to_string()),
            );
            obj.insert(
                "_labels".to_string(),
                serde_json::Value::Array(
                    n.labels
                        .iter()
                        .map(|l| serde_json::Value::String(l.clone()))
                        .collect(),
                ),
            );
            let props: serde_json::Map<String, serde_json::Value> = n
                .properties
                .iter()
                .map(|(k, v)| property_value_to_json(v).map(|v| (k.clone(), v)))
                .collect::<Result<_, _>>()?;
            obj.insert("_properties".to_string(), serde_json::Value::Object(props));
            serde_json::Value::Object(obj)
        }
        CypherValue::Relationship(e) => {
            let mut obj = serde_json::Map::new();
            // Serialize as strings: JS JSON.parse loses precision for u64 > 2^53.
            obj.insert(
                "_id".to_string(),
                serde_json::Value::String(e.id.to_string()),
            );
            obj.insert(
                "_type".to_string(),
                serde_json::Value::String(e.label.clone()),
            );
            obj.insert(
                "_src".to_string(),
                serde_json::Value::String(e.src.to_string()),
            );
            obj.insert(
                "_dst".to_string(),
                serde_json::Value::String(e.dst.to_string()),
            );
            let props: serde_json::Map<String, serde_json::Value> = e
                .properties
                .iter()
                .map(|(k, v)| property_value_to_json(v).map(|v| (k.clone(), v)))
                .collect::<Result<_, _>>()?;
            obj.insert("_properties".to_string(), serde_json::Value::Object(props));
            serde_json::Value::Object(obj)
        }
        CypherValue::Path(_) => {
            return Err(JsValue::from_str(
                "Path results are not yet supported in WASM bindings",
            ));
        }
    })
}

fn property_value_to_json(pv: &PropertyValue) -> Result<serde_json::Value, JsValue> {
    Ok(match pv {
        PropertyValue::String(s) => serde_json::Value::String(s.clone()),
        PropertyValue::Int(i) => serde_json::Value::Number((*i).into()),
        PropertyValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| JsValue::from_str(&format!("Non-finite float value: {f}")))?,
        PropertyValue::Bool(b) => serde_json::Value::Bool(*b),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn test_create_and_query() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        let json = g.execute("MATCH (p:Person) RETURN p.name, p.age").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["p.name"], "Alice");
        assert_eq!(v[0]["p.age"], 30);
    }

    #[wasm_bindgen_test]
    fn test_node_and_edge_count() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:A), (:B)").unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 0);
    }

    #[wasm_bindgen_test]
    fn test_sparse_row_returns_null() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:Thing {x: 1})").unwrap();
        let json = g.execute("MATCH (t:Thing) RETURN t.x, t.missing").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["t.missing"], serde_json::Value::Null);
    }

    #[wasm_bindgen_test]
    fn test_execute_error_on_invalid_query() {
        let mut g = WasmGraph::new();
        assert!(g.execute("THIS IS NOT CYPHER").is_err());
    }

    #[wasm_bindgen_test]
    fn test_where_filter() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        let json = g
            .execute("MATCH (p:Person) WHERE p.age > 28 RETURN p.name")
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rows = v.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["p.name"], "Alice");
    }

    #[wasm_bindgen_test]
    fn test_node_serialization() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        let json = g.execute("MATCH (p:Person) RETURN p").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let node = &v[0]["p"];
        // IDs are serialized as strings to preserve u64 precision in JS
        assert!(node["_id"].is_string());
        assert_eq!(node["_labels"][0], "Person");
        assert_eq!(node["_properties"]["name"], "Alice");
    }

    #[wasm_bindgen_test]
    fn test_relationship_serialization() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:A)-[:KNOWS]->(:B)").unwrap();
        let json = g.execute("MATCH ()-[r:KNOWS]->() RETURN r").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rel = &v[0]["r"];
        // IDs are serialized as strings to preserve u64 precision in JS
        assert!(rel["_id"].is_string());
        assert!(rel["_src"].is_string());
        assert!(rel["_dst"].is_string());
        assert_eq!(rel["_type"], "KNOWS");
    }

    #[wasm_bindgen_test]
    fn test_export_cypher() {
        let mut g = WasmGraph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute(
            "MATCH (a:Person {name: \"Alice\"}), (b:Person {name: \"Bob\"}) CREATE (a)-[:KNOWS]->(b)",
        )
        .unwrap();

        let cypher = g.export_cypher();
        assert!(cypher.starts_with("CREATE\n"));
        assert!(cypher.contains(":Person"));
        assert!(cypher.contains("-[:KNOWS]->"));

        // Roundtrip: recreate from exported Cypher
        let mut g2 = WasmGraph::new();
        g2.execute(&cypher).unwrap();
        assert_eq!(g2.node_count(), 2);
        assert_eq!(g2.edge_count(), 1);
    }

    #[wasm_bindgen_test]
    fn test_float_round_trip() {
        let mut g = WasmGraph::new();
        // 3.5 is exactly representable in IEEE 754 so serde_json always serializes
        // it as "3.5", avoiding rounding-related false negatives.
        g.execute("CREATE (:Metric {val: 3.5})").unwrap();
        let json = g.execute("MATCH (m:Metric) RETURN m.val").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let got = v[0]["m.val"].as_f64().unwrap();
        assert!((got - 3.5).abs() < f64::EPSILON);
    }
}
