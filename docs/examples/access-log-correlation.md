# アクセスログ相関分析

複数システムのアクセスログを Egrph に取り込み、`RequestID` を介してログ間の因果関係をグラフとして表現するサンプルです。本ドキュメントでは次の 3 つのユースケースを扱います。

- 特定の `RequestID` に関連する全レイヤのログを取得する
- 末端ログ（DB クエリなど）から大元のクライアント IP を特定する
- グラフ構造を活かした不正検知パターン

クエリ構文の詳細は [cypher.md](../cypher.md)、関数は [cypher-functions.md](../cypher-functions.md)、CSV 読み込みは [load-csv.md](../load-csv.md) を参照してください。

---

## 想定するシステム構成

リクエストは次の経路で流れる前提とします。

```
Client ──► Frontend ──► Gateway ──► Backend ──► Database
              (LB)       (API GW)    (App)       (DB)
```

**1 リクエストにつき 1 つの `request_id` をシステム全体で共有します。** 各レイヤは受け取った `request_id` をそのまま自身のログに記録します。

| レイヤ | 記録する ID |
|-------|-------------|
| Frontend | `request_id`（クライアント発信） |
| Gateway | 同じ `request_id` |
| Backend | 同じ `request_id` |
| Database | 同じ `request_id` |

---

## グラフモデル

ノード・リレーションシップを次のように設計します。

| 要素 | ラベル / タイプ | 主なプロパティ |
|------|-----------------|----------------|
| ログエントリ | `:LogEntry` | `request_id`, `layer`, `timestamp`, `status`, ... |
| クライアント | `:Client` | `ip` |
| クライアント → 起点ログ | `[:INITIATED]` | — |
| 親ログ → 子ログ | `[:FORWARDED_TO]` | — |

**設計上のポイント**

- 同一リクエストの全レイヤが同じ `request_id` を共有するため、`:LogEntry` ノードは `{request_id, layer}` の組み合わせで識別します。
- 全ログを単一の `:LogEntry` ラベルで統一し、レイヤは `layer` プロパティ（`'frontend' | 'gateway' | 'backend' | 'database'`）で区別します。複数ラベル時の `labels(n)` の順序は保証されないため、判定はプロパティで行うのが安全です。
- プロパティ値はスカラ（`String` / `Integer` / `Float` / `Boolean`）のみ保存可能なため、タイムスタンプは ISO 8601 文字列として保存します。ISO 8601 文字列は辞書順比較で時系列順に並ぶため、`ORDER BY timestamp` がそのまま使えます。
- ログの取り込み順序に依存しないよう、すべて `MERGE` で書きます。

---

## サンプル CSV

以下、`/data/` 配下に各レイヤのログを置く想定です。

サンプルには 2 種類の IP が登場します。`203.0.113.10` は正常なユーザ、`198.51.100.7` は偵察・ブルートフォースを行う攻撃者を模しています。不正検知クエリ（クエリ 3）はこのデータで動作確認できます。

### `frontend_logs.csv`

```csv
timestamp,request_id,client_ip,method,path,status,user_agent
2026-04-25T10:00:01Z,req-001,203.0.113.10,GET,/api/users/42,200,Mozilla/5.0
2026-04-25T10:00:02Z,req-002,203.0.113.10,POST,/api/login,401,Mozilla/5.0
2026-04-25T10:00:03Z,req-003,198.51.100.7,GET,/api/orders,500,curl/8.4
2026-04-25T10:00:04Z,req-004,198.51.100.7,GET,/api/users/99,404,curl/8.4
2026-04-25T10:00:05Z,req-005,198.51.100.7,POST,/api/login,401,curl/8.4
2026-04-25T10:00:06Z,req-006,198.51.100.7,POST,/api/login,401,curl/8.4
2026-04-25T10:00:07Z,req-007,198.51.100.7,POST,/api/login,401,curl/8.4
2026-04-25T10:00:08Z,req-008,198.51.100.7,GET,/api/users/1,404,curl/8.4
2026-04-25T10:00:09Z,req-009,198.51.100.7,GET,/api/users/2,404,curl/8.4
2026-04-25T10:00:10Z,req-010,198.51.100.7,GET,/api/users/3,404,curl/8.4
2026-04-25T10:00:11Z,req-011,198.51.100.7,GET,/api/admin,403,curl/8.4
2026-04-25T10:00:12Z,req-012,198.51.100.7,POST,/api/orders,500,curl/8.4
2026-04-25T10:00:13Z,req-013,203.0.113.10,GET,/api/orders/1,200,Mozilla/5.0
```

