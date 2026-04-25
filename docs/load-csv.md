# LOAD CSV

`LOAD CSV` は CSV ファイルを読み込み、各行をグラフクエリ内で利用可能な変数として展開する Cypher 句です。

## 構文

```
LOAD CSV [WITH HEADERS] FROM <url> AS <alias> [FIELDTERMINATOR <char>]
```

| 要素 | 必須 | 説明 |
|------|------|------|
| `WITH HEADERS` | 任意 | 指定すると先頭行をヘッダとして扱い、各行をマップとして返す |
| `FROM <url>` | 必須 | 読み込む CSV ファイルのパスまたは `file://` URL |
| `AS <alias>` | 必須 | 各行をバインドする変数名 |
| `FIELDTERMINATOR <char>` | 任意 | フィールド区切り文字（デフォルト: `,`）。ASCII 1 文字のみ指定可能 |

## 行の型

### WITH HEADERS なし

各行は **リスト**（`List<String>`）として返されます。フィールドには 0 始まりのインデックスでアクセスします。

```cypher
LOAD CSV FROM '/path/to/file.csv' AS row
RETURN row[0], row[1]
```

### WITH HEADERS あり

各行は **マップ**（`Map<String, String>`）として返されます。ヘッダ名をキーとしてフィールドにアクセスできます。

```cypher
LOAD CSV WITH HEADERS FROM '/path/to/file.csv' AS row
RETURN row.name, row.age
```

## URL の指定方法

ファイルパスは以下の形式を受け付けます。

- 絶対パス: `/data/nodes.csv`
- `file://` スキーム: `file:///data/nodes.csv`

## 空フィールドの扱い

空フィールド（`,,` や末尾の `,`）は `null` として返されます。

```csv
name,age
Alice,
Bob,25
```

```cypher
LOAD CSV WITH HEADERS FROM '/path/to/file.csv' AS row
RETURN row.name, row.age
// → ["Alice", null], ["Bob", "25"]
```

> **注意**: すべてのフィールド値は文字列として返されます。数値として扱う場合は `toInteger()` や `toFloat()` で変換してください。

## 使用例

### ノードの一括作成

```csv
name,age
Alice,30
Bob,25
Carol,35
```

```cypher
LOAD CSV WITH HEADERS FROM '/data/persons.csv' AS row
CREATE (:Person {name: row.name, age: toInteger(row.age)})
```

### エッジを含むグラフの構築

```csv
src,dst,weight
Alice,Bob,1.5
Bob,Carol,2.0
```

```cypher
LOAD CSV WITH HEADERS FROM '/data/edges.csv' AS row
MATCH (a:Person {name: row.src}), (b:Person {name: row.dst})
CREATE (a)-[:KNOWS {weight: toFloat(row.weight)}]->(b)
```

### カスタム区切り文字（セミコロン区切り）

```csv
name;age
Alice;30
Bob;25
```

```cypher
LOAD CSV WITH HEADERS FROM '/data/persons.csv' AS row FIELDTERMINATOR ';'
RETURN row.name, row.age
```

### ヘッダなし CSV のインデックスアクセス

```csv
Alice,30
Bob,25
```

```cypher
LOAD CSV FROM '/data/persons.csv' AS row
RETURN row[0] AS name, row[1] AS age
```

### WHERE による行のフィルタリング

```cypher
LOAD CSV WITH HEADERS FROM '/data/persons.csv' AS row
WHERE row.age IS NOT NULL
CREATE (:Person {name: row.name, age: toInteger(row.age)})
```

## 他の句との組み合わせ

`LOAD CSV` は他の Cypher 句と組み合わせて使用できます。

| 組み合わせ | 用途 |
|-----------|------|
| `LOAD CSV ... CREATE` | CSV 行からノード・エッジを作成する |
| `LOAD CSV ... MATCH` | 既存ノードを CSV データで検索・照合する |
| `LOAD CSV ... WITH` | 中間変数を整形して後続句へ渡す |
| `LOAD CSV ... RETURN` | CSV データをそのまま返す |

## エラー

| エラー | 原因 |
|--------|------|
| `cannot open '...'` | ファイルが存在しないかアクセス権がない |
| `URL must be a string` | URL 式が文字列以外に評価された |
| `FIELDTERMINATOR must not be empty` | `FIELDTERMINATOR` に空文字列を指定した |
| `FIELDTERMINATOR must be an ASCII character` | `FIELDTERMINATOR` に非 ASCII 文字を指定した |
