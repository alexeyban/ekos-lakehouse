//! EKOS RFC 0226 §2.3: an operator's changes to a proposed plan, one table at a time.
//!
//! A change is applied to the proposal on every read, so a re-plan keeps it; one that no longer
//! fits (the table left the plan, lost its key, stopped being purged) is reported, not applied.

use crate::mode::{
    Cadence, DeletePlan, LoadMode, ModeSettings, PurgePolicy, delete_plan, orderable,
};
use crate::model::WriteFacts;
use crate::plan::{Plan, TablePlan};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeChoice {
    Full,
    Incremental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CadenceChoice {
    Run,
    Audit,
}

/// What an operator changes about one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change", content = "to")]
pub enum Change {
    Mode(ModeChoice),
    Deletes(CadenceChoice),
    Purge(PurgePolicy),
}

impl Change {
    /// One change of each kind per table; a newer one replaces the older.
    pub fn kind(&self) -> &'static str {
        match self {
            Change::Mode(_) => "mode",
            Change::Deletes(_) => "deletes",
            Change::Purge(_) => "purge",
        }
    }

    fn order(&self) -> u8 {
        match self {
            Change::Mode(_) => 0,
            Change::Deletes(_) => 1,
            Change::Purge(_) => 2,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Change::Mode(ModeChoice::Full) => "load in full".into(),
            Change::Mode(ModeChoice::Incremental) => "load incrementally".into(),
            Change::Deletes(CadenceChoice::Run) => "look for deletes every run".into(),
            Change::Deletes(CadenceChoice::Audit) => "audit deletes periodically".into(),
            Change::Purge(PurgePolicy::Retain) => "keep purged rows".into(),
            Change::Purge(PurgePolicy::Propagate) => "remove purged rows".into(),
        }
    }
}

/// One recorded change: who made it, when and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableChange {
    /// `schema.table`, as the plan spells it.
    pub table: String,
    #[serde(flatten)]
    pub change: Change,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub by: String,
    pub at: String,
}

/// A change and whether it applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Applied {
    #[serde(flatten)]
    pub change: TableChange,
    /// Why it was not applied: it no longer fits the plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
}

fn qualified(t: &TablePlan) -> String {
    format!("{}.{}", t.schema, t.table)
}

/// The delete plan an operator-chosen incremental table gets: from the facts when there are
/// some, else every run (nothing says how the table loses rows).
fn deletes_for(qualified: &str, facts: Option<&WriteFacts>, modes: &ModeSettings) -> DeletePlan {
    match facts {
        Some(f) => delete_plan(qualified, f, modes),
        None => DeletePlan {
            method: crate::mode::BUCKETED_KEY_DIFF.into(),
            cadence: Cadence::Run,
            kind: "delete".into(),
            purge_policy: None,
            contradicts_code: false,
            classes: vec![],
        },
    }
}

fn who(c: &TableChange) -> String {
    match &c.reason {
        Some(r) => format!("{}: {r}", c.by),
        None => c.by.clone(),
    }
}

/// Apply `c` to its table in `plan`, or say why it does not fit.
pub fn apply(
    plan: &mut Plan,
    facts: &BTreeMap<String, WriteFacts>,
    c: &TableChange,
) -> Result<(), String> {
    let settings = plan.settings.clone();
    let Some(t) = plan.tables.iter_mut().find(|t| qualified(t) == c.table) else {
        return Err(format!(
            "{} is not in the plan (refused, out of scope or no longer in the source)",
            c.table
        ));
    };
    let note = format!("operator ({}) changed: {}", who(c), c.change.describe());
    // The proposal's reasons for the other mode stay readable, as what it said, not as why.
    let set_aside = |d: &mut crate::mode::ModeDecision| {
        let old: Vec<String> = d
            .evidence
            .drain(..)
            .map(|e| format!("the proposal said: {e}"))
            .collect();
        d.notes.extend(old);
    };
    match c.change {
        Change::Mode(ModeChoice::Full) => {
            if matches!(t.decision.mode, LoadMode::Full) {
                return Err("already in full".into());
            }
            set_aside(&mut t.decision);
            t.decision.mode = LoadMode::Full;
            t.decision.deletes = None;
        }
        Change::Mode(ModeChoice::Incremental) => {
            if matches!(t.decision.mode, LoadMode::Incremental { .. }) {
                return Err("already incremental".into());
            }
            let Some(w) = &settings.modes.application_watermark else {
                return Err("no application-watermark is set for this source".into());
            };
            let Some(col) = t.columns.iter().find(|m| m.column.eq_ignore_ascii_case(w)) else {
                return Err(format!("{} has no column {w}", c.table));
            };
            if !orderable(settings.source_engine, &col.pg_type) {
                return Err(format!(
                    "{} is {}, which cannot order writes",
                    col.column, col.pg_type
                ));
            }
            if t.primary_key.is_empty() {
                return Err("without a primary key deletes could not be found".into());
            }
            if c.reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
                return Err(
                    "the facts keep this table in full: say why it may load incrementally \
                     (--reason)"
                        .into(),
                );
            }
            set_aside(&mut t.decision);
            t.decision.mode = LoadMode::Incremental {
                watermark: col.column.clone(),
                keyset_deletes: false,
            };
            let d = deletes_for(&c.table, facts.get(&c.table), &settings.modes);
            t.decision.deletes = Some(d);
            t.decision.evidence.push(format!(
                "an update that does not set {} is missed by the load; the operator accepted \
                 that, and the next reconciliation shows it",
                col.column
            ));
        }
        Change::Deletes(choice) => {
            let Some(d) = &mut t.decision.deletes else {
                return Err("the table has no delete plan (it is not incremental)".into());
            };
            d.cadence = match choice {
                CadenceChoice::Run => Cadence::Run,
                CadenceChoice::Audit => Cadence::Audit {
                    days: settings.modes.delete_audit_days,
                },
            };
        }
        Change::Purge(policy) => {
            let Some(d) = t.decision.deletes.as_mut().filter(|d| d.kind == "purge") else {
                return Err("the table is not one that is only purged".into());
            };
            d.purge_policy = Some(policy);
        }
    }
    t.decision.evidence.push(note);
    Ok(())
}

