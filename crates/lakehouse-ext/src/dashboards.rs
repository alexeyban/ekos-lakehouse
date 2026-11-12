//! EKOS RFC 0258 phase 3: dashboard health.
//!
//! RFC 0248 §2.4 says a published dashboard is a Superset dashboard plus a YAML bundle in git.
//! The bundle was never built: the Superset fork's "Ask your data" page publishes dashboards
//! itself (a virtual dataset `ai_<6 hex>_<n>` per chart, holding the chart's SQL) and records
//! nothing anywhere else. So `lakehouse dashboards sync` reads Superset's REST API — dashboards,
//! their charts and datasets — and records the minimum health needs as one ledger fact per
//! dashboard: its charts, and the tables each chart's dataset reads, as the SQL names them. Health
//! itself is computed on read from that and the facts of RFC 0196 / RFC 0228
//! ([`ekos_lakehouse_health::dashboard`]); nothing here stores a state.
//!
//! The ledger is the only input of `status` and of the MCP tool. Superset is reached by `sync`
//! (reads only) and by `publish-health` (writes a header and the certification), the latter only
//! when `superset-header = true`.

use crate::facts::{APPROVAL_KIND, METRIC_KIND};
use ekos_kir::{KirId, KirObject, ObjectKind};
use ekos_lakehouse_health::dashboard::{
    self as dh, ChartRecord, DashState, DashboardHealth, DashboardRecord, MartFacts, MartRecon,
};
use ekos_ledger::KnowledgeStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// `[extensions.lakehouse.dashboards]`. Credentials are named by the variable that holds them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DashboardsSettings {
    /// Superset, e.g. `http://127.0.0.1:8088`.
    pub superset_url: Option<String>,
    /// The variable holding an access token (sent as a bearer token).
    pub superset_token_env: Option<String>,
    /// Or a database user and the variable holding its password (`/api/v1/security/login`).
    pub superset_user: Option<String>,
    pub superset_password_env: Option<String>,
    /// Which dashboards: `ekos` (default — every dataset is one the fork published,
    /// `ai_<6 hex>_<n>`) or `all`.
    pub select: Option<String>,
    /// These dashboard ids as well, whatever `select` says.
    pub ids: Vec<i64>,
    /// Write the health onto each dashboard: a markdown header and the certification. Default
    /// false: `publish-health` refuses, so nothing in Superset changes unless this is set.
    pub superset_header: Option<bool>,
    /// The console page the header links to (the chain), e.g. `https://ekos.example/ext/lakehouse`.
    pub console_url: Option<String>,
    /// EKOS RFC 0258 phase 1: the Superset database (its id) new datasets are created on.
    pub superset_database_id: Option<i64>,
    /// The schema those datasets run in (default: the gold schema).
    pub superset_schema: Option<String>,
    /// Who may merge a change into someone else's dashboard, besides its author.
    pub reviewers: Vec<String>,
    /// How a mart added or changed for a dashboard is built: `dagster` (default: the description
    /// is written and `lakehouse marts build` builds it) or `trino` (`CREATE OR REPLACE TABLE …
    /// AS` on the engine — for a sandbox without Dagster).
    pub mart_build: Option<String>,
    /// RFC 0258 §3.3: an abandoned branch is closed after this many days (default 30).
    pub branch_ttl_days: Option<i64>,
}

pub const RECORD_METRIC: &str = "dashboard";
/// The certifier EKOS writes. A certification by anyone else is never removed.
pub const CERTIFIER: &str = "EKOS (pipeline health)";
pub const HEADER_ROW: &str = "ROW-ekos-health";
pub const HEADER_MARKDOWN: &str = "MARKDOWN-ekos-health";

fn det_id(seed: &str) -> KirId {
    KirId(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        seed.as_bytes(),
    ))
}

/// One id per dashboard: the ledger's current version is the dashboard as last read.
pub fn record_id(dashboard_id: i64) -> KirId {
    det_id(&format!("lakehouse-dashboard:{dashboard_id}"))
}

// ─────────────────────────────── the ledger ───────────────────────────────

