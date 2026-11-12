//! EKOS RFC 0258 phases 1, 2 and 4 (and RFC 0248 §2.4's bundle): a published dashboard is a
//! Superset dashboard plus `dashboards/<key>.yaml` in the pipeline repository. Every publication
//! is a commit of the bundle (a **version**); a change by someone who is not the author is a
//! **copy** on a branch `ekos/dash/<key>/<id>`, published to Superset separately; a merge is the
//! original author's (or a reviewer's) acceptance, at the level of terms. Before any change the
//! published dashboard is read back from Superset: a hand edit is **drift**, and a change request
//! on a drifted dashboard is refused.
//!
//! The pure part (terms, SQL, classes, diff, merge) is `ekos_lakehouse_gold::dashboard`; this
//! module does the I/O: git, Superset's REST API and the engine, each behind a seam so the flow
//! is tested without them.

use crate::dashboards::{DashboardsSettings, Superset};
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::typemap::ident;
use ekos_lakehouse_gold::dashboard::{
    self as dash, Bundle, BundleChart, ChangeClass, ChartRequest, Classified, Delta, Mart,
    MartDigest, MartSpecView, MeasureCheck, Origin, Presentation, Reconciled, Side, SupersetChart,
    SupersetDashboard, Target,
};
use ekos_lakehouse_gold::{self as gold, MartSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

// ─────────────────────────────── seams ───────────────────────────────

/// Superset's REST API, as far as publishing needs it.
pub trait SupersetApi {
    fn read(&self, path: &str) -> anyhow::Result<Value>;
    fn write(&self, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<Value>;
}

impl SupersetApi for Superset {
    fn read(&self, path: &str) -> anyhow::Result<Value> {
        Superset::read(self, path)
    }
    fn write(&self, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
        Superset::write(self, method, path, body)
    }
}

/// The query engine: reads, and — only for a sandbox build of a mart — one write.
pub trait Engine {
    fn rows(&self, sql: &str) -> anyhow::Result<Vec<Vec<String>>>;
    fn build(&self, _sql: &str) -> anyhow::Result<()> {
        anyhow::bail!("this engine reads only")
    }
}

/// Trino: reads through the read-only reader; a build through a client.
pub struct Trino {
    pub conn: ekos_lakehouse_trino::Connection,
}

impl Engine for Trino {
    fn rows(&self, sql: &str) -> anyhow::Result<Vec<Vec<String>>> {
        Ok(ekos_lakehouse_trino::TrinoReader::new(self.conn.clone())?.read(sql)?)
    }
    fn build(&self, sql: &str) -> anyhow::Result<()> {
        ekos_lakehouse_trino::Client::new(self.conn.clone())?.execute(sql)?;
        Ok(())
    }
}

/// Everything one command works with.
pub struct Ctx<'a> {
    pub repo: PathBuf,
    pub plan: Plan,
    pub target: Target,
    pub sup: &'a dyn SupersetApi,
    pub engine: &'a dyn Engine,
    pub settings: DashboardsSettings,
    /// `.lakehouse/` (requests, consents).
    pub state_dir: PathBuf,
    pub now: String,
}

impl Ctx<'_> {
    /// The marts the pipeline repository describes on its checked-out line.
    pub fn marts(&self) -> Vec<Mart> {
        dash::marts_of(&crate::ask_cmd::existing_marts(&self.repo).0, &self.plan)
    }

    fn database_id(&self) -> anyhow::Result<i64> {
        self.settings.superset_database_id.ok_or_else(|| {
            anyhow::anyhow!("set superset-database-id in [extensions.lakehouse.dashboards]")
        })
    }

    fn schema(&self) -> String {
        self.settings
            .superset_schema
            .clone()
            .unwrap_or_else(|| self.target.gold_schema.clone())
    }
}

// ─────────────────────────────── git ───────────────────────────────

fn git_env(repo: &Path, args: &[&str], env: &[(&str, &str)]) -> anyhow::Result<String> {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo).args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c
        .output()
        .map_err(|e| anyhow::anyhow!("running git (is it installed and on PATH?): {e}"))?;
    anyhow::ensure!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    git_env(repo, args, &[])
}

fn email(who: &str) -> String {
    let local: String = who
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '.' })
        .collect();
    format!("{local}@ekos.local")
}

/// Author and committer: the person EKOS acts for.
fn as_person(who: &str) -> Vec<(&'static str, String)> {
    vec![
        ("GIT_AUTHOR_NAME", who.to_string()),
        ("GIT_AUTHOR_EMAIL", email(who)),
        ("GIT_COMMITTER_NAME", who.to_string()),
        ("GIT_COMMITTER_EMAIL", email(who)),
    ]
}

fn git_as(repo: &Path, who: &str, args: &[&str]) -> anyhow::Result<String> {
    let env = as_person(who);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    git_env(repo, args, &env)
}

pub fn branch_name(key: &str, id: &str) -> String {
    format!("ekos/dash/{key}/{id}")
}

/// The bundle on the checked-out line (the dashboard as published now).
pub fn bundle_now(repo: &Path, key: &str) -> anyhow::Result<Bundle> {
    let path = repo.join(Bundle::path(key));
    let text = std::fs::read_to_string(&path)
        .map_err(|_| anyhow::anyhow!("no dashboard {key}: {} does not exist", path.display()))?;
    Bundle::from_yaml(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// The bundle at a commit or branch.
pub fn bundle_at(repo: &Path, rev: &str, key: &str) -> anyhow::Result<Bundle> {
    let text = crate::marts::file_at(repo, rev, &Bundle::path(key))
        .ok_or_else(|| anyhow::anyhow!("{rev} holds no dashboard {key}"))?;
    Bundle::from_yaml(&text).map_err(|e| anyhow::anyhow!("{rev}:{}: {e}", Bundle::path(key)))
}

/// The key of the only dashboard a branch changes.
pub fn branch_key(branch: &str) -> anyhow::Result<String> {
    let rest = branch.strip_prefix("ekos/dash/").ok_or_else(|| {
        anyhow::anyhow!("{branch} is not a dashboard branch (ekos/dash/<key>/<id>)")
    })?;
    let (key, _) = rest
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("{branch} is not a dashboard branch"))?;
    Ok(key.to_string())
}

/// Every dashboard key on the checked-out line.
pub fn keys(repo: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(repo.join(dash::BUNDLE_DIR))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".yaml")
                .map(str::to_string)
        })
        .filter(|k| dash::valid_key(k))
        .collect();
    out.sort();
    out
}

/// Commit the bundle on the checked-out line (the dashboard's main line). Only that file is
/// committed, whatever else the working tree holds.
fn commit_main(repo: &Path, b: &Bundle, who: &str, message: &str) -> anyhow::Result<String> {
    let rel = Bundle::path(&b.dashboard);
    let path = repo.join(&rel);
    std::fs::create_dir_all(path.parent().expect("dashboards/"))?;
    std::fs::write(&path, b.to_yaml())?;
    git(repo, &["add", "--", &rel])?;
    git_as(repo, who, &["commit", "-q", "-m", message, "--", &rel])?;
    git(repo, &["rev-parse", "HEAD"])
}

/// Commit files onto a branch without checking it out: a temporary index, `commit-tree`, and the
/// branch moved to it. `parent`: the branch's head, or the commit a new branch starts from.
fn commit_files_to_branch(
    repo: &Path,
    branch: &str,
    parent: &str,
    files: &[(String, String)],
    who: &str,
    message: &str,
) -> anyhow::Result<String> {
    let git_dir = PathBuf::from(git(repo, &["rev-parse", "--absolute-git-dir"])?);
    let dir = git_dir.join(format!("ekos-dash-{}", short_id()));
    std::fs::create_dir_all(&dir)?;
    let result = commit_with_index(repo, branch, parent, files, who, message, &dir);
    let _ = std::fs::remove_file(dir.join("index"));
    let _ = std::fs::remove_file(dir.join("blob"));
    let _ = std::fs::remove_dir(&dir);
    result
}

fn commit_with_index(
    repo: &Path,
    branch: &str,
    parent: &str,
    files: &[(String, String)],
    who: &str,
    message: &str,
    dir: &Path,
) -> anyhow::Result<String> {
    let index = dir.join("index");
    let index = index.to_string_lossy().to_string();
    let env_idx = [("GIT_INDEX_FILE", index.as_str())];
    git_env(repo, &["read-tree", parent], &env_idx)?;
    for (rel, text) in files {
        let f = dir.join("blob");
        std::fs::write(&f, text)?;
        let blob = git(repo, &["hash-object", "-w", &f.to_string_lossy()])?;
        git_env(
            repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{blob},{rel}"),
            ],
            &env_idx,
        )?;
    }
    let tree = git_env(repo, &["write-tree"], &env_idx)?;
    let commit = git_as(
        repo,
        who,
        &["commit-tree", &tree, "-p", parent, "-m", message],
    )?;
    git(
        repo,
        &["update-ref", &format!("refs/heads/{branch}"), &commit],
    )?;
    Ok(commit)
}

