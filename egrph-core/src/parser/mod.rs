use pest::Parser;
use pest_derive::Parser;

use crate::ast::*;
use crate::error::CypherError;

#[derive(Parser)]
#[grammar = "parser/cypher.pest"]
pub struct CypherParser;

/// Main parse entry point. Parses a Cypher query string into an AST Statement.
pub fn parse(input: &str) -> Result<Statement, CypherError> {
    let pairs = CypherParser::parse(Rule::statement, input)
        .map_err(|e| CypherError::ParseError(format!("{}", e)))?;

    for pair in pairs {
        if pair.as_rule() == Rule::statement {
            let mut queries: Vec<Query> = Vec::new();
            let mut union_alls: Vec<bool> = Vec::new();

            for inner in pair.into_inner() {
                match inner.as_rule() {
                    Rule::create_constraint_stmt => {
                        return parse_create_constraint_stmt(inner);
                    }
                    Rule::query => queries.push(parse_query(inner)?),
                    Rule::union_op => {
                        let is_all = inner.into_inner().any(|t| t.as_rule() == Rule::all_kw);
                        union_alls.push(is_all);
                    }
                    _ => {}
                }
            }

            if queries.is_empty() {
                return Err(CypherError::ParseError("Empty statement".to_string()));
            }

            // Build left-associative Union tree
            let mut result = Statement::Query(queries.remove(0));
            for (all, query) in union_alls.into_iter().zip(queries.into_iter()) {
                result = Statement::Union {
                    left: Box::new(result),
                    right: Box::new(Statement::Query(query)),
                    all,
                };
            }
            return Ok(result);
        }
    }

    Err(CypherError::ParseError("No statement found".to_string()))
}

fn parse_query(pair: pest::iterators::Pair<Rule>) -> Result<Query, CypherError> {
    let mut clauses = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::clause {
            parse_clause(inner, &mut clauses)?;
        }
    }
    Ok(Query { clauses })
}

/// Backward-compatible alias
pub fn parse_with_return_extraction(input: &str) -> Result<Statement, CypherError> {
    parse(input)
}

fn parse_clause(
    pair: pest::iterators::Pair<Rule>,
    clauses: &mut Vec<Clause>,
) -> Result<(), CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty clause".to_string()))?;
    match inner.as_rule() {
        Rule::create_clause => {
            clauses.push(parse_create_clause(inner)?);
        }
        Rule::match_clause => {
            clauses.push(parse_match_clause(inner)?);
        }
        Rule::return_clause => {
            clauses.push(parse_return_clause(inner)?);
        }
        Rule::where_clause => {
            clauses.push(parse_where_clause(inner)?);
        }
        Rule::with_clause => {
            clauses.push(parse_with_clause(inner)?);
        }
        Rule::unwind_clause => {
            clauses.push(parse_unwind_clause(inner)?);
        }
        Rule::set_clause => {
            clauses.push(parse_set_clause(inner)?);
        }
        Rule::remove_clause => {
            clauses.push(parse_remove_clause(inner)?);
        }
        Rule::delete_clause => {
            clauses.push(parse_delete_clause(inner)?);
        }
        Rule::merge_clause => {
            clauses.push(parse_merge_clause(inner)?);
        }
        _ => {}
    }
    Ok(())
}

fn parse_create_constraint_stmt(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Statement, CypherError> {
    // Grammar: CREATE CONSTRAINT FOR "(" variable labels ")" REQUIRE variable "." IDENTIFIER IS UNIQUE
    let mut inners = pair.into_inner();

    // First variable (pattern variable, e.g. "n")
    let var_pair = inners
        .next()
        .ok_or_else(|| CypherError::ParseError("Expected variable in constraint".to_string()))?;
    let variable = var_pair.as_str().to_string();

    // labels (e.g. ":User") — first IDENTIFIER inside labels rule
    let labels_pair = inners
        .next()
        .ok_or_else(|| CypherError::ParseError("Expected label in constraint".to_string()))?;
    let label = labels_pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Expected label identifier".to_string()))?
        .as_str()
        .to_string();

    // Second variable after REQUIRE (ignored, same as first)
    inners.next(); // skip REQUIRE variable

    // property name (IDENTIFIER after ".")
    let prop_pair = inners
        .next()
        .ok_or_else(|| CypherError::ParseError("Expected property in constraint".to_string()))?;
    let property = prop_pair.as_str().to_string();

    Ok(Statement::CreateConstraint(CreateConstraintStatement {
        variable,
        label,
        property,
        constraint_type: ConstraintType::Unique,
    }))
}

fn parse_create_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut patterns = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::pattern_list {
            patterns = parse_pattern_list(inner)?;
        }
    }
    let pattern = Pattern { parts: patterns };
    Ok(Clause::Create(CreateClause { pattern }))
}

fn parse_match_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty MATCH clause".to_string()))?;
    let (optional, pattern_list_pair) = match inner.as_rule() {
        Rule::optional_match => {
            let pl = inner
                .into_inner()
                .next()
                .ok_or_else(|| CypherError::ParseError("Empty OPTIONAL MATCH".to_string()))?;
            (true, pl)
        }
        Rule::regular_match => {
            let pl = inner
                .into_inner()
                .next()
                .ok_or_else(|| CypherError::ParseError("Empty MATCH".to_string()))?;
            (false, pl)
        }
        _ => {
            return Err(CypherError::ParseError(
                "Expected match variant".to_string(),
            ));
        }
    };

    let parts = parse_pattern_list(pattern_list_pair)?;
    Ok(Clause::Match(MatchClause {
        pattern: Pattern { parts },
        optional,
    }))
}

