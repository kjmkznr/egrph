# Implementation Tasks

Plan: `/home/kkojima/.claude/plans/functions-md-askuserquestion-compressed-milner.md`

## Phase 1: 純粋スカラー/リスト関数の追加

型システム変更なし。`eval_function()` にケースを追加するのみ。

- [x] キャストエイリアス: `STRING/CAST_TO_STRING`, `INT64/CAST_TO_INT64`, `DOUBLE/CAST_TO_DOUBLE`
- [x] 文字列: `CONCAT`
- [x] 正規表現関数: `REGEXP_MATCHES`, `REGEXP_REPLACE`, `REGEXP_EXTRACT`
- [x] リスト: `LIST_APPEND/array_append`, `LIST_PREPEND/array_prepend`
- [x] リスト: `LIST_EXTRACT/array_extract` (1-based)
- [x] リスト: `LIST_CONTAINS/array_contains`
- [x] リスト: `LIST_SORT/array_sort`
- [x] リスト: `LIST_DISTINCT/array_distinct`
- [x] リスト: `LIST_CREATION/list_value/make_list`
- [x] イントロスペクション: `SHOW_FUNCTIONS`
- [x] テスト追加 (`egrph-core/src/lib.rs`)
- [x] `functions.md` テーブル関数節を削除

## Phase 2: Date/Timestamp 型と関数群

`CypherValue` に `Date`/`Timestamp` variant を追加。chrono クレート導入。

- [x] `egrph-core/Cargo.toml` に chrono 追加
- [x] `egrph-core/src/graph/types.rs` — CypherValue に Date/Timestamp variant 追加
- [x] `compare_values`, `cypher_value_to_string`, `cypher_value_to_stable_key`, 各クレートの JSON変換への波及対応
- [x] 関数実装: `CURRENT_DATE`, `CURRENT_TIMESTAMP`
- [x] 関数実装: `MAKE_DATE(year, month, day)`
- [x] 関数実装: `DATE_PART(part, date_or_ts)`
- [x] 関数実装: `DATE_TRUNC(part, date_or_ts)`
- [x] 関数実装: `TO_TIMESTAMP(epoch_seconds)`, `EPOCH_MS(ts)`
- [x] 関数実装: `DATE(x)` / `CAST_TO_DATE(x)` (ISO 8601 文字列から変換)
- [x] テスト追加 (200 tests passed)
- [x] `functions.md` の更新(テーブル関数節削除、日付関数節更新)

## Phase 3 (対象外)

`SHOW_TABLES / SHOW_CONNECTIONS / PARQUET_SCAN / CSV_SCAN / EXPORT_CSV` はグラフ DB の趣旨と合わないため実装しない。`functions.md` のテーブル関数節は削除済み。