#[derive(Debug, Clone, Serialize)]
pub struct Version {
    pub commit: String,
    pub author: String,
    pub at: String,
    pub message: String,
}

/// Every version of a dashboard on its main line, newest first.
pub fn versions(repo: &Path, key: &str) -> anyhow::Result<Vec<Version>> {
    let out = git(
        repo,
        // The main line only: a merged branch's own commits are not versions of the original.
        &[
            "log",
            "--first-parent",
            "--format=%H%x1f%an%x1f%aI%x1f%s",
            "--",
            &Bundle::path(key),
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 4).then(|| Version {
                commit: p[0].into(),
                author: p[1].into(),
                at: p[2].into(),
                message: p[3].into(),
            })
        })
        .collect())
}

/// The open branches of a dashboard (or of all), with their heads.
pub fn branches(repo: &Path, key: Option<&str>) -> anyhow::Result<Vec<(String, String)>> {
    let prefix = match key {
        Some(k) => format!("refs/heads/ekos/dash/{k}/"),
        None => "refs/heads/ekos/dash/".into(),
    };
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname:short) %(objectname)",
            &prefix,
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect())
}

/// Keep a branch's commits under `refs/ekos/dash-<what>/…` and remove the branch.
fn retire_branch(repo: &Path, branch: &str, what: &str) -> anyhow::Result<()> {
    let head = git(repo, &["rev-parse", &format!("refs/heads/{branch}")])?;
    let keep = format!(
        "refs/ekos/dash-{what}/{}",
        branch.trim_start_matches("ekos/dash/")
    );
    git(repo, &["update-ref", &keep, &head])?;
    git(repo, &["update-ref", "-d", &format!("refs/heads/{branch}")])?;
    Ok(())
}

// ─────────────────────────────── Superset ───────────────────────────────

