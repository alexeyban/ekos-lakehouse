//! Dashboard health (EKOS RFC 0258 §3.5): a published dashboard's charts, the marts and tables
//! each one reads, and the table health of RFC 0196 reduced to one state per chart and one per
//! dashboard — with the facts that put it there and "data as of". Computed on read, never stored
//! as a decision. Pure: the caller reads the ledger and supplies the table health.
//!
//! What is evaluated, and what is not yet (each chart says so in `not_evaluated`):
//!
//! - `failing`, `unverified`, `late` from table health (RFC 0196) and mart facts (RFC 0228).
//! - The chart reconciliation of RFC 0248 §2.4 does not exist yet; the mart's own
//!   reconciliation with silver (RFC 0228 §2.5) stands in for it on a chart that reads a mart.
//! - `drifted` needs drift detection (RFC 0258 phase 4): not evaluated.
//! - `needs review` needs the terms a chart was compiled with (RFC 0258 §3.1's bundle) and
//!   the requirement approval's validity against the pipeline repository: not evaluated.

use crate::{Health, State};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Worse is greater. A chart's state is the worst condition that holds (RFC 0258 §3.5 lists
/// them first-that-holds, worst first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashState {
    Trusted,
    NeedsReview,
    Drifted,
    Late,
    Unverified,
    Failing,
}

impl DashState {
    pub fn name(self) -> &'static str {
        match self {
            DashState::Trusted => "trusted",
            DashState::NeedsReview => "needs_review",
            DashState::Drifted => "drifted",
            DashState::Late => "late",
            DashState::Unverified => "unverified",
            DashState::Failing => "failing",
        }
    }
}

/// What the evaluation could not judge, said on every chart so "trusted" is read for what it is.
pub const NOT_EVALUATED: [&str; 3] = [
    "chart reconciliation (RFC 0248 §2.4 is not built; the mart's reconciliation with silver stands in)",
    "drift from the published bundle (RFC 0258 phase 4)",
    "stale instructions or synonyms and requirement approval validity (no bundle records a chart's terms)",
];

/// One chart as read from Superset: the tables its dataset reads, as the SQL names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartRecord {
    pub chart_id: i64,
    pub name: String,
    /// The Superset dataset, `schema.table_name` (a virtual dataset's name is EKOS's own).
    pub dataset: String,
    /// `schema.table` or `catalog.schema.table`, as written; lower case.
    pub reads: Vec<String>,
    /// Why the dataset's tables could not be read (unparsable SQL, no dataset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
}

/// One dashboard as `lakehouse dashboards sync` recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardRecord {
    pub dashboard_id: i64,
    pub title: String,
    pub url: String,
    pub charts: Vec<ChartRecord>,
    /// The chain's tables (`layer:table`) that had a lakehouse fact when the dashboard was
    /// recorded. The ledger is append-only, so one missing on a later read was hidden by the
    /// reader's scope: the chain leaves their scope and the dashboard is not shown.
    #[serde(default)]
    pub chain_known: Vec<String>,
    /// Set when the dashboard was no longer in Superset at the last sync.
    #[serde(default)]
    pub removed: bool,
}

/// One measure of a mart's reconciliation with silver (RFC 0228 §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MartRecon {
    pub measure: String,
    pub mart_total: String,
    pub silver_total: String,
    pub equal: bool,
    /// The gold run it checked, when known.
    pub gold_run_id: Option<String>,
    /// The reconciliation's own time stamp: measures of one reconciliation share it.
    pub at: String,
}

