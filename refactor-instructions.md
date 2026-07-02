# refactor-instructions.md — egrph リファクタリング指示書

対象リポジトリ: `kjmkznr/egrph`
基準コミット: `8783b85` (main / 2026-07-02 時点)
この指示書は事前のコードベース全読 + 実機検証(CLI でのクエリ再現、テスト・lint・fmt のベースライン実行)に基づいて作成されている。
§8B の openCypher 準拠ギャップは全件、CLI(`cargo run -p egrph-cli -- --file <probes>`)で再現確認済み。

---

## 1. Objective

**既存仕様を一切壊さずに**、egrph の技術的負債を減らし、今後の変更を安全にする。

具体的なゴール:

1. 壊れているベースライン(`cargo fmt --check` 失敗、CI の sled feature 未テスト)を修復する
2. 重要挙動に安全網(characterization test)を張る
3. 証拠のある重複・巨大ファイルを、**挙動を変えないムーブ中心の分割**で整理する
4. **§8B の openCypher 準拠ギャップ(C1〜C12)を、指定の優先順で仕様準拠に改修する**(2026-07-02 ユーザー指示によりタスク化済み。ただし §6 の未回答質問が付くものは回答待ち)
5. それ以外の正しさ・仕様ギャップ(制約強制 D4 等)は **勝手に直さず**、テストで固定した上で人間の承認を待つ

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

1. クエリの**結果(行数・値・エラー有無)が変わる**変更のうち、§8B(C1〜C12)として明示的にタスク化されて**いない**もの — 特に D4(制約チェックの抜け)の修正。テストで現状を固定するのは可、修正は承認後。C1〜C12 は Phase 5 の手順に従って実装してよいが、**期待する準拠挙動が本書の記載から一意に決まらないケースに遭遇したら止めて質問**する。
2. `StorageBackend` トレイト、C ABI シグネチャ、各バインディングの JSON 形状、sled キーフォーマットに触れる変更。
3. テストの期待値を変えないと通らない変更(= 挙動が変わっている証拠)。
4. 削除したいコードが本当に未使用か確信が持てない場合(`pub` なもの、feature ゲート下、バインディングから参照されうるもの)。
5. エラーメッセージ文字列の変更がテストや下流(バインディングの error JSON)に影響する場合。
6. 複数の設計案がありプロダクト判断が要る場合(§11 の提案項目)。

---

## 6. 実装前に確認すべき質問(人間の回答待ち)

> 回答が得られるまで、該当タスクは着手しない。それ以外は回答不要で進められる。
>
> **【解決済み】旧 Q1(複数パターン MATCH の共有変数 join)**: 2026-07-02 のユーザー指示「openCypher 非準拠箇所を洗い出し改修タスクに追加」により、openCypher 準拠化(C1)としてタスク化済み。承認待ち不要。

