package egrph

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestGraph(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	n1 := g.createNode()
	n2 := g.createNode()
	e := g.CreateEdge("KNOWS", n1, n2)

	if g.GetNodeCount() != 2 {
		t.Errorf("expected 2 nodes, got %d", g.GetNodeCount())
	}
	if g.GetEdgeCount() != 1 {
		t.Errorf("expected 1 edge, got %d", g.GetEdgeCount())
	}
	if e == -1 {
		t.Error("failed to create edge")
	}
}

func (g *Graph) createNode() uint64 {
	return g.CreateNode()
}

func TestExportCypher(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	n1 := g.CreateNode()
	n2 := g.CreateNode()
	g.CreateEdge("KNOWS", n1, n2)

	cypher := g.ExportCypher()
	if cypher == "" {
		t.Error("expected non-empty Cypher output")
	}
	if !contains(cypher, "CREATE") {
		t.Errorf("expected CREATE in output, got: %s", cypher)
	}
	if !contains(cypher, "-[:KNOWS]->") {
		t.Errorf("expected relationship in output, got: %s", cypher)
	}
}

func contains(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || len(s) > 0 && containsStr(s, substr))
}

func containsStr(s, substr string) bool {
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}

type execResult struct {
	Columns []string        `json:"columns"`
	Rows    [][]interface{} `json:"rows"`
}

func decodeResult(t *testing.T, raw string) execResult {
	t.Helper()
	var r execResult
	if err := json.Unmarshal([]byte(raw), &r); err != nil {
		t.Fatalf("unmarshal result %q: %v", raw, err)
	}
	return r
}

func TestExecute(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	raw, err := g.Execute("CREATE (n:Foo {name: 'bar'}) RETURN n.name AS x")
	if err != nil {
		t.Fatalf("Execute failed: %v", err)
	}
	r := decodeResult(t, raw)
	if len(r.Columns) != 1 || r.Columns[0] != "x" {
		t.Errorf("unexpected columns: %v", r.Columns)
	}
	if len(r.Rows) != 1 || r.Rows[0][0] != "bar" {
		t.Errorf("unexpected rows: %v", r.Rows)
	}
}

func TestExecuteSyntaxError(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	_, err := g.Execute("THIS IS NOT CYPHER")
	if err == nil {
		t.Fatal("expected syntax error, got nil")
	}
}

func TestExecuteWithParamsIntegerRoundtrip(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	raw, err := g.ExecuteWithParams(
		"CREATE (n:Log {id: $id, ts: $ts}) RETURN n.id AS id, n.ts AS ts",
		map[string]interface{}{"id": "u1", "ts": int64(1715600000000)},
	)
	if err != nil {
		t.Fatalf("ExecuteWithParams failed: %v", err)
	}
	r := decodeResult(t, raw)
	if r.Rows[0][0] != "u1" {
		t.Errorf("expected id=u1, got %v", r.Rows[0][0])
	}
	// JSON numbers decode to float64 by default; verify the value matches.
	if got, ok := r.Rows[0][1].(float64); !ok || int64(got) != 1715600000000 {
		t.Errorf("expected ts=1715600000000, got %v (%T)", r.Rows[0][1], r.Rows[0][1])
	}
}

func TestExecuteWithParamsEscapesQuotes(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	raw, err := g.ExecuteWithParams(
		"CREATE (n:U {name: $name}) RETURN n.name AS name",
		map[string]interface{}{"name": "O'Reilly"},
	)
	if err != nil {
		t.Fatalf("ExecuteWithParams failed: %v", err)
	}
	r := decodeResult(t, raw)
	if r.Rows[0][0] != "O'Reilly" {
		t.Errorf("expected O'Reilly, got %v", r.Rows[0][0])
	}
}

func TestExecuteWithParamsUnwindBatch(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	rows := []map[string]interface{}{
		{"id": "a", "ts": int64(1)},
		{"id": "b", "ts": int64(2)},
		{"id": "c", "ts": int64(3)},
	}
	raw, err := g.ExecuteWithParams(
		"UNWIND $rows AS r CREATE (n:Log {id: r.id, ts: r.ts}) RETURN count(n) AS c",
		map[string]interface{}{"rows": rows},
	)
	if err != nil {
		t.Fatalf("ExecuteWithParams failed: %v", err)
	}
	r := decodeResult(t, raw)
	if got, ok := r.Rows[0][0].(float64); !ok || int64(got) != 3 {
		t.Errorf("expected count=3, got %v", r.Rows[0][0])
	}
	if g.GetNodeCount() != 3 {
		t.Errorf("expected 3 nodes, got %d", g.GetNodeCount())
	}
}

func TestExecuteWithParamsNilMap(t *testing.T) {
	g := NewGraph()
	defer g.Free()

	raw, err := g.ExecuteWithParams("RETURN 1 AS one", nil)
	if err != nil {
		t.Fatalf("ExecuteWithParams(nil) failed: %v", err)
	}
	if !strings.Contains(raw, `"columns":["one"]`) {
		t.Errorf("unexpected result: %s", raw)
	}
}
