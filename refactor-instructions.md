# refactor-instructions.md — egrph リファクタリング指示書

対象リポジトリ: `kjmkznr/egrph`
基準コミット: `8783b85` (main / 2026-07-02 時点)
この指示書は事前のコードベース全読 + 実機検証(CLI でのクエリ再現、テスト・lint・fmt のベースライン実行)に基づいて作成されている。

---

## 1. Objective

**既存仕様を一切壊さずに**、egrph の技術的負債を減らし、今後の変更を安全にする。

具体的なゴール:

1. 壊れているベースライン(`cargo fmt --check` 失敗、CI の sled feature 未テスト)を修復する
2. 重要挙動に安全網(characterization test)を張る
3. 証拠のある重複・巨大ファイルを、**挙動を変えないムーブ中心の分割**で整理する
4. 発見済みの正しさバグ・仕様ギャップは **勝手に直さず**、テストで固定した上で人間の承認を待つ

**見た目の綺麗さは目的ではない。** 変更は「テストが通り続けること」「diff が説明可能であること」で正当化されなければならない。

---

## 2. Project Understanding

### 2.1 このプロジェクトは何か

Rust 製の軽量インメモリ/永続化グラフデータベースライブラリ。openCypher のサブセットを実装した本格的なクエリエンジン(パーサ → プランナ → エグゼキュータ)を持ち、C ABI 経由で Python / Go / WASM / CLI から使える。

### 2.2 ワークスペース構成

| crate | 役割 | 規模感 |
|---|---|---|
| `egrph-core` | エンジン本体(parser/planner/executor/storage) | 約 14,000 行 |
| `egrph-cli` | 対話 REPL / バッチ実行 CLI (`--command`, `--file`, table/csv/json/line 出力) | 595 行 |
| `egrph-c-abi` | C ABI (staticlib/cdylib `libegrph_c`)。結果を JSON 文字列で返す | 370 行 |
| `egrph-python` | PyO3 バインディング(モジュール名 `egrph`, `PyGraph`) | 175 行 |
| `egrph-wasm` | wasm-bindgen バインディング(`WasmGraph`)。npm 公開対象 | 310 行 |
| `egrph-go` | cgo 経由で `egrph-c-abi` を呼ぶ Go パッケージ | — |

Rust toolchain は `rust-toolchain.toml` で **1.94 に固定**(edition 2024)。

### 2.3 データフロー

```
query 文字列
  → parser (PEST, egrph-core/src/parser/cypher.pest + mod.rs) → ast::Statement
  → planner (planner/mod.rs) → plan::LogicalPlan (木構造)
  → executor (executor/mod.rs execute_to_records) → (columns, Vec<Record>)
  → QueryResult { columns, rows }
```

- `Graph<S: StorageBackend>` (graph/mod.rs) がエントリーポイント。`execute()` / `execute_with_params()` が主 API。クエリ文字列キーの **plan cache**(上限 1024)を持つ。
- `Record` (executor/expression.rs) は `Arc<HashMap<String, CypherValue>>` の copy-on-write。パフォーマンス最適化(Arc 参照カウント、move-last、hash join 等)が直近コミットで大量に入っており、**このホットパスの挙動を崩さないこと**。
- ストレージは `StorageBackend` トレイト (graph/backend.rs) で抽象化。実装は `MemoryStorage` (graph/storage.rs, ラベル/プロパティ/エッジ隣接インデックス付き) と `SledStorage` (graph/sled_storage.rs, feature `sled-storage`, **オンディスクフォーマットあり**)。

### 2.4 外部依存

serde / pest / regex / csv / chrono(core)、sled + bincode(optional)、pyo3、wasm-bindgen、serde_json(各バインディング)、clap + rustyline(CLI)、criterion(bench)。ネットワーク・認証・課金・ジョブ・キューは存在しない。外部境界は「C ABI / JSON 出力契約」「sled のディスクフォーマット」「LOAD CSV のファイル読み取り」の3つ。

### 2.5 テストと検証の現状(実測ベースライン, 2026-07-02, HEAD 8783b85)

| コマンド | 結果 |
|---|---|
| `cargo test --workspace --exclude egrph-wasm` | ✅ 234 passed (core) + c-abi 5 tests |
| `cargo test -p egrph-core --features sled-storage` | ✅ 237 passed(sled テスト3件は feature 付きのみ) |
| `cargo clippy --workspace --exclude egrph-wasm -- -D warnings` | ✅ クリーン |
| `cargo fmt --all -- --check` | ❌ **失敗(16 diffs)** — 直近の perf コミットが未 fmt |