fn parse_return_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut distinct = false;
    let mut items = Vec::new();
    let mut order_by = None;
    let mut skip = None;
    let mut limit = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::distinct_kw => distinct = true,
            Rule::return_items => {
                items = parse_return_items(inner)?;
            }
            Rule::order_by_clause => {
                order_by = Some(parse_order_by(inner)?);
            }
            Rule::skip_clause => {
                skip = Some(parse_skip_limit_expr(inner)?);
            }
            Rule::limit_clause => {
                limit = Some(parse_skip_limit_expr(inner)?);
            }
            _ => {}
        }
    }

    Ok(Clause::Return(ReturnClause {
        items,
        distinct,
        order_by,
        skip,
        limit,
    }))
}

fn parse_where_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let expr_pair = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty WHERE clause".to_string()))?;
    let expression = parse_expression(expr_pair)?;
    Ok(Clause::Where(WhereClause { expression }))
}

fn parse_with_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut distinct = false;
    let mut items = Vec::new();
    let mut order_by = None;
    let mut skip = None;
    let mut limit = None;
    let mut where_expr = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::distinct_kw => distinct = true,
            Rule::return_items => {
                items = parse_return_items(inner)?;
            }
            Rule::order_by_clause => {
                order_by = Some(parse_order_by(inner)?);
            }
            Rule::skip_clause => {
                skip = Some(parse_skip_limit_expr(inner)?);
            }
            Rule::limit_clause => {
                limit = Some(parse_skip_limit_expr(inner)?);
            }
            Rule::where_clause => {
                if let Some(expr_pair) = inner.into_inner().next() {
                    where_expr = Some(parse_expression(expr_pair)?);
                }
            }
            _ => {}
        }
    }

    Ok(Clause::With(WithClause {
        items,
        distinct,
        order_by,
        skip,
        limit,
        where_clause: where_expr,
    }))
}

fn parse_unwind_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut expression = None;
    let mut alias = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::expression => {
                expression = Some(parse_expression(inner)?);
            }
            Rule::variable => {
                alias = Some(inner.as_str().to_string());
            }
            _ => {}
        }
    }

    Ok(Clause::Unwind(UnwindClause {
        expression: expression
            .ok_or_else(|| CypherError::ParseError("Missing expression in UNWIND".to_string()))?,
        alias: alias
            .ok_or_else(|| CypherError::ParseError("Missing alias in UNWIND".to_string()))?,
    }))
}

fn parse_set_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut items = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::set_item {
            items.push(parse_set_item(inner)?);
        }
    }
    Ok(Clause::Set(SetClause { items }))
}

fn parse_set_item(pair: pest::iterators::Pair<Rule>) -> Result<SetItem, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty SET item".to_string()))?;
    match inner.as_rule() {
        Rule::set_property => {
            let mut variable = String::new();
            let mut property = String::new();
            let mut expression = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::variable => variable = p.as_str().to_string(),
                    Rule::IDENTIFIER => property = p.as_str().to_string(),
                    Rule::expression => expression = Some(parse_expression(p)?),
                    _ => {}
                }
            }
            Ok(SetItem::Property {
                variable,
                property,
                expression: expression.ok_or_else(|| {
                    CypherError::ParseError("Missing expression in SET property".to_string())
                })?,
            })
        }
        Rule::set_merge_properties => {
            let mut variable = String::new();
            let mut expression = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::variable => variable = p.as_str().to_string(),
                    Rule::expression => expression = Some(parse_expression(p)?),
                    _ => {}
                }
            }
            Ok(SetItem::MergeProperties {
                variable,
                expression: expression.ok_or_else(|| {
                    CypherError::ParseError("Missing expression in SET +=".to_string())
                })?,
            })
        }
        Rule::set_all_properties => {
            let mut variable = String::new();
            let mut expression = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::variable => variable = p.as_str().to_string(),
                    Rule::expression => expression = Some(parse_expression(p)?),
                    _ => {}
                }
            }
            Ok(SetItem::AllProperties {
                variable,
                expression: expression.ok_or_else(|| {
                    CypherError::ParseError("Missing expression in SET =".to_string())
                })?,
            })
        }
        Rule::set_label => {
            let mut variable = String::new();
            let mut labels = Vec::new();
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::variable => variable = p.as_str().to_string(),
                    Rule::labels => {
                        for l in p.into_inner() {
                            labels.push(l.as_str().to_string());
                        }
                    }
                    _ => {}
                }
            }
            Ok(SetItem::Label { variable, labels })
        }
        _ => Err(CypherError::ParseError(format!(
            "Unexpected SET item: {:?}",
            inner.as_rule()
        ))),
    }
}

fn parse_remove_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut items = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::remove_item {
            items.push(parse_remove_item(inner)?);
        }
    }
    Ok(Clause::Remove(RemoveClause { items }))
}

fn parse_remove_item(pair: pest::iterators::Pair<Rule>) -> Result<RemoveItem, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty REMOVE item".to_string()))?;
    match inner.as_rule() {
        Rule::remove_property => {
            let mut variable = String::new();
            let mut property = String::new();
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::variable => variable = p.as_str().to_string(),
                    Rule::IDENTIFIER => property = p.as_str().to_string(),
                    _ => {}
                }
            }
            Ok(RemoveItem::Property { variable, property })
        }
        Rule::remove_label => {
            let mut variable = String::new();
            let mut labels = Vec::new();
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::variable => variable = p.as_str().to_string(),
                    Rule::labels => {
                        for l in p.into_inner() {
                            labels.push(l.as_str().to_string());
                        }
                    }
                    _ => {}
                }
            }
            Ok(RemoveItem::Label { variable, labels })
        }
        _ => Err(CypherError::ParseError(format!(
            "Unexpected REMOVE item: {:?}",
            inner.as_rule()
        ))),
    }
}

