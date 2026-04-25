use egrph_core::{Graph, NodeId};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

pub struct CGraph {
    graph: Graph,
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_new() -> *mut CGraph {
    Box::into_raw(Box::new(CGraph {
        graph: Graph::new(),
    }))
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_free(ptr: *mut CGraph) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_create_node(ptr: *mut CGraph) -> NodeId {
    if ptr.is_null() {
        return 0;
    }
    let c_graph = unsafe { &mut *ptr };
    c_graph.graph.create_node(vec![], HashMap::new())
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
/// `label` must be a valid null-terminated C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_create_edge(
    ptr: *mut CGraph,
    label: *const c_char,
    src: NodeId,
    dst: NodeId,
) -> i64 {
    if ptr.is_null() || label.is_null() {
        return -1;
    }
    let c_graph = unsafe { &mut *ptr };
    let c_label = unsafe { CStr::from_ptr(label) }
        .to_string_lossy()
        .into_owned();
    match c_graph.graph.create_edge(c_label, src, dst, HashMap::new()) {
        Ok(id) => id as i64,
        Err(_) => -1,
    }
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_get_node_count(ptr: *const CGraph) -> usize {
    if ptr.is_null() {
        return 0;
    }
    let c_graph = unsafe { &*ptr };
    c_graph.graph.node_count()
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_get_edge_count(ptr: *const CGraph) -> usize {
    if ptr.is_null() {
        return 0;
    }
    let c_graph = unsafe { &*ptr };
    c_graph.graph.edge_count()
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
/// `query` must be a valid null-terminated C string, or null.
/// The returned pointer must be freed with `graph_free_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_execute(ptr: *mut CGraph, query: *const c_char) -> *mut c_char {
    if ptr.is_null() || query.is_null() {
        let err = CString::new("{\"error\": \"null pointer\"}").unwrap_or_default();
        return err.into_raw();
    }
    let c_graph = unsafe { &mut *ptr };
    let q = unsafe { CStr::from_ptr(query) }.to_string_lossy();
    match c_graph.graph.execute(&q) {
        Ok(result) => {
            // Serialize the full result: {columns: [...], rows: [[...]]}
            let rows_json: Vec<Vec<serde_json::Value>> = result
                .rows
                .iter()
                .map(|row| row.values.iter().map(cypher_value_to_json).collect())
                .collect();
            let output = serde_json::json!({
                "columns": result.columns,
                "rows": rows_json
            });
            CString::new(output.to_string())
                .unwrap_or_default()
                .into_raw()
        }
        Err(e) => {
            let err_msg = format!("{{\"error\": \"{}\"}}", e);
            CString::new(err_msg).unwrap_or_default().into_raw()
        }
    }
}

fn cypher_value_to_json(val: &egrph_core::CypherValue) -> serde_json::Value {
    match val {
        egrph_core::CypherValue::Null => serde_json::Value::Null,
        egrph_core::CypherValue::Boolean(b) => serde_json::Value::Bool(*b),
        egrph_core::CypherValue::Integer(i) => serde_json::Value::Number((*i).into()),
        egrph_core::CypherValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
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
            serde_json::json!({
                "_id": n.id,
                "_labels": n.labels,
                "_properties": n.properties.iter()
                    .map(|(k, v)| (k.clone(), property_value_to_json(v)))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            })
        }
        egrph_core::CypherValue::Relationship(e) => {
            serde_json::json!({
                "_id": e.id,
                "_type": e.label,
                "_src": e.src,
                "_dst": e.dst,
                "_properties": e.properties.iter()
                    .map(|(k, v)| (k.clone(), property_value_to_json(v)))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            })
        }
        egrph_core::CypherValue::Path(_) => serde_json::Value::String("Path(...)".to_string()),
        egrph_core::CypherValue::Date(d) => serde_json::Value::String(d.to_string()),
        egrph_core::CypherValue::Timestamp(ts) => serde_json::Value::String(ts.to_rfc3339()),
    }
}

fn property_value_to_json(pv: &egrph_core::PropertyValue) -> serde_json::Value {
    match pv {
        egrph_core::PropertyValue::String(s) => serde_json::Value::String(s.clone()),
        egrph_core::PropertyValue::Int(i) => serde_json::Value::Number((*i).into()),
        egrph_core::PropertyValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        egrph_core::PropertyValue::Bool(b) => serde_json::Value::Bool(*b),
    }
}

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
/// The returned pointer must be freed with `graph_free_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_export_cypher(ptr: *const CGraph) -> *mut c_char {
    if ptr.is_null() {
        return CString::new("").unwrap_or_default().into_raw();
    }
    let c_graph = unsafe { &*ptr };
    CString::new(c_graph.graph.export_cypher())
        .unwrap_or_default()
        .into_raw()
}

/// # Safety
/// `s` must be a pointer previously returned by `graph_execute` or `graph_export_cypher`, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}
