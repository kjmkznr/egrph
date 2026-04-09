use super::expression::{
    Parameters, Record, compare_values, cypher_value_to_stable_key, eval_with_params, to_f64,
};
use crate::ast::{Expression, ReturnItem};
use crate::error::CypherError;
use crate::graph::storage::GraphStorage;
use crate::graph::types::CypherValue;
use std::collections::HashMap;

/// Returns true if any return item contains an aggregate function call.
pub fn items_contain_aggregation(items: &[ReturnItem]) -> bool {
    items
        .iter()
        .any(|item| expr_contains_aggregation(&item.expression))
}

fn expr_contains_aggregation(expr: &Expression) -> bool {
    match expr {
        Expression::FunctionCall { name, .. } => {
            matches!(
                name.to_lowercase().as_str(),
                "count"
                    | "sum"
                    | "avg"
                    | "min"
                    | "max"
                    | "collect"
                    | "percentilecont"
                    | "percentiledisc"
                    | "stdev"
                    | "stdevp"
            )
        }
        Expression::BinaryOp { left, right, .. } => {
            expr_contains_aggregation(left) || expr_contains_aggregation(right)
        }
        Expression::UnaryOp { operand, .. } => expr_contains_aggregation(operand),
        Expression::And(l, r) | Expression::Or(l, r) | Expression::Xor(l, r) => {
            expr_contains_aggregation(l) || expr_contains_aggregation(r)
        }
        Expression::Not(e) | Expression::IsNull(e) | Expression::IsNotNull(e) => {
            expr_contains_aggregation(e)
        }
        Expression::Comparison { left, right, .. } | Expression::StringOp { left, right, .. } => {
            expr_contains_aggregation(left) || expr_contains_aggregation(right)
        }
        Expression::Property(base, _) => expr_contains_aggregation(base),
        Expression::DynamicProperty { expr: e, key } => {
            expr_contains_aggregation(e) || expr_contains_aggregation(key)
        }
        Expression::ListSlice {
            expr: e,
            start,
            end,
        } => {
            expr_contains_aggregation(e)
                || start
                    .as_ref()
                    .map(|s| expr_contains_aggregation(s))
                    .unwrap_or(false)
                || end
                    .as_ref()
                    .map(|en| expr_contains_aggregation(en))
                    .unwrap_or(false)
        }
        Expression::Case {
            operand,
            alternatives,
            default,
        } => {
            operand
                .as_ref()
                .map(|o| expr_contains_aggregation(o))
                .unwrap_or(false)
                || alternatives.iter().any(|a| {
                    expr_contains_aggregation(&a.when) || expr_contains_aggregation(&a.then)
                })
                || default
                    .as_ref()
                    .map(|d| expr_contains_aggregation(d))
                    .unwrap_or(false)
        }
        Expression::RegexMatch { expr: e, pattern } => {
            expr_contains_aggregation(e) || expr_contains_aggregation(pattern)
        }
        Expression::In { expr: e, list } => {
            expr_contains_aggregation(e) || expr_contains_aggregation(list)
        }
        Expression::ListComprehension {
            list,
            predicate,
            map_expr,
            ..
        } => {
            expr_contains_aggregation(list)
                || predicate
                    .as_ref()
                    .map(|p| expr_contains_aggregation(p))
                    .unwrap_or(false)
                || map_expr
                    .as_ref()
                    .map(|m| expr_contains_aggregation(m))
                    .unwrap_or(false)
        }
        Expression::Literal(_) | Expression::Variable(_) | Expression::Parameter(_) => false,
        Expression::FilterPredicate {
            list, predicate, ..
        } => expr_contains_aggregation(list) || expr_contains_aggregation(predicate),
        Expression::Reduce {
            init, list, body, ..
        } => {
            expr_contains_aggregation(init)
                || expr_contains_aggregation(list)
                || expr_contains_aggregation(body)
        }
    }
}