fn parse_delete_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut detach = false;
    let mut expressions = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::detach_kw => detach = true,
            Rule::expression => {
                expressions.push(parse_expression(inner)?);
            }
            _ => {}
        }
    }

    Ok(Clause::Delete(DeleteClause {
        detach,
        expressions,
    }))
}

fn parse_merge_clause(pair: pest::iterators::Pair<Rule>) -> Result<Clause, CypherError> {
    let mut pattern_element = None;
    let mut on_create = None;
    let mut on_match = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::pattern_element => {
                pattern_element = Some(parse_pattern_element(inner)?);
            }
            Rule::merge_action => {
                let Some(action_inner) = inner.into_inner().next() else {
                    continue;
                };
                match action_inner.as_rule() {
                    Rule::on_create_action => {
                        let mut items = Vec::new();
                        for si in action_inner.into_inner() {
                            if si.as_rule() == Rule::set_item {
                                items.push(parse_set_item(si)?);
                            }
                        }
                        on_create = Some(items);
                    }
                    Rule::on_match_action => {
                        let mut items = Vec::new();
                        for si in action_inner.into_inner() {
                            if si.as_rule() == Rule::set_item {
                                items.push(parse_set_item(si)?);
                            }
                        }
                        on_match = Some(items);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let element = pattern_element
        .ok_or_else(|| CypherError::ParseError("Missing pattern in MERGE".to_string()))?;

    // Wrap in a PatternPart and Pattern
    let variable = match &element {
        PatternElement::Node(np) => np.variable.clone(),
        PatternElement::Chain { start, .. } => start.variable.clone(),
    };

    let pattern = Pattern {
        parts: vec![PatternPart { variable, element }],
    };

    Ok(Clause::Merge(MergeClause {
        pattern,
        on_create,
        on_match,
    }))
}

fn parse_return_items(pair: pest::iterators::Pair<Rule>) -> Result<Vec<ReturnItem>, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty return items".to_string()))?;
    match inner.as_rule() {
        Rule::star_item => Ok(vec![ReturnItem {
            expression: Expression::Variable("*".to_string()),
            alias: None,
        }]),
        Rule::return_item_list => {
            let mut items = Vec::new();
            for item_pair in inner.into_inner() {
                if item_pair.as_rule() == Rule::return_item {
                    items.push(parse_return_item(item_pair)?);
                }
            }
            Ok(items)
        }
        _ => Err(CypherError::ParseError("Expected return items".to_string())),
    }
}

fn parse_return_item(pair: pest::iterators::Pair<Rule>) -> Result<ReturnItem, CypherError> {
    let mut expression = None;
    let mut alias = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::expression => {
                expression = Some(parse_expression(inner)?);
            }
            Rule::variable => {
                alias = Some(inner.as_str().to_string());
            }
            _ => {}
        }
    }

    Ok(ReturnItem {
        expression: expression.ok_or_else(|| {
            CypherError::ParseError("Missing expression in RETURN item".to_string())
        })?,
        alias,
    })
}

fn parse_order_by(pair: pest::iterators::Pair<Rule>) -> Result<Vec<SortItem>, CypherError> {
    let mut items = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::sort_item {
            let mut expr = None;
            let mut ascending = true;
            for si in inner.into_inner() {
                match si.as_rule() {
                    Rule::expression => {
                        expr = Some(parse_expression(si)?);
                    }
                    Rule::sort_direction => {
                        let dir = si.as_str().to_uppercase();
                        ascending = dir == "ASC" || dir == "ASCENDING";
                    }
                    _ => {}
                }
            }
            if let Some(e) = expr {
                items.push(SortItem {
                    expression: e,
                    ascending,
                });
            }
        }
    }
    Ok(items)
}

fn parse_skip_limit_expr(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let expr_pair = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty SKIP/LIMIT expression".to_string()))?;
    parse_expression(expr_pair)
}

// --- Pattern parsing ---

fn parse_pattern_list(pair: pest::iterators::Pair<Rule>) -> Result<Vec<PatternPart>, CypherError> {
    let mut parts = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::pattern {
            parts.push(parse_pattern(inner)?);
        }
    }
    Ok(parts)
}

fn parse_pattern(pair: pest::iterators::Pair<Rule>) -> Result<PatternPart, CypherError> {
    let mut variable = None;
    let mut element = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::variable => {
                variable = Some(inner.as_str().to_string());
            }
            Rule::pattern_element => {
                element = Some(parse_pattern_element(inner)?);
            }
            _ => {}
        }
    }

    let element =
        element.ok_or_else(|| CypherError::ParseError("Missing pattern element".to_string()))?;

    // If the variable is not explicitly set via "p = (...)",
    // use the node's variable as the pattern part variable
    let variable = variable.or_else(|| match &element {
        PatternElement::Node(np) => np.variable.clone(),
        PatternElement::Chain { start, .. } => start.variable.clone(),
    });

    Ok(PatternPart { variable, element })
}

fn parse_pattern_element(pair: pest::iterators::Pair<Rule>) -> Result<PatternElement, CypherError> {
    let mut nodes = Vec::new();
    let mut rels = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::node_pattern => {
                nodes.push(parse_node_pattern(inner)?);
            }
            Rule::relationship_pattern => {
                rels.push(parse_relationship_pattern(inner)?);
            }
            _ => {}
        }
    }

    if rels.is_empty() {
        Ok(PatternElement::Node(nodes.into_iter().next().ok_or_else(
            || CypherError::ParseError("Missing node in pattern element".to_string()),
        )?))
    } else {
        let start = nodes.remove(0);
        let elements: Vec<PatternChainElement> = rels
            .into_iter()
            .zip(nodes)
            .map(|(rel, node)| PatternChainElement {
                relationship: rel,
                node,
            })
            .collect();
        Ok(PatternElement::Chain { start, elements })
    }
}