/// Every change applied to `plan`, modes first, in the order recorded within a kind.
pub fn apply_all(
    plan: &mut Plan,
    facts: &BTreeMap<String, WriteFacts>,
    changes: &[TableChange],
) -> Vec<Applied> {
    let mut sorted: Vec<&TableChange> = changes.iter().collect();
    sorted.sort_by_key(|c| c.change.order());
    sorted
        .into_iter()
        .map(|c| Applied {
            change: c.clone(),
            stale: apply(plan, facts, c).err(),
        })
        .collect()
}

/// `changes` with `new` in place of an older change of the same kind to the same table.
pub fn record(changes: &mut Vec<TableChange>, new: TableChange) {
    changes.retain(|c| !(c.table == new.table && c.change.kind() == new.change.kind()));
    changes.push(new);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SourceColumn, SourceEngine, SourceTable, TypeKind};
    use crate::plan::{PlanSettings, plan_with};

    fn col(name: &str, ty: &str, trino: &str) -> SourceColumn {
        SourceColumn {
            name: name.into(),
            pg_type: ty.into(),
            kind: TypeKind::Base,
            not_null: true,
            trino_type: Some(trino.into()),
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 0,
            has_default: false,
        }
    }

    fn table(name: &str, key: bool) -> SourceTable {
        SourceTable {
            schema: "ADEMPIERE".into(),
            name: name.into(),
            oid: 0,
            columns: vec![
                col("ID", "NUMBER(10,0)", "decimal(10,0)"),
                col("UPDATED", "DATE", "timestamp(0)"),
            ],
            primary_key: if key { vec!["ID".into()] } else { vec![] },
            row_estimate: 10,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }
    }

    fn facts(update: &str, deletes: &[&str]) -> WriteFacts {
        WriteFacts {
            update_class: update.into(),
            delete_classes: deletes.iter().map(|s| s.to_string()).collect(),
            change_log: None,
        }
    }

    fn setup() -> (Plan, BTreeMap<String, WriteFacts>) {
        let settings = PlanSettings {
            source_engine: SourceEngine::Oracle,
            modes: ModeSettings {
                application_watermark: Some("UPDATED".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let facts: BTreeMap<String, WriteFacts> = [
            (
                "ADEMPIERE.A".to_string(),
                facts("po_only", &["deleted_by_users"]),
            ),
            (
                "ADEMPIERE.B".to_string(),
                facts("raw_sql_skips_updated", &["purged"]),
            ),
            ("ADEMPIERE.C".to_string(), facts("po_only", &["purged"])),
        ]
        .into();
        let plan = plan_with(
            &[
                table("A", true),
                table("B", true),
                table("C", true),
                table("D", false),
            ],
            &settings,
            &facts,
        );
        (plan, facts)
    }

    fn change(table: &str, c: Change, reason: Option<&str>) -> TableChange {
        TableChange {
            table: format!("ADEMPIERE.{table}"),
            change: c,
            reason: reason.map(Into::into),
            by: "op".into(),
            at: "2026-10-26T00:00:00Z".into(),
        }
    }

    fn get<'a>(p: &'a Plan, t: &str) -> &'a TablePlan {
        p.tables.iter().find(|x| x.table == t).unwrap()
    }

    #[test]
    fn full_is_always_allowed_and_drops_the_delete_plan() {
        let (mut p, f) = setup();
        apply(
            &mut p,
            &f,
            &change("A", Change::Mode(ModeChoice::Full), None),
        )
        .unwrap();
        let a = get(&p, "A");
        assert_eq!(a.decision.mode, LoadMode::Full);
        assert!(a.decision.deletes.is_none());
        assert_eq!(a.decision.evidence.len(), 1);
        assert!(a.decision.evidence[0].contains("operator (op)"));
        assert!(
            a.decision
                .notes
                .iter()
                .any(|n| n.starts_with("the proposal said: update class po_only"))
        );
    }

    #[test]
    fn incremental_against_the_facts_needs_a_reason_and_a_key() {
        let (mut p, f) = setup();
        assert_eq!(get(&p, "B").decision.mode, LoadMode::Full);
        let e = apply(
            &mut p,
            &f,
            &change("B", Change::Mode(ModeChoice::Incremental), None),
        );
        assert!(e.unwrap_err().contains("--reason"));
        let e = apply(
            &mut p,
            &f,
            &change("D", Change::Mode(ModeChoice::Incremental), Some("x")),
        );
        assert!(e.unwrap_err().contains("primary key"));
        apply(
            &mut p,
            &f,
            &change(
                "B",
                Change::Mode(ModeChoice::Incremental),
                Some("the raw update is a one-off migration"),
            ),
        )
        .unwrap();
        let b = get(&p, "B");
        assert!(
            matches!(&b.decision.mode, LoadMode::Incremental { watermark, .. } if watermark == "UPDATED")
        );
        // Its delete plan comes from its facts: only purged.
        let d = b.decision.deletes.as_ref().unwrap();
        assert_eq!(d.kind, "purge");
        assert_eq!(d.purge_policy, Some(PurgePolicy::Retain));
    }

    #[test]
    fn cadence_and_purge_need_the_plan_they_change() {
        let (mut p, f) = setup();
        apply(
            &mut p,
            &f,
            &change("A", Change::Deletes(CadenceChoice::Audit), None),
        )
        .unwrap();
        assert_eq!(
            get(&p, "A").decision.deletes.as_ref().unwrap().cadence,
            Cadence::Audit { days: 7 }
        );
        let e = apply(
            &mut p,
            &f,
            &change("A", Change::Purge(PurgePolicy::Propagate), None),
        );
        assert!(e.unwrap_err().contains("only purged"));
        apply(
            &mut p,
            &f,
            &change("C", Change::Purge(PurgePolicy::Propagate), None),
        )
        .unwrap();
        assert_eq!(
            get(&p, "C").decision.deletes.as_ref().unwrap().purge_policy,
            Some(PurgePolicy::Propagate)
        );
        let e = apply(
            &mut p,
            &f,
            &change("D", Change::Mode(ModeChoice::Full), None),
        );
        assert_eq!(e.unwrap_err(), "already in full");
        let e = apply(
            &mut p,
            &f,
            &change("D", Change::Deletes(CadenceChoice::Run), None),
        );
        assert!(e.unwrap_err().contains("no delete plan"));
    }

    #[test]
    fn modes_apply_first_and_a_change_that_no_longer_fits_is_stale() {
        let (mut p, f) = setup();
        let changes = vec![
            change("B", Change::Deletes(CadenceChoice::Audit), None),
            change("B", Change::Mode(ModeChoice::Incremental), Some("ok")),
            change("Z", Change::Mode(ModeChoice::Full), None),
        ];
        let out = apply_all(&mut p, &f, &changes);
        assert_eq!(out.len(), 3);
        assert!(
            out.iter()
                .find(|a| a.change.table == "ADEMPIERE.B" && a.change.change.kind() == "deletes")
                .unwrap()
                .stale
                .is_none()
        );
        assert!(
            out.iter()
                .find(|a| a.change.table == "ADEMPIERE.Z")
                .unwrap()
                .stale
                .is_some()
        );
    }

    #[test]
    fn a_newer_change_of_a_kind_replaces_the_older() {
        let mut v = vec![];
        record(&mut v, change("A", Change::Mode(ModeChoice::Full), None));
        record(
            &mut v,
            change("A", Change::Deletes(CadenceChoice::Run), None),
        );
        record(
            &mut v,
            change("A", Change::Mode(ModeChoice::Incremental), Some("r")),
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].change, Change::Mode(ModeChoice::Incremental));
        let text = serde_json::to_string(&v[1]).unwrap();
        assert!(
            text.contains("\"change\":\"mode\",\"to\":\"incremental\""),
            "{text}"
        );
        let back: TableChange = serde_json::from_str(&text).unwrap();
        assert_eq!(back, v[1]);
    }
}