- **Q2 [D4] 制約チェックの抜け**: UNIQUE 等の制約が単独ノード CREATE でのみ検査され、**パスパターン CREATE(`CREATE (:A)-[:R]->(:U {name:'x'})`)と MERGE のチェーン作成、SET によるプロパティ変更では素通り**する(実機再現済み: UNIQUE 制約下で重複ノード3個作成成功)。どの経路まで制約を強制するか?(強制すると、今まで成功していたクエリがエラーになる。※制約は openCypher 仕様外の Neo4j 拡張なので C 系には含めず、承認ゲートを維持)
- **Q3 [D8] バインディング JSON 契約**: ノード ID のシリアライズが wasm=文字列 / c-abi・python=数値、`Path` が wasm=エラー / c-abi・python=`"Path(...)"` と分岐している。**統一するか、現状を仕様として文書化するか**(統一は下流互換性を壊す。推奨: 文書化のみ)。
- **Q4 [D10] StorageBackend のエラー型**: 現状 `Result<_, String>` + sled 書込み失敗は `expect` で **panic**。型付きエラーへの移行はトレイトの破壊的変更になる。0.x のうちにやるか?
- **Q5 [D14] LOAD CSV のファイルアクセス**: 任意のローカルパスを無制限に読める。組込みライブラリとして許容か、パス制限(allowlist/opt-in)を入れるか?
- **Q6 [C4-b] RETURN 列名の準拠化**: openCypher では未エイリアス式の列名は**式のテキストそのまま**(例: `1+1`, `count(*)`)。現状は `?column?` / `count(..)`。準拠化すると **バインディング JSON の行オブジェクトのキーが変わる**(下流互換性に影響)。列名準拠化を実施してよいか?(値衝突バグ C4-a の修正自体は列名を変えずに可能なので先行実装する)
- **Q7 [C10] float の 0 除算**: 現状 `1.0/0.0` は **NULL**。openCypher/Neo4j は IEEE 754 準拠で `Infinity`。ただし Infinity は JSON 非表現(c-abi は非有限 float を null 化、wasm はエラー化)のため、準拠化するとバインディング出力に波及する。`Infinity` 準拠にするか、現状 NULL を仕様として文書化するか?
- **Q8 [C12] 文字列 + 数値**: 現状 `'a' + 1` は **NULL**。Neo4j は `"a1"`(連結)。openCypher 仕様は string+string のみ規定。Neo4j 互換の連結にするか、現状 NULL のままか?

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

### D3. CartesianProduct が共有変数を join しない(正しさバグ) 【→ C1 としてタスク化済み・Phase 5 で修正】
- **根拠**: `planner/mod.rs:85-110`(カンマ区切りパターンを無条件に `CartesianProduct` 化)+ `executor/mod.rs:460-527`(`extend` で右側の束縛が左側を**上書き**。`LeftOuterJoin` には shared_vars join があるのに CartesianProduct には無い)。実機再現: `CREATE (:P {n:'a1'})-[:X]->(:Q {n:'b1'}), (:Q {n:'b2'})-[:Y]->(:R {n:'c1'})` 後、`MATCH (a:P)-[:X]->(b:Q), (b)-[:Y]->(c:R) RETURN a.n,b.n,c.n` が `a1,b2,c1` を返す(正: 0 行)。
- **なぜ負債か**: openCypher セマンティクス違反。誤った結果を静かに返す。既存テストはこのケースを 1 つもカバーしていない(コンマ区切り MATCH のテストは全て変数が非共有)。
- **影響範囲**: 複数パターン MATCH で変数を共有する全クエリ。
- **変更リスク**: 修正はクエリ結果を変える(= 挙動変更)。
- **改善案**: (a) まず正しい期待値の characterization test を `#[ignore]` + コメント付きで追加(Phase 2)。(b) Phase 5 C1 として、CartesianProduct 実行時に共有カラムの equality チェック(LeftOuterJoin と同じ `cypher_value_to_stable_key` ベースの hash join に寄せる)を実装。**カンマ区切りだけでなく連続する `MATCH` 句でも同一バグが再現済み**(詳細は C1)。
- **検証**: 上記再現クエリが 0 行になり、共有変数が一致するケースは従来どおり返ること。全既存テスト pass。

### D4. 制約チェックが CREATE 単独ノード経路にしかない 【テスト追加は実装可 / 修正は承認後 (Q2)】
- **根拠**: チェック呼び出しは `executor/mod.rs:855-872`(`execute_create_node_from_records` 内)のみ。`execute_create_path_from_records`(mod.rs:889-)、MERGE の作成経路(mod.rs:1527, 1853, 1866, 1882, 1896)、`apply_set_item`(SET)は `check_unique_constraint` 等を一切呼ばない。実機再現: UNIQUE 制約下で `CREATE (:A)-[:R]->(:U {name:'x'})` と `MERGE (m:V)-[:R2]->(u:U {name:'x'})` が重複 `:U {name:'x'}` を作成成功(最終的に重複3ノード)。
- **なぜ負債か**: 制約が「単独 CREATE のときだけ効く飾り」になっており、ユーザーのデータ整合性期待を裏切る。
- **影響範囲**: 制約機能を使う全ユーザー。sled バックエンドも同様(チェックは executor 層のため)。
- **変更リスク**: 強制すると今まで成功していた書込みがエラーになる(= 挙動変更)。
- **改善案**: (a) characterization test を追加(Phase 2)。(b) 承認後、ノード作成箇所を「検査付き作成ヘルパ」1 本に集約してから全経路で強制(Phase 6)。この集約自体(呼び出しの一本化、チェックは従来経路のみ有効)は挙動を変えずに先行実装してよい。
- **検証**: 承認された経路で ConstraintError が出ること、既存の制約テスト(lib.rs:2488-)が pass し続けること。

