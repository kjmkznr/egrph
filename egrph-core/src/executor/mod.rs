pub mod aggregation;
pub mod expression;
pub mod result;

use self::aggregation::{execute_aggregation, items_contain_aggregation};
use self::expression::{
    Parameters, Record, compare_values, cypher_value_to_stable_key, eval_with_params, is_truthy,
};
use self::result::{QueryResult, ResultRow};
use crate::ast::*;
use crate::error::CypherError;
use crate::graph::backend::StorageBackend;
use crate::graph::types::*;
use crate::planner::plan::LogicalPlan;
use std::collections::HashMap;

pub fn execute<S: StorageBackend>(
    plan: &LogicalPlan,
    storage: &mut S,
) -> Result<QueryResult, CypherError> {
    let params: Parameters = HashMap::new();
    let (cols, records) = execute_to_records(plan, storage, &params)?;
    Ok(records_to_query_result(cols, records))
}

pub fn execute_with_params<S: StorageBackend>(
    plan: &LogicalPlan,
    storage: &mut S,
    params: Parameters,
) -> Result<QueryResult, CypherError> {
    let (cols, records) = execute_to_records(plan, storage, &params)?;
    Ok(records_to_query_result(cols, records))
}

/// Execute a plan and return (columns, records) where each record is a HashMap.
fn execute_to_records<S: StorageBackend>(
    plan: &LogicalPlan,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    match plan {
        LogicalPlan::EmptyRow => Ok((Vec::new(), vec![Record::new()])),

        LogicalPlan::CreateNode { .. } | LogicalPlan::CreatePath { .. } => {
            // Collect the chain of CreateNode/CreatePath nodes iteratively to avoid
            // deep recursion when a single CREATE clause has many patterns
            // (e.g. 1000 patterns produce a 1000-level deep input chain).
            enum CreateOp<'a> {
                Node(&'a NodePattern),
                Path(&'a NodePattern, &'a [PatternChainElement]),
            }
            let mut ops: Vec<CreateOp<'_>> = Vec::new();
            let mut current = plan;
            loop {
                match current {
                    LogicalPlan::CreateNode { input, pattern } => {
                        ops.push(CreateOp::Node(pattern));
                        current = input.as_ref();
                    }
                    LogicalPlan::CreatePath {
                        input,
                        start,
                        elements,
                    } => {
                        ops.push(CreateOp::Path(start, elements));
                        current = input.as_ref();
                    }
                    _ => break,
                }
            }
            ops.reverse(); // process in original order

            // Execute the base (non-Create) input first
            let (mut cols, mut records) = execute_to_records(current, storage, params)?;

            // Apply each create operation in order
            for op in &ops {
                match op {
                    CreateOp::Node(pattern) => {
                        let (new_cols, new_records) = execute_create_node_from_records(
                            cols, records, pattern, storage, params,
                        )?;
                        cols = new_cols;
                        records = new_records;
                    }
                    CreateOp::Path(start, elements) => {
                        let (new_cols, new_records) = execute_create_path_from_records(
                            cols, records, start, elements, storage, params,
                        )?;
                        cols = new_cols;
                        records = new_records;
                    }
                }
            }
            Ok((cols, records))
        }

        LogicalPlan::ScanNodes {
            label_filter,
            inline_props,
            variable,
        } => {
            let nodes = if inline_props.is_empty() {
                storage.match_nodes(label_filter.as_deref())
            } else {
                // Evaluate inline property expressions against an empty record.
                // Inline node-pattern properties are always literals, so this
                // succeeds immediately without graph access.
                let empty_rec = Record::new();
                let mut props_map: HashMap<String, PropertyValue> =
                    HashMap::with_capacity(inline_props.len());
                let mut use_index = true;
                for (key, expr) in inline_props {
                    match eval_with_params(expr, &empty_rec, params, storage) {
                        Ok(CypherValue::String(s)) => {
                            props_map.insert(key.clone(), PropertyValue::String(s));
                        }
                        Ok(CypherValue::Integer(i)) => {
                            props_map.insert(key.clone(), PropertyValue::Int(i));
                        }
                        Ok(CypherValue::Float(f)) => {
                            props_map.insert(key.clone(), PropertyValue::Float(f));
                        }
                        Ok(CypherValue::Boolean(b)) => {
                            props_map.insert(key.clone(), PropertyValue::Bool(b));
                        }
                        _ => {
                            use_index = false;
                            break;
                        }
                    }
                }
                if use_index {
                    storage.match_nodes_with_props(label_filter.as_deref(), &props_map)
                } else {
                    storage.match_nodes(label_filter.as_deref())
                }
            };
            let records: Vec<Record> = nodes
                .into_iter()
                .map(|node| {
                    let mut rec = Record::new();
                    rec.insert(variable.clone(), CypherValue::Node(node.clone()));
                    rec
                })
                .collect();
            Ok((vec![variable.clone()], records))
        }

        LogicalPlan::Expand {
            input,
            src_variable,
            rel_variable,
            dst_variable,
            rel_types,
            direction,
        } => {
            let (mut cols, input_records) = execute_to_records(input, storage, params)?;

            if let Some(rv) = rel_variable
                && !cols.contains(rv)
            {
                cols.push(rv.clone());
            }
            if !cols.contains(dst_variable) {
                cols.push(dst_variable.clone());
            }

            let mut result_records = Vec::new();

            for rec in input_records {
                let src_node_id = match rec.get(src_variable.as_str()) {
                    Some(CypherValue::Node(n)) => n.id,
                    _ => continue,
                };

                let edges = match direction {
                    Direction::Outgoing => storage.outgoing_edges(src_node_id),
                    Direction::Incoming => storage.incoming_edges(src_node_id),
                    Direction::Undirected => {
                        let mut all = storage.outgoing_edges(src_node_id);
                        all.extend(storage.incoming_edges(src_node_id));
                        all
                    }
                };

                // Resolve all matched (edge, dst_node) pairs first so we know the
                // final emission — that one can consume `rec` by move rather than
                // clone, saving one Arc::make_mut deep-copy per input row.
                let mut matched: Vec<(Edge, Node)> = Vec::new();
                for edge in edges {
                    if !rel_types.is_empty() && !rel_types.iter().any(|rt| rt == &edge.label) {
                        continue;
                    }
                    let dst_id = match direction {
                        Direction::Outgoing => edge.dst,
                        Direction::Incoming => edge.src,
                        Direction::Undirected => {
                            if edge.src == src_node_id {
                                edge.dst
                            } else {
                                edge.src
                            }
                        }
                    };
                    if let Some(dst_node) = storage.get_node(dst_id) {
                        matched.push((edge, dst_node));
                    }
                }

                let mut pending = Some(rec);
                let n = matched.len();
                for (i, (edge, dst_node)) in matched.into_iter().enumerate() {
                    let is_last = i + 1 == n;
                    let mut new_rec = if is_last {
                        pending.take().expect("pending present on last iteration")
                    } else {
                        pending
                            .as_ref()
                            .expect("pending present mid-iteration")
                            .clone()
                    };
                    if let Some(rv) = rel_variable {
                        new_rec.insert(rv.clone(), CypherValue::Relationship(edge));
                    }
                    new_rec.insert(dst_variable.clone(), CypherValue::Node(dst_node));
                    result_records.push(new_rec);
                }
            }

            Ok((cols, result_records))
        }

        LogicalPlan::Filter { input, predicate } => {
            let (cols, mut records) = execute_to_records(input, storage, params)?;
            let mut filter_err: Option<CypherError> = None;
            records.retain(|rec| {
                if filter_err.is_some() {
                    return false;
                }
                match eval_with_params(predicate, rec, params, storage) {
                    Ok(v) => is_truthy(&v),
                    Err(e) => {
                        filter_err = Some(e);
                        false
                    }
                }
            });
            if let Some(e) = filter_err {
                return Err(e);
            }
            Ok((cols, records))
        }

        LogicalPlan::Return {
            input,
            items,
            distinct,
        } => {
            let (input_cols, input_records) = execute_to_records(input, storage, params)?;

            // Expand RETURN * into all columns visible from the input plan.
            let effective_items: Vec<ReturnItem> = if items.len() == 1
                && matches!(&items[0].expression, Expression::Variable(v) if v == "*")
            {
                input_cols
                    .iter()
                    .map(|col| ReturnItem {
                        expression: Expression::Variable(col.clone()),
                        alias: None,
                    })
                    .collect()
            } else {
                items.to_vec()
            };

            let columns: Vec<String> = effective_items
                .iter()
                .map(|item| {
                    item.alias
                        .clone()
                        .unwrap_or_else(|| expr_to_column_name(&item.expression))
                })
                .collect();

            let mut rows: Vec<Record> = if items_contain_aggregation(&effective_items) {
                execute_aggregation(&effective_items, &input_records, &columns, params, storage)?
            } else {
                let mut projected = Vec::with_capacity(input_records.len());
                for rec in &input_records {
                    let mut new_rec = Record::new();
                    for (i, item) in effective_items.iter().enumerate() {
                        let val = eval_with_params(&item.expression, rec, params, storage)?;
                        new_rec.insert(columns[i].clone(), val);
                    }
                    projected.push(new_rec);
                }
                projected
            };

            if *distinct {
                let mut seen = std::collections::HashSet::new();
                rows.retain(|rec| {
                    // Build dedup key without intermediate Vec allocation.
                    let mut key = String::new();
                    for c in &columns {
                        if !key.is_empty() {
                            key.push('\x00');
                        }
                        key.push_str(&cypher_value_to_stable_key(
                            rec.get(c).unwrap_or(&CypherValue::Null),
                        ));
                    }
                    seen.insert(key)
                });
            }

            Ok((columns, rows))
        }

        LogicalPlan::Sort { input, items } => {
            let (cols, records) = execute_to_records(input, storage, params)?;

            // Pre-compute sort keys once per record: O(N*M) evaluations instead of
            // O(N log N * M) when re-evaluating expressions inside the comparator.
            let mut keyed: Vec<(Vec<CypherValue>, Record)> = Vec::with_capacity(records.len());
            for rec in records {
                let mut keys = Vec::with_capacity(items.len());
                for item in items {
                    keys.push(eval_with_params(&item.expression, &rec, params, storage)?);
                }
                keyed.push((keys, rec));
            }

            keyed.sort_by(|(keys_a, _), (keys_b, _)| {
                for (i, item) in items.iter().enumerate() {
                    let ord =
                        compare_values(&keys_a[i], &keys_b[i]).unwrap_or(std::cmp::Ordering::Equal);
                    let ord = if item.ascending { ord } else { ord.reverse() };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
                std::cmp::Ordering::Equal
            });

            let records = keyed.into_iter().map(|(_, rec)| rec).collect();
            Ok((cols, records))
        }

        LogicalPlan::Skip { input, count } => {
            let (cols, records) = execute_to_records(input, storage, params)?;
            // SKIP/LIMIT count expressions must be parameter-only or literals;
            // evaluate them against an empty record as per openCypher spec.
            let n = match eval_with_params(count, &Record::new(), params, storage)? {
                CypherValue::Integer(i) if i >= 0 => i as usize,
                CypherValue::Integer(i) => {
                    return Err(CypherError::TypeError(format!(
                        "SKIP requires a non-negative integer, got {}",
                        i
                    )));
                }
                other => {
                    return Err(CypherError::TypeError(format!(
                        "SKIP requires an integer expression, got {}",
                        cypher_value_type_name(&other)
                    )));
                }
            };
            let skipped: Vec<Record> = records.into_iter().skip(n).collect();
            Ok((cols, skipped))
        }

        LogicalPlan::Limit { input, count } => {
            let (cols, records) = execute_to_records(input, storage, params)?;
            let n = match eval_with_params(count, &Record::new(), params, storage)? {
                CypherValue::Integer(i) if i >= 0 => i as usize,
                CypherValue::Integer(i) => {
                    return Err(CypherError::TypeError(format!(
                        "LIMIT requires a non-negative integer, got {}",
                        i
                    )));
                }
                other => {
                    return Err(CypherError::TypeError(format!(
                        "LIMIT requires an integer expression, got {}",
                        cypher_value_type_name(&other)
                    )));
                }
            };
            let limited: Vec<Record> = records.into_iter().take(n).collect();
            Ok((cols, limited))
        }

        LogicalPlan::With {
            input,
            items,
            distinct,
            where_predicate,
        } => execute_with(
            input,
            items,
            *distinct,
            where_predicate.as_ref(),
            storage,
            params,
        ),

        LogicalPlan::Unwind {
            input,
            expression,
            alias,
        } => execute_unwind(input, expression, alias, storage, params),

        LogicalPlan::LoadCsv {
            input,
            url,
            alias,
            with_headers,
            field_terminator,
        } => execute_load_csv(
            input,
            url,
            alias,
            *with_headers,
            field_terminator.as_ref(),
            storage,
            params,
        ),

        LogicalPlan::SetOp { input, items } => execute_set(input, items, storage, params),

        LogicalPlan::RemoveOp { input, items } => execute_remove(input, items, storage, params),

        LogicalPlan::DeleteOp {
            input,
            expressions,
            detach,
        } => execute_delete(input, expressions, *detach, storage, params),

        LogicalPlan::MergeOp {
            input,
            pattern,
            on_create,
            on_match,
        } => execute_merge(
            input,
            pattern,
            on_create.as_deref(),
            on_match.as_deref(),
            storage,
            params,
        ),

        LogicalPlan::CartesianProduct { .. } => {
            // Collect the left-leaning chain of CartesianProduct nodes iteratively
            // to avoid deep recursion (e.g. 300 MATCH pairs produce a 300-level deep
            // left-skewed tree, which would overflow the WASM stack).
            let mut legs: Vec<&LogicalPlan> = Vec::new();
            let mut current = plan;
            while let LogicalPlan::CartesianProduct { left, right } = current {
                legs.push(right.as_ref());
                current = left.as_ref();
            }
            // `current` is now the leftmost non-CartesianProduct leaf
            legs.push(current);
            legs.reverse(); // legs[0] = leftmost leaf, legs[last] = rightmost right child

            // Execute each leg and combine iteratively
            let (mut cols, mut acc_records) = execute_to_records(legs[0], storage, params)?;
            for leg in &legs[1..] {
                let (right_cols, right_records) = execute_to_records(leg, storage, params)?;

                // Merge column lists, preserving order and deduplicating
                for c in &right_cols {
                    if !cols.contains(c) {
                        cols.push(c.clone());
                    }
                }

                // Fast path: when both sides have exactly one record (very common
                // for chained MATCH-with-unique-property queries), avoid allocating
                // a new Vec and cloning the left record — just extend it in place.
                if acc_records.len() == 1 && right_records.len() == 1 {
                    let right_rec = right_records.into_iter().next().unwrap();
                    acc_records[0].extend(right_rec);
                } else {
                    // General case: full cartesian product.
                    //
                    // Build each output row by cloning the left record and extending
                    // with the right entries. With Arc-backed Records, the per-cell
                    // `left_rec.clone()` is just an Arc bump and the first `extend`
                    // triggers `make_mut`'s deep copy. We also consume `acc_records`
                    // so the final right iteration for each left row *moves* the
                    // left record — its Arc refcount drops to 1 and `make_mut`
                    // mutates in place, saving one HashMap deep copy per left row.
                    let n = acc_records.len();
                    let m = right_records.len();
                    if m == 0 || n == 0 {
                        acc_records = Vec::new();
                    } else {
                        let mut next_acc = Vec::with_capacity(n * m);
                        let (right_last, right_init) = right_records.split_last().unwrap();
                        for left_rec in acc_records {
                            for right_rec in right_init {
                                let mut merged = left_rec.clone();
                                merged
                                    .extend(right_rec.iter().map(|(k, v)| (k.clone(), v.clone())));
                                next_acc.push(merged);
                            }
                            // Last right iteration for this left row: move left_rec.
                            let mut merged = left_rec;
                            merged.extend(right_last.iter().map(|(k, v)| (k.clone(), v.clone())));
                            next_acc.push(merged);
                        }
                        acc_records = next_acc;
                    }
                }
            }

            Ok((cols, acc_records))
        }

        LogicalPlan::MandatoryGuard { input } => {
            let (cols, records) = execute_to_records(input, storage, params)?;
            if records.is_empty() {
                return Err(CypherError::RuntimeError(
                    "MANDATORY MATCH pattern matched zero rows".to_string(),
                ));
            }
            Ok((cols, records))
        }

        LogicalPlan::LeftOuterJoin { left, right } => {
            let (left_cols, left_records) = execute_to_records(left, storage, params)?;
            let (right_cols, right_records) = execute_to_records(right, storage, params)?;

            // Shared variables: used as join conditions
            let shared_vars: Vec<String> = left_cols
                .iter()
                .filter(|c| right_cols.contains(c))
                .cloned()
                .collect();

            // Right-only variables: set to NULL when no right row matches
            let right_only_vars: Vec<String> = right_cols
                .iter()
                .filter(|c| !left_cols.contains(c))
                .cloned()
                .collect();

            let mut cols = left_cols.clone();
            for c in &right_only_vars {
                cols.push(c.clone());
            }

            // Build a composite stable key over the shared-variable values in a
            // record. Null bytes produced by cypher_value_to_stable_key are already
            // escaped for strings, so '\x00' is safe as a field separator.
            let build_key = |rec: &Record| -> String {
                let mut key = String::new();
                for (i, sv) in shared_vars.iter().enumerate() {
                    if i > 0 {
                        key.push('\x00');
                    }
                    key.push_str(&cypher_value_to_stable_key(
                        rec.get(sv.as_str()).unwrap_or(&CypherValue::Null),
                    ));
                }
                key
            };

            // Hash-index the right side by the join key so each left row probes
            // matches in O(1) instead of scanning all right rows.
            let mut right_index: HashMap<String, Vec<usize>> =
                HashMap::with_capacity(right_records.len());
            for (idx, rr) in right_records.iter().enumerate() {
                right_index.entry(build_key(rr)).or_default().push(idx);
            }

            let mut result = Vec::new();
            // Consume left_records by value so the final emission per left row
            // can move `left_rec` (Arc refcount 1 -> make_mut mutates in place,
            // saving one HashMap deep copy).
            for left_rec in left_records {
                let key = build_key(&left_rec);
                match right_index.get(&key) {
                    None => {
                        // No matching right rows: emit left row with right-only vars as NULL.
                        // Move left_rec — refcount becomes 1, so insert is in-place.
                        let mut merged = left_rec;
                        for v in &right_only_vars {
                            merged.insert(v.clone(), CypherValue::Null);
                        }
                        result.push(merged);
                    }
                    Some(right_indices) => {
                        // Matching right rows: emit one combined row per match.
                        // Clone left_rec for all but the last; move it on the last.
                        let mut pending = Some(left_rec);
                        let n = right_indices.len();
                        for (i, &ri) in right_indices.iter().enumerate() {
                            let is_last = i + 1 == n;
                            let right_rec = &right_records[ri];
                            let mut merged = if is_last {
                                pending.take().expect("pending present on last iteration")
                            } else {
                                pending
                                    .as_ref()
                                    .expect("pending present mid-iteration")
                                    .clone()
                            };
                            for v in &right_only_vars {
                                let val = right_rec
                                    .get(v.as_str())
                                    .cloned()
                                    .unwrap_or(CypherValue::Null);
                                merged.insert(v.clone(), val);
                            }
                            result.push(merged);
                        }
                    }
                }
            }

            Ok((cols, result))
        }

        LogicalPlan::VarLengthExpand {
            input,
            src_variable,
            rel_variable,
            dst_variable,
            rel_types,
            direction,
            min_hops,
            max_hops,
        } => execute_var_length_expand(
            input,
            src_variable,
            rel_variable.as_deref(),
            dst_variable,
            rel_types,
            direction,
            *min_hops,
            *max_hops,
            storage,
            params,
        ),

        LogicalPlan::ShortestPath {
            input,
            src_variable,
            dst_variable,
            rel_variable,
            path_variable,
            rel_types,
            direction,
            min_hops,
            max_hops,
            all_shortest,
        } => execute_shortest_path(
            input,
            src_variable,
            dst_variable,
            rel_variable.as_deref(),
            path_variable,
            rel_types,
            direction,
            *min_hops,
            *max_hops,
            *all_shortest,
            storage,
            params,
        ),

        LogicalPlan::CreateConstraint {
            label,
            properties,
            constraint_type,
        } => {
            match constraint_type {
                ConstraintType::Unique => {
                    let property = properties.first().ok_or_else(|| {
                        CypherError::SemanticError(
                            "UNIQUE constraint requires a property".to_string(),
                        )
                    })?;
                    storage
                        .add_unique_constraint(label, property)
                        .map_err(CypherError::ConstraintError)?;
                }
                ConstraintType::NotNull => {
                    let property = properties.first().ok_or_else(|| {
                        CypherError::SemanticError(
                            "NOT NULL constraint requires a property".to_string(),
                        )
                    })?;
                    storage
                        .add_not_null_constraint(label, property)
                        .map_err(CypherError::ConstraintError)?;
                }
                ConstraintType::NodeKey => {
                    storage
                        .add_node_key_constraint(label, properties)
                        .map_err(CypherError::ConstraintError)?;
                }
                ConstraintType::PropertyType(kind) => {
                    let property = properties.first().ok_or_else(|| {
                        CypherError::SemanticError(
                            "PROPERTY TYPE constraint requires a property".to_string(),
                        )
                    })?;
                    let type_name = match kind {
                        PropertyTypeKind::Boolean => "BOOLEAN",
                        PropertyTypeKind::String => "STRING",
                        PropertyTypeKind::Integer => "INTEGER",
                        PropertyTypeKind::Float => "FLOAT",
                    };
                    storage
                        .add_property_type_constraint(label, property, type_name)
                        .map_err(CypherError::ConstraintError)?;
                }
            }
            Ok((Vec::new(), Vec::new()))
        }

        LogicalPlan::Union { left, right, all } => {
            let (left_cols, left_records) = execute_to_records(left, storage, params)?;
            let (right_cols, right_records) = execute_to_records(right, storage, params)?;

            if left_cols.len() != right_cols.len() {
                return Err(CypherError::TypeError(
                    "All sub queries in a UNION must have the same number of columns".to_string(),
                ));
            }

            // Remap right-side records to use left-side column names (match by position)
            let remapped: Vec<Record> = right_records
                .into_iter()
                .map(|rec| {
                    right_cols.iter().zip(left_cols.iter()).fold(
                        Record::new(),
                        |mut r, (rk, lk)| {
                            r.insert(
                                lk.clone(),
                                rec.get(rk).cloned().unwrap_or(CypherValue::Null),
                            );
                            r
                        },
                    )
                })
                .collect();

            let mut combined = left_records;
            combined.extend(remapped);

            if !all {
                // UNION: deduplicate by stable key
                let mut seen = std::collections::HashSet::new();
                combined.retain(|rec| {
                    let key = left_cols
                        .iter()
                        .map(|col| {
                            cypher_value_to_stable_key(rec.get(col).unwrap_or(&CypherValue::Null))
                        })
                        .collect::<Vec<_>>()
                        .join("\x00");
                    seen.insert(key)
                });
            }

            Ok((left_cols, combined))
        }
    }
}

// --- Record sync helpers ---

/// Re-reads a node from storage after modification and updates its binding in the record.
///
/// Necessary because `Record` holds a snapshot of the node at read time; after a SET or
/// REMOVE operation the storage copy changes, so the record must be refreshed to reflect
/// the new state for any subsequent pipeline stages.
fn sync_node_in_record<S: StorageBackend>(
    rec: &mut Record,
    variable: &str,
    node_id: NodeId,
    storage: &S,
) {
    if let Some(updated) = storage.get_node(node_id) {
        rec.insert(variable.to_string(), CypherValue::Node(updated));
    }
}

/// Re-reads an edge from storage after modification and updates its binding in the record.
fn sync_edge_in_record<S: StorageBackend>(
    rec: &mut Record,
    variable: &str,
    edge_id: EdgeId,
    storage: &S,
) {
    if let Some(updated) = storage.get_edge(edge_id) {
        rec.insert(variable.to_string(), CypherValue::Relationship(updated));
    }
}

// --- Concrete executors ---

fn execute_create_node_from_records<S: StorageBackend>(
    mut cols: Vec<String>,
    input_records: Vec<Record>,
    pattern: &NodePattern,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    // Bind the created node variable if provided
    let var = pattern.variable.clone();
    if let Some(ref v) = var
        && !cols.contains(v)
    {
        cols.push(v.clone());
    }

    // For each input row, create one node and augment the record
    let base_records = if input_records.is_empty() {
        vec![Record::new()]
    } else {
        input_records
    };
    let mut result = Vec::with_capacity(base_records.len());
    for mut rec in base_records {
        let labels = pattern.labels.clone();
        let properties =
            resolve_map_literal_to_properties(&pattern.properties, &rec, params, storage)?;
        // Check all constraints before creating the node
        for label in &labels {
            for (prop_key, prop_val) in &properties {
                storage
                    .check_unique_constraint(label, prop_key, prop_val)
                    .map_err(CypherError::ConstraintError)?;
            }
        }
        storage
            .check_not_null_constraints(&labels, &properties)
            .map_err(CypherError::ConstraintError)?;
        storage
            .check_node_key_constraints(&labels, &properties)
            .map_err(CypherError::ConstraintError)?;
        storage
            .check_property_type_constraints(&labels, &properties)
            .map_err(CypherError::ConstraintError)?;
        let id = storage.create_node(labels, properties);
        if let Some(ref v) = var {
            let node = storage
                .get_node(id)
                .ok_or_else(|| {
                    CypherError::RuntimeError("Newly created node not found".to_string())
                })?
                .clone();
            rec.insert(v.clone(), CypherValue::Node(node));
        }
        result.push(rec);
    }

    Ok((cols, result))
}

fn execute_create_path_from_records<S: StorageBackend>(
    mut cols: Vec<String>,
    input_records: Vec<Record>,
    start: &NodePattern,
    elements: &[PatternChainElement],
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    // Collect all variable names we will bind
    let start_var = start.variable.clone();
    if let Some(ref v) = start_var
        && !cols.contains(v)
    {
        cols.push(v.clone());
    }
    for elem in elements {
        if let Some(ref rv) = elem.relationship.variable
            && !cols.contains(rv)
        {
            cols.push(rv.clone());
        }
        if let Some(ref dv) = elem.node.variable
            && !cols.contains(dv)
        {
            cols.push(dv.clone());
        }
    }

    let base_records = if input_records.is_empty() {
        vec![Record::new()]
    } else {
        input_records
    };
    let mut result = Vec::with_capacity(base_records.len());

    for mut rec in base_records {
        // If the start variable is already bound (e.g. from a preceding MATCH),
        // reuse that existing node rather than creating a new one.
        let start_id = if let Some(ref v) = start_var {
            if let Some(CypherValue::Node(n)) = rec.get(v) {
                n.id
            } else {
                let labels = start.labels.clone();
                let props =
                    resolve_map_literal_to_properties(&start.properties, &rec, params, storage)?;
                let id = storage.create_node(labels, props);
                let node = storage
                    .get_node(id)
                    .ok_or_else(|| {
                        CypherError::RuntimeError("Newly created node not found".to_string())
                    })?
                    .clone();
                rec.insert(v.clone(), CypherValue::Node(node));
                id
            }
        } else {
            let labels = start.labels.clone();
            let props =
                resolve_map_literal_to_properties(&start.properties, &rec, params, storage)?;
            storage.create_node(labels, props)
        };

        let mut prev_id = start_id;
        for elem in elements {
            // Same: reuse an already-bound destination node if present.
            let dst_id = if let Some(ref dv) = elem.node.variable {
                if let Some(CypherValue::Node(n)) = rec.get(dv) {
                    n.id
                } else {
                    let labels = elem.node.labels.clone();
                    let props = resolve_map_literal_to_properties(
                        &elem.node.properties,
                        &rec,
                        params,
                        storage,
                    )?;
                    let id = storage.create_node(labels, props);
                    let node = storage
                        .get_node(id)
                        .ok_or_else(|| {
                            CypherError::RuntimeError("Newly created node not found".to_string())
                        })?
                        .clone();
                    rec.insert(dv.clone(), CypherValue::Node(node));
                    id
                }
            } else {
                let labels = elem.node.labels.clone();
                let props = resolve_map_literal_to_properties(
                    &elem.node.properties,
                    &rec,
                    params,
                    storage,
                )?;
                storage.create_node(labels, props)
            };

            let edge_label = elem
                .relationship
                .rel_types
                .first()
                .cloned()
                .unwrap_or_default();
            let edge_props = resolve_map_literal_to_properties(
                &elem.relationship.properties,
                &rec,
                params,
                storage,
            )?;

            let (src, dst) = match elem.relationship.direction {
                Direction::Incoming => (dst_id, prev_id),
                _ => (prev_id, dst_id),
            };

            let eid = storage
                .create_edge(edge_label, src, dst, edge_props)
                .map_err(CypherError::RuntimeError)?;

            if let Some(ref rv) = elem.relationship.variable {
                let edge = storage
                    .get_edge(eid)
                    .ok_or_else(|| {
                        CypherError::RuntimeError("Newly created edge not found".to_string())
                    })?
                    .clone();
                rec.insert(rv.clone(), CypherValue::Relationship(edge));
            }

            prev_id = dst_id;
        }

        // Always emit exactly one row per input row: all created nodes/relationships
        // are bound in `rec` and available for subsequent pipeline stages.
        result.push(rec);
    }

    Ok((cols, result))
}

fn execute_with<S: StorageBackend>(
    input: &LogicalPlan,
    items: &[ReturnItem],
    distinct: bool,
    where_predicate: Option<&Expression>,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (_input_cols, input_records) = execute_to_records(input, storage, params)?;

    let columns: Vec<String> = items
        .iter()
        .map(|item| {
            item.alias
                .clone()
                .unwrap_or_else(|| expr_to_column_name(&item.expression))
        })
        .collect();

    let mut rows: Vec<Record> = if items_contain_aggregation(items) {
        execute_aggregation(items, &input_records, &columns, params, storage)?
    } else {
        let mut projected = Vec::with_capacity(input_records.len());
        for rec in &input_records {
            let mut new_rec = Record::new();
            for (i, item) in items.iter().enumerate() {
                let val = eval_with_params(&item.expression, rec, params, storage)?;
                new_rec.insert(columns[i].clone(), val);
            }
            projected.push(new_rec);
        }
        projected
    };

    if distinct {
        let mut seen = std::collections::HashSet::new();
        rows.retain(|rec| {
            // Build dedup key without intermediate Vec allocation.
            let mut key = String::new();
            for c in &columns {
                if !key.is_empty() {
                    key.push('\x00');
                }
                key.push_str(&cypher_value_to_stable_key(
                    rec.get(c).unwrap_or(&CypherValue::Null),
                ));
            }
            seen.insert(key)
        });
    }

    // Apply WHERE predicate if present
    if let Some(predicate) = where_predicate {
        let mut filtered = Vec::new();
        for rec in rows {
            if is_truthy(&eval_with_params(predicate, &rec, params, storage)?) {
                filtered.push(rec);
            }
        }
        rows = filtered;
    }

    Ok((columns, rows))
}

fn execute_unwind<S: StorageBackend>(
    input: &LogicalPlan,
    expression: &Expression,
    alias: &str,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

    if !cols.contains(&alias.to_string()) {
        cols.push(alias.to_string());
    }

    let mut result_records = Vec::new();

    for rec in input_records {
        let list_val = eval_with_params(expression, &rec, params, storage)?;
        match list_val {
            CypherValue::List(items) => {
                let n = items.len();
                let mut pending = Some(rec);
                for (i, item) in items.into_iter().enumerate() {
                    let is_last = i + 1 == n;
                    let mut new_rec = if is_last {
                        pending.take().expect("pending present on last iteration")
                    } else {
                        pending
                            .as_ref()
                            .expect("pending present mid-iteration")
                            .clone()
                    };
                    new_rec.insert(alias.to_string(), item);
                    result_records.push(new_rec);
                }
            }
            CypherValue::Null => {
                // UNWIND null produces no rows (openCypher spec)
            }
            other => {
                // UNWIND on a non-list scalar is a TypeError per openCypher spec
                return Err(CypherError::TypeError(format!(
                    "Type mismatch: expected List but was {}",
                    cypher_value_type_name(&other)
                )));
            }
        }
    }

    Ok((cols, result_records))
}

fn execute_load_csv<S: StorageBackend>(
    input: &LogicalPlan,
    url: &Expression,
    alias: &str,
    with_headers: bool,
    field_terminator: Option<&Expression>,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

    if !cols.contains(&alias.to_string()) {
        cols.push(alias.to_string());
    }

    let mut result_records = Vec::new();

    for rec in input_records {
        // Evaluate URL expression
        let url_val = eval_with_params(url, &rec, params, storage)?;
        let url_str = match url_val {
            CypherValue::String(s) => s,
            other => {
                return Err(CypherError::TypeError(format!(
                    "LOAD CSV: URL must be a string, got {}",
                    cypher_value_type_name(&other)
                )));
            }
        };

        // Resolve filesystem path from URL
        // Supports "file:///path" → "/path"  and bare paths
        let file_path = if let Some(rest) = url_str.strip_prefix("file://") {
            rest.to_string()
        } else {
            url_str.clone()
        };

        // Determine field terminator (default: comma)
        let delimiter: u8 = if let Some(ft_expr) = field_terminator {
            let ft_val = eval_with_params(ft_expr, &rec, params, storage)?;
            match ft_val {
                CypherValue::String(s) => {
                    let ch = s.chars().next().ok_or_else(|| {
                        CypherError::RuntimeError(
                            "LOAD CSV: FIELDTERMINATOR must not be empty".to_string(),
                        )
                    })?;
                    if !ch.is_ascii() {
                        return Err(CypherError::RuntimeError(
                            "LOAD CSV: FIELDTERMINATOR must be an ASCII character".to_string(),
                        ));
                    }
                    ch as u8
                }
                other => {
                    return Err(CypherError::TypeError(format!(
                        "LOAD CSV: FIELDTERMINATOR must be a string, got {}",
                        cypher_value_type_name(&other)
                    )));
                }
            }
        } else {
            b','
        };

        // Open and parse CSV file
        let file = std::fs::File::open(&file_path).map_err(|e| {
            CypherError::RuntimeError(format!("LOAD CSV: cannot open '{}': {}", file_path, e))
        })?;

        let mut reader = csv::ReaderBuilder::new()
            .has_headers(with_headers)
            .delimiter(delimiter)
            .from_reader(file);

        // Collect rows; when with_headers, the csv reader exposes headers separately
        if with_headers {
            let headers: Vec<String> = reader
                .headers()
                .map_err(|e| {
                    CypherError::RuntimeError(format!("LOAD CSV: failed to read headers: {}", e))
                })?
                .iter()
                .map(|s| s.to_string())
                .collect();

            for result in reader.records() {
                let csv_rec = result.map_err(|e| {
                    CypherError::RuntimeError(format!("LOAD CSV: failed to read record: {}", e))
                })?;

                let mut map: HashMap<String, CypherValue> = HashMap::new();
                for (i, field) in csv_rec.iter().enumerate() {
                    let key = headers.get(i).cloned().unwrap_or_else(|| i.to_string());
                    let val = if field.is_empty() {
                        CypherValue::Null
                    } else {
                        CypherValue::String(field.to_string())
                    };
                    map.insert(key, val);
                }

                let mut new_rec = rec.clone();
                new_rec.insert(alias.to_string(), CypherValue::Map(map));
                result_records.push(new_rec);
            }
        } else {
            for result in reader.records() {
                let csv_rec = result.map_err(|e| {
                    CypherError::RuntimeError(format!("LOAD CSV: failed to read record: {}", e))
                })?;

                let list: Vec<CypherValue> = csv_rec
                    .iter()
                    .map(|f| {
                        if f.is_empty() {
                            CypherValue::Null
                        } else {
                            CypherValue::String(f.to_string())
                        }
                    })
                    .collect();

                let mut new_rec = rec.clone();
                new_rec.insert(alias.to_string(), CypherValue::List(list));
                result_records.push(new_rec);
            }
        }
    }

    Ok((cols, result_records))
}

fn execute_set<S: StorageBackend>(
    input: &LogicalPlan,
    items: &[SetItem],
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, mut records) = execute_to_records(input, storage, params)?;

    for rec in &mut records {
        for item in items {
            apply_set_item(item, rec, storage, params)?;
        }
    }

    Ok((cols, records))
}

