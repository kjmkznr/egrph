pub mod ast;
pub mod error;
pub mod executor;
pub mod graph;
pub mod parser;
pub mod planner;

// Re-export primary types for public API
pub use error::CypherError;
pub use executor::result::QueryResult;
pub use graph::Graph;
pub use graph::backend::StorageBackend;
pub use graph::storage::MemoryStorage;
pub use graph::types::{CypherValue, Edge, EdgeId, Node, NodeId, Path, PropertyValue};

/// Default in-memory graph (backward-compatible alias).
pub type InMemoryGraph = graph::Graph<MemoryStorage>;

#[cfg(feature = "sled-storage")]
pub use graph::sled_storage::SledStorage;

/// Persistent graph backed by sled (requires the `sled-storage` feature).
#[cfg(feature = "sled-storage")]
pub type PersistentGraph = graph::Graph<SledStorage>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_cypher_query() {
        let mut g = Graph::new();
        #[allow(deprecated)]
        {
            g.query("CREATE (:Person {name: \"Alice\", age: 30})")
                .unwrap();
            g.query("CREATE (:Person {name: \"Bob\", age: 25})")
                .unwrap();

            let results = g.query("MATCH (p:Person) RETURN p").unwrap();
            assert_eq!(results.len(), 2);

            let n1 = g.get_node(results[0]).unwrap();
            assert!(n1.labels.contains(&"Person".to_string()));
        }
    }

    #[test]
    fn test_graph_ops() {
        let mut g = Graph::new();
        let n1 = g.create_node(vec!["Person".to_string()], HashMap::new());
        let n2 = g.create_node(vec!["Person".to_string()], HashMap::new());
        let e1 = g
            .create_edge("KNOWS".to_string(), n1, n2, HashMap::new())
            .unwrap();

        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert_eq!(g.get_edge(e1).unwrap().src, n1);
    }

    #[test]
    fn test_execute_api() {
        let mut g = Graph::new();
        let result = g
            .execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        assert_eq!(result.rows.len(), 1);

        let result = g
            .execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        assert_eq!(result.rows.len(), 1);

        let result = g.execute("MATCH (p:Person) RETURN p").unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_parser_produces_ast() {
        let stmt =
            parser::parse_with_return_extraction("CREATE (:Person {name: \"Alice\", age: 30})")
                .unwrap();

        match stmt {
            ast::Statement::Query(q) => {
                assert_eq!(q.clauses.len(), 1);
                match &q.clauses[0] {
                    ast::Clause::Create(c) => {
                        let part = &c.pattern.parts[0];
                        match &part.element {
                            ast::PatternElement::Node(np) => {
                                assert_eq!(np.labels, vec!["Person"]);
                                assert!(np.properties.is_some());
                            }
                            _ => panic!("Expected node pattern"),
                        }
                    }
                    _ => panic!("Expected Create clause"),
                }
            }
            _ => panic!("Expected single Query statement"),
        }
    }

    // --- Phase 1 tests ---

    #[test]
    fn test_create_multiple_nodes_in_one_query() {
        let mut g = Graph::new();
        g.execute(
            "CREATE (:Person {name: \"Alice\", age: 30})\n\
             CREATE (:Person {name: \"Bob\", age: 25})\n\
             CREATE (:Person {name: \"Carol\", age: 35})",
        )
        .unwrap();
        assert_eq!(g.node_count(), 3);
    }

    #[test]
    fn test_create_relationship() {
        let mut g = Graph::new();
        let result = g
            .execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();
        // Should create 2 nodes and 1 edge. CREATE without RETURN produces
        // exactly 1 row per path (carrying bound variables, no projected columns).
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_match_relationship() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();

        let result = g
            .execute("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a, b")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.columns[0], "a");
        assert_eq!(result.columns[1], "b");
    }

    #[test]
    fn test_where_clause() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.age > 28 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 2); // Alice (30) and Charlie (35)
    }

    #[test]
    fn test_where_with_and() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.age > 28 AND n.age < 33 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 1); // Only Alice (30)
    }

    #[test]
    fn test_order_by() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN n.name ORDER BY n.age")
            .unwrap();
        assert_eq!(result.rows.len(), 3);
        // Should be sorted by age ascending: Bob(25), Alice(30), Charlie(35)
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Bob"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[2].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Charlie"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_order_by_desc() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN n.name ORDER BY n.age DESC")
            .unwrap();
        assert_eq!(result.rows.len(), 3);
        // Should be sorted descending: Charlie(35), Alice(30), Bob(25)
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Charlie"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[2].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Bob"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_skip() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN n.name ORDER BY n.age SKIP 1")
            .unwrap();
        assert_eq!(result.rows.len(), 2); // Skipped Bob(25)
    }

    #[test]
    fn test_limit() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g.execute("MATCH (n:Person) RETURN n LIMIT 2").unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_skip_and_limit() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN n.name ORDER BY n.age SKIP 1 LIMIT 1")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        // After sorting by age and skipping 1, should get Alice(30)
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_return_property_access() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();

        let result = g.execute("MATCH (n:Person) RETURN n.name, n.age").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.columns.len(), 2);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[0].values[1] {
            CypherValue::Integer(i) => assert_eq!(*i, 30),
            other => panic!("Expected Integer, got {:?}", other),
        }
    }

    #[test]
    fn test_expression_arithmetic() {
        let mut g = Graph::new();
        g.execute("CREATE (:Num {val: 10})").unwrap();

        let result = g.execute("MATCH (n:Num) RETURN n.val + 5").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 15),
            other => panic!("Expected Integer(15), got {:?}", other),
        }
    }

    #[test]
    fn test_where_comparison_operators() {
        let mut g = Graph::new();
        g.execute("CREATE (:Num {val: 10})").unwrap();
        g.execute("CREATE (:Num {val: 20})").unwrap();
        g.execute("CREATE (:Num {val: 30})").unwrap();

        // Test >=
        let result = g
            .execute("MATCH (n:Num) WHERE n.val >= 20 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 2);

        // Test =
        let result = g
            .execute("MATCH (n:Num) WHERE n.val = 20 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 1);

        // Test <>
        let result = g
            .execute("MATCH (n:Num) WHERE n.val <> 20 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_where_or() {
        let mut g = Graph::new();
        g.execute("CREATE (:Num {val: 10})").unwrap();
        g.execute("CREATE (:Num {val: 20})").unwrap();
        g.execute("CREATE (:Num {val: 30})").unwrap();

        let result = g
            .execute("MATCH (n:Num) WHERE n.val = 10 OR n.val = 30 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_match_with_relationship_type_filter() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();
        g.execute("CREATE (c:Person {name: \"Charlie\"})-[:LIKES]->(d:Person {name: \"Dave\"})")
            .unwrap();

        // Only KNOWS relationships
        let result = g
            .execute("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a, b")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_return_with_alias() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN n.name AS person_name")
            .unwrap();
        assert_eq!(result.columns[0], "person_name");
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_multi_clause_query() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();
        g.execute("CREATE (c:Person {name: \"Charlie\"})-[:KNOWS]->(d:Person {name: \"Dave\"})")
            .unwrap();

        // MATCH + WHERE + RETURN with ORDER BY
        let result = g
            .execute("MATCH (a:Person)-[:KNOWS]->(b:Person) WHERE b.name = \"Bob\" RETURN a.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    // --- Phase 2 tests ---

    #[test]
    fn test_string_starts_with() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Amanda\"})").unwrap();

        let result = g
            .execute(
                "MATCH (n:Person) WHERE n.name STARTS WITH \"A\" RETURN n.name ORDER BY n.name",
            )
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[1].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Amanda"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_string_ends_with() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Grace\"})").unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.name ENDS WITH \"ce\" RETURN n.name ORDER BY n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2); // Alice, Grace
    }

    #[test]
    fn test_string_contains() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\"})").unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.name CONTAINS \"li\" RETURN n.name ORDER BY n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2); // Alice, Charlie
    }

    #[test]
    fn test_regex_match() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Amanda\"})").unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.name =~ \"A.*\" RETURN n.name ORDER BY n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2); // Alice, Amanda
    }

    #[test]
    fn test_in_operator() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\"})").unwrap();

        let result = g.execute(
            "MATCH (n:Person) WHERE n.name IN [\"Alice\", \"Charlie\"] RETURN n.name ORDER BY n.name"
        ).unwrap();
        assert_eq!(result.rows.len(), 2);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[1].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Charlie"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_case_simple() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 15})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 45})")
            .unwrap();

        let result = g.execute(
            "MATCH (n:Person) RETURN n.name, CASE WHEN n.age < 18 THEN \"minor\" WHEN n.age >= 18 THEN \"adult\" END AS category ORDER BY n.name"
        ).unwrap();
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.columns[1], "category");
        // Alice = adult, Bob = minor, Charlie = adult
        match &result.rows[0].values[1] {
            CypherValue::String(s) => assert_eq!(s, "adult"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[1].values[1] {
            CypherValue::String(s) => assert_eq!(s, "minor"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_case_with_else() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 15})")
            .unwrap();

        let result = g.execute(
            "MATCH (n:Person) RETURN n.name, CASE WHEN n.age < 18 THEN \"minor\" ELSE \"adult\" END AS category ORDER BY n.name"
        ).unwrap();
        assert_eq!(result.rows.len(), 2);
        match &result.rows[0].values[1] {
            CypherValue::String(s) => assert_eq!(s, "adult"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[1].values[1] {
            CypherValue::String(s) => assert_eq!(s, "minor"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_string_functions() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: \"  Hello World  \"})")
            .unwrap();

        let result = g.execute(
            "MATCH (n:Data) RETURN trim(n.val) AS trimmed, toUpper(n.val) AS upper, size(n.val) AS len"
        ).unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Hello World"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[0].values[1] {
            CypherValue::String(s) => assert_eq!(s, "  HELLO WORLD  "),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[0].values[2] {
            CypherValue::Integer(i) => assert_eq!(*i, 15),
            other => panic!("Expected Integer, got {:?}", other),
        }
    }

    #[test]
    fn test_math_functions() {
        let mut g = Graph::new();
        g.execute("CREATE (:Num {val: -5})").unwrap();

        let result = g
            .execute("MATCH (n:Num) RETURN abs(n.val), ceil(4.3), floor(4.7), round(4.5)")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 5),
            other => panic!("Expected Integer(5), got {:?}", other),
        }
        match &result.rows[0].values[1] {
            CypherValue::Float(f) => assert_eq!(*f, 5.0),
            other => panic!("Expected Float(5.0), got {:?}", other),
        }
        match &result.rows[0].values[2] {
            CypherValue::Float(f) => assert_eq!(*f, 4.0),
            other => panic!("Expected Float(4.0), got {:?}", other),
        }
        match &result.rows[0].values[3] {
            CypherValue::Float(f) => assert_eq!(*f, 5.0),
            other => panic!("Expected Float(5.0), got {:?}", other),
        }
    }

    #[test]
    fn test_list_functions() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: 1})").unwrap();

        let result = g.execute(
            "MATCH (n:Data) RETURN head([1, 2, 3]), last([1, 2, 3]), tail([1, 2, 3]), size([1, 2, 3])"
        ).unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 1),
            other => panic!("Expected Integer(1), got {:?}", other),
        }
        match &result.rows[0].values[1] {
            CypherValue::Integer(i) => assert_eq!(*i, 3),
            other => panic!("Expected Integer(3), got {:?}", other),
        }
        match &result.rows[0].values[2] {
            CypherValue::List(l) => assert_eq!(l.len(), 2),
            other => panic!("Expected List, got {:?}", other),
        }
        match &result.rows[0].values[3] {
            CypherValue::Integer(i) => assert_eq!(*i, 3),
            other => panic!("Expected Integer(3), got {:?}", other),
        }
    }

    #[test]
    fn test_range_function() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: 1})").unwrap();

        let result = g.execute("MATCH (n:Data) RETURN range(1, 5)").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::List(l) => {
                assert_eq!(l.len(), 5);
                match &l[0] {
                    CypherValue::Integer(i) => assert_eq!(*i, 1),
                    other => panic!("Expected Integer, got {:?}", other),
                }
                match &l[4] {
                    CypherValue::Integer(i) => assert_eq!(*i, 5),
                    other => panic!("Expected Integer, got {:?}", other),
                }
            }
            other => panic!("Expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_is_null_is_not_null() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();

        // Alice has no age property, so n.age IS NULL should match Alice
        let result = g
            .execute("MATCH (n:Person) WHERE n.age IS NULL RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }

        // Bob has age, so n.age IS NOT NULL should match Bob
        let result = g
            .execute("MATCH (n:Person) WHERE n.age IS NOT NULL RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Bob"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_coalesce_function() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN coalesce(n.age, 0) AS age")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 0),
            other => panic!("Expected Integer(0), got {:?}", other),
        }
    }

    #[test]
    fn test_single_quote_string() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice'})").unwrap();

        let result = g.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_list_concat() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: 1})").unwrap();

        let result = g.execute("MATCH (n:Data) RETURN [1, 2] + [3, 4]").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::List(l) => {
                assert_eq!(l.len(), 4);
            }
            other => panic!("Expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_string_concat() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {first: \"Alice\", last: \"Smith\"})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) RETURN n.first + \" \" + n.last AS full_name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice Smith"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_type_conversion() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: 42})").unwrap();

        let result = g
            .execute("MATCH (n:Data) RETURN toString(n.val), toFloat(n.val), toInteger(\"123\")")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "42"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[0].values[1] {
            CypherValue::Float(f) => assert_eq!(*f, 42.0),
            other => panic!("Expected Float, got {:?}", other),
        }
        match &result.rows[0].values[2] {
            CypherValue::Integer(i) => assert_eq!(*i, 123),
            other => panic!("Expected Integer, got {:?}", other),
        }
    }

    #[test]
    fn test_replace_function() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: \"hello world\"})").unwrap();

        let result = g
            .execute("MATCH (n:Data) RETURN replace(n.val, \"world\", \"Cypher\")")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "hello Cypher"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_split_function() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: \"a,b,c\"})").unwrap();

        let result = g
            .execute("MATCH (n:Data) RETURN split(n.val, \",\")")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::List(l) => {
                assert_eq!(l.len(), 3);
                match &l[0] {
                    CypherValue::String(s) => assert_eq!(s, "a"),
                    other => panic!("Expected String, got {:?}", other),
                }
            }
            other => panic!("Expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_null_three_value_logic() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: 1})").unwrap();

        // null AND true = null, null AND false = false
        // Testing via filter: n.missing should be null
        let result = g
            .execute("MATCH (n:Data) WHERE n.val = 1 AND n.missing = 1 RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 0); // null comparison = null, not true

        // Testing null comparison: null = null should be null (not true)
        let result = g
            .execute("MATCH (n:Data) WHERE n.missing = n.other RETURN n")
            .unwrap();
        assert_eq!(result.rows.len(), 0); // null = null => null => not truthy
    }

    // --- Phase 3 tests ---

    #[test]
    fn test_with_clause() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WITH n.name AS name, n.age AS age ORDER BY age RETURN name")
            .unwrap();
        assert_eq!(result.rows.len(), 3);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Bob"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_with_where() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g.execute(
            "MATCH (n:Person) WITH n.name AS name, n.age AS age WHERE age > 28 RETURN name ORDER BY name"
        ).unwrap();
        assert_eq!(result.rows.len(), 2); // Alice(30), Charlie(35)
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_with_distinct() {
        let mut g = Graph::new();
        g.execute("CREATE (:Item {category: \"A\", val: 1})")
            .unwrap();
        g.execute("CREATE (:Item {category: \"A\", val: 2})")
            .unwrap();
        g.execute("CREATE (:Item {category: \"B\", val: 3})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Item) WITH DISTINCT n.category AS cat RETURN cat ORDER BY cat")
            .unwrap();
        assert_eq!(result.rows.len(), 2); // "A" and "B"
    }

    #[test]
    fn test_unwind_list() {
        let mut g = Graph::new();
        g.execute("CREATE (:Data {val: 1})").unwrap();

        let result = g
            .execute("MATCH (n:Data) UNWIND [1, 2, 3] AS x RETURN x")
            .unwrap();
        assert_eq!(result.rows.len(), 3);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 1),
            other => panic!("Expected Integer, got {:?}", other),
        }
        match &result.rows[2].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 3),
            other => panic!("Expected Integer, got {:?}", other),
        }
    }

    #[test]
    fn test_unwind_no_match() {
        let mut g = Graph::new();
        // UNWIND without prior match (from empty row)
        let result = g.execute("UNWIND [10, 20, 30] AS x RETURN x").unwrap();
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn test_unwind_create_with_expressions() {
        let mut g = Graph::new();
        g.execute(
            "UNWIND range(1, 5) AS i
             CREATE (:Person {
               gnId: 'node-' + toString(i),
               name: 'Person ' + toString(i),
               age:  (i % 80) + 20,
               group: i % 3
             })",
        )
        .unwrap();

        let result = g
            .execute("MATCH (p:Person) RETURN p.gnId, p.name, p.age, p.group ORDER BY p.gnId")
            .unwrap();
        assert_eq!(result.rows.len(), 5);

        // First row: i=1
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "node-1"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[0].values[1] {
            CypherValue::String(s) => assert_eq!(s, "Person 1"),
            other => panic!("Expected String, got {:?}", other),
        }
        match &result.rows[0].values[2] {
            CypherValue::Integer(i) => assert_eq!(*i, 21), // (1 % 80) + 20
            other => panic!("Expected Integer, got {:?}", other),
        }
        match &result.rows[0].values[3] {
            CypherValue::Integer(i) => assert_eq!(*i, 1), // 1 % 3
            other => panic!("Expected Integer, got {:?}", other),
        }
    }

    #[test]
    fn test_set_property() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" SET n.age = 31")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.name = \"Alice\" RETURN n.age")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 31),
            other => panic!("Expected Integer(31), got {:?}", other),
        }
    }

    #[test]
    fn test_set_new_property() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" SET n.email = \"alice@example.com\"")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.name = \"Alice\" RETURN n.email")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "alice@example.com"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_set_label() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" SET n:Employee")
            .unwrap();

        // Should now be findable by :Employee label
        let result = g.execute("MATCH (n:Employee) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_property() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" REMOVE n.age")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WHERE n.name = \"Alice\" RETURN n.age")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Null => {} // age was removed
            other => panic!("Expected Null, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_label() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person:Employee {name: \"Alice\"})")
            .unwrap();

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" REMOVE n:Employee")
            .unwrap();

        // Should no longer be findable by :Employee label
        let result = g.execute("MATCH (n:Employee) RETURN n").unwrap();
        assert_eq!(result.rows.len(), 0);

        // But still findable by :Person
        let result = g.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_delete_node() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        assert_eq!(g.node_count(), 2);

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" DELETE n")
            .unwrap();

        assert_eq!(g.node_count(), 1);
        let result = g.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Bob"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_delete_node_with_relationship_fails() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();

        // Deleting a connected node without DETACH should fail
        let result = g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" DELETE n");
        assert!(result.is_err());
    }

    #[test]
    fn test_detach_delete() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);

        g.execute("MATCH (n:Person) WHERE n.name = \"Alice\" DETACH DELETE n")
            .unwrap();

        assert_eq!(g.node_count(), 1);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn test_merge_create() {
        let mut g = Graph::new();

        // MERGE should create when not found
        g.execute("MERGE (n:Person {name: \"Alice\"})").unwrap();
        assert_eq!(g.node_count(), 1);

        let result = g.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::String(s) => assert_eq!(s, "Alice"),
            other => panic!("Expected String, got {:?}", other),
        }
    }

    #[test]
    fn test_merge_match() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();

        // MERGE should find existing and not create duplicate
        g.execute("MERGE (n:Person {name: \"Alice\"})").unwrap();
        assert_eq!(g.node_count(), 1); // Still just 1

        let result = g.execute("MATCH (n:Person) RETURN n.name").unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_merge_on_create() {
        let mut g = Graph::new();

        g.execute("MERGE (n:Person {name: \"Alice\"}) ON CREATE SET n.created = true")
            .unwrap();
        assert_eq!(g.node_count(), 1);

        let result = g
            .execute("MATCH (n:Person) WHERE n.name = \"Alice\" RETURN n.created")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Boolean(b) => assert!(*b),
            other => panic!("Expected Boolean(true), got {:?}", other),
        }
    }

    #[test]
    fn test_merge_on_match() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", visits: 0})")
            .unwrap();

        g.execute("MERGE (n:Person {name: \"Alice\"}) ON MATCH SET n.visits = 1")
            .unwrap();
        assert_eq!(g.node_count(), 1);

        let result = g
            .execute("MATCH (n:Person) WHERE n.name = \"Alice\" RETURN n.visits")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 1),
            other => panic!("Expected Integer(1), got {:?}", other),
        }
    }

    #[test]
    fn test_merge_chain_creates_new_path() {
        let mut g = Graph::new();
        g.execute("MERGE (a:Person {name: \"A\"})-[:KNOWS]->(b:Person {name: \"B\"})")
            .unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);

        let result = g
            .execute("MATCH (a:Person {name: \"A\"})-[r:KNOWS]->(b:Person {name: \"B\"}) RETURN r")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_merge_chain_matches_existing_path() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"A\"})-[:KNOWS]->(:Person {name: \"B\"})")
            .unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);

        g.execute("MERGE (a:Person {name: \"A\"})-[:KNOWS]->(b:Person {name: \"B\"})")
            .unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn test_merge_chain_reuses_bound_start() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"A\"})").unwrap();
        g.execute("CREATE (:Person {name: \"A\"})").unwrap();
        assert_eq!(g.node_count(), 2);

        g.execute("MATCH (a:Person {name: \"A\"}) MERGE (a)-[:KNOWS]->(b:Person {name: \"B\"})")
            .unwrap();

        // Two A nodes, each gets a new B and a new edge.
        assert_eq!(g.node_count(), 4);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn test_merge_chain_reuses_bound_endpoints_creates_edge() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"A\"})").unwrap();
        g.execute("CREATE (:Person {name: \"B\"})").unwrap();
        assert_eq!(g.edge_count(), 0);

        g.execute(
            "MATCH (a:Person {name: \"A\"}), (b:Person {name: \"B\"}) MERGE (a)-[:KNOWS]->(b)",
        )
        .unwrap();

        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn test_merge_chain_reuses_bound_endpoints_no_duplicate() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"A\"})-[:KNOWS]->(:Person {name: \"B\"})")
            .unwrap();
        assert_eq!(g.edge_count(), 1);

        g.execute(
            "MATCH (a:Person {name: \"A\"}), (b:Person {name: \"B\"}) MERGE (a)-[:KNOWS]->(b)",
        )
        .unwrap();

        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
    }

    #[test]
    fn test_merge_chain_on_create_only_fires_on_create() {
        let mut g = Graph::new();
        g.execute(
            "MERGE (a:Person {name: \"A\"})-[r:KNOWS]->(b:Person {name: \"B\"}) \
             ON CREATE SET r.w = 1 ON MATCH SET r.w = 999",
        )
        .unwrap();

        let result = g
            .execute("MATCH (:Person {name: \"A\"})-[r:KNOWS]->(:Person {name: \"B\"}) RETURN r.w")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 1),
            other => panic!("Expected Integer(1), got {:?}", other),
        }
    }

    #[test]
    fn test_merge_chain_on_match_only_fires_on_match() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"A\"})-[:KNOWS]->(:Person {name: \"B\"})")
            .unwrap();

        g.execute(
            "MERGE (a:Person {name: \"A\"})-[r:KNOWS]->(b:Person {name: \"B\"}) \
             ON CREATE SET r.w = 1 ON MATCH SET r.w = 999",
        )
        .unwrap();

        assert_eq!(g.edge_count(), 1);
        let result = g
            .execute("MATCH (:Person {name: \"A\"})-[r:KNOWS]->(:Person {name: \"B\"}) RETURN r.w")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0].values[0] {
            CypherValue::Integer(i) => assert_eq!(*i, 999),
            other => panic!("Expected Integer(999), got {:?}", other),
        }
    }

    #[test]
    fn test_merge_chain_respects_direction() {
        let mut g = Graph::new();
        // Seed an edge going the wrong way: B -> A
        g.execute("CREATE (:Person {name: \"A\"})<-[:KNOWS]-(:Person {name: \"B\"})")
            .unwrap();
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);

        // MERGE requires A -> B; the full path doesn't exist, so per
        // openCypher semantics the entire pattern is created — fresh A, fresh
        // B, fresh edge.
        g.execute("MERGE (:Person {name: \"A\"})-[:KNOWS]->(:Person {name: \"B\"})")
            .unwrap();

        assert_eq!(g.node_count(), 4);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn test_merge_chain_multiple_matches_yield_multiple_rows() {
        let mut g = Graph::new();
        g.execute("CREATE (a:A)-[:R]->(:B), (a)-[:R]->(:B)")
            .unwrap();
        assert_eq!(g.node_count(), 3);
        assert_eq!(g.edge_count(), 2);

        let result = g
            .execute("MATCH (a:A) MERGE (a)-[:R]->(b:B) RETURN b")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn test_merge_chain_rel_inline_property_filter() {
        let mut g = Graph::new();
        g.execute("CREATE (:A)-[:R {w: 1}]->(:B)").unwrap();
        assert_eq!(g.edge_count(), 1);

        // Existing edge has w:1; MERGE asks for w:2 so it must create a new path.
        g.execute("MERGE (:A)-[:R {w: 2}]->(:B)").unwrap();
        assert_eq!(g.node_count(), 4);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn test_delete_relationship() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();
        assert_eq!(g.edge_count(), 1);

        g.execute("MATCH (a:Person)-[r:KNOWS]->(b:Person) DELETE r")
            .unwrap();

        assert_eq!(g.edge_count(), 0);
        assert_eq!(g.node_count(), 2); // Nodes should still exist
    }

    #[test]
    fn test_with_limit() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Charlie\", age: 35})")
            .unwrap();

        let result = g
            .execute("MATCH (n:Person) WITH n ORDER BY n.age LIMIT 2 RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    // --- TCK Compliance Tests ---

    // Issue 3: 複数ラベルフィルター
    #[test]
    fn test_multi_label_filter() {
        let mut g = Graph::new();
        // ノードを3つ作成: A:B, A, B
        g.execute("CREATE (:A:B {name: \"both\"})").unwrap();
        g.execute("CREATE (:A {name: \"only_a\"})").unwrap();
        g.execute("CREATE (:B {name: \"only_b\"})").unwrap();

        // MATCH (n:A:B) は A と B の両方を持つノードのみを返す
        let result = g.execute("MATCH (n:A:B) RETURN n.name").unwrap();
        assert_eq!(
            result.rows.len(),
            1,
            "MATCH (n:A:B) should return only nodes with both labels"
        );
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("both".to_string())
        );
    }

    // Issue 4 & 6: toString() の型チェックと Float 表現
    #[test]
    fn test_tostring_scalar_types() {
        let mut g = Graph::new();

        // Integer
        let r = g.execute("RETURN toString(42)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::String("42".to_string()));

        // Float: 整数値のFloatは小数点付きで返される
        let r = g.execute("RETURN toString(1.0)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::String("1.0".to_string()));

        // Boolean
        let r = g.execute("RETURN toString(true)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::String("true".to_string()));

        // Null → Null
        let r = g.execute("RETURN toString(null)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Null);

        // List → Null (openCypher: TypeError)
        let r = g.execute("RETURN toString([1,2,3])").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Null);
    }

    // Issue 5: 述語関数
    #[test]
    fn test_any_predicate() {
        let mut g = Graph::new();

        let r = g.execute("RETURN any(x IN [1, 2, 3] WHERE x > 2)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(true));

        let r = g
            .execute("RETURN any(x IN [1, 2, 3] WHERE x > 10)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(false));

        // 空リスト → false
        let r = g.execute("RETURN any(x IN [] WHERE x > 0)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(false));
    }

    #[test]
    fn test_all_predicate() {
        let mut g = Graph::new();

        let r = g.execute("RETURN all(x IN [2, 3, 4] WHERE x > 1)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(true));

        let r = g.execute("RETURN all(x IN [1, 2, 3] WHERE x > 1)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(false));

        // 空リスト → true (vacuous truth)
        let r = g.execute("RETURN all(x IN [] WHERE x > 0)").unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(true));
    }

    #[test]
    fn test_none_predicate() {
        let mut g = Graph::new();

        let r = g
            .execute("RETURN none(x IN [1, 2, 3] WHERE x > 10)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(true));

        let r = g
            .execute("RETURN none(x IN [1, 2, 3] WHERE x > 2)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(false));
    }

    #[test]
    fn test_single_predicate() {
        let mut g = Graph::new();

        let r = g
            .execute("RETURN single(x IN [1, 2, 3] WHERE x = 2)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(true));

        let r = g
            .execute("RETURN single(x IN [1, 2, 3] WHERE x > 1)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Boolean(false));
    }

    #[test]
    fn test_reduce_expression() {
        let mut g = Graph::new();

        // reduce(s = 0, x IN [1, 2, 3] | s + x) = 6
        let r = g
            .execute("RETURN reduce(s = 0, x IN [1, 2, 3] | s + x)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::Integer(6));

        // reduce(s = \"\", x IN [\"a\", \"b\", \"c\"] | s + x) = \"abc\"
        let r = g
            .execute("RETURN reduce(s = \"\", x IN [\"a\", \"b\", \"c\"] | s + x)")
            .unwrap();
        assert_eq!(r.rows[0].values[0], CypherValue::String("abc".to_string()));
    }

    // Issue 1: OPTIONAL MATCH
    #[test]
    fn test_optional_match_no_match() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();

        // AliceにはKNOWSリレーションシップがないので r と b は NULL になる
        let result = g
            .execute("MATCH (a:Person) OPTIONAL MATCH (a)-[r:KNOWS]->(b) RETURN a.name, r, b")
            .unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
        assert_eq!(result.rows[0].values[1], CypherValue::Null);
        assert_eq!(result.rows[0].values[2], CypherValue::Null);
    }

    #[test]
    fn test_optional_match_with_match() {
        let mut g = Graph::new();
        g.execute("CREATE (a:Person {name: \"Alice\"})-[:KNOWS]->(b:Person {name: \"Bob\"})")
            .unwrap();

        // AliceはBobを知っているので結果が返る
        let result = g.execute(
            "MATCH (a:Person {name: \"Alice\"}) OPTIONAL MATCH (a)-[r:KNOWS]->(b) RETURN a.name, b.name"
        ).unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
        assert_eq!(
            result.rows[0].values[1],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_optional_match_mixed() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (c:Person {name: \"Charlie\"})-[:KNOWS]->(d:Person {name: \"Dave\"})")
            .unwrap();

        // AliceはKNOWSなし(NULL)、CharlieはDaveを知っている
        let result = g.execute(
            "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name, b.name ORDER BY a.name"
        ).unwrap();

        assert_eq!(result.rows.len(), 3); // Alice(null), Charlie(Dave), Dave(null)
        // Charlie → Dave のペアが含まれているか確認
        let charlie_row = result
            .rows
            .iter()
            .find(|r| r.values[0] == CypherValue::String("Charlie".to_string()));
        assert!(charlie_row.is_some());
        assert_eq!(
            charlie_row.unwrap().values[1],
            CypherValue::String("Dave".to_string())
        );
    }

    // Issue 2: 可変長リレーションシップ
    #[test]
    fn test_var_length_exact_hops() {
        let mut g = Graph::new();
        // A -> B -> C -> D のチェーンを作成
        g.execute(
            "CREATE (a:Node {name: \"A\"})-[:NEXT]->(b:Node {name: \"B\"})-[:NEXT]->(c:Node {name: \"C\"})-[:NEXT]->(d:Node {name: \"D\"})"
        ).unwrap();

        // 1ホップ
        let r = g
            .execute("MATCH (a:Node {name: \"A\"})-[*1..1]->(b) RETURN b.name")
            .unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].values[0], CypherValue::String("B".to_string()));

        // 2ホップ
        let r = g
            .execute("MATCH (a:Node {name: \"A\"})-[*2..2]->(b) RETURN b.name")
            .unwrap();
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].values[0], CypherValue::String("C".to_string()));

        // 1〜2ホップ
        let r = g
            .execute("MATCH (a:Node {name: \"A\"})-[*1..2]->(b) RETURN b.name ORDER BY b.name")
            .unwrap();
        assert_eq!(r.rows.len(), 2);
    }

    #[test]
    fn test_var_length_unbounded() {
        let mut g = Graph::new();
        g.execute(
            "CREATE (a:VNode {name: \"A\"})-[:EDGE]->(b:VNode {name: \"B\"})-[:EDGE]->(c:VNode {name: \"C\"})"
        ).unwrap();

        // A から到達可能な全ノード (*1以上)
        let r = g
            .execute("MATCH (a:VNode {name: \"A\"})-[*]->(b) RETURN b.name ORDER BY b.name")
            .unwrap();
        assert_eq!(r.rows.len(), 2); // B と C
    }

    #[test]
    fn test_match_after_separate_create_queries() {
        // Reproduces: CREATE in one execute() call, then MATCH relationship in another.
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();

        // Verify MATCH alone finds both nodes first
        assert_eq!(g.node_count(), 2, "should have Alice and Bob in storage");
        let match_single = g
            .execute("MATCH (a:Person {name: \"Alice\"}) RETURN a.name")
            .unwrap();
        assert_eq!(
            match_single.rows.len(),
            1,
            "single-node MATCH should find Alice"
        );
        let match_only = g
            .execute("MATCH (a:Person {name: \"Alice\"}), (b:Person {name: \"Bob\"}) RETURN a.name, b.name")
            .unwrap();
        assert_eq!(match_only.rows.len(), 1, "MATCH should find Alice and Bob");

        g.execute(
            "MATCH (a:Person {name: \"Alice\"}), (b:Person {name: \"Bob\"}) CREATE (a)-[:KNOWS]->(b)"
        ).unwrap();
        // Confirm edge was actually created
        assert_eq!(g.edge_count(), 1, "edge should exist after MATCH+CREATE");
        let result = g
            .execute("MATCH (a:Person)-[r:KNOWS]->(b:Person) RETURN a.name AS from, b.name AS to")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn test_export_cypher_empty() {
        let g = Graph::new();
        assert_eq!(g.export_cypher(), "");
    }

    #[test]
    fn test_export_cypher_nodes_only() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();

        let cypher = g.export_cypher();
        assert!(cypher.starts_with("CREATE\n"));
        assert!(cypher.contains(":Person"));
        assert!(cypher.contains("name: \"Alice\""));
        assert!(cypher.contains("age: 30"));
        assert!(cypher.contains("name: \"Bob\""));
        assert!(cypher.contains("age: 25"));
    }

    #[test]
    fn test_export_cypher_with_edges() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: \"Alice\"})").unwrap();
        g.execute("CREATE (:Person {name: \"Bob\"})").unwrap();
        g.execute(
            "MATCH (a:Person {name: \"Alice\"}), (b:Person {name: \"Bob\"}) CREATE (a)-[:KNOWS]->(b)",
        )
        .unwrap();

        let cypher = g.export_cypher();
        assert!(cypher.contains("-[:KNOWS]->"));
    }

    #[test]
    fn test_export_cypher_roundtrip_special_chars() {
        let mut g1 = Graph::new();
        // 改行・ダブルクォート・タブを含む文字列プロパティ
        g1.execute("CREATE (:Company {note: \"## aa\\nhoo\\r\\n### hoge\\tend\", name: \"test \\\"quoted\\\" value\"})")
            .unwrap();

        let cypher = g1.export_cypher();

        // エクスポートされたCypherが再度パース・実行できることを確認
        let mut g2 = Graph::new();
        g2.execute(&cypher).unwrap();

        assert_eq!(g2.node_count(), 1);

        let result = g2
            .execute("MATCH (c:Company) RETURN c.note, c.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("## aa\nhoo\r\n### hoge\tend".to_string())
        );
        assert_eq!(
            result.rows[0].values[1],
            CypherValue::String("test \"quoted\" value".to_string())
        );
    }

    #[test]
    fn test_export_cypher_roundtrip() {
        let mut g1 = Graph::new();
        g1.execute("CREATE (:Person {name: \"Alice\", age: 30})")
            .unwrap();
        g1.execute("CREATE (:Person {name: \"Bob\", age: 25})")
            .unwrap();
        g1.execute(
            "MATCH (a:Person {name: \"Alice\"}), (b:Person {name: \"Bob\"}) CREATE (a)-[:KNOWS]->(b)",
        )
        .unwrap();

        let cypher = g1.export_cypher();

        let mut g2 = Graph::new();
        g2.execute(&cypher).unwrap();

        assert_eq!(g2.node_count(), 2);
        assert_eq!(g2.edge_count(), 1);

        let result = g2
            .execute("MATCH (p:Person) RETURN p.name ORDER BY p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    // --- Parameterized query tests ---

    #[test]
    fn test_parameterized_query_integer() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice', age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: 'Bob', age: 25})")
            .unwrap();

        let mut params = HashMap::new();
        params.insert("min_age".to_string(), CypherValue::Integer(28));

        let result = g
            .execute_with_params(
                "MATCH (p:Person) WHERE p.age > $min_age RETURN p.name",
                params,
            )
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_parameterized_query_string() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice'})").unwrap();
        g.execute("CREATE (:Person {name: 'Bob'})").unwrap();

        let mut params = HashMap::new();
        params.insert(
            "target".to_string(),
            CypherValue::String("Alice".to_string()),
        );

        let result = g
            .execute_with_params(
                "MATCH (p:Person) WHERE p.name = $target RETURN p.name",
                params,
            )
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_parameterized_query_multiple_params() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice', age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: 'Bob', age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: 'Carol', age: 35})")
            .unwrap();

        let mut params = HashMap::new();
        params.insert("min_age".to_string(), CypherValue::Integer(26));
        params.insert("max_age".to_string(), CypherValue::Integer(32));

        let result = g
            .execute_with_params(
                "MATCH (p:Person) WHERE p.age >= $min_age AND p.age <= $max_age RETURN p.name",
                params,
            )
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_parameterized_query_missing_param_is_null() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice'})").unwrap();

        // $undefined resolves to NULL, NULL <> 'Alice' → no match
        let result = g
            .execute_with_params(
                "MATCH (p:Person) WHERE p.name = $undefined RETURN p.name",
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn test_parameterized_query_in_create() {
        let mut g = Graph::new();

        let mut params = HashMap::new();
        params.insert("name".to_string(), CypherValue::String("Dave".to_string()));
        params.insert("age".to_string(), CypherValue::Integer(40));

        g.execute_with_params("CREATE (:Person {name: $name, age: $age})", params)
            .unwrap();

        let result = g.execute("MATCH (p:Person) RETURN p.name, p.age").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Dave".to_string())
        );
        assert_eq!(result.rows[0].values[1], CypherValue::Integer(40));
    }

    // --- UNION / UNION ALL tests ---

    #[test]
    fn test_union_basic() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {name: 'Alice'})").unwrap();
        g.execute("CREATE (:B {name: 'Bob'})").unwrap();

        let result = g
            .execute("MATCH (n:A) RETURN n.name UNION MATCH (n:B) RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_union_dedup() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {name: 'Alice'})").unwrap();
        g.execute("CREATE (:B {name: 'Alice'})").unwrap();

        let result = g
            .execute("MATCH (n:A) RETURN n.name UNION MATCH (n:B) RETURN n.name")
            .unwrap();
        // UNION removes duplicates: both sides return 'Alice', result is 1 row
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_union_all_keeps_duplicates() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {name: 'Alice'})").unwrap();
        g.execute("CREATE (:B {name: 'Alice'})").unwrap();

        let result = g
            .execute("MATCH (n:A) RETURN n.name UNION ALL MATCH (n:B) RETURN n.name")
            .unwrap();
        // UNION ALL preserves duplicates
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_union_all_case_insensitive() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {name: 'Alice'})").unwrap();
        g.execute("CREATE (:B {name: 'Bob'})").unwrap();

        let result = g
            .execute("MATCH (n:A) RETURN n.name union all MATCH (n:B) RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn test_union_column_count_mismatch_error() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {name: 'Alice', age: 30})").unwrap();

        let result = g.execute("MATCH (n:A) RETURN n.name UNION MATCH (n:A) RETURN n.name, n.age");
        assert!(result.is_err());
    }

    #[test]
    fn test_union_three_way() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {val: 1})").unwrap();
        g.execute("CREATE (:B {val: 2})").unwrap();
        g.execute("CREATE (:C {val: 3})").unwrap();

        let result = g
            .execute(
                "MATCH (n:A) RETURN n.val \
                 UNION MATCH (n:B) RETURN n.val \
                 UNION MATCH (n:C) RETURN n.val",
            )
            .unwrap();
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn test_union_empty_side() {
        let mut g = Graph::new();
        g.execute("CREATE (:A {name: 'Alice'})").unwrap();
        // No :B nodes

        let result = g
            .execute("MATCH (n:A) RETURN n.name UNION MATCH (n:B) RETURN n.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    // --- Aggregation tests ---

    #[test]
    fn test_count_star() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice'})").unwrap();
        g.execute("CREATE (:Person {name: 'Bob'})").unwrap();
        g.execute("CREATE (:Person {name: 'Carol'})").unwrap();

        let result = g.execute("MATCH (p:Person) RETURN count(*)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(3));
    }

    #[test]
    fn test_count_star_empty() {
        let mut g = Graph::new();
        // No nodes — COUNT(*) should return 0, not empty
        let result = g.execute("MATCH (p:Person) RETURN count(*)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(0));
    }

    #[test]
    fn test_count_expr() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice'})").unwrap();
        g.execute("CREATE (:Person {name: 'Bob'})").unwrap();

        let result = g.execute("MATCH (p:Person) RETURN count(p)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(2));
    }

    #[test]
    fn test_count_distinct() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {city: 'Tokyo'})").unwrap();
        g.execute("CREATE (:Person {city: 'Tokyo'})").unwrap();
        g.execute("CREATE (:Person {city: 'Osaka'})").unwrap();

        let result = g
            .execute("MATCH (p:Person) RETURN count(DISTINCT p.city)")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(2));
    }

    #[test]
    fn test_sum() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {age: 30})").unwrap();
        g.execute("CREATE (:Person {age: 25})").unwrap();
        g.execute("CREATE (:Person {age: 35})").unwrap();

        let result = g.execute("MATCH (p:Person) RETURN sum(p.age)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(90));
    }

    #[test]
    fn test_avg() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {age: 10})").unwrap();
        g.execute("CREATE (:Person {age: 20})").unwrap();
        g.execute("CREATE (:Person {age: 30})").unwrap();

        let result = g.execute("MATCH (p:Person) RETURN avg(p.age)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Float(20.0));
    }

    #[test]
    fn test_min() {
        let mut g = Graph::new();
        g.execute("CREATE (:Item {score: 5})").unwrap();
        g.execute("CREATE (:Item {score: 1})").unwrap();
        g.execute("CREATE (:Item {score: 9})").unwrap();

        let result = g.execute("MATCH (i:Item) RETURN min(i.score)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(1));
    }

    #[test]
    fn test_max() {
        let mut g = Graph::new();
        g.execute("CREATE (:Item {score: 5})").unwrap();
        g.execute("CREATE (:Item {score: 1})").unwrap();
        g.execute("CREATE (:Item {score: 9})").unwrap();

        let result = g.execute("MATCH (i:Item) RETURN max(i.score)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(9));
    }

    #[test]
    fn test_collect() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice'})").unwrap();
        g.execute("CREATE (:Person {name: 'Bob'})").unwrap();

        let result = g
            .execute("MATCH (p:Person) RETURN collect(p.name)")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        if let CypherValue::List(mut items) = result.rows[0].values[0].clone() {
            items.sort_by(|a, b| {
                let ka = if let CypherValue::String(s) = a {
                    s.as_str()
                } else {
                    ""
                };
                let kb = if let CypherValue::String(s) = b {
                    s.as_str()
                } else {
                    ""
                };
                ka.cmp(kb)
            });
            assert_eq!(
                items,
                vec![
                    CypherValue::String("Alice".to_string()),
                    CypherValue::String("Bob".to_string()),
                ]
            );
        } else {
            panic!("Expected List from collect()");
        }
    }

    #[test]
    fn test_collect_distinct() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {city: 'Tokyo'})").unwrap();
        g.execute("CREATE (:Person {city: 'Tokyo'})").unwrap();
        g.execute("CREATE (:Person {city: 'Osaka'})").unwrap();

        let result = g
            .execute("MATCH (p:Person) RETURN collect(DISTINCT p.city)")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        if let CypherValue::List(mut items) = result.rows[0].values[0].clone() {
            items.sort_by(|a, b| {
                let ka = if let CypherValue::String(s) = a {
                    s.as_str()
                } else {
                    ""
                };
                let kb = if let CypherValue::String(s) = b {
                    s.as_str()
                } else {
                    ""
                };
                ka.cmp(kb)
            });
            assert_eq!(
                items,
                vec![
                    CypherValue::String("Osaka".to_string()),
                    CypherValue::String("Tokyo".to_string()),
                ]
            );
        } else {
            panic!("Expected List from collect(DISTINCT ...)");
        }
    }

    #[test]
    fn test_group_by_aggregation() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {city: 'Tokyo', age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {city: 'Tokyo', age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {city: 'Osaka', age: 40})")
            .unwrap();

        let result = g
            .execute("MATCH (p:Person) RETURN p.city, count(*) ORDER BY p.city")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        // Osaka comes first alphabetically
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Osaka".to_string())
        );
        assert_eq!(result.rows[0].values[1], CypherValue::Integer(1));
        assert_eq!(
            result.rows[1].values[0],
            CypherValue::String("Tokyo".to_string())
        );
        assert_eq!(result.rows[1].values[1], CypherValue::Integer(2));
    }

    #[test]
    fn test_group_by_sum() {
        let mut g = Graph::new();
        g.execute("CREATE (:Sale {region: 'East', amount: 100})")
            .unwrap();
        g.execute("CREATE (:Sale {region: 'East', amount: 200})")
            .unwrap();
        g.execute("CREATE (:Sale {region: 'West', amount: 150})")
            .unwrap();

        let result = g
            .execute("MATCH (s:Sale) RETURN s.region, sum(s.amount) ORDER BY s.region")
            .unwrap();
        assert_eq!(result.rows.len(), 2);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("East".to_string())
        );
        assert_eq!(result.rows[0].values[1], CypherValue::Integer(300));
        assert_eq!(
            result.rows[1].values[0],
            CypherValue::String("West".to_string())
        );
        assert_eq!(result.rows[1].values[1], CypherValue::Integer(150));
    }

    #[test]
    fn test_aggregation_with_with_clause() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name: 'Alice', age: 30})")
            .unwrap();
        g.execute("CREATE (:Person {name: 'Bob', age: 25})")
            .unwrap();
        g.execute("CREATE (:Person {name: 'Carol', age: 35})")
            .unwrap();

        // COUNT via WITH then RETURN
        let result = g
            .execute("MATCH (p:Person) WITH count(*) AS total RETURN total")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(3));
    }

    #[test]
    fn test_sum_empty() {
        let mut g = Graph::new();
        // No nodes — SUM should return 0
        let result = g.execute("MATCH (p:Person) RETURN sum(p.age)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Integer(0));
    }

    #[test]
    fn test_avg_empty() {
        let mut g = Graph::new();
        // No nodes — AVG should return Null
        let result = g.execute("MATCH (p:Person) RETURN avg(p.age)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Null);
    }

    #[test]
    fn test_min_empty() {
        let mut g = Graph::new();
        // No nodes — MIN should return Null
        let result = g.execute("MATCH (i:Item) RETURN min(i.score)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Null);
    }

    #[test]
    fn test_max_empty() {
        let mut g = Graph::new();
        // No nodes — MAX should return Null
        let result = g.execute("MATCH (i:Item) RETURN max(i.score)").unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::Null);
    }

    #[test]
    fn test_collect_empty() {
        let mut g = Graph::new();
        // No nodes — COLLECT should return empty list
        let result = g
            .execute("MATCH (p:Person) RETURN collect(p.name)")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].values[0], CypherValue::List(vec![]));
    }

    #[test]
    fn test_create_unique_constraint() {
        let mut g = Graph::new();
        // 制約作成は成功する
        g.execute("CREATE CONSTRAINT FOR (n:User) REQUIRE n.gnId IS UNIQUE")
            .unwrap();
        // 1件目は成功
        g.execute("CREATE (n:User {gnId: 'abc'})").unwrap();
        // 同じ値は制約違反でエラー
        assert!(g.execute("CREATE (n:User {gnId: 'abc'})").is_err());
        // 別の値は成功
        g.execute("CREATE (n:User {gnId: 'xyz'})").unwrap();
    }

    #[test]
    fn test_create_unique_constraint_existing_violation() {
        let mut g = Graph::new();
        // 重複データを先に作成
        g.execute("CREATE (n:User {gnId: 'dup'})").unwrap();
        g.execute("CREATE (n:User {gnId: 'dup'})").unwrap();
        // 制約追加は既存データの違反でエラー
        assert!(
            g.execute("CREATE CONSTRAINT FOR (n:User) REQUIRE n.gnId IS UNIQUE")
                .is_err()
        );
    }

    #[test]
    fn test_many_match_pairs_no_stack_overflow() {
        // Regression test: 300 MATCH pairs used to produce a 300-level deep
        // CartesianProduct tree which caused a stack overflow (visible as
        // "memory access out of bounds" in WASM).  The fix flattens the
        // left-leaning CartesianProduct chain into an iterative loop.
        let mut g = Graph::new();

        // Insert 20 nodes
        let node_count = 20usize;
        for i in 0..node_count {
            g.execute(&format!(
                "CREATE (:Person {{gnId: \"node-{}\", name: \"Person {}\"}})",
                i + 1,
                i + 1
            ))
            .unwrap();
        }
        assert_eq!(g.node_count(), node_count);

        // Build a query with 1000 MATCH pairs and one CREATE (regression for 1000-pair case)
        let batch_size = 1000usize;
        let mut match_lines = Vec::new();
        let mut create_parts = Vec::new();
        for i in 1..=batch_size {
            let src = (i % node_count) + 1;
            let dst = ((i * 3 + 1) % node_count) + 1;
            if src != dst {
                let idx = i - 1;
                match_lines.push(format!(
                    "MATCH (_a{}:Person {{gnId: \"node-{}\"}}), (_b{}:Person {{gnId: \"node-{}\"}})",
                    idx, src, idx, dst
                ));
                create_parts.push(format!(
                    "(_a{})-[:KNOWS {{weight: {}}}]->(_b{})",
                    idx,
                    i % 10,
                    idx
                ));
            }
        }
        let query = format!(
            "{}\nCREATE {}",
            match_lines.join("\n"),
            create_parts.join(", ")
        );
        // Must not panic or overflow the stack
        g.execute(&query).unwrap();
        assert!(g.edge_count() > 0);
    }

    #[test]
    fn test_create_unique_constraint_no_conflict_other_label() {
        let mut g = Graph::new();
        g.execute("CREATE CONSTRAINT FOR (n:User) REQUIRE n.gnId IS UNIQUE")
            .unwrap();
        // 別ラベルは制約対象外なので同じ値でも成功
        g.execute("CREATE (n:User {gnId: 'abc'})").unwrap();
        g.execute("CREATE (n:Admin {gnId: 'abc'})").unwrap();
    }

    #[cfg(feature = "sled-storage")]
    mod sled_tests {
        use super::*;
        use crate::SledStorage;
        use crate::graph::Graph;
        use tempfile::tempdir;

        #[test]
        fn test_sled_persist_roundtrip() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("graph.sled");

            // Write phase
            {
                let storage = SledStorage::open(&path).unwrap();
                let mut g = Graph::new_with_storage(storage);
                g.execute("CREATE (:Person {name: \"Alice\", age: 30})")
                    .unwrap();
                g.execute("CREATE (:Person {name: \"Bob\", age: 25})")
                    .unwrap();
            }

            // Reopen and read phase
            {
                let storage = SledStorage::open(&path).unwrap();
                let mut g = Graph::new_with_storage(storage);
                let result = g.execute("MATCH (p:Person) RETURN p.name").unwrap();
                assert_eq!(result.rows.len(), 2);
            }
        }

        #[test]
        fn test_sled_edge_persist() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("edge_graph.sled");

            {
                let storage = SledStorage::open(&path).unwrap();
                let mut g = Graph::new_with_storage(storage);
                g.execute("CREATE (:Person {name: \"Alice\"})-[:KNOWS]->(:Person {name: \"Bob\"})")
                    .unwrap();
            }

            {
                let storage = SledStorage::open(&path).unwrap();
                let mut g = Graph::new_with_storage(storage);
                let result = g
                    .execute("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name")
                    .unwrap();
                assert_eq!(result.rows.len(), 1);
            }
        }

        #[test]
        fn test_sled_export_cypher() {
            let dir = tempdir().unwrap();
            let path = dir.path().join("export.sled");

            let storage = SledStorage::open(&path).unwrap();
            let mut g = Graph::new_with_storage(storage);
            g.execute("CREATE (:Foo {x: 1})").unwrap();
            let cypher = g.export_cypher();
            assert!(cypher.contains("Foo"));
            assert!(cypher.contains("x: 1"));
        }
    }

    // ─── EXISTS subquery tests ────────────────────────────────────────────────

    fn setup_exists_graph() -> Graph {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name:'Alice'})").unwrap();
        g.execute("CREATE (:Person {name:'Bob'})").unwrap();
        g.execute("CREATE (:Car {model:'Tesla', year:2020})")
            .unwrap();
        g.execute("MATCH (p:Person {name:'Bob'}), (c:Car) CREATE (p)-[:OWNS {primary:true}]->(c)")
            .unwrap();
        g
    }

    #[test]
    fn test_exists_simple_pattern() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS]->() } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_not_exists_negation() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE NOT EXISTS { (p)-[:OWNS]->() } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_exists_with_target_label() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS]->(:Car) } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_exists_with_wrong_target_label() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS]->(:Boat) } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn test_exists_with_node_properties() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS]->({year:2020}) } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_exists_with_node_properties_no_match() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS]->({year:1999}) } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn test_exists_with_rel_properties() {
        let mut g = setup_exists_graph();
        let result = g
            .execute(
                "MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS {primary:true}]->() } RETURN p.name",
            )
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_exists_with_rel_properties_no_match() {
        let mut g = setup_exists_graph();
        let result = g
            .execute(
                "MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS {primary:false}]->() } RETURN p.name",
            )
            .unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn test_exists_direction_incoming() {
        let mut g = setup_exists_graph();
        // Car should have an incoming OWNS edge
        let result = g
            .execute("MATCH (c:Car) WHERE EXISTS { (c)<-[:OWNS]-() } RETURN c.model")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Tesla".to_string())
        );
    }

    #[test]
    fn test_exists_direction_undirected() {
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS]-() } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_exists_multi_hop() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name:'Alice'})").unwrap();
        g.execute("CREATE (:Person {name:'Bob'})").unwrap();
        g.execute("CREATE (:Car {model:'Tesla'})").unwrap();
        // Alice KNOWS Bob; Bob OWNS Car
        g.execute(
            "MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) CREATE (a)-[:KNOWS]->(b)",
        )
        .unwrap();
        g.execute("MATCH (b:Person {name:'Bob'}), (c:Car) CREATE (b)-[:OWNS]->(c)")
            .unwrap();

        let result = g
            .execute(
                "MATCH (p:Person) WHERE EXISTS { (p)-[:KNOWS]->()-[:OWNS]->(:Car) } RETURN p.name",
            )
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_exists_rel_type_alternation() {
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name:'Alice'})").unwrap();
        g.execute("CREATE (:Person {name:'Bob'})").unwrap();
        g.execute("CREATE (:Car {model:'Tesla'})").unwrap();
        g.execute("MATCH (p:Person {name:'Bob'}), (c:Car) CREATE (p)-[:LEASES]->(c)")
            .unwrap();

        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (p)-[:OWNS|LEASES]->() } RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Bob".to_string())
        );
    }

    #[test]
    fn test_exists_scalar_function_regression() {
        // The existing exists(expr) scalar function must still work
        let mut g = Graph::new();
        g.execute("CREATE (:Person {name:'Alice'})").unwrap();
        g.execute("CREATE (:Person {})").unwrap();
        let result = g
            .execute("MATCH (p:Person) WHERE exists(p.name) RETURN p.name")
            .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[0],
            CypherValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_exists_var_length_unsupported() {
        let mut g = setup_exists_graph();
        let result = g.execute("MATCH (p:Person) WHERE EXISTS { (p)-[*1..3]->() } RETURN p.name");
        assert!(result.is_err());
    }

    #[test]
    fn test_exists_only_node_pattern() {
        // EXISTS { (:Car) } — no relationship, just checks node existence
        let mut g = setup_exists_graph();
        let result = g
            .execute("MATCH (p:Person) WHERE EXISTS { (:Car) } RETURN p.name")
            .unwrap();
        // Both Alice and Bob match since there is at least one Car node
        assert_eq!(result.rows.len(), 2);
    }
}
