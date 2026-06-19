use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use egrph_core::graph::Graph;

// Simulates a nodetrace-style workload: creating spans/traces as nodes with
// properties, then querying them by label+property and traversing edges.

fn bench_create_nodes_with_props(c: &mut Criterion) {
    c.bench_function("create_1000_nodes_with_props", |b| {
        b.iter(|| {
            let mut g = Graph::new();
            for i in 0..1000u64 {
                g.execute(&format!(
                    "CREATE (:Span {{trace_id: \"{}\", span_id: {}, service: \"api\", op: \"handle\"}})",
                    i / 10,
                    i
                ))
                .unwrap();
            }
            black_box(g)
        });
    });
}

fn bench_match_nodes_by_label(c: &mut Criterion) {
    let mut g = Graph::new();
    for i in 0..1000u64 {
        g.execute(&format!(
            "CREATE (:Span {{trace_id: \"{}\", span_id: {}, service: \"api\"}})",
            i / 10,
            i
        ))
        .unwrap();
    }

    c.bench_function("match_1000_nodes_by_label", |b| {
        b.iter(|| {
            let result = g.execute("MATCH (s:Span) RETURN s.span_id").unwrap();
            black_box(result)
        });
    });
}

fn bench_match_by_property(c: &mut Criterion) {
    let mut g = Graph::new();
    for i in 0..1000u64 {
        g.execute(&format!(
            "CREATE (:Span {{trace_id: \"trace-{}\", span_id: {}, service: \"api\"}})",
            i / 10,
            i
        ))
        .unwrap();
    }

    c.bench_function("match_node_by_property_indexed", |b| {
        b.iter(|| {
            let result = g
                .execute("MATCH (s:Span {trace_id: \"trace-5\"}) RETURN s.span_id")
                .unwrap();
            black_box(result)
        });
    });
}

fn bench_merge_operations(c: &mut Criterion) {
    c.bench_function("merge_500_nodes_idempotent", |b| {
        b.iter_batched(
            || {
                // Setup: pre-populate with 500 nodes
                let mut g = Graph::new();
                for i in 0..500u64 {
                    g.execute(&format!(
                        "CREATE (:Service {{name: \"svc-{}\", version: 1}})",
                        i
                    ))
                    .unwrap();
                }
                g
            },
            |mut g| {
                // Benchmark: MERGE on already-existing nodes (match branch)
                for i in 0..500u64 {
                    g.execute(&format!(
                        "MERGE (s:Service {{name: \"svc-{}\"}}) ON MATCH SET s.hits = 1",
                        i
                    ))
                    .unwrap();
                }
                black_box(g)
            },
            BatchSize::LargeInput,
        );
    });
}

fn bench_graph_traversal(c: &mut Criterion) {
    let mut g = Graph::new();
    // Build a chain: root -> 100 children, each with 10 grandchildren = 1100 nodes
    g.execute("CREATE (:Root {id: 0})").unwrap();
    for i in 1..=100u64 {
        g.execute(&format!(
            "MATCH (r:Root {{id: 0}}) CREATE (r)-[:CALLS]->(:Service {{id: {}}})",
            i
        ))
        .unwrap();
    }
    for i in 1..=100u64 {
        for j in 0..10u64 {
            g.execute(&format!(
                "MATCH (s:Service {{id: {}}}) CREATE (s)-[:CALLS]->(:Handler {{id: {}}})",
                i,
                i * 100 + j
            ))
            .unwrap();
        }
    }

    c.bench_function("expand_two_hops_1100_nodes", |b| {
        b.iter(|| {
            let result = g
                .execute("MATCH (r:Root)-[:CALLS]->(s:Service)-[:CALLS]->(h:Handler) RETURN h.id")
                .unwrap();
            black_box(result)
        });
    });
}

fn bench_where_filter(c: &mut Criterion) {
    let mut g = Graph::new();
    for i in 0..2000u64 {
        g.execute(&format!(
            "CREATE (:Event {{code: {}, level: \"{}\"}})",
            i,
            if i % 3 == 0 { "ERROR" } else { "INFO" }
        ))
        .unwrap();
    }

    c.bench_function("match_where_filter_2000_nodes", |b| {
        b.iter(|| {
            let result = g
                .execute("MATCH (e:Event) WHERE e.level = \"ERROR\" RETURN e.code")
                .unwrap();
            black_box(result)
        });
    });
}

fn bench_prop_value_key_hot_path(c: &mut Criterion) {
    // This benchmark stress-tests the property index which calls prop_value_key on every
    // create_node and find_nodes. It reflects the "insert-then-lookup" pattern in nodetrace.
    c.bench_function("create_and_find_by_prop_500_nodes", |b| {
        b.iter(|| {
            let mut g = Graph::new();
            for i in 0..500u64 {
                g.execute(&format!(
                    "MERGE (:Trace {{id: \"t-{}\"}}) RETURN 1",
                    i
                ))
                .unwrap();
            }
            // Now look up each one: exercises find_nodes + prop_value_key hot path
            for i in 0..500u64 {
                let r = g
                    .execute(&format!(
                        "MATCH (t:Trace {{id: \"t-{}\"}}) RETURN t.id",
                        i
                    ))
                    .unwrap();
                black_box(r);
            }
        });
    });
}