fn apply_set_item<S: StorageBackend>(
    item: &SetItem,
    rec: &mut Record,
    storage: &mut S,
    params: &Parameters,
) -> Result<(), CypherError> {
    match item {
        SetItem::Property {
            variable,
            property,
            expression,
        } => {
            let val = eval_with_params(expression, rec, params, storage)?;
            // Setting a property to null removes it (openCypher spec).
            if matches!(val, CypherValue::Null) {
                match rec.get(variable) {
                    Some(CypherValue::Node(n)) => {
                        let nid = n.id;
                        storage.remove_node_property(nid, property);
                        sync_node_in_record(rec, variable, nid, storage);
                    }
                    Some(CypherValue::Relationship(e)) => {
                        let eid = e.id;
                        storage.remove_edge_property(eid, property);
                        sync_edge_in_record(rec, variable, eid, storage);
                    }
                    _ => {}
                }
            } else {
                let prop_val = cypher_value_to_property(&val)?;
                match rec.get(variable) {
                    Some(CypherValue::Node(n)) => {
                        let nid = n.id;
                        storage.set_node_property(nid, property.clone(), prop_val);
                        sync_node_in_record(rec, variable, nid, storage);
                    }
                    Some(CypherValue::Relationship(e)) => {
                        let eid = e.id;
                        storage.set_edge_property(eid, property.clone(), prop_val);
                        sync_edge_in_record(rec, variable, eid, storage);
                    }
                    _ => {}
                }
            }
        }
        SetItem::AllProperties {
            variable,
            expression,
        } => {
            let val = eval_with_params(expression, rec, params, storage)?;
            let props = cypher_value_to_property_map(&val)?;
            match rec.get(variable) {
                Some(CypherValue::Node(n)) => {
                    let nid = n.id;
                    storage.set_node_all_properties(nid, props);
                    sync_node_in_record(rec, variable, nid, storage);
                }
                Some(CypherValue::Relationship(e)) => {
                    let eid = e.id;
                    storage.set_edge_all_properties(eid, props);
                    sync_edge_in_record(rec, variable, eid, storage);
                }
                _ => {}
            }
        }
        SetItem::MergeProperties {
            variable,
            expression,
        } => {
            let val = eval_with_params(expression, rec, params, storage)?;
            let props = cypher_value_to_property_map(&val)?;
            match rec.get(variable) {
                Some(CypherValue::Node(n)) => {
                    let nid = n.id;
                    storage.merge_node_properties(nid, props);
                    sync_node_in_record(rec, variable, nid, storage);
                }
                Some(CypherValue::Relationship(e)) => {
                    let eid = e.id;
                    storage.merge_edge_properties(eid, props);
                    sync_edge_in_record(rec, variable, eid, storage);
                }
                _ => {}
            }
        }
        SetItem::Label { variable, labels } => {
            if let Some(CypherValue::Node(n)) = rec.get(variable) {
                let nid = n.id;
                storage.add_node_labels(nid, labels);
                sync_node_in_record(rec, variable, nid, storage);
            }
        }
    }
    Ok(())
}

