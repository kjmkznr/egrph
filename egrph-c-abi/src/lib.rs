use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use egrph_core::{Graph, NodeId, EdgeId, PropertyValue};

pub struct CGraph {
    graph: Graph,
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_new() -> *mut CGraph {
    Box::into_raw(Box::new(CGraph {
        graph: Graph::new(),
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_free(ptr: *mut CGraph) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_create_node(ptr: *mut CGraph) -> NodeId {
    let c_graph = unsafe { &mut *ptr };
    c_graph.graph.create_node(vec![], HashMap::new())
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_create_edge(ptr: *mut CGraph, label: *const c_char, src: NodeId, dst: NodeId) -> i64 {
    let c_graph = unsafe { &mut *ptr };
    let c_label = unsafe { CStr::from_ptr(label) }.to_string_lossy().into_owned();
    match c_graph.graph.create_edge(c_label, src, dst, HashMap::new()) {
        Ok(id) => id as i64,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_get_node_count(ptr: *const CGraph) -> usize {
    let c_graph = unsafe { &*ptr };
    c_graph.graph.nodes.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn graph_get_edge_count(ptr: *const CGraph) -> usize {
    let c_graph = unsafe { &*ptr };
    c_graph.graph.edges.len()
}
