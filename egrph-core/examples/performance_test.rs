use egrph_core::{Graph, PropertyValue};
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let sizes = [100, 1000, 10000, 100000, 1000000];

    for &size in &sizes {
        println!("--- Testing with size: {} ---", size);
        let mut g = Graph::new();

        // 1. ノード作成 (API経由)
        let start = Instant::now();
        for i in 0..size {
            let mut props = HashMap::new();
            props.insert("id".to_string(), PropertyValue::Int(i as i64));
            g.create_node(vec!["Person".to_string()], props);
        }
        let duration = start.elapsed();
        println!("Create {} nodes (API): {:?}", size, duration);

        // 2. エッジ作成 (API経由)
        // 各ノードから次のノードへエッジを張る
        let start = Instant::now();
        for i in 0..size - 1 {
            g.create_edge("KNOWS".to_string(), i as u64, (i + 1) as u64, HashMap::new()).unwrap();
        }
        let duration = start.elapsed();
        println!("Create {} edges (API): {:?}", size - 1, duration);

        // 3. MATCH クエリ (Cypher)
        // 全てのPersonノードを検索
        let start = Instant::now();
        let results = g.execute("MATCH (p:Person) RETURN p").unwrap();
        let duration = start.elapsed();
        println!("MATCH (:Person) (found {}): {:?}", results.rows.len(), duration);

        // 4. 特定のラベルがないノードの検索 (最悪ケース)
        let start = Instant::now();
        let results = g.execute("MATCH (n:NonExistent) RETURN n").unwrap();
        let duration = start.elapsed();
        println!("MATCH (:NonExistent) (found {}): {:?}", results.rows.len(), duration);

        // 5. CREATE クエリ (Cypher)
        let start = Instant::now();
        for _i in 0..1000 {
            g.execute("CREATE (:Person {name: \"Test\", age: 20})").unwrap();
        }
        let duration = start.elapsed();
        println!("CREATE 1000 nodes (Cypher): {:?}", duration);

        println!();
    }
}
