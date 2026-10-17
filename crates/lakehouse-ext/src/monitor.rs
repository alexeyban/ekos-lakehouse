//! Monitoring (RFC 0196): health from the ledger alone, rendered for a person, posted as alerts,
//! pushed as OpenTelemetry metrics, and served to agents over MCP.

use crate::facts::{CHANGE_KIND, METRIC_KIND, RUN_KIND};
use ekos_lakehouse_health::{
    ChangeFact, Facts, Health, ReconFact, RunFact, SchemaStateFact, State, evaluate, transitions,
};
use ekos_ledger::KnowledgeStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// Everything health needs, read from the ledger (RFC 0196 §1).
pub fn facts(store: &dyn KnowledgeStore) -> anyhow::Result<Facts> {
    let mut f = Facts::default();
    for o in store.all_objects()? {
        let ekos_kir::ObjectKind::Custom(kind) = &o.kind else {
            continue;
        };
        let p = serde_json::to_value(&o.properties)?;
        let at = o.created_at.timestamp();
        match kind.as_str() {
            RUN_KIND => f.runs.push(RunFact {
                table: s(&p, "table").unwrap_or_default(),
                layer: s(&p, "layer").unwrap_or_else(|| "bronze".into()),
                run_id: s(&p, "run_id").unwrap_or_default(),
                ok: s(&p, "status").as_deref() == Some("ok"),
                at,
                rows: p.get("rows").and_then(Value::as_i64),
                duration_ms: p.get("duration_ms").and_then(Value::as_u64),
                message: s(&p, "message"),
            }),
            METRIC_KIND => match s(&p, "metric").as_deref() {
                Some("reconciliation") => f.reconciliations.push(ReconFact {
                    table: s(&p, "table").unwrap_or_default(),
                    run_id: s(&p, "run_id").unwrap_or_default(),
                    verdict: s(&p, "verdict").unwrap_or_default(),
                    control_fired: p.get("control_fired").and_then(Value::as_bool),
                    divergences: p["tiers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|t| t["divergences"].as_array().cloned().unwrap_or_default())
                        .filter_map(|d| d.as_str().map(str::to_string))
                        .collect(),
                    at,
                }),
                Some("schema_state") => f.schema_states.push(SchemaStateFact {
                    table: s(&p, "table").unwrap_or_default(),
                    state: s(&p, "state").unwrap_or_else(|| "active".into()),
                    proposal: s(&p, "proposal"),
                }),
                _ => {}
            },
            CHANGE_KIND => f.changes.push(ChangeFact {
                table: s(&p, "table").unwrap_or_default(),
                class: s(&p, "class").unwrap_or_default(),
                column: s(&p, "column"),
                proposal: s(&p, "proposal"),
                at,
            }),
            _ => {}
        }
    }
    Ok(f)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn health(store: &dyn KnowledgeStore, late_after_minutes: i64) -> anyhow::Result<Vec<Health>> {
    Ok(evaluate(&facts(store)?, now(), late_after_minutes))
}

/// The table-by-layer grid.
pub fn render(h: &[Health]) -> String {
    let mut by_table: BTreeMap<&str, BTreeMap<&str, &Health>> = BTreeMap::new();
    for x in h {
        by_table.entry(&x.table).or_default().insert(&x.layer, x);
    }
    let mut out = format!(
        "{:<40} {:<16} {:<16} {:<16}\n",
        "table", "bronze", "silver", "gold"
    );
    for (t, layers) in &by_table {
        let cell = |l: &str| layers.get(l).map(|x| x.state.name()).unwrap_or("-");
        out.push_str(&format!(
            "{t:<40} {:<16} {:<16} {:<16}\n",
            cell("bronze"),
            cell("silver"),
            cell("gold")
        ));
    }
    let bad: Vec<&Health> = h.iter().filter(|x| x.state != State::Healthy).collect();
    if !bad.is_empty() {
        out.push('\n');
    }
    for x in bad {
        out.push_str(&format!("{} {} is {}:\n", x.table, x.layer, x.state.name()));
        for e in &x.evidence {
            out.push_str(&format!("    {e}\n"));
        }
        if let Some(c) = &x.cause {
            out.push_str(&format!("    probable cause: {c}\n"));
        }
    }
    let count = |st: State| h.iter().filter(|x| x.state == st).count();
    out.push_str(&format!(
        "\n{} healthy, {} late, {} unverified, {} failing, {} schema pending\n",
        count(State::Healthy),
        count(State::Late),
        count(State::Unverified),
        count(State::Failing),
        count(State::SchemaPending)
    ));
    out
}

/// Post each state transition once (RFC 0196 §3). The last states seen are kept in
/// `.lakehouse/alerts.json` beside the pipeline repository's run-time state.
pub fn alert(h: &[Health], state_dir: &Path, webhook: Option<&str>) -> anyhow::Result<Vec<String>> {
    let path = state_dir.join("alerts.json");
    let before: BTreeMap<String, State> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut sent = vec![];
    for (table, layer, from, to) in transitions(&before, h) {
        let x = h
            .iter()
            .find(|x| x.table == table && x.layer == layer)
            .expect("from h");
        let text = format!(
            "EKOS Lakehouse: {table} ({layer}) is {}{}.{}{}",
            to.name(),
            from.map(|f| format!(", was {}", f.name()))
                .unwrap_or_default(),
            x.evidence
                .first()
                .map(|e| format!(" {e}."))
                .unwrap_or_default(),
            x.cause
                .as_ref()
                .map(|c| format!(" Probable cause: {c}."))
                .unwrap_or_default()
        );
        if let Some(url) = webhook {
            let body = json!({"text": text, "table": table, "layer": layer, "state": to.name(),
                              "previous": from.map(State::name), "evidence": x.evidence, "cause": x.cause});
            reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?
                .post(url)
                .json(&body)
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|e| anyhow::anyhow!("posting the alert: {e}"))?;
        }
        sent.push(text);
    }
    let now: BTreeMap<String, State> = h
        .iter()
        .map(|x| (format!("{}|{}", x.table, x.layer), x.state))
        .collect();
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&path, serde_json::to_string_pretty(&now)? + "\n")?;
    Ok(sent)
}