/// What the ledger says about one mart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MartFacts {
    /// The silver tables it reads, from its requirement's approval (RFC 0215). `None`: unknown.
    pub needs: Option<Vec<String>>,
    pub reconciliations: Vec<MartRecon>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub state: DashState,
    pub fact: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainNode {
    pub layer: String,
    pub table: String,
    /// The table's RFC 0196 state, or `none` when it has no fact.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartHealth {
    pub chart_id: i64,
    pub name: String,
    pub dataset: String,
    pub state: DashState,
    /// Every condition found, worst first — not only the one that decided the state.
    pub evidence: Vec<Finding>,
    /// The earliest last successful load among the bronze (source) tables on the chain.
    pub data_as_of: Option<i64>,
    pub chain: Vec<ChainNode>,
    pub not_evaluated: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardHealth {
    pub dashboard_id: i64,
    pub title: String,
    pub url: String,
    pub state: DashState,
    /// The findings of its charts, worst first, each prefixed with the chart.
    pub evidence: Vec<String>,
    pub data_as_of: Option<i64>,
    pub charts: Vec<ChartHealth>,
}

/// `layer` + `table` — the key of [`DashboardRecord::chain_known`].
pub fn chain_key(layer: &str, table: &str) -> String {
    format!("{layer}:{}", table.to_ascii_lowercase())
}

/// A table name as a dataset's SQL writes it → the lakehouse table and layer it is, when it is
/// one. The catalog is dropped. `gold[_tag].m` → gold `gold.m`; `silver_[tag_]<schema>.t` →
/// silver `<schema>.t`; `bronze_[tag_]<schema>.t` → bronze `<schema>.t` — the schema chosen
/// among the tables health knows, because a tag and a schema are both `_`-joined.
pub fn resolve_ref(
    reference: &str,
    known: &BTreeSet<(String, String)>,
) -> Option<(String, String)> {
    let parts: Vec<&str> = reference.split('.').collect();
    let (schema, name) = match parts.as_slice() {
        [.., s, n] => (s.to_ascii_lowercase(), n.to_ascii_lowercase()),
        _ => return None,
    };
    if schema == "gold" || schema.starts_with("gold_") {
        return Some(("gold".into(), format!("gold.{name}")));
    }
    for layer in ["silver", "bronze"] {
        let Some(rest) = schema.strip_prefix(&format!("{layer}_")) else {
            continue;
        };
        let found = known.iter().find(|(l, t)| {
            l == layer
                && t.rsplit_once('.').is_some_and(|(s, n)| {
                    n == name && (rest == s || rest.ends_with(&format!("_{s}")))
                })
        });
        return Some(match found {
            Some((l, t)) => (l.clone(), t.clone()),
            None => (layer.to_string(), format!("{rest}.{name}")),
        });
    }
    None
}

fn table_finding(h: &Health) -> Option<Finding> {
    let what = |s: &str| format!("{} {} is {s}", h.layer, h.table);
    let mut fact = match h.state {
        State::Healthy => return None,
        State::SchemaPending => what("waiting for a schema change to be approved"),
        other => what(other.name()),
    };
    if let Some(e) = h.evidence.first() {
        fact.push_str(&format!(": {e}"));
    }
    if let Some(c) = &h.cause {
        fact.push_str(&format!(" (probable cause: {c})"));
    }
    Some(Finding {
        state: match h.state {
            State::Failing => DashState::Failing,
            State::Unverified | State::SchemaPending => DashState::Unverified,
            State::Late => DashState::Late,
            State::Healthy => unreachable!("returned above"),
        },
        fact,
        layer: Some(h.layer.clone()),
        table: Some(h.table.clone()),
    })
}

struct Ctx<'a> {
    tables: BTreeMap<(String, String), &'a Health>,
    marts: &'a BTreeMap<String, MartFacts>,
}

impl Ctx<'_> {
    fn get(&self, layer: &str, table: &str) -> Option<&Health> {
        self.tables
            .get(&(layer.to_string(), table.to_ascii_lowercase()))
            .copied()
    }
}

/// The mart's reconciliation for its latest build, as findings.
fn mart_findings(ctx: &Ctx<'_>, mart_table: &str, out: &mut Vec<Finding>) -> Vec<String> {
    let mart = mart_table.strip_prefix("gold.").unwrap_or(mart_table);
    let facts = ctx.marts.get(mart).cloned().unwrap_or_default();
    let finding = |state, fact: String| Finding {
        state,
        fact,
        layer: Some("gold".into()),
        table: Some(mart_table.to_string()),
    };
    let last_run = ctx.get("gold", mart_table).and_then(|h| h.last_run.clone());
    if let Some(run) = last_run.as_ref().filter(|r| r.ok) {
        let latest_at = facts
            .reconciliations
            .iter()
            .filter(|r| r.gold_run_id.as_deref().is_none_or(|id| id == run.run_id))
            .map(|r| r.at.clone())
            .max();
        match latest_at {
            None => out.push(finding(
                DashState::Unverified,
                format!(
                    "{mart_table} was built in run {} and not reconciled with silver since",
                    run.run_id
                ),
            )),
            Some(at) => {
                let differ: Vec<String> = facts
                    .reconciliations
                    .iter()
                    .filter(|r| r.at == at && !r.equal)
                    .map(|r| {
                        format!(
                            "{}: mart {} / silver {}",
                            r.measure, r.mart_total, r.silver_total
                        )
                    })
                    .collect();
                if !differ.is_empty() {
                    out.push(finding(
                        DashState::Failing,
                        format!(
                            "{mart_table} diverged from silver at {at}: {}",
                            differ.join("; ")
                        ),
                    ));
                }
            }
        }
    }
    match facts.needs {
        Some(n) => n,
        None => {
            out.push(finding(
                DashState::Unverified,
                format!(
                    "the silver tables {mart_table} reads are not recorded (no approved requirement)"
                ),
            ));
            vec![]
        }
    }
}

