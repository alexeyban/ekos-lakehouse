//! Table health (RFC 0196 §2): the ledger's run, reconciliation, schema-state and change facts
//! reduced to one state per table and layer, with the facts that put it there. Pure: the caller
//! reads the ledger and supplies the clock.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Healthy,
    Late,
    Unverified,
    Failing,
    SchemaPending,
}

impl State {
    /// For metrics: higher is worse.
    pub fn level(self) -> u8 {
        self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            State::Healthy => "healthy",
            State::Late => "late",
            State::Unverified => "unverified",
            State::Failing => "failing",
            State::SchemaPending => "schema_pending",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFact {
    pub table: String,
    pub layer: String,
    pub run_id: String,
    pub ok: bool,
    /// Seconds since the Unix epoch.
    pub at: i64,
    pub rows: Option<i64>,
    pub duration_ms: Option<u64>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconFact {
    pub table: String,
    pub run_id: String,
    pub verdict: String,
    pub control_fired: Option<bool>,
    pub divergences: Vec<String>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaStateFact {
    pub table: String,
    /// `active` | `pending` | `paused`
    pub state: String,
    pub proposal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeFact {
    pub table: String,
    pub class: String,
    pub column: Option<String>,
    pub proposal: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub runs: Vec<RunFact>,
    pub reconciliations: Vec<ReconFact>,
    pub schema_states: Vec<SchemaStateFact>,
    pub changes: Vec<ChangeFact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub table: String,
    pub layer: String,
    pub state: State,
    /// The facts that put it there.
    pub evidence: Vec<String>,
    /// For a failing table: what probably explains it.
    pub cause: Option<String>,
    pub last_success_at: Option<i64>,
    pub last_run: Option<RunFact>,
}

fn latest<'a, T>(items: impl Iterator<Item = &'a T>, at: impl Fn(&T) -> i64) -> Option<&'a T>
where
    T: 'a,
{
    items.max_by_key(|x| at(x))
}

fn ago(now: i64, at: i64) -> String {
    let s = (now - at).max(0);
    match s {
        s if s < 120 => format!("{s} s ago"),
        s if s < 7200 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
}

/// One `Health` per (table, layer) that has any fact, sorted.
pub fn evaluate(f: &Facts, now: i64, late_after_minutes: i64) -> Vec<Health> {
    let mut keys: BTreeMap<(String, String), ()> = BTreeMap::new();
    for r in &f.runs {
        keys.insert((r.table.clone(), r.layer.clone()), ());
    }
    for r in &f.reconciliations {
        keys.insert((r.table.clone(), "bronze".into()), ());
    }
    for s in &f.schema_states {
        keys.insert((s.table.clone(), "bronze".into()), ());
    }
    let mut out = vec![];
    for (table, layer) in keys.into_keys() {
        let runs: Vec<&RunFact> = f
            .runs
            .iter()
            .filter(|r| r.table == table && r.layer == layer)
            .collect();
        let last_run = latest(runs.iter().copied(), |r| r.at).cloned();
        let last_ok = latest(runs.iter().copied().filter(|r| r.ok), |r| r.at);
        let recon = if layer == "bronze" {
            latest(f.reconciliations.iter().filter(|r| r.table == table), |r| {
                r.at
            })
        } else {
            None
        };
        let schema = f.schema_states.iter().find(|s| s.table == table);
        let recent_change = latest(f.changes.iter().filter(|c| c.table == table), |c| c.at);

        let mut evidence = vec![];
        let mut cause = None;
        let state = if let Some(s) = schema.filter(|s| s.state != "active") {
            evidence.push(format!(
                "schema change {} on {}",
                s.state,
                s.proposal.as_deref().unwrap_or("a proposal")
            ));
            State::SchemaPending
        } else if let Some(r) = last_run.as_ref().filter(|r| !r.ok) {
            evidence.push(format!(
                "run {} failed {}: {}",
                r.run_id,
                ago(now, r.at),
                r.message.as_deref().unwrap_or("no message")
            ));
            State::Failing
        } else if let Some(r) = recon.filter(|r| r.verdict == "diverged") {
            evidence.push(format!(
                "reconciliation {} diverged {}",
                r.run_id,
                ago(now, r.at)
            ));
            evidence.extend(r.divergences.iter().take(3).cloned());
            State::Failing
        } else if let Some(r) =
            recon.filter(|r| r.verdict == "unverified" || r.control_fired == Some(false))
        {
            evidence.push(format!(
                "reconciliation {}: the planted control did not fire",
                r.run_id
            ));
            State::Unverified
        } else if last_ok.is_none_or(|r| now - r.at > late_after_minutes * 60) {
            evidence.push(match last_ok {
                Some(r) => format!(
                    "last successful run {} ({}), over {late_after_minutes} min",
                    r.run_id,
                    ago(now, r.at)
                ),
                None => "no successful run recorded".into(),
            });
            State::Late
        } else {
            let r = last_ok.expect("checked above");
            evidence.push(format!("last run {} ok {}", r.run_id, ago(now, r.at)));
            if let Some(rc) = recon {
                evidence.push(format!("reconciled {} at {}", rc.verdict, rc.run_id));
            }
            State::Healthy
        };
        if state == State::Failing {
            cause = Some(match recent_change {
                Some(c) => format!(
                    "a source change: {} {}{}{}",
                    c.class,
                    c.table,
                    c.column
                        .as_ref()
                        .map(|x| format!(".{x}"))
                        .unwrap_or_default(),
                    c.proposal
                        .as_ref()
                        .map(|p| format!(" ({p})"))
                        .unwrap_or_default()
                ),
                None if recon.is_some_and(|r| r.verdict == "diverged") => {
                    "the data: bronze no longer matches the source (see the divergences)".into()
                }
                None => "the run itself (see its message)".into(),
            });
        }
        out.push(Health {
            table,
            layer,
            state,
            evidence,
            cause,
            last_success_at: last_ok.map(|r| r.at),
            last_run,
        });
    }
    out
}

/// What changed between two evaluations: `(table, layer, from, to)` — what alerts fire on.
pub fn transitions(
    before: &BTreeMap<String, State>,
    now: &[Health],
) -> Vec<(String, String, Option<State>, State)> {
    now.iter()
        .filter_map(|h| {
            let key = format!("{}|{}", h.table, h.layer);
            let prev = before.get(&key).copied();
            (prev != Some(h.state) && !(prev.is_none() && h.state == State::Healthy))
                .then(|| (h.table.clone(), h.layer.clone(), prev, h.state))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(table: &str, ok: bool, at: i64) -> RunFact {
        RunFact {
            table: table.into(),
            layer: "bronze".into(),
            run_id: format!("r{at}"),
            ok,
            at,
            rows: Some(10),
            duration_ms: Some(5),
            message: (!ok).then(|| "boom".into()),
        }
    }

    fn recon(table: &str, verdict: &str, control: Option<bool>, at: i64) -> ReconFact {
        ReconFact {
            table: table.into(),
            run_id: format!("r{at}"),
            verdict: verdict.into(),
            control_fired: control,
            divergences: vec!["bucket 3".into()],
            at,
        }
    }

    fn state(f: &Facts, now: i64) -> State {
        evaluate(f, now, 120)[0].state
    }

    #[test]
    fn healthy_late_failing_unverified_pending_in_precedence() {
        let mut f = Facts {
            runs: vec![run("t", true, 1000)],
            ..Default::default()
        };
        assert_eq!(state(&f, 1100), State::Healthy);
        assert_eq!(state(&f, 1000 + 121 * 60), State::Late);
        f.reconciliations
            .push(recon("t", "unverified", Some(false), 1000));
        assert_eq!(state(&f, 1100), State::Unverified);
        f.reconciliations.push(recon("t", "diverged", None, 1001));
        assert_eq!(state(&f, 1100), State::Failing);
        f.runs.push(run("t", false, 1002));
        assert_eq!(state(&f, 1100), State::Failing);
        f.schema_states.push(SchemaStateFact {
            table: "t".into(),
            state: "paused".into(),
            proposal: Some("ekos/change/t/1".into()),
        });
        assert_eq!(state(&f, 1100), State::SchemaPending);
    }

    #[test]
    fn a_later_good_run_clears_a_failure_and_the_cause_is_named() {
        let mut f = Facts {
            runs: vec![run("t", false, 1000)],
            ..Default::default()
        };
        f.changes.push(ChangeFact {
            table: "t".into(),
            class: "drop".into(),
            column: Some("c".into()),
            proposal: None,
            at: 900,
        });
        let h = &evaluate(&f, 1100, 120)[0];
        assert_eq!(h.state, State::Failing);
        assert!(h.cause.as_deref().unwrap().contains("drop t.c"), "{h:?}");
        f.runs.push(run("t", true, 1050));
        assert_eq!(state(&f, 1100), State::Healthy);
    }

    #[test]
    fn layers_are_separate() {
        let mut silver = run("t", false, 1000);
        silver.layer = "silver".into();
        let f = Facts {
            runs: vec![run("t", true, 1000), silver],
            ..Default::default()
        };
        let h = evaluate(&f, 1100, 120);
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0].layer.as_str(), h[0].state),
            ("bronze", State::Healthy)
        );
        assert_eq!(
            (h[1].layer.as_str(), h[1].state),
            ("silver", State::Failing)
        );
    }

    #[test]
    fn transitions_fire_once_and_not_for_a_new_healthy_table() {
        let f = Facts {
            runs: vec![run("t", false, 1000), run("u", true, 1000)],
            ..Default::default()
        };
        let h = evaluate(&f, 1100, 120);
        let t = transitions(&BTreeMap::new(), &h);
        assert_eq!(
            t,
            [("t".to_string(), "bronze".to_string(), None, State::Failing)]
        );
        let before: BTreeMap<String, State> = h
            .iter()
            .map(|x| (format!("{}|{}", x.table, x.layer), x.state))
            .collect();
        assert!(transitions(&before, &h).is_empty(), "no change, no alert");
    }
}