- テストはほぼ全て `egrph-core/src/lib.rs` の `mod tests`(4,306 行、237 個)に集中。他に `egrph-c-abi/src/lib.rs`(5個)、`egrph-wasm/src/lib.rs`(wasm-bindgen-test, CI の別ジョブ)、`egrph-go/egrph_test.go`。
- CI (`.github/workflows/ci.yml`): fmt / clippy / test / test-wasm(Node) / test-go。**`--features sled-storage` のテストジョブは無い。**

---

## 3. Behaviors To Preserve(絶対に壊してはいけない挙動)

1. **egrph-core の公開 API 全体**: `Graph::new / new_with_storage / execute / execute_with_params / query(deprecated) / create_node / create_edge / get_node / get_edge / match_nodes / node_count / edge_count / export_cypher`、re-export される型 (`CypherValue`, `PropertyValue`, `Node`, `Edge`, `Path`, `QueryResult`, `CypherError`, `StorageBackend`, `MemoryStorage`)、後方互換エイリアス `GraphStorage`, `InMemoryGraph`, `PersistentGraph`, `parser::parse_with_return_extraction`。
2. **`StorageBackend` トレイトのシグネチャ**(公開トレイト。外部実装者が存在しうる)。
3. **C ABI のシンボル名と JSON 出力形状**: `graph_new/free/create_node/create_edge/get_node_count/get_edge_count/execute/execute_with_params/export_cypher/free_string`。成功時 `{"columns":[...],"rows":[[...]]}`、エラー時 `{"error":"..."}`。Node は `_id`(数値)/`_labels`/`_properties`、Relationship は `_id/_type/_src/_dst/_properties`。
4. **Python**: モジュール名 `egrph`、`PyGraph` のメソッド群、JSON 形状(ID は数値)。
5. **WASM**: `WasmGraph` の API(`execute` は行オブジェクト配列の JSON、**ID は文字列** — u64 精度対策で意図的)、`nodeCount`/`edgeCount`/`exportCypher`。npm 公開物。
6. **Go**: `egrph` パッケージの公開関数群と cgo 宣言(C ABI と対)。
7. **sled のオンディスクフォーマット**: tree 名(nodes/edges/label_idx/outgoing/incoming/meta/prop_idx/constraints)とキーエンコーディング。既存 DB ファイルが読めなくなる変更は禁止。
8. **`export_cypher()` の出力形式**(`CREATE\n  (...)` 形式、プロパティのキーソート、エスケープ規則)。ラウンドトリップ(export → execute で再構築)が成立すること。
9. **237 個の既存テストが検証している Cypher セマンティクス全て**(CREATE/MATCH/OPTIONAL・MANDATORY MATCH/WHERE/WITH/UNWIND/LOAD CSV/SET/REMOVE/DELETE/MERGE + ON CREATE/ON MATCH/ORDER BY/SKIP/LIMIT/UNION/制約4種/EXISTS/shortestPath/可変長パス/パラメータ/全関数群)。
10. **CLI のフラグと出力モード**(table/csv/json/line、`.help` 等のドットコマンド)。
11. **パフォーマンス特性**: プロパティ/ラベルインデックスによる O(1) ルックアップ、`Record` の Arc-COW、CartesianProduct/CreateNode チェーンの**反復処理**(WASM のスタックオーバーフロー修正。再帰に戻すと wasm-0.3.1/0.4.0 の修正が退行する)。`egrph-core/benches/core_perf.rs` のベンチが極端に悪化しないこと。

---

## 4. Non-Negotiables(交渉不可の制約)

- 最初に `git status` を確認する。既存の未コミット変更があれば **触らず報告**し、自分の変更と混ぜない。
- 編集前に §7 の Baseline Commands を全て実行し、結果を記録する(このファイルの実測値と一致するはず。ズレたらまず報告)。
- 変更は小さく戻しやすい単位で。**1 コミット = 1 関心事**。ムーブ(ファイル分割)と編集(ロジック変更)を同一コミットに混ぜない。
- 無関係な整形・ついでのリファクタリング・コメント削除をしない(例外: Phase 1 の `cargo fmt` は専用コミットとして許可)。
- 既存挙動を勝手に変えない。§5 の条件に当たったら**実装を止めて質問**する。
- 新しい依存クレートの追加、edition / toolchain / Cargo feature 構成の変更は承認なしに行わない。
- deprecated エイリアス(`GraphStorage`, `query()`, `parse_with_return_extraction`)を削除しない。
- 各フェーズ完了ごとに §9 の検証を実行し、結果を記録してからコミットする。
- 最後に実行したコマンドと結果を必ず報告する。

