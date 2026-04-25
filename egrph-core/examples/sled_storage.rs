//! SledStorage を使った永続化グラフのサンプル
//!
//! 実行方法:
//! ```bash
//! cargo run --example sled_storage --features sled-storage
//! ```

#[cfg(feature = "sled-storage")]
fn main() {
    use egrph_core::{Graph, SledStorage};
    use std::collections::HashMap;

    let db_path = "/tmp/egrph_example.sled";

    // ── 1. DBを開いてデータを書き込む ────────────────────────────────────────
    println!("=== 書き込みフェーズ ===");
    {
        let storage = SledStorage::open(db_path).expect("DBのオープンに失敗しました");
        let mut g = Graph::new_with_storage(storage);

        // 直接APIでノードを作成
        let alice = g.create_node(
            vec!["Person".to_string()],
            [
                (
                    "name".to_string(),
                    egrph_core::PropertyValue::String("Alice".to_string()),
                ),
                ("age".to_string(), egrph_core::PropertyValue::Int(30)),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        );
        let bob = g.create_node(
            vec!["Person".to_string()],
            [
                (
                    "name".to_string(),
                    egrph_core::PropertyValue::String("Bob".to_string()),
                ),
                ("age".to_string(), egrph_core::PropertyValue::Int(25)),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        );

        // エッジを作成
        g.create_edge("KNOWS".to_string(), alice, bob, HashMap::new())
            .expect("エッジの作成に失敗しました");

        // Cypher Query でノードを追加
        g.execute("CREATE (:Person {name: \"Carol\", age: 35})")
            .expect("CREATEクエリに失敗しました");

        println!("ノード数: {}", g.node_count());
        println!("エッジ数: {}", g.edge_count());

        // グラフをCypherとしてエクスポート
        println!("\n--- エクスポートされたCypher ---");
        println!("{}", g.export_cypher());
    } // ここでgがドロップされ、データはディスクに永続化される

    // ── 2. DBを再オープンしてデータを読み込む ────────────────────────────────
    println!("\n=== 読み込みフェーズ（再オープン後）===");
    {
        let storage = SledStorage::open(db_path).expect("DBのオープンに失敗しました");
        let mut g = Graph::new_with_storage(storage);

        println!("ノード数: {}", g.node_count());
        println!("エッジ数: {}", g.edge_count());

        // Cypher Query でPersonノードを検索
        let results = g
            .execute("MATCH (p:Person) RETURN p")
            .expect("MATCHクエリに失敗しました");

        println!("\n--- Personノード一覧 ---");
        for row in &results.rows {
            println!("{:?}", row);
        }

        // 新しいノードを追加して永続化を確認
        g.execute("CREATE (:Company {name: \"Acme\"})")
            .expect("CREATEクエリに失敗しました");
        println!("\nCompanyノードを追加後のノード数: {}", g.node_count());
    }

    // ── 3. 後片付け ───────────────────────────────────────────────────────────
    // std::fs::remove_dir_all(db_path).ok();
    // println!("\nDBを削除しました: {}", db_path);
}

#[cfg(not(feature = "sled-storage"))]
fn main() {
    eprintln!("このサンプルは `sled-storage` フィーチャーが必要です。");
    eprintln!("実行方法: cargo run --example sled_storage --features sled-storage");
    std::process::exit(1);
}
