//! `lakehouse dashboards publish|change|revert|versions|diff|branches|review|request-merge|
//! merge|close|drift|adopt` (EKOS RFC 0258 phases 1, 2, 4, 5; RFC 0248 §2.4's bundle). The work is
//! in [`crate::dashboard_change`] and [`crate::dashboard_adopt`]; this file reads the arguments,
//! builds the context off the async runtime and prints.

use crate::dashboard_change::{self as dc, ChangeRequest, Ctx, Mode};
use crate::dashboards::Superset;
use crate::settings::Settings;
use ekos::extension::CommandContext;
use ekos_lakehouse_gold::dashboard::{self as dash, Bundle, Side, Target};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const NAMES: &[&str] = &[
    "publish",
    "change",
    "revert",
    "versions",
    "diff",
    "branches",
    "review",
    "request-merge",
    "merge",
    "close",
    "drift",
    "republish",
    "adopt",
];

pub fn handles(name: &str) -> bool {
    NAMES.contains(&name)
}

fn arg(name: &'static str, help: &'static str) -> clap::Arg {
    clap::Arg::new(name).long(name).help(help)
}

fn flag(name: &'static str, help: &'static str) -> clap::Arg {
    clap::Arg::new(name)
        .long(name)
        .action(clap::ArgAction::SetTrue)
        .help(help)
}

/// Id `by`, flag `--as`: the console fills it with the signed-in person for every class.
fn who() -> clap::Arg {
    clap::Arg::new("by")
        .long("as")
        .help("Who asks (default: $USER; the console sets it)")
}

fn json_flag() -> clap::Arg {
    flag("json", "Machine-readable output")
}

pub fn commands() -> Vec<clap::Command> {
    use clap::Command;
    vec![
        Command::new("publish")
            .about("Publish a dashboard from terms (RFC 0248 §2.4): compile, reconcile with silver, \
                    create it in Superset and commit its bundle as version 1")
            .arg(clap::Arg::new("file").required(true).help("The dashboard as JSON: title, charts (presentation + request)"))
            .arg(who())
            .arg(json_flag()),
        Command::new("change")
            .about("Change a published dashboard by terms (RFC 0258 §3.2): a new version, or a copy \
                    on a branch for someone else's")
            .arg(arg("dashboard", "The dashboard's key, or a branch of it (ekos/dash/<key>/<id>)").required(true))
            .arg(arg("delta", "The change as JSON terms (a delta)"))
            .arg(arg("text", "The change in words: a local model turns it into a delta"))
            .group(clap::ArgGroup::new("what").args(["delta", "text"]).required(true))
            .arg(who())
            .arg(flag("branch", "On a branch even for the author (a copy in Superset)"))
            .arg(flag("preview", "Old and new side by side, the SQL and term diff; publish nothing"))
            .arg(flag("confirm", "Yes to a modifying mart change's confirmation (RFC 0248 §2.3)"))
            .arg(flag("allow-metered", "Allow a metered model route for --text"))
            .arg(json_flag()),
        Command::new("revert")
            .about("Revert a version (git revert) and republish the dashboard as it was")
            .arg(arg("dashboard", "The dashboard's key").required(true))
            .arg(arg("commit", "The version (its commit) to revert; default the latest"))
            .arg(who())
            .arg(json_flag()),
        Command::new("versions")
            .about("Every version of a dashboard: author, request, term diff")
            .arg(arg("dashboard", "The dashboard's key"))
            .arg(json_flag()),
        Command::new("diff")
            .about("Terms first, SQL second, between two versions or branches")
            .arg(arg("dashboard", "The dashboard's key").required(true))
            .arg(arg("from", "A commit or branch").required(true))
            .arg(arg("to", "A commit or branch (default: the current version)"))
            .arg(json_flag()),
        Command::new("branches")
            .about("Open branches (copies), with their pull requests")
            .arg(arg("dashboard", "Only this dashboard's"))
            .arg(json_flag()),
        Command::new("review")
            .about("A branch's change as a pull request shows it: terms, SQL, both charts side by side")
            .arg(arg("branch", "ekos/dash/<key>/<id>").required(true))
            .arg(json_flag()),
        Command::new("request-merge")
            .about("Ask the original's author to accept the branch's change")
            .arg(arg("branch", "ekos/dash/<key>/<id>").required(true))
            .arg(who())
            .arg(json_flag()),
        Command::new("merge")
            .about("Accept a branch into the original (its author or a reviewer), at the level of terms")
            .arg(arg("branch", "ekos/dash/<key>/<id>").required(true))
            .arg(
                clap::Arg::new("choose")
                    .long("choose")
                    .action(clap::ArgAction::Append)
                    .help("For a conflict: `<term>=ours|theirs` (repeat)"),
            )
            .arg(flag("keep-copy", "Keep the copy in Superset after the merge"))
            .arg(who())
            .arg(json_flag()),
        Command::new("close")
            .about("Close a branch: its copy and branch marts are removed; its commits stay")
            .arg(arg("branch", "ekos/dash/<key>/<id>; or --abandoned"))
            .arg(flag("abandoned", "Close every branch older than branch-ttl-days"))
            .arg(who())
            .arg(json_flag()),
        Command::new("drift")
            .about("Read each published dashboard back from Superset and compare it with its bundle")
            .arg(arg("dashboard", "Only this dashboard"))
            .arg(json_flag()),
        Command::new("republish")
            .about("Overwrite a drifted dashboard in Superset with its bundle (RFC 0258 §3.4)")
            .arg(arg("dashboard", "The dashboard's key").required(true))
            .arg(json_flag()),
        Command::new("adopt")
            .about("Adopt a Superset dashboard EKOS did not publish (RFC 0258 §3.6): resolve its \
                    charts to terms over the gold marts; publishes nothing")
            .arg(arg("superset-id", "The Superset dashboard id").required(true))
            .arg(arg("key", "The bundle's key (default: adopted-<id>)"))
            .arg(flag("dry-run", "Report only; write no bundle"))
            .arg(who())
            .arg(json_flag()),
    ]
}