---

## 5. Stop And Ask Conditions(実装を止めて質問する条件)

以下に該当したら、変更をコミットせず質問を返すこと:

1. クエリの**結果(行数・値・エラー有無)が変わる**変更 — 特に Debt D3(共有変数 join)と D4(制約チェックの抜け)の修正。テストで現状を固定するのは可、修正は承認後。
2. `StorageBackend` トレイト、C ABI シグネチャ、各バインディングの JSON 形状、sled キーフォーマットに触れる変更。
3. テストの期待値を変えないと通らない変更(= 挙動が変わっている証拠)。
4. 削除したいコードが本当に未使用か確信が持てない場合(`pub` なもの、feature ゲート下、バインディングから参照されうるもの)。
5. エラーメッセージ文字列の変更がテストや下流(バインディングの error JSON)に影響する場合。
6. 複数の設計案がありプロダクト判断が要る場合(§11 の提案項目)。

---

## 6. 実装前に確認すべき質問(人間の回答待ち)

> 回答が得られるまで、該当フェーズ(Phase 5)は着手しない。それ以外のフェーズは回答不要で進められる。

- **Q1 [D3] 複数パターン MATCH の共有変数**: `MATCH (a:P)-[:X]->(b:Q), (b)-[:Y]->(c:R)` で、2つのパターンが共有する `b` が **join されず右側の束縛で上書きされる**(実機再現済み: b1≠b2 でも 1 行返る。openCypher では 0 行が正しい)。openCypher 準拠の equality join に修正してよいか?(修正すると、このバグに依存した結果を返していたクエリの結果が変わる)
- **Q2 [D4] 制約チェックの抜け**: UNIQUE 等の制約が単独ノード CREATE でのみ検査され、**パスパターン CREATE(`CREATE (:A)-[:R]->(:U {name:'x'})`)と MERGE のチェーン作成、SET によるプロパティ変更では素通り**する(実機再現済み: UNIQUE 制約下で重複ノード3個作成成功)。どの経路まで制約を強制するか?(強制すると、今まで成功していたクエリがエラーになる)
- **Q3 [D8] バインディング JSON 契約**: ノード ID のシリアライズが wasm=文字列 / c-abi・python=数値、`Path` が wasm=エラー / c-abi・python=`"Path(...)"` と分岐している。**統一するか、現状を仕様として文書化するか**(統一は下流互換性を壊す。推奨: 文書化のみ)。
- **Q4 [D10] StorageBackend のエラー型**: 現状 `Result<_, String>` + sled 書込み失敗は `expect` で **panic**。型付きエラーへの移行はトレイトの破壊的変更になる。0.x のうちにやるか?
- **Q5 [D14] LOAD CSV のファイルアクセス**: 任意のローカルパスを無制限に読める。組込みライブラリとして許容か、パス制限(allowlist/opt-in)を入れるか?

---

## 7. Baseline Commands

作業開始時に以下を実行し、出力を記録すること(期待値は §2.5):

```bash
git status                                                   # クリーンであること
git log --oneline -3                                         # 基準コミットの確認
cargo test --workspace --exclude egrph-wasm                  # 期待: 全 pass
cargo test -p egrph-core --features sled-storage             # 期待: 237 pass
cargo clippy --workspace --exclude egrph-wasm -- -D warnings # 期待: クリーン
cargo fmt --all -- --check                                   # 期待: ❌ 16 diffs(Phase 1 で解消する)
```

環境にツールチェーンがあれば追加で:

```bash
wasm-pack test --node egrph-wasm                             # WASM テスト(CI 相当)
cd egrph-c-abi && cargo build --release && cd ../egrph-go && go test ./...  # Go(CI の手順は ci.yml 参照)
```

スモークテスト(CLI 経由の実行確認):

```bash
cargo run -q -p egrph-cli -- --command 'CREATE (:P {n:1}) RETURN 1' --mode csv
```

---

## 8. Debt Map

