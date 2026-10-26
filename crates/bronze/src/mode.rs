//! RFC 0192 §3: how a table is loaded, chosen from facts and recorded with them.

use crate::model::{SourceEngine, SourceTable, WriteFacts};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum LoadMode {
    /// `CREATE OR REPLACE TABLE … AS SELECT`: the whole table, one atomic commit per run.
    Full,
    /// Rows whose `watermark` is at or after the highest one already loaded; with a key, a key-set
    /// diff appends a tombstone for each key the source no longer has.
    Incremental {
        watermark: String,
        keyset_deletes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ModeSettings {
    /// `"schema.table" = "column"`: a column the team says moves on every write.
    pub watermarks: BTreeMap<String, String>,
    /// Above this many rows, a table loaded in full is called out in the plan.
    pub full_refresh_max_rows: i64,
    /// RFC 0225 §2.2: the column the application sets on every write it makes (iDempiere:
    /// `UPDATED`). Used only where EKOS's write-behaviour facts say the code keeps it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_watermark: Option<String>,
    /// Below this many rows, a table stays in full even with an application watermark.
    #[serde(skip_serializing_if = "is_zero")]
    pub incremental_min_rows: i64,
    /// Days between delete audits of tables the code never hard-deletes (RFC 0225 §2.3).
    #[serde(skip_serializing_if = "is_default_audit_days")]
    pub delete_audit_days: u32,
    /// `"schema.table" = "retain" | "propagate"`, for tables only purged (RFC 0225 §2.3).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub purge_policy: BTreeMap<String, PurgePolicy>,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

const DELETE_AUDIT_DAYS: u32 = 7;

fn is_default_audit_days(n: &u32) -> bool {
    *n == DELETE_AUDIT_DAYS
}

impl Default for ModeSettings {
    fn default() -> Self {
        Self {
            watermarks: BTreeMap::new(),
            full_refresh_max_rows: 5_000_000,
            application_watermark: None,
            incremental_min_rows: 0,
            delete_audit_days: DELETE_AUDIT_DAYS,
            purge_policy: BTreeMap::new(),
        }
    }
}

/// What a purge found in the source does to the lakehouse (plan III.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurgePolicy {
    /// The rows stay in the lakehouse, marked purged at the source: history outlives the OLTP
    /// retention.
    Retain,
    /// The rows leave the lakehouse's current state, as a delete would.
    Propagate,
}

/// How often deletes are looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "every")]
pub enum Cadence {
    Run,
    /// A periodic check of a claim the code makes (no hard deletes).
    Audit {
        days: u32,
    },
}

/// RFC 0225 §2.3: how a table's hard deletes are found and what they become.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletePlan {
    /// `bucketed_key_diff` (`lakehouse deletes`).
    pub method: String,
    pub cadence: Cadence,
    /// `delete` or `purge`: what a found key is recorded as.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purge_policy: Option<PurgePolicy>,
    /// The code says this table is never hard-deleted: a found delete contradicts it.
    #[serde(default)]
    pub contradicts_code: bool,
    /// The write-behaviour classes it rests on.
    pub classes: Vec<String>,
}

pub const BUCKETED_KEY_DIFF: &str = "bucketed_key_diff";

const DELETED: &[&str] = &["deleted_by_app", "deleted_by_users", "deleted_by_cascade"];

/// The delete plan the write-behaviour classes call for.
pub fn delete_plan(qualified: &str, facts: &WriteFacts, s: &ModeSettings) -> DeletePlan {
    let has = |c: &str| facts.delete_classes.iter().any(|x| x == c);
    let mut p = DeletePlan {
        method: BUCKETED_KEY_DIFF.into(),
        cadence: Cadence::Run,
        kind: "delete".into(),
        purge_policy: None,
        contradicts_code: false,
        classes: facts.delete_classes.clone(),
    };
    if DELETED.iter().any(|c| has(c)) {
        return p;
    }
    if has("purged") {
        p.kind = "purge".into();
        p.purge_policy = Some(
            s.purge_policy
                .get(qualified)
                .copied()
                .unwrap_or(PurgePolicy::Retain),
        );
        return p;
    }
    // soft_delete_only, never_deleted_in_code, or no class: the claim is audited.
    p.cadence = Cadence::Audit {
        days: s.delete_audit_days,
    };
    p.contradicts_code = true;
    p
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeDecision {
    pub mode: LoadMode,
    /// The facts the decision rests on.
    pub evidence: Vec<String>,
    /// What a person should know: a declared watermark that was not proven, a large full load.
    pub notes: Vec<String>,
    /// RFC 0225: how deletes are found, for an incremental table planned from write behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletes: Option<DeletePlan>,
}

/// Types a watermark can be compared on.
pub fn orderable(engine: SourceEngine, source_type: &str) -> bool {
    match engine {
        SourceEngine::Postgres => {
            source_type.starts_with("timestamp")
                || matches!(source_type, "date" | "integer" | "bigint")
        }
        // A DATE carries a time; an integer NUMBER orders like one.
        SourceEngine::Oracle => {
            source_type == "DATE"
                || source_type.starts_with("TIMESTAMP")
                || source_type == "INTEGER"
                || (source_type.starts_with("NUMBER(") && source_type.ends_with(",0)"))
        }
    }
}

/// [`choose_for`] a PostgreSQL source.
pub fn choose(t: &SourceTable, s: &ModeSettings) -> ModeDecision {
    choose_for(t, s, SourceEngine::Postgres)
}

pub fn choose_for(t: &SourceTable, s: &ModeSettings, engine: SourceEngine) -> ModeDecision {
    choose_with(t, s, engine, None)
}

/// [`choose_for`], with EKOS's write-behaviour facts for the table when the ledger has them
/// (RFC 0225).
pub fn choose_with(
    t: &SourceTable,
    s: &ModeSettings,
    engine: SourceEngine,
    facts: Option<&WriteFacts>,
) -> ModeDecision {
    let mut d = by_trigger(t, s, engine);
    if let (LoadMode::Full, Some(f)) = (&d.mode, facts) {
        by_application(&mut d, t, s, engine, f);
    }
    if let (LoadMode::Incremental { .. }, Some(f), false) =
        (&d.mode, facts, t.primary_key.is_empty())
    {
        let p = delete_plan(&t.qualified(), f, s);
        d.evidence.push(format!(
            "deletes: {} ({}) by bucketed key diff, {}",
            match p.cadence {
                Cadence::Run => "every run".to_string(),
                Cadence::Audit { days } => format!("audited every {days} days"),
            },
            if p.classes.is_empty() {
                "no delete class".to_string()
            } else {
                p.classes.join(", ")
            },
            if p.contradicts_code {
                "a delete found contradicts the code".to_string()
            } else {
                format!("recorded as {}", p.kind)
            }
        ));
        d.deletes = Some(p);
        // `lakehouse deletes` finds them; the full key-set diff is not generated as well.
        if let LoadMode::Incremental { keyset_deletes, .. } = &mut d.mode {
            *keyset_deletes = false;
        }
    }
    if matches!(d.mode, LoadMode::Full) {
        d.evidence.push(format!("about {} rows", t.row_estimate));
        if t.row_estimate > s.full_refresh_max_rows {
            d.notes.push(format!(
                "more than {} rows loaded in full on every run: declare and prove a watermark",
                s.full_refresh_max_rows
            ));
        }
    }
    d
}

/// RFC 0225 §2.2: the application keeps `application-watermark` on every write it makes.
fn by_application(
    d: &mut ModeDecision,
    t: &SourceTable,
    s: &ModeSettings,
    engine: SourceEngine,
    f: &WriteFacts,
) {
    let Some(w) = &s.application_watermark else {
        return;
    };
    let Some(c) = t.columns.iter().find(|c| c.name.eq_ignore_ascii_case(w)) else {
        return;
    };
    if !matches!(f.update_class.as_str(), "po_only" | "raw_sql_sets_updated") {
        d.notes.push(format!(
            "update class {}: some code updates the table without setting {}, so it is loaded in \
             full",
            f.update_class, c.name
        ));
        return;
    }
    if !orderable(engine, &c.pg_type) {
        d.notes.push(format!(
            "{} is {}, which cannot order writes",
            c.name, c.pg_type
        ));
        return;
    }
    if t.primary_key.is_empty() {
        d.notes.push(format!(
            "{} is kept by the application, but without a primary key deletes could not be \
             found: loaded in full",
            c.name
        ));
        return;
    }
    if t.row_estimate < s.incremental_min_rows {
        return;
    }
    d.evidence.push(format!(
        "update class {}: the application sets {} on every write it makes (EKOS write behaviour, \
         RFC 0212)",
        f.update_class, c.name
    ));
    d.evidence.push(format!(
        "an update made outside the application does not set {}: the load misses it and the \
         next reconciliation shows it",
        c.name
    ));
    d.mode = LoadMode::Incremental {
        watermark: c.name.clone(),
        keyset_deletes: false,
    };
}

fn by_trigger(t: &SourceTable, s: &ModeSettings, engine: SourceEngine) -> ModeDecision {
    let mut d = ModeDecision {
        mode: LoadMode::Full,
        evidence: vec![],
        notes: vec![],
        deletes: None,
    };
    if let Some(w) = s.watermarks.get(&t.qualified()) {
        match t.column(w) {
            None => d.notes.push(format!(
                "declared watermark {w} is not a column of {}",
                t.qualified()
            )),
            Some(c) if !orderable(engine, &c.pg_type) => d.notes.push(format!(
                "declared watermark {w} is {}, which cannot order writes",
                c.pg_type
            )),
            Some(_) => {
                let proof = t
                    .update_triggers
                    .iter()
                    .find(|tr| tr.assigns.iter().any(|a| a == w));
                match proof {
                    Some(tr) => {
                        d.evidence
                            .push(format!("{w} declared as the watermark in settings"));
                        d.evidence.push(format!(
                            "trigger {} runs {} before every update and assigns NEW.{w}",
                            tr.trigger, tr.function
                        ));
                        if !tr.body_sha.is_empty() {
                            // A changed routine is a change (RFC 0194): the proof names its body.
                            d.evidence.push(format!(
                                "{} body sha256 {}",
                                tr.function,
                                &tr.body_sha[..tr.body_sha.len().min(16)]
                            ));
                        }
                        let keyed = !t.primary_key.is_empty();
                        if keyed {
                            d.evidence.push(format!(
                                "primary key ({}) allows a key-set diff for deletes",
                                t.primary_key.join(", ")
                            ));
                        } else {
                            d.notes.push(
                                "no primary key: deletes in the source are not detected".into(),
                            );
                        }
                        d.mode = LoadMode::Incremental {
                            watermark: w.clone(),
                            keyset_deletes: keyed,
                        };
                    }
                    None => d.notes.push(format!(
                        "{w} is declared, but no BEFORE UPDATE row trigger assigns NEW.{w}: \
                         an update that does not touch it would be missed, so the table is \
                         loaded in full"
                    )),
                }
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SourceColumn, TypeKind, UpdateTrigger, WriteFacts};

    fn table(triggers: Vec<UpdateTrigger>, pk: &[&str], rows: i64) -> SourceTable {
        let c = |n: &str, t: &str| SourceColumn {
            name: n.into(),
            pg_type: t.into(),
            kind: TypeKind::Base,
            not_null: true,
            trino_type: None,
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 0,
            has_default: false,
        };
        SourceTable {
            schema: "public".into(),
            oid: 0,
            name: "film".into(),
            columns: vec![
                c("film_id", "integer"),
                c("title", "text"),
                c("last_update", "timestamp with time zone"),
            ],
            primary_key: pk.iter().map(|s| s.to_string()).collect(),
            row_estimate: rows,
            update_triggers: triggers,
            foreign_keys: vec![],
            comment: None,
        }
    }

    fn pagila_trigger() -> UpdateTrigger {
        UpdateTrigger {
            trigger: "last_updated".into(),
            function: "public.last_updated()".into(),
            assigns: vec!["last_update".into()],
            body_sha: String::new(),
        }
    }

    fn declared(col: &str) -> ModeSettings {
        ModeSettings {
            watermarks: [("public.film".to_string(), col.to_string())].into(),
            ..Default::default()
        }
    }

    fn idempiere(rows: i64) -> SourceTable {
        let mut t = table(vec![], &["C_ORDER_ID"], rows);
        t.schema = "ADEMPIERE".into();
        t.name = "C_ORDER".into();
        t.columns[0].name = "C_ORDER_ID".into();
        t.columns[0].pg_type = "NUMBER(10,0)".into();
        t.columns[2].name = "UPDATED".into();
        t.columns[2].pg_type = "DATE".into();
        t
    }

    fn facts(update: &str, deletes: &[&str]) -> WriteFacts {
        WriteFacts {
            update_class: update.into(),
            delete_classes: deletes.iter().map(|s| s.to_string()).collect(),
            change_log: None,
        }
    }

    fn app() -> ModeSettings {
        ModeSettings {
            application_watermark: Some("Updated".into()),
            ..Default::default()
        }
    }

    fn oracle(t: &SourceTable, s: &ModeSettings, f: &WriteFacts) -> ModeDecision {
        choose_with(t, s, SourceEngine::Oracle, Some(f))
    }

    /// RFC 0225 §2.2: the application's own column, where the code keeps it.
    #[test]
    fn an_application_watermark_needs_the_code_to_keep_it() {
        let t = idempiere(10);
        let d = oracle(&t, &app(), &facts("po_only", &["deleted_by_users"]));
        assert_eq!(
            d.mode,
            LoadMode::Incremental {
                watermark: "UPDATED".into(),
                keyset_deletes: false
            }
        );
        assert!(
            d.evidence
                .iter()
                .any(|e| e.contains("update class po_only")),
            "{d:?}"
        );
        assert!(
            d.evidence
                .iter()
                .any(|e| e.contains("outside the application")),
            "{d:?}"
        );

        let skips = oracle(&t, &app(), &facts("raw_sql_skips_updated", &[]));
        assert_eq!(skips.mode, LoadMode::Full);
        assert!(
            skips.notes[0].contains("without setting UPDATED"),
            "{skips:?}"
        );
        assert!(skips.deletes.is_none());

        // Not configured, or no facts: as before.
        let none = oracle(&t, &ModeSettings::default(), &facts("po_only", &[]));
        assert_eq!(none.mode, LoadMode::Full);
        assert_eq!(
            choose_for(&t, &app(), SourceEngine::Oracle).mode,
            LoadMode::Full
        );

        let small = ModeSettings {
            incremental_min_rows: 100,
            ..app()
        };
        assert_eq!(
            oracle(&t, &small, &facts("po_only", &[])).mode,
            LoadMode::Full
        );

        let mut keyless = idempiere(10);
        keyless.primary_key.clear();
        let k = oracle(&keyless, &app(), &facts("po_only", &[]));
        assert_eq!(k.mode, LoadMode::Full);
        assert!(k.notes[0].contains("without a primary key"), "{k:?}");
    }

    /// RFC 0225 §2.3: cadence and kind from the delete classes.
    #[test]
    fn delete_plans_follow_the_delete_classes() {
        let t = idempiere(10);
        let plan = |deletes: &[&str], s: &ModeSettings| {
            oracle(&t, s, &facts("po_only", deletes)).deletes.unwrap()
        };
        let users = plan(&["deleted_by_users", "purged"], &app());
        assert_eq!(
            (users.cadence, users.kind.as_str()),
            (Cadence::Run, "delete")
        );
        assert_eq!(users.method, BUCKETED_KEY_DIFF);
        assert!(!users.contradicts_code);

        let purged = plan(&["purged"], &app());
        assert_eq!(purged.kind, "purge");
        assert_eq!(purged.purge_policy, Some(PurgePolicy::Retain));
        let propagate = ModeSettings {
            purge_policy: [("ADEMPIERE.C_ORDER".to_string(), PurgePolicy::Propagate)].into(),
            ..app()
        };
        assert_eq!(
            plan(&["purged"], &propagate).purge_policy,
            Some(PurgePolicy::Propagate)
        );

        for quiet in [&["soft_delete_only"][..], &["never_deleted_in_code"][..]] {
            let p = plan(quiet, &app());
            assert_eq!(p.cadence, Cadence::Audit { days: 7 });
            assert!(p.contradicts_code);
        }
    }

    /// A trigger-proven watermark keeps its full key-set diff unless facts give a delete plan.
    #[test]
    fn a_delete_plan_replaces_the_full_key_set_diff() {
        let mut t = idempiere(10);
        t.update_triggers = vec![UpdateTrigger {
            trigger: "T".into(),
            function: "T".into(),
            assigns: vec!["UPDATED".into()],
            body_sha: String::new(),
        }];
        let declared = ModeSettings {
            watermarks: [("ADEMPIERE.C_ORDER".to_string(), "UPDATED".to_string())].into(),
            ..Default::default()
        };
        let without = choose_for(&t, &declared, SourceEngine::Oracle);
        assert!(matches!(
            without.mode,
            LoadMode::Incremental {
                keyset_deletes: true,
                ..
            }
        ));
        let with = oracle(
            &t,
            &declared,
            &facts("raw_sql_skips_updated", &["deleted_by_app"]),
        );
        assert!(matches!(
            with.mode,
            LoadMode::Incremental {
                keyset_deletes: false,
                ..
            }
        ));
        assert!(with.deletes.is_some());
    }

    /// A PostgreSQL lock's settings do not change shape.
    #[test]
    fn default_mode_settings_serialize_as_before() {
        let v = serde_json::to_value(ModeSettings::default()).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["full-refresh-max-rows", "watermarks"]);
    }

    /// RFC 0216: an Oracle DATE orders writes (it carries a time); a VARCHAR2 does not.
    #[test]
    fn oracle_watermark_types() {
        let mut t = table(vec![], &["C_ORDER_ID"], 10);
        t.schema = "ADEMPIERE".into();
        t.name = "C_ORDER".into();
        t.columns[2].name = "UPDATED".into();
        t.columns[2].pg_type = "DATE".into();
        t.columns[1].name = "DOCUMENTNO".into();
        t.columns[1].pg_type = "VARCHAR2(30 CHAR)".into();
        let trigger = UpdateTrigger {
            trigger: "C_ORDER_TRG".into(),
            function: "C_ORDER_TRG".into(),
            assigns: vec!["UPDATED".into()],
            body_sha: String::new(),
        };
        let at = |col: &str| ModeSettings {
            watermarks: [("ADEMPIERE.C_ORDER".to_string(), col.to_string())].into(),
            ..Default::default()
        };
        t.update_triggers = vec![trigger];
        let d = choose_for(&t, &at("UPDATED"), SourceEngine::Oracle);
        assert!(matches!(d.mode, LoadMode::Incremental { .. }), "{d:?}");
        let d = choose_for(&t, &at("DOCUMENTNO"), SourceEngine::Oracle);
        assert_eq!(d.mode, LoadMode::Full);
        assert!(d.notes[0].contains("cannot order writes"), "{d:?}");
        assert!(orderable(SourceEngine::Oracle, "NUMBER(10,0)"));
        assert!(!orderable(SourceEngine::Oracle, "NUMBER"));
        assert!(orderable(SourceEngine::Oracle, "TIMESTAMP(6)"));
    }

    #[test]
    fn a_declared_and_proven_watermark_is_incremental_with_deletes() {
        let d = choose(
            &table(vec![pagila_trigger()], &["film_id"], 1000),
            &declared("last_update"),
        );
        assert_eq!(
            d.mode,
            LoadMode::Incremental {
                watermark: "last_update".into(),
                keyset_deletes: true
            }
        );
        assert!(
            d.evidence
                .iter()
                .any(|e| e.contains("trigger last_updated")),
            "{d:?}"
        );
    }

    #[test]
    fn declared_but_unproven_is_full_and_says_why() {
        let d = choose(&table(vec![], &["film_id"], 1000), &declared("last_update"));
        assert_eq!(d.mode, LoadMode::Full);
        assert!(d.notes[0].contains("no BEFORE UPDATE row trigger"), "{d:?}");
    }

    #[test]
    fn a_trigger_alone_is_not_a_declaration() {
        let d = choose(
            &table(vec![pagila_trigger()], &["film_id"], 1000),
            &ModeSettings::default(),
        );
        assert_eq!(d.mode, LoadMode::Full);
    }

    #[test]
    fn bad_declarations_are_full_with_a_note() {
        for (col, needle) in [("nope", "not a column"), ("title", "cannot order")] {
            let d = choose(
                &table(vec![pagila_trigger()], &["film_id"], 1),
                &declared(col),
            );
            assert_eq!(d.mode, LoadMode::Full);
            assert!(d.notes[0].contains(needle), "{col}: {d:?}");
        }
    }

    #[test]
    fn no_key_means_no_delete_detection_and_a_note() {
        let d = choose(
            &table(vec![pagila_trigger()], &[], 1000),
            &declared("last_update"),
        );
        assert!(matches!(
            d.mode,
            LoadMode::Incremental {
                keyset_deletes: false,
                ..
            }
        ));
        assert!(d.notes.iter().any(|n| n.contains("deletes")));
    }

    #[test]
    fn a_large_full_load_is_called_out() {
        let d = choose(
            &table(vec![], &["film_id"], 10_000_000),
            &ModeSettings::default(),
        );
        assert_eq!(d.mode, LoadMode::Full);
        assert!(d.notes.iter().any(|n| n.contains("declare and prove")));
    }
}
