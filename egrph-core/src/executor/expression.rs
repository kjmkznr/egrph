use crate::ast::*;
use crate::error::CypherError;
use crate::graph::backend::StorageBackend;
use crate::graph::types::*;
use regex::Regex;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

/// Thread-local LRU regex cache: compiled patterns are stored keyed by their
/// anchored form.  A `VecDeque` tracks insertion order so the oldest entry is
/// evicted first when the cache reaches capacity.
struct RegexCache {
    map: HashMap<String, Option<Regex>>,
    order: VecDeque<String>,
}

impl RegexCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Look up a compiled regex, compiling and inserting it if absent.
    /// Evicts the oldest entry when the cache is full.
    fn get_or_insert(&mut self, pattern: &str) -> Option<&Regex> {
        if !self.map.contains_key(pattern) {
            if self.map.len() >= REGEX_CACHE_MAX_ENTRIES {
                // Evict the oldest entry (front of the insertion-order queue).
                if let Some(oldest) = self.order.pop_front() {
                    self.map.remove(&oldest);
                }
            }
            let compiled = Regex::new(pattern).ok();
            self.map.insert(pattern.to_string(), compiled);
            self.order.push_back(pattern.to_string());
        }
        self.map.get(pattern)?.as_ref()
    }
}

thread_local! {
    static REGEX_CACHE: RefCell<RegexCache> = RefCell::new(RegexCache::new());
}

/// Maximum number of compiled regex patterns to keep per thread before evicting
/// the oldest (LRU) entry.  This bounds per-thread memory usage for regex-heavy workloads.
const REGEX_CACHE_MAX_ENTRIES: usize = 256;

/// A record is a mapping of variable names to CypherValues.
///
/// Internally backed by `Arc<HashMap<...>>` so that pipeline stages which
/// clone records for row multiplication (Expand, VarLengthExpand, Unwind,
/// CartesianProduct, ...) pay only an atomic refcount bump on `clone()`.
/// Mutating methods (`insert`, `remove`, `extend`, `retain`) go through
/// `Arc::make_mut`, producing a unique copy only on first write after a
/// shared clone (copy-on-write).
#[derive(Debug, Clone, Default)]
pub struct Record {
    inner: Arc<HashMap<String, CypherValue>>,
}

impl Record {
    pub fn new() -> Self {
        // Share a single empty HashMap across every `Record::new()` call so the
        // countless placeholder records (empty_rec, SKIP/LIMIT eval ctx, etc.)
        // don't each allocate their own HashMap.
        static EMPTY: OnceLock<Arc<HashMap<String, CypherValue>>> = OnceLock::new();
        Self {
            inner: EMPTY.get_or_init(|| Arc::new(HashMap::new())).clone(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            inner: Arc::new(HashMap::with_capacity(cap)),
        }
    }

    pub fn insert(&mut self, k: String, v: CypherValue) -> Option<CypherValue> {
        Arc::make_mut(&mut self.inner).insert(k, v)
    }

    pub fn remove<Q>(&mut self, k: &Q) -> Option<CypherValue>
    where
        String: std::borrow::Borrow<Q>,
        Q: ?Sized + std::hash::Hash + Eq,
    {
        Arc::make_mut(&mut self.inner).remove(k)
    }

    pub fn extend<I: IntoIterator<Item = (String, CypherValue)>>(&mut self, iter: I) {
        Arc::make_mut(&mut self.inner).extend(iter)
    }

    pub fn retain<F: FnMut(&String, &mut CypherValue) -> bool>(&mut self, f: F) {
        Arc::make_mut(&mut self.inner).retain(f)
    }

    /// Reserve capacity for at least `additional` more entries.
    /// Triggers a unique-copy if the inner Arc is shared.
    pub fn reserve(&mut self, additional: usize) {
        Arc::make_mut(&mut self.inner).reserve(additional)
    }
}

impl std::ops::Deref for Record {
    type Target = HashMap<String, CypherValue>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) || *self.inner == *other.inner
    }
}

impl IntoIterator for Record {
    type Item = (String, CypherValue);
    type IntoIter = std::collections::hash_map::IntoIter<String, CypherValue>;

    /// Consumes the record. Moves entries out if the inner `Arc` is uniquely
    /// owned; otherwise falls back to a deep clone of the backing HashMap.
    fn into_iter(self) -> Self::IntoIter {
        Arc::try_unwrap(self.inner)
            .unwrap_or_else(|arc| (*arc).clone())
            .into_iter()
    }
}

