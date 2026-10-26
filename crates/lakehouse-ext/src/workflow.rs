//! EKOS RFC 0213: this extension's steps of the operator workflow, and the sign-off gate.
//!
//! Steps 2 and 4–7 are reported from the lakehouse facts in the ledger. With
//! `require-signoff = true`, the commands that write pipeline content (`generate`, `sync`,
//! `gold draft`) refuse to run until every review subject has a valid sign-off.

use crate::facts::{MART_KIND, METRIC_KIND, RUN_KIND};
use ekos::extension::WorkflowContext;
use ekos_kir::{KirObject, ObjectKind};
use ekos_semantic::workflow::{Step, StepState};
use serde_json::Value;
use std::collections::BTreeSet;

/// What the steps are computed from, counted from the ledger's lakehouse objects.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Counts {
    pub marts_proposed: usize,
    pub marts_with_questions: usize,
    pub tables_synced: usize,
    /// Tables with a successful / failed latest bronze or silver run.
    pub built_ok: usize,
    pub built_failing: usize,
    pub gold_ok: usize,
    pub gold_failing: usize,
}

fn prop<'a>(o: &'a KirObject, k: &str) -> Option<&'a str> {
    o.properties.get(k).and_then(Value::as_str)
}

/// Pure: the latest version of every object in, counts out.
pub fn counts(objects: &[KirObject]) -> Counts {
    let mut c = Counts::default();
    // (layer group, table) → latest (at, ok)
    let mut latest: std::collections::BTreeMap<(bool, String), (i64, bool)> = Default::default();
    let mut synced = BTreeSet::new();
    for o in objects {
        let ObjectKind::Custom(kind) = &o.kind else {
            continue;
        };
        match kind.as_str() {
            MART_KIND => match prop(o, "status") {
                Some("proposed") => c.marts_proposed += 1,
                Some("questions") => c.marts_with_questions += 1,
                _ => {}
            },
            METRIC_KIND if prop(o, "metric") == Some("schema_state") => {
                if let Some(t) = prop(o, "table") {
                    synced.insert(t.to_string());
                }
            }
            RUN_KIND => {
                let gold = prop(o, "layer") == Some("gold");
                let table = prop(o, "table").unwrap_or_default().to_string();
                let at = o.created_at.timestamp();
                let ok = prop(o, "status") == Some("ok");
                let e = latest.entry((gold, table)).or_insert((at, ok));
                if at >= e.0 {
                    *e = (at, ok);
                }
            }
            _ => {}
        }
    }
    c.tables_synced = synced.len();
    for ((gold, _), (_, ok)) in latest {
        match (gold, ok) {
            (false, true) => c.built_ok += 1,
            (false, false) => c.built_failing += 1,
            (true, true) => c.gold_ok += 1,
            (true, false) => c.gold_failing += 1,
        }
    }
    c
}

fn built(n: u8, ok: usize, failing: usize, what: &str, todo: &str) -> Step {
    match (ok, failing) {
        (0, 0) => Step::new(n, StepState::Todo, todo),
        (_, 0) => Step::new(n, StepState::Done, format!("{ok} {what} built")),
        _ => Step::new(
            n,
            StepState::Partial,
            format!("{ok} {what} built, {failing} failing"),
        ),
    }
}

/// Pure: step 4 from the requirements' states (EKOS RFC 0215), or `None` without any.
pub fn requirements_step(states: &[String]) -> Option<Step> {
    if states.is_empty() {
        return None;
    }
    let n = |s: &str| states.iter().filter(|x| x.as_str() == s).count();
    let (approved, questions, ready) = (n("approved"), n("questions"), n("resolved"));
    let other = states.len() - approved - questions - ready;
    let text = format!(
        "{approved} of {} requirement(s) approved{}{}{}",
        states.len(),
        if questions > 0 {
            format!(", {questions} with questions")
        } else {
            String::new()
        },
        if ready > 0 {
            format!(", {ready} ready to approve")
        } else {
            String::new()
        },
        if other > 0 {
            format!(", {other} changed since approval")
        } else {
            String::new()
        },
    );
    Some(Step::new(
        4,
        if approved == states.len() {
            StepState::Done
        } else {
            StepState::Partial
        },
        text,
    ))
}