fn person(m: &clap::ArgMatches) -> anyhow::Result<String> {
    m.try_get_one::<String>("by")
        .ok()
        .flatten()
        .cloned()
        .or_else(|| std::env::var("USER").ok())
        .filter(|b| !b.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("who is asking? Pass --as <you>"))
}

struct Prepared {
    repo: PathBuf,
    plan: ekos_lakehouse_bronze::plan::Plan,
    target: Target,
    settings: crate::dashboards::DashboardsSettings,
    conn: Option<ekos_lakehouse_trino::Connection>,
    state_dir: PathBuf,
}

fn prepare(ctx: &CommandContext<'_>, s: &Settings, repo: &Path) -> anyhow::Result<Prepared> {
    let plan = crate::plan_cmd::read_lock(repo)?;
    let target = Target {
        catalog: plan.settings.target_catalog.clone(),
        gold_schema: ekos_lakehouse_bronze::plan::gold_schema(&plan.settings.namespace_prefix),
    };
    Ok(Prepared {
        repo: repo.to_path_buf(),
        plan,
        target,
        settings: s.dashboards.clone(),
        conn: s.trino_connection().ok(),
        state_dir: ctx.config.ekos_dir(ctx.cwd).join("lakehouse"),
    })
}

/// No engine configured: a command that needs one says so when it asks.
struct NoEngine;

impl dc::Engine for NoEngine {
    fn rows(&self, _sql: &str) -> anyhow::Result<Vec<Vec<String>>> {
        anyhow::bail!("set `trino-url` in [extensions.lakehouse]")
    }
}

/// Run `f` with a full context on a thread of its own (the HTTP clients block).
fn with_ctx<T: Send>(
    p: Prepared,
    f: impl FnOnce(&Ctx<'_>) -> anyhow::Result<T> + Send,
) -> anyhow::Result<T> {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let sup = Superset::connect(&p.settings)?;
                let trino = p.conn.clone().map(|conn| dc::Trino { conn });
                let engine: &dyn dc::Engine = match &trino {
                    Some(t) => t,
                    None => &NoEngine,
                };
                let cx = Ctx {
                    repo: p.repo,
                    plan: p.plan,
                    target: p.target,
                    sup: &sup,
                    engine,
                    settings: p.settings,
                    state_dir: p.state_dir,
                    now: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                };
                f(&cx)
            })
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the worker thread panicked")))
    })
}