/// The title a copy is published under (RFC 0258 §3.3: titled with its origin and author). The
/// bundle keeps the original title as a term; this is only what Superset shows.
pub fn superset_title(b: &Bundle) -> String {
    match &b.origin {
        Some(o) => format!("{} (copy by {}, from {})", b.title, b.author, o.dashboard),
        None => b.title.clone(),
    }
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

fn id_of(v: &Value) -> anyhow::Result<i64> {
    v["id"]
        .as_i64()
        .or_else(|| v["result"]["id"].as_i64())
        .ok_or_else(|| anyhow::anyhow!("Superset returned no id: {v}"))
}

fn create_chart(
    cx: &Ctx<'_>,
    c: &mut BundleChart,
    dashboard_id: i64,
    key: &str,
) -> anyhow::Result<()> {
    let name = format!("ekos_{}_{}_{}", key.replace('-', "_"), c.key, short_id());
    let ds = cx.sup.write(
        "POST",
        "/api/v1/dataset/",
        Some(
            &json!({"database": cx.database_id()?, "schema": cx.schema(),
                     "table_name": name, "sql": c.sql}),
        ),
    )?;
    let dataset_id = id_of(&ds)?;
    let params = dash::form_data(c, dataset_id);
    let ch = cx.sup.write(
        "POST",
        "/api/v1/chart/",
        Some(
            &json!({"slice_name": c.presentation.title, "viz_type": params["viz_type"],
                     "params": params.to_string(), "datasource_id": dataset_id,
                     "datasource_type": "table", "dashboards": [dashboard_id]}),
        ),
    )?;
    c.superset = Some(SupersetChart {
        chart_id: id_of(&ch)?,
        dataset_id,
        dataset: name,
    });
    c.params = Some(params);
    Ok(())
}

fn layout_of(b: &Bundle) -> Vec<(i64, String)> {
    let mut charts: Vec<&BundleChart> = b.charts.iter().collect();
    charts.sort_by_key(|c| (c.presentation.order, c.key.clone()));
    charts
        .into_iter()
        .filter_map(|c| {
            c.superset
                .as_ref()
                .map(|s| (s.chart_id, c.presentation.title.clone()))
        })
        .collect()
}

/// The layout, keeping the health header (RFC 0258 phase 3) when the dashboard has one.
fn position(cx: &Ctx<'_>, b: &Bundle, dashboard_id: i64) -> anyhow::Result<String> {
    let fresh = dash::layout(&superset_title(b), &layout_of(b));
    let current = cx.sup.read(&format!("/api/v1/dashboard/{dashboard_id}"))?;
    let header = current["result"]["position_json"]
        .as_str()
        .and_then(|p| serde_json::from_str::<Value>(p).ok())
        .and_then(|p| {
            p.get(crate::dashboards::HEADER_MARKDOWN)
                .and_then(|m| m["meta"]["code"].as_str().map(str::to_string))
        });
    Ok(match header {
        Some(text) => crate::dashboards::with_header(Some(&fresh), &text)?.unwrap_or(fresh),
        None => fresh,
    })
}

/// A new Superset dashboard for the bundle: one dataset and chart per chart, then the layout.
pub fn create_dashboard(cx: &Ctx<'_>, b: &mut Bundle) -> anyhow::Result<()> {
    let d = cx.sup.write(
        "POST",
        "/api/v1/dashboard/",
        Some(&json!({"dashboard_title": superset_title(b), "published": true})),
    )?;
    let id = id_of(&d)?;
    let key = b.dashboard.clone();
    for c in &mut b.charts {
        create_chart(cx, c, id, &key)?;
    }
    b.superset = Some(SupersetDashboard {
        dashboard_id: id,
        url: format!("/superset/dashboard/{id}/"),
    });
    let pos = position(cx, b, id)?;
    cx.sup.write(
        "PUT",
        &format!("/api/v1/dashboard/{id}"),
        Some(&json!({"position_json": pos})),
    )?;
    Ok(())
}

/// Change the published dashboard from `before` to `after` in place: changed SQL and form data,
/// charts added and removed, the title and layout.
pub fn update_dashboard(cx: &Ctx<'_>, before: &Bundle, after: &mut Bundle) -> anyhow::Result<()> {
    let id = before
        .superset
        .as_ref()
        .map(|s| s.dashboard_id)
        .ok_or_else(|| anyhow::anyhow!("{} was never published", before.dashboard))?;
    let key = after.dashboard.clone();
    for c in &mut after.charts {
        let old = before.charts.iter().find(|o| o.key == c.key);
        let Some(s) = old.and_then(|o| o.superset.clone()) else {
            create_chart(cx, c, id, &key)?;
            continue;
        };
        let old = old.expect("checked");
        let mut s = s;
        if old.sql != c.sql && !s.dataset.starts_with("ekos_") {
            // An adopted chart's dataset was built by hand and may serve other charts: EKOS
            // gives the chart a dataset of its own rather than rewriting that one.
            let name = format!("ekos_{}_{}_{}", key.replace('-', "_"), c.key, short_id());
            let ds = cx.sup.write(
                "POST",
                "/api/v1/dataset/",
                Some(
                    &json!({"database": cx.database_id()?, "schema": cx.schema(),
                             "table_name": name, "sql": c.sql}),
                ),
            )?;
            s = SupersetChart {
                chart_id: s.chart_id,
                dataset_id: id_of(&ds)?,
                dataset: name,
            };
            let params = dash::form_data(c, s.dataset_id);
            cx.sup.write(
                "PUT",
                &format!("/api/v1/chart/{}", s.chart_id),
                Some(
                    &json!({"slice_name": c.presentation.title, "viz_type": params["viz_type"],
                             "params": params.to_string(), "datasource_id": s.dataset_id,
                             "datasource_type": "table"}),
                ),
            )?;
            c.params = Some(params);
            c.superset = Some(s);
            continue;
        }
        if old.sql != c.sql {
            cx.sup.write(
                "PUT",
                &format!("/api/v1/dataset/{}?override_columns=true", s.dataset_id),
                Some(&json!({"sql": c.sql})),
            )?;
            cx.sup.write(
                "PUT",
                &format!("/api/v1/dataset/{}/refresh", s.dataset_id),
                None,
            )?;
        }
        let params = dash::form_data(c, s.dataset_id);
        if old.params.as_ref() != Some(&params) || old.presentation.title != c.presentation.title {
            cx.sup.write(
                "PUT",
                &format!("/api/v1/chart/{}", s.chart_id),
                Some(
                    &json!({"slice_name": c.presentation.title, "viz_type": params["viz_type"],
                             "params": params.to_string()}),
                ),
            )?;
        }
        c.params = Some(params);
        c.superset = Some(s);
    }
    for gone in before
        .charts
        .iter()
        .filter(|o| !after.charts.iter().any(|c| c.key == o.key))
    {
        if let Some(s) = &gone.superset {
            cx.sup
                .write("DELETE", &format!("/api/v1/chart/{}", s.chart_id), None)?;
            cx.sup
                .write("DELETE", &format!("/api/v1/dataset/{}", s.dataset_id), None)?;
        }
    }
    after.superset = before.superset.clone();
    let pos = position(cx, after, id)?;
    cx.sup.write(
        "PUT",
        &format!("/api/v1/dashboard/{id}"),
        Some(&json!({"dashboard_title": superset_title(after), "position_json": pos})),
    )?;
    Ok(())
}

/// Remove a published dashboard with its charts and datasets.
pub fn delete_dashboard(cx: &Ctx<'_>, b: &Bundle) -> anyhow::Result<()> {
    for c in &b.charts {
        if let Some(s) = &c.superset {
            cx.sup
                .write("DELETE", &format!("/api/v1/chart/{}", s.chart_id), None)?;
            cx.sup
                .write("DELETE", &format!("/api/v1/dataset/{}", s.dataset_id), None)?;
        }
    }
    if let Some(s) = &b.superset {
        cx.sup.write(
            "DELETE",
            &format!("/api/v1/dashboard/{}", s.dashboard_id),
            None,
        )?;
    }
    Ok(())
}

// ─────────────────────────────── drift (phase 4) ───────────────────────────────

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// RFC 0258 §3.4: what in Superset differs from the bundle — a chart edited by hand, a dataset's
/// SQL changed, a chart added or removed, the title. The health header is EKOS's own and is not
/// judged. Empty: no drift.
pub fn drift(sup: &dyn SupersetApi, b: &Bundle) -> anyhow::Result<Vec<String>> {
    let Some(d) = &b.superset else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    let dashboard = sup.read(&format!("/api/v1/dashboard/{}", d.dashboard_id))?["result"].clone();
    if dashboard["dashboard_title"].as_str() != Some(superset_title(b).as_str()) {
        out.push(format!(
            "the dashboard's title is {:?}, not {:?}",
            dashboard["dashboard_title"].as_str().unwrap_or_default(),
            superset_title(b)
        ));
    }
    let on: BTreeSet<i64> =
        sup.read(&format!("/api/v1/dashboard/{}/charts", d.dashboard_id))?["result"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["id"].as_i64())
            .collect();
    let ours: BTreeSet<i64> = b
        .charts
        .iter()
        .filter_map(|c| c.superset.as_ref().map(|s| s.chart_id))
        .chain(b.unadopted.iter().map(|u| u.chart_id))
        .collect();
    for extra in on.difference(&ours) {
        out.push(format!(
            "chart {extra} is on the dashboard but not in its bundle"
        ));
    }
    for c in &b.charts {
        let Some(s) = &c.superset else { continue };
        if !on.contains(&s.chart_id) {
            out.push(format!(
                "{}: chart {} is no longer on the dashboard",
                c.presentation.title, s.chart_id
            ));
            continue;
        }
        out.extend(chart_drift(sup, c, s)?);
    }
    Ok(out)
}

fn chart_drift(
    sup: &dyn SupersetApi,
    c: &BundleChart,
    s: &SupersetChart,
) -> anyhow::Result<Vec<String>> {
    let mut out = vec![];
    let t = &c.presentation.title;
    let chart = sup.read(&format!("/api/v1/chart/{}", s.chart_id))?["result"].clone();
    if chart["slice_name"].as_str() != Some(t.as_str()) {
        out.push(format!(
            "{t}: the chart's name is {:?}",
            chart["slice_name"].as_str().unwrap_or_default()
        ));
    }
    let now: Value = chart["params"]
        .as_str()
        .and_then(|p| serde_json::from_str(p).ok())
        .unwrap_or(Value::Null);
    if let Some(Value::Object(published)) = &c.params {
        for (k, v) in published {
            if now.get(k) != Some(v) {
                out.push(format!(
                    "{t}: `{k}` is {} in Superset, {} as published",
                    now.get(k)
                        .map(Value::to_string)
                        .unwrap_or_else(|| "unset".into()),
                    v
                ));
            }
        }
    }
    let ds = sup.read(&format!("/api/v1/dataset/{}", s.dataset_id))?["result"].clone();
    if squash(ds["sql"].as_str().unwrap_or_default()) != squash(&c.sql) {
        out.push(format!("{t}: the dataset's SQL was changed in Superset"));
    }
    Ok(out)
}

// ─────────────────────────────── building charts ───────────────────────────────

/// RFC 0248 §2.4: the chart against silver. A chart with no sum or count has nothing to compare
/// and says so.
pub fn reconcile(
    cx: &Ctx<'_>,
    req: &ChartRequest,
    served: &dash::Served,
) -> anyhow::Result<Reconciled> {
    let qs = dash::reconcile_queries(req, served, &cx.plan, &cx.target.catalog)
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut measures = vec![];
    for (m, chart, silver) in &qs {
        let first = |sql: &str| -> anyhow::Result<String> {
            Ok(cx
                .engine
                .rows(sql)?
                .into_iter()
                .next()
                .and_then(|r| r.into_iter().next())
                .unwrap_or_default())
        };
        let (a, b) = (first(chart)?, first(silver)?);
        measures.push(MeasureCheck {
            measure: m.clone(),
            equal: dash::totals_equal(&a, &b),
            chart_total: a,
            silver_total: b,
        });
    }
    Ok(Reconciled {
        at: cx.now.clone(),
        ok: measures.iter().all(|m| m.equal),
        note: qs
            .is_empty()
            .then(|| "no sum or count among its measures: nothing to reconcile".into()),
        measures,
    })
}

fn chart_from(
    key: &str,
    presentation: Presentation,
    request: ChartRequest,
    served: &dash::Served,
    reconciled: Reconciled,
) -> BundleChart {
    BundleChart {
        key: key.to_string(),
        presentation,
        request: dash::with_inherited(&request, served),
        mart: served.mart.clone(),
        marts: vec![MartDigest {
            mart: served.mart.clone(),
            digest: served.digest.clone(),
        }],
        terms: served.terms.clone(),
        sql: served.sql.clone(),
        reconciled: Some(reconciled),
        superset: None,
        params: None,
    }
}

/// A chart compiled over the marts and reconciled, or the reason it cannot be published.
fn build_chart(
    cx: &Ctx<'_>,
    marts: &[Mart],
    key: &str,
    presentation: Presentation,
    request: ChartRequest,
    prefer: Option<&str>,
    inherit: bool,
) -> anyhow::Result<BundleChart> {
    let served = dash::compile_chart(&request, &cx.plan, marts, &cx.target, prefer, inherit)
        .map_err(|u| anyhow::anyhow!("chart {key}: {}", unserved_text(&u)))?;
    let req = dash::with_inherited(&request, &served);
    presentation
        .check(&req)
        .map_err(|e| anyhow::anyhow!("chart {key}: {e}"))?;
    let rec = reconcile(cx, &req, &served)?;
    anyhow::ensure!(
        rec.ok,
        "chart {key} does not reconcile with silver ({}); nothing is published",
        rec.measures
            .iter()
            .filter(|m| !m.equal)
            .map(|m| format!(
                "{}: chart {} / silver {}",
                m.measure, m.chart_total, m.silver_total
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    Ok(chart_from(key, presentation, req, &served, rec))
}

fn unserved_text(u: &dash::Unserved) -> String {
    match u {
        dash::Unserved::Invalid(e) => e.clone(),
        dash::Unserved::Questions(qs) => qs
            .iter()
            .map(|q| q.text.clone())
            .collect::<Vec<_>>()
            .join("; "),
        dash::Unserved::NoMart(why) => format!(
            "no mart serves it ({})",
            why.iter()
                .map(|(m, w)| format!("{m}: {w}"))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

// ─────────────────────────────── publishing a new dashboard ───────────────────────────────

/// One chart of a new dashboard.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewChart {
    pub presentation: Presentation,
    pub request: ChartRequest,
}

/// `lakehouse dashboards publish <file>`: a dashboard from terms.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewDashboard {
    #[serde(default)]
    pub key: Option<String>,
    pub title: String,
    #[serde(default)]
    pub request_text: String,
    pub charts: Vec<NewChart>,
}

fn slug(title: &str) -> String {
    let s: String = title
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let s: String = s.chars().take(40).collect();
    format!("{}-{}", s.trim_end_matches('-'), short_id())
}

/// Compile, reconcile and publish a new dashboard, and commit its bundle as version 1.
pub fn publish_new(cx: &Ctx<'_>, d: NewDashboard, author: &str) -> anyhow::Result<Bundle> {
    anyhow::ensure!(!d.charts.is_empty(), "a dashboard needs a chart");
    let key = d.key.clone().unwrap_or_else(|| slug(&d.title));
    anyhow::ensure!(dash::valid_key(&key), "{key:?} is not a dashboard key");
    anyhow::ensure!(
        !cx.repo.join(Bundle::path(&key)).exists(),
        "dashboard {key} exists: change it instead"
    );
    let marts = cx.marts();
    let mut charts = vec![];
    for (i, c) in d.charts.into_iter().enumerate() {
        let k = format!("c{}", i + 1);
        charts.push(build_chart(
            cx,
            &marts,
            &k,
            c.presentation,
            c.request,
            None,
            true,
        )?);
    }
    let mut b = Bundle {
        format: dash::BUNDLE_FORMAT,
        dashboard: key,
        title: d.title,
        author: author.to_string(),
        request_text: d.request_text,
        superset: None,
        origin: None,
        charts,
        unadopted: vec![],
    };
    create_dashboard(cx, &mut b)?;
    let msg = format!("lakehouse: publish dashboard {} ({})", b.dashboard, b.title);
    commit_main(&cx.repo, &b, author, &msg)?;
    Ok(b)
}

// ─────────────────────────────── a change ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The author's own dashboard in place; anyone else's as a copy.
    Auto,
    /// A branch even for the author ("try something without touching what others read").
    Branch,
}

/// One change request.
pub struct ChangeRequest<'a> {
    /// The dashboard, or a branch of it (`ekos/dash/<key>/<id>`) its author changes further.
    pub target: &'a str,
    pub delta: Delta,
    pub user: &'a str,
    pub text: &'a str,
    pub mode: Mode,
    pub preview: bool,
    /// RFC 0248 §2.3's confirmation of a modifying mart change ("Continue? Yes").
    pub confirm: bool,
}

fn rows_preview(cx: &Ctx<'_>, sql: &str) -> Value {
    match cx.engine.rows(&format!(
        "select * from (\n{sql}\n) as preview order by 1 limit 20"
    )) {
        Ok(r) => json!(r),
        Err(e) => json!({"error": e.to_string()}),
    }
}

/// The old and the new chart on the same data, the SQL diff and the term diff.
fn side_by_side(cx: &Ctx<'_>, old: &Bundle, new: &Bundle) -> Value {
    let mut charts = vec![];
    for c in &new.charts {
        let before = old.charts.iter().find(|o| o.key == c.key);
        if before.is_some_and(|b| b.sql == c.sql && b.presentation == c.presentation) {
            continue;
        }
        charts.push(json!({
            "chart": c.key,
            "title": c.presentation.title,
            "before": before.map(|b| json!({"viz": b.presentation.viz, "rows": rows_preview(cx, &b.sql)})),
            "after": {"viz": c.presentation.viz, "rows": rows_preview(cx, &c.sql)},
            "sql_diff": dash::sql_diff(before.map(|b| b.sql.as_str()).unwrap_or(""), &c.sql),
            "reconciled": c.reconciled,
        }));
    }
    json!({"terms": dash::term_diff(old, new), "charts": charts})
}

/// Where a change lands.
enum Landing {
    Main,
    NewBranch(String),
    Branch(String),
}

fn landing(
    b: &Bundle,
    req: &ChangeRequest<'_>,
    on_branch: Option<&str>,
) -> anyhow::Result<Landing> {
    if let Some(br) = on_branch {
        anyhow::ensure!(
            b.author == req.user,
            "{br} is {}'s branch: only its author changes it",
            b.author
        );
        return Ok(Landing::Branch(br.to_string()));
    }
    if b.author != req.user || req.mode == Mode::Branch {
        return Ok(Landing::NewBranch(branch_name(&b.dashboard, &short_id())));
    }
    Ok(Landing::Main)
}

/// RFC 0258 §3.2–§3.3: apply a change request. Returns what happened (or why nothing did).
pub fn change(cx: &Ctx<'_>, req: ChangeRequest<'_>) -> anyhow::Result<Value> {
    let on_branch = req.target.starts_with("ekos/dash/").then_some(req.target);
    let (key, b) = match on_branch {
        Some(br) => {
            let key = branch_key(br)?;
            let b = bundle_at(&cx.repo, br, &key)?;
            (key, b)
        }
        None => (req.target.to_string(), bundle_now(&cx.repo, req.target)?),
    };
    let drifted = drift(cx.sup, &b)?;
    if !drifted.is_empty() {
        return Ok(json!({
            "status": "refused",
            "reason": "the dashboard was edited in Superset since it was published (drifted): \
                       adopt the edit or republish before changing it",
            "drift": drifted,
        }));
    }
    let mut marts = cx.marts();
    let mut classified: Vec<Classified> = vec![];
    for d in &req.delta.charts {
        let Some(c) = b.chart(&d.chart) else {
            anyhow::bail!("{key} has no chart {:?}", d.chart);
        };
        let (p, r) = match dash::apply_chart_delta(c, d) {
            Ok(x) => x,
            Err(e) => {
                return Ok(json!({"status": "refused", "reason": format!("chart {}: {e}", c.key)}));
            }
        };
        classified.push(dash::classify(c, p, r, &cx.plan, &marts, &cx.target));
    }
    let blocked: Vec<&Classified> = classified
        .iter()
        .filter(|c| {
            matches!(
                c.class,
                ChangeClass::Impossible | ChangeClass::NeedsClarification
            )
        })
        .collect();
    if !blocked.is_empty() {
        return Ok(json!({
            "status": blocked[0].class.name(),
            "reason": "a term does not resolve: nothing is changed",
            "charts": classified,
        }));
    }
    let place = landing(&b, &req, on_branch)?;
    let branch_id = match &place {
        Landing::NewBranch(br) | Landing::Branch(br) => br.rsplit('/').next().map(str::to_string),
        Landing::Main => None,
    };
    // Phase 2: a chart that needs a mart goes through RFC 0248 §2.3 first.
    let mut mart_steps = vec![];
    for c in classified
        .iter()
        .filter(|c| matches!(c.class, ChangeClass::MartAddition | ChangeClass::MartChange))
    {
        match mart_step(cx, c, branch_id.as_deref(), &req, &key)? {
            MartOutcome::Built(m, report) => {
                marts.retain(|x| x.spec.mart != m.spec.mart);
                marts.push(m);
                mart_steps.push(report);
            }
            MartOutcome::Stop(v) => return Ok(v),
        }
    }
    let mut nb = b.clone();
    if let Some(t) = &req.delta.title {
        nb.title = t.clone();
    }
    nb.request_text = req.text.to_string();
    for c in &classified {
        let i = nb
            .charts
            .iter()
            .position(|x| x.key == c.chart)
            .expect("found above");
        let old = &b.charts[i];
        nb.charts[i] = if c.class == ChangeClass::Presentation {
            let mut x = old.clone();
            x.presentation = c.presentation.clone();
            x
        } else {
            let prefer = mart_steps
                .iter()
                .find(|s: &&Value| s["chart"] == json!(c.chart))
                .and_then(|s| s["mart"].as_str().map(str::to_string))
                .unwrap_or_else(|| old.mart.clone());
            match build_chart(
                cx,
                &marts,
                &c.chart,
                c.presentation.clone(),
                c.request.clone(),
                Some(&prefer),
                false,
            ) {
                Ok(mut x) => {
                    x.superset = old.superset.clone();
                    x.params = old.params.clone();
                    x
                }
                Err(e) => {
                    return Ok(
                        json!({"status": "refused", "reason": e.to_string(), "charts": classified}),
                    );
                }
            }
        };
    }
    let preview = side_by_side(cx, &b, &nb);
    let classes: Vec<Value> = classified
        .iter()
        .map(|c| json!({"chart": c.chart, "class": c.class.name()}))
        .collect();
    if req.preview {
        return Ok(
            json!({"status": "preview", "classes": classes, "preview": preview,
                         "marts": mart_steps, "nothing_published": true}),
        );
    }
    let mut files = vec![];
    for s in &mut mart_steps {
        if let Some([rel, text]) = s["file"].as_array().map(Vec::as_slice)
            && let (Some(rel), Some(text)) = (rel.as_str(), text.as_str())
        {
            files.push((rel.to_string(), text.to_string()));
        }
        if let Some(o) = s.as_object_mut() {
            o.remove("file");
        }
    }
    let message = format!(
        "lakehouse: change dashboard {key}: {} (by {})",
        req.text.chars().take(120).collect::<String>(),
        req.user
    );
    let out = match place {
        Landing::Main => {
            update_dashboard(cx, &b, &mut nb)?;
            let commit = commit_main(&cx.repo, &nb, req.user, &message)?;
            json!({"status": "published", "version": commit, "url": nb.superset.as_ref().map(|s| &s.url)})
        }
        Landing::Branch(br) => {
            update_dashboard(cx, &b, &mut nb)?;
            let head = git(&cx.repo, &["rev-parse", &format!("refs/heads/{br}")])?;
            let commit = commit_files_to_branch(
                &cx.repo,
                &br,
                &head,
                &[vec![(Bundle::path(&key), nb.to_yaml())], files.clone()].concat(),
                req.user,
                &message,
            )?;
            json!({"status": "published on the branch", "branch": br, "version": commit,
                   "url": nb.superset.as_ref().map(|s| &s.url)})
        }
        Landing::NewBranch(br) => {
            let base = git(&cx.repo, &["rev-parse", "HEAD"])?;
            nb.author = req.user.to_string();
            nb.origin = Some(Origin {
                dashboard: key.clone(),
                commit: base.clone(),
                superset_dashboard_id: b.superset.as_ref().map(|s| s.dashboard_id),
            });
            for c in &mut nb.charts {
                c.superset = None;
                c.params = None;
            }
            create_dashboard(cx, &mut nb)?;
            let commit = commit_files_to_branch(
                &cx.repo,
                &br,
                &base,
                &[vec![(Bundle::path(&key), nb.to_yaml())], files.clone()].concat(),
                req.user,
                &message,
            )?;
            json!({"status": "published as a copy", "branch": br, "version": commit,
                   "copy": nb.superset.as_ref().map(|s| &s.url),
                   "original_untouched": b.superset.as_ref().map(|s| &s.url)})
        }
    };
    let mut out = out;
    out["classes"] = json!(classes);
    out["preview"] = preview;
    if !mart_steps.is_empty() {
        out["marts"] = json!(mart_steps);
    }
    Ok(out)
}

/// RFC 0258 §3.4: overwrite a drifted dashboard with its bundle. Every chart's dataset SQL, name
/// and form data, the title and the layout are written again; a chart added by hand is taken off
/// the dashboard (not deleted). Nothing is committed: the bundle is already the version.
pub fn republish(cx: &Ctx<'_>, key: &str) -> anyhow::Result<Value> {
    let b = bundle_now(&cx.repo, key)?;
    let before = drift(cx.sup, &b)?;
    let d = b
        .superset
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{key} was never published"))?;
    for c in &b.charts {
        let (Some(s), Some(params)) = (&c.superset, &c.params) else {
            continue;
        };
        cx.sup.write(
            "PUT",
            &format!("/api/v1/dataset/{}?override_columns=true", s.dataset_id),
            Some(&json!({"sql": c.sql})),
        )?;
        cx.sup.write(
            "PUT",
            &format!("/api/v1/chart/{}", s.chart_id),
            Some(
                &json!({"slice_name": c.presentation.title, "viz_type": params["viz_type"],
                         "params": params.to_string()}),
            ),
        )?;
    }
    let ours: BTreeSet<i64> = b
        .charts
        .iter()
        .filter_map(|c| c.superset.as_ref().map(|s| s.chart_id))
        .chain(b.unadopted.iter().map(|u| u.chart_id))
        .collect();
    let on = cx
        .sup
        .read(&format!("/api/v1/dashboard/{}/charts", d.dashboard_id))?;
    for extra in on["result"].as_array().into_iter().flatten() {
        let Some(id) = extra["id"].as_i64().filter(|i| !ours.contains(i)) else {
            continue;
        };
        let full = cx.sup.read(&format!("/api/v1/chart/{id}"))?;
        let keep: Vec<i64> = full["result"]["dashboards"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["id"].as_i64())
            .filter(|x| *x != d.dashboard_id)
            .collect();
        cx.sup.write(
            "PUT",
            &format!("/api/v1/chart/{id}"),
            Some(&json!({"dashboards": keep})),
        )?;
    }
    let pos = position(cx, &b, d.dashboard_id)?;
    cx.sup.write(
        "PUT",
        &format!("/api/v1/dashboard/{}", d.dashboard_id),
        Some(&json!({"dashboard_title": superset_title(&b), "position_json": pos})),
    )?;
    let after = drift(cx.sup, &b)?;
    Ok(
        json!({"status": if after.is_empty() { "republished" } else { "republished, still drifted" },
              "overwritten": before, "drift": after}),
    )
}

// ─────────────────────────────── revert ───────────────────────────────

/// `git revert` of a version and republish: the dashboard as it was before that version.
pub fn revert(cx: &Ctx<'_>, key: &str, version: Option<&str>, user: &str) -> anyhow::Result<Value> {
    let all = versions(&cx.repo, key)?;
    anyhow::ensure!(
        all.len() >= 2,
        "{key} has one version: nothing to revert to"
    );
    let target = match version {
        Some(v) => all
            .iter()
            .find(|x| x.commit.starts_with(v))
            .ok_or_else(|| anyhow::anyhow!("{v} is not a version of {key}"))?,
        None => &all[0],
    };
    let current = bundle_now(&cx.repo, key)?;
    let drifted = drift(cx.sup, &current)?;
    anyhow::ensure!(
        drifted.is_empty(),
        "{key} drifted in Superset ({}): republish or adopt first",
        drifted.join("; ")
    );
    if let Err(e) = git_as(&cx.repo, user, &["revert", "--no-commit", &target.commit]) {
        let _ = git(&cx.repo, &["revert", "--abort"]);
        anyhow::bail!(
            "{} does not revert cleanly ({e}); nothing changed",
            target.commit
        );
    }
    let restored = match bundle_now(&cx.repo, key) {
        Ok(b) => b,
        Err(e) => {
            let _ = git(&cx.repo, &["revert", "--abort"]);
            return Err(e);
        }
    };
    let result = (|| -> anyhow::Result<Bundle> {
        let mut nb = restored.clone();
        // The restored charts are checked again on today's data before they are shown.
        let marts = cx.marts();
        for c in &mut nb.charts {
            if current
                .charts
                .iter()
                .any(|o| o.key == c.key && o.sql == c.sql)
            {
                continue;
            }
            let served = dash::compile_chart(
                &c.request,
                &cx.plan,
                &marts,
                &cx.target,
                Some(&c.mart),
                false,
            )
            .map_err(|u| anyhow::anyhow!("chart {}: {}", c.key, unserved_text(&u)))?;
            let rec = reconcile(cx, &c.request, &served)?;
            anyhow::ensure!(rec.ok, "chart {} no longer reconciles with silver", c.key);
            c.reconciled = Some(rec);
        }
        update_dashboard(cx, &current, &mut nb)?;
        Ok(nb)
    })();
    let nb = match result {
        Ok(nb) => nb,
        Err(e) => {
            let _ = git(&cx.repo, &["revert", "--abort"]);
            return Err(e);
        }
    };
    std::fs::write(cx.repo.join(Bundle::path(key)), nb.to_yaml())?;
    git(&cx.repo, &["add", "--", &Bundle::path(key)])?;
    let msg = format!(
        "lakehouse: revert dashboard {key} version {} (by {user})",
        &target.commit[..12]
    );
    git_as(&cx.repo, user, &["commit", "-q", "-m", &msg])?;
    let commit = git(&cx.repo, &["rev-parse", "HEAD"])?;
    Ok(
        json!({"status": "reverted", "reverted": target.commit, "version": commit,
              "terms": dash::term_diff(&current, &nb)}),
    )
}

// ─────────────────────────────── pull request and merge ───────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PullRequest {
    pub branch: String,
    pub dashboard: String,
    pub by: String,
    pub at: String,
    pub status: String,
    #[serde(default)]
    pub decided_by: Option<String>,
    #[serde(default)]
    pub decided_at: Option<String>,
}

fn requests_path(cx: &Ctx<'_>) -> PathBuf {
    cx.state_dir.join("dashboard-requests.json")
}

pub fn read_requests(cx: &Ctx<'_>) -> Vec<PullRequest> {
    std::fs::read_to_string(requests_path(cx))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_requests(cx: &Ctx<'_>, prs: &[PullRequest]) -> anyhow::Result<()> {
    std::fs::create_dir_all(&cx.state_dir)?;
    std::fs::write(requests_path(cx), serde_json::to_string_pretty(prs)? + "\n")?;
    Ok(())
}

fn three(cx: &Ctx<'_>, branch: &str) -> anyhow::Result<(String, Bundle, Bundle, Bundle)> {
    let key = branch_key(branch)?;
    let theirs = bundle_at(&cx.repo, branch, &key)?;
    let origin = theirs
        .origin
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{branch} records no origin"))?;
    let base = bundle_at(&cx.repo, &origin.commit, &key)?;
    let ours = bundle_now(&cx.repo, &key)?;
    Ok((key, base, ours, theirs))
}

/// The branch's change as the author asks for it to be accepted: terms first, then SQL, both
/// charts side by side, and whether it merges today.
pub fn review(cx: &Ctx<'_>, branch: &str) -> anyhow::Result<Value> {
    let (key, base, ours, theirs) = three(cx, branch)?;
    let merges = dash::merge3(&base, &ours, &theirs, &BTreeMap::new());
    Ok(json!({
        "dashboard": key,
        "branch": branch,
        "original": ours.superset.as_ref().map(|s| &s.url),
        "copy": theirs.superset.as_ref().map(|s| &s.url),
        "change": side_by_side(cx, &base, &theirs),
        "original_moved_on": dash::term_diff(&base, &ours),
        "conflicts": merges.err().unwrap_or_default(),
    }))
}

/// The copy's author asks for its change to be accepted into the original.
pub fn request_merge(cx: &Ctx<'_>, branch: &str, user: &str) -> anyhow::Result<Value> {
    let (key, _, _, theirs) = three(cx, branch)?;
    anyhow::ensure!(
        theirs.author == user,
        "{branch} is {}'s: only its author asks",
        theirs.author
    );
    let mut prs = read_requests(cx);
    prs.retain(|p| p.branch != branch || p.status != "open");
    prs.push(PullRequest {
        branch: branch.into(),
        dashboard: key,
        by: user.into(),
        at: cx.now.clone(),
        status: "open".into(),
        ..Default::default()
    });
    write_requests(cx, &prs)?;
    let mut v = review(cx, branch)?;
    v["status"] = json!("requested");
    Ok(v)
}

fn decide(cx: &Ctx<'_>, branch: &str, status: &str, user: &str) -> anyhow::Result<()> {
    let mut prs = read_requests(cx);
    for p in prs
        .iter_mut()
        .filter(|p| p.branch == branch && p.status == "open")
    {
        p.status = status.into();
        p.decided_by = Some(user.into());
        p.decided_at = Some(cx.now.clone());
    }
    write_requests(cx, &prs)
}

/// RFC 0258 §3.3's merge: the original's author or a reviewer accepts the branch. Terms merge
/// three-way; a conflict needs a choice (`choose`); the result is compiled over today's marts and
/// every chart reconciled, or nothing is merged. Then the original is republished, the merge is
/// committed, and the copy is removed from Superset unless `keep_copy`.
pub fn merge(
    cx: &Ctx<'_>,
    branch: &str,
    user: &str,
    choose: &BTreeMap<String, Side>,
    keep_copy: bool,
) -> anyhow::Result<Value> {
    let (key, base, ours, theirs) = three(cx, branch)?;
    anyhow::ensure!(
        user == ours.author || cx.settings.reviewers.iter().any(|r| r == user),
        "{key} is {}'s: only its author or a reviewer ({}) merges a change into it",
        ours.author,
        cx.settings.reviewers.join(", ")
    );
    let drifted = drift(cx.sup, &ours)?;
    if !drifted.is_empty() {
        return Ok(
            json!({"status": "refused", "reason": "the original drifted in Superset", "drift": drifted}),
        );
    }
    let merged = match dash::merge3(&base, &ours, &theirs, choose) {
        Ok(m) => m,
        Err(conflicts) => {
            return Ok(json!({"status": "conflict", "conflicts": conflicts,
                             "reason": "both changed the same terms: choose one side for each (--choose term=ours|theirs)"}));
        }
    };
    let (title, charts) =
        dash::from_terms(&merged, &ours, &theirs).map_err(|e| anyhow::anyhow!(e))?;
    let marts = cx.marts();
    let mut nb = ours.clone();
    nb.title = title;
    nb.request_text = format!("merge of {branch} ({})", theirs.request_text);
    let mut out_charts = vec![];
    for (k, p, r) in charts {
        let o = ours.charts.iter().find(|c| c.key == k);
        if let Some(o) = o
            && o.request.same_terms(&r)
            && o.presentation == p
        {
            out_charts.push(o.clone());
            continue;
        }
        let prefer = o.map(|o| o.mart.clone());
        let mut marts = marts.clone();
        if dash::compile_chart(&r, &cx.plan, &marts, &cx.target, prefer.as_deref(), false).is_err()
        {
            // A mart the branch added under a branch-suffixed name: added for real, through the
            // same gates, before the chart is compiled over it.
            let spec = r.to_spec(&dash::added_mart_name(&r));
            let c = Classified {
                chart: k.clone(),
                class: ChangeClass::MartAddition,
                presentation: p.clone(),
                request: r.clone(),
                served: None,
                mart: Some((&spec).into()),
                extend: None,
                changes: vec![],
                questions: vec![],
                reasons: vec![],
            };
            let creq = ChangeRequest {
                target: &key,
                delta: Delta::default(),
                user,
                text: &format!("merge of {branch}"),
                mode: Mode::Auto,
                preview: false,
                confirm: false,
            };
            match mart_step(cx, &c, None, &creq, &key)? {
                MartOutcome::Built(m, _) => marts.push(m),
                MartOutcome::Stop(v) => {
                    return Ok(
                        json!({"status": "refused", "reason": "a mart the branch added does not pass the gates on the main line", "detail": v}),
                    );
                }
            }
        }
        match build_chart(cx, &marts, &k, p, r, prefer.as_deref(), false) {
            Ok(mut c) => {
                if let Some(o) = o {
                    c.superset = o.superset.clone();
                    c.params = o.params.clone();
                }
                out_charts.push(c);
            }
            Err(e) => {
                return Ok(json!({
                    "status": "refused",
                    "reason": format!("the merged dashboard does not pass the checks on today's original ({e}): \
                                       change the branch again on the new original first"),
                }));
            }
        }
    }
    nb.charts = out_charts;
    update_dashboard(cx, &ours, &mut nb)?;
    // A merge commit: the branch's history joins the main line, with the merged bundle.
    if let Err(e) = git(
        &cx.repo,
        &[
            "merge",
            "-q",
            "-s",
            "ours",
            "--no-ff",
            "--no-commit",
            branch,
        ],
    ) {
        let _ = git(&cx.repo, &["merge", "--abort"]);
        anyhow::bail!("could not start the merge commit: {e}");
    }
    std::fs::write(cx.repo.join(Bundle::path(&key)), nb.to_yaml())?;
    git(&cx.repo, &["add", "--", &Bundle::path(&key)])?;
    let msg = format!("lakehouse: merge {branch} into dashboard {key}, accepted by {user}");
    git_as(&cx.repo, user, &["commit", "-q", "-m", &msg])?;
    let commit = git(&cx.repo, &["rev-parse", "HEAD"])?;
    if !keep_copy {
        delete_dashboard(cx, &theirs)?;
        drop_branch_marts(cx, branch)?;
    }
    retire_branch(&cx.repo, branch, "merged")?;
    decide(cx, branch, "merged", user)?;
    Ok(
        json!({"status": "merged", "version": commit, "by": user, "at": cx.now,
              "copy_removed": !keep_copy, "terms": dash::term_diff(&ours, &nb)}),
    )
}

/// Close a branch: its copy and branch-suffixed marts are removed; its commits stay.
pub fn close(cx: &Ctx<'_>, branch: &str, user: &str) -> anyhow::Result<Value> {
    let key = branch_key(branch)?;
    let theirs = bundle_at(&cx.repo, branch, &key)?;
    anyhow::ensure!(
        user == theirs.author || cx.settings.reviewers.iter().any(|r| r == user),
        "{branch} is {}'s: only its author or a reviewer closes it",
        theirs.author
    );
    delete_dashboard(cx, &theirs)?;
    let dropped = drop_branch_marts(cx, branch)?;
    retire_branch(&cx.repo, branch, "closed")?;
    decide(cx, branch, "closed", user)?;
    Ok(
        json!({"status": "closed", "branch": branch, "copy_removed": true, "marts_dropped": dropped}),
    )
}

/// RFC 0258 §3.3: branches whose head is older than `branch-ttl-days` are closed. (Superset's
/// view counts are not read: only the last change counts.)
pub fn close_abandoned(cx: &Ctx<'_>, now_unix: i64) -> anyhow::Result<Vec<String>> {
    let ttl = cx.settings.branch_ttl_days.unwrap_or(30);
    let mut closed = vec![];
    for (br, head) in branches(&cx.repo, None)? {
        let t: i64 = git(&cx.repo, &["log", "-1", "--format=%ct", &head])?
            .parse()
            .unwrap_or(now_unix);
        if now_unix - t > ttl * 86_400 {
            let key = branch_key(&br)?;
            let b = bundle_at(&cx.repo, &br, &key)?;
            delete_dashboard(cx, &b)?;
            drop_branch_marts(cx, &br)?;
            retire_branch(&cx.repo, &br, "closed")?;
            decide(cx, &br, "closed", "EKOS (branch-ttl-days)")?;
            closed.push(br);
        }
    }
    Ok(closed)
}

// ─────────────────────────────── phase 2: marts ───────────────────────────────

enum MartOutcome {
    Built(Mart, Value),
    Stop(Value),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Consent {
    at: String,
    by: String,
    mart: String,
    text_version: String,
    dashboard: String,
}

pub const CONSENT_TEXT_VERSION: &str = "rfc0248-2.3-v1";
pub const CONSENT_TEXT: &str = "This changes a gold mart other dashboards read. It counts as \
today's one modifying change. The mart is rebuilt and reconciled with silver before anything \
is published; it can be reverted (git revert and rebuild).";

fn consents(cx: &Ctx<'_>) -> Vec<Consent> {
    std::fs::read_to_string(cx.state_dir.join("dashboard-consents.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The other dashboards (and open branches) that read a mart: RFC 0248 §2.3's impact.
pub fn readers_of(cx: &Ctx<'_>, mart: &str, except: &str) -> anyhow::Result<Vec<Value>> {
    let mut out = vec![];
    for k in keys(&cx.repo) {
        let b = bundle_now(&cx.repo, &k)?;
        if k != except && b.marts().contains(mart) {
            out.push(json!({"dashboard": k, "title": b.title, "url": b.superset.as_ref().map(|s| &s.url)}));
        }
    }
    for (br, _) in branches(&cx.repo, None)? {
        let key = branch_key(&br)?;
        if let Ok(b) = bundle_at(&cx.repo, &br, &key)
            && b.marts().contains(mart)
        {
            out.push(
                json!({"branch": br, "dashboard": key, "url": b.superset.as_ref().map(|s| &s.url)}),
            );
        }
    }
    Ok(out)
}

/// The hard gates of RFC 0248 §2.3 on a built table: the grain is unique (the generated dbt
/// test), the mart reconciles with silver, and it is under the size limit.
fn gates_after_build(
    cx: &Ctx<'_>,
    spec: &MartSpec,
    resolved: &[gold::Resolved],
    c: &gold::Compiled,
    table: &str,
) -> anyhow::Result<Vec<Value>> {
    let mut gates = vec![];
    let fq = format!(
        "{}.{}.{}",
        ident(&cx.target.catalog),
        ident(&cx.target.gold_schema),
        ident(table)
    );
    let grain: Vec<String> = spec.grain.iter().map(|g| ident(g)).collect();
    let dup_sql = if grain.is_empty() {
        format!("select cast(greatest(count(*) - 1, 0) as varchar) from {fq}")
    } else {
        format!(
            "select cast(count(*) as varchar) from (select {g} from {fq} group by {g} having count(*) > 1) as d",
            g = grain.join(", ")
        )
    };
    let dups = cx
        .engine
        .rows(&dup_sql)?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .unwrap_or_default();
    gates.push(json!({"gate": "grain is unique (dbt test)", "ok": dups == "0", "detail": format!("{dups} duplicate key(s)")}));
    let rows = cx
        .engine
        .rows(&format!("select cast(count(*) as varchar) from {fq}"))?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .unwrap_or_default();
    let n: i64 = rows.parse().unwrap_or(i64::MAX);
    gates.push(json!({"gate": "size limit (1,000,000 rows)", "ok": n <= 1_000_000, "detail": format!("{rows} rows")}));
    let mut renamed = spec.clone();
    renamed.mart = table.to_string();
    for (measure, sql) in gold::reconcile_sql(
        &renamed,
        resolved,
        c,
        &cx.target.catalog,
        &cx.target.gold_schema,
    ) {
        let row = cx.engine.rows(&sql)?.into_iter().next().unwrap_or_default();
        let (a, b) = (
            row.first().cloned().unwrap_or_default(),
            row.get(1).cloned().unwrap_or_default(),
        );
        gates.push(json!({"gate": format!("reconciles with silver: {measure}"), "ok": dash::totals_equal(&a, &b),
                          "detail": format!("mart {a} / silver {b}")}));
    }
    Ok(gates)
}

fn all_ok(gates: &[Value]) -> bool {
    gates.iter().all(|g| g["ok"] == json!(true))
}

/// RFC 0248 §2.3 for one chart's mart: the hard gates before the build (every term resolves,
/// no invented atom), the confirmation and daily limit of a modifying change, the build, the
/// gates after it. A branch's addition is built under a branch-suffixed name; a modifying change
/// is never built per branch.
fn mart_step(
    cx: &Ctx<'_>,
    c: &Classified,
    branch_id: Option<&str>,
    req: &ChangeRequest<'_>,
    dashboard: &str,
) -> anyhow::Result<MartOutcome> {
    let view: &MartSpecView = c.mart.as_ref().expect("a mart class carries its mart");
    let modifying = c.class == ChangeClass::MartChange;
    let name = match (modifying, branch_id) {
        (false, Some(id)) => format!("{}__b{id}", view.mart),
        _ => view.mart.clone(),
    };
    let text = view.markdown(&format!(
        "{} for dashboard {dashboard} (EKOS RFC 0258): {}",
        if modifying { "Changed" } else { "Added" },
        req.text
    ));
    let mut spec =
        gold::parse(&text).map_err(|e| anyhow::anyhow!("the mart does not read back: {e}"))?;
    spec.mart = name.clone();
    let resolved = gold::resolve(&spec, &cx.plan);
    let mut gates = vec![];
    let compiled = gold::compile(&spec, &cx.plan, &resolved, &BTreeMap::new());
    gates.push(json!({"gate": "every term resolves", "ok": compiled.is_ok()}));
    let Ok(compiled) = compiled else {
        return Ok(MartOutcome::Stop(
            json!({"status": "refused", "reason": "a mart term does not resolve", "gates": gates}),
        ));
    };
    let inv = gold::check(&compiled);
    gates.push(json!({"gate": "0 invented atoms", "ok": inv.accepted(), "detail": format!("{} atoms checked", inv.checked)}));
    if !all_ok(&gates) {
        return Ok(MartOutcome::Stop(
            json!({"status": "refused", "reason": "a hard gate failed", "gates": gates}),
        ));
    }
    let impact = if modifying {
        readers_of(cx, &view.mart, dashboard)?
    } else {
        vec![]
    };
    if modifying && req.preview {
        return Ok(MartOutcome::Stop(json!({
            "status": "preview", "class": "mart_change", "mart": view, "changes": c.changes,
            "gates": gates, "impact": impact, "nothing_published": true,
        })));
    }
    if modifying {
        let today = &cx.now[..10.min(cx.now.len())];
        let spent = consents(cx).into_iter().any(|x| x.at.starts_with(today));
        if !req.confirm || spent {
            return Ok(MartOutcome::Stop(json!({
                "status": if spent { "refused" } else { "needs_confirmation" },
                "reason": if spent { "today's one modifying change is used" } else { "a modifying mart change needs your confirmation (--confirm)" },
                "mart": view, "changes": c.changes, "gates": gates, "impact": impact,
                "consent": {"text": CONSENT_TEXT, "version": CONSENT_TEXT_VERSION},
                "daily_limit": {"per_day": 1, "used_today": spent},
            })));
        }
    }
    // The build. Through Dagster the description is written and built by `lakehouse marts build`;
    // a sandbox builds it on the engine.
    let rel = format!("marts/{name}.md");
    let mut built_text = text.clone();
    built_text = built_text.replacen(&format!("mart: {}", view.mart), &format!("mart: {name}"), 1);
    if cx.settings.mart_build.as_deref() != Some("trino") {
        return Ok(MartOutcome::Stop(json!({
            "status": "mart_described",
            "reason": format!("the chart needs mart {name}: build it with Dagster, then change the dashboard again"),
            "mart": view, "gates": gates, "impact": impact,
        })));
    }
    let candidate = format!("{name}__candidate");
    let engine = gold::engine_sql(&compiled, &cx.target.catalog);
    let fq = |t: &str| {
        format!(
            "{}.{}.{}",
            ident(&cx.target.catalog),
            ident(&cx.target.gold_schema),
            ident(t)
        )
    };
    // Drop and create rather than `CREATE OR REPLACE` (a sandbox's memory connector has no
    // replace); the real table is only touched once the candidate passed every gate.
    cx.engine
        .build(&format!("drop table if exists {}", fq(&candidate)))?;
    cx.engine
        .build(&format!("create table {} as\n{engine}", fq(&candidate)))?;
    gates.extend(gates_after_build(
        cx, &spec, &resolved, &compiled, &candidate,
    )?);
    if !all_ok(&gates) {
        let _ = cx
            .engine
            .build(&format!("drop table if exists {}", fq(&candidate)));
        return Ok(MartOutcome::Stop(
            json!({"status": "refused", "reason": "a hard gate failed after the build", "gates": gates}),
        ));
    }
    cx.engine
        .build(&format!("drop table if exists {}", fq(&name)))?;
    cx.engine.build(&format!(
        "create table {} as select * from {}",
        fq(&name),
        fq(&candidate)
    ))?;
    cx.engine
        .build(&format!("drop table if exists {}", fq(&candidate)))?;
    // The description is committed: on the main line, or (an addition on a branch) on the branch.
    let mut file = Value::Null;
    match branch_id {
        // An addition on a branch is committed with the branch's bundle.
        Some(_) if !modifying => file = json!([rel, built_text]),
        _ => {
            std::fs::create_dir_all(cx.repo.join("marts"))?;
            std::fs::write(cx.repo.join(&rel), &built_text)?;
            git(&cx.repo, &["add", "--", &rel])?;
            let msg = format!(
                "lakehouse: {} gold mart {name} for dashboard {dashboard} (RFC 0248 §2.3, by {})",
                if modifying { "change" } else { "add" },
                req.user
            );
            git_as(
                &cx.repo,
                req.user,
                &["commit", "-q", "-m", &msg, "--", &rel],
            )?;
        }
    }
    if let Some(id) = branch_id
        && !modifying
    {
        record_branch_mart(cx, id, &name)?;
    }
    if modifying {
        let mut all = consents(cx);
        all.push(Consent {
            at: cx.now.clone(),
            by: req.user.into(),
            mart: name.clone(),
            text_version: CONSENT_TEXT_VERSION.into(),
            dashboard: dashboard.into(),
        });
        std::fs::create_dir_all(&cx.state_dir)?;
        std::fs::write(
            cx.state_dir.join("dashboard-consents.json"),
            serde_json::to_string_pretty(&all)? + "\n",
        )?;
    }
    let m = Mart {
        digest: gold::mart_id(&compiled),
        spec,
        resolved,
    };
    Ok(MartOutcome::Built(
        m,
        json!({"chart": c.chart, "class": c.class.name(), "mart": name, "gates": gates, "impact": impact,
               "extend_instead": c.extend, "file": file}),
    ))
}

fn branch_marts_path(cx: &Ctx<'_>) -> PathBuf {
    cx.state_dir.join("dashboard-branch-marts.json")
}

fn record_branch_mart(cx: &Ctx<'_>, id: &str, mart: &str) -> anyhow::Result<()> {
    let mut all: BTreeMap<String, Vec<String>> = std::fs::read_to_string(branch_marts_path(cx))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let e = all.entry(id.to_string()).or_default();
    if !e.contains(&mart.to_string()) {
        e.push(mart.to_string());
    }
    std::fs::create_dir_all(&cx.state_dir)?;
    std::fs::write(
        branch_marts_path(cx),
        serde_json::to_string_pretty(&all)? + "\n",
    )?;
    Ok(())
}

/// Drop a branch's branch-suffixed marts (built only in a sandbox).
fn drop_branch_marts(cx: &Ctx<'_>, branch: &str) -> anyhow::Result<Vec<String>> {
    let id = branch.rsplit('/').next().unwrap_or_default().to_string();
    let mut all: BTreeMap<String, Vec<String>> = std::fs::read_to_string(branch_marts_path(cx))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let dropped = all.remove(&id).unwrap_or_default();
    for m in &dropped {
        cx.engine.build(&format!(
            "drop table if exists {}.{}.{}",
            ident(&cx.target.catalog),
            ident(&cx.target.gold_schema),
            ident(m)
        ))?;
    }
    if !dropped.is_empty() {
        std::fs::write(
            branch_marts_path(cx),
            serde_json::to_string_pretty(&all)? + "\n",
        )?;
    }
    Ok(dropped)
}

// ─────────────────────────────── sync (RFC 0248 §2.4 → phase 3) ───────────────────────────────

/// Bundles the Superset fork wrote into the pipeline repository and did not commit: each is
/// checked and committed as its author's version. Returns `(key, commit or why not)`.
pub fn record_written_bundles(
    repo: &Path,
) -> anyhow::Result<Vec<(String, Result<String, String>)>> {
    let mut out = vec![];
    let status = git(
        repo,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            dash::BUNDLE_DIR,
        ],
    )?;
    for line in status.lines() {
        let rel = line.get(3..).unwrap_or_default().trim().to_string();
        let Some(key) = rel
            .strip_prefix(&format!("{}/", dash::BUNDLE_DIR))
            .and_then(|f| f.strip_suffix(".yaml"))
            .map(str::to_string)
        else {
            continue;
        };
        let r = match bundle_now(repo, &key) {
            Ok(b) => {
                let msg = format!(
                    "lakehouse: record dashboard {key} as published by {} (RFC 0248 §2.4)",
                    b.author
                );
                git(repo, &["add", "--", &rel])
                    .and_then(|_| {
                        git_as(repo, &b.author, &["commit", "-q", "-m", &msg, "--", &rel])
                    })
                    .and_then(|_| git(repo, &["rev-parse", "HEAD"]))
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(format!("not recorded: {e}")),
        };
        out.push((key, r));
    }
    Ok(out)
}

/// Each described mart's digest and the silver tables it reads (`schema.table`, as RFC 0196 health
/// names them), compiled from the pipeline repository.
pub fn mart_facts(
    repo: &Path,
    plan: &Plan,
) -> (BTreeMap<String, String>, BTreeMap<String, Vec<String>>) {
    let mut digests = BTreeMap::new();
    let mut reads = BTreeMap::new();
    for spec in crate::ask_cmd::existing_marts(repo).0 {
        let resolved = gold::resolve(&spec, plan);
        if let Ok(c) = gold::compile(&spec, plan, &resolved, &BTreeMap::new()) {
            digests.insert(spec.mart.clone(), gold::mart_id(&c));
            let tables: Vec<String> = c
                .tables
                .iter()
                .filter_map(|t| {
                    plan.tables
                        .iter()
                        .find(|p| &p.table == t || &model_of(p) == t)
                })
                .map(|p| format!("{}.{}", p.schema, p.table).to_ascii_lowercase())
                .collect();
            reads.insert(spec.mart.clone(), tables);
        }
    }
    (digests, reads)
}

fn model_of(p: &ekos_lakehouse_bronze::plan::TablePlan) -> String {
    ekos_lakehouse_silver::model_name(p)
}

/// Every bundle to sync: the main line's, and each open branch's copy.
pub fn all_bundles(repo: &Path) -> Vec<(String, Bundle)> {
    let mut out: Vec<(String, Bundle)> = keys(repo)
        .into_iter()
        .filter_map(|k| bundle_now(repo, &k).ok().map(|b| (Bundle::path(&k), b)))
        .collect();
    for (br, _) in branches(repo, None).unwrap_or_default() {
        if let Ok(key) = branch_key(&br)
            && let Ok(b) = bundle_at(repo, &br, &key)
        {
            out.push((format!("{br}:{}", Bundle::path(&key)), b));
        }
    }
    out
}

/// A bundle as the health record `sync` stores: each chart reads its mart, carries its own
/// reconciliation against the mart's digest now, and the drift read from Superset.
pub fn bundle_record(
    sup: &dyn SupersetApi,
    label: &str,
    b: &Bundle,
    target: &Target,
    digests: &BTreeMap<String, String>,
    mart_reads: &BTreeMap<String, Vec<String>>,
) -> anyhow::Result<ekos_lakehouse_health::dashboard::DashboardRecord> {
    use ekos_lakehouse_health::dashboard::{ChartRecon, ChartRecord, DashboardRecord};
    let d = b
        .superset
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{label} was never published"))?;
    let drifted = drift(sup, b)?;
    let mut charts = vec![];
    let mut dashboard_drift = vec![];
    for x in &drifted {
        if !b
            .charts
            .iter()
            .any(|c| x.starts_with(&format!("{}:", c.presentation.title)))
        {
            dashboard_drift.push(x.clone());
        }
    }
    for c in &b.charts {
        let prefix = format!("{}:", c.presentation.title);
        let digest = c
            .marts
            .first()
            .map(|m| m.digest.clone())
            .unwrap_or_default();
        charts.push(ChartRecord {
            chart_id: c.superset.as_ref().map(|s| s.chart_id).unwrap_or_default(),
            name: c.presentation.title.clone(),
            dataset: c
                .superset
                .as_ref()
                .map(|s| format!("{}.{}", target.gold_schema, s.dataset))
                .unwrap_or_default(),
            reads: vec![format!(
                "{}.{}.{}",
                target.catalog, target.gold_schema, c.mart
            )],
            mart_reads: mart_reads.get(&c.mart).cloned().unwrap_or_default(),
            unreadable: None,
            drift: drifted
                .iter()
                .filter(|x| x.starts_with(&prefix))
                .map(|x| x[prefix.len()..].trim().to_string())
                .collect(),
            reconciled: Some(ChartRecon {
                ok: c.reconciled.as_ref().map(|r| r.ok),
                at: c
                    .reconciled
                    .as_ref()
                    .map(|r| r.at.clone())
                    .unwrap_or_default(),
                mart_digest: digest,
                current_digest: digests.get(&c.mart).cloned(),
                differs: c
                    .reconciled
                    .iter()
                    .flat_map(|r| {
                        r.measures
                            .iter()
                            .filter(|m| !m.equal)
                            .map(|m| m.measure.clone())
                    })
                    .collect(),
            }),
        });
    }
    if !b.unadopted.is_empty() {
        // Charts adoption could not read are judged as before a bundle: by their SQL.
        let list = |what: &str| -> anyhow::Result<Vec<Value>> {
            Ok(
                sup.read(&format!("/api/v1/dashboard/{}/{what}", d.dashboard_id))?["result"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            )
        };
        let (all, datasets) = (list("charts")?, list("datasets")?);
        let ids: BTreeSet<i64> = b.unadopted.iter().map(|u| u.chart_id).collect();
        charts.extend(
            crate::dashboards::chart_records(&all, &datasets)
                .into_iter()
                .filter(|c| ids.contains(&c.chart_id)),
        );
    }
    Ok(DashboardRecord {
        dashboard_id: d.dashboard_id,
        title: superset_title(b),
        url: d.url.clone(),
        charts,
        chain_known: vec![],
        removed: false,
        bundle: Some(label.to_string()),
        drift: dashboard_drift,
    })
}

#[cfg(test)]
#[path = "dashboard_change_tests.rs"]
mod tests;
