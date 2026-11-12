//! EKOS RFC 0256 §3.6: a warning on the extraction plan review for an incremental table that
//! something outside the code writes.
//!
//! The watermark of an incremental table was proven from code. An `unexplained_writer` (a database
//! role writing where no code it runs declares that write) or `unattributed_writes` (the table's
//! counters show writes no observed statement explains) says that proof may not cover every
//! writer. **EKOS advises; a person decides**: a warning never changes the plan. It carries the
//! mode EKOS would recommend, the reason and the cost; the operator answers it with an explicit
//! `lakehouse extraction change` on that table (a new mode, or `--keep`), which records the
//! evidence it answers. A warning whose evidence changes (another role, other operations) is open
//! again. Pure: the findings come in from the ledger and the row estimates from the source read.

use ekos_lakehouse_bronze::extraction::TableChange;
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::{Cadence, LoadMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The finding types that warn (RFC 0256 §3.6). The other two do not say a write escapes the
/// watermark: several declared writing systems are all code, and a declared write not observed
/// is a write that did not happen.
pub const WARNING_FINDINGS: [&str; 2] = ["unexplained_writer", "unattributed_writes"];

/// One current EKOS interaction finding, as `lakehouse plan` read it (kept in the proposal, so a
/// later `extraction show` needs no ledger).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WriterFinding {
    /// `unexplained_writer` or `unattributed_writes`.
    pub finding: String,
    /// The table as EKOS keys it: the bare name, lower case.
    pub table: String,
    /// `db_role:<name>` for an unexplained writer; none for unattributed writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default)]
    pub operations: Vec<String>,
    /// Calls, the window, rows: what EKOS recorded, shown as it is.
    #[serde(default)]
    pub detail: Value,
}

/// What EKOS would recommend for the table, and what it costs. Never applied by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Advice {
    /// The `lakehouse extraction change` flag that would follow the advice, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<&'static str>,
    pub words: String,
    pub reason: String,
    pub cost: String,
}

/// A warning on one planned table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WriterWarning {
    /// `schema.table`, as the plan spells it.
    pub table: String,
    pub finding: String,
    /// The role's name, for an unexplained writer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub operations: Vec<String>,
    /// The observation window in words, or what EKOS recorded instead (rows for unattributed
    /// writes).
    pub window: String,
    /// What a decision answers: finding, table, role and operations — never counts, so a
    /// re-observation of the same writer keeps it answered.
    pub evidence: String,
    pub advice: Advice,
    /// The change that answers it, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<TableChange>,
}

impl WriterWarning {
    pub fn is_open(&self) -> bool {
        self.answered_by.is_none()
    }

    pub fn state(&self) -> &'static str {
        if self.is_open() { "open" } else { "answered" }
    }
}

fn evidence(f: &WriterFinding, table: &str) -> String {
    let mut ops = f.operations.clone();
    ops.sort();
    let key = format!(
        "{}|{}|{}|{}",
        f.finding,
        table,
        f.actor.as_deref().unwrap_or(""),
        ops.join(",")
    );
    let h = crate::plan_cmd::sha256(&key);
    h[..h.len().min(16)].to_string()
}

fn window_words(f: &WriterFinding) -> String {
    let d = &f.detail;
    if let Some(rows) = d.get("rows").and_then(Value::as_object) {
        let parts: Vec<String> = rows.iter().map(|(op, n)| format!("{op} {n}")).collect();
        return format!(
            "rows the table's counters show and no observed statement explains: {}",
            parts.join(", ")
        );
    }
    let day = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(|s| s.get(..10).unwrap_or(s).to_string())
    };
    let from = day(d.get("window_start")).unwrap_or_else(|| "an unknown start".into());
    let to = day(d.get("window_end")).unwrap_or_else(|| "?".into());
    let calls = d
        .get("calls")
        .and_then(Value::as_u64)
        .map(|c| format!(", {c} call(s)"))
        .unwrap_or_default();
    let cut = if d.get("truncated").and_then(Value::as_bool) == Some(true) {
        " (the statistics were truncated)"
    } else {
        ""
    };
    // EKOS RFC 0256 open question 2: since when EKOS has seen this writer.
    let since = day(d.get("first_observed"))
        .map(|f| format!(", first seen {f}"))
        .unwrap_or_default();
    format!("{from} – {to}{calls}{since}{cut}")
}