/// Execute aggregation over input records, returning projected+aggregated output rows.
///
/// Items that are not aggregate functions are treated as grouping keys.
/// Items that are aggregate functions are computed over each group.
pub fn execute_aggregation(
    items: &[ReturnItem],
    input_records: &[Record],
    columns: &[String],
    params: &Parameters,
    storage: &GraphStorage,
) -> Result<Vec<Record>, CypherError> {
    // Separate grouping keys from aggregate items
    let group_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| !expr_contains_aggregation(&item.expression))
        .map(|(i, _)| i)
        .collect();

    let agg_indices: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| expr_contains_aggregation(&item.expression))
        .map(|(i, _)| i)
        .collect();

    // Group input records by the grouping key values
    let mut groups: Vec<(Vec<CypherValue>, Vec<&Record>)> = Vec::new();
    let mut group_key_map: HashMap<String, usize> = HashMap::new();

    for rec in input_records {
        let mut key_values: Vec<CypherValue> = Vec::with_capacity(group_indices.len());
        for &i in &group_indices {
            key_values.push(eval_with_params(
                &items[i].expression,
                rec,
                params,
                storage,
            )?);
        }

        // Build the group key string without an intermediate Vec allocation.
        // Null byte separator cannot appear in cypher_value_to_stable_key output.
        let mut key_str = String::new();
        for kv in &key_values {
            if !key_str.is_empty() {
                key_str.push('\x00');
            }
            key_str.push_str(&cypher_value_to_stable_key(kv));
        }

        let idx = if let Some(&existing) = group_key_map.get(&key_str) {
            existing
        } else {
            let new_idx = groups.len();
            groups.push((key_values, Vec::new()));
            group_key_map.insert(key_str, new_idx);
            new_idx
        };
        groups[idx].1.push(rec);
    }

    // If no input records and there are no grouping keys, produce one empty row so that
    // pure aggregations like COUNT(*) return 0 rather than no rows at all.
    // When grouping keys ARE present, empty input -> empty output (no groups).
    if input_records.is_empty() && group_indices.is_empty() {
        groups.push((Vec::new(), Vec::new()));
    }

    // For each group, compute aggregated values and build output rows
    let mut result = Vec::new();
    for (key_values, group_records) in groups {
        let mut out_rec = Record::new();

        // Set grouping key columns
        for (pos, &i) in group_indices.iter().enumerate() {
            out_rec.insert(columns[i].clone(), key_values[pos].clone());
        }

        // Compute aggregate columns
        for &i in &agg_indices {
            let val = compute_aggregate(&items[i].expression, &group_records, params, storage)?;
            out_rec.insert(columns[i].clone(), val);
        }

        result.push(out_rec);
    }

    Ok(result)
}

fn compute_aggregate(
    expr: &Expression,
    records: &[&Record],
    params: &Parameters,
    storage: &GraphStorage,
) -> Result<CypherValue, CypherError> {
    match expr {
        Expression::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let lower = name.to_lowercase();
            Ok(match lower.as_str() {
                "count" => {
                    if args.is_empty() {
                        // count(*) -- count all rows
                        CypherValue::Integer(records.len() as i64)
                    } else {
                        let vals: Vec<CypherValue> =
                            collect_non_null_values(&args[0], records, params, *distinct, storage)?;
                        CypherValue::Integer(vals.len() as i64)
                    }
                }
                "sum" => {
                    if let Some(arg) = args.first() {
                        let vals =
                            collect_non_null_values(arg, records, params, *distinct, storage)?;
                        return sum_values(&vals);
                    } else {
                        CypherValue::Integer(0)
                    }
                }
                "avg" => {
                    if let Some(arg) = args.first() {
                        let vals =
                            collect_non_null_values(arg, records, params, *distinct, storage)?;
                        avg_values(&vals)
                    } else {
                        CypherValue::Null
                    }
                }
                "min" => {
                    if let Some(arg) = args.first() {
                        let vals = collect_non_null_values(arg, records, params, false, storage)?;
                        min_value(&vals)
                    } else {
                        CypherValue::Null
                    }
                }
                "max" => {
                    if let Some(arg) = args.first() {
                        let vals = collect_non_null_values(arg, records, params, false, storage)?;
                        max_value(&vals)
                    } else {
                        CypherValue::Null
                    }
                }
                "collect" => {
                    if let Some(arg) = args.first() {
                        let vals =
                            collect_non_null_values(arg, records, params, *distinct, storage)?;
                        CypherValue::List(vals)
                    } else {
                        CypherValue::List(Vec::new())
                    }
                }
                "stdev" => {
                    if let Some(arg) = args.first() {
                        let vals =
                            collect_non_null_values(arg, records, params, *distinct, storage)?;
                        stdev_values(&vals, true)
                    } else {
                        CypherValue::Null
                    }
                }
                "stdevp" => {
                    if let Some(arg) = args.first() {
                        let vals =
                            collect_non_null_values(arg, records, params, *distinct, storage)?;
                        stdev_values(&vals, false)
                    } else {
                        CypherValue::Null
                    }
                }
                "percentilecont" | "percentiledisc" => {
                    if args.len() >= 2 {
                        let vals =
                            collect_non_null_values(&args[0], records, params, false, storage)?;
                        // The percentile argument is a constant/parameter, not row-dependent;
                        // evaluate it against an empty record to avoid accidental row coupling.
                        let percentile_val =
                            eval_with_params(&args[1], &Record::new(), params, storage)?;
                        if let CypherValue::Float(p) = percentile_val {
                            if lower == "percentilecont" {
                                percentile_cont(&vals, p)
                            } else {
                                percentile_disc(&vals, p)
                            }
                        } else if let CypherValue::Integer(p) = percentile_val {
                            let pf = p as f64;
                            if lower == "percentilecont" {
                                percentile_cont(&vals, pf)
                            } else {
                                percentile_disc(&vals, pf)
                            }
                        } else {
                            CypherValue::Null
                        }
                    } else {
                        CypherValue::Null
                    }
                }
                _ => CypherValue::Null,
            })
        }
        // For non-aggregate expressions inside aggregate context, evaluate against first record
        _ => {
            let empty = Record::new();
            let rec = records.first().copied().unwrap_or(&empty);
            eval_with_params(expr, rec, params, storage)
        }
    }
}