fn execute_remove<S: StorageBackend>(
    input: &LogicalPlan,
    items: &[RemoveItem],
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, mut records) = execute_to_records(input, storage, params)?;

    for rec in &mut records {
        for item in items {
            match item {
                RemoveItem::Property { variable, property } => match rec.get(variable) {
                    Some(CypherValue::Node(n)) => {
                        let nid = n.id;
                        storage.remove_node_property(nid, property);
                        sync_node_in_record(rec, variable, nid, storage);
                    }
                    Some(CypherValue::Relationship(e)) => {
                        let eid = e.id;
                        storage.remove_edge_property(eid, property);
                        sync_edge_in_record(rec, variable, eid, storage);
                    }
                    _ => {}
                },
                RemoveItem::Label { variable, labels } => {
                    if let Some(CypherValue::Node(n)) = rec.get(variable) {
                        let nid = n.id;
                        storage.remove_node_labels(nid, labels);
                        sync_node_in_record(rec, variable, nid, storage);
                    }
                }
            }
        }
    }

    Ok((cols, records))
}

fn execute_delete<S: StorageBackend>(
    input: &LogicalPlan,
    expressions: &[Expression],
    detach: bool,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, records) = execute_to_records(input, storage, params)?;

    // Collect all entities to delete first, using sets for O(1) deduplication.
    let mut node_ids: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    let mut edge_ids: std::collections::HashSet<EdgeId> = std::collections::HashSet::new();

    for rec in &records {
        for expr in expressions {
            let val = eval_with_params(expr, rec, params, storage)?;
            match val {
                CypherValue::Node(n) => {
                    node_ids.insert(n.id);
                }
                CypherValue::Relationship(e) => {
                    edge_ids.insert(e.id);
                }
                _ => {}
            }
        }
    }

    // Delete edges first, then nodes
    for eid in &edge_ids {
        storage
            .delete_edge(*eid)
            .map_err(CypherError::RuntimeError)?;
    }

    for nid in &node_ids {
        storage
            .delete_node(*nid, detach)
            .map_err(CypherError::RuntimeError)?;
    }

    // Keep rows but remove deleted entity bindings from each record so that
    // subsequent pipeline stages (e.g. RETURN a, b after DELETE r) still work.
    let output_records: Vec<Record> = records
        .into_iter()
        .map(|mut rec| {
            rec.retain(|_k, v| match v {
                CypherValue::Node(n) => !node_ids.contains(&n.id),
                CypherValue::Relationship(e) => !edge_ids.contains(&e.id),
                _ => true,
            });
            rec
        })
        .collect();

    Ok((cols, output_records))
}

