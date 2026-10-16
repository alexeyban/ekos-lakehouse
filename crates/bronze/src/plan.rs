//! The plan: every table's landing, mode and mappings, with the evidence (RFC 0192 §5). Written
//! to the pipeline repository as `lakehouse.lock.json`, it is also M3's "last deployed snapshot".

use crate::mode::{ModeDecision, ModeSettings, choose};
use crate::model::SourceTable;
use crate::typemap::{Lossiness, Mapping, map_column};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct PlanSettings {
    /// The Trino catalog the source is read through.
    pub source_catalog: String,
    /// The Iceberg catalog bronze is written to.
    pub target_catalog: String,
    /// Bronze namespace = prefix + source schema.
    pub namespace_prefix: String,
    #[serde(flatten)]
    pub modes: ModeSettings,
}

impl Default for PlanSettings {
    fn default() -> Self {
        Self {
            source_catalog: "postgresql".into(),
            target_catalog: "iceberg".into(),
            namespace_prefix: "bronze_".into(),
            modes: ModeSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TablePlan {
    pub schema: String,
    pub table: String,
    pub namespace: String,
    pub primary_key: Vec<String>,
    pub decision: ModeDecision,
    pub columns: Vec<Mapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedTable {
    pub schema: String,
    pub table: String,
    /// One per refused column.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The format of this file; bumped when its meaning changes.
    pub format: u32,
    pub settings: PlanSettings,
    pub tables: Vec<TablePlan>,
    pub refused: Vec<RefusedTable>,
}

pub const FORMAT: u32 = 1;

/// Every table's plan, sorted by (schema, table). A table with any refused column is refused
/// whole: bronze never drops a column.
pub fn plan(sources: &[SourceTable], settings: &PlanSettings) -> Plan {
    let mut sorted: Vec<&SourceTable> = sources.iter().collect();
    sorted.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    let mut out = Plan {
        format: FORMAT,
        settings: settings.clone(),
        tables: vec![],
        refused: vec![],
    };
    for t in sorted {
        let columns: Vec<Mapping> = t.columns.iter().map(map_column).collect();
        let reasons: Vec<String> = columns
            .iter()
            .filter(|m| m.lossiness == Lossiness::Refused)
            .map(|m| format!("{} ({}): {}", m.column, m.pg_type, m.rationale))
            .collect();
        if !reasons.is_empty() {
            out.refused.push(RefusedTable {
                schema: t.schema.clone(),
                table: t.name.clone(),
                reasons,
            });
            continue;
        }
        out.tables.push(TablePlan {
            schema: t.schema.clone(),
            table: t.name.clone(),
            namespace: format!("{}{}", settings.namespace_prefix, t.schema),
            primary_key: t.primary_key.clone(),
            decision: choose(t, &settings.modes),
            columns,
        });
    }
    out
}