fn chart_health(ctx: &Ctx<'_>, known: &BTreeSet<(String, String)>, c: &ChartRecord) -> ChartHealth {
    let mut evidence = vec![];
    let mut chain: Vec<(String, String)> = vec![];
    let push = |chain: &mut Vec<(String, String)>, l: &str, t: &str| {
        let k = (l.to_string(), t.to_ascii_lowercase());
        if !chain.contains(&k) {
            chain.push(k);
        }
    };
    if let Some(why) = &c.unreadable {
        evidence.push(Finding {
            state: DashState::Unverified,
            fact: format!(
                "the tables of dataset {} could not be read: {why}",
                c.dataset
            ),
            layer: None,
            table: None,
        });
    }
    for r in &c.reads {
        match resolve_ref(r, known) {
            None => evidence.push(Finding {
                state: DashState::Unverified,
                fact: format!("reads {r}, which is not a lakehouse table (bronze, silver or gold)"),
                layer: None,
                table: Some(r.clone()),
            }),
            Some((layer, table)) => {
                push(&mut chain, &layer, &table);
                let upstream = match layer.as_str() {
                    "gold" => mart_findings(ctx, &table, &mut evidence),
                    "silver" => vec![table.clone()],
                    _ => vec![],
                };
                for t in upstream {
                    push(&mut chain, "silver", &t);
                    push(&mut chain, "bronze", &t);
                }
                if layer == "silver" {
                    push(&mut chain, "bronze", &table);
                }
            }
        }
    }
    if c.reads.is_empty() && c.unreadable.is_none() {
        evidence.push(Finding {
            state: DashState::Unverified,
            fact: format!("dataset {} reads no table EKOS could name", c.dataset),
            layer: None,
            table: None,
        });
    }
    let mut nodes = vec![];
    let mut sources: Vec<Option<i64>> = vec![];
    for (layer, table) in &chain {
        let h = ctx.get(layer, table);
        match h {
            None => evidence.push(Finding {
                state: DashState::Unverified,
                fact: format!("{layer} {table} has no lakehouse fact (never loaded or built)"),
                layer: Some(layer.clone()),
                table: Some(table.clone()),
            }),
            Some(h) => evidence.extend(table_finding(h)),
        }
        if layer == "bronze" {
            sources.push(h.and_then(|h| h.last_success_at));
        }
        nodes.push(ChainNode {
            layer: layer.clone(),
            table: h.map(|h| h.table.clone()).unwrap_or_else(|| table.clone()),
            state: h.map(|h| h.state.name()).unwrap_or("none").to_string(),
        });
    }
    // Worst first; the order within a state is the chain's.
    evidence.sort_by_key(|f| std::cmp::Reverse(f.state));
    let data_as_of = if sources.is_empty() || sources.iter().any(Option::is_none) {
        None
    } else {
        sources.into_iter().flatten().min()
    };
    ChartHealth {
        chart_id: c.chart_id,
        name: c.name.clone(),
        dataset: c.dataset.clone(),
        state: evidence.first().map_or(DashState::Trusted, |f| f.state),
        evidence,
        data_as_of,
        chain: nodes,
        not_evaluated: NOT_EVALUATED.iter().map(|s| s.to_string()).collect(),
    }
}

