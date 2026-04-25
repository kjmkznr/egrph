算術関数

    基本演算: +, -, *, /, %, ^
    数学関数: ABS, ACOS, ASIN, ATAN, ATAN2, COS, SIN, TAN
    その他: CEIL, FLOOR, ROUND, SQRT, LOG, LOG10, EXP, PI, E, RAND, SIGN, DEGREES, RADIANS, COT, HAVERSIN

文字列関数

    操作: CONCAT, LOWER, UPPER, TRIM, LTRIM, RTRIM, SUBSTR(SUBSTRING), LEFT, RIGHT, SPLIT, REPLACE, REVERSE
    検索: CONTAINS, STARTS_WITH, ENDS_WITH
    正規表現: REGEXP_MATCHES, REGEXP_REPLACE, REGEXP_EXTRACT

リスト関数

    作成: LIST_VALUE (make_list, list_creation), RANGE
    操作: LIST_APPEND (array_append), LIST_PREPEND (array_prepend), LIST_EXTRACT (array_extract, 1-based), LIST_CONTAINS (array_contains, list_has)
    変換: LIST_SORT (array_sort), LIST_DISTINCT (array_distinct), REVERSE
    その他: HEAD, LAST, TAIL, SIZE

キャスト関数

    型変換: TOSTRING (STRING, CAST_TO_STRING), TOINTEGER (TOINT, INT64, CAST_TO_INT64), TOFLOAT (DOUBLE, CAST_TO_DOUBLE), TOBOOLEAN
    日付変換: DATE (CAST_TO_DATE) — ISO 8601 文字列から Date 型へ

日付・時刻関数

    日付: CURRENT_DATE, MAKE_DATE(year, month, day), DATE(str)
    タイムスタンプ: CURRENT_TIMESTAMP, TO_TIMESTAMP(epoch_seconds), EPOCH_MS(timestamp)
    抽出: DATE_PART(part, date_or_ts) — part は 'year'/'month'/'day'/'hour'/'minute'/'second'/'epoch'
    切り捨て: DATE_TRUNC(part, date_or_ts) — part は 'year'/'month'/'day'/'hour'/'minute'

集約関数

    COUNT, COUNT(*), SUM, AVG, MIN, MAX, COLLECT
    統計: STDEV, STDEVP, PERCENTILECONT, PERCENTILEDISC

グラフ特化関数

    ノード/リレーション: ID, TYPE, LABELS, START_NODE (startnode), END_NODE (endnode), PROPERTIES, KEYS
    パス: NODES, RELS (RELATIONSHIPS), LENGTH

その他

    COALESCE, EXISTS, RANDOMUUID, SHOW_FUNCTIONS, SIZE