impl<'a> IntoIterator for &'a Record {
    type Item = (&'a String, &'a CypherValue);
    type IntoIter = std::collections::hash_map::Iter<'a, String, CypherValue>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

/// Parameters passed to a query.
pub type Parameters = HashMap<String, CypherValue>;

/// Evaluate an expression with both variable bindings and query parameters.
pub fn eval_with_params(
    expr: &Expression,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    Ok(match expr {
        Expression::Literal(lit) => eval_literal(lit, record, params, storage)?,
        Expression::Variable(name) => record.get(name).cloned().unwrap_or(CypherValue::Null),
        Expression::Property(base_expr, prop_name) => {
            let base = eval_with_params(base_expr, record, params, storage)?;
            get_property(&base, prop_name)
        }
        Expression::BinaryOp { left, op, right } => {
            let l = eval_with_params(left, record, params, storage)?;
            let r = eval_with_params(right, record, params, storage)?;
            eval_binary_op(&l, op, &r)?
        }
        Expression::UnaryOp { op, operand } => {
            let v = eval_with_params(operand, record, params, storage)?;
            eval_unary_op(op, &v)
        }
        Expression::Comparison { left, op, right } => {
            let l = eval_with_params(left, record, params, storage)?;
            let r = eval_with_params(right, record, params, storage)?;
            eval_comparison(&l, op, &r)
        }
        Expression::And(left, right) => {
            let l = eval_with_params(left, record, params, storage)?;
            let r = eval_with_params(right, record, params, storage)?;
            eval_and(&l, &r)
        }
        Expression::Or(left, right) => {
            let l = eval_with_params(left, record, params, storage)?;
            let r = eval_with_params(right, record, params, storage)?;
            eval_or(&l, &r)
        }
        Expression::Xor(left, right) => {
            let l = eval_with_params(left, record, params, storage)?;
            let r = eval_with_params(right, record, params, storage)?;
            eval_xor(&l, &r)
        }
        Expression::Not(operand) => {
            let v = eval_with_params(operand, record, params, storage)?;
            eval_not(&v)
        }
        Expression::IsNull(operand) => {
            let v = eval_with_params(operand, record, params, storage)?;
            CypherValue::Boolean(matches!(v, CypherValue::Null))
        }
        Expression::IsNotNull(operand) => {
            let v = eval_with_params(operand, record, params, storage)?;
            CypherValue::Boolean(!matches!(v, CypherValue::Null))
        }
        Expression::FunctionCall { name, args, .. } => {
            eval_function(name, args, record, params, storage)?
        }
        Expression::StringOp { left, op, right } => {
            let l = eval_with_params(left, record, params, storage)?;
            let r = eval_with_params(right, record, params, storage)?;
            eval_string_op(&l, op, &r)
        }
        Expression::RegexMatch { expr: e, pattern } => {
            let val = eval_with_params(e, record, params, storage)?;
            let pat = eval_with_params(pattern, record, params, storage)?;
            eval_regex_match(&val, &pat)
        }
        Expression::In { expr: e, list } => {
            let val = eval_with_params(e, record, params, storage)?;
            let list_val = eval_with_params(list, record, params, storage)?;
            eval_in(&val, &list_val)
        }
        Expression::DynamicProperty { expr: e, key } => {
            let base = eval_with_params(e, record, params, storage)?;
            let key_val = eval_with_params(key, record, params, storage)?;
            eval_dynamic_property(&base, &key_val)
        }
        Expression::ListSlice {
            expr: e,
            start,
            end,
        } => {
            let base = eval_with_params(e, record, params, storage)?;
            let start_val = match start.as_ref() {
                Some(s) => Some(eval_with_params(s, record, params, storage)?),
                None => None,
            };
            let end_val = match end.as_ref() {
                Some(e) => Some(eval_with_params(e, record, params, storage)?),
                None => None,
            };
            eval_list_slice(&base, start_val.as_ref(), end_val.as_ref())
        }
        Expression::Case {
            operand,
            alternatives,
            default,
        } => eval_case(
            operand.as_deref(),
            alternatives,
            default.as_deref(),
            record,
            params,
            storage,
        )?,
        Expression::ListComprehension {
            variable,
            list,
            predicate,
            map_expr,
        } => eval_list_comprehension(
            variable,
            list,
            predicate.as_deref(),
            map_expr.as_deref(),
            record,
            params,
            storage,
        )?,
        Expression::FilterPredicate {
            kind,
            variable,
            list,
            predicate,
        } => eval_filter_predicate(kind, variable, list, predicate, record, params, storage)?,
        Expression::Reduce {
            accumulator,
            init,
            variable,
            list,
            body,
        } => eval_reduce(
            accumulator,
            init,
            variable,
            list,
            body,
            record,
            params,
            storage,
        )?,
        Expression::Exists { pattern } => {
            CypherValue::Boolean(eval_exists(pattern, record, params, storage)?)
        }
        Expression::Parameter(name) => params.get(name).cloned().unwrap_or(CypherValue::Null),
    })
}

fn eval_literal(
    lit: &Literal,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    Ok(match lit {
        Literal::Integer(i) => CypherValue::Integer(*i),
        Literal::Float(f) => CypherValue::Float(*f),
        Literal::String(s) => CypherValue::String(s.clone()),
        Literal::Boolean(b) => CypherValue::Boolean(*b),
        Literal::Null => CypherValue::Null,
        Literal::List(exprs) => {
            let mut items = Vec::with_capacity(exprs.len());
            for e in exprs {
                items.push(eval_with_params(e, record, params, storage)?);
            }
            CypherValue::List(items)
        }
        Literal::Map(map_lit) => {
            let mut entries = HashMap::new();
            for (k, v) in &map_lit.entries {
                entries.insert(k.clone(), eval_with_params(v, record, params, storage)?);
            }
            CypherValue::Map(entries)
        }
    })
}

fn get_property(value: &CypherValue, prop_name: &str) -> CypherValue {
    match value {
        CypherValue::Node(node) => node
            .properties
            .get(prop_name)
            .map(property_value_to_cypher)
            .unwrap_or(CypherValue::Null),
        CypherValue::Relationship(edge) => edge
            .properties
            .get(prop_name)
            .map(property_value_to_cypher)
            .unwrap_or(CypherValue::Null),
        CypherValue::Map(map) => map.get(prop_name).cloned().unwrap_or(CypherValue::Null),
        CypherValue::Null => CypherValue::Null,
        _ => CypherValue::Null,
    }
}

pub fn property_value_to_cypher(pv: &PropertyValue) -> CypherValue {
    match pv {
        PropertyValue::String(s) => CypherValue::String(s.clone()),
        PropertyValue::Int(i) => CypherValue::Integer(*i),
        PropertyValue::Float(f) => CypherValue::Float(*f),
        PropertyValue::Bool(b) => CypherValue::Boolean(*b),
    }
}

pub fn to_f64(v: &CypherValue) -> Option<f64> {
    match v {
        CypherValue::Integer(i) => Some(*i as f64),
        CypherValue::Float(f) => Some(*f),
        _ => None,
    }
}

fn eval_binary_op(
    left: &CypherValue,
    op: &BinaryOp,
    right: &CypherValue,
) -> Result<CypherValue, CypherError> {
    if matches!(left, CypherValue::Null) || matches!(right, CypherValue::Null) {
        return Ok(CypherValue::Null);
    }

    // String concatenation with +
    if matches!(op, BinaryOp::Add)
        && let (CypherValue::String(l), CypherValue::String(r)) = (left, right)
    {
        return Ok(CypherValue::String(format!("{}{}", l, r)));
    }

    // List concatenation with +
    if matches!(op, BinaryOp::Add)
        && let (CypherValue::List(l), CypherValue::List(r)) = (left, right)
    {
        let mut result = l.clone();
        result.extend(r.clone());
        return Ok(CypherValue::List(result));
    }

    // Integer arithmetic when both are integers (except Div which may produce Float)
    if let (CypherValue::Integer(l), CypherValue::Integer(r)) = (left, right) {
        return Ok(match op {
            BinaryOp::Add => l
                .checked_add(*r)
                .map(CypherValue::Integer)
                .unwrap_or(CypherValue::Null),
            BinaryOp::Sub => l
                .checked_sub(*r)
                .map(CypherValue::Integer)
                .unwrap_or(CypherValue::Null),
            BinaryOp::Mul => l
                .checked_mul(*r)
                .map(CypherValue::Integer)
                .unwrap_or(CypherValue::Null),
            BinaryOp::Div => {
                if *r == 0 {
                    return Err(CypherError::RuntimeError("Division by zero".to_string()));
                } else {
                    l.checked_div(*r)
                        .map(CypherValue::Integer)
                        .unwrap_or(CypherValue::Null)
                }
            }
            BinaryOp::Mod => {
                if *r == 0 {
                    return Err(CypherError::RuntimeError("Division by zero".to_string()));
                } else {
                    l.checked_rem(*r)
                        .map(CypherValue::Integer)
                        .unwrap_or(CypherValue::Null)
                }
            }
            BinaryOp::Pow => CypherValue::Float((*l as f64).powf(*r as f64)),
        });
    }

    // Float arithmetic
    let l = to_f64(left);
    let r = to_f64(right);
    if let (Some(l), Some(r)) = (l, r) {
        return Ok(match op {
            BinaryOp::Add => CypherValue::Float(l + r),
            BinaryOp::Sub => CypherValue::Float(l - r),
            BinaryOp::Mul => CypherValue::Float(l * r),
            BinaryOp::Div => {
                if r == 0.0 {
                    CypherValue::Null
                } else {
                    CypherValue::Float(l / r)
                }
            }
            BinaryOp::Mod => {
                if r == 0.0 {
                    CypherValue::Null
                } else {
                    CypherValue::Float(l % r)
                }
            }
            BinaryOp::Pow => CypherValue::Float(l.powf(r)),
        });
    }

    Ok(CypherValue::Null)
}

fn eval_unary_op(op: &UnaryOp, value: &CypherValue) -> CypherValue {
    match (op, value) {
        (_, CypherValue::Null) => CypherValue::Null,
        (UnaryOp::Neg, CypherValue::Integer(i)) => i
            .checked_neg()
            .map(CypherValue::Integer)
            .unwrap_or(CypherValue::Null),
        (UnaryOp::Neg, CypherValue::Float(f)) => CypherValue::Float(-f),
        (UnaryOp::Pos, v) => v.clone(),
        _ => CypherValue::Null,
    }
}

fn eval_comparison(left: &CypherValue, op: &CompOp, right: &CypherValue) -> CypherValue {
    if matches!(left, CypherValue::Null) || matches!(right, CypherValue::Null) {
        return CypherValue::Null;
    }

    let ord = compare_values(left, right);
    match ord {
        Some(ord) => {
            let result = match op {
                CompOp::Eq => ord == std::cmp::Ordering::Equal,
                CompOp::Neq => ord != std::cmp::Ordering::Equal,
                CompOp::Lt => ord == std::cmp::Ordering::Less,
                CompOp::Gt => ord == std::cmp::Ordering::Greater,
                CompOp::Lte => ord != std::cmp::Ordering::Greater,
                CompOp::Gte => ord != std::cmp::Ordering::Less,
            };
            CypherValue::Boolean(result)
        }
        None => CypherValue::Null,
    }
}

pub fn compare_values(left: &CypherValue, right: &CypherValue) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (CypherValue::Integer(l), CypherValue::Integer(r)) => Some(l.cmp(r)),
        (CypherValue::Float(l), CypherValue::Float(r)) => l.partial_cmp(r),
        (CypherValue::Integer(l), CypherValue::Float(r)) => (*l as f64).partial_cmp(r),
        (CypherValue::Float(l), CypherValue::Integer(r)) => l.partial_cmp(&(*r as f64)),
        (CypherValue::String(l), CypherValue::String(r)) => Some(l.cmp(r)),
        (CypherValue::Boolean(l), CypherValue::Boolean(r)) => Some(l.cmp(r)),
        _ => None,
    }
}