### D5. `eval_function` が約 1,200 行の単一 match + 集約関数名リストの二重管理 【実装可】
- **根拠**: `executor/expression.rs:987`〜約 2,185 行目まで、100 超の関数(グラフ introspection・文字列・リスト・数学・日付・正規表現・キャスト)が 1 つの match に同居。さらに集約関数名の集合が `aggregation.rs:19-33`(`items_contain_aggregation`)と `expression.rs`(row-level では Null を返す分岐)の 2 箇所にハードコードされ、同期を要する。
- **なぜ負債か**: 関数追加(TASK.md の履歴どおり頻繁)のたびに巨大ファイルが伸び、レビュー困難。名前リストの不一致は「集約が行レベルで Null になる/ならない」の静かなバグ源。
- **影響範囲**: 式評価全体。**変更リスク: 中**(ムーブのみなら低。237 テストが関数を広くカバーしており安全網は厚い)。
- **改善案**: ① 集約関数名を `pub(crate) const AGGREGATE_FN_NAMES: &[&str]` として 1 箇所に定義し両者から参照(小 diff、先行可)。② `eval_function` をカテゴリ別サブモジュール(例: `executor/functions/{string,list,math,datetime,graph,cast}.rs`)へ**シグネチャと dispatch を維持したまま**分割。1 カテゴリ = 1 コミット。関数名→挙動のマッピング(大文字小文字非依存の `to_lowercase` dispatch を含む)は一切変えない。
- **検証**: 各コミットで全テスト + `SHOW_FUNCTIONS` 系・日付系・リスト系テストの pass を確認。

### D6. `executor/mod.rs` が 2,563 行 【実装可】
- **根拠**: `execute_to_records` の巨大 match + CREATE/MERGE/DELETE/LOAD CSV/var-length/shortestPath の実装が全て同居。
- **なぜ負債か**: C 系の準拠化(Phase 5)と D4 の修正(Phase 6)を安全にやるには、まず責務単位で見通しを良くする必要がある。
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

## 8B. openCypher 準拠ギャップマップ(C1〜C12・全件実機再現済み)

> **準拠基準**: openCypher 仕様(および openCypher TCK)を正とする。仕様が沈黙している箇所は Neo4j 5 の挙動を参照実装とする。両者が食い違う・バインディング契約に波及する項目は §6 の質問(Q6/Q7/Q8)で回答を待つ。
>
> **再現方法**: 各項目の再現クエリを `cargo run -q -p egrph-cli -- --file <file> --mode csv` で実行(セミコロン区切り、毎回新規インメモリグラフ)。2026-07-02 に基準コミットで全件再現済み。
>
> **既存テストとの整合**: 以下の全修正について、既存 237 テストに矛盾する期待値を持つものが**無い**ことを確認済み(`OPTIONAL MATCH ... WHERE` を使うテスト 0 件、NULL を含む ORDER BY / count テスト 0 件、`?column?` 依存 0 件。lib.rs:1803 の `MATCH (a:Person) MANDATORY MATCH (a)-[:KNOWS]->(b)` は共有変数を跨ぐ唯一のテストだが、修正前後どちらでも 0 行 → エラーで結果が一致する)。もし修正中に既存テストの期待値変更が必要になったら、それは想定外の波及なので**止めて質問**すること。