凡例 — **実装可**: このリファクタで実装してよい / **承認後**: Q に回答が出てから / **提案のみ**: 本指示書のスコープでは実装しない。

### D1. `cargo fmt --check` がベースラインで失敗している 【実装可・最優先】
- **根拠**: `cargo fmt --all -- --check` が 16 diffs で失敗。対象: `egrph-core/benches/core_perf.rs`(10箇所)、`executor/expression.rs:112`、`executor/mod.rs:1545`、`graph/storage.rs:342,351`、`planner/mod.rs:463,471`。toolchain は pinned 1.94 で一致確認済みなので、バージョン差ではなく未 fmt コミットが原因。
- **なぜ負債か**: CI の fmt ジョブが main で赤。以後の全 PR が fmt 起因で落ちる。
- **影響範囲**: 空白のみ。**リスク: 極小**。
- **改善案**: `cargo fmt --all` を**単独コミット**で実行。
- **検証**: `git diff -w` が空(空白以外の変更なし)であること + 全テスト pass。

### D2. CI が `sled-storage` feature をテストしていない 【実装可】
- **根拠**: `.github/workflows/ci.yml` に `--features` 指定が皆無。`lib.rs:2680` の `sled_tests`(3件)と `sled_storage.rs` 本体(1,049 行)は CI で一度もコンパイル・実行されない。
- **なぜ負債か**: 永続化バックエンドの退行が CI をすり抜ける。
- **影響範囲**: CI 設定のみ。**リスク: 極小**。
- **改善案**: test ジョブに `cargo test -p egrph-core --features sled-storage` のステップ(または matrix)を追加。Makefile の `test` ターゲットにも併記を検討。
- **検証**: ローカルで同コマンドが 237 pass。

### D3. CartesianProduct が共有変数を join しない(正しさバグ) 【テスト追加は実装可 / 修正は承認後 (Q1)】
- **根拠**: `planner/mod.rs:85-110`(カンマ区切りパターンを無条件に `CartesianProduct` 化)+ `executor/mod.rs:460-527`(`extend` で右側の束縛が左側を**上書き**。`LeftOuterJoin` には shared_vars join があるのに CartesianProduct には無い)。実機再現: `CREATE (:P {n:'a1'})-[:X]->(:Q {n:'b1'}), (:Q {n:'b2'})-[:Y]->(:R {n:'c1'})` 後、`MATCH (a:P)-[:X]->(b:Q), (b)-[:Y]->(c:R) RETURN a.n,b.n,c.n` が `a1,b2,c1` を返す(正: 0 行)。
- **なぜ負債か**: openCypher セマンティクス違反。誤った結果を静かに返す。既存テストはこのケースを 1 つもカバーしていない(コンマ区切り MATCH のテストは全て変数が非共有)。
- **影響範囲**: 複数パターン MATCH で変数を共有する全クエリ。
- **変更リスク**: 修正はクエリ結果を変える(= 挙動変更)。
- **改善案**: (a) まず現状挙動を明示した characterization test を `#[ignore]` + コメント付きで追加(Phase 2)。(b) 承認後、CartesianProduct 実行時に共有カラムの equality チェック(LeftOuterJoin と同じ `cypher_value_to_stable_key` ベースの hash join に寄せる)を実装(Phase 5)。
- **検証**: 上記再現クエリが 0 行になり、共有変数が一致するケースは従来どおり返ること。全既存テスト pass。

### D4. 制約チェックが CREATE 単独ノード経路にしかない 【テスト追加は実装可 / 修正は承認後 (Q2)】
- **根拠**: チェック呼び出しは `executor/mod.rs:855-872`(`execute_create_node_from_records` 内)のみ。`execute_create_path_from_records`(mod.rs:889-)、MERGE の作成経路(mod.rs:1527, 1853, 1866, 1882, 1896)、`apply_set_item`(SET)は `check_unique_constraint` 等を一切呼ばない。実機再現: UNIQUE 制約下で `CREATE (:A)-[:R]->(:U {name:'x'})` と `MERGE (m:V)-[:R2]->(u:U {name:'x'})` が重複 `:U {name:'x'}` を作成成功(最終的に重複3ノード)。
- **なぜ負債か**: 制約が「単独 CREATE のときだけ効く飾り」になっており、ユーザーのデータ整合性期待を裏切る。
- **影響範囲**: 制約機能を使う全ユーザー。sled バックエンドも同様(チェックは executor 層のため)。
- **変更リスク**: 強制すると今まで成功していた書込みがエラーになる(= 挙動変更)。
- **改善案**: (a) characterization test を追加(Phase 2)。(b) 承認後、ノード作成箇所を「検査付き作成ヘルパ」1 本に集約してから全経路で強制(Phase 5)。この集約自体(呼び出しの一本化、チェックは従来経路のみ有効)は挙動を変えずに先行実装してよい。
- **検証**: 承認された経路で ConstraintError が出ること、既存の制約テスト(lib.rs:2488-)が pass し続けること。

