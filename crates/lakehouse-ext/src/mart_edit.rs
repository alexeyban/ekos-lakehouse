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
                 change_join, add_dimension, drop_dimension, add_measure, drop_measure): {e}"
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
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => anyhow::bail!(
            "no edit: --add-filter, --change-filter/--to, --drop-filter, --join-term/--via, \
             --add-dimension/--definition, --drop-dimension, --add-measure/--definition, \
             --drop-measure, or --edit <json>. Anything else is a comment on the step \
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
    pub plans: String,
    /// `None` when the mart has no grain.
    pub grain: Option<String>,
    /// (measure, SQL returning `mart_total, silver_total` as text).
    pub reconcile: Vec<(String, String)>,
}

/// Pure: the engine gates' SQL.
pub fn engine_gates(
    spec: &MartSpec,
    resolved: &[gold::Resolved],
    c: &Compiled,
    catalog: &str,
) -> EngineGates {
    let engine = gold::engine_sql(c, catalog);
    let grain = (!spec.grain.is_empty()).then(|| {
        let g = spec
            .grain
            .iter()
            .map(|x| ident(x))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "select count(*) from (select {g} from ({engine}) m group by {g} having count(*) > 1) d"
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
        .map(|(m, sql)| (m, sql.replace(&table, &format!("({engine}) m"))))
        .collect();
    EngineGates {
        plans: format!("EXPLAIN {engine}"),
        grain,
        reconcile,
    }
}

/// The gates' verdicts from what the engine answered.
pub fn engine_verdicts(
    plans: Result<(), String>,
    duplicates: Option<i64>,
    totals: &[(String, String, String)],
) -> Vec<Gate> {
    let mut out = vec![Gate {
        gate: "engine",
        passed: plans.is_ok(),
        detail: match plans {
            Ok(()) => "Trino plans the edited mart".into(),
            Err(e) => format!("Trino cannot plan it: {e}"),
        },
    }];
    if let Some(d) = duplicates {
        out.push(Gate {
            gate: "grain",
            passed: d == 0,
            detail: if d == 0 {
                "one row per grain (the dbt uniqueness test)".into()
            } else {
                format!("{d} grain value(s) repeat: the grain is not unique")
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
