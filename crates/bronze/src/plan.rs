//! The plan: every table's landing, mode and mappings, with the evidence (RFC 0192 §5). Written
//! to the pipeline repository as `lakehouse.lock.json`, it is also M3's "last deployed snapshot".

use crate::mode::{ModeDecision, ModeSettings, choose_with};
use crate::model::{SourceEngine, SourceTable, WriteFacts};
use crate::typemap::{Lossiness, Mapping, map_column};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    /// Cron for the bronze job, and for Iceberg maintenance.
    pub bronze_schedule: String,
    pub maintenance_schedule: String,
    /// RFC 0216. Not written for PostgreSQL, so a PostgreSQL lock file is unchanged.
    #[serde(skip_serializing_if = "SourceEngine::is_postgres")]
    pub source_engine: SourceEngine,
}

impl Default for PlanSettings {
    fn default() -> Self {
        Self {
            source_catalog: "postgresql".into(),
            target_catalog: "iceberg".into(),
            namespace_prefix: "bronze_".into(),
            modes: ModeSettings::default(),
            bronze_schedule: "0 * * * *".into(),
            maintenance_schedule: "0 3 * * *".into(),
            source_engine: SourceEngine::Postgres,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TablePlan {
    pub schema: String,
    pub table: String,
    /// The source table's OID (RFC 0194 §1).
    #[serde(default)]
    pub oid: u32,
    pub namespace: String,
    pub primary_key: Vec<String>,
    pub decision: ModeDecision,
    pub columns: Vec<Mapping>,
    /// The source facts silver is generated from (RFC 0193): keys, comments, labels, candidates.
    #[serde(default)]
    pub facts: TableFacts,
    /// Approved column renames, oldest first (RFC 0194): bronze renames the column in place, so
    /// the rows already loaded keep their values. Each applies once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<ColumnRename>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnRename {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<crate::model::ForeignKey>,
    pub columns: Vec<ColumnFacts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnFacts {
    pub name: String,
    /// `attnum` (RFC 0194 §1).
    #[serde(default)]
    pub attnum: i32,
    pub not_null: bool,
    #[serde(default)]
    pub has_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii_candidate: Option<String>,
    /// RFC 0227: a check constraint listing the column's values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<crate::model::ValueList>,
    /// RFC 0227: what the application model says about the column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<crate::model::CodeFacts>,
}

/// EKOS RFC 0227: the Iceberg table's name. Trino reads `t$x` as a metadata table of `t` (`t$files`),
/// so a source table named with `$` (`CREATE$JAVA$LOB$TABLE`) could be created but never found again:
/// it takes [`safe_name`]. Every other name is kept as it is. The Dagster template mirrors it.
pub fn iceberg_name(name: &str) -> String {
    if name.contains('$') {
        safe_name(name)
    } else {
        name.to_string()
    }
}

/// EKOS RFC 0227: `name` as Dagster and dbt accept it (`[A-Za-z0-9_]`). Oracle allows `$` and
/// `#` (`CREATE$JAVA$LOB$TABLE`): each becomes `_`, and a changed name gets the FNV-1a hash of the
/// original so two names cannot meet. The Dagster template mirrors it (`_safe`).
pub fn safe_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe == name {
        return safe;
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{safe}_{h:08x}")
}

/// RFC 0227: each check list onto its column's facts (matched exactly: the catalog's names).
pub fn attach_checks(plan: &mut Plan, checks: &[crate::model::ColumnCheck]) {
    for c in checks {
        let Some(t) = plan
            .tables
            .iter_mut()
            .find(|t| t.schema == c.schema && t.table == c.table)
        else {
            continue;
        };
        if let Some(f) = t.facts.columns.iter_mut().find(|f| f.name == c.column) {
            f.check = Some(c.list.clone());
        }
    }
}

/// RFC 0227: the application model's facts, by lowercase table then lowercase column, onto the
/// plan's columns. The model names tables without a schema.
pub fn attach_code(
    plan: &mut Plan,
    code: &BTreeMap<String, BTreeMap<String, crate::model::CodeFacts>>,
) {
    for t in &mut plan.tables {
        let Some(cols) = code.get(&t.table.to_ascii_lowercase()) else {
            continue;
        };
        for f in &mut t.facts.columns {
            if let Some(c) = cols
                .get(&f.name.to_ascii_lowercase())
                .filter(|c| !c.is_empty())
            {
                f.code = Some(c.clone());
            }
        }
    }
}

impl TablePlan {
    pub fn column_facts(&self, name: &str) -> Option<&ColumnFacts> {
        self.facts.columns.iter().find(|c| c.name == name)
    }

    /// `live` as it replaces `deployed`: the renames already approved, then each column whose
    /// `attnum` is the same but whose name changed. An `attnum` of 0 is unknown (a lock written
    /// before format 3) and never matches.
    pub fn succeeding(deployed: &TablePlan, live: &TablePlan) -> TablePlan {
        let mut out = live.clone();
        out.renames = deployed.renames.clone();
        for f in &live.facts.columns {
            let renamed = deployed
                .facts
                .columns
                .iter()
                .find(|d| f.attnum > 0 && d.attnum == f.attnum && d.name != f.name);
            if let Some(d) = renamed {
                let r = ColumnRename {
                    from: d.name.clone(),
                    to: f.name.clone(),
                };
                if !out.renames.contains(&r) {
                    out.renames.push(r);
                }
            }
        }
        out
    }
}

/// `plan`, read fresh from the source, as it replaces `deployed`: each table keeps the renames
/// already approved and records the columns renamed since (by table OID and `attnum`).
pub fn carry_renames(plan: &mut Plan, deployed: &Plan) {
    for t in &mut plan.tables {
        if let Some(d) = deployed
            .tables
            .iter()
            .find(|d| d.oid != 0 && d.oid == t.oid)
        {
            *t = TablePlan::succeeding(d, t);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedTable {
    pub schema: String,
    pub table: String,
    #[serde(default)]
    pub oid: u32,
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

/// 2: table and column facts for silver (RFC 0193). 3: OIDs, attnums, defaults (RFC 0194).
pub const FORMAT: u32 = 3;

/// Every table's plan, sorted by (schema, table). A table with any refused column is refused
/// whole: bronze never drops a column.
pub fn plan(sources: &[SourceTable], settings: &PlanSettings) -> Plan {
    plan_with(sources, settings, &BTreeMap::new())
}

/// [`plan`], with EKOS's write-behaviour facts by `schema.table` (RFC 0225).
pub fn plan_with(
    sources: &[SourceTable],
    settings: &PlanSettings,
    facts: &BTreeMap<String, WriteFacts>,
) -> Plan {
    let mut sorted: Vec<&SourceTable> = sources.iter().collect();
    sorted.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    let mut out = Plan {
        format: FORMAT,
        settings: settings.clone(),
        tables: vec![],
        refused: vec![],
    };
    for t in sorted {
        let columns: Vec<Mapping> = t
            .columns
            .iter()
            .map(|c| match settings.source_engine {
                SourceEngine::Postgres => map_column(c),
                SourceEngine::Oracle => crate::typemap_oracle::map_column(c),
            })
            .collect();
        let reasons: Vec<String> = columns
            .iter()
            .filter(|m| m.lossiness == Lossiness::Refused)
            .map(|m| format!("{} ({}): {}", m.column, m.pg_type, m.rationale))
            .collect();
        if !reasons.is_empty() {
            out.refused.push(RefusedTable {
                schema: t.schema.clone(),
                table: t.name.clone(),
                oid: t.oid,
                reasons,
            });
            continue;
        }
        out.tables.push(TablePlan {
            schema: t.schema.clone(),
            table: t.name.clone(),
            oid: t.oid,
            namespace: format!("{}{}", settings.namespace_prefix, t.schema),
            primary_key: t.primary_key.clone(),
            decision: choose_with(
                t,
                &settings.modes,
                settings.source_engine,
                facts.get(&t.qualified()),
            ),
            columns,
            facts: TableFacts {
                comment: t.comment.clone(),
                foreign_keys: t.foreign_keys.clone(),
                columns: t
                    .columns
                    .iter()
                    .map(|c| ColumnFacts {
                        name: c.name.clone(),
                        attnum: c.attnum,
                        not_null: c.not_null,
                        has_default: c.has_default,
                        comment: c.comment.clone(),
                        enum_labels: c.enum_labels.clone(),
                        pii_candidate: c.pii_candidate.clone(),
                        check: None,
                        code: None,
                    })
                    .collect(),
            },
            renames: vec![],
        });
    }
    out
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    /// RFC 0216: a PostgreSQL lock does not name its engine, so it is byte-identical to one
    /// written before the seam; an Oracle lock does, and reads back as Oracle.
    #[test]
    fn only_a_non_default_engine_is_written() {
        let pg = serde_json::to_string(&PlanSettings::default()).unwrap();
        assert!(!pg.contains("source-engine"), "{pg}");
        let oracle = PlanSettings {
            source_engine: SourceEngine::Oracle,
            ..Default::default()
        };
        let text = serde_json::to_string(&oracle).unwrap();
        assert!(text.contains("\"source-engine\":\"oracle\""), "{text}");
        let back: PlanSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.source_engine, SourceEngine::Oracle);
        let old: PlanSettings = serde_json::from_str(&pg).unwrap();
        assert_eq!(old.source_engine, SourceEngine::Postgres);
    }
}