pub fn record(store: &dyn KnowledgeStore, run: &str, r: &DashboardRecord) -> anyhow::Result<()> {
    store.set_write_context(Some(ekos_ledger::provenance::WriteContext {
        run_id: run.to_string(),
        stage: "lakehouse:dashboards".into(),
        source_artifact_id: None,
    }));
    let out = (|| {
        let mut obj = KirObject::new(
            format!("dashboard {} {}", r.dashboard_id, r.title),
            ObjectKind::Custom(METRIC_KIND.into()),
        );
        obj.id = record_id(r.dashboard_id);
        obj.properties.insert(
            "table".into(),
            json!(format!("dashboard:{}", r.dashboard_id)),
        );
        obj.properties.insert("metric".into(), json!(RECORD_METRIC));
        obj.properties.insert("run_id".into(), json!(run));
        if let Value::Object(m) = serde_json::to_value(r)? {
            obj.properties.extend(m);
        }
        store.append_object(&obj)?;
        Ok(())
    })();
    store.set_write_context(None);
    out
}

fn prop<'a>(o: &'a KirObject, k: &str) -> Option<&'a str> {
    o.properties.get(k).and_then(Value::as_str)
}

/// The recorded dashboards and what the ledger says of each mart, in one pass.
pub fn read(
    store: &dyn ekos_ledger::KnowledgeReader,
) -> anyhow::Result<(Vec<DashboardRecord>, BTreeMap<String, MartFacts>)> {
    let mut records: BTreeMap<i64, (i64, DashboardRecord)> = BTreeMap::new();
    let mut marts: BTreeMap<String, MartFacts> = BTreeMap::new();
    for o in store.all_objects()? {
        let ObjectKind::Custom(kind) = &o.kind else {
            continue;
        };
        let at = o.created_at.timestamp_micros();
        match (kind.as_str(), prop(&o, "metric"), prop(&o, "subject")) {
            (METRIC_KIND, Some(RECORD_METRIC), _) => {
                let v = serde_json::to_value(&o.properties)?;
                if let Ok(r) = serde_json::from_value::<DashboardRecord>(v) {
                    let id = r.dashboard_id;
                    if records.get(&id).is_none_or(|(t, _)| at >= *t) {
                        records.insert(id, (at, r));
                    }
                }
            }
            (METRIC_KIND, Some("mart_reconcile"), _) => {
                let Some(mart) = prop(&o, "table").and_then(|t| t.strip_prefix("gold.")) else {
                    continue;
                };
                marts
                    .entry(mart.to_string())
                    .or_default()
                    .reconciliations
                    .push(MartRecon {
                        measure: prop(&o, "measure").unwrap_or_default().into(),
                        mart_total: prop(&o, "mart_total").unwrap_or_default().into(),
                        silver_total: prop(&o, "silver_total").unwrap_or_default().into(),
                        equal: o
                            .properties
                            .get("equal")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        gold_run_id: prop(&o, "gold_run_id").map(str::to_string),
                        at: prop(&o, "at").unwrap_or_default().into(),
                    });
            }
            (APPROVAL_KIND, _, Some("mart requirement")) => {
                let Some(mart) = prop(&o, "mart") else {
                    continue;
                };
                let needs = o
                    .properties
                    .get("needs")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                marts.entry(mart.to_string()).or_default().needs = Some(needs);
            }
            _ => {}
        }
    }
    Ok((records.into_values().map(|(_, r)| r).collect(), marts))
}

/// Every dashboard's health, from the ledger alone.
pub fn health(
    store: &dyn ekos_ledger::KnowledgeStore,
    late_after_minutes: i64,
) -> anyhow::Result<Vec<DashboardHealth>> {
    let tables = crate::monitor::health(store, late_after_minutes)?;
    let (records, marts) = read(store)?;
    Ok(dh::evaluate(&records, &tables, &marts))
}

// ─────────────────────────────── reading Superset (pure) ───────────────────────────────

