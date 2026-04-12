use crate::ast::{
    ConstraintType, Direction, Expression, NodePattern, PatternChainElement, PatternElement,
    RemoveItem, ReturnItem, SetItem, SortItem,
};

#[derive(Debug, Clone)]
pub enum LogicalPlan {
    /// Create a single node with labels and properties.
    CreateNode {
        input: Box<LogicalPlan>,
        pattern: NodePattern,
    },

    /// Create a path: start node + chain of (relationship, node).
    CreatePath {
        input: Box<LogicalPlan>,
        start: NodePattern,
        elements: Vec<PatternChainElement>,
    },

    /// Scan all nodes, optionally filtering by label, binding to variable.
    ScanNodes {
        label_filter: Option<String>,
        variable: String,
    },

    /// Expand from a source variable along relationships to target nodes.
    Expand {
        input: Box<LogicalPlan>,
        src_variable: String,
        rel_variable: Option<String>,
        dst_variable: String,
        rel_types: Vec<String>,
        direction: Direction,
    },

    /// Filter rows by a predicate expression.
    Filter {
        input: Box<LogicalPlan>,
        predicate: Expression,
    },

    /// Project/return items from the input plan.
    Return {
        input: Box<LogicalPlan>,
        items: Vec<ReturnItem>,
        distinct: bool,
    },

    /// Sort rows by sort items.
    Sort {
        input: Box<LogicalPlan>,
        items: Vec<SortItem>,
    },

    /// Skip the first N rows.
    Skip {
        input: Box<LogicalPlan>,
        count: Expression,
    },

    /// Limit output to N rows.
    Limit {
        input: Box<LogicalPlan>,
        count: Expression,
    },

    /// No-op: produces a single empty row (used as input seed).
    EmptyRow,

    /// WITH clause: project and optionally filter/sort/paginate within a pipeline.
    With {
        input: Box<LogicalPlan>,
        items: Vec<ReturnItem>,
        distinct: bool,
        where_predicate: Option<Expression>,
    },

    /// UNWIND: expand a list expression into individual rows.
    Unwind {
        input: Box<LogicalPlan>,
        expression: Expression,
        alias: String,
    },

    /// SET: modify properties or labels on existing entities.
    SetOp {
        input: Box<LogicalPlan>,
        items: Vec<SetItem>,
    },

    /// REMOVE: remove properties or labels from existing entities.
    RemoveOp {
        input: Box<LogicalPlan>,
        items: Vec<RemoveItem>,
    },

    /// DELETE: delete nodes or relationships.
    DeleteOp {
        input: Box<LogicalPlan>,
        expressions: Vec<Expression>,
        detach: bool,
    },

    /// MERGE: match or create a pattern.
    MergeOp {
        input: Box<LogicalPlan>,
        pattern: PatternElement,
        on_create: Option<Vec<SetItem>>,
        on_match: Option<Vec<SetItem>>,
    },

    /// Cartesian product: for each row from `left`, emit it combined with every
    /// row from `right`.  Used to thread prior bindings into a new MATCH scan.
    CartesianProduct {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
    },

    /// Left outer join: for each left row, match compatible right rows using shared
    /// variable bindings.  If no right rows match, emit the left row with right-only
    /// variables set to NULL.  Used for OPTIONAL MATCH.
    LeftOuterJoin {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
    },

    /// Variable-length expand: traverse min..max hops along relationships.
    VarLengthExpand {
        input: Box<LogicalPlan>,
        src_variable: String,
        rel_variable: Option<String>,
        dst_variable: String,
        rel_types: Vec<String>,
        direction: Direction,
        min_hops: u64,
        max_hops: Option<u64>,
    },

    /// UNION / UNION ALL: combine results of two sub-plans.
    Union {
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
        all: bool,
    },

    /// CREATE CONSTRAINT: register a constraint on the storage layer.
    CreateConstraint {
        label: String,
        property: String,
        constraint_type: ConstraintType,
    },
}