fn advice(
    t: &ekos_lakehouse_bronze::TablePlan,
    f: &WriterFinding,
    rows: Option<i64>,
    max_rows: i64,
) -> Advice {
    let watermark = match &t.decision.mode {
        LoadMode::Incremental { watermark, .. } => watermark.as_str(),
        LoadMode::Full => "",
    };
    let missed = format!(
        "an incremental load reads the rows whose {watermark} moved; a write outside the code may \
         not move it, and that row stays as it was in bronze until a full load"
    );
    if let Some(n) = rows.filter(|n| *n <= max_rows) {
        return Advice {
            flag: Some("full"),
            words: "load in full".into(),
            reason: missed,
            cost: format!(
                "about {n} rows read on every run instead of the changed ones \
                 (full-refresh-max-rows is {max_rows})"
            ),
        };
    }
    let size = match rows {
        Some(n) => format!("about {n} rows, more than full-refresh-max-rows ({max_rows})"),
        None => "the row count is not known".into(),
    };
    let deletes_audited = t
        .decision
        .deletes
        .as_ref()
        .is_some_and(|d| matches!(d.cadence, Cadence::Audit { .. }));
    if deletes_audited && f.operations.iter().any(|o| o == "delete") {
        return Advice {
            flag: Some("deletes-every-run"),
            words: "look for deletes every run".into(),
            reason: "the writer outside the code deletes, and deletes are looked for only at the \
                     audit: until then a deleted row stays in bronze"
                .into(),
            cost: format!("a key comparison on every run; a full load is not advised ({size})"),
        };
    }
    Advice {
        flag: None,
        words: "keep the mode and reconcile the table after each run (`lakehouse reconcile`)"
            .into(),
        reason: format!("{missed}; reconciliation shows when it happens"),
        cost: format!("a reconciliation per run; a full load is not advised ({size})"),
    }
}

/// Pure: the warnings on `plan`'s incremental tables, answered by `changes` where one names the
/// warning's evidence. A full table gets none: every run reads every row.
pub fn warnings(
    plan: &Plan,
    findings: &[WriterFinding],
    rows: &BTreeMap<String, i64>,
    changes: &[TableChange],
) -> Vec<WriterWarning> {
    let max_rows = plan.settings.modes.full_refresh_max_rows;
    let mut out = vec![];
    for t in &plan.tables {
        if matches!(t.decision.mode, LoadMode::Full) {
            continue;
        }
        let key = ekos_kir::interaction::table_key(&t.table);
        let q = format!("{}.{}", t.schema, t.table);
        for f in findings
            .iter()
            .filter(|f| f.table == key && WARNING_FINDINGS.contains(&f.finding.as_str()))
        {
            let evidence = evidence(f, &q);
            let answered_by = changes
                .iter()
                .rev()
                .find(|c| c.table == q && c.answers.contains(&evidence))
                .cloned();
            let mut operations = f.operations.clone();
            operations.sort();
            out.push(WriterWarning {
                table: q.clone(),
                finding: f.finding.clone(),
                role: f
                    .actor
                    .as_deref()
                    .map(|a| a.strip_prefix("db_role:").unwrap_or(a).to_string()),
                operations,
                window: window_words(f),
                evidence,
                advice: advice(t, f, rows.get(&q).copied(), max_rows),
                answered_by,
            });
        }
    }
    out
}