fn parse_node_pattern(pair: pest::iterators::Pair<Rule>) -> Result<NodePattern, CypherError> {
    let mut variable = None;
    let mut labels = Vec::new();
    let mut properties = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::variable => {
                variable = Some(inner.as_str().to_string());
            }
            Rule::labels => {
                for label in inner.into_inner() {
                    labels.push(label.as_str().to_string());
                }
            }
            Rule::properties => {
                properties = Some(parse_properties(inner)?);
            }
            _ => {}
        }
    }

    Ok(NodePattern {
        variable,
        labels,
        properties,
    })
}

fn parse_relationship_pattern(
    pair: pest::iterators::Pair<Rule>,
) -> Result<RelationshipPattern, CypherError> {
    let mut variable = None;
    let mut rel_types = Vec::new();
    let mut range = None;
    let mut properties = None;
    let mut has_left_arrow = false;
    let mut has_right_arrow = false;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::left_arrow => has_left_arrow = true,
            Rule::right_arrow => has_right_arrow = true,
            Rule::relationship_detail => {
                for detail in inner.into_inner() {
                    match detail.as_rule() {
                        Rule::variable => {
                            variable = Some(detail.as_str().to_string());
                        }
                        Rule::rel_types => {
                            for rt in detail.into_inner() {
                                rel_types.push(rt.as_str().to_string());
                            }
                        }
                        Rule::range_literal => {
                            range = Some(parse_range_literal(detail)?);
                        }
                        Rule::properties => {
                            properties = Some(parse_properties(detail)?);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    let direction = match (has_left_arrow, has_right_arrow) {
        (false, true) => Direction::Outgoing,
        (true, false) => Direction::Incoming,
        _ => Direction::Undirected,
    };

    Ok(RelationshipPattern {
        variable,
        rel_types,
        direction,
        range,
        properties,
    })
}

fn parse_range_literal(pair: pest::iterators::Pair<Rule>) -> Result<RangeSpec, CypherError> {
    let text = pair.as_str().trim();
    let after_star = text.strip_prefix('*').unwrap_or("");
    if after_star.is_empty() {
        return Ok(RangeSpec {
            min: None,
            max: None,
        });
    }

    if let Some((left, right)) = after_star.split_once("..") {
        let min = if left.is_empty() {
            None
        } else {
            Some(
                left.trim()
                    .parse()
                    .map_err(|_| CypherError::ParseError("Invalid range min".to_string()))?,
            )
        };
        let max = if right.is_empty() {
            None
        } else {
            Some(
                right
                    .trim()
                    .parse()
                    .map_err(|_| CypherError::ParseError("Invalid range max".to_string()))?,
            )
        };
        Ok(RangeSpec { min, max })
    } else {
        let n: u64 = after_star
            .trim()
            .parse()
            .map_err(|_| CypherError::ParseError("Invalid range".to_string()))?;
        Ok(RangeSpec {
            min: Some(n),
            max: Some(n),
        })
    }
}

fn parse_properties(pair: pest::iterators::Pair<Rule>) -> Result<MapLiteral, CypherError> {
    let mut entries = Vec::new();
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::property_list {
            for prop in inner.into_inner() {
                if prop.as_rule() == Rule::property_pair {
                    let mut key = String::new();
                    let mut val = None;
                    for p in prop.into_inner() {
                        match p.as_rule() {
                            Rule::property_key => {
                                key = p.as_str().to_string();
                            }
                            Rule::expression => {
                                val = Some(parse_expression(p)?);
                            }
                            _ => {}
                        }
                    }
                    if let Some(v) = val {
                        entries.push((key, v));
                    }
                }
            }
        }
    }
    Ok(MapLiteral { entries })
}

// --- Expression parsing ---

pub fn parse_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty expression".to_string()))?;
    parse_or_expression(inner)
}

fn parse_or_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair
        .into_inner()
        .filter(|p| p.as_rule() != Rule::or_kw)
        .collect();
    if parts.len() == 1 {
        return parse_xor_expression(parts.remove(0));
    }
    let mut expr = parse_xor_expression(parts.remove(0))?;
    for part in parts {
        let right = parse_xor_expression(part)?;
        expr = Expression::Or(Box::new(expr), Box::new(right));
    }
    Ok(expr)
}

fn parse_xor_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair
        .into_inner()
        .filter(|p| p.as_rule() != Rule::xor_kw)
        .collect();
    if parts.len() == 1 {
        return parse_and_expression(parts.remove(0));
    }
    let mut expr = parse_and_expression(parts.remove(0))?;
    for part in parts {
        let right = parse_and_expression(part)?;
        expr = Expression::Xor(Box::new(expr), Box::new(right));
    }
    Ok(expr)
}

fn parse_and_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair
        .into_inner()
        .filter(|p| p.as_rule() != Rule::and_kw)
        .collect();
    if parts.len() == 1 {
        return parse_not_expression(parts.remove(0));
    }
    let mut expr = parse_not_expression(parts.remove(0))?;
    for part in parts {
        let right = parse_not_expression(part)?;
        expr = Expression::And(Box::new(expr), Box::new(right));
    }
    Ok(expr)
}

