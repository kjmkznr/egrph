# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Egrph** is a lightweight graph database library written in Rust, providing core logic in Rust with bindings for Python, Go, and WebAssembly via C ABI. It supports **Cypher query language** for graph traversal and manipulation.

## Repository Structure

```
egrph/
├── egrph-core/          # Core Rust implementation (parser, planner, executor, storage)
├── egrph-python/        # Python bindings via PyO3
├── egrph-c-abi/         # C ABI bridge for other language bindings
├── egrph-wasm/         # WebAssembly bindings
├── egrph-go/           # Go bindings (calls via cgo to C ABI)
└── README.md
```

## Common Commands

### Build all packages
```bash
cargo build --release
```

### Format check
```bash
cargo fmt --all -- --check
```

### Lint with Clippy
```bash
cargo clippy --workspace --exclude egrph-wasm -D warnings
```

### Run all tests
```bash
cargo test --workspace --exclude egrph-wasm
```

### Run specific test (example: any_predicate)
```bash
cargo test --package egrph-core test_any_predicate
```

### Run a single test with grep pattern
```bash
cargo test --package egrph-core -- test_optional_match_no_match
```

### Build Python bindings (using maturin)
```bash
cd egrph-python
maturin build --release
# or for local development
maturin develop
```

### Build C ABI static library
```bash
cd egrph-c-abi
cargo build --release
```

### Build Go bindings
```bash
cd egrph-go
go test .
```

### Build WebAssembly module
```bash
cd egrph-wasm
wasm-pack build --target nodejs
```

## Architecture

### Core Module (`egrph-core/src/`)

The core module implements a full Cypher query engine with the following architecture:

**Key Components:**

1. **AST (`src/ast/mod.rs`)** - Abstract Syntax Tree representing parsed queries
   - `Statement`: Envelopes queries with clause list, column names, and row capacity
   - `Clause`: Top-level operations (CREATE, MATCH, RETURN, WITH, etc.)
   - `PatternElement`: Node and relationship patterns in Cypher queries

2. **Parser (`src/parser/`)** - Uses PEST for recursive descent parsing of Cypher
   - Parser grammar defined in `parser/cypher.pest` supports:
     - CREATE/DELETE/MERGE clauses with pattern matching
     - MATCH with optional match support
     - Complex expression handling (arithmetic, comparisons, logic operators)
     - List comprehension and reduce expressions
     - Case expressions for conditional evaluation
   - Parser produces AST from Cypher queries

3. **Planner (`src/planner/`)** - Translates AST to executable plans
   - `Phase` enum: CREATE → MATCH → WITH → RETURN pipeline
   - Handles clause ordering, variable binding propagation across clauses
   - Resolves pattern matches and prepares execution for the executor
   - Critical: Tracks which variables exist at each phase with `VariableSet`

4. **Executor (`src/executor/`)** - Executes plans against storage
   - `src/executor/mod.rs`: Main execution orchestration
     - `execute_plan()`: Orchestrates phase-by-phase execution
     - `execute_phase()`: Runs one logical phase (CREATE, MATCH, etc.)
   - `src/executor/expression.rs`: Expression evaluation utilities
     - Implements function implementations for Cypher functions
     - Handles comparison operators and arithmetic operations
   - `src/executor/aggregation.rs`: Aggregation functions

5. **Storage (`src/graph/storage.rs`)** - In-memory graph storage
   - `GraphStorage`: Thread-safe in-memory graph with HashMap backends
     - `nodes`: Map<NodeId, Node>
     - `edges`: Map<EdgeId, Edge>
     - Indexes nodes by label for efficient MATCH operations
     - Tracks ID generation state (next available NodeId, EdgeId)

### Data Types (`src/graph/types.rs`)

Core type definitions shared across the system:
- `NodeId` / `EdgeId`: UInt64 identifiers
- `PropertyValue`: String | Int | Float | Bool
- `Node`: id, labels Vec<String>, properties HashMap
- `Edge`: id, label, src, dst, properties
- `Path`: Sequence of nodes and relationships
- `CypherValue`: Result type for query execution (Null, Boolean, Integer, Float, String, List, Map, Node, Relationship, Path)

### Public API (`egrph-core/src/lib.rs`)

Exports the main `Graph` struct and types:
- `Graph::execute()`: Parse and execute a Cypher query (primary entry point)
- `Graph::query()`: Legacy API (deprecated, uses execute internally)
- Direct APIs preserved: create_node(), create_edge(), get_node(), etc.

## Key Implementation Patterns

1. **Parser → Planner → Executor Pipeline**: Query execution follows strict separation of concerns
2. **Variable Propagation**: Planner tracks variable availability across query phases
3. **Pattern Matching**: Storage indexes nodes by label for efficient MATCH resolution
4. **Result Row Structure**: CREATE without RETURN produces internal rows (not projected as columns)

## Testing Approach

All tests in `egrph-core/src/lib.rs` follow a pattern:
```rust
#[test]
fn test_<name>() {
    let mut g = Graph::new();
    // Setup: create initial graph state via execute()
    let result = g.execute("CYpher query").unwrap();
    assert_eq!(result.rows.len(), expected);
    // Validate returned values structure
}
```

Tests cover CREATE, MATCH, WHERE, ORDER BY, SKIP/LIMIT, all Cypher functions, and special clauses.

## Important Design Decisions

- **Single execution method**: `execute()` handles parsing+planning+execution together
- **Phase-based planning**: Queries flow through logical phases (CREATE→MATCH→WITH→RETURN)
- **Label-indexed storage**: O(1) node lookup by label for MATCH clauses
- **PEST recursive descent**: Chainless parser for efficient Cypher grammar processing
