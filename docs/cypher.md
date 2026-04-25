# サポートされている Cypher クエリ

Egrph は openCypher のサブセットを実装したクエリエンジンを内蔵しています。本ドキュメントは egrph のパーサ・プランナ・実行器が解釈できる Cypher の構文要素を網羅的に列挙します。組み込み関数の詳細は [cypher-functions.md](./cypher-functions.md) を参照してください。

## 目次

- [文 (Statement)](#文-statement)
- [句 (Clause)](#句-clause)
  - [MATCH / OPTIONAL MATCH](#match--optional-match)
  - [CREATE](#create)
  - [MERGE](#merge)
  - [SET](#set)
  - [REMOVE](#remove)
  - [DELETE / DETACH DELETE](#delete--detach-delete)
  - [WHERE](#where)
  - [WITH](#with)
  - [RETURN](#return)
  - [UNWIND](#unwind)
  - [LOAD CSV](#load-csv)
  - [UNION / UNION ALL](#union--union-all)
  - [CREATE CONSTRAINT](#create-constraint)
- [パターン](#パターン)
- [式と演算子](#式と演算子)
- [リテラル](#リテラル)
- [パラメータ](#パラメータ)
- [型](#型)
- [コメント](#コメント)
- [予約語](#予約語)

---

## 文 (Statement)

トップレベルの構文は次のいずれかです。

| 文 | 構文 |
|----|------|
| クエリ | 1 つ以上の句を順に並べたもの |
| `UNION` クエリ | `query UNION [ALL] query [UNION [ALL] query ...]` |
| 制約定義 | `CREATE CONSTRAINT ...`（[CREATE CONSTRAINT](#create-constraint) を参照）|

複数の文をセミコロンで連結することはできません。1 リクエストにつき 1 文を渡してください。

## 句 (Clause)

サポートされている句は以下のとおりです。

| 句 | 用途 |
|----|------|
| `MATCH` / `OPTIONAL MATCH` | 既存ノード・リレーションシップの検索 |
| `CREATE` | ノード・リレーションシップの作成 |
| `MERGE` | 一致するパターンが無ければ作成、あれば取得 |
| `SET` | プロパティの設定、ラベルの追加 |
| `REMOVE` | プロパティの削除、ラベルの削除 |
| `DELETE` / `DETACH DELETE` | ノード・リレーションシップの削除 |
| `WHERE` | 行のフィルタリング |
| `WITH` | パイプラインの中間射影 |
| `RETURN` | 結果の射影と並び替え |
| `UNWIND` | リストを行に展開 |
| `LOAD CSV` | CSV ファイルの読み込み（[load-csv.md](./load-csv.md) を参照）|

### MATCH / OPTIONAL MATCH

```
MATCH <pattern_list>
OPTIONAL MATCH <pattern_list>
```

- パターンに一致するノード・エッジ・パスを行として返します。
- `OPTIONAL MATCH` は一致が無い場合でも左外部結合のように行を保持し、未バインド変数を `null` とします。
- `MATCH (a)-[r]->(b), (c)` のように `,` で複数パターンをカンマ区切りで指定できます。
- `MATCH p = (a)-[*1..3]->(b)` のようにパス変数（左辺の `p =`）を割り当てられます。

```cypher
MATCH (p:Person {name: 'Alice'})-[:KNOWS]->(friend)
RETURN friend.name
```

```cypher
OPTIONAL MATCH (p:Person)-[:OWNS]->(c:Car)
RETURN p.name, c.model
```

### CREATE

```
CREATE <pattern_list>
```

- ノード・リレーションシップを作成します。
- パターン中の変数は後続句で参照できます。
- リレーションシップを作成するときは方向 (`->` または `<-`) が必須です。

```cypher
CREATE (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'})
CREATE (a)-[:KNOWS {since: 2020}]->(b)
```

`UNWIND` と組み合わせた一括作成も可能です。

```cypher
UNWIND [{name:'A'},{name:'B'}] AS row
CREATE (:Person {name: row.name})
```

### MERGE

```
MERGE <pattern_element>
  [ON CREATE SET <set_item> [, ...]]
  [ON MATCH  SET <set_item> [, ...]]
```

- パターンが一致すれば取得し、無ければ作成します。
- `ON CREATE` / `ON MATCH` でケースごとの `SET` を実行できます。
- ノード単独パターン、ならびにリレーションシップを含むチェーンパターンに対応しています。

```cypher
MERGE (p:Person {email: 'alice@example.com'})
  ON CREATE SET p.created_at = current_timestamp()
  ON MATCH  SET p.last_seen  = current_timestamp()
```

```cypher
MATCH (a:Person {name: 'A'}), (b:Person {name: 'B'})
MERGE (a)-[r:KNOWS]->(b)
  ON CREATE SET r.weight = 1
```

### SET

```
SET <set_item> [, <set_item> ...]
```

`<set_item>` は以下のいずれかです。

| 形式 | 意味 |
|------|------|
| `n.prop = expr` | プロパティを設定 |
| `n = expr` | プロパティ全体を `expr`（マップ）で置き換える |
| `n += expr` | プロパティを `expr` でマージ（既存キーは上書き、未指定キーは保持）|
| `n:Label[:Label2 ...]` | ラベルを追加 |

```cypher
MATCH (p:Person {name: 'Alice'})
SET p.age = 31, p:Customer, p += {city: 'Tokyo'}
```

### REMOVE

```
REMOVE <remove_item> [, <remove_item> ...]
```

`<remove_item>` は以下のいずれかです。

| 形式 | 意味 |
|------|------|
| `n.prop` | プロパティを削除 |
| `n:Label[:Label2 ...]` | ラベルを削除 |

```cypher
MATCH (p:Person {name: 'Alice'})
REMOVE p.tmp_flag, p:Customer
```

### DELETE / DETACH DELETE

```
[DETACH] DELETE <expression> [, <expression> ...]
```

- `DELETE n` はノード `n` を削除します。リレーションシップが残っている場合はエラーになります。
- `DETACH DELETE n` は接続するリレーションシップを先に削除してからノードを削除します。
- リレーションシップを削除するときは `DELETE r` を使います。

```cypher
MATCH (p:Person {name: 'Alice'})
DETACH DELETE p
```

### WHERE

```
WHERE <expression>
```

- 単独の句として、または `MATCH` / `OPTIONAL MATCH` / `WITH` の直後に置けます。
- `expression` は真偽値に評価されます。
- `null` を返す式は「不一致」として扱われます。

```cypher
MATCH (p:Person)
WHERE p.age >= 20 AND p.name STARTS WITH 'A'
RETURN p
```

### WITH

```
WITH [DISTINCT] <return_items>
  [ORDER BY <sort_item> [, ...]]
  [SKIP <expression>]
  [LIMIT <expression>]
  [WHERE <expression>]
```

- パイプラインの中間で射影・集約・並び替えを行い、後続句に流します。
- `WITH` の `WHERE` は射影後の式に対して評価されます。
- 集計関数（[cypher-functions.md](./cypher-functions.md#集計関数) 参照）を含めると、`WITH` でグループ化が行われます。

```cypher
MATCH (p:Person)-[:KNOWS]->(f)
WITH p, count(f) AS friends
WHERE friends > 5
RETURN p.name, friends
ORDER BY friends DESC
```

### RETURN

```
RETURN [DISTINCT] (* | <return_items>)
  [ORDER BY <sort_item> [, ...]]
  [SKIP <expression>]
  [LIMIT <expression>]
```

- `RETURN *` は現在バインドされている全変数を返します。
- 各 `<return_item>` は `expression [AS alias]` の形式です。
- `ORDER BY` の `<sort_item>` には `ASC` / `ASCENDING` / `DESC` / `DESCENDING` を付けられます（既定は昇順）。

```cypher
MATCH (p:Person)
RETURN DISTINCT p.city AS city, count(*) AS people
ORDER BY people DESC
SKIP 0 LIMIT 10
```

### UNWIND

```
UNWIND <expression> AS <variable>
```

- リストを各要素ごとの行に展開します。
- 空リストを `UNWIND` すると行は 0 行になります。
- `null` を `UNWIND` すると 1 行（値 `null`）が生成されます。

```cypher
UNWIND [1, 2, 3] AS x
RETURN x, x * x AS sq
```

```cypher
WITH ['Alice', 'Bob'] AS names
UNWIND names AS name
MERGE (:Person {name: name})
```

### LOAD CSV

```
LOAD CSV [WITH HEADERS] FROM <url> AS <alias> [FIELDTERMINATOR <char>]
```

詳細は [load-csv.md](./load-csv.md) を参照してください。

### UNION / UNION ALL

```
<query> UNION [ALL] <query> [UNION [ALL] <query> ...]
```

- 列名と列数が一致する複数のクエリ結果を縦結合します。
- `UNION` は重複を除外、`UNION ALL` は重複を保持します。

```cypher
MATCH (p:Person) RETURN p.name AS name
UNION
MATCH (c:Company) RETURN c.name AS name
```

### CREATE CONSTRAINT

```
CREATE CONSTRAINT FOR ( <variable> :<Label> )
REQUIRE <constraint_body>
```

`<constraint_body>` は以下の形式に対応しています。

| 形式 | 意味 |
|------|------|
| `<var>.<prop> IS UNIQUE` | 単一プロパティの一意性制約 |
| `<var>.<prop> IS NOT NULL` | 必須プロパティ制約 |
| `<var>.<prop> IS :: BOOLEAN \| STRING \| INTEGER \| FLOAT` | 型制約 |
| `( <var>.<p1>, <var>.<p2>, ... ) IS NODE KEY` | 複合一意性 + 必須制約 |

```cypher
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.email IS UNIQUE
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.name IS NOT NULL
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.age IS :: INTEGER
CREATE CONSTRAINT FOR (p:Person) REQUIRE (p.first_name, p.last_name) IS NODE KEY
```

> 制約は `CREATE` / `MERGE` / `SET` 実行時に検証され、違反時は実行が失敗します。

---

## パターン

```
pattern_list   = pattern ("," pattern)*
pattern        = (variable "=")? pattern_element
pattern_element = node_pattern (relationship_pattern node_pattern)*
```

### ノードパターン

```
( [<variable>] [:<Label> [:<Label> ...]] [{<prop_map>}] )
```

| 例 | 説明 |
|----|------|
| `()` | ラベル・変数を省略 |
| `(n)` | 変数のみ |
| `(:Person)` | ラベルのみ |
| `(n:Person:Customer)` | 複数ラベル（AND 条件）|
| `(n:Person {name: 'Alice', age: 30})` | インラインプロパティ述語 |

### リレーションシップパターン

```
[ <left_arrow> ] - [ [<variable>] [:<TYPE> ("|" [":"]<TYPE>)*] [<range>] [{<prop_map>}] ] - [ <right_arrow> ]
```

| 例 | 説明 |
|----|------|
| `-->` | 方向あり（タイプ任意）|
| `<--` | 逆方向 |
| `--` | 無向 |
| `-[r]->` | 変数あり |
| `-[:KNOWS]->` | タイプ指定 |
| `-[:KNOWS\|FOLLOWS]->` | 複数タイプ（OR 条件、`:` は 2 番目以降では省略可）|
| `-[:KNOWS*]->` | 可変長（1 ホップ以上、上限なし）|
| `-[:KNOWS*2]->` | ちょうど 2 ホップ |
| `-[:KNOWS*1..3]->` | 1〜3 ホップ |
| `-[:KNOWS*..3]->` | 上限のみ |
| `-[:KNOWS*2..]->` | 下限のみ |
| `-[r:KNOWS {since: 2020}]->` | プロパティ述語 |

### パス変数

```
MATCH p = (a)-[*1..3]->(b)
RETURN nodes(p), relationships(p), length(p)
```

---

## 式と演算子

### 演算子の優先順位（低い → 高い）

| カテゴリ | 演算子 |
|----------|--------|
| 論理 OR | `OR` |
| 論理 XOR | `XOR` |
| 論理 AND | `AND` |
| 論理 NOT | `NOT` |
| 比較 | `=`, `<>`, `<`, `<=`, `>`, `>=`, `=~` |
| 文字列述語 / IN | `STARTS WITH`, `ENDS WITH`, `CONTAINS`, `IN` |
| 加減算 | `+`, `-` |
| 乗除算 / 剰余 | `*`, `/`, `%` |
| べき乗 | `^` |
| 単項 | `-`, `+` |
| 後置 | `.<prop>`, `[<expr>]`, `[<start>..<end>]`, `IS [NOT] NULL` |

### 比較

```cypher
WHERE n.age >= 18 AND n.name <> 'Anonymous'
```

`null` との比較はすべて `null` を返します。`null` 判定は `IS NULL` / `IS NOT NULL` を使ってください。

### 文字列述語

```cypher
WHERE n.name STARTS WITH 'Al'
WHERE n.name ENDS WITH 'son'
WHERE n.name CONTAINS 'ic'
WHERE n.email =~ '.+@example\\.com'
```

### IN

```cypher
WHERE n.name IN ['Alice', 'Bob', 'Carol']
```

### プロパティアクセス・サブスクリプト

| 構文 | 例 | 意味 |
|------|----|------|
| `expr.<id>` | `n.name` | 静的プロパティ |
| `expr[<expr>]` | `n['na' + 'me']`, `list[0]` | 動的プロパティ / リスト要素（0 始まり）|
| `expr[<s>..<e>]` | `list[1..3]` | リスト/文字列スライス（半開区間、`s`/`e` は省略可）|

### CASE 式

一般形と単純形の両方に対応します。

```cypher
RETURN
  CASE
    WHEN n.age < 18 THEN 'child'
    WHEN n.age < 65 THEN 'adult'
    ELSE 'senior'
  END AS bucket
```

```cypher
RETURN
  CASE n.country
    WHEN 'JP' THEN 'Japan'
    WHEN 'US' THEN 'USA'
    ELSE 'Other'
  END AS country
```

### リスト内包・コレクション述語・REDUCE

```cypher
// リスト内包: [<var> IN <list> [WHERE <pred>] [| <map>]]
RETURN [x IN range(1, 10) WHERE x % 2 = 0 | x * x] AS even_squares
```

```cypher
// 真偽述語: any / all / none / single
WHERE all(x IN n.scores WHERE x >= 0)
WHERE any(x IN labels(n) WHERE x = 'Person')
```

```cypher
// REDUCE
RETURN reduce(s = 0, x IN [1, 2, 3, 4] | s + x) AS sum
```

### EXISTS サブクエリ

```cypher
MATCH (p:Person)
WHERE EXISTS { (p)-[:OWNS]->(:Car) }
RETURN p
```

`EXISTS { <pattern_element> }` は内側のパターンが現在の変数バインディングで 1 件以上一致するかを真偽値で返します。

---

## リテラル

| 種類 | 例 |
|------|----|
| 整数（10 進） | `42`, `-7` |
| 整数（16 進） | `0xFF` |
| 整数（8 進） | `0o755` |
| 浮動小数 | `3.14`, `-1.5e-3` |
| 文字列 | `'foo'`, `"bar"`（`\n`, `\t`, `\r`, `\\`, `\"`, `\'`, `\/`, `\uXXXX` をエスケープシーケンスとしてサポート）|
| 真偽値 | `true`, `false` |
| Null | `null` |
| リスト | `[1, 2, 'three']` |
| マップ | `{name: 'Alice', age: 30}`, `{'dynamic-key': 1, $param_key: 2}` |

マップキーは識別子・文字列リテラル・パラメータのいずれかを指定できます。

---

## パラメータ

`$<identifier>` でパラメータを参照できます。値は `Graph::execute_with_params()` などのバインディング API 経由で渡してください。

```cypher
MATCH (p:Person {name: $name})
WHERE p.age > $min_age
RETURN p
```

---

## 型

実行時値は内部で次の型として扱われます。

| 型 | 説明 |
|----|------|
| `Null` | 未定義値 |
| `Boolean` | `true` / `false` |
| `Integer` | 64 bit 符号付き整数 |
| `Float` | 64 bit IEEE 754 |
| `String` | UTF-8 文字列 |
| `List` | `CypherValue` の配列 |
| `Map` | 文字列キー → `CypherValue` |
| `Node` | グラフ上のノード |
| `Relationship` | グラフ上のリレーションシップ |
| `Path` | ノードとリレーションシップの交互列 |
| `Date` | 日付（`NaiveDate`）|
| `Timestamp` | UTC タイムスタンプ |

ノード・リレーションシップのプロパティ値は `String` / `Integer` / `Float` / `Boolean` のスカラのみ保存可能です。

---

## コメント

| 構文 | 意味 |
|------|------|
| `// ...` | 行コメント（行末まで）|
| `/* ... */` | ブロックコメント（ネスト不可）|

---

## 予約語

以下の語は識別子として使用できません（大文字小文字は区別しません）。

```
MATCH OPTIONAL CREATE RETURN WHERE
ORDER BY SKIP LIMIT AND OR NOT XOR
NULL TRUE FALSE AS ASC ASCENDING DESC DESCENDING
DISTINCT IS IN STARTS ENDS CONTAINS
CASE WHEN THEN ELSE END
WITH UNWIND SET REMOVE DELETE DETACH MERGE ON
REDUCE ANY ALL NONE SINGLE
UNION CONSTRAINT REQUIRE UNIQUE
```

加えて、識別子は二重アンダースコア（`__`）で開始することはできません（プランナ内部用に予約）。