fn parse_not_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let parts: Vec<pest::iterators::Pair<Rule>> = pair.into_inner().collect();
    let not_count = parts.iter().filter(|p| p.as_rule() == Rule::not_kw).count();
    let comp = parts
        .into_iter()
        .find(|p| p.as_rule() != Rule::not_kw)
        .ok_or_else(|| CypherError::ParseError("Missing operand in NOT expression".to_string()))?;
    let mut expr = parse_comparison_expression(comp)?;
    for _ in 0..not_count {
        expr = Expression::Not(Box::new(expr));
    }
    Ok(expr)
}

fn parse_comparison_expression(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair.into_inner().collect();
    if parts.len() == 1 {
        return parse_string_predicate_expression(parts.remove(0));
    }

    let mut iter = parts.into_iter();
    let mut expr = parse_string_predicate_expression(
        iter.next()
            .ok_or_else(|| CypherError::ParseError("Empty comparison expression".to_string()))?,
    )?;

    while let Some(op_pair) = iter.next() {
        let right_pair = iter
            .next()
            .ok_or_else(|| CypherError::ParseError("Missing right operand".to_string()))?;

        if op_pair.as_str() == "=~" {
            let right = parse_string_predicate_expression(right_pair)?;
            expr = Expression::RegexMatch {
                expr: Box::new(expr),
                pattern: Box::new(right),
            };
        } else {
            let op = match op_pair.as_str() {
                "=" => CompOp::Eq,
                "<>" => CompOp::Neq,
                "<=" => CompOp::Lte,
                ">=" => CompOp::Gte,
                "<" => CompOp::Lt,
                ">" => CompOp::Gt,
                _ => {
                    return Err(CypherError::ParseError(format!(
                        "Unknown comparison op: {}",
                        op_pair.as_str()
                    )));
                }
            };
            let right = parse_string_predicate_expression(right_pair)?;
            expr = Expression::Comparison {
                left: Box::new(expr),
                op,
                right: Box::new(right),
            };
        }
    }
    Ok(expr)
}

fn parse_string_predicate_expression(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Expression, CypherError> {
    let mut inner = pair.into_inner();
    let add_pair = inner
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty string predicate expression".to_string()))?;
    let expr = parse_add_expression(add_pair)?;

    if let Some(suffix_pair) = inner.next() {
        // string_predicate_suffix
        let mut suffix_inner = suffix_pair.into_inner();
        let kw_pair = suffix_inner.next().ok_or_else(|| {
            CypherError::ParseError("Missing keyword in string predicate".to_string())
        })?;
        let rhs_pair = suffix_inner.next().ok_or_else(|| {
            CypherError::ParseError("Missing right-hand side in string predicate".to_string())
        })?;
        let rhs = parse_add_expression(rhs_pair)?;

        match kw_pair.as_rule() {
            Rule::starts_with_kw => Ok(Expression::StringOp {
                left: Box::new(expr),
                op: StringMatchOp::StartsWith,
                right: Box::new(rhs),
            }),
            Rule::ends_with_kw => Ok(Expression::StringOp {
                left: Box::new(expr),
                op: StringMatchOp::EndsWith,
                right: Box::new(rhs),
            }),
            Rule::contains_kw => Ok(Expression::StringOp {
                left: Box::new(expr),
                op: StringMatchOp::Contains,
                right: Box::new(rhs),
            }),
            Rule::in_kw => Ok(Expression::In {
                expr: Box::new(expr),
                list: Box::new(rhs),
            }),
            _ => Err(CypherError::ParseError(format!(
                "Unknown string predicate: {:?}",
                kw_pair.as_rule()
            ))),
        }
    } else {
        Ok(expr)
    }
}

fn parse_add_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair.into_inner().collect();
    if parts.len() == 1 {
        return parse_mult_expression(parts.remove(0));
    }

    let mut iter = parts.into_iter();
    let mut expr = parse_mult_expression(
        iter.next()
            .ok_or_else(|| CypherError::ParseError("Empty add expression".to_string()))?,
    )?;

    while let Some(op_pair) = iter.next() {
        let op = match op_pair.as_str() {
            "+" => BinaryOp::Add,
            "-" => BinaryOp::Sub,
            _ => {
                return Err(CypherError::ParseError(format!(
                    "Unknown add op: {}",
                    op_pair.as_str()
                )));
            }
        };
        let right = parse_mult_expression(
            iter.next()
                .ok_or_else(|| CypherError::ParseError("Missing right operand".to_string()))?,
        )?;
        expr = Expression::BinaryOp {
            left: Box::new(expr),
            op,
            right: Box::new(right),
        };
    }
    Ok(expr)
}

fn parse_mult_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair.into_inner().collect();
    if parts.len() == 1 {
        return parse_power_expression(parts.remove(0));
    }

    let mut iter = parts.into_iter();
    let mut expr =
        parse_power_expression(iter.next().ok_or_else(|| {
            CypherError::ParseError("Empty multiplication expression".to_string())
        })?)?;

    while let Some(op_pair) = iter.next() {
        let op = match op_pair.as_str() {
            "*" => BinaryOp::Mul,
            "/" => BinaryOp::Div,
            "%" => BinaryOp::Mod,
            _ => {
                return Err(CypherError::ParseError(format!(
                    "Unknown mult op: {}",
                    op_pair.as_str()
                )));
            }
        };
        let right = parse_power_expression(
            iter.next()
                .ok_or_else(|| CypherError::ParseError("Missing right operand".to_string()))?,
        )?;
        expr = Expression::BinaryOp {
            left: Box::new(expr),
            op,
            right: Box::new(right),
        };
    }
    Ok(expr)
}