/// Every recorded dashboard that is still in Superset and whose chain the reader can see,
/// sorted by id. `tables` is RFC 0196's evaluation; `marts` by mart name.
pub fn evaluate(
    records: &[DashboardRecord],
    tables: &[Health],
    marts: &BTreeMap<String, MartFacts>,
) -> Vec<DashboardHealth> {
    let ctx = Ctx {
        tables: tables
            .iter()
            .map(|h| ((h.layer.clone(), h.table.to_ascii_lowercase()), h))
            .collect(),
        marts,
    };
    let known: BTreeSet<(String, String)> = ctx.tables.keys().cloned().collect();
    let mut out: Vec<DashboardHealth> = records
        .iter()
        .filter(|d| !d.removed)
        .filter(|d| {
            d.chain_known.iter().all(|k| {
                k.split_once(':')
                    .is_some_and(|(l, t)| known.contains(&(l.to_string(), t.to_string())))
            })
        })
        .map(|d| {
            let charts: Vec<ChartHealth> = d
                .charts
                .iter()
                .map(|c| chart_health(&ctx, &known, c))
                .collect();
            let state = charts
                .iter()
                .map(|c| c.state)
                .max()
                .unwrap_or(DashState::Unverified);
            let mut findings: Vec<(DashState, String)> = charts
                .iter()
                .flat_map(|c| {
                    c.evidence
                        .iter()
                        .map(move |f| (f.state, format!("{}: {}", c.name, f.fact)))
                })
                .collect();
            if charts.is_empty() {
                findings.push((DashState::Unverified, "the dashboard has no charts".into()));
            }
            findings.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
            let as_of: Vec<Option<i64>> = charts.iter().map(|c| c.data_as_of).collect();
            DashboardHealth {
                dashboard_id: d.dashboard_id,
                title: d.title.clone(),
                url: d.url.clone(),
                state,
                evidence: findings.into_iter().map(|(_, f)| f).collect(),
                data_as_of: if as_of.is_empty() || as_of.iter().any(Option::is_none) {
                    None
                } else {
                    as_of.into_iter().flatten().min()
                },
                charts,
            }
        })
        .collect();
    out.sort_by_key(|d| d.dashboard_id);
    out
}

/// The chain's tables (`layer:table`) a record names that have table health now — what
/// `sync` stores as [`DashboardRecord::chain_known`].
pub fn known_chain(
    record: &DashboardRecord,
    tables: &[Health],
    marts: &BTreeMap<String, MartFacts>,
) -> Vec<String> {
    let probe = DashboardRecord {
        chain_known: vec![],
        removed: false,
        ..record.clone()
    };
    let mut out: BTreeSet<String> = BTreeSet::new();
    for d in evaluate(&[probe], tables, marts) {
        for c in d.charts {
            for n in c.chain.into_iter().filter(|n| n.state != "none") {
                out.insert(chain_key(&n.layer, &n.table));
            }
        }
    }
    out.into_iter().collect()
}