### C1. パターン間・MATCH 句間で共有変数が join されない 【最重要・実装可(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:P {n:'a1'})-[:X]->(:Q {n:'b1'}), (:Q {n:'b2'})-[:Y]->(:R {n:'c1'});
  MATCH (a:P)-[:X]->(b:Q), (b)-[:Y]->(c:R) RETURN a.n, b.n, c.n;   -- 実際: a1,b2,c1 / 正: 0 行
  ```
  カンマ区切りだけでなく **連続する MATCH 句**(`MATCH (a:P)-[:X]->(b:Q) MATCH (b)-[:Y]->(c:R)`)でも同一結果を確認済み。`b` の束縛が右側スキャンの結果で黙って上書きされる。
- **原因**: `planner/mod.rs:85-110` が共有変数の有無に関係なく `CartesianProduct` を生成し、`executor/mod.rs:460-527` の merge が `Record::extend` で上書きする(`LeftOuterJoin` には shared_vars の hash join があるのに CartesianProduct には無い)。
- **正しい仕様**: 同一クエリ内で同名変数は同一エンティティに束縛される(equality join)。
- **修正方針**: CartesianProduct 実行時に左右の共有カラムを検出し、`LeftOuterJoin` と同じ `cypher_value_to_stable_key` ベースの hash join(inner join 版)で結合する。共有カラムが無ければ現行の直積 + 高速パスを維持(性能退行させない)。
- **検証**: 再現クエリ(カンマ形・連続 MATCH 形の両方)が 0 行 / 共有変数が一致するケースは行が返る / 全既存テスト + bench 退行なし。

### C2. OPTIONAL MATCH の WHERE が後置フィルタになり null 行が消える 【実装可(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:Person {name:'Alice'})-[:HAS]->(:Item {v:1});
  CREATE (:Person {name:'Bob'})-[:HAS]->(:Item {v:2});
  MATCH (p:Person) OPTIONAL MATCH (p)-[:HAS]->(m:Item) WHERE m.v = 1 RETURN p.name, m.v;
  -- 実際: Alice,1 の 1 行のみ / 正: Alice,1 と Bob,NULL の 2 行
  ```
- **原因**: パーサが WHERE を独立句として積み、`plan_where` が **LeftOuterJoin の外側**に Filter を置くため、`m.v = 1` が NULL 行(Bob)を落とす。openCypher では WHERE は直前の OPTIONAL MATCH の**一部**であり、マッチ候補の絞り込みにのみ作用する。
- **修正方針**: プランナで「直前句が OPTIONAL MATCH の WHERE」を検出し、述語を LeftOuterJoin の**右レッグ内**(scan/expand 直後)へ押し込む。通常 MATCH の WHERE は現行どおり。パーサで WHERE を MatchClause に取り込む改修でも可(AST 変更は最小限に)。
- **検証**: 再現クエリが 2 行(Bob は NULL)になること / 通常 MATCH + WHERE の既存テスト群が全 pass。

