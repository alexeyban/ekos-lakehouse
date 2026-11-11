//! The plan: every table's landing, mode and mappings, with the evidence (RFC 0192 §5). Written
//! to the pipeline repository as `lakehouse.lock.json`, it is also M3's "last deployed snapshot".

use crate::mode::{ModeDecision, ModeSettings, choose_with};
use crate::model::{SourceEngine, SourceTable, WriteFacts};
use crate::typemap::{Lossiness, Mapping};
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
            source_engine: SourceEngine::POSTGRES,
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

/// EKOS RFC 0231: what keeps a project's silver and gold apart from another project's on the same
/// catalog — its namespace prefix without the default `bronze_` lead (`bronze_sales_` → `sales_`).
/// Empty for the default prefix, so a project on the defaults keeps `silver_<schema>` and `gold`.
pub fn layer_tag(namespace_prefix: &str) -> &str {
    namespace_prefix
        .strip_prefix("bronze_")
        .unwrap_or(namespace_prefix)
}

/// EKOS RFC 0231: the project's gold schema — `gold`, or `gold_<tag>` (`gold_sales`).
pub fn gold_schema(namespace_prefix: &str) -> String {
    match layer_tag(namespace_prefix).trim_end_matches('_') {
        "" => "gold".into(),
        tag => format!("gold_{tag}"),
    }
}

/// 2: table and column facts for silver (RFC 0193). 3: OIDs, attnums, defaults (RFC 0194).
pub const FORMAT: u32 = 3;

/// Every table's plan, sorted by (schema, table). A table with any refused column is refused
/// whole: bronze never drops a column.
pub fn plan(sources: &[SourceTable], settings: &PlanSettings) -> Plan {
    plan_with(sources, settings, &BTreeMap::new())
}

/// [`plan`], with EKOS's write-behaviour facts by `schema.table` (RFC 0225), for a built-in
/// engine. An engine this crate does not define refuses every table, naming the engine: plan an
/// out-of-tree engine with [`plan_for`] and its spec.
pub fn plan_with(
    sources: &[SourceTable],
    settings: &PlanSettings,
    facts: &BTreeMap<String, WriteFacts>,
) -> Plan {
    match crate::engine::builtin(&settings.source_engine) {
        Some(spec) => plan_for(spec, sources, settings, facts),
        None => {
            let reason = crate::engine::not_in_this_build(
                &settings.source_engine,
                &crate::engine::builtins().map(|s| s.engine()),
            );
            let mut sorted: Vec<&SourceTable> = sources.iter().collect();
            sorted.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
            Plan {
                format: FORMAT,
                settings: settings.clone(),
                tables: vec![],
                refused: sorted
                    .into_iter()
                    .map(|t| RefusedTable {
                        schema: t.schema.clone(),
                        table: t.name.clone(),
                        oid: t.oid,
                        reasons: vec![reason.clone()],
                    })
                    .collect(),
            }
        }
    }
}