fn execute_merge<S: StorageBackend>(
    input: &LogicalPlan,
    pattern: &PatternElement,
    on_create: Option<&[SetItem]>,
    on_match: Option<&[SetItem]>,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

    match pattern {
        PatternElement::Node(np) => {
            let variable = np
                .variable
                .clone()
                .unwrap_or_else(|| "_merge_node".to_string());
            if !cols.contains(&variable) {
                cols.push(variable.clone());
            }

            let labels = np.labels.clone();

            let base_records = if input_records.is_empty() {
                vec![Record::new()]
            } else {
                input_records
            };

            let mut result_records = Vec::new();

            // Resolve properties per input record so that bound variables
            // (e.g. from LOAD CSV or WITH) are evaluated correctly.
            for base_rec in base_records {
                let properties =
                    resolve_map_literal_to_properties(&np.properties, &base_rec, params, storage)?;
                let existing_ids = storage.find_nodes(&labels, &properties);

                if existing_ids.is_empty() {
                    // Not found — create one node and apply ON CREATE SET
                    let node_id = storage.create_node(labels.clone(), properties);
                    let node = storage
                        .get_node(node_id)
                        .ok_or_else(|| {
                            CypherError::RuntimeError("Newly created node not found".to_string())
                        })?
                        .clone();
                    let mut rec = base_rec;
                    rec.insert(variable.clone(), CypherValue::Node(node));
                    if let Some(items) = on_create {
                        for item in items {
                            apply_set_item(item, &mut rec, storage, params)?;
                        }
                    }
                    result_records.push(rec);
                } else {
                    // Found — produce one output row per matched node and apply ON MATCH SET
                    for node_id in existing_ids {
                        let node = storage
                            .get_node(node_id)
                            .ok_or_else(|| {
                                CypherError::RuntimeError("Merged node not found".to_string())
                            })?
                            .clone();
                        let mut rec = base_rec.clone();
                        rec.insert(variable.clone(), CypherValue::Node(node));
                        if let Some(items) = on_match {
                            for item in items {
                                apply_set_item(item, &mut rec, storage, params)?;
                            }
                        }
                        result_records.push(rec);
                    }
                }
            }

            Ok((cols, result_records))
        }
        PatternElement::Chain { start, elements } => execute_merge_chain(
            cols,
            input_records,
            start,
            elements,
            on_create,
            on_match,
            storage,
            params,
        ),
        PatternElement::ShortestPath { .. } | PatternElement::AllShortestPaths { .. } => {
            Err(CypherError::SemanticError(
                "shortestPath/allShortestPaths cannot be used in MERGE".to_string(),
            ))
        }
    }
}