### C3. リレーションシップ一意性(isomorphism)未実装 【実装可(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:A {n:'a'})-[:R]->(:B {n:'b'});
  MATCH (x)-[r1]->(y)<-[r2]-(z) RETURN x.n, z.n;   -- 実際: a,a の 1 行(r1=r2 同一辺) / 正: 0 行
  ```
- **原因**: `Expand`(executor/mod.rs:156-238)が同一 MATCH パターン内で既に束縛された辺を除外しない。openCypher は**同一パターン内の関係は互いに異なる**ことを要求する(ノードは重複可)。なお可変長パス内部の辺重複は `path_chain_contains` で除外済み(準拠確認済み、下記リスト参照)。
- **修正方針**: プランナがパターン部ごとの関係変数(匿名含む — 匿名辺には内部変数を強制割当て)を把握しているので、Expand/VarLengthExpand に「同一パターン内で束縛済みの辺 ID 集合と重複したら捨てる」チェックを追加する。パターン境界(カンマ区切りの別パート・別 MATCH 句)を跨ぐ辺の重複は**許容**(仕様どおり)なので、一意性スコープを 1 パターン部に限定すること。
- **検証**: 再現クエリ 0 行 / `(a)-[r1]->(b)-[r2]->(c)` の直鎖・三角形などの既存テスト pass / 別 MATCH 句間では同一辺の再束縛が引き続き可能なこと。

### C4. RETURN 列名の非準拠と、重複列名による値衝突 【C4-a は実装可(Phase 5)/ C4-b は Q6 回答待ち】
- **再現**:
  ```cypher
  RETURN 1+1, 2+2;   -- 実際: 列名 ?column?,?column? で値 4,4(両方とも後者の値!) / 正: 2,4
  ```
- **原因**: `expr_to_column_name`(executor/mod.rs:2324)が変数・プロパティ・関数以外を一律 `"?column?"` にし、Return 射影(mod.rs:296-305)が**列名をキーにした HashMap** に insert するため、同名列が上書きされる。`records_to_query_result` も列名で引くため、同名列は全て最後の値になる。
- **修正方針**:
  - **C4-a(値衝突の修正・列名は不変)**: 射影と結果構築を「列名キー」から**位置ベース**に変える(例: Return 射影で `Vec<CypherValue>` を持つ、または内部列名を `?column?_1` 等でユニーク化して表示名と分離)。出力される列名文字列は現状のまま維持し、**値だけ正しくする**。
  - **C4-b(列名を式テキストに準拠化)**: openCypher は未エイリアス式の列名を式の原文(`1+1`, `count(*)`)とする。これはバインディング JSON のキー変更 = 互換性影響のため **Q6 の回答後**に実施。
- **検証**: `RETURN 1+1, 2+2` が 2,4 / 集約列 `RETURN t.k, count(*)` の値が従来どおり / c-abi・wasm・python の既存 JSON テスト pass(C4-a では列名不変のはず)。

### C5. ORDER BY で NULL が昇順の先頭に来る 【実装可(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:O {v:2}), (:O), (:O {v:1});
  MATCH (o:O) RETURN o.v ORDER BY o.v;   -- 実際: NULL,1,2 / 正: 1,2,NULL(昇順は NULL 末尾)
  ```
- **原因**: `Sort`(executor/mod.rs:327-356)が `compare_values(...).unwrap_or(Equal)` で NULL を「等しい」扱いにし、実質挿入順に依存。openCypher は昇順で NULL 最後、降順で NULL 最初。
- **修正方針**: Sort の比較器で NULL を明示処理(asc: null は最大値扱い / desc はその逆)。`compare_values` 自体のセマンティクス(式評価の 3 値論理)は**変えない**こと — 変更は Sort 内に閉じる。ついでに `min`/`max` 集約が NULL をスキップしているかを確認し、していなければ同フェーズで揃える。
- **検証**: 再現クエリの順序 / DESC で NULL 先頭 / 既存 ORDER BY テスト群 pass。