### `gateway_logs.csv`

```csv
timestamp,request_id,route,status
2026-04-25T10:00:01Z,req-001,users-service,200
2026-04-25T10:00:02Z,req-002,auth-service,401
2026-04-25T10:00:03Z,req-003,orders-service,500
2026-04-25T10:00:04Z,req-004,users-service,404
2026-04-25T10:00:05Z,req-005,auth-service,401
2026-04-25T10:00:06Z,req-006,auth-service,401
2026-04-25T10:00:07Z,req-007,auth-service,401
2026-04-25T10:00:08Z,req-008,users-service,404
2026-04-25T10:00:09Z,req-009,users-service,404
2026-04-25T10:00:10Z,req-010,users-service,404
2026-04-25T10:00:11Z,req-011,admin-service,403
2026-04-25T10:00:12Z,req-012,orders-service,500
2026-04-25T10:00:13Z,req-013,orders-service,200
```

### `backend_logs.csv`

```csv
timestamp,request_id,service,action,user_id,status
2026-04-25T10:00:01Z,req-001,users,read,42,200
2026-04-25T10:00:02Z,req-002,auth,login_attempt,42,401
2026-04-25T10:00:03Z,req-003,orders,list,17,500
2026-04-25T10:00:05Z,req-005,auth,login_attempt,10,401
2026-04-25T10:00:06Z,req-006,auth,login_attempt,11,401
2026-04-25T10:00:07Z,req-007,auth,login_attempt,12,401
2026-04-25T10:00:12Z,req-012,orders,create,99,500
2026-04-25T10:00:13Z,req-013,orders,get,42,200
```

> req-004, req-008〜req-011 はゲートウェイ層でエラーを返しバックエンドに到達しないため、`backend_logs.csv` には含まれません。

### `database_logs.csv`

```csv
timestamp,request_id,query_type,table,rows
2026-04-25T10:00:01Z,req-001,SELECT,users,1
2026-04-25T10:00:03Z,req-003,SELECT,orders,0
2026-04-25T10:00:12Z,req-012,INSERT,orders,0
2026-04-25T10:00:13Z,req-013,SELECT,orders,3
```

---

## ログの取り込み

各レイヤを順に `LOAD CSV` で読み込みます。`MERGE` で書いているため、CSV の読み込み順は問いません。

### Frontend ログ

クライアント発のリクエストなので、`Client` ノードと `INITIATED` リレーションシップを同時に作成します。

```cypher
LOAD CSV WITH HEADERS FROM '/data/frontend_logs.csv' AS row
MERGE (c:Client {ip: row.client_ip})
MERGE (log:LogEntry {request_id: row.request_id, layer: 'frontend'})
SET log.timestamp  = row.timestamp,
    log.method     = row.method,
    log.path       = row.path,
    log.status     = toInteger(row.status),
    log.user_agent = row.user_agent
MERGE (c)-[:INITIATED]->(log)
```

### Gateway ログ

同じ `request_id` を持つ Frontend ログを `MERGE` でスタブとして取得し、`FORWARDED_TO` で連結します。

```cypher
LOAD CSV WITH HEADERS FROM '/data/gateway_logs.csv' AS row
MERGE (prev:LogEntry {request_id: row.request_id, layer: 'frontend'})
MERGE (log:LogEntry {request_id: row.request_id, layer: 'gateway'})
SET log.timestamp = row.timestamp,
    log.route     = row.route,
    log.status    = toInteger(row.status)
MERGE (prev)-[:FORWARDED_TO]->(log)
```

### Backend ログ

```cypher
LOAD CSV WITH HEADERS FROM '/data/backend_logs.csv' AS row
MERGE (prev:LogEntry {request_id: row.request_id, layer: 'gateway'})
MERGE (log:LogEntry {request_id: row.request_id, layer: 'backend'})
SET log.timestamp = row.timestamp,
    log.service   = row.service,
    log.action    = row.action,
    log.user_id   = toInteger(row.user_id),
    log.status    = toInteger(row.status)
MERGE (prev)-[:FORWARDED_TO]->(log)
```

### Database ログ

```cypher
LOAD CSV WITH HEADERS FROM '/data/database_logs.csv' AS row
MERGE (prev:LogEntry {request_id: row.request_id, layer: 'backend'})
MERGE (log:LogEntry {request_id: row.request_id, layer: 'database'})
SET log.timestamp  = row.timestamp,
    log.query_type = row.query_type,
    log.target     = row.table,
    log.rows       = toInteger(row.rows)
MERGE (prev)-[:FORWARDED_TO]->(log)
```

