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
        self.inner.nodes.len()
    }

    fn get_edge_count(&self) -> usize {
        self.inner.edges.len()
    }
}

#[pymodule]
fn egrph(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyGraph>()?;
    Ok(())
}