### C6. `count(expr)` が NULL をカウントする 【実装可(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:V {v:1}), (:V {v:2}), (:V);
  MATCH (n:V) RETURN count(n.v), sum(n.v), avg(n.v), count(*);
  -- 実際: 3,3,1.5,3 / 正: 2,3,1.5,3(count(expr) は NULL を数えない。sum/avg は既に準拠)
  ```
- **原因**: `aggregation.rs` の count 分岐が評価結果の NULL を除外していない(sum/avg は除外済みなので不整合)。
- **修正方針**: count(expr) の集計時に `CypherValue::Null` をスキップ。`count(*)` は行数のまま。`count(DISTINCT expr)` の NULL 除外も同時に確認(DISTINCT 自体は準拠確認済み)。
- **検証**: 再現クエリが 2,3,1.5,3 / 既存 count テスト pass。

### C7. WITH で射影しなかった変数がエラーにならず NULL になる 【実装可・ただし段階導入(Phase 5 後半)】
- **再現**:
  ```cypher
  CREATE (:S {x:1});
  MATCH (n:S) WITH n.x AS x RETURN n;   -- 実際: NULL の 1 行 / 正: コンパイルエラー(n is not defined)
  ```
- **原因**: 未定義変数の参照が実行時に黙って NULL になる(`Expression::Variable` が record に無ければ Null)。プランナに変数スコープ検証が無い(CLAUDE.md の「VariableSet で追跡」という記述は実装に存在しない誤記 — D12 で修正済みのはず)。
- **修正方針**: プランナで句ごとの可視変数集合を追跡し、**WITH/RETURN の射影対象・WHERE/ORDER BY が参照する変数が未定義なら `SemanticError`** を返す。ただし影響範囲が広いため、(1) まず WITH 通過後の「射影で消えた変数」の参照のみをエラー化、(2) 全面的な未定義変数検証は既存テストへの影響を計測してから拡大、の 2 段階で行う。パターン内の新規束縛・UNWIND alias・LOAD CSV alias・パス変数を漏らさず「定義」として扱うこと。
- **検証**: 再現クエリがエラー / 既存テスト(特に OPTIONAL MATCH で NULL になる正当ケース)が全 pass。**既存テストが落ちる場合はスコープ規則の解釈を示して質問**。

### C8. 束縛済み変数の単独 CREATE が黙って新ノードを作る 【実装可(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:C {n:1});
  MATCH (a:C) CREATE (a) RETURN a;   -- 実際: ラベル無し新ノードを作成し a を再束縛(総ノード数 2) / 正: エラー(Variable `a` already declared)
  ```
- **原因**: `execute_create_node_from_records`(executor/mod.rs:830)が変数の既存束縛を確認しない。※パスパターン内での束縛済み変数の再利用(`MATCH (a),(b) CREATE (a)-[:R]->(b)`)は**正当**で、`execute_create_path_from_records` が既に正しく処理している — この挙動は壊さないこと。
- **修正方針**: CreateNode(単独ノードパターン)で変数が既に束縛済みなら `SemanticError` を返す。CreatePath の始点・終点での再利用は現行維持。
- **検証**: 再現クエリがエラー / `MATCH ... CREATE (a)-[:R]->(b)` 系の既存テスト(lib.rs:1888 ほか多数)が全 pass。

### C9. 無向マッチで自己ループが 2 行重複する 【実装可・小(Phase 5)】
- **再現**:
  ```cypher
  CREATE (a:L {n:'a'})-[:R]->(a);
  MATCH (x:L)-[r]-(y) RETURN x.n, y.n;   -- 実際: a,a が 2 行 / 正(Neo4j): 1 行
  ```
- **原因**: `Expand` の Undirected が outgoing+incoming を連結するため、自己ループ辺が両リストに現れて二重マッチする。
- **修正方針**: Undirected 展開時、`edge.src == edge.dst` の辺は片側のみ採用(重複除去)。`VarLengthExpand` の同種経路も確認。
- **検証**: 再現クエリ 1 行 / 非自己ループの無向マッチが従来どおり双方向 2 行を返すこと。

### C10. float の 0 除算が NULL を返す 【Q7 回答待ち】
- **再現**: `RETURN 1.0/0.0` → 実際: NULL / openCypher・Neo4j: `Infinity`(IEEE 754)。整数 `1/0` は現状エラーで、これは Neo4j と同挙動(準拠)。
- **保留理由**: `Infinity` は JSON 非表現で、c-abi は非有限 float を null 化・wasm はエラー化する実装のため、準拠化はバインディング契約に波及する。Q7 の回答後に対応。

### C11. WHERE 内の集約関数がエラーにならず空結果になる 【実装可・小(Phase 5)】
- **再現**:
  ```cypher
  CREATE (:W {v:1});
  MATCH (w:W) WHERE count(w) > 0 RETURN w.v;   -- 実際: 0 行(count が行文脈で NULL → 全行除外) / 正: 構文エラー
  ```
- **原因**: 集約関数が行文脈で黙って NULL を返す設計(expression.rs の該当分岐)+ プランナに検証なし。
- **修正方針**: `plan_where` で述語に集約関数が含まれる場合 `SemanticError("Invalid use of aggregating function")` を返す(`items_contain_aggregation` の式版が aggregation.rs に既にあるので流用)。WITH の WHERE(集約後のフィルタ、HAVING 相当)は**合法**なので誤検知しないこと。
- **検証**: 再現クエリがエラー / `WITH ... WHERE cnt > 1` 系の既存テスト pass。