/// MERGE on a relationship chain pattern.
///
/// For each incoming record, attempt to match the full chain against existing
/// graph state. If the whole chain matches, emit one row per matching path and
/// apply `ON MATCH SET`. If any step fails to match, emit a single row in
/// which every node/edge not already bound in the input record is created
/// (nodes/edges that *are* already bound are reused), then apply
/// `ON CREATE SET`.
///
/// Semantics decisions:
/// - `ON CREATE SET` fires only on the create branch; `ON MATCH SET` only on
///   the match branch. Both apply every item to the emitted row(s).
/// - Inline labels / properties attached to a variable that is already bound
///   in the input record act as additional equality filters in the match
///   branch (spec-compliant).
#[allow(clippy::too_many_arguments)]
fn execute_merge_chain<S: StorageBackend>(
    mut cols: Vec<String>,
    input_records: Vec<Record>,
    start: &NodePattern,
    elements: &[PatternChainElement],
    on_create: Option<&[SetItem]>,
    on_match: Option<&[SetItem]>,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    // Collect all variable names the chain binds.
    if let Some(v) = &start.variable
        && !cols.contains(v)
    {
        cols.push(v.clone());
    }
    for elem in elements {
        if let Some(rv) = &elem.relationship.variable
            && !cols.contains(rv)
        {
            cols.push(rv.clone());
        }
        if let Some(dv) = &elem.node.variable
            && !cols.contains(dv)
        {
            cols.push(dv.clone());
        }
    }

    let base_records = if input_records.is_empty() {
        vec![Record::new()]
    } else {
        input_records
    };

    let mut result_records: Vec<Record> = Vec::new();

    for rec in base_records {
        // Seed candidate walks from the start node.
        let start_candidates: Vec<(Record, NodeId)> =
            seed_merge_candidates(&rec, start, storage, params)?;

        // Walk each chain element, expanding candidates as we go.
        let mut candidates = start_candidates;
        for elem in elements {
            if candidates.is_empty() {
                break;
            }
            let mut next: Vec<(Record, NodeId)> = Vec::new();
            for (cand_rec, prev_id) in &candidates {
                expand_merge_candidate(cand_rec, *prev_id, elem, &mut next, storage, params)?;
            }
            candidates = next;
        }

        if !candidates.is_empty() {
            // Match branch: emit one row per successful walk and run ON MATCH.
            let mut branch: Vec<Record> = candidates.into_iter().map(|(r, _)| r).collect();
            if let Some(items) = on_match {
                for r in &mut branch {
                    for item in items {
                        apply_set_item(item, r, storage, params)?;
                    }
                }
            }
            result_records.extend(branch);
        } else {
            // Create branch: create every not-yet-bound node/edge, emit one row.
            let created = create_merge_chain_row(rec, start, elements, storage, params)?;
            let mut branch = vec![created];
            if let Some(items) = on_create {
                for r in &mut branch {
                    for item in items {
                        apply_set_item(item, r, storage, params)?;
                    }
                }
            }
            result_records.extend(branch);
        }
    }

    Ok((cols, result_records))
}

