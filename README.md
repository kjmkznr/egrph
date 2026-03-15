# egrph

Rustで書かれた軽量なグラフライブラリです。コアロジックをRustで提供し、Python、Go、およびC ABI経由で他の言語からも利用可能です。

## プロジェクト構成

- `egrph-core`: Rustによるコアロジック。NodeとEdgeの管理、基本的なクエリ機能、および **Cypher Query サポート** を提供します。
- `egrph-python`: PyO3を使用したPythonバインディング。
- `egrph-c-abi`: C互換のABIインターフェース。
- `egrph-go`: `egrph-c-abi`をcgo経由で呼び出すGoパッケージ。

## 使い方

### Rust

```rust
use egrph_core::{Graph, PropertyValue};
use std::collections::HashMap;

fn main() {
    let mut g = Graph::new();
    
    // APIを使用したノードとエッジの作成
    let n1 = g.create_node(vec!["Person".to_string()], HashMap::new());
    let n2 = g.create_node(vec!["Person".to_string()], HashMap::new());
    let e1 = g.create_edge("KNOWS".to_string(), n1, n2, HashMap::new()).unwrap();

    // Cypher Query (CREATE) によるノードの作成
    g.query("CREATE (:Person {name: \"Alice\", age: 30})").unwrap();

    // Cypher Query (MATCH) によるノードの検索
    let results = g.query("MATCH (:Person) RETURN p").unwrap();
    println!("Found {} persons", results.len());
}
```

### Python

`egrph-python`をビルドしてインポートします。

```python
import egrph

g = egrph.PyGraph()
n1 = g.create_node(["Person"], {"name": "Alice"})
n2 = g.create_node(["Person"], {"name": "Bob"})
e1 = g.create_edge("KNOWS", n1, n2, {})

print(f"Nodes: {g.get_node_count()}, Edges: {g.get_edge_count()}")
```

### Go

`egrph-c-abi`をビルドした後、`egrph-go`パッケージを利用します。

```go
package main

import (
    "fmt"
    "github.com/kjmkznr/egrph/egrph-go"
)

func main() {
    g := egrph.NewGraph()
    defer g.Free()

    n1 := g.CreateNode()
    n2 := g.CreateNode()
    e1 := g.CreateEdge("KNOWS", n1, n2)

    fmt.Printf("Nodes: %d, Edges: %d\n", g.GetNodeCount(), g.GetEdgeCount())
}
```

## ビルド

### Core / C ABI / Python
```bash
cargo build --release
```

Pythonバインディングを使用する場合は、`maturin`などを用いてビルドすることをお勧めします。

### Go
`egrph-c-abi`を事前にビルドしておく必要があります。
```bash
cd egrph-c-abi
cargo build --release
cd ../egrph-go
go test .
```

## ライセンス
[MIT License](LICENSE) (またはプロジェクトに適したライセンス)