### C12. `'a' + 1` が NULL を返す 【Q8 回答待ち】
- **再現**: `RETURN 'a' + 1` → 実際: NULL / Neo4j: `"a1"`(文字列連結)。openCypher 仕様は string+string のみ規定。Q8 の回答後に対応。

### 準拠を確認済みの挙動(再検証不要・変更禁止)

以下は今回のプローブで **openCypher 準拠を確認済み**。実装担当はこれらを「直すべきもの」と誤認しないこと:

- 3値論理: `1 = null` / `null = null` / `1 <> null` → NULL、`null IS NULL` → true、`WHERE null` → 全行除外
- `IN` の NULL 伝播、型を跨ぐ比較(`1 < 'a'` → NULL)
- `UNWIND null` / `UNWIND []` → 0 行
- 暗黙のグルーピング(`RETURN t.k, count(*)` がキー毎に集計)、`count(DISTINCT ...)`
- `sum`/`avg` の NULL スキップ、`count(*)` の行数カウント
- 整数除算 `3/2` → 1、`2^3` → 8.0(float)、整数 `1/0` → エラー
- 可変長パス内の辺再利用禁止(`(s)-[*1..3]-(t)` が同一辺を往復しない)
- MERGE の複数マッチ時の全行返却、OPTIONAL MATCH 単体の NULL 行生成

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
1. **C1〜C12 の準拠テストを全件追加**: §8B の再現クエリをそのままテスト化し、**openCypher 準拠の正しい期待値**で書いた上で `#[ignore = "openCypher non-compliance C<N>: <一行説明> (see refactor-instructions.md §8B)"]` を付ける。Phase 5 で修正するたびに該当 `#[ignore]` を外す(= 修正の完了条件)。Q6/Q7/Q8 待ちの C4-b/C10/C12 も期待値をテストとして固定だけしておく。
2. **§8B「準拠を確認済みの挙動」のうちテストが無いものを通常テストとして追加**(3値論理、UNWIND null、count(DISTINCT)、可変長パスの辺再利用禁止など)。Phase 5 の修正がこれらを壊した場合に即検知するため。
3. **D4 の characterization test**: パスパターン CREATE / MERGE チェーンでの制約バイパスを `#[ignore]` 付きで追加(修正は Q2 回答後)。
4. `export_cypher` ラウンドトリップ、C ABI JSON 形状(既存 5 テストで不足なら Node/Relationship/Date 形状)、CLI スモーク(`--command`)など、Behaviors To Preserve のうちテストが薄い箇所を補強。
5. D12 のドキュメント修正(CLAUDE.md ほか)。D14 の注意書き追記。

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

### Phase 5 — openCypher 準拠化(C 系・タスク化済み)

> 前提: Phase 2 の準拠テストと Phase 4 の構造分割が済んでいること(executor を触る修正が多いため、分割後の方が安全)。**1 項目 = 1 コミット**。各コミットで該当 `#[ignore]` を外し、§10 の全検証 + bench 確認を行い、CHANGELOG.md の `[Unreleased]` に Fixed として記載する。

推奨実装順(依存が少なく影響が局所的なものから):

1. **C6** `count(expr)` の NULL スキップ(aggregation.rs 内で完結)
2. **C5** ORDER BY の NULL 順序(Sort 比較器内で完結。min/max の NULL 扱いも同時確認)
3. **C4-a** RETURN 重複列の値衝突修正(**列名は変えない**。射影の位置ベース化)
4. **C11** WHERE 内集約の SemanticError 化(plan_where の検証追加)
5. **C8** 束縛済み変数の単独 CREATE を SemanticError 化(CreatePath の正当な再利用は維持)
6. **C9** 無向自己ループの重複除去
7. **C1** 共有変数の equality join(CartesianProduct への inner hash join 導入。**カンマ区切り・連続 MATCH の両形をテスト**。共有変数なしの直積の高速パスと性能を維持)
8. **C3** リレーションシップ一意性(匿名辺への内部変数割当て + パターン部スコープの辺 ID 重複チェック)
9. **C2** OPTIONAL MATCH + WHERE の述語押し込み(プランナ改修。C1 完了後のほうが LeftOuterJoin まわりの見通しが良い)
10. **C7** WITH スコープ検証(2 段階導入。既存テストが落ちたら止めて質問)