/// Produce the initial candidate set for a MERGE chain walk.
///
/// If `start.variable` is already bound in `rec`, reuse that node (subject to
/// any inline label/property filters in the pattern). Otherwise look up
/// candidate start nodes in storage.
fn seed_merge_candidates<S: StorageBackend>(
    rec: &Record,
    start: &NodePattern,
    storage: &S,
    params: &Parameters,
) -> Result<Vec<(Record, NodeId)>, CypherError> {
    let inline_props = resolve_map_literal_to_properties(&start.properties, rec, params, storage)?;

    if let Some(var) = &start.variable
        && let Some(CypherValue::Node(n)) = rec.get(var)
    {
        // Already bound — treat labels/props as additional filters.
        if !node_matches_filters(n, &start.labels, &inline_props) {
            return Ok(Vec::new());
        }
        return Ok(vec![(rec.clone(), n.id)]);
    }

    let ids = storage.find_nodes(&start.labels, &inline_props);
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let node = match storage.get_node(id) {
            Some(n) => n,
            None => continue,
        };
        let mut r = rec.clone();
        if let Some(var) = &start.variable {
            r.insert(var.clone(), CypherValue::Node(node));
        }
        out.push((r, id));
    }
    Ok(out)
}

/// Expand one chain element for a single candidate, pushing surviving
/// extensions into `next`.
fn expand_merge_candidate<S: StorageBackend>(
    cand_rec: &Record,
    prev_id: NodeId,
    elem: &PatternChainElement,
    next: &mut Vec<(Record, NodeId)>,
    storage: &S,
    params: &Parameters,
) -> Result<(), CypherError> {
    let rel_types = &elem.relationship.rel_types;
    let rel_props = resolve_map_literal_to_properties(
        &elem.relationship.properties,
        cand_rec,
        params,
        storage,
    )?;
    let node_props =
        resolve_map_literal_to_properties(&elem.node.properties, cand_rec, params, storage)?;
    let node_labels = &elem.node.labels;

    // If the destination variable is already bound in this candidate, require
    // the other endpoint to equal the bound node's id.
    let bound_dst: Option<NodeId> = elem
        .node
        .variable
        .as_ref()
        .and_then(|dv| cand_rec.get(dv))
        .and_then(|v| match v {
            CypherValue::Node(n) => Some(n.id),
            _ => None,
        });

    let edges = match elem.relationship.direction {
        Direction::Outgoing => storage.outgoing_edges(prev_id),
        Direction::Incoming => storage.incoming_edges(prev_id),
        Direction::Undirected => {
            let mut all = storage.outgoing_edges(prev_id);
            all.extend(storage.incoming_edges(prev_id));
            all
        }
    };

    for edge in edges {
        if !rel_types.is_empty() && !rel_types.iter().any(|rt| rt == &edge.label) {
            continue;
        }
        if !properties_match(&edge.properties, &rel_props) {
            continue;
        }
        let other_id = match elem.relationship.direction {
            Direction::Outgoing => edge.dst,
            Direction::Incoming => edge.src,
            Direction::Undirected => {
                if edge.src == prev_id {
                    edge.dst
                } else {
                    edge.src
                }
            }
        };
        if let Some(bid) = bound_dst
            && bid != other_id
        {
            continue;
        }
        let dst_node = match storage.get_node(other_id) {
            Some(n) => n,
            None => continue,
        };
        if !node_matches_filters(&dst_node, node_labels, &node_props) {
            continue;
        }
        let mut new_rec = cand_rec.clone();
        if let Some(rv) = &elem.relationship.variable {
            new_rec.insert(rv.clone(), CypherValue::Relationship(edge));
        }
        if let Some(dv) = &elem.node.variable {
            new_rec.insert(dv.clone(), CypherValue::Node(dst_node));
        }
        next.push((new_rec, other_id));
    }
    Ok(())
}

/// Create every not-yet-bound node/edge in the chain against the given input
/// record. Mirrors `execute_create_path_from_records`'s per-record body,
/// but always emits exactly one row (the caller handles ON CREATE SET).
fn create_merge_chain_row<S: StorageBackend>(
    mut rec: Record,
    start: &NodePattern,
    elements: &[PatternChainElement],
    storage: &mut S,
    params: &Parameters,
) -> Result<Record, CypherError> {
    let start_id = if let Some(v) = &start.variable {
        if let Some(CypherValue::Node(n)) = rec.get(v) {
            n.id
        } else {
            let labels = start.labels.clone();
            let props =
                resolve_map_literal_to_properties(&start.properties, &rec, params, storage)?;
            let id = storage.create_node(labels, props);
            let node = storage
                .get_node(id)
                .ok_or_else(|| {
                    CypherError::RuntimeError("Newly created node not found".to_string())
                })?
                .clone();
            rec.insert(v.clone(), CypherValue::Node(node));
            id
        }
    } else {
        let labels = start.labels.clone();
        let props = resolve_map_literal_to_properties(&start.properties, &rec, params, storage)?;
        storage.create_node(labels, props)
    };

    let mut prev_id = start_id;
    for elem in elements {
        let dst_id = if let Some(dv) = &elem.node.variable {
            if let Some(CypherValue::Node(n)) = rec.get(dv) {
                n.id
            } else {
                let labels = elem.node.labels.clone();
                let props = resolve_map_literal_to_properties(
                    &elem.node.properties,
                    &rec,
                    params,
                    storage,
                )?;
                let id = storage.create_node(labels, props);
                let node = storage
                    .get_node(id)
                    .ok_or_else(|| {
                        CypherError::RuntimeError("Newly created node not found".to_string())
                    })?
                    .clone();
                rec.insert(dv.clone(), CypherValue::Node(node));
                id
            }
        } else {
            let labels = elem.node.labels.clone();
            let props =
                resolve_map_literal_to_properties(&elem.node.properties, &rec, params, storage)?;
            storage.create_node(labels, props)
        };

        let edge_label = elem
            .relationship
            .rel_types
            .first()
            .cloned()
            .unwrap_or_default();
        let edge_props = resolve_map_literal_to_properties(
            &elem.relationship.properties,
            &rec,
            params,
            storage,
        )?;
        let (src, dst) = match elem.relationship.direction {
            Direction::Incoming => (dst_id, prev_id),
            _ => (prev_id, dst_id),
        };
        let eid = storage
            .create_edge(edge_label, src, dst, edge_props)
            .map_err(CypherError::RuntimeError)?;

        if let Some(rv) = &elem.relationship.variable {
            let edge = storage
                .get_edge(eid)
                .ok_or_else(|| {
                    CypherError::RuntimeError("Newly created edge not found".to_string())
                })?
                .clone();
            rec.insert(rv.clone(), CypherValue::Relationship(edge));
        }

        prev_id = dst_id;
    }

    Ok(rec)
}