/// The plan of `sources` with `spec` deciding every engine rule (RFC 0255 §3.2): the type map and
/// which columns can order a watermark. `settings.source_engine` is recorded in the lock as it is,
/// and must name `spec`'s engine.
pub fn plan_for(
    spec: &dyn crate::engine::EngineSpec,
    sources: &[SourceTable],
    settings: &PlanSettings,
    facts: &BTreeMap<String, WriteFacts>,
) -> Plan {
    debug_assert_eq!(
        spec.engine(),
        settings.source_engine,
        "the spec plans its own engine"
    );
    let mut sorted: Vec<&SourceTable> = sources.iter().collect();
    sorted.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    let mut out = Plan {
        format: FORMAT,
        settings: settings.clone(),
        tables: vec![],
        refused: vec![],
    };
    for t in sorted {
        let columns: Vec<Mapping> = t.columns.iter().map(|c| spec.map_column(c)).collect();
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
            decision: choose_with(t, &settings.modes, spec, facts.get(&t.qualified())),
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

    /// EKOS RFC 0231: the default prefix tags nothing; a project's own prefix tags its silver and
    /// gold, so two projects on one catalog never write into each other's tables.
    #[test]
    fn a_project_prefix_names_its_own_silver_and_gold() {
        assert_eq!(layer_tag("bronze_"), "");
        assert_eq!(gold_schema("bronze_"), "gold");
        assert_eq!(layer_tag("bronze_sales_"), "sales_");
        assert_eq!(gold_schema("bronze_sales_"), "gold_sales");
        assert_eq!(layer_tag("raw_"), "raw_");
        assert_eq!(gold_schema("raw_"), "gold_raw");
    }

    fn one_table() -> SourceTable {
        SourceTable {
            schema: "s".into(),
            name: "t".into(),
            oid: 7,
            columns: vec![crate::model::SourceColumn {
                name: "ID".into(),
                pg_type: "WEIRDINT".into(),
                kind: crate::model::TypeKind::Base,
                not_null: true,
                trino_type: Some("bigint".into()),
                numeric_used: None,
                comment: None,
                enum_labels: vec![],
                pii_candidate: None,
                infinite_values: None,
                attnum: 1,
                has_default: false,
            }],
            primary_key: vec!["ID".into()],
            row_estimate: 10,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }
    }

    /// RFC 0255 §3.1: a plan for an engine this build lacks refuses every table by the engine's
    /// name, never falls back to another engine's type map.
    #[test]
    fn an_engine_this_build_lacks_refuses_every_table_by_name() {
        let s = PlanSettings {
            source_engine: SourceEngine::new("fixture").unwrap(),
            ..Default::default()
        };
        let p = plan(&[one_table()], &s);
        assert!(p.tables.is_empty());
        assert_eq!(p.refused.len(), 1);
        assert!(
            p.refused[0].reasons[0].starts_with("source engine `fixture` is not in this build"),
            "{:?}",
            p.refused[0].reasons
        );
    }

    /// RFC 0255 §3.2: an out-of-tree spec decides the type map and the watermark rule.
    #[test]
    fn an_out_of_tree_spec_plans_its_own_types() {
        struct Fixture;
        impl crate::engine::EngineSpec for Fixture {
            fn engine(&self) -> SourceEngine {
                SourceEngine::from_static("fixture")
            }
            fn map_column(&self, c: &crate::model::SourceColumn) -> Mapping {
                let mut m = crate::typemap::map_column(&crate::model::SourceColumn {
                    pg_type: "bigint".into(),
                    ..c.clone()
                });
                m.pg_type = c.pg_type.clone();
                m
            }
            fn orderable(&self, t: &str) -> bool {
                t == "WEIRDINT"
            }
        }
        let mut s = PlanSettings {
            source_engine: SourceEngine::from_static("fixture"),
            ..Default::default()
        };
        s.modes.watermarks.insert("s.t".into(), "ID".into());
        let p = plan_for(&Fixture, &[one_table()], &s, &BTreeMap::new());
        assert_eq!(p.tables.len(), 1, "{:?}", p.refused);
        assert_eq!(p.tables[0].columns[0].iceberg_type, "bigint");
        let text = serde_json::to_string(&p.settings).unwrap();
        assert!(text.contains("\"source-engine\":\"fixture\""), "{text}");
    }

    /// RFC 0216: a PostgreSQL lock does not name its engine, so it is byte-identical to one
    /// written before the seam; an Oracle lock does, and reads back as Oracle.
    #[test]
    fn only_a_non_default_engine_is_written() {
        let pg = serde_json::to_string(&PlanSettings::default()).unwrap();
        assert!(!pg.contains("source-engine"), "{pg}");
        let oracle = PlanSettings {
            source_engine: SourceEngine::ORACLE,
            ..Default::default()
        };
        let text = serde_json::to_string(&oracle).unwrap();
        assert!(text.contains("\"source-engine\":\"oracle\""), "{text}");
        let back: PlanSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.source_engine, SourceEngine::ORACLE);
        let old: PlanSettings = serde_json::from_str(&pg).unwrap();
        assert_eq!(old.source_engine, SourceEngine::POSTGRES);
    }
}
