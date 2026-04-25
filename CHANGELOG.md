# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [wasm-0.4.0] - 2026-04-25

### Added

- `LOAD CSV` 句のサポートを追加。
- `NOT NULL`、`NODE KEY`、`PROPERTY TYPE` 制約のサポートを追加。
- `EXISTS { pattern }` サブクエリのサポートを追加。
- `MERGE` でリレーションシップチェーンパターンのサポートを追加。
- `MATCH` パターン内でのインラインプロパティ述語のサポートを追加。
- `randomUUID()` 組み込み関数を追加。
- ブロックコメント（`/* ... */`）のサポートを追加。
- `egrph-cli`: DuckDB スタイルのインタラクティブ Cypher シェルを追加。
- `sled_storage` の使用例を追加。
- `WasmGraph` に `Default` トレイトを実装。

### Fixed

- 1000 件以上の CREATE パターンを含むクエリ（例: `CREATE (a0)-[:R]->(b0), (a1)-[:R]->(b1), ...` を 1000 個）で発生していた `RuntimeError: memory access out of bounds` を修正。プランナーが生成する 1000 段の `CreatePath`/`CreateNode` チェーンをエグゼキューターで再帰的に処理していたため、WASM 環境でスタックオーバーフローが発生していた。`CreatePath`/`CreateNode` チェーンを反復処理に変換することで修正。
- `InMemoryGraph` の型エイリアスを `Graph` 型を直接使用するように修正。

### Changed

- 複数の `MATCH` 句を連結するクエリ（例: 1000 個の MATCH + CREATE）のパフォーマンスを改善。`CartesianProduct` の結合処理で、両側が各 1 レコードの場合（プロパティ指定による一意ルックアップの連鎖で頻出）に HashMap の clone を省略し、in-place extend で結合するファストパスを追加。
- `LeftOuterJoin` をハッシュ結合に変更し、O(N*M) スキャンを除去。
- `VarLengthExpand` BFS でアリーナチェーンによるパスプレフィックス共有を導入し、パフォーマンスを改善。
- `Record` を `Arc<HashMap>` で裏付け、パイプライン出力時に move-last 最適化を適用。
- `CartesianProduct` マージ時のキー/値クローンを削減。
- 制約バリデーションのリファクタリングによりネスト条件を簡素化。
- プロパティマップのキーとして式やパラメータをサポート。
- 未使用の `thiserror` および `serde_json` 依存を削除。

## [wasm-0.3.1] - 2026-04-15

### Fixed

- 複数の `MATCH` 句を含むクエリ（例: 300 ペア）で発生していた `RuntimeError: memory access out of bounds` を修正。プランナーが生成する左偏り `CartesianProduct` ツリーをエグゼキューターで再帰的に処理していたため、深いスタックオーバーフローが WASM 環境で発生していた。`CartesianProduct` チェーンを反復処理に変換することで修正。

## [wasm-0.3.0] - 2026-04-15

### Added

- `UNWIND` 句と `CREATE` 句を組み合わせたクエリで、プロパティマップに複雑な式（変数参照・演算など）を使用できるようにサポート。

### Changed

- ノードのプロパティ値によるルックアップをプロパティインデックスにより O(1) に改善。

## [wasm-0.2.1] - 2026-04-12

### Added

- `CREATE CONSTRAINT` 文によるユニーク制約のサポートを追加。

### Fixed

- `export_cypher()` でプロパティ値に特殊文字（バックスラッシュ、ダブルクォート等）が含まれる場合のエスケープ処理を修正。

## [0.2.0] - 2026-04-11

### Added

- `StorageBackend` トレイトによるプラガブルなストレージバックエンド抽象化を追加。デフォルトはこれまで通りのインメモリ実装。
- `sled` ベースの永続化ストレージバックエンドを追加（オプション feature: `sled-storage`）。
- `UNION` / `UNION ALL` のサポートを追加。
- `Graph::execute_with_params()`: パラメータ付きクエリ API を追加。

### Changed

- `egrph-c-abi` および `egrph-python` のライブラリターゲット名をよりわかりやすい名前にリネーム。
- `extension-module` feature を依存関係側からクレートレベルの定義に移動。

## [0.1.1] - 2026-04-09

### Added

- `Graph::export_cypher()`: グラフ全体を Cypher の `CREATE` 文としてエクスポートする機能を追加。出力は `Graph::execute()` に渡すことでグラフを再構築できる。
- `WasmGraph.exportCypher()`: 上記機能を WebAssembly バインディングでも利用可能に。
- `PyGraph.export_cypher()`: 上記機能を Python バインディングでも利用可能に。
- `graph_export_cypher()`: 上記機能を C ABI で公開。返却ポインタは `graph_free_string()` で解放する。
- `(*Graph).ExportCypher()`: 上記機能を Go バインディングでも利用可能に。

## [0.1.0] - 2026-04-05

### Added

- Initial release of egrph graph database library
- Cypher query language support (CREATE, MATCH, WHERE, RETURN, WITH, DELETE, MERGE, SET, REMOVE, ORDER BY, SKIP, LIMIT, OPTIONAL MATCH)
- In-memory graph storage with label indexing
- Python bindings via PyO3
- C ABI bridge for cross-language bindings
- Go bindings via cgo
- WebAssembly bindings with npm package
- Demo page with Cytoscape.js graph visualization
