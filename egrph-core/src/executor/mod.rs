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
use crate::graph::storage::GraphStorage;
use crate::graph::types::*;
use crate::planner::plan::LogicalPlan;
use std::collections::HashMap;

pub fn execute(plan: &LogicalPlan, storage: &mut GraphStorage) -> Result<QueryResult, CypherError> {
    let params: Parameters = HashMap::new();
    let (cols, records) = execute_to_records(plan, storage, &params)?;
    Ok(records_to_query_result(cols, records))
}

/// Execute a plan and return (columns, records) where each record is a HashMap.
fn execute_to_records(
    plan: &LogicalPlan,
    storage: &mut GraphStorage,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    match plan {
        LogicalPlan::EmptyRow => Ok((Vec::new(), vec![Record::new()])),

        LogicalPlan::CreateNode { input, pattern } => {
            execute_create_node(input, pattern, storage, params)
        }

        LogicalPlan::CreatePath {
            input,
            start,
            elements,
        } => execute_create_path(input, start, elements, storage, params),

        LogicalPlan::ScanNodes {
            label_filter,
            variable,
        } => {
            let nodes = storage.match_nodes(label_filter.as_deref());
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

            for rec in &input_records {
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
                        let mut new_rec = rec.clone();
                        if let Some(rv) = rel_variable {
                            new_rec.insert(rv.clone(), CypherValue::Relationship(edge.clone()));
                        }
                        new_rec.insert(dst_variable.clone(), CypherValue::Node(dst_node.clone()));
                        result_records.push(new_rec);
                    }
                }
            }

            Ok((cols, result_records))
        }

        LogicalPlan::Filter { input, predicate } => {
            let (cols, records) = execute_to_records(input, storage, params)?;
            let mut filtered: Vec<Record> = Vec::new();
            for rec in records {
                if is_truthy(&eval_with_params(predicate, &rec, params, storage)?) {
                    filtered.push(rec);
                }
            }
            Ok((cols, filtered))
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
                    let key = columns
                        .iter()
                        .map(|c| {
                            cypher_value_to_stable_key(rec.get(c).unwrap_or(&CypherValue::Null))
                        })
                        .collect::<Vec<_>>()
                        .join("\x00");
                    seen.insert(key)
                });
            }

            Ok((columns, rows))
        }

        LogicalPlan::Sort { input, items } => {
            let (cols, mut records) = execute_to_records(input, storage, params)?;
            let mut sort_err: Option<CypherError> = None;
            records.sort_by(|a, b| {
                if sort_err.is_some() {
                    return std::cmp::Ordering::Equal;
                }
                for item in items {
                    let va = match eval_with_params(&item.expression, a, params, storage) {
                        Ok(v) => v,
                        Err(e) => {
                            sort_err = Some(e);
                            return std::cmp::Ordering::Equal;
                        }
                    };
                    let vb = match eval_with_params(&item.expression, b, params, storage) {
                        Ok(v) => v,
                        Err(e) => {
                            sort_err = Some(e);
                            return std::cmp::Ordering::Equal;
                        }
                    };
                    let ord = compare_values(&va, &vb).unwrap_or(std::cmp::Ordering::Equal);
                    let ord = if item.ascending { ord } else { ord.reverse() };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
                std::cmp::Ordering::Equal
            });
            if let Some(e) = sort_err {
                return Err(e);
            }
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

        LogicalPlan::CartesianProduct { left, right } => {
            let (left_cols, left_records) = execute_to_records(left, storage, params)?;
            let (right_cols, right_records) = execute_to_records(right, storage, params)?;

            // Merge column lists, preserving order and deduplicating
            let mut cols = left_cols.clone();
            for c in &right_cols {
                if !cols.contains(c) {
                    cols.push(c.clone());
                }
            }

            // For each left row, produce one output row combined with each right row
            let mut result = Vec::with_capacity(left_records.len() * right_records.len().max(1));
            for left_rec in &left_records {
                for right_rec in &right_records {
                    let mut merged = left_rec.clone();
                    for (k, v) in right_rec {
                        merged.insert(k.clone(), v.clone());
                    }
                    result.push(merged);
                }
            }

            Ok((cols, result))
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

            let mut result = Vec::new();
            for left_rec in &left_records {
                let matching_right: Vec<&Record> = right_records
                    .iter()
                    .filter(|rr| {
                        shared_vars.iter().all(|sv| {
                            let lv = cypher_value_to_stable_key(
                                left_rec.get(sv.as_str()).unwrap_or(&CypherValue::Null),
                            );
                            let rv = cypher_value_to_stable_key(
                                rr.get(sv.as_str()).unwrap_or(&CypherValue::Null),
                            );
                            lv == rv
                        })
                    })
                    .collect();

                if matching_right.is_empty() {
                    // No matching right rows: emit left row with right-only vars as NULL
                    let mut merged = left_rec.clone();
                    for v in &right_only_vars {
                        merged.insert(v.clone(), CypherValue::Null);
                    }
                    result.push(merged);
                } else {
                    // Matching right rows found: emit one combined row per match
                    for right_rec in matching_right {
                        let mut merged = left_rec.clone();
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
    }
}

// --- Concrete executors ---

fn execute_create_node(
    input: &LogicalPlan,
    pattern: &NodePattern,
    storage: &mut GraphStorage,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

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
        let properties = resolve_map_literal_to_properties(&pattern.properties, params)?;
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

fn execute_create_path(
    input: &LogicalPlan,
    start: &NodePattern,
    elements: &[PatternChainElement],
    storage: &mut GraphStorage,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

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
        let start_labels = start.labels.clone();
        let start_props = resolve_map_literal_to_properties(&start.properties, params)?;
        let start_id = storage.create_node(start_labels, start_props);
        if let Some(ref v) = start_var {
            let node = storage
                .get_node(start_id)
                .ok_or_else(|| {
                    CypherError::RuntimeError("Newly created node not found".to_string())
                })?
                .clone();
            rec.insert(v.clone(), CypherValue::Node(node));
        }

        let mut prev_id = start_id;
        for elem in elements {
            let dst_labels = elem.node.labels.clone();
            let dst_props = resolve_map_literal_to_properties(&elem.node.properties, params)?;
            let dst_id = storage.create_node(dst_labels, dst_props);

            let edge_label = elem
                .relationship
                .rel_types
                .first()
                .cloned()
                .unwrap_or_default();
            let edge_props =
                resolve_map_literal_to_properties(&elem.relationship.properties, params)?;

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
            if let Some(ref dv) = elem.node.variable {
                let node = storage
                    .get_node(dst_id)
                    .ok_or_else(|| {
                        CypherError::RuntimeError("Newly created node not found".to_string())
                    })?
                    .clone();
                rec.insert(dv.clone(), CypherValue::Node(node));
            }

            prev_id = dst_id;
        }

        // Always emit exactly one row per input row: all created nodes/relationships
        // are bound in `rec` and available for subsequent pipeline stages.
        result.push(rec);
    }

    Ok((cols, result))
}

fn execute_with(
    input: &LogicalPlan,
    items: &[ReturnItem],
    distinct: bool,
    where_predicate: Option<&Expression>,
    storage: &mut GraphStorage,
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
            let key = columns
                .iter()
                .map(|c| cypher_value_to_stable_key(rec.get(c).unwrap_or(&CypherValue::Null)))
                .collect::<Vec<_>>()
                .join("\x00");
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

fn execute_unwind(
    input: &LogicalPlan,
    expression: &Expression,
    alias: &str,
    storage: &mut GraphStorage,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage, params)?;

    if !cols.contains(&alias.to_string()) {
        cols.push(alias.to_string());
    }

    let mut result_records = Vec::new();

    for rec in &input_records {
        let list_val = eval_with_params(expression, rec, params, storage)?;
        match list_val {
            CypherValue::List(items) => {
                for item in items {
                    let mut new_rec = rec.clone();
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

fn execute_set(
    input: &LogicalPlan,
    items: &[SetItem],
    storage: &mut GraphStorage,
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

fn apply_set_item(
    item: &SetItem,
    rec: &mut Record,
    storage: &mut GraphStorage,
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
                        if let Some(updated) = storage.get_node(nid) {
                            rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                        }
                    }
                    Some(CypherValue::Relationship(e)) => {
                        let eid = e.id;
                        storage.remove_edge_property(eid, property);
                        if let Some(updated) = storage.get_edge(eid) {
                            rec.insert(
                                variable.clone(),
                                CypherValue::Relationship(updated.clone()),
                            );
                        }
                    }
                    _ => {}
                }
            } else {
                let prop_val = cypher_value_to_property(&val)?;
                match rec.get(variable) {
                    Some(CypherValue::Node(n)) => {
                        let nid = n.id;
                        storage.set_node_property(nid, property.clone(), prop_val);
                        // Update the record's node copy
                        if let Some(updated) = storage.get_node(nid) {
                            rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                        }
                    }
                    Some(CypherValue::Relationship(e)) => {
                        let eid = e.id;
                        storage.set_edge_property(eid, property.clone(), prop_val);
                        if let Some(updated) = storage.get_edge(eid) {
                            rec.insert(
                                variable.clone(),
                                CypherValue::Relationship(updated.clone()),
                            );
                        }
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
                    storage.set_node_all_properties(n.id, props);
                    if let Some(updated) = storage.get_node(n.id) {
                        rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                    }
                }
                Some(CypherValue::Relationship(e)) => {
                    storage.set_edge_all_properties(e.id, props);
                    if let Some(updated) = storage.get_edge(e.id) {
                        rec.insert(variable.clone(), CypherValue::Relationship(updated.clone()));
                    }
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
                    storage.merge_node_properties(n.id, props);
                    if let Some(updated) = storage.get_node(n.id) {
                        rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                    }
                }
                Some(CypherValue::Relationship(e)) => {
                    storage.merge_edge_properties(e.id, props);
                    if let Some(updated) = storage.get_edge(e.id) {
                        rec.insert(variable.clone(), CypherValue::Relationship(updated.clone()));
                    }
                }
                _ => {}
            }
        }
        SetItem::Label { variable, labels } => {
            if let Some(CypherValue::Node(n)) = rec.get(variable) {
                storage.add_node_labels(n.id, labels);
                if let Some(updated) = storage.get_node(n.id) {
                    rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                }
            }
        }
    }
    Ok(())
}

fn execute_remove(
    input: &LogicalPlan,
    items: &[RemoveItem],
    storage: &mut GraphStorage,
    params: &Parameters,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, mut records) = execute_to_records(input, storage, params)?;

    for rec in &mut records {
        for item in items {
            match item {
                RemoveItem::Property { variable, property } => match rec.get(variable) {
                    Some(CypherValue::Node(n)) => {
                        storage.remove_node_property(n.id, property);
                        if let Some(updated) = storage.get_node(n.id) {
                            rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                        }
                    }
                    Some(CypherValue::Relationship(e)) => {
                        storage.remove_edge_property(e.id, property);
                        if let Some(updated) = storage.get_edge(e.id) {
                            rec.insert(
                                variable.clone(),
                                CypherValue::Relationship(updated.clone()),
                            );
                        }
                    }
                    _ => {}
                },
                RemoveItem::Label { variable, labels } => {
                    if let Some(CypherValue::Node(n)) = rec.get(variable) {
                        storage.remove_node_labels(n.id, labels);
                        if let Some(updated) = storage.get_node(n.id) {
                            rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                        }
                    }
                }
            }
        }
    }

    Ok((cols, records))
}

fn execute_delete(
    input: &LogicalPlan,
    expressions: &[Expression],
    detach: bool,
    storage: &mut GraphStorage,
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

fn execute_merge(
    input: &LogicalPlan,
    pattern: &PatternElement,
    on_create: Option<&[SetItem]>,
    on_match: Option<&[SetItem]>,
    storage: &mut GraphStorage,
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
            let properties = resolve_map_literal_to_properties(&np.properties, params)?;

            // Find all existing nodes matching labels and properties.
            // MERGE runs ON MATCH for every matching node (not just the first),
            // and ON CREATE only when no match exists at all.
            let existing_ids = storage.find_nodes(&labels, &properties);

            let base_records = if input_records.is_empty() {
                vec![Record::new()]
            } else {
                input_records
            };

            let mut result_records = Vec::new();

            if existing_ids.is_empty() {
                // Not found — create one node and apply ON CREATE SET
                let node_id = storage.create_node(labels, properties);
                let node = storage
                    .get_node(node_id)
                    .ok_or_else(|| {
                        CypherError::RuntimeError("Newly created node not found".to_string())
                    })?
                    .clone();
                for mut rec in base_records {
                    rec.insert(variable.clone(), CypherValue::Node(node.clone()));
                    result_records.push(rec);
                }
                if let Some(items) = on_create {
                    for rec in &mut result_records {
                        for item in items {
                            apply_set_item(item, rec, storage, params)?;
                        }
                    }
                }
            } else {
                // Found — produce one output row per matched node and apply ON MATCH SET
                for node_id in existing_ids {
                    let node = storage
                        .get_node(node_id)
                        .ok_or_else(|| {
                            CypherError::RuntimeError("Merged node not found".to_string())
                        })?
                        .clone();
                    for base_rec in &base_records {
                        let mut rec = base_rec.clone();
                        rec.insert(variable.clone(), CypherValue::Node(node.clone()));
                        result_records.push(rec);
                    }
                }
                if let Some(items) = on_match {
                    for rec in &mut result_records {
                        for item in items {
                            apply_set_item(item, rec, storage, params)?;
                        }
                    }
                }
            }

            Ok((cols, result_records))
        }
        PatternElement::Chain { .. } => {
            // MERGE on path patterns requires matching/creating the entire path atomically,
            // which is not yet implemented. Return a clear error rather than silently
            // producing wrong results.
            Err(CypherError::NotImplemented(
                "MERGE with relationship chain patterns is not yet supported; use MERGE on nodes only".to_string(),
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_var_length_expand(
    input: &LogicalPlan,
    src_variable: &str,
    rel_variable: Option<&str>,
    dst_variable: &str,
    rel_types: &[String],
    direction: &Direction,
    min_hops: u64,
    max_hops: Option<u64>,
    storage: &mut GraphStorage,
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

    // Hard ceiling to prevent runaway BFS on unbounded patterns (`-[*]->`).
    // Bounded patterns (`-[*..N]->`) use the user-specified max directly.
    const BFS_DEFAULT_MAX_HOPS: u64 = 100;
    let effective_max = max_hops.unwrap_or(BFS_DEFAULT_MAX_HOPS);
    let mut result_records = Vec::new();

    for rec in &input_records {
        let start_id = match rec.get(src_variable) {
            Some(CypherValue::Node(n)) => n.id,
            _ => continue,
        };

        // BFS state: (current_node_id, path_of_edges_taken)
        // We avoid revisiting the same edge within a single path to prevent infinite loops.
        let mut queue: std::collections::VecDeque<(NodeId, Vec<Edge>)> =
            std::collections::VecDeque::new();
        queue.push_back((start_id, Vec::new()));

        while let Some((cur_id, path_edges)) = queue.pop_front() {
            let depth = path_edges.len() as u64;

            if depth >= effective_max {
                continue;
            }

            let next_edges: Vec<Edge> = match direction {
                Direction::Outgoing => storage
                    .outgoing_edges(cur_id)
                    .into_iter()
                    .cloned()
                    .collect(),
                Direction::Incoming => storage
                    .incoming_edges(cur_id)
                    .into_iter()
                    .cloned()
                    .collect(),
                Direction::Undirected => {
                    let mut all: Vec<Edge> = storage
                        .outgoing_edges(cur_id)
                        .into_iter()
                        .cloned()
                        .collect();
                    all.extend(storage.incoming_edges(cur_id).into_iter().cloned());
                    all
                }
            };

            for edge in next_edges {
                // Skip edges already in the path (no repeated relationships)
                if path_edges.iter().any(|pe| pe.id == edge.id) {
                    continue;
                }

                // Apply relationship type filter
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

                let mut new_path = path_edges.clone();
                new_path.push(edge.clone());
                let new_depth = new_path.len() as u64;

                // Emit a result row when depth is within [min_hops, max_hops]
                if new_depth >= min_hops
                    && let Some(dst_node) = storage.get_node(next_id)
                {
                    let mut new_rec = rec.clone();
                    if let Some(rv) = rel_variable {
                        // Bind the relationship variable to the list of traversed edges
                        new_rec.insert(
                            rv.to_string(),
                            CypherValue::List(
                                new_path
                                    .iter()
                                    .map(|e| CypherValue::Relationship(e.clone()))
                                    .collect(),
                            ),
                        );
                    }
                    new_rec.insert(
                        dst_variable.to_string(),
                        CypherValue::Node(dst_node.clone()),
                    );
                    result_records.push(new_rec);
                }

                // Continue BFS if we haven't hit the maximum depth
                if new_depth < effective_max {
                    queue.push_back((next_id, new_path));
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

fn resolve_map_literal_to_properties(
    map_lit: &Option<MapLiteral>,
    params: &Parameters,
) -> Result<HashMap<String, PropertyValue>, CypherError> {
    let mut properties = HashMap::new();
    if let Some(map) = map_lit {
        for (key, expr) in &map.entries {
            let value = expr_to_property_value(expr, params)?;
            properties.insert(key.clone(), value);
        }
    }
    Ok(properties)
}

fn expr_to_property_value(
    expr: &Expression,
    params: &Parameters,
) -> Result<PropertyValue, CypherError> {
    match expr {
        Expression::Literal(Literal::String(s)) => Ok(PropertyValue::String(s.clone())),
        Expression::Literal(Literal::Integer(i)) => Ok(PropertyValue::Int(*i)),
        Expression::Literal(Literal::Float(f)) => Ok(PropertyValue::Float(*f)),
        Expression::Literal(Literal::Boolean(b)) => Ok(PropertyValue::Bool(*b)),
        Expression::Literal(Literal::Null) => Err(CypherError::TypeError(
            "Property value cannot be null".to_string(),
        )),
        // Handle negative literals: -(integer) or -(float)
        Expression::UnaryOp {
            op: UnaryOp::Neg,
            operand,
        } => match operand.as_ref() {
            Expression::Literal(Literal::Integer(i)) => Ok(PropertyValue::Int(-i)),
            Expression::Literal(Literal::Float(f)) => Ok(PropertyValue::Float(-f)),
            _ => Err(CypherError::NotImplemented(
                "Complex expressions in property values".to_string(),
            )),
        },
        // Support $param references in inline property maps
        Expression::Parameter(name) => match params.get(name) {
            Some(CypherValue::String(s)) => Ok(PropertyValue::String(s.clone())),
            Some(CypherValue::Integer(i)) => Ok(PropertyValue::Int(*i)),
            Some(CypherValue::Float(f)) => Ok(PropertyValue::Float(*f)),
            Some(CypherValue::Boolean(b)) => Ok(PropertyValue::Bool(*b)),
            Some(_) => Err(CypherError::TypeError(format!(
                "Parameter ${} must be a scalar value (String, Integer, Float, or Boolean)",
                name
            ))),
            None => Err(CypherError::RuntimeError(format!(
                "Parameter ${} is not defined",
                name
            ))),
        },
        _ => Err(CypherError::NotImplemented(
            "Complex expressions in property values".to_string(),
        )),
    }
}