### D5. `eval_function` が約 1,200 行の単一 match + 集約関数名リストの二重管理 【実装可】
- **根拠**: `executor/expression.rs:987`〜約 2,185 行目まで、100 超の関数(グラフ introspection・文字列・リスト・数学・日付・正規表現・キャスト)が 1 つの match に同居。さらに集約関数名の集合が `aggregation.rs:19-33`(`items_contain_aggregation`)と `expression.rs`(row-level では Null を返す分岐)の 2 箇所にハードコードされ、同期を要する。
- **なぜ負債か**: 関数追加(TASK.md の履歴どおり頻繁)のたびに巨大ファイルが伸び、レビュー困難。名前リストの不一致は「集約が行レベルで Null になる/ならない」の静かなバグ源。
- **影響範囲**: 式評価全体。**変更リスク: 中**(ムーブのみなら低。237 テストが関数を広くカバーしており安全網は厚い)。
- **改善案**: ① 集約関数名を `pub(crate) const AGGREGATE_FN_NAMES: &[&str]` として 1 箇所に定義し両者から参照(小 diff、先行可)。② `eval_function` をカテゴリ別サブモジュール(例: `executor/functions/{string,list,math,datetime,graph,cast}.rs`)へ**シグネチャと dispatch を維持したまま**分割。1 カテゴリ = 1 コミット。関数名→挙動のマッピング(大文字小文字非依存の `to_lowercase` dispatch を含む)は一切変えない。
- **検証**: 各コミットで全テスト + `SHOW_FUNCTIONS` 系・日付系・リスト系テストの pass を確認。

### D6. `executor/mod.rs` が 2,563 行 【実装可】
- **根拠**: `execute_to_records` の巨大 match + CREATE/MERGE/DELETE/LOAD CSV/var-length/shortestPath の実装が全て同居。
- **なぜ負債か**: D3/D4 の修正(Phase 5)を安全にやるには、まず責務単位で見通しを良くする必要がある。
- **影響範囲**: エグゼキュータ全体。**変更リスク: 中**(ムーブのみなら低)。
- **改善案**: プラン種別ごとにサブモジュールへ**関数単位でムーブ**(例: `executor/{create,merge,mutate,csv,paths}.rs`)。`execute_to_records` の match 本体は mod.rs に残し、腕から関数を呼ぶ形は維持。可視性は `pub(super)`/`pub(crate)` 最小限。ロジック変更・最適化の「ついで」は禁止。
- **検証**: 各ムーブコミットで全テスト pass + `git log --follow` で追跡可能なこと。

### D7. テスト 237 個が `lib.rs`(4,306 行)に集中 【実装可】
- **根拠**: `egrph-core/src/lib.rs:26` 以降の単一 `mod tests`。`egrph-core/tests/`(統合テストディレクトリ)は存在しない。
- **なぜ負債か**: テスト追加のたびに巨大ファイル競合。機能領域ごとの見通しが悪い。
- **影響範囲**: テストのみ。**変更リスク: 低**。ただし **テスト関数名を変えない**こと(`cargo test test_optional_match_no_match` のような名前フィルタ実行が README/CLAUDE.md に記載されており、運用手順の一部)。
- **改善案**: テストが公開 API(`Graph::execute`)しか使っていないことを確認の上、機能別に `egrph-core/tests/*.rs` へムーブ(内部 API 依存のテストは `src/` 内の `#[cfg(test)]` サブモジュールに残す)。sled テストの `#[cfg(feature = "sled-storage")]` ゲートを維持。
- **検証**: `cargo test --workspace --exclude egrph-wasm` と `--features sled-storage` の pass 数が移動前後で一致(234 / 237)。

