//! Source facts, as read from PostgreSQL's own catalog and the Trino connector (RFC 0192 §1).

use serde::{Deserialize, Serialize};

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
    /// `format_type(atttypid, atttypmod)`, e.g. `numeric(5,2)`, `timestamp with time zone`.
    pub pg_type: String,
    pub kind: TypeKind,
    pub not_null: bool,
    /// The connector's type for this column, or `None` when the connector does not expose it.
    pub trino_type: Option<String>,
    /// From a P1 profile, for an unconstrained `numeric` (RFC 0157): precision and scale the data
    /// actually uses.
    pub numeric_used: Option<(u32, u32)>,
}

/// A `BEFORE UPDATE … FOR EACH ROW` trigger whose function assigns `NEW.<column>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTrigger {
    pub trigger: String,
    pub function: String,
    pub assigns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTable {
    pub schema: String,
    pub name: String,
    pub columns: Vec<SourceColumn>,
    pub primary_key: Vec<String>,
    /// `reltuples`, or an exact count when one was taken.
    pub row_estimate: i64,
    pub update_triggers: Vec<UpdateTrigger>,
}

impl SourceTable {
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    pub fn column(&self, name: &str) -> Option<&SourceColumn> {
        self.columns.iter().find(|c| c.name == name)
    }
}
