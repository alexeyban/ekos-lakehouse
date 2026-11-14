//! EKOS RFC 0257 §3.5 — `lakehouse marts edit`: a structured edit made on a step of a mart's
//! chain, previewed and gated before it changes the requirement.
//!
//! The edit set, its classification and the static gates are `ekos_lakehouse_gold::edits`
//! (pure). Here: the step it is made on, the engine gates (the mart plans, the grain stays unique,
//! every additive measure reconciles with silver), the preview (the chain before and after, with
//! the row counts of §3.3 for both), and the record — a requirement edit logged with its step,
//! which invalidates the approval: the mart then goes through approve, propose, merge, build and
//! reconcile again (RFC 0215, 0228). A failing gate, or a modifying edit without `--confirm`,
//! records nothing.

use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::typemap::ident;
use ekos_lakehouse_gold::edits::{Gate, StepEdit};
use ekos_lakehouse_gold::{self as gold, Compiled, MartSpec};
use serde_json::{Value, json};

/// The edit from the command's flags, or from `--edit <json>` (what the assistant returns).
#[derive(Debug, Default)]
pub struct EditArgs<'a> {
    pub json: Option<&'a str>,
    pub add_filter: Option<&'a str>,
    pub drop_filter: Option<&'a str>,
    pub change_filter: Option<&'a str>,
    pub to: Option<&'a str>,
    pub join_term: Option<&'a str>,
    pub via: Option<&'a str>,
    pub add_dimension: Option<&'a str>,
    pub drop_dimension: Option<&'a str>,
    pub add_measure: Option<&'a str>,
    pub drop_measure: Option<&'a str>,
    pub change_measure: Option<&'a str>,
    pub definition: Option<&'a str>,
}

/// One edit, or why the flags do not name exactly one.
pub fn edit_from(a: &EditArgs<'_>) -> anyhow::Result<StepEdit> {
    let need = |v: Option<&str>, flag: &str| -> anyhow::Result<String> {
        v.map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("{flag} is needed with this edit"))
    };
    let mut found: Vec<StepEdit> = vec![];
    if let Some(j) = a.json {
        found.push(serde_json::from_str(j).map_err(|e| {
            anyhow::anyhow!(
                "--edit is not one of the edits (add_filter, change_filter, drop_filter, \
                 change_join, add_dimension, drop_dimension, add_measure, drop_measure, \
                 change_measure): {e}"
            )
        })?);
    }
    if let Some(f) = a.add_filter {
        found.push(StepEdit::AddFilter { filter: f.into() });
    }
    if let Some(f) = a.drop_filter {
        found.push(StepEdit::DropFilter { filter: f.into() });
    }
    if let Some(f) = a.change_filter {
        found.push(StepEdit::ChangeFilter {
            from: f.into(),
            to: need(a.to, "--to")?,
        });
    }
    if let Some(t) = a.join_term {
        found.push(StepEdit::ChangeJoin {
            term: t.into(),
            via: need(a.via, "--via")?,
        });
    }
    if let Some(n) = a.add_dimension {
        found.push(StepEdit::AddDimension {
            name: n.into(),
            definition: need(a.definition, "--definition")?,
        });
    }
    if let Some(n) = a.drop_dimension {
        found.push(StepEdit::DropDimension { name: n.into() });
    }
    if let Some(n) = a.add_measure {
        found.push(StepEdit::AddMeasure {
            name: n.into(),
            definition: need(a.definition, "--definition")?,
        });
    }
    if let Some(n) = a.drop_measure {
        found.push(StepEdit::DropMeasure { name: n.into() });
    }
    if let Some(n) = a.change_measure {
        found.push(StepEdit::ChangeMeasure {
            name: n.into(),
            definition: need(a.definition, "--definition")?,
        });
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => anyhow::bail!(
            "no edit: --add-filter, --change-filter/--to, --drop-filter, --join-term/--via, \
             --add-dimension/--definition, --drop-dimension, --add-measure/--definition, \
             --drop-measure, --change-measure/--definition, or --edit <json>. Anything else is a comment on the step \
             (`ekos annotate add <table> comment …`)"
        ),
        _ => anyhow::bail!("one edit at a time"),
    }
}