fn print(v: &Value, json_out: bool) -> anyhow::Result<()> {
    if json_out {
        println!("{}", serde_json::to_string_pretty(v)?);
    } else {
        print!("{}", render(v));
    }
    Ok(())
}

/// A short plain rendering: the status, then each field on its own lines.
fn render(v: &Value) -> String {
    let mut out = String::new();
    if let Some(s) = v["status"].as_str() {
        out.push_str(&format!("status: {s}\n"));
    }
    if let Some(r) = v["reason"].as_str() {
        out.push_str(&format!("reason: {r}\n"));
    }
    for k in [
        "version",
        "branch",
        "url",
        "copy",
        "original_untouched",
        "reverted",
    ] {
        if let Some(x) = v[k].as_str() {
            out.push_str(&format!("{k}: {x}\n"));
        }
    }
    for c in v["classes"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "chart {}: {}\n",
            c["chart"].as_str().unwrap_or_default(),
            c["class"].as_str().unwrap_or_default()
        ));
    }
    for t in v["preview"]["terms"]
        .as_array()
        .or(v["terms"].as_array())
        .into_iter()
        .flatten()
    {
        out.push_str(&format!(
            "  term {}: {} -> {}\n",
            t["term"].as_str().unwrap_or_default(),
            t["before"],
            t["after"]
        ));
    }
    for d in v["drift"].as_array().into_iter().flatten() {
        out.push_str(&format!("  drift: {}\n", d.as_str().unwrap_or_default()));
    }
    for c in v["conflicts"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  conflict {}: ours {} / theirs {}\n",
            c["term"].as_str().unwrap_or_default(),
            c["ours"],
            c["theirs"]
        ));
    }
    if out.is_empty() {
        out = format!("{}\n", serde_json::to_string_pretty(v).unwrap_or_default());
    }
    out
}