### D8. CypherValue→JSON 変換が 4 crate に重複 【提案のみ (Q3)】
- **根拠**: `egrph-c-abi/src/lib.rs:181-267`、`egrph-python/src/lib.rs:98-169`、`egrph-wasm/src/lib.rs:101-191`、`egrph-cli/src/main.rs:355-455` にほぼ同型の変換関数。ただし **意図的差分**あり(wasm は ID を文字列化・Path はエラー、他は数値・`"Path(...)"`、CLI は表示用も別実装)。
- **なぜ負債か**: `CypherValue` に variant を足すたび 4 箇所修正(Date/Timestamp 追加時に実際に全箇所へ波及している)。
- **変更リスク**: **高**。出力形状は各バインディングの公開契約。統一は互換性破壊。
- **改善案(提案)**: Q3 の回答後に、core に「設定可能なシリアライザ(ID 表現・Path 表現をオプション化)」を置き各バインディングから利用する案を提示。当面は各実装に「他 3 箇所と同期必須」のコメントを追記するだけに留める(コメント追記は実装可)。
- **検証**: (実装する場合)各バインディングの既存テストの JSON assertion が無変更で pass すること。

### D9. `property_values_equal` / `property_value_matches_type` の重複 【実装可】
- **根拠**: `graph/storage.rs:811-829` と `graph/sled_storage.rs:1030-1049` に同一実装が 2 つ。
- **なぜ負債か**: 比較セマンティクス(例: Int/Float 比較の扱い)を将来変える際に片方だけ直す事故が起きる。
- **影響範囲**: ストレージ層。**変更リスク: 低**。
- **改善案**: `graph/backend.rs` か `graph/types.rs` に `pub(crate)` で 1 本化し、両バックエンドから参照。挙動は現行の厳密一致(型が違えば false)のまま。
- **検証**: 全テスト + sled feature テスト pass。

### D10. ストレージ層の文字列エラー & sled 書込み panic 【提案のみ (Q4)】
- **根拠**: `StorageBackend` の write 系が `Result<_, String>` または戻り値なし。`sled_storage.rs` は書込み失敗時 `.expect("constraint insert")` 等で **panic**(グレップで多数)。executor 側は `map_err(CypherError::RuntimeError/ConstraintError)` でアドホックに包む。
- **なぜ負債か**: ディスクフルなどの I/O エラーでライブラリ利用者のプロセスが落ちる。エラー種別のマッチが文字列頼み。
- **変更リスク**: **高**(公開トレイトのシグネチャ変更 = 外部バックエンド実装者への破壊的変更)。
- **改善案(提案)**: Q4 の回答後、`StorageError` enum 導入 + `set_*` 系を `Result` 化する移行案を設計書として提出。今回は実装しない。

### D11. `parse_with_return_extraction` は `parse` の別名 【実装可】
- **根拠**: `parser/mod.rs:65-68`(「Backward-compatible alias」コメント付き)。内部呼び出しは `graph/mod.rs:65` の 1 箇所。CLAUDE.md にも旧名で記載。
- **改善案**: 内部呼び出しを `parse()` に切替え、エイリアスは `#[deprecated(note = "Use parse() instead")]` を付けて**残す**(削除禁止)。
- **変更リスク: 極小**。**検証**: ビルド + 全テスト。deprecated 警告が新規に clippy を落とさないか確認(落ちるなら allow を局所付与)。

### D12. CLAUDE.md / ドキュメントの記述が実態とズレ 【実装可】
- **根拠**: CLAUDE.md の「`Statement`: Envelopes queries with clause list, column names, and row capacity」は誤り(実際は `Query | Union | CreateConstraint`)。リポジトリ構成に `egrph-cli` と `docs/` が無い。「`GraphStorage`: Thread-safe in-memory graph」は誤り(内部同期なし、`&mut` 必須。現名称は `MemoryStorage`)。WASM ビルドコマンドも Makefile(bundler/web/node の 3 ターゲット)と不一致。ルート `functions.md` と `docs/cypher-functions.md` の重複。
- **なぜ負債か**: この指示書の読者を含む将来のエージェント/人間が誤情報で判断する。
- **改善案**: CLAUDE.md を実態に合わせて修正(構成に egrph-cli・docs 追加、Statement/MemoryStorage/スレッド安全性の記述修正、fmt/clippy/test/sled feature/Makefile ターゲットの正確なコマンド)。`functions.md` は docs への参照に一本化するか冒頭に相互参照を明記。
- **変更リスク: 極小**(ドキュメントのみ)。**検証**: 記載コマンドを全て実際に実行して確認。