/// Whether an edit may be made on a step of this type (§3.5's "On").
pub fn on_step(e: &StepEdit, step_type: &str) -> Result<(), String> {
    if e.on().contains(&step_type) {
        Ok(())
    } else {
        Err(format!(
            "{} is made on a {} step, not a {step_type} step",
            e.kind().replace('_', " "),
            e.on().join(" or ")
        ))
    }
}

/// The SQL of the engine gates, over the edited mart's own SQL (never a stored table).
pub struct EngineGates {
    /// On the engine the mart is built on.
    pub plans: String,
    /// `None` when the mart has no grain. On the engine the mart is built on.
    pub grain: Option<String>,
    /// (measure, SQL returning `mart_total, silver_total` as text) — always Trino, where silver
    /// is: both sides of the comparison on one engine (RFC 0257).
    pub reconcile: Vec<(String, String)>,
    /// EKOS RFC 0260 §2.2 (X8): (measure, SQL returning the measure's total) over the edited
    /// mart, on the engine the mart is built on.
    pub totals: Vec<(String, String)>,
}

/// Pure: the engine gates' SQL. `run_sql` is the edited mart as the gold engine runs it
/// (`gold::engine_sql` on Trino, `gold::duckdb::engine_sql` on DuckDB, RFC 0260 §2.3).
pub fn engine_gates(
    spec: &MartSpec,
    resolved: &[gold::Resolved],
    c: &Compiled,
    catalog: &str,
    run_sql: &str,
) -> EngineGates {
    let trino = gold::engine_sql(c, catalog);
    let grain = (!spec.grain.is_empty()).then(|| {
        let g = spec
            .grain
            .iter()
            .map(|x| ident(x))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "select count(*) from (select {g} from ({run_sql}) m group by {g} having count(*) > 1) d"
        )
    });
    // RFC 0228's reconciliation queries, read against the edited SQL instead of the built table.
    const PREVIEW: &str = "ekos_preview_gold";
    let table = format!(
        "{}.{}.{}",
        ident(catalog),
        ident(PREVIEW),
        ident(&spec.mart)
    );
    let reconcile = gold::reconcile_sql(spec, resolved, c, catalog, PREVIEW)
        .into_iter()
        .map(|(m, sql)| (m, sql.replace(&table, &format!("({trino}) m"))))
        .collect();
    EngineGates {
        plans: format!("EXPLAIN {run_sql}"),
        grain,
        reconcile,
        totals: totals_sql(resolved, run_sql),
    }
}

/// EKOS RFC 0260 §2.2: each additive measure's total over a mart's SQL, as text. A sum of the
/// mart's sums (or counts) is the measure's total, whatever the grain.
pub fn totals_sql(resolved: &[gold::Resolved], mart_sql: &str) -> Vec<(String, String)> {
    resolved
        .iter()
        .filter(|r| r.section == "measures")
        .filter(|r| {
            matches!(&r.resolution, Ok(gold::Term::Aggregate { func, distinct: false, .. })
                if func == "sum" || func == "count")
        })
        .map(|r| {
            (
                r.term.clone(),
                format!(
                    "select cast(sum({}) as varchar) from ({mart_sql}) m",
                    ident(&r.term)
                ),
            )
        })
        .collect()
}