保留(回答待ち): **C4-b**(列名準拠化, Q6)/ **C10**(float 0 除算 → Infinity, Q7)/ **C12**(`'a'+1`, Q8)。回答が来たら本フェーズ末尾に追加。

### Phase 6 — 承認ゲート付きの正しさ修正(Q2 の回答後のみ)
1. Q2 承認時: ノード作成の検査付きヘルパへの集約(挙動不変の下準備は Phase 4 と同時でも可)→ 承認された経路で制約を強制。D4 の `#[ignore]` を外す。
2. CHANGELOG.md の `[Unreleased]` に Fixed として記載。

### Phase 7 — 提案書の作成(実装しない)
- D8(JSON 契約統一)、D10(StorageError 設計)、D13(制約インデックス)、Q5(LOAD CSV 制限)、および未回答の Q6/Q7/Q8 について、それぞれ 1 ページ以内の設計提案を最終報告に含める。

---

## 10. Verification Requirements

- **毎コミット前**: `cargo fmt --all -- --check` / `cargo clippy --workspace --exclude egrph-wasm -- -D warnings` / `cargo test --workspace --exclude egrph-wasm`。
- **storage / executor / core の型に触れたコミット**: 追加で `cargo test -p egrph-core --features sled-storage`(237 pass 維持)。
- **バインディングに触れたコミット**: 可能なら `wasm-pack test --node egrph-wasm` と Go テスト(手順は ci.yml の test-go ジョブ準拠)。ツールチェーンが無ければ「未実行」と明記して報告。
- **executor のホットパスに触れたコミット**: `cargo bench -p egrph-core`(または対象ベンチのみ)で、直近の perf コミット群(git log 参照)が守った改善を大きく退行させていないことを確認。厳密な閾値は求めないが、2 倍級の悪化が見えたら止めて報告。
- **テスト移設(D7)**: 移設前後で pass 数が 234 / 237(feature 付き)から変わらないこと。
- **ムーブ系コミット**: `git diff --stat` と、可能なら `git diff --color-moved=dimmed-zebra` で「移動のみ」であることを確認。
- **Phase 5(C 系)の各コミット**: 該当 C 項目の `#[ignore]` を外してテストが pass すること、§8B「準拠を確認済みの挙動」のテスト群が引き続き pass すること、他の C 項目の `#[ignore]` テストの期待値を書き換えていないことを確認。

---

## 11. Reporting Format

最終報告には以下を含めること:

1. **実行したフェーズと各コミット一覧**(hash、1 行説明、ムーブ/編集の別)
2. **ベースライン記録**(Phase 0 の全コマンド出力要約)と**最終検証結果**(§10 の各コマンドの最後の実行結果。未実行のものは理由付きで明記)
3. **スキップ・保留した項目**とその理由
4. **Phase 4 中に発見したがあえて直さなかった問題**のリスト
5. **Phase 7 の設計提案**(該当時)
6. **C1〜C12 の対応状況一覧**(修正済み / `#[ignore]` のまま回答待ち / 未着手、を項目ごとに明記)
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
- パーサ文法(`cypher.pest`)の変更 — ただし **C2(OPTIONAL MATCH の WHERE 帰属)の実装に最小限必要な範囲は例外として許可**。その場合も受理するクエリ集合を広げない・狭めないこと(既存の全テストクエリが引き続き同じ AST 相当に解釈されること)を確認する。
- README / docs の全面書き直し(D12 で実態と食い違う箇所の修正のみ)。