/// A dataset the fork published: `ai_<6 hex>_<n>` (`superset/ekos_ai/service.py::_build`).
pub fn is_fork_dataset(table_name: &str) -> bool {
    let Some(rest) = table_name.strip_prefix("ai_") else {
        return false;
    };
    let Some((tag, n)) = rest.split_once('_') else {
        return false;
    };
    tag.len() == 6
        && tag.chars().all(|c| c.is_ascii_hexdigit())
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

/// Whether a dashboard is one EKOS health covers.
pub fn selected(s: &DashboardsSettings, dashboard_id: i64, datasets: &[Value]) -> bool {
    if s.ids.contains(&dashboard_id) || s.select.as_deref() == Some("all") {
        return true;
    }
    !datasets.is_empty()
        && datasets
            .iter()
            .all(|d| d["table_name"].as_str().is_some_and(is_fork_dataset))
}

#[derive(Default)]
struct Relations {
    ctes: Vec<String>,
    names: Vec<String>,
}

impl sqlparser::ast::Visitor for Relations {
    type Break = ();
    fn pre_visit_query(&mut self, q: &sqlparser::ast::Query) -> std::ops::ControlFlow<()> {
        if let Some(w) = &q.with {
            for c in &w.cte_tables {
                self.ctes.push(c.alias.name.value.to_ascii_lowercase());
            }
        }
        std::ops::ControlFlow::Continue(())
    }
    fn pre_visit_relation(&mut self, r: &sqlparser::ast::ObjectName) -> std::ops::ControlFlow<()> {
        let name =
            r.0.iter()
                .map(|i| i.value.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(".");
        if !self.names.contains(&name) {
            self.names.push(name);
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// The tables a dataset's SQL reads, lower case, without CTE names; a one-part name is qualified
/// with the dataset's schema.
pub fn sql_reads(sql: &str, schema: Option<&str>) -> Result<Vec<String>, String> {
    use sqlparser::ast::Visit;
    let dialect = sqlparser::dialect::GenericDialect {};
    let statements =
        sqlparser::parser::Parser::parse_sql(&dialect, sql).map_err(|e| e.to_string())?;
    let mut v = Relations::default();
    let _ = statements.visit(&mut v);
    Ok(v.names
        .into_iter()
        .filter(|n| !v.ctes.contains(n))
        .map(|n| match (n.contains('.'), schema) {
            (false, Some(s)) if !s.is_empty() => format!("{}.{n}", s.to_ascii_lowercase()),
            _ => n,
        })
        .collect())
}

/// `form_data.datasource` is `<id>__table`.
fn chart_dataset_id(chart: &Value) -> Option<i64> {
    let fd = &chart["form_data"];
    fd["datasource"]
        .as_str()
        .and_then(|d| d.split("__").next())
        .and_then(|n| n.parse().ok())
        .or_else(|| fd["datasource_id"].as_i64())
}

/// Superset's `/dashboard/<id>/charts` and `/dashboard/<id>/datasets` → the charts health needs.
pub fn chart_records(charts: &[Value], datasets: &[Value]) -> Vec<ChartRecord> {
    let mut out: Vec<ChartRecord> = charts
        .iter()
        .map(|c| {
            let chart_id = c["id"].as_i64().unwrap_or_default();
            let name = c["slice_name"].as_str().unwrap_or_default().to_string();
            let ds = chart_dataset_id(c)
                .and_then(|id| datasets.iter().find(|d| d["id"].as_i64() == Some(id)));
            let Some(ds) = ds else {
                return ChartRecord {
                    chart_id,
                    name,
                    dataset: String::new(),
                    reads: vec![],
                    unreadable: Some("the chart's dataset is not among the dashboard's".into()),
                    ..Default::default()
                };
            };
            let schema = ds["schema"].as_str();
            let table = ds["table_name"].as_str().unwrap_or_default();
            let dataset = match schema {
                Some(s) if !s.is_empty() => format!("{s}.{table}"),
                _ => table.to_string(),
            };
            let (reads, unreadable) = match ds["sql"].as_str().filter(|s| !s.trim().is_empty()) {
                Some(sql) => match sql_reads(sql, schema) {
                    Ok(r) => (r, None),
                    Err(e) => (vec![], Some(format!("its SQL does not parse: {e}"))),
                },
                None => (vec![dataset.to_ascii_lowercase()], None),
            };
            ChartRecord {
                chart_id,
                name,
                dataset,
                reads,
                unreadable,
                ..Default::default()
            }
        })
        .collect();
    out.sort_by_key(|c| c.chart_id);
    out
}

// ─────────────────────────────── the header (pure) ───────────────────────────────

pub fn when(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| at.to_string())
}

/// The header's markdown: the state, data as of, the first facts, a link to the chain — and what
/// the state is not.
pub fn header_text(d: &DashboardHealth, console_url: Option<&str>) -> String {
    let mut s = format!(
        "**Data pipeline: {}** · data as of {}",
        d.state.name().replace('_', " "),
        d.data_as_of.map(when).unwrap_or_else(|| "unknown".into())
    );
    if let Some(u) = console_url {
        s.push_str(&format!(" · [the chain in EKOS]({u})"));
    }
    s.push_str("\n\n");
    for e in d.evidence.iter().take(3) {
        s.push_str(&format!("- {e}\n"));
    }
    if d.evidence.len() > 3 {
        s.push_str(&format!("- and {} more\n", d.evidence.len() - 3));
    }
    s.push_str(
        "\n*The state of the data pipeline behind these numbers (EKOS Lakehouse), not a check \
         that they answer the business question.*",
    );
    s
}

/// The layout with the header as its first row (added, or its text replaced). `None` when the
/// dashboard has no layout of its own: Superset then places the charts itself, and a layout that
/// held only the header would hide them.
pub fn with_header(position_json: Option<&str>, text: &str) -> anyhow::Result<Option<String>> {
    let Some(raw) = position_json.filter(|p| !p.trim().is_empty()) else {
        return Ok(None);
    };
    let mut doc: Value = serde_json::from_str(raw)?;
    let Some(grid) = doc.get("GRID_ID").cloned() else {
        return Ok(None);
    };
    let mut children: Vec<Value> = grid["children"].as_array().cloned().unwrap_or_default();
    if !children.iter().any(|c| c == HEADER_ROW) {
        children.insert(0, json!(HEADER_ROW));
    }
    doc["GRID_ID"]["children"] = Value::Array(children);
    doc[HEADER_ROW] = json!({
        "type": "ROW", "id": HEADER_ROW, "children": [HEADER_MARKDOWN],
        "parents": ["ROOT_ID", "GRID_ID"], "meta": {"background": "BACKGROUND_TRANSPARENT"},
    });
    doc[HEADER_MARKDOWN] = json!({
        "type": "MARKDOWN", "id": HEADER_MARKDOWN, "children": [],
        "parents": ["ROOT_ID", "GRID_ID", HEADER_ROW],
        "meta": {"width": 12, "height": 20, "code": text},
    });
    Ok(Some(serde_json::to_string(&doc)?))
}

/// The certification to write: `Some((by, details))` sets it (`("", "")` removes EKOS's own),
/// `None` leaves it. A certification by someone other than EKOS is a person's and is left.
pub fn certification(d: &DashboardHealth, current_by: Option<&str>) -> Option<(String, String)> {
    let by = current_by.unwrap_or_default();
    let ours = by.is_empty() || by == CERTIFIER;
    match (d.state, ours) {
        (_, false) => None,
        (DashState::Trusted, _) => Some((
            CERTIFIER.into(),
            format!(
                "Every table and mart behind it is healthy; data as of {}.",
                d.data_as_of.map(when).unwrap_or_else(|| "unknown".into())
            ),
        )),
        (_, true) if by.is_empty() => None,
        _ => Some((String::new(), String::new())),
    }
}

// ─────────────────────────────── Superset ───────────────────────────────

pub struct Superset {
    base: String,
    http: reqwest::blocking::Client,
    bearer: String,
}

fn env(var: &str) -> anyhow::Result<String> {
    std::env::var(var).map_err(|_| {
        anyhow::anyhow!("{var} is named in [extensions.lakehouse.dashboards] but is not set")
    })
}

impl Superset {
    pub fn connect(s: &DashboardsSettings) -> anyhow::Result<Self> {
        let base = s
            .superset_url
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!("set superset-url in [extensions.lakehouse.dashboards]")
            })?
            .trim_end_matches('/')
            .to_string();
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let bearer = match (
            &s.superset_token_env,
            &s.superset_user,
            &s.superset_password_env,
        ) {
            (Some(var), _, _) => env(var)?,
            (None, Some(user), Some(var)) => {
                let v: Value = http
                    .post(format!("{base}/api/v1/security/login"))
                    .json(&json!({"username": user, "password": env(var)?, "provider": "db", "refresh": false}))
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| anyhow::anyhow!("signing in to Superset: {e}"))?
                    .json()?;
                v["access_token"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Superset's login returned no access token"))?
                    .to_string()
            }
            _ => anyhow::bail!(
                "name Superset's credentials: superset-token-env, or superset-user and superset-password-env"
            ),
        };
        Ok(Self { base, http, bearer })
    }

    /// A client with a token already in hand (a test's stub, or a caller that signed in itself).
    pub fn with_token(base: &str, token: &str) -> anyhow::Result<Self> {
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            bearer: token.to_string(),
        })
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.base);
        Ok(self
            .http
            .get(&url)
            .bearer_auth(&self.bearer)
            .query(query)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| anyhow::anyhow!("GET {path}: {e}"))?
            .json()?)
    }

    pub fn dashboards(&self) -> anyhow::Result<Vec<Value>> {
        let mut out = vec![];
        for page in 0.. {
            let q = format!("(page:{page},page_size:100)");
            let v = self.get("/api/v1/dashboard/", &[("q", &q)])?;
            let rows = v["result"].as_array().cloned().unwrap_or_default();
            let n = rows.len();
            out.extend(rows);
            if n < 100 || page >= 100 {
                break;
            }
        }
        Ok(out)
    }

    pub fn charts(&self, id: i64) -> anyhow::Result<Vec<Value>> {
        let v = self.get(&format!("/api/v1/dashboard/{id}/charts"), &[])?;
        Ok(v["result"].as_array().cloned().unwrap_or_default())
    }

    pub fn datasets(&self, id: i64) -> anyhow::Result<Vec<Value>> {
        let v = self.get(&format!("/api/v1/dashboard/{id}/datasets"), &[])?;
        Ok(v["result"].as_array().cloned().unwrap_or_default())
    }

    pub fn dashboard(&self, id: i64) -> anyhow::Result<Value> {
        Ok(self.get(&format!("/api/v1/dashboard/{id}"), &[])?["result"].clone())
    }

    /// A write needs Superset's CSRF token, bound to the session cookie it is issued with.
    pub fn put_dashboard(&self, id: i64, body: &Value) -> anyhow::Result<()> {
        self.write("PUT", &format!("/api/v1/dashboard/{id}"), Some(body))
            .map(|_| ())
    }

    /// One write (`POST`, `PUT`, `DELETE`) with the CSRF token and its session cookie. Returns the
    /// answer's JSON (`null` when there is none).
    pub fn write(&self, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
        let r = self
            .http
            .get(format!("{}/api/v1/security/csrf_token/", self.base))
            .bearer_auth(&self.bearer)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| anyhow::anyhow!("Superset's CSRF token: {e}"))?;
        let cookie = r
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let token: Value = r.json()?;
        let m = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut req = self
            .http
            .request(m, format!("{}{path}", self.base))
            .bearer_auth(&self.bearer)
            .header("X-CSRFToken", token["result"].as_str().unwrap_or_default())
            .header(reqwest::header::REFERER, format!("{}/", self.base));
        if let Some(b) = body {
            req = req.json(b);
        }
        if !cookie.is_empty() {
            req = req.header(reqwest::header::COOKIE, cookie);
        }
        let resp = req
            .send()
            .map_err(|e| anyhow::anyhow!("{method} {path}: {e}"))?;
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        anyhow::ensure!(
            status.is_success(),
            "{method} {path}: {status}: {}",
            text.chars().take(400).collect::<String>()
        );
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// One read.
    pub fn read(&self, path: &str) -> anyhow::Result<Value> {
        self.get(path, &[])
    }
}