fn parse_power_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut parts: Vec<pest::iterators::Pair<Rule>> = pair.into_inner().collect();
    if parts.len() == 1 {
        return parse_unary_expression(parts.remove(0));
    }

    let mut exprs: Vec<Expression> = parts
        .into_iter()
        .map(|p| parse_unary_expression(p))
        .collect::<Result<Vec<_>, _>>()?;

    // Right-associative: a ^ b ^ c = a ^ (b ^ c)
    let mut expr = exprs
        .pop()
        .ok_or_else(|| CypherError::ParseError("Empty power expression".to_string()))?;
    while let Some(left) = exprs.pop() {
        expr = Expression::BinaryOp {
            left: Box::new(left),
            op: BinaryOp::Pow,
            right: Box::new(expr),
        };
    }
    Ok(expr)
}

fn parse_unary_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut unary_op = None;
    let mut postfix_expr = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::unary_op => {
                unary_op = Some(match inner.as_str() {
                    "-" => UnaryOp::Neg,
                    _ => UnaryOp::Pos,
                });
            }
            Rule::postfix_expression => {
                postfix_expr = Some(parse_postfix_expression(inner)?);
            }
            _ => {}
        }
    }

    let expr =
        postfix_expr.ok_or_else(|| CypherError::ParseError("Missing expression".to_string()))?;

    if let Some(op) = unary_op {
        Ok(Expression::UnaryOp {
            op,
            operand: Box::new(expr),
        })
    } else {
        Ok(expr)
    }
}

fn parse_postfix_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut expr = None;
    let mut null_is_not = None;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::atom => {
                expr = Some(parse_atom(inner)?);
            }
            Rule::postfix_op => {
                let Some(op_inner) = inner.into_inner().next() else {
                    continue;
                };
                match op_inner.as_rule() {
                    Rule::property_lookup => {
                        let Some(prop_pair) = op_inner.into_inner().next() else {
                            continue;
                        };
                        let prop_name = prop_pair.as_str().to_string();
                        let base = expr.take().ok_or_else(|| {
                            CypherError::ParseError(
                                "Expected expression before property access".to_string(),
                            )
                        })?;
                        expr = Some(Expression::Property(Box::new(base), prop_name));
                    }
                    Rule::subscript => {
                        let Some(content) = op_inner.into_inner().next() else {
                            continue;
                        };
                        let Some(content_inner) = content.into_inner().next() else {
                            continue;
                        };
                        match content_inner.as_rule() {
                            Rule::slice_range => {
                                let (start, end) = parse_slice_range(content_inner)?;
                                let base = expr.take().ok_or_else(|| {
                                    CypherError::ParseError(
                                        "Expected expression before slice operator".to_string(),
                                    )
                                })?;
                                expr = Some(Expression::ListSlice {
                                    expr: Box::new(base),
                                    start,
                                    end,
                                });
                            }
                            Rule::expression => {
                                let key = parse_expression(content_inner)?;
                                let base = expr.take().ok_or_else(|| {
                                    CypherError::ParseError(
                                        "Expected expression before subscript operator".to_string(),
                                    )
                                })?;
                                expr = Some(Expression::DynamicProperty {
                                    expr: Box::new(base),
                                    key: Box::new(key),
                                });
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Rule::null_predicate => {
                let text = inner.as_str().to_uppercase();
                null_is_not = Some(text.contains("NOT"));
            }
            _ => {}
        }
    }

    let mut result = expr.ok_or_else(|| CypherError::ParseError("Missing atom".to_string()))?;

    if let Some(is_not) = null_is_not {
        result = if is_not {
            Expression::IsNotNull(Box::new(result))
        } else {
            Expression::IsNull(Box::new(result))
        };
    }

    Ok(result)
}

type SliceRange = (Option<Box<Expression>>, Option<Box<Expression>>);

fn parse_slice_range(pair: pest::iterators::Pair<Rule>) -> Result<SliceRange, CypherError> {
    // The grammar is: slice_range = { expression? ~ dotdot ~ expression? }
    // We distinguish which expression appears before vs. after ".." by checking
    // byte offsets against the `dotdot` token, which the grammar now exposes as
    // a named rule.  This is correct even with leading/trailing whitespace inside
    // the brackets because pest spans are byte-accurate.

    let mut dotdot_start: Option<usize> = None;
    let mut expr_pairs: Vec<pest::iterators::Pair<Rule>> = Vec::new();

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::dotdot => {
                dotdot_start = Some(inner.as_span().start());
            }
            Rule::expression => {
                expr_pairs.push(inner);
            }
            _ => {}
        }
    }

    let dotdot_pos = dotdot_start.unwrap_or(usize::MAX);

    match expr_pairs.len() {
        0 => Ok((None, None)),
        1 => {
            let expr_pair = expr_pairs.remove(0);
            let expr_offset = expr_pair.as_span().start();
            let expr = parse_expression(expr_pair)?;
            if expr_offset < dotdot_pos {
                // expression comes before "..": "expr.."
                Ok((Some(Box::new(expr)), None))
            } else {
                // expression comes after "..": "..expr"
                Ok((None, Some(Box::new(expr))))
            }
        }
        _ => {
            let start_expr = parse_expression(expr_pairs.remove(0))?;
            let end_expr = parse_expression(expr_pairs.remove(0))?;
            Ok((Some(Box::new(start_expr)), Some(Box::new(end_expr))))
        }
    }
}