/// The gates' verdicts from what the engines answered. `engine` names the one the mart is built
/// on (`Trino`, `DuckDB`).
pub fn engine_verdicts(
    engine: &str,
    plans: Result<(), String>,
    duplicates: Option<i64>,
    totals: &[(String, String, String)],
) -> Vec<Gate> {
    let mut out = vec![Gate {
        gate: "engine",
        passed: plans.is_ok(),
        detail: match plans {
            Ok(()) => format!("{engine} plans the edited mart"),
            Err(e) => format!("{engine} cannot plan it: {e}"),
        },
    }];
    if let Some(d) = duplicates {
        out.push(Gate {
            gate: "grain",
            passed: d == 0,
            detail: if d == 0 {
                format!("one row per grain in {engine} (the dbt uniqueness test)")
            } else {
                format!("{d} grain value(s) repeat in {engine}: the grain is not unique")
            },
        });
    }
    for (m, mart, silver) in totals {
        out.push(Gate {
            gate: "reconciliation",
            passed: crate::marts::totals_equal(mart, silver),
            detail: format!("`{m}`: mart {mart} / silver {silver}"),
        });
    }
    out
}

/// EKOS RFC 0260 §2.2 (X8): each measure's total before and after the edit, in measure order
/// (before's first, then the ones only after has). Shown, never a gate.
pub fn totals_block(
    before: &[(String, Option<String>)],
    after: &[(String, Option<String>)],
) -> Vec<Value> {
    let mut names: Vec<&str> = before.iter().map(|(m, _)| m.as_str()).collect();
    for (m, _) in after {
        if !names.contains(&m.as_str()) {
            names.push(m);
        }
    }
    let get = |side: &[(String, Option<String>)], m: &str| {
        side.iter().find(|(n, _)| n == m).map(|(_, v)| v.clone())
    };
    names
        .into_iter()
        .map(|m| {
            let (b, a) = (get(before, m), get(after, m));
            let line = match (&b, &a) {
                (Some(b), Some(a)) => {
                    let (bs, as_) = (
                        b.as_deref().unwrap_or("null"),
                        a.as_deref().unwrap_or("null"),
                    );
                    match difference(bs, as_) {
                        Some(d) => format!("{m}: before {bs} → after {as_} ({d})"),
                        None => format!("{m}: before {bs} → after {as_}"),
                    }
                }
                (Some(b), None) => {
                    format!("{m}: before {} (dropped)", b.as_deref().unwrap_or("null"))
                }
                (None, Some(a)) => format!("{m}: after {} (new)", a.as_deref().unwrap_or("null")),
                (None, None) => m.to_string(),
            };
            json!({
                "measure": m,
                "before": b.flatten(),
                "after": a.flatten(),
                "line": line,
            })
        })
        .collect()
}

/// `after − before` with the inputs' own number of decimals, signed; `None` unless both are
/// numbers.
fn difference(before: &str, after: &str) -> Option<String> {
    let (b, a): (f64, f64) = (before.trim().parse().ok()?, after.trim().parse().ok()?);
    let decimals = |s: &str| s.split_once('.').map_or(0, |(_, f)| f.len()).min(10);
    let places = decimals(before).max(decimals(after));
    let d = a - b;
    if d == 0.0 {
        return Some("no change".into());
    }
    let sign = if d > 0.0 { "+" } else { "−" };
    Some(format!("{sign}{:.*}", places, d.abs()))
}

/// What the preview shows and the record keeps.
pub fn outcome(
    e: &StepEdit,
    step: u32,
    said: &str,
    class: &gold::edits::Class,
    gates: &[Gate],
    confirmed: bool,
) -> Value {
    let failed: Vec<&Gate> = gates.iter().filter(|g| !g.passed).collect();
    let blocked = if !failed.is_empty() {
        Some(format!(
            "refused: {}",
            failed
                .iter()
                .map(|g| format!("{} ({})", g.gate, g.detail))
                .collect::<Vec<_>>()
                .join("; ")
        ))
    } else if class.class == "modifying" && !confirmed {
        Some(
            "a modifying change needs your confirmation (--confirm): existing rows or values \
             change"
                .into(),
        )
    } else {
        None
    };
    json!({
        "edit": e,
        "kind": e.kind(),
        "step": step,
        "change": said,
        "class": class.class,
        "reasons": class.reasons,
        "gates": gates,
        "blocked": blocked,
    })
}

