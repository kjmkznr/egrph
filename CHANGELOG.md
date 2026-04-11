# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