pub fn run(
    ctx: &CommandContext<'_>,
    s: &Settings,
    repo: &Path,
    name: &str,
    m: &clap::ArgMatches,
) -> anyhow::Result<()> {
    let json_out = m
        .try_get_one::<bool>("json")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);
    let get = |k: &str| m.try_get_one::<String>(k).ok().flatten().cloned();
    let v: Value = match name {
        "versions" => versions_json(repo, get("dashboard").as_deref())?,
        "diff" => {
            let key = get("dashboard").expect("required");
            let from = dc::bundle_at(repo, &get("from").expect("required"), &key)?;
            let to = match get("to") {
                Some(r) => dc::bundle_at(repo, &r, &key)?,
                None => dc::bundle_now(repo, &key)?,
            };
            diff_json(&from, &to)
        }
        "branches" => {
            let p = prepare(ctx, s, repo)?;
            let prs: Vec<dc::PullRequest> =
                std::fs::read_to_string(p.state_dir.join("dashboard-requests.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default();
            let rows: Vec<Value> = dc::branches(repo, get("dashboard").as_deref())?
                .into_iter()
                .map(|(br, head)| {
                    let key = dc::branch_key(&br).unwrap_or_default();
                    let b = dc::bundle_at(repo, &br, &key).ok();
                    json!({
                        "branch": br, "head": head, "dashboard": key,
                        "author": b.as_ref().map(|b| b.author.clone()),
                        "copy": b.as_ref().and_then(|b| b.superset.as_ref().map(|s| s.url.clone())),
                        "request": prs.iter().rev().find(|p| p.branch == br).map(|p| p.status.clone()),
                    })
                })
                .collect();
            json!({"branches": rows, "requests": prs})
        }
        _ => {
            let p = prepare(ctx, s, repo)?;
            let user = person(m).ok();
            let delta = match (get("delta"), get("text")) {
                (Some(f), _) => Some(read_delta(&f)?),
                (None, Some(t)) if name == "change" => Some(model_delta(
                    ctx,
                    m,
                    repo,
                    &get("dashboard").unwrap_or_default(),
                    &t,
                )?),
                _ => None,
            };
            let name = name.to_string();
            let m = m.clone();
            with_ctx(p, move |cx| dispatch(cx, &name, &m, user.as_deref(), delta))?
        }
    };
    print(&v, json_out)?;
    if let Some(st) = v["status"].as_str()
        && matches!(
            st,
            "refused" | "conflict" | "impossible" | "needs_clarification"
        )
    {
        anyhow::bail!("{}", v["reason"].as_str().unwrap_or(st));
    }
    Ok(())
}

fn need_user(u: Option<&str>) -> anyhow::Result<&str> {
    u.ok_or_else(|| anyhow::anyhow!("who is asking? Pass --as <you>"))
}

fn dispatch(
    cx: &Ctx<'_>,
    name: &str,
    m: &clap::ArgMatches,
    user: Option<&str>,
    delta: Option<dash::Delta>,
) -> anyhow::Result<Value> {
    let get = |k: &str| m.try_get_one::<String>(k).ok().flatten().cloned();
    let on = |k: &str| {
        m.try_get_one::<bool>(k)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false)
    };
    match name {
        "publish" => {
            let f = get("file").expect("required");
            let text = std::fs::read_to_string(&f).map_err(|e| anyhow::anyhow!("{f}: {e}"))?;
            let d: dc::NewDashboard = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("{f}: not a dashboard of terms: {e}"))?;
            let b = dc::publish_new(cx, d, need_user(user)?)?;
            let version = dc::versions(&cx.repo, &b.dashboard)?
                .first()
                .map(|v| v.commit.clone());
            Ok(
                json!({"status": "published", "dashboard": b.dashboard, "version": version,
                      "url": b.superset.as_ref().map(|s| s.url.clone()),
                      "charts": b.charts.iter().map(|c| json!({"chart": c.key, "mart": c.mart, "reconciled": c.reconciled})).collect::<Vec<_>>()}),
            )
        }
        "change" => dc::change(
            cx,
            ChangeRequest {
                target: &get("dashboard").expect("required"),
                delta: delta.expect("delta or text"),
                user: need_user(user)?,
                text: &get("text").unwrap_or_else(|| {
                    let f = get("delta").unwrap_or_default();
                    let name = Path::new(&f)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or(f);
                    format!("change {name}")
                }),
                mode: if on("branch") {
                    Mode::Branch
                } else {
                    Mode::Auto
                },
                preview: on("preview"),
                confirm: on("confirm"),
            },
        ),
        "revert" => dc::revert(
            cx,
            &get("dashboard").expect("required"),
            get("commit").as_deref(),
            need_user(user)?,
        ),
        "review" => dc::review(cx, &get("branch").expect("required")),
        "request-merge" => {
            dc::request_merge(cx, &get("branch").expect("required"), need_user(user)?)
        }
        "merge" => {
            let mut choose = BTreeMap::new();
            for c in m.get_many::<String>("choose").into_iter().flatten() {
                let (term, side) = c
                    .rsplit_once('=')
                    .ok_or_else(|| anyhow::anyhow!("--choose {c:?}: `<term>=ours|theirs`"))?;
                let side = match side {
                    "ours" => Side::Ours,
                    "theirs" => Side::Theirs,
                    x => anyhow::bail!("--choose {term}={x}: ours or theirs"),
                };
                choose.insert(term.to_string(), side);
            }
            dc::merge(
                cx,
                &get("branch").expect("required"),
                need_user(user)?,
                &choose,
                on("keep-copy"),
            )
        }
        "close" => {
            if on("abandoned") {
                let closed = dc::close_abandoned(cx, chrono::Utc::now().timestamp())?;
                Ok(json!({"status": "closed", "branches": closed}))
            } else {
                let br =
                    get("branch").ok_or_else(|| anyhow::anyhow!("--branch, or --abandoned"))?;
                dc::close(cx, &br, need_user(user)?)
            }
        }
        "drift" => {
            let keys = match get("dashboard") {
                Some(k) => vec![k],
                None => dc::keys(&cx.repo),
            };
            let mut rows = vec![];
            for k in keys {
                let b = dc::bundle_now(&cx.repo, &k)?;
                let d = dc::drift(cx.sup, &b)?;
                rows.push(json!({"dashboard": k, "drifted": !d.is_empty(), "drift": d}));
            }
            Ok(json!({"dashboards": rows}))
        }
        "republish" => dc::republish(cx, &get("dashboard").expect("required")),
        "adopt" => {
            let id: i64 = get("superset-id")
                .expect("required")
                .parse()
                .map_err(|_| anyhow::anyhow!("--superset-id is a number"))?;
            crate::dashboard_adopt::adopt(
                cx,
                id,
                get("key").as_deref(),
                need_user(user)?,
                on("dry-run"),
            )
        }
        other => anyhow::bail!("unknown dashboards command {other}"),
    }
}

