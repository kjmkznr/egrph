# Cypher 組み込み関数

Egrph がサポートする組み込み関数の一覧です。関数名は **大文字小文字を区別しません**（内部で小文字に正規化されます）。クエリ全体の構文は [cypher.md](./cypher.md) を参照してください。

`null` を引数に渡した関数は、特記が無い限り `null` を返します。

## 目次

- [集計関数](#集計関数)
- [スカラ関数](#スカラ関数)
- [型変換関数](#型変換関数)
- [文字列関数](#文字列関数)
- [正規表現関数](#正規表現関数)
- [数学関数](#数学関数)
- [リスト関数](#リスト関数)
- [マップ・プロパティ関数](#マッププロパティ関数)
- [グラフ関数](#グラフ関数)
- [日付・時刻関数](#日付時刻関数)
- [メタ関数](#メタ関数)

---

## 集計関数

集計関数は `RETURN` または `WITH` で使用し、グルーピングを引き起こします。`DISTINCT` で重複除去できます。

| 関数 | 説明 |
|------|------|
| `count(*)` | 行数を数える |
| `count(expr)` | `null` でない行数を数える |
| `count(DISTINCT expr)` | 重複除去後の数 |
| `sum(expr)` | 数値の合計 |
| `avg(expr)` | 数値の平均 |
| `min(expr)` | 最小値 |
| `max(expr)` | 最大値 |
| `collect(expr)` | 値をリストに集約 |
| `collect(DISTINCT expr)` | 重複除去後リスト |
| `stdev(expr)` | 標本標準偏差 |
| `stdevp(expr)` | 母標準偏差 |
| `percentileCont(expr, p)` | 連続パーセンタイル（補間あり）。`p` は `0..1` |
| `percentileDisc(expr, p)` | 離散パーセンタイル。`p` は `0..1` |

```cypher
MATCH (p:Person)
RETURN p.country AS country, count(*) AS people, avg(p.age) AS avg_age
```

> 集計関数を行レベル（非集計コンテキスト）で呼び出した場合は `null` を返します。

---

## スカラ関数

| 関数 | 引数 | 戻り値 | 説明 |
|------|------|--------|------|
| `id(node \| rel)` | ノード/リレーションシップ | Integer | 内部 ID |
| `type(rel)` | リレーションシップ | String | リレーションシップ・タイプ |
| `labels(node)` | ノード | List<String> | ノードのラベル一覧 |
| `coalesce(v1, v2, ...)` | 任意 | 任意 | 最初の非 `null` 値 |
| `exists(expr)` | 任意 | Boolean | 値が `null` でなければ `true` |
| `size(s \| list)` | String / List | Integer | 文字数またはリスト長 |
| `length(path)` | Path | Integer | パスのホップ数 |

`size()` と `length()` の用途は openCypher 仕様に従います。

```cypher
MATCH (n)
RETURN id(n), labels(n), coalesce(n.nick, n.name) AS display
```

---

## 型変換関数

| 関数 | 別名 | 説明 |
|------|------|------|
| `toInteger(x)` | `toInt`, `int64`, `cast_to_int64` | 整数に変換。Boolean には未定義（`null`）|
| `toFloat(x)` | `double`, `cast_to_double` | 浮動小数に変換 |
| `toString(x)` | `string`, `cast_to_string` | スカラを文字列に変換 |
| `toBoolean(x)` | — | `'true'` / `'false'` 文字列、Boolean のみ受理 |
| `date(x)` | `cast_to_date` | `Date` 値に変換（`'YYYY-MM-DD'` 形式の文字列をパース）|

```cypher
RETURN toInteger('42') + toFloat('3.14')
RETURN toBoolean('TRUE'), toString(123)
RETURN date('2026-04-25')
```

---

## 文字列関数

| 関数 | 説明 |
|------|------|
| `trim(s)` | 前後の空白を除去 |
| `ltrim(s)` | 先頭の空白を除去 |
| `rtrim(s)` | 末尾の空白を除去 |
| `toUpper(s)` | 大文字化 |
| `toLower(s)` | 小文字化 |
| `reverse(s \| list)` | 文字列・リストの反転 |
| `replace(s, search, replacement)` | 部分文字列の置換（全件）|
| `substring(s, start [, length])` | 部分文字列（0 始まり、文字単位）|
| `left(s, n)` | 先頭 `n` 文字 |
| `right(s, n)` | 末尾 `n` 文字 |
| `split(s, sep)` | セパレータで分割し List<String> |
| `concat(s1, s2, ...)` | 文字列連結。引数のいずれかが `null` の場合は `null` |

```cypher
RETURN trim('  hello  '), toUpper('abc'), substring('hello', 1, 3)
RETURN split('a,b,c', ','), concat('Hello, ', 'World')
```

---

## 正規表現関数

正規表現は Rust の `regex` クレートの構文（PCRE 風、後方参照は非対応）で評価されます。

| 関数 | 説明 |
|------|------|
| `regexp_matches(s, pattern)` | パターン一致なら `true` |
| `regexp_replace(s, pattern, replacement)` | 全マッチを置換 |
| `regexp_extract(s, pattern [, group])` | キャプチャ抽出。`group` 既定は 0（全体）|

`=~` 演算子も正規表現マッチに対応しています。

```cypher
WHERE n.email =~ '.+@example\\.com'
RETURN regexp_extract('order-1234', '(\\d+)', 1) AS id
```

---

## 数学関数

### 基本

| 関数 | 説明 |
|------|------|
| `abs(x)` | 絶対値 |
| `ceil(x)` | 切り上げ |
| `floor(x)` | 切り捨て |
| `round(x)` | 四捨五入 |
| `sign(x)` | 符号（-1 / 0 / 1）|
| `sqrt(x)` | 平方根 |
| `log(x)` | 自然対数 |
| `log10(x)` | 常用対数 |
| `exp(x)` | 指数関数 e<sup>x</sup> |

### 三角関数

`sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2(y, x)`, `cot`, `haversin`, `degrees(x)`, `radians(x)`

### 定数・乱数

| 関数 | 説明 |
|------|------|
| `pi()` | 円周率 π |
| `e()` | 自然対数の底 e |
| `rand()` | `[0.0, 1.0)` の擬似乱数 |
| `randomUUID()` | バージョン 4 UUID 文字列 |

```cypher
RETURN pi(), sin(radians(30)), abs(-5), randomUUID()
```

---

## リスト関数

| 関数 | 別名 | 説明 |
|------|------|------|
| `head(list)` | — | 先頭要素 |
| `last(list)` | — | 末尾要素 |
| `tail(list)` | — | 先頭以外（リスト）|
| `range(start, end [, step])` | — | 等差リスト（両端を含む）。最大 1,000,000 要素 |
| `reverse(list)` | — | 逆順リスト |
| `list_append(list, x)` | `array_append`, `list_push_back` | 末尾追加 |
| `list_prepend(list, x)` | `array_prepend`, `list_push_front` | 先頭追加 |
| `list_extract(list, i)` | `array_extract` | 1 始まりインデックスで要素取得（負値で末尾から）|
| `list_contains(list, x)` | `array_contains`, `list_has` | 含有判定 |
| `list_sort(list)` | `array_sort` | 昇順ソート |
| `list_distinct(list)` | `array_distinct` | 重複除去 |
| `list_value(...)` | `make_list`, `list_creation` | 引数からリストを構築 |

```cypher
RETURN range(1, 5)                       // [1, 2, 3, 4, 5]
RETURN range(0, 10, 2)                   // [0, 2, 4, 6, 8, 10]
RETURN list_distinct([1, 2, 2, 3])      // [1, 2, 3]
RETURN list_extract([10, 20, 30], -1)   // 30
```

> `range()` は要素数が 1,000,000 を超える場合、それ以上の生成を打ち切ります。

---

## マップ・プロパティ関数

| 関数 | 引数 | 説明 |
|------|------|------|
| `keys(node \| rel \| map)` | ノード/リレ/マップ | キー一覧（List<String>）|
| `properties(node \| rel \| map)` | 同上 | プロパティをマップで取得 |

```cypher
MATCH (p:Person {name: 'Alice'})
RETURN keys(p), properties(p)
```

---

## グラフ関数

| 関数 | 引数 | 戻り値 | 説明 |
|------|------|--------|------|
| `startNode(rel)` | リレーションシップ | Node | 始点ノード |
| `endNode(rel)` | リレーションシップ | Node | 終点ノード |
| `nodes(path)` | Path | List<Node> | パスを構成するノード列 |
| `relationships(path)` | Path | List<Relationship> | パスを構成するリレ列 |
| `rels(path)` | Path | List<Relationship> | `relationships` の別名 |
| `length(path)` | Path | Integer | ホップ数 |

```cypher
MATCH p = (a:Person {name: 'Alice'})-[:KNOWS*1..3]->(b)
RETURN nodes(p), rels(p), length(p)
```

---

## 日付・時刻関数

| 関数 | 説明 |
|------|------|
| `current_date()` | 現在の UTC 日付（`Date`）|
| `current_timestamp()` | 現在の UTC タイムスタンプ |
| `make_date(year, month, day)` | `Date` を構築 |
| `date(s)` | `'YYYY-MM-DD'` 形式の文字列を `Date` に変換 |
| `to_timestamp(secs)` | エポック秒（Integer / Float）から `Timestamp` |
| `epoch_ms(ts)` | `Timestamp` をエポックミリ秒（Integer）に |
| `date_part(part, dt)` | 日付/タイムスタンプの一部を取り出す |
| `date_trunc(part, dt)` | 指定単位で切り捨て |

`date_part` の `part`（小文字に正規化）と対応値：

| `part` | `Date` | `Timestamp` |
|--------|--------|-------------|
| `'year'` | ✅ | ✅ |
| `'month'` | ✅ | ✅ |
| `'day'` | ✅ | ✅ |
| `'hour'` | — | ✅ |
| `'minute'` | — | ✅ |
| `'second'` | — | ✅ |
| `'epoch'` | — | ✅（秒）|

`date_trunc` は `Timestamp` で `'year'`, `'month'`, `'day'`, `'hour'`, `'minute'`、`Date` で `'year'`, `'month'`, `'day'` をサポートします。

```cypher
RETURN current_timestamp(), current_date()
RETURN make_date(2026, 4, 25)
RETURN date_part('year', current_date())
RETURN date_trunc('month', current_timestamp())
```

---

## メタ関数

| 関数 | 説明 |
|------|------|
| `show_functions()` | 利用可能な関数名のリストを返す（List<String>）|

```cypher
RETURN show_functions()
```

> 内部関数 `__has_label(node, label)` はプランナが生成するもので、ユーザーが直接呼び出すことは想定されていません（識別子の `__` 接頭辞は予約済みです）。