### D13. NODE KEY / UNIQUE 追加時のフルスキャン系検査 【提案のみ・低優先】
- **根拠**: `storage.rs:648-732`(check_node_key がラベル集合を毎回全走査)、sled 版はさらに prefix scan 多用。`format!("{:?}")` をキー化に使うのも脆い。
- **なぜ負債か**: 大ラベルでの INSERT が O(N) 化。ただし現状の利用規模では顕在化しにくい。
- **改善案(提案)**: 制約強制の設計(D4/Q2)が決まった後に、複合キー用インデックスとセットで再設計。今回は触らない。

### D14. LOAD CSV が任意ローカルパスを読む 【提案のみ (Q5)】
- **根拠**: `executor/mod.rs:1145-1279`。`file://` プレフィックスを剥がすだけで検証なしに `File::open`。
- **なぜ負債か**: セキュリティ境界が暗黙。クエリ文字列を外部入力から組み立てるアプリに埋め込まれた場合、パストラバーサル的にローカルファイルを読める。
- **改善案(提案)**: Q5 の回答後に allowlist / opt-in フラグを設計。今回は `docs/load-csv.md` に「プロセス権限で読める任意パスにアクセスする」旨の注意書きを追記するのみ(追記は実装可)。

### D15. `plan_merge` が複数パターンを黙って先頭のみ採用 【実装可・小】
- **根拠**: `planner/mod.rs:713-726` は `pattern.parts.first()` のみ使用。ただし文法(`cypher.pest`)が `MERGE (a), (b)` を parse error にするため実害は現状なし(実機確認済み)。
- **なぜ負債か**: 文法を緩めた瞬間に「黙って捨てる」バグになる防御の穴。
- **改善案**: `parts.len() > 1` なら `SemanticError("MERGE supports exactly one pattern")` を返す明示ガードに変更。到達不能なので挙動不変。
- **検証**: 全テスト pass(パーサが先に弾くため影響なし)。

### D16. Python `execute` の欠損セル黙殺 【提案のみ・軽微】
- **根拠**: `egrph-python/src/lib.rs:88` は `i < row.values.len()` で欠損キーを黙って省略。wasm 版は Null を入れて `debug_assert`。
- **改善案(提案)**: Q3(JSON 契約)とまとめて扱う。単独では触らない。

---

## 9. Implementation Phases

> 各フェーズの完了条件: 全 Baseline Commands(§7)がグリーン(Phase 1 以降は fmt もグリーン)+ フェーズ固有の検証。1 フェーズ = 1〜数コミット。フェーズを跨ぐ変更の先取り禁止。

### Phase 0 — 現状確認(コード変更なし)
1. `git status` / `git log --oneline -3` で基準を確認。
2. §7 を全実行し、結果を作業ログに記録(この文書の実測値と比較)。
3. ズレがあれば作業を止めて報告。

### Phase 1 — 壊れたベースラインの修復(機械的・単独コミット)
1. `cargo fmt --all` → **単独コミット**。`git diff -w` が空であることを確認して記録。
2. CI に sled feature テストを追加(D2)→ 単独コミット。
3. 以後のフェーズでは fmt チェックも常時グリーンを維持。

### Phase 2 — 安全網の構築(挙動変更なし・テスト/ドキュメントのみ)
1. **D3 の characterization test**: 共有変数 MATCH の再現ケースを追加。現状の(誤った)出力を assert するのではなく、`#[ignore = "known bug: shared variables across comma-separated MATCH parts are not joined (see refactor-instructions.md D3/Q1)"]` を付けて**正しい期待値(0 行)**で書く。
2. **D4 の characterization test**: パスパターン CREATE / MERGE チェーンでの制約バイパスを同様に `#[ignore]` 付きで追加。
3. `export_cypher` ラウンドトリップ、C ABI JSON 形状(既存 5 テストで不足なら Node/Relationship/Date 形状)、CLI スモーク(`--command`)など、Behaviors To Preserve のうちテストが薄い箇所を補強。
4. D12 のドキュメント修正(CLAUDE.md ほか)。D14 の注意書き追記。