fn parse_atom(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty atom".to_string()))?;
    match inner.as_rule() {
        Rule::case_expression => parse_case_expression(inner),
        Rule::list_comprehension => parse_list_comprehension(inner),
        Rule::filter_predicate => parse_filter_predicate(inner),
        Rule::reduce_expression => parse_reduce_expression(inner),
        Rule::literal => parse_literal(inner),
        Rule::parameter => parse_parameter(inner),
        Rule::function_invocation => parse_function_invocation(inner),
        Rule::parenthesized_expression => {
            let expr_pair = inner.into_inner().next().ok_or_else(|| {
                CypherError::ParseError("Empty parenthesized expression".to_string())
            })?;
            parse_expression(expr_pair)
        }
        Rule::variable => Ok(Expression::Variable(inner.as_str().to_string())),
        _ => Err(CypherError::ParseError(format!(
            "Unexpected atom: {:?}",
            inner.as_rule()
        ))),
    }
}

fn parse_filter_predicate(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    // filter_pred_kw ~ "(" ~ variable ~ in_kw ~ expression ~ where_kw ~ expression ~ ")"
    let mut kind_str = String::new();
    let mut variable = String::new();
    let mut list_expr = None;
    let mut predicate_expr = None;

    enum Section {
        List,
        Predicate,
    }
    let mut section = Section::List;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::filter_pred_kw => {
                kind_str = inner.as_str().to_lowercase();
            }
            Rule::variable => {
                variable = inner.as_str().to_string();
            }
            Rule::in_kw => {
                section = Section::List;
            }
            Rule::where_kw => {
                section = Section::Predicate;
            }
            Rule::expression => {
                let expr = parse_expression(inner)?;
                match section {
                    Section::List => {
                        list_expr = Some(expr);
                    }
                    Section::Predicate => {
                        predicate_expr = Some(expr);
                    }
                }
            }
            _ => {}
        }
    }

    let kind = match kind_str.as_str() {
        "any" => FilterPredicateKind::Any,
        "all" => FilterPredicateKind::All,
        "none" => FilterPredicateKind::None,
        "single" => FilterPredicateKind::Single,
        _ => {
            return Err(CypherError::ParseError(format!(
                "Unknown filter predicate: {}",
                kind_str
            )));
        }
    };

    Ok(Expression::FilterPredicate {
        kind,
        variable,
        list: Box::new(list_expr.ok_or_else(|| {
            CypherError::ParseError("Missing list in filter predicate".to_string())
        })?),
        predicate: Box::new(predicate_expr.ok_or_else(|| {
            CypherError::ParseError("Missing predicate in filter predicate".to_string())
        })?),
    })
}

fn parse_reduce_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    // reduce_kw ~ "(" ~ variable ~ "=" ~ expression ~ "," ~ variable ~ in_kw ~ expression ~ pipe_op ~ expression ~ ")"
    let mut variables: Vec<String> = Vec::new();
    let mut expressions: Vec<Expression> = Vec::new();

    enum ExprSection {
        Init,
        List,
        Body,
    }
    let mut section = ExprSection::Init;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::reduce_kw => {}
            Rule::variable => {
                variables.push(inner.as_str().to_string());
            }
            Rule::in_kw => {
                section = ExprSection::List;
            }
            Rule::pipe_op => {
                section = ExprSection::Body;
            }
            Rule::expression => {
                let expr = parse_expression(inner)?;
                match section {
                    ExprSection::Init => {
                        expressions.push(expr);
                    }
                    ExprSection::List => {
                        expressions.push(expr);
                    }
                    ExprSection::Body => {
                        expressions.push(expr);
                    }
                }
            }
            _ => {}
        }
    }

    if variables.len() < 2 || expressions.len() < 3 {
        return Err(CypherError::ParseError(
            "Invalid reduce expression".to_string(),
        ));
    }

    Ok(Expression::Reduce {
        accumulator: variables[0].clone(),
        init: Box::new(expressions[0].clone()),
        variable: variables[1].clone(),
        list: Box::new(expressions[1].clone()),
        body: Box::new(expressions[2].clone()),
    })
}

fn parse_case_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut operand = None;
    let mut alternatives = Vec::new();
    let mut default = None;
    let mut seen_alternative = false;

    // The grammar is: CASE ~ expression? ~ case_alternative+ ~ (ELSE ~ expression)? ~ END
    // We need to distinguish the optional operand expression from ELSE expression.
    // The operand appears before any WHEN, the ELSE appears after all alternatives.
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::expression => {
                if !seen_alternative {
                    // This is the operand (simple CASE form)
                    operand = Some(Box::new(parse_expression(inner)?));
                } else {
                    // This is the ELSE expression
                    default = Some(Box::new(parse_expression(inner)?));
                }
            }
            Rule::case_alternative => {
                seen_alternative = true;
                let mut when_expr = None;
                let mut then_expr = None;
                for alt_inner in inner.into_inner() {
                    if alt_inner.as_rule() == Rule::expression {
                        if when_expr.is_none() {
                            when_expr = Some(parse_expression(alt_inner)?);
                        } else {
                            then_expr = Some(parse_expression(alt_inner)?);
                        }
                    }
                }
                if let (Some(w), Some(t)) = (when_expr, then_expr) {
                    alternatives.push(CaseAlternative { when: w, then: t });
                }
            }
            _ => {}
        }
    }

    Ok(Expression::Case {
        operand,
        alternatives,
        default,
    })
}

