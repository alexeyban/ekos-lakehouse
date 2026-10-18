//! The relational IR (RFC 0202).

use ekos_lakehouse_kettle::Connection;
use ekos_lakehouse_kettle_expr::{Expr, Fidelity, KType};
use serde::{Deserialize, Serialize};

/// One field of a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: KType,
    #[serde(default = "minus_one")]
    pub length: i32,
    #[serde(default = "minus_one")]
    pub precision: i32,
}

fn minus_one() -> i32 {
    -1
}

impl Field {
    pub fn new(name: &str, ty: KType) -> Field {
        Field {
            name: name.to_string(),
            ty,
            length: -1,
            precision: -1,
        }
    }
}

/// Where a node's index sits in [`Ir::nodes`].
pub type NodeId = usize;

/// A table a source reads or a sink writes, as the Kettle step names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableRef {
    pub connection: Option<Connection>,
    pub schema: Option<String>,
    pub table: String,
}

/// What a Table input reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceQuery {
    /// The step's SQL after variable substitution, in the connection's dialect, with the
    /// tables it reads.
    Sql {
        text: String,
        tables: Vec<TableRef>,
        /// Table input's row limit (0 in Kettle means none).
        limit: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

/// One value a lookup returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookupValue {
    /// The field in the lookup stream or table.
    pub name: String,
    /// Its name in the output.
    pub rename: String,
    /// Used when no row matches (not when the matched value is null).
    pub default: Option<Expr>,
    pub ty: KType,
}

/// An aggregate of a Group by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agg {
    pub name: String,
    /// `sum`, `avg`, `min`, `max`, `count`, `count_distinct`, `count_all`, `string_agg`,
    /// `first`, `last`, `stddev`, `median`, `percentile`.
    pub func: String,
    pub subject: Option<String>,
    /// The percentile (0–100) or the separator of a concatenation.
    pub arg: Option<String>,
    pub ty: KType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub field: String,
    pub ascending: bool,
    pub case_sensitive: bool,
}

/// A window value: Add sequence, Analytic query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fn", rename_all = "snake_case")]
pub enum WindowFn {
    /// `start + (row_number() - 1) * increment`.
    Sequence {
        start: i64,
        increment: i64,
    },
    Lag {
        subject: String,
        offset: i64,
    },
    Lead {
        subject: String,
        offset: i64,
    },
}

/// How a sink writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinkMode {
    /// Table output: insert every row (truncate first when asked).
    Insert,
    /// Insert / update: update matching rows, insert the rest.
    Upsert,
    /// Update: update matching rows only.
    Update,
    /// Delete matching rows.
    Delete,
    /// Dimension lookup/update with update on: a slowly changing dimension.
    Dimension,
    /// Combination lookup/update: a junk dimension keyed by its natural fields.
    Combination,
}

/// A slowly changing dimension's bookkeeping columns (Dimension lookup/update).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scd {
    pub technical_key: String,
    pub version: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// Per field: `insert` (type 2, new version), `update` / `punch_through` (type 1).
    pub field_modes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sink {
    pub mode: SinkMode,
    pub target: TableRef,
    /// `(stream field, table column)` written.
    pub columns: Vec<(String, String)>,
    /// `(stream field, table column)` matched on.
    pub keys: Vec<(String, String)>,
    pub truncate: bool,
    pub scd: Option<Scd>,
}

/// What a node does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Source {
        query: SourceQuery,
    },
    /// Inline rows (Data grid).
    Values {
        rows: Vec<Vec<Expr>>,
    },
    /// One row repeated `count` times (Generate rows).
    Generate {
        row: Vec<Expr>,
        count: u64,
    },
    /// The output is exactly these fields, in order.
    Project {
        items: Vec<(String, Expr)>,
    },
    Filter {
        predicate: Expr,
    },
    /// Inputs `[left, right]`; output is left's fields then right's.
    Join {
        kind: JoinKind,
        keys: Vec<(String, String)>,
        condition: Option<Expr>,
    },
    /// Inputs `[main]` with `table`, or `[main, lookup stream]`: a left join that returns at
    /// most one lookup row per main row.
    Lookup {
        table: Option<TableRef>,
        keys: Vec<(String, String)>,
        values: Vec<LookupValue>,
        /// A slowly changing dimension read as of a date: `(stream date field — current time
        /// when `None`, valid-from column, valid-to column)`.
        valid_at: Option<(Option<String>, String, String)>,
        /// Drop main rows with no match (Database lookup "do not pass the row").
        inner: bool,
    },
    Aggregate {
        group_by: Vec<String>,
        aggs: Vec<Agg>,
    },
    /// Appends window values to every row.
    Window {
        partition: Vec<String>,
        order: Vec<SortKey>,
        items: Vec<(String, WindowFn)>,
    },
    /// One row per distinct `keys` (all fields when empty), optionally with a count.
    Distinct {
        keys: Vec<String>,
        count: Option<String>,
    },
    Sort {
        keys: Vec<SortKey>,
    },
    Union,
    /// Fails the run when any row reaches it (Abort): a test, not a model.
    Assert {
        message: String,
    },
    Sink {
        sink: Sink,
    },
    Gap {
        reason: String,
    },
}

/// One node: the step it came from, its inputs, output schema and fidelity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelNode {
    pub id: NodeId,
    /// The step's name (inlined sub-transformation steps are `mapping/step`).
    pub step: String,
    pub step_type: String,
    /// For a routed output (Filter rows, Switch/case): the step it feeds.
    pub branch: Option<String>,
    pub op: Op,
    pub inputs: Vec<NodeId>,
    pub schema: Vec<Field>,
    pub fidelity: Fidelity,
    pub notes: Vec<String>,
    /// The step's source line, for evidence.
    pub line: u32,
}

impl RelNode {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.schema.iter().find(|f| f.name == name)
    }

    pub fn is_gap(&self) -> bool {
        matches!(self.op, Op::Gap { .. })
    }
}

/// The IR of one transformation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Ir {
    pub transformation: String,
    pub nodes: Vec<RelNode>,
}

impl Ir {
    pub fn node(&self, id: NodeId) -> &RelNode {
        &self.nodes[id]
    }

    /// Nodes no other node reads.
    pub fn leaves(&self) -> Vec<NodeId> {
        let used: std::collections::BTreeSet<NodeId> = self
            .nodes
            .iter()
            .flat_map(|n| n.inputs.iter().copied())
            .collect();
        self.nodes
            .iter()
            .map(|n| n.id)
            .filter(|id| !used.contains(id))
            .collect()
    }

    /// Every node `id` depends on, itself included.
    pub fn upstream(&self, id: NodeId) -> Vec<NodeId> {
        let mut seen = vec![id];
        let mut i = 0;
        while i < seen.len() {
            for &up in &self.nodes[seen[i]].inputs {
                if !seen.contains(&up) {
                    seen.push(up);
                }
            }
            i += 1;
        }
        seen
    }

    /// Per fidelity, how many nodes (one per step and branch).
    pub fn fidelity_counts(&self) -> std::collections::BTreeMap<Fidelity, usize> {
        let mut m = std::collections::BTreeMap::new();
        for n in &self.nodes {
            *m.entry(n.fidelity).or_default() += 1;
        }
        m
    }
}