> CSV ヘッダ名 `table` はクエリ内では `target` プロパティに格納しています（CSV 列名が必ずしも分かりやすいプロパティ名とは限らないため、ロード時に整理する例）。

---

## クエリ 1: 特定の `RequestID` に関連する全ログを取得する

`request_id` はシステム間で共有されているため、単純にプロパティ検索するだけで全レイヤのログが得られます。

```cypher
MATCH (log:LogEntry {request_id: 'req-001'})
RETURN log.layer     AS layer,
       log.timestamp AS timestamp,
       log.status    AS status
ORDER BY timestamp
```

**結果（例）**

| layer    | timestamp             | status |
|----------|-----------------------|--------|
| frontend | 2026-04-25T10:00:01Z  | 200    |
| gateway  | 2026-04-25T10:00:01Z  | 200    |
| backend  | 2026-04-25T10:00:01Z  | 200    |
| database | 2026-04-25T10:00:01Z  | null   |

database 層のログには `status` プロパティを持たせていないため `null` になります。レイヤごとに存在するプロパティが異なる場合は `CASE` や `coalesce` で補完してください。

### チェーンを辿って取得する変種

`FORWARDED_TO` チェーンを明示的に辿ることで、フロントエンドから末端までの処理順を保証しながら取得できます。`layer` 順に並べることと等価ですが、処理経路の可視化に使えます。

```cypher
MATCH (start:LogEntry {request_id: 'req-001', layer: 'frontend'})
OPTIONAL MATCH (start)-[:FORWARDED_TO*1..]->(descendant:LogEntry)
WITH start, collect(DISTINCT descendant) AS descendants
UNWIND ([start] + descendants) AS log
RETURN DISTINCT
       log.layer     AS layer,
       log.timestamp AS timestamp,
       log.status    AS status
ORDER BY timestamp
```

- `OPTIONAL MATCH` は一致が無い場合 `descendant` を `null` にしますが、`collect` は `null` を除外するため、子孫が居ないケースでも空リストになります。

---

## クエリ 2: 末端ログから大元のクライアント IP を特定する

`request_id` がシステム間で共有されているため、データベース層のログからでもクライアント IP へ直接たどれます。

```cypher
MATCH (c:Client)-[:INITIATED]->(:LogEntry {request_id: 'req-003'})
RETURN c.ip AS client_ip
```

**結果（例）**

| client_ip      |
|----------------|
| 198.51.100.7   |

### 処理経路も合わせて取得する変種

チェーンを辿ることで、フロントエンドから末端までの `layer` 順の処理経路も同時に返せます。

```cypher
MATCH (c:Client)-[:INITIATED]->(front:LogEntry {request_id: 'req-003', layer: 'frontend'})
      -[r:FORWARDED_TO*]->(leaf:LogEntry {layer: 'database'})
RETURN c.ip                                                       AS client_ip,
       size(r) + 1                                                AS hops,
       [front.layer] + [rel IN r | endNode(rel).layer]            AS chain
```

**結果（例）**

| client_ip      | hops | chain                                                   |
|----------------|------|---------------------------------------------------------|
| 198.51.100.7   | 4    | ["frontend","gateway","backend","database"]             |

**ポイント**

- 可変長リレーションシップ `[r:FORWARDED_TO*]` は `r` をリレーションシップのリストとして束縛します。`size(r)` で `FORWARDED_TO` のホップ数が得られ、`+1` で `INITIATED` を含めた総ホップ数になります。
- `[rel IN r | endNode(rel).layer]` で経路上の各レイヤを順に取り出し、先頭の `front.layer` と結合して全 chain を構築します。
- 末端 `request_id` のリクエストが `FORWARDED_TO` チェーンを持たない（フロントエンド層のみ）場合はマッチしません。

---

## クエリ 3: 不正検知パターン

グラフ構造を活かした典型的な検知ルールをいくつか示します。サンプル CSV のしきい値は動作確認しやすい小さな値にしています。実環境では IP あたりの規模に合わせて調整してください。

### 3.1 同一 IP からの大量リクエスト（バースト）

```cypher
MATCH (c:Client)-[:INITIATED]->(f:LogEntry {layer: 'frontend'})
WHERE f.timestamp >= '2026-04-25T10:00:00Z'
  AND f.timestamp <  '2026-04-25T10:01:00Z'
WITH c, count(*) AS req_count
WHERE req_count > 5
RETURN c.ip AS ip, req_count
ORDER BY req_count DESC
```