/// What `sync` read: every selected dashboard, as a record (without `chain_known`). A dashboard
/// with a bundle (`with_bundles`, by Superset id) is taken from its bundle — the marts each chart
/// reads, its reconciliation, its drift — instead of parsing its datasets' SQL (RFC 0248 §2.4);
/// it is selected whatever its datasets are named.
pub fn read_superset(
    s: &DashboardsSettings,
    sup: &Superset,
    with_bundles: &[DashboardRecord],
) -> anyhow::Result<Vec<DashboardRecord>> {
    let mut out = vec![];
    for d in sup.dashboards()? {
        let Some(id) = d["id"].as_i64() else { continue };
        if let Some(b) = with_bundles.iter().find(|b| b.dashboard_id == id) {
            out.push(b.clone());
            continue;
        }
        let datasets = sup.datasets(id)?;
        if !selected(s, id, &datasets) {
            continue;
        }
        let charts = sup.charts(id)?;
        out.push(DashboardRecord {
            dashboard_id: id,
            title: d["dashboard_title"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            url: d["url"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("/superset/dashboard/{id}/")),
            charts: chart_records(&charts, &datasets),
            ..Default::default()
        });
    }
    Ok(out)
}

/// Record what Superset holds now: each dashboard read, with the chain tables the ledger knows,
/// and every previously recorded one no longer there as removed. Returns (recorded, removed).
pub fn sync(
    store: &dyn KnowledgeStore,
    read_now: &[DashboardRecord],
    late_after_minutes: i64,
    run: &str,
) -> anyhow::Result<(usize, usize)> {
    let tables = crate::monitor::health(store, late_after_minutes)?;
    let (before, marts) = read(store)?;
    for r in read_now {
        let mut r = r.clone();
        r.chain_known = dh::known_chain(&r, &tables, &marts);
        record(store, run, &r)?;
    }
    let mut removed = 0;
    for b in before.into_iter().filter(|b| !b.removed) {
        if !read_now.iter().any(|r| r.dashboard_id == b.dashboard_id) {
            record(store, run, &DashboardRecord { removed: true, ..b })?;
            removed += 1;
        }
    }
    Ok((read_now.len(), removed))
}

/// Write each dashboard's header and certification. `dry`: say what would change, write nothing.
pub fn publish(
    s: &DashboardsSettings,
    health: &[DashboardHealth],
    sup: &Superset,
    dry: bool,
) -> anyhow::Result<Vec<String>> {
    let mut said = vec![];
    for d in health {
        let current = sup.dashboard(d.dashboard_id)?;
        let mut body = serde_json::Map::new();
        let text = header_text(d, s.console_url.as_deref());
        match with_header(current["position_json"].as_str(), &text)? {
            Some(p) if current["position_json"].as_str() != Some(p.as_str()) => {
                body.insert("position_json".into(), json!(p));
            }
            Some(_) => {}
            None => said.push(format!(
                "{} ({}): no layout of its own, so no header (certification only)",
                d.title, d.dashboard_id
            )),
        }
        let by = current["certified_by"].as_str().unwrap_or_default();
        let details = current["certification_details"]
            .as_str()
            .unwrap_or_default();
        match certification(d, Some(by)) {
            Some((b, det)) if b != by || det != details => {
                body.insert("certified_by".into(), json!(b));
                body.insert("certification_details".into(), json!(det));
            }
            None if !by.is_empty() && by != CERTIFIER => said.push(format!(
                "{} ({}): certified by {by:?}, not EKOS — left as it is",
                d.title, d.dashboard_id
            )),
            _ => {}
        }
        if body.is_empty() {
            said.push(format!(
                "{} ({}): {} — unchanged",
                d.title,
                d.dashboard_id,
                d.state.name()
            ));
            continue;
        }
        let what: Vec<&str> = body.keys().map(String::as_str).collect();
        said.push(format!(
            "{} ({}): {} — {} {}",
            d.title,
            d.dashboard_id,
            d.state.name(),
            if dry { "would write" } else { "wrote" },
            what.join(", ")
        ));
        if !dry {
            sup.put_dashboard(d.dashboard_id, &Value::Object(body))?;
        }
    }
    Ok(said)
}

// ─────────────────────────────── surfaces ───────────────────────────────

pub fn render(h: &[DashboardHealth]) -> String {
    if h.is_empty() {
        return "no dashboards recorded: run `lakehouse dashboards sync`\n".into();
    }
    let mut out = format!(
        "{:<6} {:<40} {:<14} {}\n",
        "id", "dashboard", "state", "data as of"
    );
    for d in h {
        out.push_str(&format!(
            "{:<6} {:<40} {:<14} {}\n",
            d.dashboard_id,
            d.title,
            d.state.name(),
            d.data_as_of.map(when).unwrap_or_else(|| "unknown".into())
        ));
    }
    for d in h.iter().filter(|d| d.state != DashState::Trusted) {
        out.push_str(&format!(
            "\n{} ({}) is {}:\n",
            d.title,
            d.dashboard_id,
            d.state.name()
        ));
        for e in &d.evidence {
            out.push_str(&format!("    {e}\n"));
        }
    }
    let bundled = h.iter().all(|d| {
        d.charts
            .iter()
            .all(|c| c.not_evaluated.len() == dh::NOT_EVALUATED_WITH_BUNDLE.len())
    });
    out.push_str(&format!(
        "\nnot evaluated: {}\n",
        if bundled {
            dh::NOT_EVALUATED_WITH_BUNDLE.join("; ")
        } else {
            dh::NOT_EVALUATED.join("; ")
        }
    ));
    out
}

/// RFC 0258 §3.5's alert rule: a dashboard that leaves `trusted`, once per transition. The last
/// states are kept in `dashboard-alerts.json` beside RFC 0196's `alerts.json`.
pub fn alert(
    h: &[DashboardHealth],
    state_dir: &Path,
    webhook: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let path = state_dir.join("dashboard-alerts.json");
    let before: BTreeMap<String, DashState> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut sent = vec![];
    for (id, from, to) in dh::left_trusted(&before, h) {
        let d = h.iter().find(|d| d.dashboard_id == id).expect("from h");
        let text = format!(
            "EKOS Lakehouse: dashboard \"{}\" ({id}) is {}{}.{}",
            d.title,
            to.name(),
            from.map(|f| format!(", was {}", f.name()))
                .unwrap_or_default(),
            d.evidence
                .first()
                .map(|e| format!(" {e}."))
                .unwrap_or_default()
        );
        if let Some(url) = webhook {
            crate::monitor::post_alert(
                url,
                &json!({"text": text, "dashboard": id, "title": d.title, "state": to.name(),
                        "previous": from.map(DashState::name), "evidence": d.evidence,
                        "data_as_of": d.data_as_of}),
            )?;
        }
        sent.push(text);
    }
    let now: BTreeMap<String, DashState> = h
        .iter()
        .map(|d| (d.dashboard_id.to_string(), d.state))
        .collect();
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&path, serde_json::to_string_pretty(&now)? + "\n")?;
    Ok(sent)
}