/// Return true if `node` carries every label in `required_labels` and every
/// property in `required_props` matches by value.
fn node_matches_filters(
    node: &Node,
    required_labels: &[String],
    required_props: &HashMap<String, PropertyValue>,
) -> bool {
    if !required_labels
        .iter()
        .all(|l| node.labels.iter().any(|nl| nl == l))
    {
        return false;
    }
    properties_match(&node.properties, required_props)
}

/// Return true if `actual` contains every (key, value) pair in `required`.
fn properties_match(
    actual: &HashMap<String, PropertyValue>,
    required: &HashMap<String, PropertyValue>,
) -> bool {
    required
        .iter()
        .all(|(k, v)| actual.get(k).map(|av| av == v).unwrap_or(false))
}

/// Walk a chain-encoded path backwards and return true if `edge` is already on it.
/// The arena stores each step as (parent_index, edge_id), so traversal follows parent
/// links until reaching the empty-path sentinel (`None`).
fn path_chain_contains(
    arena: &[(Option<u32>, EdgeId)],
    mut cur: Option<u32>,
    edge: EdgeId,
) -> bool {
    while let Some(i) = cur {
        let (parent, e) = arena[i as usize];
        if e == edge {
            return true;
        }
        cur = parent;
    }
    false
}

/// Like `path_chain_contains` but for the 3-tuple arena used by `execute_shortest_path`.
fn path_chain_contains_sp(
    arena: &[(Option<u32>, EdgeId, NodeId)],
    mut cur: Option<u32>,
    edge: EdgeId,
) -> bool {
    while let Some(i) = cur {
        let (parent, e, _) = arena[i as usize];
        if e == edge {
            return true;
        }
        cur = parent;
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn execute_var_length_expand<S: StorageBackend>(
    input: &LogicalPlan,
    src_variable: &str,
    rel_variable: Option<&str>,
    dst_variable: &str,
    rel_types: &[String],
    direction: &Direction,
    min_hops: u64,
    max_hops: Option<u64>,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

    if let Some(rv) = rel_variable
        && !cols.contains(&rv.to_string())
    {
        cols.push(rv.to_string());
    }
    if !cols.contains(&dst_variable.to_string()) {
        cols.push(dst_variable.to_string());
    }

    // Default ceiling for unbounded variable-length patterns (`-[*]->`).
    // Bounded patterns (`-[*..N]->`) use the user-specified max directly.
    // Unbounded patterns that reach this limit return a query error instead of
    // silently truncating results, so callers are aware they need an explicit bound.
    const BFS_DEFAULT_MAX_HOPS: u64 = 100;
    let is_unbounded = max_hops.is_none();
    let effective_max = max_hops.unwrap_or(BFS_DEFAULT_MAX_HOPS);
    let mut result_records = Vec::new();

    for rec in &input_records {
        let start_id = match rec.get(src_variable) {
            Some(CypherValue::Node(n)) => n.id,
            _ => continue,
        };

        // BFS state: (current_node_id, path_index, depth).
        // The path is represented as an index into `path_arena`, where each entry is
        // (parent_index, edge_id). Extending a path is O(1) (one arena push) instead of
        // cloning the full Vec<EdgeId> at every step. Prefixes are shared across sibling
        // paths, so memory use is proportional to unique BFS frontier edges rather than
        // (paths × depth).
        let mut path_arena: Vec<(Option<u32>, EdgeId)> = Vec::new();
        let mut queue: std::collections::VecDeque<(NodeId, Option<u32>, u64)> =
            std::collections::VecDeque::new();
        queue.push_back((start_id, None, 0));

        while let Some((cur_id, path_idx, depth)) = queue.pop_front() {
            if depth >= effective_max {
                if is_unbounded {
                    return Err(CypherError::RuntimeError(format!(
                        "Variable-length path traversal exceeded the default limit of {} hops. \
                         Use an explicit upper bound, e.g. [*..{}], to suppress this error.",
                        BFS_DEFAULT_MAX_HOPS, BFS_DEFAULT_MAX_HOPS
                    )));
                }
                continue;
            }

            // Collect candidate edge IDs. The trait returns Vec<EdgeId> (owned).
            let candidate_ids: Vec<EdgeId> = match direction {
                Direction::Outgoing => storage.outgoing_edge_ids(cur_id),
                Direction::Incoming => storage.incoming_edge_ids(cur_id),
                Direction::Undirected => {
                    let mut out = storage.outgoing_edge_ids(cur_id);
                    out.extend(storage.incoming_edge_ids(cur_id));
                    out
                }
            };

            for &eid in &candidate_ids {
                // Resolve edge — skip stale IDs left by earlier deletes.
                let Some(edge) = storage.get_edge(eid) else {
                    continue;
                };

                // Skip edges already in the path (no repeated relationships).
                if path_chain_contains(&path_arena, path_idx, eid) {
                    continue;
                }

                // Apply relationship type filter before extending the path.
                if !rel_types.is_empty() && !rel_types.iter().any(|rt| rt == &edge.label) {
                    continue;
                }

                let next_id = match direction {
                    Direction::Outgoing => edge.dst,
                    Direction::Incoming => edge.src,
                    Direction::Undirected => {
                        if edge.src == cur_id {
                            edge.dst
                        } else {
                            edge.src
                        }
                    }
                };

                let new_idx = u32::try_from(path_arena.len()).map_err(|_| {
                    CypherError::RuntimeError(
                        "Variable-length path traversal exceeded internal path arena capacity"
                            .to_string(),
                    )
                })?;
                path_arena.push((path_idx, eid));
                let new_path_idx = Some(new_idx);
                let new_depth = depth + 1;

                // Emit a result row when depth is within [min_hops, max_hops].
                if new_depth >= min_hops
                    && let Some(dst_node) = storage.get_node(next_id)
                {
                    let mut new_rec = rec.clone();
                    if let Some(rv) = rel_variable {
                        // Walk the chain once to resolve edges in traversal order.
                        let mut edges: Vec<CypherValue> = Vec::with_capacity(new_depth as usize);
                        let mut cur = new_path_idx;
                        while let Some(i) = cur {
                            let (parent, e) = path_arena[i as usize];
                            if let Some(edge) = storage.get_edge(e) {
                                edges.push(CypherValue::Relationship(edge));
                            }
                            cur = parent;
                        }
                        edges.reverse();
                        new_rec.insert(rv.to_string(), CypherValue::List(edges));
                    }
                    new_rec.insert(dst_variable.to_string(), CypherValue::Node(dst_node));
                    result_records.push(new_rec);
                }

                // Continue BFS if we haven't hit the maximum depth.
                if new_depth < effective_max {
                    queue.push_back((next_id, new_path_idx, new_depth));
                }
            }
        }
    }

    Ok((cols, result_records))
}

/// Convert a CypherValue to a PropertyValue for storage.
fn cypher_value_to_property(val: &CypherValue) -> Result<PropertyValue, CypherError> {
    match val {
        CypherValue::String(s) => Ok(PropertyValue::String(s.clone())),
        CypherValue::Integer(i) => Ok(PropertyValue::Int(*i)),
        CypherValue::Float(f) => Ok(PropertyValue::Float(*f)),
        CypherValue::Boolean(b) => Ok(PropertyValue::Bool(*b)),
        CypherValue::Null => Err(CypherError::TypeError(
            "Cannot store null as a property value".to_string(),
        )),
        _ => Err(CypherError::TypeError(
            "Cannot store complex value as a property".to_string(),
        )),
    }
}

/// Convert a CypherValue (expected to be a Map) to a property map.
fn cypher_value_to_property_map(
    val: &CypherValue,
) -> Result<HashMap<String, PropertyValue>, CypherError> {
    match val {
        CypherValue::Map(map) => {
            let mut result = HashMap::new();
            for (k, v) in map {
                result.insert(k.clone(), cypher_value_to_property(v)?);
            }
            Ok(result)
        }
        CypherValue::Node(n) => {
            // When assigning a node to properties, copy its properties
            Ok(n.properties.clone())
        }
        CypherValue::Null => Ok(HashMap::new()),
        _ => Err(CypherError::TypeError(
            "Expected a map value for property assignment".to_string(),
        )),
    }
}

fn records_to_query_result(columns: Vec<String>, records: Vec<Record>) -> QueryResult {
    let rows = records
        .into_iter()
        .map(|rec| {
            let values = columns
                .iter()
                .map(|col| rec.get(col).cloned().unwrap_or(CypherValue::Null))
                .collect();
            ResultRow { values }
        })
        .collect();
    QueryResult { columns, rows }
}

fn cypher_value_type_name(val: &CypherValue) -> &'static str {
    match val {
        CypherValue::Null => "Null",
        CypherValue::Boolean(_) => "Boolean",
        CypherValue::Integer(_) => "Integer",
        CypherValue::Float(_) => "Float",
        CypherValue::String(_) => "String",
        CypherValue::List(_) => "List",
        CypherValue::Map(_) => "Map",
        CypherValue::Node(_) => "Node",
        CypherValue::Relationship(_) => "Relationship",
        CypherValue::Path(_) => "Path",
        CypherValue::Date(_) => "Date",
        CypherValue::Timestamp(_) => "Timestamp",
    }
}

fn expr_to_column_name(expr: &Expression) -> String {
    match expr {
        Expression::Variable(name) => name.clone(),
        Expression::Property(base, prop) => {
            format!("{}.{}", expr_to_column_name(base), prop)
        }
        Expression::FunctionCall { name, .. } => format!("{}(..)", name),
        _ => "?column?".to_string(),
    }
}

fn resolve_map_literal_to_properties<S: StorageBackend>(
    map_lit: &Option<MapLiteral>,
    record: &Record,
    params: &Parameters,
    storage: &S,
) -> Result<HashMap<String, PropertyValue>, CypherError> {
    let mut properties = HashMap::new();
    if let Some(map) = map_lit {
        for (key, expr) in &map.entries {
            let MapKey::Identifier(key_str) = key else {
                continue;
            };
            let val = eval_with_params(expr, record, params, storage)?;
            let prop = cypher_value_to_property(&val)?;
            properties.insert(key_str.clone(), prop);
        }
    }
    Ok(properties)
}

#[allow(clippy::too_many_arguments)]
fn execute_shortest_path<S: StorageBackend>(
    input: &LogicalPlan,
    src_variable: &str,
    dst_variable: &str,
    rel_variable: Option<&str>,
    path_variable: &str,
    rel_types: &[String],
    direction: &Direction,
    min_hops: u64,
    max_hops: Option<u64>,
    all_shortest: bool,
    storage: &mut S,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

    if !cols.contains(&path_variable.to_string()) {
        cols.push(path_variable.to_string());
    }
    if let Some(rv) = rel_variable
        && !cols.contains(&rv.to_string())
    {
        cols.push(rv.to_string());
    }

    const BFS_DEFAULT_MAX_HOPS: u64 = 100;
    let is_unbounded = max_hops.is_none();
    let effective_max = max_hops.unwrap_or(BFS_DEFAULT_MAX_HOPS);

    let mut result_records: Vec<Record> = Vec::new();

    for rec in &input_records {
        let start_id = match rec.get(src_variable) {
            Some(CypherValue::Node(n)) => n.id,
            _ => continue,
        };
        let target_id = match rec.get(dst_variable) {
            Some(CypherValue::Node(n)) => n.id,
            _ => continue,
        };

        // Trivial: src == dst (zero-length path)
        if start_id == target_id {
            if min_hops == 0
                && let Some(node) = storage.get_node(start_id)
            {
                let path_val = CypherValue::Path(Path {
                    nodes: vec![node],
                    relationships: vec![],
                });
                let mut new_rec = rec.clone();
                new_rec.insert(path_variable.to_string(), path_val);
                if let Some(rv) = rel_variable {
                    new_rec.insert(rv.to_string(), CypherValue::List(vec![]));
                }
                result_records.push(new_rec);
            }
            continue;
        }

        // Arena: (parent_idx, edge_id, arrived_node_id)
        let mut path_arena: Vec<(Option<u32>, EdgeId, NodeId)> = Vec::new();

        // frontier: (current_node_id, path_arena_index_of_last_step)
        let mut frontier: Vec<(NodeId, Option<u32>)> = vec![(start_id, None)];

        // Track the depth at which each node was first reached.
        let mut visited_depth: HashMap<NodeId, u64> = HashMap::new();
        visited_depth.insert(start_id, 0);

        let mut found_paths: Vec<u32> = Vec::new();

        'bfs: for depth in 1..=effective_max {
            if is_unbounded && depth > BFS_DEFAULT_MAX_HOPS {
                return Err(CypherError::RuntimeError(format!(
                    "shortestPath traversal exceeded the default limit of {} hops. \
                     Use an explicit upper bound, e.g. [*..{}], to suppress this error.",
                    BFS_DEFAULT_MAX_HOPS, BFS_DEFAULT_MAX_HOPS
                )));
            }

            let mut next_frontier: Vec<(NodeId, Option<u32>)> = Vec::new();

            for (cur_id, parent_idx) in &frontier {
                let candidate_eids: Vec<EdgeId> = match direction {
                    Direction::Outgoing => storage.outgoing_edge_ids(*cur_id),
                    Direction::Incoming => storage.incoming_edge_ids(*cur_id),
                    Direction::Undirected => {
                        let mut out = storage.outgoing_edge_ids(*cur_id);
                        out.extend(storage.incoming_edge_ids(*cur_id));
                        out
                    }
                };

                for eid in candidate_eids {
                    // No repeated edges on a single path
                    if path_chain_contains_sp(&path_arena, *parent_idx, eid) {
                        continue;
                    }

                    let Some(edge) = storage.get_edge(eid) else {
                        continue;
                    };

                    // Relationship type filter
                    if !rel_types.is_empty() && !rel_types.iter().any(|rt| rt == &edge.label) {
                        continue;
                    }

                    let next_id = match direction {
                        Direction::Outgoing => edge.dst,
                        Direction::Incoming => edge.src,
                        Direction::Undirected => {
                            if edge.src == *cur_id {
                                edge.dst
                            } else {
                                edge.src
                            }
                        }
                    };

                    // Visited-depth check.
                    // allShortestPaths: allow revisiting a node at the same depth (multiple paths).
                    // shortestPath: skip any already-visited node to avoid redundant paths.
                    if let Some(&prev_depth) = visited_depth.get(&next_id) {
                        if all_shortest {
                            if prev_depth != depth {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }

                    let new_idx = u32::try_from(path_arena.len()).map_err(|_| {
                        CypherError::RuntimeError(
                            "shortestPath traversal exceeded internal arena capacity".to_string(),
                        )
                    })?;
                    path_arena.push((*parent_idx, eid, next_id));
                    let new_path_idx = Some(new_idx);

                    if next_id == target_id && depth >= min_hops {
                        found_paths.push(new_idx);
                        if !all_shortest {
                            break 'bfs;
                        }
                    }

                    visited_depth.entry(next_id).or_insert(depth);
                    next_frontier.push((next_id, new_path_idx));
                }
            }

            if !found_paths.is_empty() {
                break;
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        // Reconstruct Path values from arena
        for &terminal_idx in &found_paths {
            let mut edges_rev: Vec<Edge> = Vec::new();
            let mut nodes_rev: Vec<Node> = Vec::new();

            let mut cur = Some(terminal_idx);
            while let Some(i) = cur {
                let (parent, eid, node_id) = path_arena[i as usize];
                if let (Some(edge), Some(node)) = (storage.get_edge(eid), storage.get_node(node_id))
                {
                    edges_rev.push(edge);
                    nodes_rev.push(node);
                }
                cur = parent;
            }
            edges_rev.reverse();
            nodes_rev.reverse();

            let mut all_nodes: Vec<Node> = Vec::with_capacity(nodes_rev.len() + 1);
            if let Some(start_node) = storage.get_node(start_id) {
                all_nodes.push(start_node);
            }
            all_nodes.extend(nodes_rev);

            let rel_list: Vec<CypherValue> = edges_rev
                .iter()
                .map(|e| CypherValue::Relationship(e.clone()))
                .collect();

            let path_val = CypherValue::Path(Path {
                nodes: all_nodes,
                relationships: edges_rev,
            });

            let mut new_rec = rec.clone();
            new_rec.insert(path_variable.to_string(), path_val);
            if let Some(rv) = rel_variable {
                new_rec.insert(rv.to_string(), CypherValue::List(rel_list));
            }
            result_records.push(new_rec);
        }
    }

    Ok((cols, result_records))
}
