use pyo3::prelude::*;
use pyo3::exceptions::PyValueError;
use pyo3::types::PyDict;
use egrph_core::{Graph, NodeId, EdgeId, PropertyValue};
use std::collections::HashMap;

#[pyclass]
struct PyGraph {
    inner: Graph,
}

#[pymethods]
impl PyGraph {
    #[new]
    fn new() -> Self {
        PyGraph { inner: Graph::new() }
    }

    fn create_node(&mut self, labels: Vec<String>, properties: Bound<'_, PyDict>) -> PyResult<NodeId> {
        let mut props = HashMap::new();
        for (k, v) in properties {
            let key = k.extract::<String>()?;
            if let Ok(s) = v.extract::<String>() {
                props.insert(key, PropertyValue::String(s));
            } else if let Ok(i) = v.extract::<i64>() {
                props.insert(key, PropertyValue::Int(i));
            } else if let Ok(f) = v.extract::<f64>() {
                props.insert(key, PropertyValue::Float(f));
            } else if let Ok(b) = v.extract::<bool>() {
                props.insert(key, PropertyValue::Bool(b));
            }
        }
        Ok(self.inner.create_node(labels, props))
    }

    fn create_edge(&mut self, label: String, src: NodeId, dst: NodeId, properties: Bound<'_, PyDict>) -> PyResult<EdgeId> {
        let mut props = HashMap::new();
        for (k, v) in properties {
            let key = k.extract::<String>()?;
            if let Ok(s) = v.extract::<String>() {
                props.insert(key, PropertyValue::String(s));
            } else if let Ok(i) = v.extract::<i64>() {
                props.insert(key, PropertyValue::Int(i));
            } else if let Ok(f) = v.extract::<f64>() {
                props.insert(key, PropertyValue::Float(f));
            } else if let Ok(b) = v.extract::<bool>() {
                props.insert(key, PropertyValue::Bool(b));
            }
        }
        self.inner.create_edge(label, src, dst, props).map_err(PyValueError::new_err)
    }

    fn get_node_count(&self) -> usize {
        self.inner.node_count()
    }

    fn get_edge_count(&self) -> usize {
        self.inner.edge_count()
    }

    fn execute(&mut self, query: &str) -> PyResult<String> {
        let result = self.inner.execute(query).map_err(|e| PyValueError::new_err(e.to_string()))?;
        let mut rows_json = Vec::new();
        for row in &result.rows {
            let mut obj = serde_json::Map::new();
            for (i, col) in result.columns.iter().enumerate() {
                if i < row.values.len() {
                    obj.insert(col.clone(), cypher_value_to_json(&row.values[i]));
                }
            }
            rows_json.push(serde_json::Value::Object(obj));
        }
        serde_json::to_string(&rows_json).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

fn property_value_to_json(pv: &egrph_core::PropertyValue) -> serde_json::Value {
    match pv {
        egrph_core::PropertyValue::String(s) => serde_json::Value::String(s.clone()),
        egrph_core::PropertyValue::Int(i) => serde_json::Value::Number((*i).into()),
        egrph_core::PropertyValue::Float(f) => {
            serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        }
        egrph_core::PropertyValue::Bool(b) => serde_json::Value::Bool(*b),
    }
}

fn cypher_value_to_json(val: &egrph_core::CypherValue) -> serde_json::Value {
    match val {
        egrph_core::CypherValue::Null => serde_json::Value::Null,
        egrph_core::CypherValue::Boolean(b) => serde_json::Value::Bool(*b),
        egrph_core::CypherValue::Integer(i) => serde_json::Value::Number((*i).into()),
        egrph_core::CypherValue::Float(f) => {
            serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null)
        }
        egrph_core::CypherValue::String(s) => serde_json::Value::String(s.clone()),
        egrph_core::CypherValue::List(items) => {
            serde_json::Value::Array(items.iter().map(cypher_value_to_json).collect())
        }
        egrph_core::CypherValue::Map(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), cypher_value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        egrph_core::CypherValue::Node(n) => {
            let mut obj = serde_json::Map::new();
            obj.insert("_id".to_string(), serde_json::Value::Number(n.id.into()));
            obj.insert("_labels".to_string(), serde_json::Value::Array(
                n.labels.iter().map(|l| serde_json::Value::String(l.clone())).collect()
            ));
            let props: serde_json::Map<String, serde_json::Value> = n.properties
                .iter()
                .map(|(k, v)| (k.clone(), property_value_to_json(v)))
                .collect();
            obj.insert("_properties".to_string(), serde_json::Value::Object(props));
            serde_json::Value::Object(obj)
        }
        egrph_core::CypherValue::Relationship(e) => {
            let mut obj = serde_json::Map::new();
            obj.insert("_id".to_string(), serde_json::Value::Number(e.id.into()));
            obj.insert("_type".to_string(), serde_json::Value::String(e.label.clone()));
            obj.insert("_src".to_string(), serde_json::Value::Number(e.src.into()));
            obj.insert("_dst".to_string(), serde_json::Value::Number(e.dst.into()));
            let props: serde_json::Map<String, serde_json::Value> = e.properties
                .iter()
                .map(|(k, v)| (k.clone(), property_value_to_json(v)))
                .collect();
            obj.insert("_properties".to_string(), serde_json::Value::Object(props));
            serde_json::Value::Object(obj)
        }
        egrph_core::CypherValue::Path(_) => serde_json::Value::String("Path(...)".to_string()),
    }
}

#[pymodule]
fn egrph(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyGraph>()?;
    Ok(())
}