fn parse_list_comprehension(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    // [variable IN expression (WHERE expression)? (| expression)?]
    let mut variable = String::new();
    let mut list_expr = None;
    let mut predicate = None;
    let mut map_expr = None;

    // Track which section we're in: list, where_pred, pipe_map
    enum Section {
        List,
        WherePred,
        PipeMap,
    }
    let mut section = Section::List;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::variable => {
                variable = inner.as_str().to_string();
            }
            Rule::in_kw => {
                section = Section::List;
            }
            Rule::where_kw => {
                section = Section::WherePred;
            }
            Rule::pipe_op => {
                section = Section::PipeMap;
            }
            Rule::expression => {
                let expr = parse_expression(inner)?;
                match section {
                    Section::List => {
                        list_expr = Some(expr);
                    }
                    Section::WherePred => {
                        predicate = Some(Box::new(expr));
                    }
                    Section::PipeMap => {
                        map_expr = Some(Box::new(expr));
                    }
                }
            }
            _ => {}
        }
    }

    Ok(Expression::ListComprehension {
        variable,
        list: Box::new(list_expr.ok_or_else(|| {
            CypherError::ParseError("Missing list in list comprehension".to_string())
        })?),
        predicate,
        map_expr,
    })
}

fn parse_parameter(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let name = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty parameter".to_string()))?
        .as_str()
        .to_string();
    Ok(Expression::Parameter(name))
}

fn parse_literal(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let inner = pair
        .into_inner()
        .next()
        .ok_or_else(|| CypherError::ParseError("Empty literal".to_string()))?;
    match inner.as_rule() {
        Rule::float => {
            let f: f64 = inner
                .as_str()
                .parse()
                .map_err(|e| CypherError::ParseError(format!("Invalid float: {}", e)))?;
            Ok(Expression::Literal(Literal::Float(f)))
        }
        Rule::integer => {
            let s = inner.as_str();
            let i = parse_integer_value(s)?;
            Ok(Expression::Literal(Literal::Integer(i)))
        }
        Rule::string => {
            let s = inner.as_str();
            let content = parse_string_content(s)?;
            Ok(Expression::Literal(Literal::String(content)))
        }
        Rule::boolean => {
            let b = inner.as_str().to_lowercase() == "true";
            Ok(Expression::Literal(Literal::Boolean(b)))
        }
        Rule::null_literal => Ok(Expression::Literal(Literal::Null)),
        Rule::list_literal => {
            let mut items = Vec::new();
            for list_inner in inner.into_inner() {
                if list_inner.as_rule() == Rule::expression_list {
                    for expr_pair in list_inner.into_inner() {
                        if expr_pair.as_rule() == Rule::expression {
                            items.push(parse_expression(expr_pair)?);
                        }
                    }
                }
            }
            Ok(Expression::Literal(Literal::List(items)))
        }
        Rule::map_literal => {
            let mut entries = Vec::new();
            for map_inner in inner.into_inner() {
                if map_inner.as_rule() == Rule::map_entry {
                    let mut key = String::new();
                    let mut val = None;
                    for p in map_inner.into_inner() {
                        match p.as_rule() {
                            Rule::property_key => key = p.as_str().to_string(),
                            Rule::expression => val = Some(parse_expression(p)?),
                            _ => {}
                        }
                    }
                    if let Some(v) = val {
                        entries.push((key, v));
                    }
                }
            }
            Ok(Expression::Literal(Literal::Map(MapLiteral { entries })))
        }
        _ => Err(CypherError::ParseError(format!(
            "Unexpected literal: {:?}",
            inner.as_rule()
        ))),
    }
}

/// Parse integer from string, supporting decimal, hex (0x), and octal (0o).
fn parse_integer_value(s: &str) -> Result<i64, CypherError> {
    let (negative, digits) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s)
    };

    let val = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16)
            .map_err(|e| CypherError::ParseError(format!("Invalid hex integer: {}", e)))?
    } else if let Some(oct) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        i64::from_str_radix(oct, 8)
            .map_err(|e| CypherError::ParseError(format!("Invalid octal integer: {}", e)))?
    } else {
        digits
            .parse::<i64>()
            .map_err(|e| CypherError::ParseError(format!("Invalid integer: {}", e)))?
    };

    Ok(if negative { -val } else { val })
}

/// Parse string content with escape sequence support.
fn parse_string_content(s: &str) -> Result<String, CypherError> {
    if s.len() < 2 {
        return Err(CypherError::ParseError(
            "Invalid string literal".to_string(),
        ));
    }
    // Remove outer quotes
    let quote = s.as_bytes()[0];
    let inner = &s[1..s.len() - 1];

    let mut result = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => result.push('\t'),
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('\'') => result.push('\''),
                Some('/') => result.push('/'),
                Some('u') => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        if let Some(h) = chars.next() {
                            hex.push(h);
                        }
                    }
                    let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                        CypherError::ParseError(format!("Invalid unicode escape: \\u{}", hex))
                    })?;
                    let ch = char::from_u32(code).ok_or_else(|| {
                        CypherError::ParseError(format!("Invalid unicode codepoint: \\u{}", hex))
                    })?;
                    result.push(ch);
                }
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }

    let _ = quote; // Both quote types handled uniformly
    Ok(result)
}

fn parse_function_invocation(pair: pest::iterators::Pair<Rule>) -> Result<Expression, CypherError> {
    let mut name = String::new();
    let mut distinct = false;
    let mut args = Vec::new();
    let mut is_star = false;

    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::function_name => {
                name = inner.as_str().to_string();
            }
            Rule::distinct_kw => {
                distinct = true;
            }
            Rule::star_arg => {
                is_star = true;
            }
            Rule::expression_list => {
                for expr_pair in inner.into_inner() {
                    if expr_pair.as_rule() == Rule::expression {
                        args.push(parse_expression(expr_pair)?);
                    }
                }
            }
            _ => {}
        }
    }

    // count(*) is represented as count() with no args (star signals "count all rows")
    if is_star {
        args.clear();
    }

    Ok(Expression::FunctionCall {
        name,
        distinct,
        args,
    })
}