/// Pure: the steps from the counts, with step 4 from the requirements when there are any
/// (EKOS RFC 0215) and step 5 from the extraction plan (EKOS RFC 0226).
pub fn steps(c: &Counts, step4: Option<Step>, step5: Step) -> Vec<Step> {
    vec![
        Step::new(
            2,
            StepState::Todo,
            "source-estate documentation (EKOS RFC 0214) is not built yet",
        ),
        step4.unwrap_or_else(|| match (c.marts_proposed, c.marts_with_questions) {
            (0, 0) => Step::new(
                4,
                StepState::Todo,
                "describe a mart: `lakehouse requirements draft`",
            ),
            (p, 0) => Step::new(4, StepState::Done, format!("{p} mart(s) proposed")),
            (p, q) => Step::new(
                4,
                StepState::Partial,
                format!("{p} mart(s) proposed, {q} with open questions"),
            ),
        }),
        step5,
        built(
            6,
            c.built_ok,
            c.built_failing,
            "bronze/silver table(s)",
            "no bronze or silver run recorded",
        ),
        built(
            7,
            c.gold_ok,
            c.gold_failing,
            "mart(s)",
            "no gold run recorded",
        ),
    ]
}

/// [`counts`] over the ledger, cached in `<state>/lakehouse/workflow-counts.json` under the
/// ledger's entry count (append-only: any write moves it). Writing the cache is best-effort.
fn ledger_counts(ctx: &WorkflowContext<'_>) -> anyhow::Result<Counts> {
    let key = format!(
        "1:{}:{}",
        ctx.ledger.entry_count()?,
        ctx.ledger.object_count()?
    );
    let path = ctx
        .config
        .ekos_dir(ctx.cwd)
        .join("lakehouse")
        .join("workflow-counts.json");
    if let Some(c) = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|v| v["key"] == key.as_str())
        .and_then(|v| serde_json::from_value::<Counts>(v["counts"].clone()).ok())
    {
        return Ok(c);
    }
    let c = counts(&ctx.ledger.all_objects()?);
    let body = serde_json::json!({ "key": key, "counts": c });
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(ctx.cwd))
        .and_then(|()| std::fs::write(&path, body.to_string()));
    Ok(c)
}

/// [`ekos::extension::EkosExtension::workflow_steps`].
pub fn workflow_steps(ctx: &WorkflowContext<'_>) -> Vec<Step> {
    match ledger_counts(ctx) {
        Ok(c) => steps(&c, requirements_states(ctx), extraction_step(ctx)),
        Err(e) => ekos_semantic::workflow::EXTENSION_STEPS
            .iter()
            .map(|n| {
                Step::new(
                    *n,
                    StepState::Todo,
                    format!("could not read the ledger: {e}"),
                )
            })
            .collect(),
    }
}

/// Step 4 from each requirement's state as of the last command that touched it.
fn requirements_states(ctx: &WorkflowContext<'_>) -> Option<Step> {
    let dir = crate::requirements::state_dir(ctx.config, ctx.cwd);
    let states: Vec<String> = crate::requirements::list(&dir)
        .iter()
        .filter_map(|m| crate::requirements::read(&dir, m).ok().flatten())
        .map(|d| {
            if d.record.state.is_empty() {
                "questions".to_string()
            } else {
                d.record.state
            }
        })
        .collect();
    requirements_step(&states)
}