fn collect_non_null_values(
    expr: &Expression,
    records: &[&Record],
    params: &Parameters,
    distinct: bool,
    storage: &GraphStorage,
) -> Result<Vec<CypherValue>, CypherError> {
    let mut vals: Vec<CypherValue> = Vec::new();
    for rec in records {
        let v = eval_with_params(expr, rec, params, storage)?;
        if !matches!(v, CypherValue::Null) {
            vals.push(v);
        }
    }

    if distinct {
        let mut seen = std::collections::HashSet::new();
        vals.retain(|v| seen.insert(cypher_value_to_stable_key(v)));
    }

    Ok(vals)
}

fn sum_values(vals: &[CypherValue]) -> Result<CypherValue, CypherError> {
    if vals.is_empty() {
        return Ok(CypherValue::Integer(0));
    }
    let all_int = vals.iter().all(|v| matches!(v, CypherValue::Integer(_)));
    if all_int {
        let mut acc: i64 = 0;
        for v in vals {
            if let CypherValue::Integer(i) = v {
                acc = acc.checked_add(*i).ok_or_else(|| {
                    CypherError::RuntimeError("Integer overflow in SUM".to_string())
                })?;
            }
        }
        Ok(CypherValue::Integer(acc))
    } else {
        let s: f64 = vals.iter().filter_map(to_f64).sum();
        Ok(CypherValue::Float(s))
    }
}

fn avg_values(vals: &[CypherValue]) -> CypherValue {
    // Only numeric values participate in avg; non-numeric non-null values are ignored.
    let floats: Vec<f64> = vals.iter().filter_map(to_f64).collect();
    if floats.is_empty() {
        return CypherValue::Null;
    }
    CypherValue::Float(floats.iter().sum::<f64>() / floats.len() as f64)
}

fn min_value(vals: &[CypherValue]) -> CypherValue {
    vals.iter()
        .min_by(|a, b| compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal))
        .cloned()
        .unwrap_or(CypherValue::Null)
}

fn max_value(vals: &[CypherValue]) -> CypherValue {
    vals.iter()
        .max_by(|a, b| compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal))
        .cloned()
        .unwrap_or(CypherValue::Null)
}

fn stdev_values(vals: &[CypherValue], sample: bool) -> CypherValue {
    let floats: Vec<f64> = vals.iter().filter_map(to_f64).collect();
    if floats.is_empty() {
        return CypherValue::Null;
    }
    // Guard uses floats.len() (post-filter) so non-numeric values don't
    // inflate the count and allow a sample stdev with only 1 numeric value.
    if sample && floats.len() < 2 {
        return CypherValue::Null;
    }
    let mean = floats.iter().sum::<f64>() / floats.len() as f64;
    let variance: f64 = floats.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
        / if sample {
            (floats.len() - 1) as f64
        } else {
            floats.len() as f64
        };
    CypherValue::Float(variance.sqrt())
}

fn percentile_cont(vals: &[CypherValue], p: f64) -> CypherValue {
    // percentile must be in [0.0, 1.0]
    if !(0.0..=1.0).contains(&p) {
        return CypherValue::Null;
    }
    if vals.is_empty() {
        return CypherValue::Null;
    }
    let mut floats: Vec<f64> = vals.iter().filter_map(to_f64).collect();
    if floats.is_empty() {
        return CypherValue::Null;
    }
    floats.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = p * (floats.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        CypherValue::Float(floats[lo])
    } else {
        let frac = idx - lo as f64;
        CypherValue::Float(floats[lo] * (1.0 - frac) + floats[hi] * frac)
    }
}

fn percentile_disc(vals: &[CypherValue], p: f64) -> CypherValue {
    // percentile must be in [0.0, 1.0]
    if !(0.0..=1.0).contains(&p) {
        return CypherValue::Null;
    }
    if vals.is_empty() {
        return CypherValue::Null;
    }
    let mut sorted: Vec<CypherValue> = vals.to_vec();
    sorted.sort_by(|a, b| compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal));
    // openCypher spec: index = floor(p * (n - 1)), clamped to [0, n-1].
    let n = sorted.len();
    let idx = (p * (n - 1) as f64).floor() as usize;
    let idx = idx.min(n - 1);
    sorted[idx].clone()
}
