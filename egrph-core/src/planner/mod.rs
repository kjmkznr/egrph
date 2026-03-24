pub mod plan;

use crate::ast::*;
use crate::error::CypherError;
use self::plan::LogicalPlan;

pub fn plan(stmt: &Statement) -> Result<LogicalPlan, CypherError> {
    match stmt {
        Statement::Query(query) => plan_query(query),
    }
}

fn plan_query(query: &Query) -> Result<LogicalPlan, CypherError> {
    let mut current_plan = LogicalPlan::EmptyRow;

    for clause in &query.clauses {
        current_plan = plan_clause(clause, current_plan)?;
    }

    Ok(current_plan)
}

fn plan_clause(clause: &Clause, input: LogicalPlan) -> Result<LogicalPlan, CypherError> {
    match clause {
        Clause::Create(create) => plan_create(create, input),
        Clause::Match(match_clause) => plan_match(match_clause, input),
        Clause::Return(return_clause) => plan_return(return_clause, input),
        Clause::Where(where_clause) => plan_where(where_clause, input),
        Clause::With(with_clause) => plan_with(with_clause, input),
        Clause::Unwind(unwind_clause) => plan_unwind(unwind_clause, input),
        Clause::Set(set_clause) => plan_set(set_clause, input),
        Clause::Remove(remove_clause) => plan_remove(remove_clause, input),
        Clause::Delete(delete_clause) => plan_delete(delete_clause, input),
        Clause::Merge(merge_clause) => plan_merge(merge_clause, input),
    }
}

fn plan_create(create: &CreateClause, input: LogicalPlan) -> Result<LogicalPlan, CypherError> {
    if create.pattern.parts.is_empty() {
        return Err(CypherError::SemanticError("Empty CREATE pattern".to_string()));
    }

    // Chain all pattern parts sequentially; each CREATE threads the previous plan as input
    // so that earlier bindings remain visible to later clauses.
    let mut current = input;
    for part in &create.pattern.parts {
        match &part.element {
            PatternElement::Node(node_pattern) => {
                current = LogicalPlan::CreateNode {
                    input: Box::new(current),
                    pattern: node_pattern.clone(),
                };
            }
            PatternElement::Chain { start, elements } => {
                current = LogicalPlan::CreatePath {
                    input: Box::new(current),
                    start: start.clone(),
                    elements: elements.clone(),
                };
            }
        }
    }

    Ok(current)
}

fn plan_match(match_clause: &MatchClause, input: LogicalPlan) -> Result<LogicalPlan, CypherError> {
    if match_clause.pattern.parts.is_empty() {
        return Err(CypherError::SemanticError("Empty MATCH pattern".to_string()));
    }

    // Each comma-separated pattern part becomes a scan that is cross-joined with
    // the accumulated plan so that all bindings remain visible.
    let mut current = input;
    for part in &match_clause.pattern.parts {
        let scan_plan = plan_match_scan(part)?;
        current = match current {
            LogicalPlan::EmptyRow => scan_plan,
            other => LogicalPlan::CartesianProduct {
                left: Box::new(other),
                right: Box::new(scan_plan),
            },
        };
    }

    Ok(current)
}

/// Build the scan/expand plan for a single pattern part, without threading prior input.
fn plan_match_scan(part: &crate::ast::PatternPart) -> Result<LogicalPlan, CypherError> {
    match &part.element {
        PatternElement::Node(node_pattern) => {
            let label_filter = node_pattern.labels.first().cloned();
            let variable = node_pattern
                .variable
                .clone()
                .or_else(|| part.variable.clone())
                .unwrap_or_else(|| "_anon".to_string());

            let mut current = LogicalPlan::ScanNodes {
                label_filter,
                variable: variable.clone(),
            };

            // Add property filters if inline properties are specified
            current = add_property_filters(current, &variable, &node_pattern.properties);

            Ok(current)
        }
        PatternElement::Chain { start, elements } => {
            // Start with a node scan for the start node
            let start_label = start.labels.first().cloned();
            let start_var = start
                .variable
                .clone()
                .or_else(|| part.variable.clone())
                .unwrap_or_else(|| "_anon_start".to_string());

            let mut current = LogicalPlan::ScanNodes {
                label_filter: start_label,
                variable: start_var.clone(),
            };

            // Add property filters for the start node
            current = add_property_filters(current, &start_var, &start.properties);

            // Chain expansions for each relationship + target node
            for (i, chain_elem) in elements.iter().enumerate() {
                let rel_var = chain_elem.relationship.variable.clone();
                let dst_var = chain_elem
                    .node
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("_anon_dst_{}", i));

                let src_var = if i == 0 {
                    start_var.clone()
                } else {
                    elements[i - 1]
                        .node
                        .variable
                        .clone()
                        .unwrap_or_else(|| format!("_anon_dst_{}", i - 1))
                };

                current = LogicalPlan::Expand {
                    input: Box::new(current),
                    src_variable: src_var,
                    rel_variable: rel_var,
                    dst_variable: dst_var.clone(),
                    rel_types: chain_elem.relationship.rel_types.clone(),
                    direction: chain_elem.relationship.direction.clone(),
                };

                // If the target node has label filters, add a filter for each label
                for label in &chain_elem.node.labels {
                    let dst_name = chain_elem
                        .node
                        .variable
                        .clone()
                        .unwrap_or_else(|| format!("_anon_dst_{}", i));
                    current = LogicalPlan::Filter {
                        input: Box::new(current),
                        predicate: Expression::FunctionCall {
                            name: "__has_label".to_string(),
                            distinct: false,
                            args: vec![
                                Expression::Variable(dst_name),
                                Expression::Literal(Literal::String(label.clone())),
                            ],
                        },
                    };
                }

                // Add property filters for the target node
                current = add_property_filters(current, &dst_var, &chain_elem.node.properties);
            }

            Ok(current)
        }
    }
}

