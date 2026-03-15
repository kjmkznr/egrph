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
