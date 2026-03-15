package egrph

/*
#cgo LDFLAGS: -L../egrph-c-abi -legrph
#include <stdlib.h>

typedef struct CGraph CGraph;

CGraph* graph_new();
void graph_free(CGraph* ptr);
unsigned long long graph_create_node(CGraph* ptr);
long long graph_create_edge(CGraph* ptr, const char* label, unsigned long long src, unsigned long long dst);
size_t graph_get_node_count(const CGraph* ptr);
size_t graph_get_edge_count(const CGraph* ptr);
*/
import "C"
import "unsafe"

type Graph struct {
	ptr *C.CGraph
}

func NewGraph() *Graph {
	return &Graph{ptr: C.graph_new()}
}

func (g *Graph) Free() {
	C.graph_free(g.ptr)
}

func (g *Graph) CreateNode() uint64 {
	return uint64(C.graph_create_node(g.ptr))
}

func (g *Graph) CreateEdge(label string, src uint64, dst uint64) int64 {
	cLabel := C.CString(label)
	defer C.free(unsafe.Pointer(cLabel))
	return int64(C.graph_create_edge(g.ptr, cLabel, C.ulonglong(src), C.ulonglong(dst)))
}

func (g *Graph) GetNodeCount() int {
	return int(C.graph_get_node_count(g.ptr))
}

func (g *Graph) GetEdgeCount() int {
	return int(C.graph_get_edge_count(g.ptr))
}