/// The alert rule (RFC 0258 §3.5): a dashboard that leaves `trusted` — or is first seen in
/// another state — once per transition. `before` is keyed by dashboard id.
pub fn left_trusted(
    before: &BTreeMap<String, DashState>,
    now: &[DashboardHealth],
) -> Vec<(i64, Option<DashState>, DashState)> {
    now.iter()
        .filter_map(|d| {
            let prev = before.get(&d.dashboard_id.to_string()).copied();
            (d.state != DashState::Trusted && matches!(prev, None | Some(DashState::Trusted)))
                .then_some((d.dashboard_id, prev, d.state))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Facts, RunFact, evaluate as table_health};

    const NOW: i64 = 100_000;

    fn run(table: &str, layer: &str, ok: bool, at: i64) -> RunFact {
        RunFact {
            table: table.into(),
            layer: layer.into(),
            run_id: format!("{layer}-{at}"),
            ok,
            at,
            rows: Some(1),
            duration_ms: None,
            message: (!ok).then(|| "Trino: table not found".into()),
        }
    }

    fn chart(id: i64, reads: &[&str]) -> ChartRecord {
        ChartRecord {
            chart_id: id,
            name: format!("chart {id}"),
            dataset: format!("public.ai_abc123_{id}"),
            reads: reads.iter().map(|s| s.to_string()).collect(),
            unreadable: None,
        }
    }

    fn dash(charts: Vec<ChartRecord>) -> DashboardRecord {
        DashboardRecord {
            dashboard_id: 7,
            title: "Sales".into(),
            url: "/superset/dashboard/7/".into(),
            charts,
            chain_known: vec![],
            removed: false,
        }
    }

    fn mart(needs: &[&str], run: &str, equal: bool) -> BTreeMap<String, MartFacts> {
        BTreeMap::from([(
            "sales".to_string(),
            MartFacts {
                needs: Some(needs.iter().map(|s| s.to_string()).collect()),
                reconciliations: vec![MartRecon {
                    measure: "total".into(),
                    mart_total: "10".into(),
                    silver_total: if equal { "10" } else { "9" }.into(),
                    equal,
                    gold_run_id: Some(run.into()),
                    at: "2026-11-11T06:00:00Z".into(),
                }],
            },
        )])
    }

    /// Two sources (orders, customers) through silver into the mart `sales`.
    fn healthy_runs() -> Vec<RunFact> {
        vec![
            run("public.orders", "bronze", true, NOW - 600),
            run("public.customers", "bronze", true, NOW - 1800),
            run("public.orders", "silver", true, NOW - 500),
            run("public.customers", "silver", true, NOW - 500),
            run("gold.sales", "gold", true, NOW - 400),
        ]
    }

    fn eval(runs: Vec<RunFact>, marts: &BTreeMap<String, MartFacts>) -> DashboardHealth {
        let h = table_health(
            &Facts {
                runs,
                ..Default::default()
            },
            NOW,
            120,
        );
        let d = dash(vec![chart(1, &["iceberg.gold.sales"])]);
        evaluate(&[d], &h, marts).remove(0)
    }

    #[test]
    fn trusted_with_data_as_of_the_earliest_of_two_sources() {
        let marts = mart(&["public.orders", "public.customers"], "gold-99600", true);
        let d = eval(healthy_runs(), &marts);
        assert_eq!(d.state, DashState::Trusted, "{:#?}", d.evidence);
        assert_eq!(d.data_as_of, Some(NOW - 1800), "customers loaded earlier");
        let layers: Vec<_> = d.charts[0]
            .chain
            .iter()
            .map(|n| format!("{}:{}", n.layer, n.table))
            .collect();
        assert_eq!(
            layers,
            [
                "gold:gold.sales",
                "silver:public.orders",
                "bronze:public.orders",
                "silver:public.customers",
                "bronze:public.customers"
            ]
        );
        assert_eq!(d.charts[0].not_evaluated.len(), NOT_EVALUATED.len());
    }

    #[test]
    fn late_and_failing_is_failing_with_both_facts_listed() {
        let marts = mart(&["public.orders", "public.customers"], "gold-99600", true);
        let mut runs = healthy_runs();
        // customers' last success is 3 h old: late; orders' last load failed.
        runs[1] = run("public.customers", "bronze", true, NOW - 3 * 3600);
        runs.push(run("public.orders", "bronze", false, NOW - 60));
        let d = eval(runs, &marts);
        assert_eq!(d.state, DashState::Failing);
        let ev = d.evidence.join("\n");
        assert!(ev.contains("bronze public.orders is failing"), "{ev}");
        assert!(ev.contains("Trino: table not found"), "{ev}");
        assert!(ev.contains("bronze public.customers is late"), "{ev}");
        assert!(
            d.evidence[0].contains("failing"),
            "worst first: {:?}",
            d.evidence
        );
    }

    #[test]
    fn a_planted_failed_load_turns_it_failing_and_recovery_returns_it_to_trusted() {
        let marts = mart(&["public.orders", "public.customers"], "gold-99600", true);
        let mut runs = healthy_runs();
        runs.push(run("public.orders", "bronze", false, NOW - 60));
        let failing = eval(runs.clone(), &marts);
        assert_eq!(failing.state, DashState::Failing);
        assert_eq!(
            failing.charts[0].evidence[0].table.as_deref(),
            Some("public.orders")
        );
        runs.push(run("public.orders", "bronze", true, NOW - 30));
        assert_eq!(eval(runs, &marts).state, DashState::Trusted);
    }

    #[test]
    fn mart_divergence_and_a_build_not_reconciled_since() {
        let d = eval(
            healthy_runs(),
            &mart(&["public.orders", "public.customers"], "gold-99600", false),
        );
        assert_eq!(d.state, DashState::Failing);
        assert!(
            d.evidence[0].contains("total: mart 10 / silver 9"),
            "{:?}",
            d.evidence
        );
        let d = eval(
            healthy_runs(),
            &mart(&["public.orders", "public.customers"], "gold-older", true),
        );
        assert_eq!(d.state, DashState::Unverified);
        assert!(d.evidence[0].contains("not reconciled"), "{:?}", d.evidence);
        let d = eval(healthy_runs(), &BTreeMap::new());
        assert_eq!(
            d.state,
            DashState::Unverified,
            "no requirement: chain unknown"
        );
    }

    #[test]
    fn references_resolve_to_layers() {
        let known: BTreeSet<(String, String)> = [
            ("silver".to_string(), "public.film".to_string()),
            ("silver".to_string(), "adempiere.c_order".to_string()),
        ]
        .into();
        let r = |s: &str| resolve_ref(s, &known);
        assert_eq!(r("gold.sales"), Some(("gold".into(), "gold.sales".into())));
        assert_eq!(
            r("iceberg.gold_sales.m"),
            Some(("gold".into(), "gold.m".into()))
        );
        assert_eq!(
            r("silver_sales_public.film"),
            Some(("silver".into(), "public.film".into())),
            "a layer tag before the schema"
        );
        assert_eq!(
            r("iceberg.silver_ADEMPIERE.C_ORDER"),
            Some(("silver".into(), "adempiere.c_order".into()))
        );
        assert_eq!(
            r("bronze_public.orders"),
            Some(("bronze".into(), "public.orders".into()))
        );
        assert_eq!(
            r("public.orders"),
            None,
            "the source itself is not on a chain"
        );
    }

    #[test]
    fn a_chart_outside_the_lakehouse_is_unverified() {
        let h = table_health(&Facts::default(), NOW, 120);
        let d = dash(vec![chart(1, &["adempiere.c_order"])]);
        let out = evaluate(&[d], &h, &BTreeMap::new());
        assert_eq!(out[0].state, DashState::Unverified);
        assert!(out[0].evidence[0].contains("not a lakehouse table"));
    }

    #[test]
    fn a_dashboard_whose_chain_leaves_the_readers_scope_is_not_shown() {
        let marts = mart(&["public.orders", "public.customers"], "gold-99600", true);
        let all = table_health(
            &Facts {
                runs: healthy_runs(),
                ..Default::default()
            },
            NOW,
            120,
        );
        let mut d = dash(vec![chart(1, &["gold.sales"])]);
        d.chain_known = known_chain(&d, &all, &marts);
        assert_eq!(d.chain_known.len(), 5);
        assert_eq!(evaluate(std::slice::from_ref(&d), &all, &marts).len(), 1);
        // A reader who cannot see customers' facts.
        let seen: Vec<Health> = all
            .iter()
            .filter(|h| h.table != "public.customers")
            .cloned()
            .collect();
        assert!(evaluate(&[d.clone()], &seen, &marts).is_empty());
        d.removed = true;
        assert!(
            evaluate(&[d], &all, &marts).is_empty(),
            "removed from Superset"
        );
    }

    #[test]
    fn the_alert_fires_once_when_a_dashboard_leaves_trusted() {
        let marts = mart(&["public.orders", "public.customers"], "gold-99600", true);
        let ok = eval(healthy_runs(), &marts);
        let mut runs = healthy_runs();
        runs.push(run("public.orders", "bronze", false, NOW - 60));
        let bad = eval(runs, &marts);
        let key = |d: &DashboardHealth| BTreeMap::from([(d.dashboard_id.to_string(), d.state)]);
        assert!(left_trusted(&BTreeMap::new(), std::slice::from_ref(&ok)).is_empty());
        assert_eq!(
            left_trusted(&key(&ok), std::slice::from_ref(&bad)),
            [(7, Some(DashState::Trusted), DashState::Failing)]
        );
        assert!(left_trusted(&key(&bad), std::slice::from_ref(&bad)).is_empty());
        assert_eq!(
            left_trusted(&BTreeMap::new(), std::slice::from_ref(&bad)).len(),
            1,
            "first seen untrusted"
        );
    }
}