ISO 8601 文字列の辞書順比較は時系列順と一致するため、`>=` / `<` をそのまま範囲条件に使えます。

**結果（例）**

| ip           | req_count |
|--------------|-----------|
| 198.51.100.7 | 10        |

### 3.2 ログイン失敗の連続試行

`/api/login` への 4xx 応答が一定回数を超える IP を検出します。

```cypher
MATCH (c:Client)-[:INITIATED]->(f:LogEntry {layer: 'frontend'})
WHERE f.path = '/api/login' AND f.status >= 400 AND f.status < 500
WITH c, count(*) AS failures
WHERE failures > 2
RETURN c.ip AS ip, failures
ORDER BY failures DESC
```

**結果（例）**

| ip           | failures |
|--------------|----------|
| 198.51.100.7 | 3        |

### 3.3 短時間に多数のアカウントへ試行（クレデンシャルスタッフィング）

バックエンドの `login_attempt` アクションを辿り、同一 IP が触れた `user_id` の異なる数を数えます。

```cypher
MATCH (c:Client)-[:INITIATED]->(:LogEntry)-[:FORWARDED_TO*1..]->(b:LogEntry {layer: 'backend'})
WHERE b.action = 'login_attempt'
WITH c, count(DISTINCT b.user_id) AS user_count, count(*) AS attempts
WHERE user_count >= 3
RETURN c.ip AS ip, user_count, attempts
ORDER BY user_count DESC
```

**結果（例）**

| ip           | user_count | attempts |
|--------------|------------|----------|
| 198.51.100.7 | 3          | 3        |

> 識別子は予約語（`UNIQUE` など）と同名で始まり直後にアンダースコアが続く形（例: `unique_users`）にすると、egrph の字句解析が予約語として扱おうとして失敗します。`user_count` のように予約語を含まない名前を使ってください。

### 3.4 異常に高いエラー率の IP

4xx / 5xx（クライアントエラー・サーバエラー）の割合が 50% を超える IP を抽出します。偵察目的のアクセスは正常応答がほとんど無いため、この指標で早期検出できます。`CASE` 式で集計の分岐を表現します。

```cypher
MATCH (c:Client)-[:INITIATED]->(f:LogEntry {layer: 'frontend'})
WITH c,
     count(*) AS total,
     sum(CASE WHEN f.status >= 400 THEN 1 ELSE 0 END) AS errors
WHERE total > 5 AND toFloat(errors) / total > 0.5
RETURN c.ip AS ip, total, errors, toFloat(errors) / total AS error_ratio
ORDER BY error_ratio DESC
```

**結果（例）**

| ip           | total | errors | error_ratio |
|--------------|-------|--------|-------------|
| 198.51.100.7 | 10    | 10     | 1.0         |

### 3.5 探索的アクセス（404 が連続する IP）

存在しないリソースへのアクセスが多い IP は、エンドポイント列挙攻撃の可能性があります。

```cypher
MATCH (c:Client)-[:INITIATED]->(f:LogEntry {layer: 'frontend'})
WHERE f.status = 404
WITH c, collect(DISTINCT f.path) AS scanned_paths, count(*) AS hits
WHERE hits >= 3
RETURN c.ip AS ip, hits, size(scanned_paths) AS path_count, scanned_paths
ORDER BY hits DESC
```

**結果（例）**

| ip           | hits | path_count | scanned_paths                                               |
|--------------|------|------------|-------------------------------------------------------------|
| 198.51.100.7 | 4    | 4          | ["/api/users/99","/api/users/1","/api/users/2","/api/users/3"] |

### 3.6 末端で重大エラーが出ている経路の追跡

DB レイヤで異常（ここでは取得・挿入行数 0 で例示）になった呼び出しの上流クライアントを特定します。`request_id` が共有されているため、ジョインなしで直接たどれます。

```cypher
MATCH (c:Client)-[:INITIATED]->(front:LogEntry)-[:FORWARDED_TO*]->(d:LogEntry {layer: 'database'})
WHERE d.rows = 0
RETURN c.ip             AS client_ip,
       front.request_id AS request_id,
       d.query_type     AS query_type,
       d.target         AS target,
       d.timestamp      AS at
ORDER BY at
```

**結果（例）**

| client_ip    | request_id | query_type | target | at                    |
|--------------|------------|------------|--------|-----------------------|
| 198.51.100.7 | req-003    | SELECT     | orders | 2026-04-25T10:00:03Z  |
| 198.51.100.7 | req-012    | INSERT     | orders | 2026-04-25T10:00:12Z  |

