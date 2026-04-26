/// AST types for the openCypher query language.

#[derive(Debug, Clone)]
pub enum Statement {
    Query(Query),
    Union {
        left: Box<Statement>,
        right: Box<Statement>,
        all: bool,
    },
    CreateConstraint(CreateConstraintStatement),
}

#[derive(Debug, Clone)]
pub struct CreateConstraintStatement {
    pub variable: String,
    pub label: String,
    /// For single-property constraints this is a one-element Vec.
    /// For NODE KEY it may contain multiple property names.
    pub properties: Vec<String>,
    pub constraint_type: ConstraintType,
}

#[derive(Debug, Clone)]
pub enum ConstraintType {
    Unique,
    NotNull,
    NodeKey,
    PropertyType(PropertyTypeKind),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyTypeKind {
    Boolean,
    String,
    Integer,
    Float,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub clauses: Vec<Clause>,
}

#[derive(Debug, Clone)]
pub enum Clause {
    Create(CreateClause),
    Match(MatchClause),
    Return(ReturnClause),
    Where(WhereClause),
    With(WithClause),
    Unwind(UnwindClause),
    LoadCsv(LoadCsvClause),
    Set(SetClause),
    Remove(RemoveClause),
    Delete(DeleteClause),
    Merge(MergeClause),
}

#[derive(Debug, Clone)]
pub struct CreateClause {
    pub pattern: Pattern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Regular,
    Optional,
    Mandatory,
}

#[derive(Debug, Clone)]
pub struct MatchClause {
    pub pattern: Pattern,
    pub kind: MatchKind,
}

#[derive(Debug, Clone)]
pub struct WhereClause {
    pub expression: Expression,
}

#[derive(Debug, Clone)]
pub struct ReturnClause {
    pub items: Vec<ReturnItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<SortItem>>,
    pub skip: Option<Expression>,
    pub limit: Option<Expression>,
}

/// WITH clause: projects variables through the query pipeline (like RETURN but continues).
#[derive(Debug, Clone)]
pub struct WithClause {
    pub items: Vec<ReturnItem>,
    pub distinct: bool,
    pub order_by: Option<Vec<SortItem>>,
    pub skip: Option<Expression>,
    pub limit: Option<Expression>,
    pub where_clause: Option<Expression>,
}

/// UNWIND clause: expands a list into individual rows.
#[derive(Debug, Clone)]
pub struct UnwindClause {
    pub expression: Expression,
    pub alias: String,
}

/// LOAD CSV clause: reads a CSV file and produces one row per CSV record.
#[derive(Debug, Clone)]
pub struct LoadCsvClause {
    /// URL or file path expression (typically a string literal or $parameter).
    pub url: Expression,
    /// Variable name to bind each row.
    pub alias: String,
    /// When true, the first row is treated as a header and each row is a map.
    pub with_headers: bool,
    /// Optional field terminator expression (single-character string).
    pub field_terminator: Option<Expression>,
}

/// SET clause: sets properties or labels on nodes/relationships.
#[derive(Debug, Clone)]
pub struct SetClause {
    pub items: Vec<SetItem>,
}

#[derive(Debug, Clone)]
pub enum SetItem {
    /// SET n.prop = expr
    Property {
        variable: String,
        property: String,
        expression: Expression,
    },
    /// SET n = expr (replace all properties)
    AllProperties {
        variable: String,
        expression: Expression,
    },
    /// SET n += expr (merge properties)
    MergeProperties {
        variable: String,
        expression: Expression,
    },
    /// SET n:Label
    Label {
        variable: String,
        labels: Vec<String>,
    },
}

/// REMOVE clause: removes properties or labels from nodes/relationships.
#[derive(Debug, Clone)]
pub struct RemoveClause {
    pub items: Vec<RemoveItem>,
}

#[derive(Debug, Clone)]
pub enum RemoveItem {
    /// REMOVE n.prop
    Property { variable: String, property: String },
    /// REMOVE n:Label
    Label {
        variable: String,
        labels: Vec<String>,
    },
}

/// DELETE clause: deletes nodes or relationships.
#[derive(Debug, Clone)]
pub struct DeleteClause {
    pub detach: bool,
    pub expressions: Vec<Expression>,
}

/// MERGE clause: match or create a pattern.
#[derive(Debug, Clone)]
pub struct MergeClause {
    pub pattern: Pattern,
    pub on_create: Option<Vec<SetItem>>,
    pub on_match: Option<Vec<SetItem>>,
}

#[derive(Debug, Clone)]
pub struct ReturnItem {
    pub expression: Expression,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SortItem {
    pub expression: Expression,
    pub ascending: bool,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub parts: Vec<PatternPart>,
}

#[derive(Debug, Clone)]
pub struct PatternPart {
    pub variable: Option<String>,
    pub element: PatternElement,
}

#[derive(Debug, Clone)]
pub enum PatternElement {
    Node(NodePattern),
    Chain {
        start: NodePattern,
        elements: Vec<PatternChainElement>,
    },
}

#[derive(Debug, Clone)]
pub struct PatternChainElement {
    pub relationship: RelationshipPattern,
    pub node: NodePattern,
}

#[derive(Debug, Clone)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Option<MapLiteral>,
}

#[derive(Debug, Clone)]
pub struct RelationshipPattern {
    pub variable: Option<String>,
    pub rel_types: Vec<String>,
    pub direction: Direction,
    pub range: Option<RangeSpec>,
    pub properties: Option<MapLiteral>,
}

#[derive(Debug, Clone)]
pub enum Direction {
    Outgoing,
    Incoming,
    Undirected,
}

#[derive(Debug, Clone)]
pub struct RangeSpec {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum MapKey {
    /// Static identifier key, e.g. `{name: 1}`
    Identifier(String),
    /// Dynamic key expression, e.g. `{"x": 1}` or `{$k: 1}`
    Expression(Box<Expression>),
}

#[derive(Debug, Clone)]
pub struct MapLiteral {
    pub entries: Vec<(MapKey, Expression)>,
}

#[derive(Debug, Clone)]
pub enum Expression {
    Literal(Literal),
    Variable(String),
    Property(Box<Expression>, String),
    BinaryOp {
        left: Box<Expression>,
        op: BinaryOp,
        right: Box<Expression>,
    },
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expression>,
    },
    Comparison {
        left: Box<Expression>,
        op: CompOp,
        right: Box<Expression>,
    },
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    Xor(Box<Expression>, Box<Expression>),
    Not(Box<Expression>),
    IsNull(Box<Expression>),
    IsNotNull(Box<Expression>),
    FunctionCall {
        name: String,
        distinct: bool,
        args: Vec<Expression>,
    },
    /// String pattern matching: STARTS WITH, ENDS WITH, CONTAINS
    StringOp {
        left: Box<Expression>,
        op: StringMatchOp,
        right: Box<Expression>,
    },
    /// Regex match: expr =~ pattern
    RegexMatch {
        expr: Box<Expression>,
        pattern: Box<Expression>,
    },
    /// IN operator: expr IN list_expr
    In {
        expr: Box<Expression>,
        list: Box<Expression>,
    },
    /// Dynamic property access: expr[key_expr]
    DynamicProperty {
        expr: Box<Expression>,
        key: Box<Expression>,
    },
    /// List slice: expr[start..end]
    ListSlice {
        expr: Box<Expression>,
        start: Option<Box<Expression>>,
        end: Option<Box<Expression>>,
    },
    /// CASE expression (general form)
    Case {
        operand: Option<Box<Expression>>,
        alternatives: Vec<CaseAlternative>,
        default: Option<Box<Expression>>,
    },
    /// List comprehension: [variable IN list WHERE predicate | map_expr]
    ListComprehension {
        variable: String,
        list: Box<Expression>,
        predicate: Option<Box<Expression>>,
        map_expr: Option<Box<Expression>>,
    },
    /// Collection predicate: any/all/none/single(variable IN list WHERE predicate)
    FilterPredicate {
        kind: FilterPredicateKind,
        variable: String,
        list: Box<Expression>,
        predicate: Box<Expression>,
    },
    /// Reduce: reduce(accumulator = init, variable IN list | body)
    Reduce {
        accumulator: String,
        init: Box<Expression>,
        variable: String,
        list: Box<Expression>,
        body: Box<Expression>,
    },
    /// EXISTS { pattern } subquery — true iff the pattern has at least one match
    /// when evaluated against the current (outer) variable bindings.
    Exists {
        pattern: Box<PatternElement>,
    },
    /// Parameter reference: $param
    Parameter(String),
}

#[derive(Debug, Clone)]
pub enum FilterPredicateKind {
    Any,
    All,
    None,
    Single,
}

#[derive(Debug, Clone)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Neg,
    Pos,
}

#[derive(Debug, Clone)]
pub enum CompOp {
    Eq,
    Neq,
    Lt,
    Gt,
    Lte,
    Gte,
}

#[derive(Debug, Clone)]
pub enum StringMatchOp {
    StartsWith,
    EndsWith,
    Contains,
}

#[derive(Debug, Clone)]
pub struct CaseAlternative {
    pub when: Expression,
    pub then: Expression,
}

#[derive(Debug, Clone)]
pub enum Literal {
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    Null,
    List(Vec<Expression>),
    Map(MapLiteral),
}