### Phase 3 — 明らかに安全な整理(小さな dedup)
1. D9: `property_values_equal` / `property_value_matches_type` の一本化。
2. D5-①: 集約関数名リストの const 化・共有。
3. D11: `parse` への内部切替 + deprecated 付与。
4. D15: `plan_merge` の明示エラーガード。
5. D8/D16 の「同期必須」コメント追記(実装はしない)。

### Phase 4 — 構造分割(ムーブのみ・1 分割 = 1 コミット)
1. D5-②: `eval_function` のカテゴリ別モジュール分割(dispatch 挙動不変)。
2. D6: `executor/mod.rs` のプラン種別モジュール分割。
3. D7: `lib.rs` テストの `tests/` への移設(テスト名不変、pass 数一致を毎回確認)。
> このフェーズでは **1 行たりともロジックを変えない**。診断で気づいたバグは修正せずメモに残し、報告に含める。

### Phase 5 — 承認ゲート付きの正しさ修正(Q1/Q2 の回答後のみ)
1. Q1 承認時: CartesianProduct に共有変数 equality join を実装(LeftOuterJoin の `build_key` 方式を流用)。`#[ignore]` を外し、性能退行がないことを bench(where-filter / merge 系)で確認。
2. Q2 承認時: ノード作成の検査付きヘルパへの集約(挙動不変の下準備は Phase 4 と同時でも可)→ 承認された経路で制約を強制。`#[ignore]` を外す。
3. どちらも CHANGELOG.md の `[Unreleased]` に Fixed として記載。

### Phase 6 — 提案書の作成(実装しない)
- D8(JSON 契約統一)、D10(StorageError 設計)、D13(制約インデックス)、Q5(LOAD CSV 制限)について、それぞれ 1 ページ以内の設計提案を最終報告に含める。

---

## 10. Verification Requirements

- **毎コミット前**: `cargo fmt --all -- --check` / `cargo clippy --workspace --exclude egrph-wasm -- -D warnings` / `cargo test --workspace --exclude egrph-wasm`。
- **storage / executor / core の型に触れたコミット**: 追加で `cargo test -p egrph-core --features sled-storage`(237 pass 維持)。
- **バインディングに触れたコミット**: 可能なら `wasm-pack test --node egrph-wasm` と Go テスト(手順は ci.yml の test-go ジョブ準拠)。ツールチェーンが無ければ「未実行」と明記して報告。
- **executor のホットパスに触れたコミット**: `cargo bench -p egrph-core`(または対象ベンチのみ)で、直近の perf コミット群(git log 参照)が守った改善を大きく退行させていないことを確認。厳密な閾値は求めないが、2 倍級の悪化が見えたら止めて報告。
- **テスト移設(D7)**: 移設前後で pass 数が 234 / 237(feature 付き)から変わらないこと。
- **ムーブ系コミット**: `git diff --stat` と、可能なら `git diff --color-moved=dimmed-zebra` で「移動のみ」であることを確認。

---

## 11. Reporting Format

最終報告には以下を含めること:

1. **実行したフェーズと各コミット一覧**(hash、1 行説明、ムーブ/編集の別)
2. **ベースライン記録**(Phase 0 の全コマンド出力要約)と**最終検証結果**(§10 の各コマンドの最後の実行結果。未実行のものは理由付きで明記)
3. **スキップ・保留した項目**とその理由
4. **Phase 4 中に発見したがあえて直さなかった問題**のリスト
5. **Phase 6 の設計提案**(該当時)
6. **人間への未回答質問**(§6 のうち残っているもの)

途中で Stop And Ask 条件に当たった場合は、その時点までのコミットと質問内容を同じ形式で報告する。

---

## 12. Out-of-scope Items(今回やらないこと)

- 新機能(未実装の Cypher 構文・関数)の追加。TASK.md Phase 3 に「実装しない」と明記されたテーブル系関数も対象外。
- パフォーマンス最適化(直近コミットで集中的に実施済み。退行させないことだけが責務)。
- `StorageBackend` トレイト・C ABI・各バインディング JSON 形状・sled ディスクフォーマットの変更(Q3/Q4 は提案止まり)。
- deprecated API の削除、crate バージョンの引き上げ、リリース作業、npm/PyPI 公開。
- 依存クレートの追加・更新(`Cargo.lock` の無断更新を含む)。
- エラーメッセージ文言の網羅的な統一(D10 の設計提案に委ねる)。
- パーサ文法(`cypher.pest`)の変更。
- README / docs の全面書き直し(D12 で実態と食い違う箇所の修正のみ)。
