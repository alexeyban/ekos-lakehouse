//! Source facts, as read from PostgreSQL's own catalog and the Trino connector (RFC 0192 §1).

use serde::{Deserialize, Serialize};

/// The engine a source is read from (RFC 0216). It decides the type map, which column types can
/// order a watermark, and the dialect the source side of a reconciliation is rendered in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEngine {
    #[default]
    Postgres,
    Oracle,
}

impl SourceEngine {
    /// The default: a lock file written for PostgreSQL does not name its engine, so it reads back
    /// byte-identical to one written before RFC 0216.
    pub fn is_postgres(&self) -> bool {
        *self == Self::Postgres
    }
}

/// What kind of PostgreSQL type a column has (`pg_type.typtype`, plus arrays).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TypeKind {
    Base,
    /// An enum: its labels are its values.
    Enum,
    /// A domain over `base`, in `format_type` form.
    Domain {
        base: String,
    },
    /// An array whose element type is `element`, in `format_type` form.
    Array {
        element: String,
    },
    /// Anything else (composite, range, multirange, pseudo).
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceColumn {
    pub name: String,
    /// The source engine's type as it spells it: PostgreSQL's `format_type(atttypid, atttypmod)`
    /// (`numeric(5,2)`, `timestamp with time zone`), Oracle's DDL form (`NUMBER(10,0)`,
    /// `VARCHAR2(60 CHAR)`). The field keeps its first name: lock files carry it.
    pub pg_type: String,
    pub kind: TypeKind,
    pub not_null: bool,
    /// The connector's type for this column, or `None` when the connector does not expose it.
    pub trino_type: Option<String>,
    /// From a P1 profile, for an unconstrained `numeric` (RFC 0157): precision and scale the data
    /// actually uses. Precision 0 means it was measured and holds no values (RFC 0216: an Oracle
    /// column, the scale is then [`crate::typemap_oracle::NO_VALUES_SCALE`]).
    pub numeric_used: Option<(u32, u32)>,
    /// `COMMENT ON COLUMN`, redacted (RFC 0043).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// An enum's labels, in their declared order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_labels: Vec<String>,
    /// RFC 0157's name classifier: a candidate, never applied without approval (RFC 0193 §2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii_candidate: Option<String>,
    /// Rows holding `infinity` / `-infinity` in a timestamp or date column, when measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infinite_values: Option<u64>,
    /// `pg_attribute.attnum`: a column's identity across renames (RFC 0194 §1).
    #[serde(default)]
    pub attnum: i32,
    /// Whether the column has a default (a new column with one is not "nullable, no default").
    #[serde(default)]
    pub has_default: bool,
}

/// A declared foreign key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
}

/// A `BEFORE UPDATE … FOR EACH ROW` trigger whose function assigns `NEW.<column>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTrigger {
    pub trigger: String,
    pub function: String,
    pub assigns: Vec<String>,
    /// SHA-256 of the function's source: a changed routine is a change (RFC 0194 §2).
    #[serde(default)]
    pub body_sha: String,
}

/// EKOS's write-behaviour facts for one table (RFC 0212), as the plan uses them (RFC 0225).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFacts {
    pub update_class: String,
    pub delete_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_log: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTable {
    pub schema: String,
    pub name: String,
    /// `pg_class.oid`: a table's identity across renames (RFC 0194 §1).
    #[serde(default)]
    pub oid: u32,
    pub columns: Vec<SourceColumn>,
    pub primary_key: Vec<String>,
    /// `reltuples`, or an exact count when one was taken.
    pub row_estimate: i64,
    pub update_triggers: Vec<UpdateTrigger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    /// `COMMENT ON TABLE`, redacted (RFC 0043).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl SourceTable {
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    pub fn column(&self, name: &str) -> Option<&SourceColumn> {
        self.columns.iter().find(|c| c.name == name)
    }
}