/// Parameterized benchmarks: plan is cached after the first call, so these
/// measure the pure storage/executor hot path without parser/planner overhead.
/// These are the most reliable indicators of storage-level improvements.
fn bench_create_parameterized(c: &mut Criterion) {
    use std::collections::HashMap;
    use egrph_core::CypherValue;

    c.bench_function("create_1000_nodes_parameterized", |b| {
        b.iter(|| {
            let mut g = Graph::new();
            for i in 0..1000i64 {
                g.execute_with_params(
                    "CREATE (:Span {span_id: $span_id, ts: $ts, hop: $hop, amount: $amount})",
                    HashMap::from([
                        ("span_id".to_string(), CypherValue::Integer(i)),
                        ("ts".to_string(), CypherValue::Integer(i * 1000)),
                        ("hop".to_string(), CypherValue::Integer(i % 10)),
                        ("amount".to_string(), CypherValue::Float(i as f64 * 0.5)),
                    ]),
                ).unwrap();
            }
            black_box(g)
        });
    });
}

fn bench_merge_parameterized(c: &mut Criterion) {
    use std::collections::HashMap;
    use egrph_core::CypherValue;

    c.bench_function("merge_1000_nodes_parameterized", |b| {
        b.iter_batched(
            || {
                let mut g = Graph::new();
                // Pre-populate 1000 nodes so MERGE always hits the ON MATCH branch
                for i in 0..1000i64 {
                    g.execute_with_params(
                        "CREATE (:Request {req_id: $req_id, ts: $ts})",
                        HashMap::from([
                            ("req_id".to_string(), CypherValue::Integer(i)),
                            ("ts".to_string(), CypherValue::Integer(i * 1000)),
                        ]),
                    ).unwrap();
                }
                g
            },
            |mut g| {
                // All hits ON MATCH branch: exercises find_nodes + SET
                for i in 0..1000i64 {
                    g.execute_with_params(
                        "MERGE (r:Request {req_id: $req_id}) ON MATCH SET r.last_ts = $ts",
                        HashMap::from([
                            ("req_id".to_string(), CypherValue::Integer(i)),
                            ("ts".to_string(), CypherValue::Integer(i * 2000)),
                        ]),
                    ).unwrap();
                }
                black_box(g)
            },
            criterion::BatchSize::LargeInput,
        );
    });
}

fn bench_ingest_nodetrace_style(c: &mut Criterion) {
    use std::collections::HashMap;
    use egrph_core::CypherValue;

    // Mirrors the nodetrace INGEST_CYPHER hot path: 4 MERGEs + 1 CREATE + 4 CREATE edges
    // Uses parameterized queries so plan is cached after first call.
    const INGEST: &str = r#"
MERGE (r:Request {req_id: $req_id})
  ON CREATE SET r.first_ts = $ts
  ON MATCH  SET r.last_ts  = $ts
MERGE (ip:IpAddress {ip: $ip})
  ON CREATE SET ip.first_seen = $ts
  ON MATCH  SET ip.last_seen  = $ts
MERGE (svc:Service {name: $svc})
CREATE (e:LogEvent {event_id: $event_id, ts: $ts, hop: $hop, amount: $amount, ip: $ip, req_id: $req_id})
CREATE (e)-[:PART_OF]->(r)
CREATE (e)-[:FROM_IP]->(ip)
MERGE (ip)-[a:ACCESSED]->(r)
  ON CREATE SET a.count = 1
  ON MATCH  SET a.count = a.count + 1
"#;

    c.bench_function("ingest_nodetrace_500_events", |b| {
        b.iter(|| {
            let mut g = Graph::new();
            for i in 0..500i64 {
                g.execute_with_params(
                    INGEST,
                    HashMap::from([
                        ("event_id".to_string(), CypherValue::Integer(i)),
                        ("ts".to_string(), CypherValue::Integer(i * 1000)),
                        ("req_id".to_string(), CypherValue::Integer(i / 10)),
                        ("ip".to_string(), CypherValue::String(format!("10.0.{}.{}", i / 256, i % 256))),
                        ("svc".to_string(), CypherValue::String("api".to_string())),
                        ("hop".to_string(), CypherValue::Integer(i % 10)),
                        ("amount".to_string(), CypherValue::Float(0.0)),
                    ]),
                ).unwrap();
            }
            black_box(g)
        });
    });
}

criterion_group!(
    benches,
    bench_create_nodes_with_props,
    bench_match_nodes_by_label,
    bench_match_by_property,
    bench_merge_operations,
    bench_graph_traversal,
    bench_where_filter,
    bench_prop_value_key_hot_path,
    bench_create_parameterized,
    bench_merge_parameterized,
    bench_ingest_nodetrace_style,
);
criterion_main!(benches);