fn to_bool(v: &CypherValue) -> Option<bool> {
    match v {
        CypherValue::Boolean(b) => Some(*b),
        CypherValue::Null => None,
        _ => None,
    }
}

fn eval_and(left: &CypherValue, right: &CypherValue) -> CypherValue {
    match (to_bool(left), to_bool(right)) {
        (Some(false), _) | (_, Some(false)) => CypherValue::Boolean(false),
        (Some(true), Some(true)) => CypherValue::Boolean(true),
        _ => CypherValue::Null,
    }
}

fn eval_or(left: &CypherValue, right: &CypherValue) -> CypherValue {
    match (to_bool(left), to_bool(right)) {
        (Some(true), _) | (_, Some(true)) => CypherValue::Boolean(true),
        (Some(false), Some(false)) => CypherValue::Boolean(false),
        _ => CypherValue::Null,
    }
}

fn eval_xor(left: &CypherValue, right: &CypherValue) -> CypherValue {
    match (to_bool(left), to_bool(right)) {
        (Some(l), Some(r)) => CypherValue::Boolean(l ^ r),
        _ => CypherValue::Null,
    }
}

fn eval_not(value: &CypherValue) -> CypherValue {
    match to_bool(value) {
        Some(b) => CypherValue::Boolean(!b),
        None => CypherValue::Null,
    }
}

// --- String operators ---

fn eval_string_op(left: &CypherValue, op: &StringMatchOp, right: &CypherValue) -> CypherValue {
    if matches!(left, CypherValue::Null) || matches!(right, CypherValue::Null) {
        return CypherValue::Null;
    }
    match (left, right) {
        (CypherValue::String(l), CypherValue::String(r)) => {
            let result = match op {
                StringMatchOp::StartsWith => l.starts_with(r.as_str()),
                StringMatchOp::EndsWith => l.ends_with(r.as_str()),
                StringMatchOp::Contains => l.contains(r.as_str()),
            };
            CypherValue::Boolean(result)
        }
        _ => CypherValue::Null,
    }
}

// --- Regex match ---

fn eval_regex_match(value: &CypherValue, pattern: &CypherValue) -> CypherValue {
    if matches!(value, CypherValue::Null) || matches!(pattern, CypherValue::Null) {
        return CypherValue::Null;
    }
    match (value, pattern) {
        (CypherValue::String(s), CypherValue::String(p)) => {
            // Cypher regex must match the entire string. Always wrap in a
            // non-capturing group and anchor so that alternations like "a|b"
            // are anchored correctly as a whole: "^(?:a|b)$".
            let full_pattern = format!("^(?:{})$", p);
            let is_match = REGEX_CACHE.with(|cache| {
                cache
                    .borrow_mut()
                    .get_or_insert(&full_pattern)
                    .map(|re| re.is_match(s))
            });
            match is_match {
                Some(b) => CypherValue::Boolean(b),
                None => CypherValue::Null,
            }
        }
        _ => CypherValue::Null,
    }
}

// --- IN operator ---