/// OTLP/HTTP JSON (`ExportMetricsServiceRequest`) of the current health (RFC 0196 §3).
pub fn otlp(h: &[Health], now: i64) -> Value {
    let ts = (now as u128 * 1_000_000_000).to_string();
    let attrs = |x: &Health| {
        json!([
            {"key": "table", "value": {"stringValue": x.table}},
            {"key": "layer", "value": {"stringValue": x.layer}}
        ])
    };
    let gauge = |name: &str, unit: &str, desc: &str, points: Vec<Value>| json!({"name": name, "unit": unit, "description": desc, "gauge": {"dataPoints": points}});
    let point = |x: &Health, v: Value| {
        let mut p = json!({"attributes": attrs(x), "timeUnixNano": ts});
        match v {
            Value::Number(n) if n.is_f64() => p["asDouble"] = json!(n),
            other => p["asInt"] = json!(other.to_string()),
        }
        p
    };
    let state = h.iter().map(|x| point(x, json!(x.state.level()))).collect();
    let fresh = h
        .iter()
        .filter_map(|x| x.last_success_at.map(|t| point(x, json!(now - t))))
        .collect();
    let rows = h
        .iter()
        .filter_map(|x| {
            x.last_run
                .as_ref()
                .and_then(|r| r.rows)
                .map(|n| point(x, json!(n)))
        })
        .collect();
    let dur = h
        .iter()
        .filter_map(|x| {
            x.last_run
                .as_ref()
                .and_then(|r| r.duration_ms)
                .map(|n| point(x, json!(n)))
        })
        .collect();
    json!({"resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "ekos-lakehouse"}}]},
        "scopeMetrics": [{
            "scope": {"name": "ekos-lakehouse", "version": env!("CARGO_PKG_VERSION")},
            "metrics": [
                gauge("lakehouse.table.state", "1", "0 healthy, 1 late, 2 unverified, 3 failing, 4 schema pending", state),
                gauge("lakehouse.table.freshness", "s", "seconds since the last successful run", fresh),
                gauge("lakehouse.table.rows_written", "{row}", "rows written by the last run", rows),
                gauge("lakehouse.table.run_duration", "ms", "duration of the last run", dur),
            ]
        }]
    }]})
}

pub fn push_metrics(endpoint: &str, body: &Value) -> anyhow::Result<()> {
    let url = format!("{}/v1/metrics", endpoint.trim_end_matches('/'));
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?
        .post(&url)
        .json(body)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| anyhow::anyhow!("pushing metrics to {url}: {e}"))?;
    Ok(())
}

/// dbt's `run_results.json` + `manifest.json` → one result per model (RFC 0196 §1): a model is
/// ok when it built and every test that depends on it passed.
pub fn dbt_results(target: &Path) -> anyhow::Result<Vec<crate::facts::RunReport>> {
    let rr: Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("run_results.json"))?)?;
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("manifest.json"))?)?;
    let mut models: BTreeMap<String, (bool, f64, Option<i64>, Vec<String>)> = BTreeMap::new();
    let mut tests: Vec<(String, bool, String)> = vec![];
    for r in rr["results"].as_array().into_iter().flatten() {
        let id = r["unique_id"].as_str().unwrap_or_default();
        let status = r["status"].as_str().unwrap_or_default();
        if id.starts_with("model.") || id.starts_with("snapshot.") {
            let rows = r["adapter_response"]["rows_affected"].as_i64();
            models.insert(
                id.to_string(),
                (
                    status == "success",
                    r["execution_time"].as_f64().unwrap_or(0.0),
                    rows,
                    vec![],
                ),
            );
        } else if id.starts_with("test.") {
            tests.push((
                id.to_string(),
                status == "pass",
                r["message"].as_str().unwrap_or(status).to_string(),
            ));
        }
    }
    for (id, passed, msg) in tests {
        if passed {
            continue;
        }
        for dep in manifest["nodes"][&id]["depends_on"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(m) = models.get_mut(dep.as_str().unwrap_or_default()) {
                m.0 = false;
                m.3.push(format!("{}: {msg}", id.rsplit('.').nth(1).unwrap_or(&id)));
            }
        }
    }
    Ok(models
        .into_iter()
        .map(|(id, (ok, secs, rows, failures))| {
            let name = id.rsplit('.').next().unwrap_or(&id).to_string();
            let (layer, table) = match name.split_once("__") {
                Some(("gold", m)) => ("gold", format!("gold.{m}")),
                Some((schema, t)) => ("silver", format!("{schema}.{t}")),
                None => ("silver", name.clone()),
            };
            crate::facts::RunReport {
                table,
                layer: layer.into(),
                run_id: String::new(),
                status: if ok { "ok" } else { "failed" }.into(),
                rows,
                duration_ms: Some((secs * 1000.0) as u64),
                snapshot_id: None,
                message: (!failures.is_empty()).then(|| failures.join("; ")),
            }
        })
        .collect())
}