/// The requirement-log line of a recorded edit.
pub fn log_line(step: u32, step_label: &str, said: &str, class: &str) -> String {
    format!("step {step} (`{step_label}`): {said} [{class}, RFC 0257]")
}

/// The plan's catalog.
pub fn catalog(plan: &Plan) -> &str {
    &plan.settings.target_catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_before_and_after_with_the_difference() {
        let t = |m: &str, v: &str| (m.to_string(), Some(v.to_string()));
        let b = totals_block(
            &[t("revenue", "308227.75"), t("orders", "567")],
            &[
                t("revenue", "301940.1"),
                t("orders", "567"),
                t("discount", "12.50"),
            ],
        );
        let lines: Vec<&str> = b.iter().map(|x| x["line"].as_str().unwrap()).collect();
        assert_eq!(
            lines,
            [
                "revenue: before 308227.75 → after 301940.1 (−6287.65)",
                "orders: before 567 → after 567 (no change)",
                "discount: after 12.50 (new)",
            ]
        );
        assert_eq!(b[0]["before"], "308227.75");
        let dropped = totals_block(&[t("x", "1")], &[]);
        assert_eq!(dropped[0]["line"], "x: before 1 (dropped)");
    }

    #[test]
    fn the_verdict_names_the_engine_the_mart_is_built_on() {
        let g = engine_verdicts("DuckDB", Ok(()), Some(0), &[]);
        assert_eq!(g[0].detail, "DuckDB plans the edited mart");
        assert!(g[1].detail.contains("in DuckDB"));
        let g = engine_verdicts("DuckDB", Err("Binder Error: x".into()), None, &[]);
        assert!(!g[0].passed && g[0].detail.starts_with("DuckDB cannot plan it"));
    }

    #[test]
    fn flags_name_exactly_one_edit_and_json_is_checked() {
        let e = edit_from(&EditArgs {
            add_measure: Some("orders"),
            definition: Some("count of c_order"),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(e.kind(), "add_measure");
        assert!(
            edit_from(&EditArgs::default())
                .unwrap_err()
                .to_string()
                .contains("comment")
        );
        assert!(
            edit_from(&EditArgs {
                add_filter: Some("a"),
                drop_filter: Some("b"),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            edit_from(&EditArgs {
                json: Some(r#"{"edit":"rewrite_sql","sql":"select 1"}"#),
                ..Default::default()
            })
            .is_err()
        );
        assert!(on_step(&e, "Aggregate").is_ok());
        assert!(on_step(&e, "Join").unwrap_err().contains("Aggregate"));
    }

    #[test]
    fn a_failing_gate_or_an_unconfirmed_modifying_edit_is_blocked() {
        let e = StepEdit::DropFilter { filter: "x".into() };
        let modifying = gold::edits::Class {
            class: "modifying",
            reasons: vec![],
        };
        let ok = Gate {
            gate: "resolution",
            passed: true,
            detail: String::new(),
        };
        let v = outcome(
            &e,
            3,
            "removed",
            &modifying,
            std::slice::from_ref(&ok),
            false,
        );
        assert!(v["blocked"].as_str().unwrap().contains("confirmation"));
        let v = outcome(
            &e,
            3,
            "removed",
            &modifying,
            std::slice::from_ref(&ok),
            true,
        );
        assert!(v["blocked"].is_null());
        let failed = engine_verdicts(
            "Trino",
            Ok(()),
            Some(0),
            &[("total".into(), "10".into(), "11".into())],
        );
        let v = outcome(&e, 3, "removed", &modifying, &failed, true);
        assert!(
            v["blocked"].as_str().unwrap().contains("reconciliation"),
            "a confirmed edit that fails a gate is still refused"
        );
    }
}