/// The evidence of every warning on `table`: what a change to it answers.
pub fn evidence_on(warnings: &[WriterWarning], table: &str) -> Vec<String> {
    warnings
        .iter()
        .filter(|w| w.table == table)
        .map(|w| w.evidence.clone())
        .collect()
}

/// The warning as Markdown, for the table's evidence panel.
pub fn markdown(ws: &[&WriterWarning]) -> String {
    if ws.is_empty() {
        return String::new();
    }
    let mut md = String::from(
        "### Written outside the code (EKOS RFC 0256)\n\nThe watermark was proven from code, and \
         something outside the code writes here. EKOS advises; the mode changes only when you \
         change it, and keeping it is recorded too.\n\n",
    );
    for w in ws {
        let who = match &w.role {
            Some(r) => format!("role `{r}`"),
            None => "unattributed writes".into(),
        };
        md.push_str(&format!(
            "- **{}** — {who}, {} ({})\n  - advice: {} — {}\n  - cost: {}\n",
            w.state(),
            w.operations.join(", "),
            w.window,
            w.advice.words,
            w.advice.reason,
            w.advice.cost
        ));
        if let Some(c) = &w.answered_by {
            md.push_str(&format!(
                "  - answered: {} — by {} at {}{}\n",
                c.change.describe(),
                c.by,
                c.at,
                c.reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            ));
        }
    }
    md.push('\n');
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekos_lakehouse_bronze::extraction::{Change, ModeChoice};
    use serde_json::json;

    fn plan(mode: LoadMode) -> Plan {
        let mut p = ekos_lakehouse_bronze::plan::plan(
            &[],
            &ekos_lakehouse_bronze::plan::PlanSettings::default(),
        );
        p.tables.push(table("public", "customers", mode));
        p
    }

    fn table(schema: &str, name: &str, mode: LoadMode) -> ekos_lakehouse_bronze::TablePlan {
        serde_json::from_value(json!({
            "schema": schema, "table": name, "namespace": "bronze",
            "primary_key": ["id"],
            "decision": {"mode": serde_json::to_value(&mode).unwrap(), "evidence": [], "notes": []},
            "columns": [],
        }))
        .unwrap()
    }

    fn incremental() -> LoadMode {
        LoadMode::Incremental {
            watermark: "updated".into(),
            keyset_deletes: false,
        }
    }

    fn writer(role: &str, ops: &[&str]) -> WriterFinding {
        WriterFinding {
            finding: "unexplained_writer".into(),
            table: "customers".into(),
            actor: Some(format!("db_role:{role}")),
            operations: ops.iter().map(|s| s.to_string()).collect(),
            detail: json!({"calls": 3, "window_start": "2026-11-01T00:00:00Z",
                           "window_end": "2026-11-11T00:00:00Z", "truncated": false}),
        }
    }

    fn rows(n: i64) -> BTreeMap<String, i64> {
        BTreeMap::from([("public.customers".to_string(), n)])
    }

    fn change(c: Change, answers: Vec<String>) -> TableChange {
        TableChange {
            table: "public.customers".into(),
            change: c,
            reason: Some("known nightly job".into()),
            by: "ana".into(),
            at: "2026-11-12T00:00:00Z".into(),
            answers,
        }
    }

    #[test]
    fn an_unexplained_writer_on_an_incremental_table_warns_with_advice() {
        let p = plan(incremental());
        let w = warnings(&p, &[writer("etl_nightly", &["update"])], &rows(50), &[]);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].table, "public.customers");
        assert_eq!(w[0].role.as_deref(), Some("etl_nightly"));
        assert!(w[0].is_open());
        assert_eq!(
            w[0].advice.flag,
            Some("full"),
            "a small table: load it in full"
        );
        assert!(w[0].advice.cost.contains("about 50 rows"));
        assert!(w[0].window.contains("2026-11-01 – 2026-11-11, 3 call(s)"));
        let mut seen = writer("etl_nightly", &["update"]);
        seen.detail["first_observed"] = json!("2026-11-03T08:00:00Z");
        let w = warnings(&p, &[seen], &rows(50), &[]);
        assert!(
            w[0].window.ends_with("3 call(s), first seen 2026-11-03"),
            "{}",
            w[0].window
        );
    }

    #[test]
    fn a_full_table_gets_no_warning() {
        let p = plan(LoadMode::Full);
        assert!(warnings(&p, &[writer("etl_nightly", &["update"])], &rows(50), &[]).is_empty());
    }

    #[test]
    fn a_large_table_is_advised_to_reconcile_not_to_load_in_full() {
        let p = plan(incremental());
        let max = p.settings.modes.full_refresh_max_rows;
        let w = warnings(&p, &[writer("etl", &["update"])], &rows(max + 1), &[]);
        assert_eq!(w[0].advice.flag, None);
        assert!(w[0].advice.words.contains("reconcile"));
        let unknown = warnings(&p, &[writer("etl", &["update"])], &BTreeMap::new(), &[]);
        assert!(unknown[0].advice.cost.contains("not known"));
    }

    #[test]
    fn other_finding_types_do_not_warn() {
        let p = plan(incremental());
        let mut f = writer("etl", &["update"]);
        f.finding = "multiple_writing_systems".into();
        assert!(warnings(&p, &[f], &rows(5), &[]).is_empty());
    }

    #[test]
    fn a_decision_answers_the_warning_until_the_evidence_changes() {
        let p = plan(incremental());
        let first = warnings(&p, &[writer("etl", &["update"])], &rows(5), &[]);
        let keep = change(Change::Keep, evidence_on(&first, "public.customers"));
        let answered = warnings(
            &p,
            &[writer("etl", &["update"])],
            &rows(5),
            std::slice::from_ref(&keep),
        );
        assert!(!answered[0].is_open());
        assert_eq!(
            answered[0].answered_by.as_ref().map(|c| c.by.as_str()),
            Some("ana")
        );
        // Counts are not evidence: a re-observation with more calls stays answered.
        let mut more = writer("etl", &["update"]);
        more.detail["calls"] = json!(900);
        assert!(!warnings(&p, &[more], &rows(5), std::slice::from_ref(&keep))[0].is_open());
        // Another operation, or another role, is new evidence: open again.
        let deletes = warnings(
            &p,
            &[writer("etl", &["delete", "update"])],
            &rows(5),
            std::slice::from_ref(&keep),
        );
        assert!(deletes[0].is_open());
        let other = warnings(&p, &[writer("loader", &["update"])], &rows(5), &[keep]);
        assert!(other[0].is_open());
    }

    #[test]
    fn a_change_to_another_mode_answers_too_but_only_with_the_evidence() {
        let p = plan(incremental());
        let first = warnings(&p, &[writer("etl", &["update"])], &rows(5), &[]);
        let full = change(
            Change::Mode(ModeChoice::Full),
            evidence_on(&first, "public.customers"),
        );
        assert!(!warnings(&p, &[writer("etl", &["update"])], &rows(5), &[full])[0].is_open());
        let unrelated = change(Change::Mode(ModeChoice::Full), vec![]);
        assert!(warnings(&p, &[writer("etl", &["update"])], &rows(5), &[unrelated])[0].is_open());
    }

    #[test]
    fn unattributed_writes_show_the_rows_and_name_no_role() {
        let p = plan(incremental());
        let f = WriterFinding {
            finding: "unattributed_writes".into(),
            table: "customers".into(),
            actor: None,
            operations: vec!["insert".into()],
            detail: json!({"database": "shop", "rows": {"insert": 50}}),
        };
        let w = warnings(&p, &[f], &rows(5), &[]);
        assert_eq!(w[0].role, None);
        assert!(w[0].window.contains("insert 50"));
        assert!(markdown(&[&w[0]]).contains("unattributed writes"));
    }
}
