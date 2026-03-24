pub mod result;
pub mod expression;
pub mod aggregation;

use std::collections::HashMap;
use crate::ast::*;
use crate::error::CypherError;
use crate::graph::storage::GraphStorage;
use crate::graph::types::*;
use crate::planner::plan::LogicalPlan;
use self::expression::{Record, eval, is_truthy, compare_values, cypher_value_to_stable_key};
use self::result::{QueryResult, ResultRow};
use self::aggregation::{items_contain_aggregation, execute_aggregation};

pub fn execute(
    plan: &LogicalPlan,
    storage: &mut GraphStorage,
) -> Result<QueryResult, CypherError> {
    let (cols, records) = execute_to_records(plan, storage)?;
    Ok(records_to_query_result(cols, records))
}

/// Execute a plan and return (columns, records) where each record is a HashMap.
fn execute_to_records(
    plan: &LogicalPlan,
    storage: &mut GraphStorage,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    match plan {
        LogicalPlan::EmptyRow => {
            Ok((Vec::new(), vec![Record::new()]))
        }

        LogicalPlan::CreateNode { input, pattern } => {
            execute_create_node(input, pattern, storage)
        }

        LogicalPlan::CreatePath { input, start, elements } => {
            execute_create_path(input, start, elements, storage)
        }

        LogicalPlan::ScanNodes { label_filter, variable } => {
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
            input, src_variable, rel_variable, dst_variable, rel_types, direction,
        } => {
            let (mut cols, input_records) = execute_to_records(input, storage)?;

            if let Some(rv) = rel_variable {
                if !cols.contains(rv) {
                    cols.push(rv.clone());
                }
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
                    if !rel_types.is_empty()
                        && !rel_types.iter().any(|rt| rt == &edge.label)
                    {
                        continue;
                    }

                    let dst_id = match direction {
                        Direction::Outgoing => edge.dst,
                        Direction::Incoming => edge.src,
                        Direction::Undirected => {
                            if edge.src == src_node_id { edge.dst } else { edge.src }
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
            let (cols, records) = execute_to_records(input, storage)?;
            let filtered: Vec<Record> = records
                .into_iter()
                .filter(|rec| is_truthy(&eval(predicate, rec)))
                .collect();
            Ok((cols, filtered))
        }

        LogicalPlan::Return { input, items, distinct } => {
            let (_input_cols, input_records) = execute_to_records(input, storage)?;

            let columns: Vec<String> = items
                .iter()
                .map(|item| {
                    item.alias.clone().unwrap_or_else(|| expr_to_column_name(&item.expression))
                })
                .collect();

            let mut rows: Vec<Record> = if items_contain_aggregation(items) {
                execute_aggregation(items, &input_records, &columns)
            } else {
                input_records
                    .iter()
                    .map(|rec| {
                        let mut new_rec = Record::new();
                        for (i, item) in items.iter().enumerate() {
                            let val = eval(&item.expression, rec);
                            new_rec.insert(columns[i].clone(), val);
                        }
                        new_rec
                    })
                    .collect()
            };

            if *distinct {
                let mut seen = std::collections::HashSet::new();
                rows.retain(|rec| {
                    let key = columns.iter().map(|c| {
                        cypher_value_to_stable_key(rec.get(c).unwrap_or(&CypherValue::Null))
                    }).collect::<Vec<_>>().join(",");
                    seen.insert(key)
                });
            }

            Ok((columns, rows))
        }

        LogicalPlan::Sort { input, items } => {
            let (cols, mut records) = execute_to_records(input, storage)?;
            records.sort_by(|a, b| {
                for item in items {
                    let va = eval(&item.expression, a);
                    let vb = eval(&item.expression, b);
                    let ord = compare_values(&va, &vb).unwrap_or(std::cmp::Ordering::Equal);
                    let ord = if item.ascending { ord } else { ord.reverse() };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
                std::cmp::Ordering::Equal
            });
            Ok((cols, records))
        }

        LogicalPlan::Skip { input, count } => {
            let (cols, records) = execute_to_records(input, storage)?;
            // SKIP/LIMIT count expressions must be parameter-only or literals;
            // evaluate them against an empty record as per openCypher spec.
            let n = match eval(count, &Record::new()) {
                CypherValue::Integer(i) => i.max(0) as usize,
                _ => 0,
            };
            let skipped: Vec<Record> = records.into_iter().skip(n).collect();
            Ok((cols, skipped))
        }

        LogicalPlan::Limit { input, count } => {
            let (cols, records) = execute_to_records(input, storage)?;
            let n = match eval(count, &Record::new()) {
                CypherValue::Integer(i) => i.max(0) as usize,
                _ => records.len(),
            };
            let limited: Vec<Record> = records.into_iter().take(n).collect();
            Ok((cols, limited))
        }

        LogicalPlan::With { input, items, distinct, where_predicate } => {
            execute_with(input, items, *distinct, where_predicate.as_ref(), storage)
        }

        LogicalPlan::Unwind { input, expression, alias } => {
            execute_unwind(input, expression, alias, storage)
        }

        LogicalPlan::SetOp { input, items } => {
            execute_set(input, items, storage)
        }

        LogicalPlan::RemoveOp { input, items } => {
            execute_remove(input, items, storage)
        }

        LogicalPlan::DeleteOp { input, expressions, detach } => {
            execute_delete(input, expressions, *detach, storage)
        }

        LogicalPlan::MergeOp { input, pattern, on_create, on_match } => {
            execute_merge(input, pattern, on_create.as_deref(), on_match.as_deref(), storage)
        }

        LogicalPlan::CartesianProduct { left, right } => {
            let (left_cols, left_records) = execute_to_records(left, storage)?;
            let (right_cols, right_records) = execute_to_records(right, storage)?;

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
    }
}

// --- Concrete executors ---

fn execute_create_node(
    input: &LogicalPlan,
    pattern: &NodePattern,
    storage: &mut GraphStorage,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage)?;

    // Bind the created node variable if provided
    let var = pattern.variable.clone();
    if let Some(ref v) = var {
        if !cols.contains(v) {
            cols.push(v.clone());
        }
    }

    // For each input row, create one node and augment the record
    let base_records = if input_records.is_empty() { vec![Record::new()] } else { input_records };
    let mut result = Vec::with_capacity(base_records.len());
    for mut rec in base_records {
        let labels = pattern.labels.clone();
        let properties = resolve_map_literal_to_properties(&pattern.properties)?;
        let id = storage.create_node(labels, properties);
        if let Some(ref v) = var {
            let node = storage.get_node(id).unwrap().clone();
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
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage)?;

    // Collect all variable names we will bind
    let start_var = start.variable.clone();
    if let Some(ref v) = start_var {
        if !cols.contains(v) { cols.push(v.clone()); }
    }
    for elem in elements {
        if let Some(ref rv) = elem.relationship.variable {
            if !cols.contains(rv) { cols.push(rv.clone()); }
        }
        if let Some(ref dv) = elem.node.variable {
            if !cols.contains(dv) { cols.push(dv.clone()); }
        }
    }

    let from_empty_input = matches!(input, LogicalPlan::EmptyRow);
    let base_records = if input_records.is_empty() { vec![Record::new()] } else { input_records };
    let mut result = Vec::with_capacity(base_records.len());

    for mut rec in base_records {
        let start_labels = start.labels.clone();
        let start_props = resolve_map_literal_to_properties(&start.properties)?;
        let start_id = storage.create_node(start_labels, start_props);
        if let Some(ref v) = start_var {
            let node = storage.get_node(start_id).unwrap().clone();
            rec.insert(v.clone(), CypherValue::Node(node));
        }

        let mut prev_id = start_id;
        for elem in elements {
            let dst_labels = elem.node.labels.clone();
            let dst_props = resolve_map_literal_to_properties(&elem.node.properties)?;
            let dst_id = storage.create_node(dst_labels, dst_props);

            let edge_label = elem.relationship.rel_types.first().cloned().unwrap_or_default();
            let edge_props = resolve_map_literal_to_properties(&elem.relationship.properties)?;

            let (src, dst) = match elem.relationship.direction {
                Direction::Incoming => (dst_id, prev_id),
                _ => (prev_id, dst_id),
            };

            let eid = storage.create_edge(edge_label, src, dst, edge_props)
                .map_err(CypherError::RuntimeError)?;

            if let Some(ref rv) = elem.relationship.variable {
                let edge = storage.get_edge(eid).unwrap().clone();
                rec.insert(rv.clone(), CypherValue::Relationship(edge));
            }
            if let Some(ref dv) = elem.node.variable {
                let node = storage.get_node(dst_id).unwrap().clone();
                rec.insert(dv.clone(), CypherValue::Node(node));
            }

            prev_id = dst_id;
        }

        // When started from an empty input (no prior pipeline), emit one row per
        // node in the path so that callers without a RETURN clause can observe
        // the created elements.
        if from_empty_input {
            let node_count = 1 + elements.len(); // start node + one per chain element
            for _ in 0..node_count {
                result.push(rec.clone());
            }
        } else {
            result.push(rec);
        }
    }

    Ok((cols, result))
}

fn execute_with(
    input: &LogicalPlan,
    items: &[ReturnItem],
    distinct: bool,
    where_predicate: Option<&Expression>,
    storage: &mut GraphStorage,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (_input_cols, input_records) = execute_to_records(input, storage)?;

    let columns: Vec<String> = items
        .iter()
        .map(|item| {
            item.alias.clone().unwrap_or_else(|| expr_to_column_name(&item.expression))
        })
        .collect();

    let mut rows: Vec<Record> = if items_contain_aggregation(items) {
        execute_aggregation(items, &input_records, &columns)
    } else {
        input_records
            .iter()
            .map(|rec| {
                let mut new_rec = Record::new();
                for (i, item) in items.iter().enumerate() {
                    let val = eval(&item.expression, rec);
                    new_rec.insert(columns[i].clone(), val);
                }
                new_rec
            })
            .collect()
    };

    if distinct {
        let mut seen = std::collections::HashSet::new();
        rows.retain(|rec| {
            let key = columns.iter().map(|c| {
                cypher_value_to_stable_key(rec.get(c).unwrap_or(&CypherValue::Null))
            }).collect::<Vec<_>>().join(",");
            seen.insert(key)
        });
    }

    // Apply WHERE predicate if present
    if let Some(predicate) = where_predicate {
        rows.retain(|rec| is_truthy(&eval(predicate, rec)));
    }

    Ok((columns, rows))
}

fn execute_unwind(
    input: &LogicalPlan,
    expression: &Expression,
    alias: &str,
    storage: &mut GraphStorage,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage)?;

    if !cols.contains(&alias.to_string()) {
        cols.push(alias.to_string());
    }

    let mut result_records = Vec::new();

    for rec in &input_records {
        let list_val = eval(expression, rec);
        match list_val {
            CypherValue::List(items) => {
                for item in items {
                    let mut new_rec = rec.clone();
                    new_rec.insert(alias.to_string(), item);
                    result_records.push(new_rec);
                }
            }
            CypherValue::Null => {
                // UNWIND null produces no rows
            }
            _ => {
                // UNWIND on a non-list value: treat as single-element
                let mut new_rec = rec.clone();
                new_rec.insert(alias.to_string(), list_val);
                result_records.push(new_rec);
            }
        }
    }

    Ok((cols, result_records))
}

fn execute_set(
    input: &LogicalPlan,
    items: &[SetItem],
    storage: &mut GraphStorage,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, mut records) = execute_to_records(input, storage)?;

    for rec in &mut records {
        for item in items {
            apply_set_item(item, rec, storage)?;
        }
    }

    Ok((cols, records))
}

fn apply_set_item(
    item: &SetItem,
    rec: &mut Record,
    storage: &mut GraphStorage,
) -> Result<(), CypherError> {
    match item {
        SetItem::Property { variable, property, expression } => {
            let val = eval(expression, rec);
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
                            rec.insert(variable.clone(), CypherValue::Relationship(updated.clone()));
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
                            rec.insert(variable.clone(), CypherValue::Relationship(updated.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
        SetItem::AllProperties { variable, expression } => {
            let val = eval(expression, rec);
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
        SetItem::MergeProperties { variable, expression } => {
            let val = eval(expression, rec);
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
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, mut records) = execute_to_records(input, storage)?;

    for rec in &mut records {
        for item in items {
            match item {
                RemoveItem::Property { variable, property } => {
                    match rec.get(variable) {
                        Some(CypherValue::Node(n)) => {
                            storage.remove_node_property(n.id, property);
                            if let Some(updated) = storage.get_node(n.id) {
                                rec.insert(variable.clone(), CypherValue::Node(updated.clone()));
                            }
                        }
                        Some(CypherValue::Relationship(e)) => {
                            storage.remove_edge_property(e.id, property);
                            if let Some(updated) = storage.get_edge(e.id) {
                                rec.insert(variable.clone(), CypherValue::Relationship(updated.clone()));
                            }
                        }
                        _ => {}
                    }
                }
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
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (cols, records) = execute_to_records(input, storage)?;

    // Collect all entities to delete first, using sets for O(1) deduplication.
    let mut node_ids: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    let mut edge_ids: std::collections::HashSet<EdgeId> = std::collections::HashSet::new();

    for rec in &records {
        for expr in expressions {
            let val = eval(expr, rec);
            match val {
                CypherValue::Node(n) => { node_ids.insert(n.id); }
                CypherValue::Relationship(e) => { edge_ids.insert(e.id); }
                _ => {}
            }
        }
    }

    // Delete edges first, then nodes
    for eid in &edge_ids {
        storage.delete_edge(*eid)
            .map_err(CypherError::RuntimeError)?;
    }

    for nid in &node_ids {
        storage.delete_node(*nid, detach)
            .map_err(CypherError::RuntimeError)?;
    }

    // Return empty records — deleted rows are no longer part of the pipeline.
    Ok((cols, Vec::new()))
}

fn execute_merge(
    input: &LogicalPlan,
    pattern: &PatternElement,
    on_create: Option<&[SetItem]>,
    on_match: Option<&[SetItem]>,
    storage: &mut GraphStorage,
) -> Result<(Vec<String>, Vec<Record>), CypherError> {
    let (mut cols, input_records) = execute_to_records(input, storage)?;

    match pattern {
        PatternElement::Node(np) => {
            let variable = np.variable.clone().unwrap_or_else(|| "_merge_node".to_string());
            if !cols.contains(&variable) {
                cols.push(variable.clone());
            }

            let labels = np.labels.clone();
            let properties = resolve_map_literal_to_properties(&np.properties)?;

            // Try to find existing node
            let existing = storage.find_node(&labels, &properties);

            let mut result_records = if input_records.is_empty() {
                vec![Record::new()]
            } else {
                input_records
            };

            match existing {
                Some(node_id) => {
                    // Found - apply ON MATCH SET if present
                    let node = storage.get_node(node_id).unwrap().clone();
                    for rec in &mut result_records {
                        rec.insert(variable.clone(), CypherValue::Node(node.clone()));
                    }
                    if let Some(items) = on_match {
                        for rec in &mut result_records {
                            for item in items {
                                apply_set_item(item, rec, storage)?;
                            }
                        }
                    }
                }
                None => {
                    // Not found - create and apply ON CREATE SET if present
                    let node_id = storage.create_node(labels, properties);
                    let node = storage.get_node(node_id).unwrap().clone();
                    for rec in &mut result_records {
                        rec.insert(variable.clone(), CypherValue::Node(node.clone()));
                    }
                    if let Some(items) = on_create {
                        for rec in &mut result_records {
                            for item in items {
                                apply_set_item(item, rec, storage)?;
                            }
                        }
                    }
                }
            }

            Ok((cols, result_records))
        }
        PatternElement::Chain { start, elements } => {
            // For chain patterns in MERGE, simplified: just create the whole path if not found
            // Full MERGE semantics for paths is complex; here we check start node only
            let variable = start.variable.clone().unwrap_or_else(|| "_merge_start".to_string());
            if !cols.contains(&variable) {
                cols.push(variable.clone());
            }

            let labels = start.labels.clone();
            let properties = resolve_map_literal_to_properties(&start.properties)?;
            let existing = storage.find_node(&labels, &properties);

            let mut result_records = if input_records.is_empty() {
                vec![Record::new()]
            } else {
                input_records
            };

            match existing {
                Some(node_id) => {
                    let node = storage.get_node(node_id).unwrap().clone();
                    for rec in &mut result_records {
                        rec.insert(variable.clone(), CypherValue::Node(node.clone()));
                    }
                    if let Some(items) = on_match {
                        for rec in &mut result_records {
                            for item in items {
                                apply_set_item(item, rec, storage)?;
                            }
                        }
                    }
                }
                None => {
                    // Create the full path
                    let start_id = storage.create_node(labels, properties);
                    let start_node = storage.get_node(start_id).unwrap().clone();

                    let mut prev_id = start_id;
                    for elem in elements {
                        let dst_labels = elem.node.labels.clone();
                        let dst_props = resolve_map_literal_to_properties(&elem.node.properties)?;
                        let dst_id = storage.create_node(dst_labels, dst_props);

                        let edge_label = elem.relationship.rel_types.first().cloned().unwrap_or_default();
                        let edge_props = resolve_map_literal_to_properties(&elem.relationship.properties)?;

                        let (src, dst) = match elem.relationship.direction {
                            Direction::Incoming => (dst_id, prev_id),
                            _ => (prev_id, dst_id),
                        };

                        storage.create_edge(edge_label, src, dst, edge_props)
                            .map_err(CypherError::RuntimeError)?;

                        prev_id = dst_id;
                    }

                    for rec in &mut result_records {
                        rec.insert(variable.clone(), CypherValue::Node(start_node.clone()));
                    }
                    if let Some(items) = on_create {
                        for rec in &mut result_records {
                            for item in items {
                                apply_set_item(item, rec, storage)?;
                            }
                        }
                    }
                }
            }

            Ok((cols, result_records))
        }
    }
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
fn cypher_value_to_property_map(val: &CypherValue) -> Result<HashMap<String, PropertyValue>, CypherError> {
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
) -> Result<HashMap<String, PropertyValue>, CypherError> {
    let mut properties = HashMap::new();
    if let Some(map) = map_lit {
        for (key, expr) in &map.entries {
            let value = expr_to_property_value(expr)?;
            properties.insert(key.clone(), value);
        }
    }
    Ok(properties)
}

fn expr_to_property_value(expr: &Expression) -> Result<PropertyValue, CypherError> {
    match expr {
        Expression::Literal(Literal::String(s)) => Ok(PropertyValue::String(s.clone())),
        Expression::Literal(Literal::Integer(i)) => Ok(PropertyValue::Int(*i)),
        Expression::Literal(Literal::Float(f)) => Ok(PropertyValue::Float(*f)),
        Expression::Literal(Literal::Boolean(b)) => Ok(PropertyValue::Bool(*b)),
        // Handle negative literals: -(integer) or -(float)
        Expression::UnaryOp { op: UnaryOp::Neg, operand } => {
            match operand.as_ref() {
                Expression::Literal(Literal::Integer(i)) => Ok(PropertyValue::Int(-i)),
                Expression::Literal(Literal::Float(f)) => Ok(PropertyValue::Float(-f)),
                _ => Err(CypherError::NotImplemented(
                    "Complex expressions in property values".to_string(),
                ))
            }
        }
        _ => {
            Err(CypherError::NotImplemented(
                "Complex expressions in property values".to_string(),
            ))
        }
    }
}
