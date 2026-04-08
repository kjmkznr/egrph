package egrph

import (
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