/// Step 5 from the state directory: the proposal with the changes, against the approval.
fn extraction_step(ctx: &WorkflowContext<'_>) -> Step {
    let read = || -> anyhow::Result<Step> {
        let dir = crate::extraction::state_dir(ctx.config, ctx.cwd);
        let configured = crate::settings::load(ctx.config)
            .map(|s| s.pii)
            .unwrap_or_default();
        let p = crate::extraction::read_proposal(&dir)?;
        let changes = crate::extraction::read_changes(&dir)?;
        let f = p
            .as_ref()
            .map(|p| crate::extraction::finalize(p, &changes, &configured));
        let a = crate::extraction::read_approved(&dir)?;
        Ok(crate::extraction::step(p.as_ref(), f.as_ref(), a.as_ref()))
    };
    read().unwrap_or_else(|e| {
        Step::new(
            5,
            StepState::Todo,
            format!("could not read the extraction plan: {e}"),
        )
    })
}

/// With `require-signoff = true`, refuse unless the source model is signed off (EKOS RFC 0213).
pub fn gate(
    settings: &crate::settings::Settings,
    ctx: &ekos::extension::CommandContext<'_>,
) -> anyhow::Result<()> {
    if settings.require_signoff.unwrap_or(false) {
        ekos::commands::workflow::require_signoff_in(ctx.config, ctx.cwd, &[])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(kind: &str, props: Value, at: i64) -> KirObject {
        let mut o = KirObject::new("x", ObjectKind::Custom(kind.into()));
        for (k, v) in props.as_object().unwrap() {
            o.properties.insert(k.clone(), v.clone());
        }
        o.created_at = chrono::DateTime::from_timestamp(at, 0).unwrap();
        o
    }

    #[test]
    fn step_4_comes_from_the_requirements() {
        assert!(requirements_step(&[]).is_none());
        let s = |v: &[&str]| {
            requirements_step(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>()).unwrap()
        };
        assert_eq!(s(&["approved", "approved"]).state, StepState::Done);
        let p = s(&["approved", "questions", "resolved"]);
        assert_eq!(p.state, StepState::Partial);
        assert_eq!(
            p.detail,
            "1 of 3 requirement(s) approved, 1 with questions, 1 ready to approve"
        );
    }

    #[test]
    fn steps_come_from_the_latest_run_per_table() {
        let objects = vec![
            obj(
                RUN_KIND,
                json!({"table":"public.a","layer":"bronze","status":"failed"}),
                1,
            ),
            obj(
                RUN_KIND,
                json!({"table":"public.a","layer":"bronze","status":"ok"}),
                2,
            ),
            obj(
                RUN_KIND,
                json!({"table":"public.b","layer":"silver","status":"failed"}),
                3,
            ),
            obj(
                RUN_KIND,
                json!({"table":"gold.m","layer":"gold","status":"ok"}),
                3,
            ),
            obj(MART_KIND, json!({"status":"proposed"}), 1),
            obj(MART_KIND, json!({"status":"questions"}), 1),
            obj(
                METRIC_KIND,
                json!({"metric":"schema_state","table":"public.a"}),
                1,
            ),
            obj(
                METRIC_KIND,
                json!({"metric":"reconciliation","table":"public.a"}),
                1,
            ),
        ];
        let c = counts(&objects);
        assert_eq!(
            c,
            Counts {
                marts_proposed: 1,
                marts_with_questions: 1,
                tables_synced: 1,
                built_ok: 1,
                built_failing: 1,
                gold_ok: 1,
                gold_failing: 0,
            }
        );
        let s = steps(&c, None, Step::new(5, StepState::Done, "approved"));
        assert_eq!(s.iter().map(|x| x.n).collect::<Vec<_>>(), [2, 4, 5, 6, 7]);
        assert_eq!(s[1].state, StepState::Partial);
        assert_eq!(s[2].state, StepState::Done);
        assert_eq!(s[3].detail, "1 bronze/silver table(s) built, 1 failing");
        assert_eq!(s[4].state, StepState::Done);
        assert!(
            steps(
                &Counts::default(),
                None,
                Step::new(5, StepState::Todo, "plan")
            )
            .iter()
            .all(|x| x.state == StepState::Todo)
        );
    }
}