/// Add Filter nodes for inline property constraints (e.g. `{name: "Alice"}`).
fn add_property_filters(input: LogicalPlan, variable: &str, properties: &Option<MapLiteral>) -> LogicalPlan {
    let Some(map) = properties else {
        return input;
    };
    let mut current = input;
    for (key, val_expr) in &map.entries {
        current = LogicalPlan::Filter {
            input: Box::new(current),
            predicate: Expression::Comparison {
                left: Box::new(Expression::Property(
                    Box::new(Expression::Variable(variable.to_string())),
                    key.clone(),
                )),
                op: CompOp::Eq,
                right: Box::new(val_expr.clone()),
            },
        };
    }
    current
}

fn plan_where(
    where_clause: &WhereClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    Ok(LogicalPlan::Filter {
        input: Box::new(input),
        predicate: where_clause.expression.clone(),
    })
}

fn plan_return(
    return_clause: &ReturnClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    let mut current = input;

    // Collect the set of aliases introduced by this RETURN clause.
    let aliases: std::collections::HashSet<String> = return_clause
        .items
        .iter()
        .filter_map(|item| item.alias.clone())
        .collect();

    // Determine whether any ORDER BY item references a projected alias.
    // If so, sorting must happen AFTER projection. Otherwise sort before so
    // that ORDER BY can reference non-projected columns (e.g. ORDER BY n.age).
    let sort_after = if let Some(ref order_items) = return_clause.order_by {
        order_items.iter().any(|item| order_item_uses_alias(&item.expression, &aliases))
    } else {
        false
    };

    if !sort_after {
        // Sort BEFORE projection — ORDER BY references pre-projection columns.
        if let Some(ref order_items) = return_clause.order_by {
            current = LogicalPlan::Sort {
                input: Box::new(current),
                items: order_items.clone(),
            };
        }
    }

    // Project (RETURN items)
    current = LogicalPlan::Return {
        input: Box::new(current),
        items: return_clause.items.clone(),
        distinct: return_clause.distinct,
    };

    if sort_after {
        // Sort AFTER projection — ORDER BY references projected aliases.
        if let Some(ref order_items) = return_clause.order_by {
            current = LogicalPlan::Sort {
                input: Box::new(current),
                items: order_items.clone(),
            };
        }
    }

    // Skip and Limit after sort/projection
    if let Some(ref skip_expr) = return_clause.skip {
        current = LogicalPlan::Skip {
            input: Box::new(current),
            count: skip_expr.clone(),
        };
    }

    if let Some(ref limit_expr) = return_clause.limit {
        current = LogicalPlan::Limit {
            input: Box::new(current),
            count: limit_expr.clone(),
        };
    }

    Ok(current)
}

/// Returns true if the sort expression is a simple variable reference that matches
/// one of the projected aliases (meaning it only exists after projection).
fn order_item_uses_alias(expr: &Expression, aliases: &std::collections::HashSet<String>) -> bool {
    match expr {
        Expression::Variable(name) => aliases.contains(name),
        _ => false,
    }
}

fn plan_with(
    with_clause: &WithClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    let mut current = input;

    // WITH projection (with optional WHERE filter) — project FIRST
    current = LogicalPlan::With {
        input: Box::new(current),
        items: with_clause.items.clone(),
        distinct: with_clause.distinct,
        where_predicate: with_clause.where_clause.clone(),
    };

    // Sort AFTER projection (ORDER BY references projected column aliases)
    if let Some(ref order_items) = with_clause.order_by {
        current = LogicalPlan::Sort {
            input: Box::new(current),
            items: order_items.clone(),
        };
    }

    // Skip and Limit after sort
    if let Some(ref skip_expr) = with_clause.skip {
        current = LogicalPlan::Skip {
            input: Box::new(current),
            count: skip_expr.clone(),
        };
    }

    if let Some(ref limit_expr) = with_clause.limit {
        current = LogicalPlan::Limit {
            input: Box::new(current),
            count: limit_expr.clone(),
        };
    }

    Ok(current)
}

fn plan_unwind(
    unwind_clause: &UnwindClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    Ok(LogicalPlan::Unwind {
        input: Box::new(input),
        expression: unwind_clause.expression.clone(),
        alias: unwind_clause.alias.clone(),
    })
}

fn plan_set(
    set_clause: &SetClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    Ok(LogicalPlan::SetOp {
        input: Box::new(input),
        items: set_clause.items.clone(),
    })
}

fn plan_remove(
    remove_clause: &RemoveClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    Ok(LogicalPlan::RemoveOp {
        input: Box::new(input),
        items: remove_clause.items.clone(),
    })
}

fn plan_delete(
    delete_clause: &DeleteClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    Ok(LogicalPlan::DeleteOp {
        input: Box::new(input),
        expressions: delete_clause.expressions.clone(),
        detach: delete_clause.detach,
    })
}

fn plan_merge(
    merge_clause: &MergeClause,
    input: LogicalPlan,
) -> Result<LogicalPlan, CypherError> {
    if let Some(part) = merge_clause.pattern.parts.first() {
        Ok(LogicalPlan::MergeOp {
            input: Box::new(input),
            pattern: part.element.clone(),
            on_create: merge_clause.on_create.clone(),
            on_match: merge_clause.on_match.clone(),
        })
    } else {
        Err(CypherError::SemanticError("Empty MERGE pattern".to_string()))
    }
}
