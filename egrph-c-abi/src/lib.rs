use egrph_core::{CypherValue, Graph, NodeId};
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

/// # Safety
/// `ptr` must be a valid pointer returned by `graph_new`, or null.
/// `query` and `params_json` must be valid null-terminated C strings, or null.
/// `params_json` must encode a JSON object (top-level `{}`); each value is
/// converted to a `CypherValue` (Null/Bool/Integer/Float/String/List/Map).
/// The returned pointer must be freed with `graph_free_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graph_execute_with_params(
    ptr: *mut CGraph,
    query: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    if ptr.is_null() || query.is_null() || params_json.is_null() {
        let err = CString::new("{\"error\": \"null pointer\"}").unwrap_or_default();
        return err.into_raw();
    }
    let c_graph = unsafe { &mut *ptr };
    let q = unsafe { CStr::from_ptr(query) }.to_string_lossy();
    let p_str = unsafe { CStr::from_ptr(params_json) }.to_string_lossy();

    let params = match serde_json::from_str::<serde_json::Value>(&p_str) {
        Ok(serde_json::Value::Object(map)) => {
            let mut out: HashMap<String, CypherValue> = HashMap::with_capacity(map.len());
            for (k, v) in map.into_iter() {
                out.insert(k, json_to_cypher_value(&v));
            }
            out
        }
        Ok(_) => {
            let err =
                CString::new("{\"error\": \"params_json must be a JSON object\"}".to_string())
                    .unwrap_or_default();
            return err.into_raw();
        }
        Err(e) => {
            let msg = serde_json::json!({ "error": format!("invalid params_json: {}", e) });
            return CString::new(msg.to_string()).unwrap_or_default().into_raw();
        }
    };

    match c_graph.graph.execute_with_params(&q, params) {
        Ok(result) => {
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
            let msg = serde_json::json!({ "error": e.to_string() });
            CString::new(msg.to_string()).unwrap_or_default().into_raw()
        }
    }
}

fn json_to_cypher_value(v: &serde_json::Value) -> CypherValue {
    match v {
        serde_json::Value::Null => CypherValue::Null,
        serde_json::Value::Bool(b) => CypherValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                CypherValue::Integer(i)
            } else if let Some(u) = n.as_u64() {
                // u64 above i64::MAX falls back to Float to avoid overflow
                if u <= i64::MAX as u64 {
                    CypherValue::Integer(u as i64)
                } else {
                    CypherValue::Float(u as f64)
                }
            } else {
                CypherValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => CypherValue::String(s.clone()),
        serde_json::Value::Array(items) => {
            CypherValue::List(items.iter().map(json_to_cypher_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out: HashMap<String, CypherValue> = HashMap::with_capacity(map.len());
            for (k, val) in map.iter() {
                out.insert(k.clone(), json_to_cypher_value(val));
            }
            CypherValue::Map(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn exec_with_params(g: *mut CGraph, q: &str, params: &str) -> serde_json::Value {
        let c_q = CString::new(q).unwrap();
        let c_p = CString::new(params).unwrap();
        let raw = unsafe { graph_execute_with_params(g, c_q.as_ptr(), c_p.as_ptr()) };
        let s = unsafe { CStr::from_ptr(raw) }
            .to_string_lossy()
            .into_owned();
        unsafe { graph_free_string(raw) };
        serde_json::from_str(&s).expect("returned JSON parses")
    }

    #[test]
    fn execute_with_params_integer_roundtrip() {
        let g = graph_new();
        let v = unsafe {
            exec_with_params(
                g,
                "CREATE (n:Log {id: $id, ts: $ts}) RETURN n.id AS id, n.ts AS ts",
                r#"{"id":"u1","ts":1715600000000}"#,
            )
        };
        assert_eq!(v["columns"], serde_json::json!(["id", "ts"]));
        assert_eq!(v["rows"][0][0], serde_json::json!("u1"));
        assert_eq!(v["rows"][0][1], serde_json::json!(1_715_600_000_000_i64));
        unsafe { graph_free(g) };
    }

    #[test]
    fn execute_with_params_string_escapes_safely() {
        let g = graph_new();
        // Single quote inside the parameter must not break the query.
        let v = unsafe {
            exec_with_params(
                g,
                "CREATE (n:U {name: $name}) RETURN n.name AS name",
                r#"{"name":"O'Reilly"}"#,
            )
        };
        assert_eq!(v["rows"][0][0], serde_json::json!("O'Reilly"));
        unsafe { graph_free(g) };
    }

    #[test]
    fn execute_with_params_unwind_batch() {
        let g = graph_new();
        let v = unsafe {
            exec_with_params(
                g,
                "UNWIND $rows AS r CREATE (n:Log {id: r.id, ts: r.ts}) RETURN count(n) AS c",
                r#"{"rows":[{"id":"a","ts":1},{"id":"b","ts":2},{"id":"c","ts":3}]}"#,
            )
        };
        assert_eq!(v["rows"][0][0], serde_json::json!(3));
        assert_eq!(unsafe { graph_get_node_count(g) }, 3);
        unsafe { graph_free(g) };
    }

    #[test]
    fn execute_with_params_invalid_json_returns_error() {
        let g = graph_new();
        let v = unsafe { exec_with_params(g, "RETURN 1", "not json") };
        assert!(v.get("error").is_some(), "got: {v}");
        unsafe { graph_free(g) };
    }

    #[test]
    fn execute_with_params_non_object_returns_error() {
        let g = graph_new();
        let v = unsafe { exec_with_params(g, "RETURN 1", "[1,2,3]") };
        assert!(v.get("error").is_some(), "got: {v}");
        unsafe { graph_free(g) };
    }
}