/// The MCP tool `ekos_dashboard_health`'s answer.
pub fn mcp_answer(
    ledger: &dyn KnowledgeStore,
    args: &Value,
    late_after_minutes: i64,
) -> anyhow::Result<Value> {
    let want = args["dashboard"].as_str().map(str::to_ascii_lowercase);
    let only = args["only_untrusted"].as_bool().unwrap_or(false);
    let shown: Vec<DashboardHealth> = health(ledger, late_after_minutes)?
        .into_iter()
        .filter(|d| {
            want.as_ref().is_none_or(|w| {
                *w == d.dashboard_id.to_string() || *w == d.title.to_ascii_lowercase()
            })
        })
        .filter(|d| !only || d.state != DashState::Trusted)
        .collect();
    Ok(json!({
        "dashboards": shown,
        "not_evaluated": dh::NOT_EVALUATED,
        "note": "The state of the data pipeline behind each dashboard, not a check that its numbers answer the business question.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forks_datasets_are_recognised() {
        assert!(is_fork_dataset("ai_0a1b2c_1"));
        assert!(!is_fork_dataset("ai_0a1b2c_"));
        assert!(!is_fork_dataset("ai_zzzzzz_1"));
        assert!(!is_fork_dataset("orders"));
        let s = DashboardsSettings::default();
        let fork = [
            json!({"table_name": "ai_0a1b2c_1"}),
            json!({"table_name": "ai_0a1b2c_2"}),
        ];
        assert!(selected(&s, 1, &fork));
        let mixed = [
            json!({"table_name": "ai_0a1b2c_1"}),
            json!({"table_name": "orders"}),
        ];
        assert!(!selected(&s, 1, &mixed));
        assert!(!selected(&s, 1, &[]));
        let listed = DashboardsSettings {
            ids: vec![1],
            ..Default::default()
        };
        assert!(selected(&listed, 1, &mixed));
    }

    #[test]
    fn a_datasets_sql_names_its_tables_without_ctes() {
        let sql = "with m as (select * from iceberg.gold.sales) \
                   select m.month, sum(o.total) from m join silver_public.orders o on o.id = m.id \
                   join customers c on c.id = o.customer group by 1";
        assert_eq!(
            sql_reads(sql, Some("public")).unwrap(),
            [
                "iceberg.gold.sales",
                "silver_public.orders",
                "public.customers"
            ]
        );
        assert!(sql_reads("select from where", None).is_err());
    }

    #[test]
    fn charts_map_to_their_datasets() {
        let charts = [
            json!({"id": 2, "slice_name": "B", "form_data": {"datasource": "11__table"}}),
            json!({"id": 1, "slice_name": "A", "form_data": {"datasource": "10__table"}}),
            json!({"id": 3, "slice_name": "C", "form_data": {"datasource": "99__table"}}),
        ];
        let datasets = [
            json!({"id": 10, "schema": "public", "table_name": "ai_abcdef_1", "sql": "select * from gold.sales"}),
            json!({"id": 11, "schema": "gold", "table_name": "sales", "sql": null}),
        ];
        let r = chart_records(&charts, &datasets);
        assert_eq!(r[0].reads, ["gold.sales"]);
        assert_eq!(r[0].dataset, "public.ai_abcdef_1");
        assert_eq!(
            r[1].reads,
            ["gold.sales"],
            "a physical dataset reads its table"
        );
        assert!(r[2].unreadable.is_some());
    }

    fn health_of(state: DashState) -> DashboardHealth {
        DashboardHealth {
            dashboard_id: 7,
            title: "Sales".into(),
            url: "/superset/dashboard/7/".into(),
            state,
            evidence: if state == DashState::Trusted {
                vec![]
            } else {
                vec!["chart 1: bronze public.orders is failing: run r1 failed".into()]
            },
            data_as_of: Some(1_762_840_800),
            charts: vec![],
        }
    }

    #[test]
    fn the_header_is_one_row_added_once_and_replaced() {
        let layout = r#"{"ROOT_ID":{"type":"ROOT","id":"ROOT_ID","children":["GRID_ID"]},
            "GRID_ID":{"type":"GRID","id":"GRID_ID","children":["ROW-0"],"parents":["ROOT_ID"]},
            "ROW-0":{"type":"ROW","id":"ROW-0","children":[]}}"#;
        let bad = header_text(&health_of(DashState::Failing), Some("https://c/x"));
        assert!(bad.contains("**Data pipeline: failing**"));
        assert!(bad.contains("public.orders is failing"));
        assert!(bad.contains("data as of 2025-11-11 06:00 UTC"));
        assert!(bad.contains("[the chain in EKOS](https://c/x)"));
        assert!(bad.contains("not a check that they answer the business question"));
        let once = with_header(Some(layout), &bad).unwrap().unwrap();
        let twice = with_header(
            Some(&once),
            &header_text(&health_of(DashState::Trusted), None),
        )
        .unwrap()
        .unwrap();
        let v: Value = serde_json::from_str(&twice).unwrap();
        assert_eq!(v["GRID_ID"]["children"], json!([HEADER_ROW, "ROW-0"]));
        assert!(
            v[HEADER_MARKDOWN]["meta"]["code"]
                .as_str()
                .unwrap()
                .contains("Data pipeline: trusted")
        );
        assert_eq!(
            with_header(None, "x").unwrap(),
            None,
            "no layout of its own"
        );
    }

    #[test]
    fn certification_follows_the_state_and_never_removes_a_persons() {
        let ok = health_of(DashState::Trusted);
        let bad = health_of(DashState::Late);
        assert_eq!(certification(&ok, None).unwrap().0, CERTIFIER);
        assert_eq!(
            certification(&bad, Some(CERTIFIER)),
            Some((String::new(), String::new())),
            "EKOS's mark is removed"
        );
        assert_eq!(certification(&bad, None), None, "nothing to remove");
        assert_eq!(certification(&bad, Some("Finance team")), None);
        assert_eq!(certification(&ok, Some("Finance team")), None);
    }
}
