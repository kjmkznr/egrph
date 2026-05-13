package egrph

/*
#cgo LDFLAGS: -L../egrph-c-abi -legrph_c
#include <stdlib.h>

typedef struct CGraph CGraph;

CGraph* graph_new();
void graph_free(CGraph* ptr);
unsigned long long graph_create_node(CGraph* ptr);
long long graph_create_edge(CGraph* ptr, const char* label, unsigned long long src, unsigned long long dst);
size_t graph_get_node_count(const CGraph* ptr);
size_t graph_get_edge_count(const CGraph* ptr);
char* graph_execute(CGraph* ptr, const char* query);
char* graph_execute_with_params(CGraph* ptr, const char* query, const char* params_json);
char* graph_export_cypher(const CGraph* ptr);
void graph_free_string(char* s);
*/
import "C"

import (
	"encoding/json"
	"errors"
	"strings"
	"unsafe"
)

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

func (g *Graph) ExportCypher() string {
	cStr := C.graph_export_cypher(g.ptr)
	defer C.graph_free_string(cStr)
	return C.GoString(cStr)
}

// Execute runs a Cypher query and returns the raw result JSON
// (`{"columns":[...],"rows":[[...]]}`). If the underlying engine reports an
// error, it is returned as a Go error and the JSON is empty.
func (g *Graph) Execute(query string) (string, error) {
	cQ := C.CString(query)
	defer C.free(unsafe.Pointer(cQ))
	cStr := C.graph_execute(g.ptr, cQ)
	defer C.graph_free_string(cStr)
	return parseExecuteResult(C.GoString(cStr))
}

// ExecuteWithParams runs a Cypher query with named parameters ($name syntax).
// `params` is marshaled to JSON; supported value kinds are null, bool, numbers,
// strings, slices, and maps with string keys. Returns the same raw result JSON
// as Execute.
func (g *Graph) ExecuteWithParams(query string, params map[string]interface{}) (string, error) {
	if params == nil {
		params = map[string]interface{}{}
	}
	paramsJSON, err := json.Marshal(params)
	if err != nil {
		return "", err
	}
	cQ := C.CString(query)
	defer C.free(unsafe.Pointer(cQ))
	cP := C.CString(string(paramsJSON))
	defer C.free(unsafe.Pointer(cP))
	cStr := C.graph_execute_with_params(g.ptr, cQ, cP)
	defer C.graph_free_string(cStr)
	return parseExecuteResult(C.GoString(cStr))
}

// parseExecuteResult turns the FFI return string into (json, error). The C ABI
// signals errors with a top-level `{"error": "..."}` object; everything else is
// passed through verbatim.
func parseExecuteResult(s string) (string, error) {
	if strings.HasPrefix(s, `{"error":`) {
		var e struct {
			Error string `json:"error"`
		}
		if err := json.Unmarshal([]byte(s), &e); err == nil && e.Error != "" {
			return "", errors.New(e.Error)
		}
		return "", errors.New(s)
	}
	return s, nil
}