fn eval_in(value: &CypherValue, list: &CypherValue) -> CypherValue {
    match list {
        CypherValue::Null => CypherValue::Null,
        CypherValue::List(items) => {
            if matches!(value, CypherValue::Null) {
                // null IN [anything] is always null per three-value logic
                return CypherValue::Null;
            }
            let mut has_null = false;
            for item in items {
                if matches!(item, CypherValue::Null) {
                    has_null = true;
                    continue;
                }
                if let Some(std::cmp::Ordering::Equal) = compare_values(value, item) {
                    return CypherValue::Boolean(true);
                }
            }
            if has_null {
                CypherValue::Null
            } else {
                CypherValue::Boolean(false)
            }
        }
        _ => CypherValue::Null,
    }
}

// --- Dynamic property access ---

fn eval_dynamic_property(base: &CypherValue, key: &CypherValue) -> CypherValue {
    match key {
        CypherValue::String(k) => get_property(base, k),
        CypherValue::Integer(i) => {
            // List index access
            match base {
                CypherValue::List(items) => {
                    let idx = *i;
                    let len = items.len() as i64;
                    let resolved = if idx < 0 { len + idx } else { idx };
                    if resolved >= 0 && (resolved as usize) < items.len() {
                        items[resolved as usize].clone()
                    } else {
                        CypherValue::Null
                    }
                }
                _ => CypherValue::Null,
            }
        }
        CypherValue::Null => CypherValue::Null,
        _ => CypherValue::Null,
    }
}

// --- List slice ---

/// Resolve a slice index with negative-wrapping semantics, clamped to `[0, len]`.
///
/// Negative indices count from the end (e.g. -1 → last element).
/// `default` is returned when `val` is `None` or a non-integer type.
fn resolve_slice_index(val: Option<&CypherValue>, len: i64, default: i64) -> i64 {
    match val {
        None => default,
        Some(CypherValue::Integer(i)) => {
            if *i < 0 {
                (len + *i).max(0)
            } else {
                (*i).min(len)
            }
        }
        _ => default,
    }
}

fn eval_list_slice(
    base: &CypherValue,
    start: Option<&CypherValue>,
    end: Option<&CypherValue>,
) -> CypherValue {
    match base {
        CypherValue::Null => CypherValue::Null,
        CypherValue::List(items) => {
            let len = items.len() as i64;
            let start_idx = resolve_slice_index(start, len, 0) as usize;
            let end_idx = resolve_slice_index(end, len, len) as usize;

            if start_idx >= end_idx || start_idx >= items.len() {
                CypherValue::List(Vec::new())
            } else {
                CypherValue::List(items[start_idx..end_idx.min(items.len())].to_vec())
            }
        }
        CypherValue::String(s) => {
            // String slicing (same semantics as list slicing) -- use char indices
            let char_count = s.chars().count() as i64;
            let start_idx = resolve_slice_index(start, char_count, 0) as usize;
            let end_idx = resolve_slice_index(end, char_count, char_count) as usize;
            if start_idx >= end_idx {
                CypherValue::String(String::new())
            } else {
                let result: String = s
                    .chars()
                    .skip(start_idx)
                    .take(end_idx - start_idx)
                    .collect();
                CypherValue::String(result)
            }
        }
        _ => CypherValue::Null,
    }
}

// --- CASE expression ---

fn eval_case(
    operand: Option<&Expression>,
    alternatives: &[CaseAlternative],
    default: Option<&Expression>,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    match operand {
        Some(op_expr) => {
            // Simple CASE: CASE expr WHEN val1 THEN result1 ...
            let op_val = eval_with_params(op_expr, record, params, storage)?;
            for alt in alternatives {
                let when_val = eval_with_params(&alt.when, record, params, storage)?;
                if let Some(std::cmp::Ordering::Equal) = compare_values(&op_val, &when_val) {
                    return eval_with_params(&alt.then, record, params, storage);
                }
            }
        }
        None => {
            // General CASE: CASE WHEN pred1 THEN result1 ...
            for alt in alternatives {
                let when_val = eval_with_params(&alt.when, record, params, storage)?;
                if is_truthy(&when_val) {
                    return eval_with_params(&alt.then, record, params, storage);
                }
            }
        }
    }
    // No match - return default or null
    match default {
        Some(def) => eval_with_params(def, record, params, storage),
        None => Ok(CypherValue::Null),
    }
}

// --- List comprehension ---

fn eval_list_comprehension(
    variable: &str,
    list: &Expression,
    predicate: Option<&Expression>,
    map_expr: Option<&Expression>,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    let list_val = eval_with_params(list, record, params, storage)?;
    Ok(match list_val {
        CypherValue::List(items) => {
            let mut result = Vec::new();
            for item in items {
                let mut inner_rec = record.clone();
                inner_rec.insert(variable.to_string(), item);

                // Apply predicate filter if present
                if let Some(pred) = predicate {
                    let pred_val = eval_with_params(pred, &inner_rec, params, storage)?;
                    if !is_truthy(&pred_val) {
                        continue;
                    }
                }

                // Apply map expression if present
                let output = match map_expr {
                    Some(me) => eval_with_params(me, &inner_rec, params, storage)?,
                    None => inner_rec
                        .get(variable)
                        .cloned()
                        .unwrap_or(CypherValue::Null),
                };
                result.push(output);
            }
            CypherValue::List(result)
        }
        CypherValue::Null => CypherValue::Null,
        _ => CypherValue::Null,
    })
}

fn eval_filter_predicate(
    kind: &FilterPredicateKind,
    variable: &str,
    list: &Expression,
    predicate: &Expression,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    let list_val = eval_with_params(list, record, params, storage)?;
    Ok(match list_val {
        CypherValue::List(items) => {
            if items.is_empty() {
                match kind {
                    FilterPredicateKind::Any => CypherValue::Boolean(false),
                    FilterPredicateKind::All => CypherValue::Boolean(true),
                    FilterPredicateKind::None => CypherValue::Boolean(true),
                    FilterPredicateKind::Single => CypherValue::Boolean(false),
                }
            } else {
                let mut matching = 0usize;
                let mut has_null = false;
                for item in &items {
                    let mut scope = record.clone();
                    scope.insert(variable.to_string(), item.clone());
                    let pred_val = eval_with_params(predicate, &scope, params, storage)?;
                    match pred_val {
                        CypherValue::Boolean(true) => matching += 1,
                        CypherValue::Null => has_null = true,
                        _ => {}
                    }
                }

                match kind {
                    FilterPredicateKind::Any => {
                        if matching > 0 {
                            CypherValue::Boolean(true)
                        } else if has_null {
                            CypherValue::Null
                        } else {
                            CypherValue::Boolean(false)
                        }
                    }
                    FilterPredicateKind::All => {
                        if matching == items.len() {
                            CypherValue::Boolean(true)
                        } else if has_null {
                            CypherValue::Null
                        } else {
                            CypherValue::Boolean(false)
                        }
                    }
                    FilterPredicateKind::None => {
                        if matching > 0 {
                            CypherValue::Boolean(false)
                        } else if has_null {
                            CypherValue::Null
                        } else {
                            CypherValue::Boolean(true)
                        }
                    }
                    FilterPredicateKind::Single => {
                        if matching == 1 && !has_null {
                            CypherValue::Boolean(true)
                        } else if matching > 1 {
                            CypherValue::Boolean(false)
                        } else if has_null {
                            CypherValue::Null
                        } else {
                            CypherValue::Boolean(false)
                        }
                    }
                }
            }
        }
        CypherValue::Null => CypherValue::Null,
        _ => CypherValue::Null,
    })
}