fn read_delta(f: &str) -> anyhow::Result<dash::Delta> {
    let text = std::fs::read_to_string(f).map_err(|e| anyhow::anyhow!("{f}: {e}"))?;
    dash::parse_delta(&text).map_err(|e| anyhow::anyhow!("{f}: {e}; nothing is changed"))
}

fn versions_json(repo: &Path, key: Option<&str>) -> anyhow::Result<Value> {
    let keys = match key {
        Some(k) => vec![k.to_string()],
        None => dc::keys(repo),
    };
    let mut out = vec![];
    for k in keys {
        let vs = dc::versions(repo, &k)?;
        let mut rows = vec![];
        for (i, v) in vs.iter().enumerate() {
            let now = dc::bundle_at(repo, &v.commit, &k).ok();
            let before = vs
                .get(i + 1)
                .and_then(|p| dc::bundle_at(repo, &p.commit, &k).ok());
            rows.push(json!({
                "version": vs.len() - i,
                "commit": v.commit, "author": v.author, "at": v.at, "message": v.message,
                "request": now.as_ref().map(|b| b.request_text.clone()),
                "terms": match (&before, &now) {
                    (Some(a), Some(b)) => json!(dash::term_diff(a, b)),
                    _ => json!([]),
                },
            }));
        }
        out.push(json!({"dashboard": k, "versions": rows}));
    }
    // One flat list, newest first, for the console's table.
    let mut flat: Vec<Value> = out
        .iter()
        .flat_map(|d| {
            d["versions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |mut v| {
                    v["dashboard"] = d["dashboard"].clone();
                    v["changed"] = json!(v["terms"].as_array().map_or(0, Vec::len));
                    v
                })
        })
        .collect();
    flat.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
    flat.truncate(100);
    Ok(json!({"dashboards": out, "recent": flat}))
}

fn diff_json(a: &Bundle, b: &Bundle) -> Value {
    let sql: Vec<Value> = b
        .charts
        .iter()
        .filter_map(|c| {
            let before = a
                .charts
                .iter()
                .find(|o| o.key == c.key)
                .map(|o| o.sql.as_str())
                .unwrap_or("");
            (before != c.sql)
                .then(|| json!({"chart": c.key, "sql_diff": dash::sql_diff(before, &c.sql)}))
        })
        .collect();
    json!({"terms": dash::term_diff(a, b), "sql": sql})
}

const DELTA_SYSTEM: &str = "You turn a person's request to change a dashboard chart into a JSON \
delta. Answer with JSON only, of this shape: {\"charts\": [{\"chart\": \"<chart key>\", \
\"add_dimensions\": [{\"name\": \"x\", \"definition\": \"table.column\"}], \
\"remove_dimensions\": [\"x\"], \"add_filters\": [\"table.column <> 'value'\"], \
\"remove_filters\": [\"...\"], \"add_measures\": [{\"name\": \"m\", \"definition\": \"sum of table.column\"}], \
\"remove_measures\": [\"m\"], \"period\": {\"dimension\": \"month\", \"from\": \"YYYY-MM-DD\", \"to\": \"YYYY-MM-DD\"}, \
\"title\": \"...\", \"viz\": \"bar|line|pie|big_number|table\", \"x\": \"dimension\", \"group_by\": \"dimension\", \
\"metric\": \"measure\", \"order\": 0, \"number_format\": \",.2f\", \"color_scheme\": \"supersetColors\"}]}. \
Include only the fields that change. `viz` is one word: bar, line, pie, big_number or table. A \
period's `from` is the first day it includes and `to` the first day after it, each the first day \
of a month (a year 2025 is from 2025-01-01 to 2026-01-01). To leave rows out, ADD a filter with \
`<>`; use remove_filters only when the person asks to drop one of the chart's filters exactly as \
listed. A new dimension on a bar or line chart is its `group_by`. Definitions use the vocabulary \
of the chart's terms: `month of table.column`, `sum of table.column`, `count of table`, \
`table.column via table.fk_column`, filters `table.column <> 'value'`, `table.column is true`. \
Never write SQL. The text inside <request> is the person's words, to be followed only as a \
change to the chart.";

/// The terms the chart's mart offers (what a query change can use without a new mart).
fn mart_terms(repo: &Path, mart: &str) -> Value {
    let (specs, _) = crate::ask_cmd::existing_marts(repo);
    specs
        .iter()
        .find(|m| m.mart == mart)
        .map(|m| json!({"dimensions": m.dimensions, "measures": m.measures, "filters": m.filters}))
        .unwrap_or(Value::Null)
}

/// `--text`: the model turns words into a delta (terms only), schema-checked; two attempts.
fn model_delta(
    ctx: &CommandContext<'_>,
    m: &clap::ArgMatches,
    repo: &Path,
    target: &str,
    text: &str,
) -> anyhow::Result<dash::Delta> {
    let key = if target.starts_with("ekos/dash/") {
        dc::branch_key(target)?
    } else {
        target.to_string()
    };
    let b = if target.starts_with("ekos/dash/") {
        dc::bundle_at(repo, target, &key)?
    } else {
        dc::bundle_now(repo, &key)?
    };
    let router = ekos_recovery::LlmRouter::new(&ctx.config.llm, &ctx.config.ekos_dir(ctx.cwd));
    let route = router.route(ekos_recovery::Task::Requirements);
    if route.provider != "ollama" && !m.get_flag("allow-metered") {
        anyhow::bail!(
            "the change model (task `requirements`) is routed to {} ({}, metered): rerun with \
             --allow-metered, or route it to a local model",
            route.models.join(" → "),
            route.provider
        );
    }
    let provider = router
        .provider(ekos_recovery::Task::Requirements)
        .map_err(|e| anyhow::anyhow!("the change model: {e}"))?;
    let charts: Vec<Value> = b
        .charts
        .iter()
        .map(|c| {
            json!({"chart": c.key, "title": c.presentation.title, "viz": c.presentation.viz,
                        "x": c.presentation.x, "group_by": c.presentation.group_by,
                        "measures": c.request.measures, "dimensions": c.request.dimensions,
                        "filters": c.request.filters, "period": c.request.period,
                        "other_terms_of_its_mart": mart_terms(repo, &c.mart)})
        })
        .collect();
    let clean: String = text
        .chars()
        .filter(|c| !matches!(c, '<' | '>'))
        .take(600)
        .collect();
    let user = format!(
        "<charts>\n{}\n</charts>\n<request>\n{clean}\n</request>",
        serde_json::to_string_pretty(&charts)?
    );
    std::thread::scope(|s| {
        s.spawn(|| -> anyhow::Result<dash::Delta> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let mut feedback = String::new();
            let mut last = String::new();
            for _ in 0..3 {
                let u = format!("{user}{feedback}");
                let resp = rt
                    .block_on(provider.complete(&ekos_recovery::LlmRequest {
                        system: DELTA_SYSTEM,
                        user: &u,
                        prompt_version: "lakehouse-dashboard-delta-1",
                        max_tokens: 600,
                        history: &[],
                    }))
                    .map_err(|e| anyhow::anyhow!("the change model ({}): {e}", provider.model_name()))?;
                let body = resp.content;
                let json_text = body
                    .find('{')
                    .and_then(|i| body.rfind('}').map(|j| &body[i..=j]))
                    .unwrap_or("");
                // Checked against the stored terms as the change itself will be: a delta that does
                // not apply goes back to the model with the reason.
                let checked = dash::parse_delta(json_text).and_then(|d| {
                    for cd in &d.charts {
                        let c = b
                            .chart(&cd.chart)
                            .ok_or_else(|| format!("there is no chart {:?}", cd.chart))?;
                        dash::apply_chart_delta(c, cd).map_err(|e| format!("chart {}: {e}", c.key))?;
                    }
                    Ok(d)
                });
                match checked {
                    Ok(d) => return Ok(d),
                    Err(e) => {
                        last = e.clone();
                        feedback = format!("\nYour last answer could not be used: {e}. Answer again with only the JSON.");
                    }
                }
            }
            anyhow::bail!("the model gave no usable change in 3 attempts ({last}); nothing is changed")
        })
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the model thread panicked")))
    })
}