#[allow(clippy::too_many_arguments)]
fn eval_reduce(
    accumulator: &str,
    init: &Expression,
    variable: &str,
    list: &Expression,
    body: &Expression,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    let list_val = eval_with_params(list, record, params, storage)?;
    let init_val = eval_with_params(init, record, params, storage)?;
    Ok(match list_val {
        CypherValue::List(items) => {
            let mut acc = init_val;
            for item in items {
                let mut scope = record.clone();
                scope.insert(accumulator.to_string(), acc);
                scope.insert(variable.to_string(), item);
                acc = eval_with_params(body, &scope, params, storage)?;
            }
            acc
        }
        CypherValue::Null => CypherValue::Null,
        _ => CypherValue::Null,
    })
}

// --- Built-in functions ---

fn eval_function(
    name: &str,
    args: &[Expression],
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<CypherValue, CypherError> {
    let lower = name.to_lowercase();
    Ok(match lower.as_str() {
        "__has_label" => {
            if args.len() == 2 {
                let node_val = eval_with_params(&args[0], record, params, storage)?;
                let label_val = eval_with_params(&args[1], record, params, storage)?;
                if let (CypherValue::Node(node), CypherValue::String(label)) =
                    (&node_val, &label_val)
                {
                    CypherValue::Boolean(node.labels.iter().any(|l| l == label))
                } else {
                    CypherValue::Boolean(false)
                }
            } else {
                CypherValue::Null
            }
        }
        "id" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Node(n) => CypherValue::Integer(n.id as i64),
                    CypherValue::Relationship(e) => CypherValue::Integer(e.id as i64),
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "type" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                if let CypherValue::Relationship(e) = &val {
                    CypherValue::String(e.label.clone())
                } else {
                    CypherValue::Null
                }
            } else {
                CypherValue::Null
            }
        }
        "labels" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                if let CypherValue::Node(n) = &val {
                    CypherValue::List(
                        n.labels
                            .iter()
                            .map(|l| CypherValue::String(l.clone()))
                            .collect(),
                    )
                } else {
                    CypherValue::Null
                }
            } else {
                CypherValue::Null
            }
        }
        // Aggregate functions: handled at aggregation level (executor/aggregation.rs).
        // When called at row-level (not during aggregation), they return Null.
        "count" | "sum" | "avg" | "min" | "max" | "collect" | "percentilecont"
        | "percentiledisc" | "stdev" | "stdevp" => CypherValue::Null,
        "coalesce" => {
            for arg in args {
                let val = eval_with_params(arg, record, params, storage)?;
                if !matches!(val, CypherValue::Null) {
                    return Ok(val);
                }
            }
            CypherValue::Null
        }
        "tostring" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                // openCypher: toString is defined for scalars only; other types -> null (TypeError)
                match &val {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Integer(i) => CypherValue::String(i.to_string()),
                    CypherValue::Float(f) => CypherValue::String(float_to_cypher_string(*f)),
                    CypherValue::String(s) => CypherValue::String(s.clone()),
                    CypherValue::Boolean(b) => CypherValue::String(b.to_string()),
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "tointeger" | "toint" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Integer(_) => val,
                    CypherValue::Float(f) => {
                        // Cast is UB if f is out of i64 range; guard with range check.
                        if f.is_nan()
                            || f.is_infinite()
                            || *f < i64::MIN as f64
                            || *f > i64::MAX as f64
                        {
                            CypherValue::Null
                        } else {
                            CypherValue::Integer(*f as i64)
                        }
                    }
                    CypherValue::String(s) => s
                        .parse::<i64>()
                        .map(CypherValue::Integer)
                        .unwrap_or(CypherValue::Null),
                    // openCypher spec: toInteger is not defined for Boolean; return null.
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "tofloat" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Float(_) => val,
                    CypherValue::Integer(i) => CypherValue::Float(*i as f64),
                    CypherValue::String(s) => s
                        .parse::<f64>()
                        .map(CypherValue::Float)
                        .unwrap_or(CypherValue::Null),
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "toboolean" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                // openCypher spec: toBoolean is defined for Boolean, String, and null only.
                match &val {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Boolean(_) => val,
                    CypherValue::String(s) => match s.to_lowercase().as_str() {
                        "true" => CypherValue::Boolean(true),
                        "false" => CypherValue::Boolean(false),
                        _ => CypherValue::Null,
                    },
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        // String / collection size
        // openCypher spec: size() applies to strings and lists; length() applies to paths.
        "size" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::String(s) => CypherValue::Integer(s.chars().count() as i64),
                    CypherValue::List(l) => CypherValue::Integer(l.len() as i64),
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        // openCypher spec: length() is for paths (number of hops).
        "length" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Path(p) => CypherValue::Integer(p.relationships.len() as i64),
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "trim" => string_fn_1(args, record, params, storage, |s| s.trim().to_string())?,
        "ltrim" => string_fn_1(args, record, params, storage, |s| {
            s.trim_start().to_string()
        })?,
        "rtrim" => string_fn_1(args, record, params, storage, |s| s.trim_end().to_string())?,
        "toupper" => string_fn_1(args, record, params, storage, |s| s.to_uppercase())?,
        "tolower" => string_fn_1(args, record, params, storage, |s| s.to_lowercase())?,
        "reverse" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::String(s) => CypherValue::String(s.chars().rev().collect()),
                    CypherValue::List(l) => CypherValue::List(l.iter().rev().cloned().collect()),
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "replace" => {
            if args.len() >= 3 {
                let val = eval_with_params(&args[0], record, params, storage)?;
                let search = eval_with_params(&args[1], record, params, storage)?;
                let replacement = eval_with_params(&args[2], record, params, storage)?;
                match (&val, &search, &replacement) {
                    (CypherValue::String(s), CypherValue::String(f), CypherValue::String(r)) => {
                        CypherValue::String(s.replace(f.as_str(), r.as_str()))
                    }
                    _ if matches!(&val, CypherValue::Null)
                        || matches!(&search, CypherValue::Null)
                        || matches!(&replacement, CypherValue::Null) =>
                    {
                        CypherValue::Null
                    }
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "substring" => {
            if args.len() >= 2 {
                let val = eval_with_params(&args[0], record, params, storage)?;
                let start = eval_with_params(&args[1], record, params, storage)?;
                let length = if args.len() >= 3 {
                    Some(eval_with_params(&args[2], record, params, storage)?)
                } else {
                    None
                };
                match (&val, &start) {
                    (CypherValue::String(s), CypherValue::Integer(start_idx)) => {
                        let char_count = s.chars().count();
                        let start_idx = (*start_idx).max(0) as usize;
                        if start_idx >= char_count {
                            return Ok(CypherValue::String(String::new()));
                        }
                        match length {
                            Some(CypherValue::Integer(len)) => {
                                let len = len.max(0) as usize;
                                CypherValue::String(s.chars().skip(start_idx).take(len).collect())
                            }
                            None => CypherValue::String(s.chars().skip(start_idx).collect()),
                            _ => CypherValue::Null,
                        }
                    }
                    _ if matches!(&val, CypherValue::Null)
                        || matches!(&start, CypherValue::Null) =>
                    {
                        CypherValue::Null
                    }
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "left" => {
            if args.len() >= 2 {
                let val = eval_with_params(&args[0], record, params, storage)?;
                let n = eval_with_params(&args[1], record, params, storage)?;
                match (&val, &n) {
                    (CypherValue::String(s), CypherValue::Integer(n)) => {
                        CypherValue::String(s.chars().take((*n).max(0) as usize).collect())
                    }
                    _ if matches!(&val, CypherValue::Null) || matches!(&n, CypherValue::Null) => {
                        CypherValue::Null
                    }
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "right" => {
            if args.len() >= 2 {
                let val = eval_with_params(&args[0], record, params, storage)?;
                let n = eval_with_params(&args[1], record, params, storage)?;
                match (&val, &n) {
                    (CypherValue::String(s), CypherValue::Integer(n)) => {
                        let n = (*n).max(0) as usize;
                        let char_count = s.chars().count();
                        let skip = char_count.saturating_sub(n);
                        CypherValue::String(s.chars().skip(skip).collect())
                    }
                    _ if matches!(&val, CypherValue::Null) || matches!(&n, CypherValue::Null) => {
                        CypherValue::Null
                    }
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "split" => {
            if args.len() >= 2 {
                let val = eval_with_params(&args[0], record, params, storage)?;
                let sep = eval_with_params(&args[1], record, params, storage)?;
                match (&val, &sep) {
                    (CypherValue::String(s), CypherValue::String(d)) => CypherValue::List(
                        s.split(d.as_str())
                            .map(|p| CypherValue::String(p.to_string()))
                            .collect(),
                    ),
                    _ if matches!(&val, CypherValue::Null) || matches!(&sep, CypherValue::Null) => {
                        CypherValue::Null
                    }
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        // Math functions
        "abs" => math_fn_1(args, record, params, storage, |v| match v {
            // checked_abs prevents undefined behaviour for i64::MIN (whose absolute value
            // does not fit in i64); return null on overflow to match openCypher semantics.
            CypherValue::Integer(i) => i
                .checked_abs()
                .map(CypherValue::Integer)
                .unwrap_or(CypherValue::Null),
            CypherValue::Float(f) => CypherValue::Float(f.abs()),
            _ => CypherValue::Null,
        })?,
        "ceil" => math_fn_1_f64(args, record, params, storage, f64::ceil)?,
        "floor" => math_fn_1_f64(args, record, params, storage, f64::floor)?,
        "round" => math_fn_1_f64(args, record, params, storage, f64::round)?,
        "sign" => math_fn_1(args, record, params, storage, |v| match v {
            CypherValue::Integer(i) => CypherValue::Integer(i.signum()),
            CypherValue::Float(f) => {
                if f > 0.0 {
                    CypherValue::Integer(1)
                } else if f < 0.0 {
                    CypherValue::Integer(-1)
                } else {
                    CypherValue::Integer(0)
                }
            }
            _ => CypherValue::Null,
        })?,
        "sqrt" => math_fn_1_f64(args, record, params, storage, f64::sqrt)?,
        "log" => math_fn_1_f64(args, record, params, storage, f64::ln)?,
        "log10" => math_fn_1_f64(args, record, params, storage, f64::log10)?,
        "exp" => math_fn_1_f64(args, record, params, storage, f64::exp)?,
        "sin" => math_fn_1_f64(args, record, params, storage, f64::sin)?,
        "cos" => math_fn_1_f64(args, record, params, storage, f64::cos)?,
        "tan" => math_fn_1_f64(args, record, params, storage, f64::tan)?,
        "asin" => math_fn_1_f64(args, record, params, storage, f64::asin)?,
        "acos" => math_fn_1_f64(args, record, params, storage, f64::acos)?,
        "atan" => math_fn_1_f64(args, record, params, storage, f64::atan)?,
        "atan2" => {
            if args.len() >= 2 {
                let y = eval_with_params(&args[0], record, params, storage)?;
                let x = eval_with_params(&args[1], record, params, storage)?;
                match (to_f64(&y), to_f64(&x)) {
                    (Some(y), Some(x)) => CypherValue::Float(y.atan2(x)),
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "degrees" => math_fn_1_f64(args, record, params, storage, f64::to_degrees)?,
        "radians" => math_fn_1_f64(args, record, params, storage, f64::to_radians)?,
        "pi" => CypherValue::Float(std::f64::consts::PI),
        "e" => CypherValue::Float(std::f64::consts::E),
        "rand" => CypherValue::Float(pseudo_rand()),
        // List functions
        "head" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match val {
                    CypherValue::List(items) => {
                        items.into_iter().next().unwrap_or(CypherValue::Null)
                    }
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "last" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match val {
                    CypherValue::List(items) => {
                        items.into_iter().last().unwrap_or(CypherValue::Null)
                    }
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "tail" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match val {
                    CypherValue::List(items) => {
                        if items.is_empty() {
                            CypherValue::List(Vec::new())
                        } else {
                            CypherValue::List(items[1..].to_vec())
                        }
                    }
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "range" => {
            match args.len() {
                2 | 3 => {
                    let start = eval_with_params(&args[0], record, params, storage)?;
                    let end = eval_with_params(&args[1], record, params, storage)?;
                    let step = if args.len() == 3 {
                        eval_with_params(&args[2], record, params, storage)?
                    } else {
                        CypherValue::Integer(1)
                    };
                    match (&start, &end, &step) {
                        (
                            CypherValue::Integer(s),
                            CypherValue::Integer(e),
                            CypherValue::Integer(st),
                        ) => {
                            if *st == 0 {
                                return Ok(CypherValue::Null);
                            }
                            // Cap to prevent accidental OOM allocation.
                            const MAX_RANGE_ELEMENTS: usize = 1_000_000;
                            let mut result = Vec::new();
                            let mut i = *s;
                            if *st > 0 {
                                while i <= *e && result.len() < MAX_RANGE_ELEMENTS {
                                    result.push(CypherValue::Integer(i));
                                    match i.checked_add(*st) {
                                        Some(next) => i = next,
                                        None => break,
                                    }
                                }
                            } else {
                                while i >= *e && result.len() < MAX_RANGE_ELEMENTS {
                                    result.push(CypherValue::Integer(i));
                                    match i.checked_add(*st) {
                                        Some(next) => i = next,
                                        None => break,
                                    }
                                }
                            }
                            CypherValue::List(result)
                        }
                        _ => CypherValue::Null,
                    }
                }
                _ => CypherValue::Null,
            }
        }
        "keys" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Node(n) => CypherValue::List(
                        n.properties
                            .keys()
                            .map(|k| CypherValue::String(k.clone()))
                            .collect(),
                    ),
                    CypherValue::Relationship(e) => CypherValue::List(
                        e.properties
                            .keys()
                            .map(|k| CypherValue::String(k.clone()))
                            .collect(),
                    ),
                    CypherValue::Map(m) => CypherValue::List(
                        m.keys().map(|k| CypherValue::String(k.clone())).collect(),
                    ),
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "properties" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Node(n) => CypherValue::Map(
                        n.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), property_value_to_cypher(v)))
                            .collect(),
                    ),
                    CypherValue::Relationship(e) => CypherValue::Map(
                        e.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), property_value_to_cypher(v)))
                            .collect(),
                    ),
                    CypherValue::Map(_) => val,
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "exists" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                CypherValue::Boolean(!matches!(val, CypherValue::Null))
            } else {
                CypherValue::Null
            }
        }
        "startnode" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Relationship(e) => {
                        let src_id = e.src;
                        record
                            .values()
                            .find(|v| matches!(v, CypherValue::Node(n) if n.id == src_id))
                            .cloned()
                            .or_else(|| {
                                storage
                                    .get_node(src_id)
                                    .map(|n| CypherValue::Node(n.clone()))
                            })
                            .unwrap_or(CypherValue::Null)
                    }
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "endnode" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match &val {
                    CypherValue::Relationship(e) => {
                        let dst_id = e.dst;
                        record
                            .values()
                            .find(|v| matches!(v, CypherValue::Node(n) if n.id == dst_id))
                            .cloned()
                            .or_else(|| {
                                storage
                                    .get_node(dst_id)
                                    .map(|n| CypherValue::Node(n.clone()))
                            })
                            .unwrap_or(CypherValue::Null)
                    }
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "nodes" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match val {
                    CypherValue::Path(p) => {
                        CypherValue::List(p.nodes.into_iter().map(CypherValue::Node).collect())
                    }
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "relationships" | "rels" => {
            if let Some(arg) = args.first() {
                let val = eval_with_params(arg, record, params, storage)?;
                match val {
                    CypherValue::Path(p) => CypherValue::List(
                        p.relationships
                            .into_iter()
                            .map(CypherValue::Relationship)
                            .collect(),
                    ),
                    CypherValue::Null => CypherValue::Null,
                    _ => CypherValue::Null,
                }
            } else {
                CypherValue::Null
            }
        }
        "cot" => math_fn_1_f64(args, record, params, storage, |x| 1.0 / x.tan())?,
        "haversin" => math_fn_1_f64(args, record, params, storage, |x| (1.0 - x.cos()) / 2.0)?,
        _ => CypherValue::Null,
    })
}

// Helper for single-arg string functions
fn string_fn_1(
    args: &[Expression],
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
    f: impl Fn(&str) -> String,
) -> Result<CypherValue, CypherError> {
    if let Some(arg) = args.first() {
        let val = eval_with_params(arg, record, params, storage)?;
        Ok(match &val {
            CypherValue::String(s) => CypherValue::String(f(s)),
            CypherValue::Null => CypherValue::Null,
            _ => CypherValue::Null,
        })
    } else {
        Ok(CypherValue::Null)
    }
}

// Helper for single-arg math functions that work on f64
fn math_fn_1_f64(
    args: &[Expression],
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
    f: impl Fn(f64) -> f64,
) -> Result<CypherValue, CypherError> {
    if let Some(arg) = args.first() {
        let val = eval_with_params(arg, record, params, storage)?;
        Ok(match to_f64(&val) {
            Some(v) => CypherValue::Float(f(v)),
            None => CypherValue::Null,
        })
    } else {
        Ok(CypherValue::Null)
    }
}

// Helper for single-arg math functions with custom return
fn math_fn_1(
    args: &[Expression],
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
    f: impl Fn(CypherValue) -> CypherValue,
) -> Result<CypherValue, CypherError> {
    if let Some(arg) = args.first() {
        let val = eval_with_params(arg, record, params, storage)?;
        if matches!(val, CypherValue::Null) {
            Ok(CypherValue::Null)
        } else {
            Ok(f(val))
        }
    } else {
        Ok(CypherValue::Null)
    }
}

/// Pseudo-random number generator using a thread-local xorshift state.
fn pseudo_rand() -> f64 {
    use std::cell::Cell;
    use std::time::SystemTime;
    thread_local! {
        static STATE: Cell<u64> = Cell::new({
            let t = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            if t == 0 { 0xDEAD_BEEF_CAFE_BABE } else { t }
        });
    }
    STATE.with(|s| {
        let mut x = s.get();
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        // Map to [0.0, 1.0)
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

pub fn cypher_value_to_string(val: &CypherValue) -> String {
    match val {
        CypherValue::Null => "null".to_string(),
        CypherValue::Boolean(b) => b.to_string(),
        CypherValue::Integer(i) => i.to_string(),
        CypherValue::Float(f) => float_to_cypher_string(*f),
        CypherValue::String(s) => s.clone(),
        CypherValue::List(_) => "[...]".to_string(),
        CypherValue::Map(_) => "{...}".to_string(),
        CypherValue::Node(n) => format!("Node({})", n.id),
        CypherValue::Relationship(e) => format!("Relationship({})", e.id),
        CypherValue::Path(_) => "Path(...)".to_string(),
    }
}

/// Format a float as a string in a way that is consistent with openCypher:
/// whole-number floats always include a decimal point (e.g., 1.0, not 1).
fn float_to_cypher_string(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        }
    } else {
        let s = format!("{}", f);
        if s.contains('.') || s.contains('e') || s.contains('E') {
            s
        } else {
            format!("{}.0", s)
        }
    }
}

/// Produce a stable string key for a CypherValue, suitable for deduplication.
/// Unlike Debug formatting, this produces consistent output for equal values.
pub fn cypher_value_to_stable_key(val: &CypherValue) -> String {
    match val {
        CypherValue::Null => "N".to_string(),
        CypherValue::Boolean(b) => format!("B:{}", b),
        CypherValue::Integer(i) => format!("I:{}", i),
        CypherValue::Float(f) => format!("F:{}", f.to_bits()),
        // Escape backslashes first, then null bytes, so the null-byte separator
        // used by the grouping-key joiner cannot appear inside a string value.
        CypherValue::String(s) => format!("S:{}", s.replace('\\', "\\\\").replace('\x00', "\\0")),
        CypherValue::List(items) => {
            let inner: Vec<String> = items.iter().map(cypher_value_to_stable_key).collect();
            format!("L:[{}]", inner.join(","))
        }
        CypherValue::Map(m) => {
            // Escape backslash, colon, and comma in map keys so that a key containing
            // ':' or ',' cannot collide with the key/value separator or entry separator.
            let mut entries: Vec<String> = m
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        k.replace('\\', "\\\\")
                            .replace(':', "\\:")
                            .replace(',', "\\,"),
                        cypher_value_to_stable_key(v)
                    )
                })
                .collect();
            entries.sort();
            format!("M:{{{}}}", entries.join(","))
        }
        CypherValue::Node(n) => format!("n:{}", n.id),
        CypherValue::Relationship(e) => format!("r:{}", e.id),
        CypherValue::Path(p) => {
            let node_ids: Vec<String> = p.nodes.iter().map(|n| n.id.to_string()).collect();
            format!("P:[{}]", node_ids.join(","))
        }
    }
}

/// Check if a CypherValue is truthy for filter/predicate purposes.
pub fn is_truthy(value: &CypherValue) -> bool {
    matches!(value, CypherValue::Boolean(true))
}

// ─── EXISTS subquery evaluation ──────────────────────────────────────────────

fn eval_exists(
    pattern: &PatternElement,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<bool, CypherError> {
    let (start_np, chain): (&NodePattern, &[PatternChainElement]) = match pattern {
        PatternElement::Node(np) => (np, &[]),
        PatternElement::Chain { start, elements } => (start, elements.as_slice()),
    };

    for step in chain {
        if step.relationship.range.is_some() {
            return Err(CypherError::SemanticError(
                "EXISTS does not yet support variable-length relationships".to_string(),
            ));
        }
    }

    let candidates = exists_node_candidates(start_np, record, params, storage)?;
    for start_node in candidates {
        if exists_walk_chain(&start_node, chain, 0, record, params, storage)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exists_node_candidates(
    np: &NodePattern,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<Vec<Node>, CypherError> {
    if let Some(var) = &np.variable {
        match record.get(var) {
            Some(CypherValue::Node(n)) => {
                return if exists_node_matches(n, np, record, params, storage)? {
                    Ok(vec![n.clone()])
                } else {
                    Ok(vec![])
                };
            }
            Some(_) => return Ok(vec![]),
            None => {}
        }
    }
    let label_filter = np.labels.first().map(|s| s.as_str());
    let nodes = storage.match_nodes(label_filter);
    let mut out = Vec::new();
    for n in nodes {
        if exists_node_matches(&n, np, record, params, storage)? {
            out.push(n);
        }
    }
    Ok(out)
}

fn exists_walk_chain(
    current: &Node,
    chain: &[PatternChainElement],
    idx: usize,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<bool, CypherError> {
    if idx == chain.len() {
        return Ok(true);
    }
    let step = &chain[idx];
    let edges = match step.relationship.direction {
        Direction::Outgoing => storage.outgoing_edges(current.id),
        Direction::Incoming => storage.incoming_edges(current.id),
        Direction::Undirected => {
            let mut v = storage.outgoing_edges(current.id);
            v.extend(storage.incoming_edges(current.id));
            v
        }
    };
    for edge in &edges {
        if !step.relationship.rel_types.is_empty()
            && !step.relationship.rel_types.iter().any(|t| t == &edge.label)
        {
            continue;
        }
        if !exists_rel_properties_match(
            edge,
            &step.relationship.properties,
            record,
            params,
            storage,
        )? {
            continue;
        }
        let next_id = match step.relationship.direction {
            Direction::Outgoing => edge.dst,
            Direction::Incoming => edge.src,
            Direction::Undirected => {
                if edge.src == current.id {
                    edge.dst
                } else {
                    edge.src
                }
            }
        };
        let Some(next) = storage.get_node(next_id) else {
            continue;
        };
        if let Some(var) = &step.node.variable {
            match record.get(var) {
                Some(CypherValue::Node(bound)) => {
                    if bound.id != next.id {
                        continue;
                    }
                }
                Some(_) => continue,
                None => {}
            }
        }
        if !exists_node_matches(&next, &step.node, record, params, storage)? {
            continue;
        }
        if exists_walk_chain(&next, chain, idx + 1, record, params, storage)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exists_node_matches(
    node: &Node,
    np: &NodePattern,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<bool, CypherError> {
    for required in &np.labels {
        if !node.labels.contains(required) {
            return Ok(false);
        }
    }
    if let Some(map) = &np.properties {
        for (k, expr) in &map.entries {
            let expected = eval_with_params(expr, record, params, storage)?;
            let actual = node
                .properties
                .get(k)
                .map(property_value_to_cypher)
                .unwrap_or(CypherValue::Null);
            if !exists_values_equal(&actual, &expected) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn exists_rel_properties_match(
    edge: &Edge,
    props: &Option<MapLiteral>,
    record: &Record,
    params: &Parameters,
    storage: &dyn StorageBackend,
) -> Result<bool, CypherError> {
    let Some(map) = props else { return Ok(true) };
    for (k, expr) in &map.entries {
        let expected = eval_with_params(expr, record, params, storage)?;
        let actual = edge
            .properties
            .get(k)
            .map(property_value_to_cypher)
            .unwrap_or(CypherValue::Null);
        if !exists_values_equal(&actual, &expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn exists_values_equal(actual: &CypherValue, expected: &CypherValue) -> bool {
    matches!(
        compare_values(actual, expected),
        Some(std::cmp::Ordering::Equal)
    )
}
