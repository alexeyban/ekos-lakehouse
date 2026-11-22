//! `lakehouse talend …` (EKOS RFC 0268): Talend projects into silver and gold dbt models.
//!
//! This module is a raw-content entry point (RFC 0043): every file is checked against the
//! excluded-path globs and redacted before the model sees it, as `pentaho_cmd` does.

use crate::pentaho_cmd::{WriteOutcome, write_translation_of};
use crate::plan_cmd::{read_lock, sha256};
use crate::settings::Settings;
use crate::sync::git;
use ekos::extension::CommandContext;
use ekos_common::redaction::{RedactionConfig, is_excluded_path, redact};
use ekos_talend::{PROJECT_MARKER, Project, ProjectFile, Role, read_project, role};
use serde_json::json;
use std::path::{Path, PathBuf};

/// A file larger than this is skipped (a job item is rarely over a few MB).
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// A Talend project of the workspace: its name (the directory's) and directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDir {
    pub name: String,
    pub path: PathBuf,
}

/// Whether a pipeline-repository path belongs to a Talend translation: `generate` leaves these
/// alone, `talend translate` owns them per project.
pub fn is_talend_path(path: &str) -> bool {
    path.starts_with("talend/")
        || path.starts_with("proposals/talend-")
        || path.contains("/talend/")
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || ekos_talend::SKIPPED_DIRS.contains(&name)
}

/// Every directory holding `talend.project` under the workspace's observed directories.
pub fn projects(ctx: &CommandContext<'_>) -> Vec<ProjectDir> {
    let mut out = Vec::new();
    for base in ctx.config.observe_bases(ctx.cwd) {
        let mut stack = vec![base.path.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() {
                    if !skip_dir(&name) {
                        stack.push(p);
                    }
                } else if name == PROJECT_MARKER {
                    let name = d
                        .file_name()
                        .map_or_else(|| base.key.clone(), |n| n.to_string_lossy().into_owned());
                    if !out.iter().any(|x: &ProjectDir| x.path == d) {
                        out.push(ProjectDir {
                            name,
                            path: d.clone(),
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A project's files, excluded paths dropped and every text redacted (RFC 0043).
pub fn read_dir(root: &Path, redaction: &RedactionConfig) -> Project {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !skip_dir(&name) {
                    stack.push(p);
                }
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if role(&rel) == Role::Other
                || is_excluded_path(&rel, redaction)
                || e.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES
            {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&p) {
                files.push(ProjectFile {
                    path: rel,
                    text: redact(&text, redaction),
                });
            }
        }
    }
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    read_project(&name, &files)
}

/// `lakehouse talend scan`: the projects and their jobs. Reads the source files only.
pub fn scan(ctx: &CommandContext<'_>, as_json: bool) -> anyhow::Result<()> {
    let redaction = ctx.config.redaction_config();
    let rows: Vec<serde_json::Value> = projects(ctx)
        .iter()
        .map(|d| {
            let p = read_dir(&d.path, &redaction);
            json!({
                "project": d.name,
                "path": d.path.display().to_string(),
                "jobs": p.jobs.iter().filter(|j| j.latest).count(),
                "joblets": p.joblets.len(),
            })
        })
        .collect();
    if as_json {
        println!("{}", serde_json::to_string(&rows)?);
    } else if rows.is_empty() {
        println!("no Talend project (a directory holding talend.project) under the sources");
    } else {
        for r in &rows {
            println!(
                "{}: {} jobs, {} joblets ({})",
                r["project"].as_str().unwrap_or(""),
                r["jobs"],
                r["joblets"],
                r["path"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

/// Options of `lakehouse talend translate`.
pub struct TranslateOptions {
    pub project: String,
    /// Write into this directory instead of proposing a branch.
    pub out: Option<PathBuf>,
    /// Plan every enabled model on Trino (when a Trino URL is configured).
    pub explain: bool,
    pub json: bool,
    /// Jobs translated without pattern recognition (`*` for every job).
    pub faithful: std::collections::BTreeSet<String>,
    /// The JVM locale the jobs ran under.
    pub locale: Option<String>,
    /// Let a metered model draft the gaps (RFC 0205); an Ollama route needs no permission.
    pub allow_metered: bool,
}

/// The names of a project's jobs, from its item files (`process/…/<name>_<version>.item`).
fn project_jobs(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut stack = vec![root.join("process")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(stem) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".item"))
                && let Some((name, _version)) = stem.rsplit_once('_')
            {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// Where the latest translation of each project is kept: `<state>/lakehouse/talend/<pkg>.json`.
pub fn state_dir(ctx: &CommandContext<'_>) -> PathBuf {
    ctx.config
        .ekos_dir(ctx.cwd)
        .join("lakehouse")
        .join("talend")
}

/// The report, each model's SQL and the proposal, as one file.
pub fn save_report(
    dir: &Path,
    project: &str,
    where_: &str,
    t: &ekos_lakehouse_talend_dbt::Translation,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let pkg = ekos_lakehouse_etl_dbt::slug(project);
    let sql: std::collections::BTreeMap<&str, &str> = t
        .report
        .models
        .iter()
        .filter_map(|m| t.files.get(&m.path).map(|f| (m.name.as_str(), f.as_str())))
        .collect();
    let doc = json!({
        "project": project,
        "where": where_,
        "report": t.report,
        "sql": sql,
        "proposal": t.files.get(&format!("proposals/talend-{pkg}.md")),
    });
    std::fs::write(
        dir.join(format!("{pkg}.json")),
        serde_json::to_string(&doc)? + "\n",
    )?;
    Ok(())
}

/// The checked drafts and what drafting did, kept beside the report for `verify` and a reviewer:
/// `<state>/lakehouse/talend/<pkg>.drafts.json`.
fn save_drafts(
    dir: &Path,
    project: &str,
    drafts: &std::collections::BTreeMap<(String, String, String), ekos_lakehouse_etl_rel::Draft>,
    outcomes: &[crate::pentaho_cmd::DraftOutcome],
) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let pkg = ekos_lakehouse_etl_dbt::slug(project);
    let list: Vec<serde_json::Value> = drafts
        .iter()
        .map(|((job, component, port), d)| json!({"job": job, "component": component, "port": port, "draft": d}))
        .collect();
    std::fs::write(
        dir.join(format!("{pkg}.drafts.json")),
        serde_json::to_string_pretty(&json!({"drafts": list, "outcomes": outcomes}))? + "\n",
    )?;
    Ok(())
}

/// The drafts `translate` kept for `project`, if any.
fn saved_drafts(
    dir: &Path,
    project: &str,
) -> std::collections::BTreeMap<(String, String, String), ekos_lakehouse_etl_rel::Draft> {
    let pkg = ekos_lakehouse_etl_dbt::slug(project);
    let Ok(text) = std::fs::read_to_string(dir.join(format!("{pkg}.drafts.json"))) else {
        return Default::default();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Default::default();
    };
    v["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let d: ekos_lakehouse_etl_rel::Draft =
                serde_json::from_value(e["draft"].clone()).ok()?;
            Some((
                (
                    e["job"].as_str()?.to_string(),
                    e["component"].as_str()?.to_string(),
                    e["port"].as_str()?.to_string(),
                ),
                d,
            ))
        })
        .collect()
}

/// `lakehouse talend report`: the latest translation of each project (one, with `project`; one
/// component of it, with `component` = `JOB:COMPONENT`). Reads the files `translate` saved.
pub fn report_json(
    dir: &Path,
    project: Option<&str>,
    component: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let mut docs: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter(|p| {
                let n = p.to_string_lossy();
                !n.ends_with(".verification.json") && !n.ends_with(".drafts.json")
            })
            .collect();
        paths.sort();
        for p in paths {
            let text = std::fs::read_to_string(&p)?;
            docs.push(serde_json::from_str(&text)?);
        }
    }
    let summary = |d: &serde_json::Value| {
        let r = &d["report"];
        let models = r["models"].as_array().cloned().unwrap_or_default();
        let count = |f: &dyn Fn(&serde_json::Value) -> bool| models.iter().filter(|m| f(m)).count();
        json!({
            "models": models.len(),
            "enabled": count(&|m| m["enabled"] == true),
            "snapshots": count(&|m| m["layer"] == "snapshot"),
            "exact": count(&|m| m["fidelity"] == "exact"),
            "rewritten": count(&|m| m["fidelity"] == "rewritten"),
            "gap": count(&|m| m["fidelity"] == "gap"),
            "questions": r["questions"].as_array().map_or(0, Vec::len),
            "patterns": r["patterns"].as_array().map_or(0, Vec::len),
            "statements": r["statements"].as_array().map_or(0, Vec::len),
        })
    };
    let projects: Vec<serde_json::Value> = docs
        .iter()
        .map(|d| {
            json!({
                "project": d["project"],
                "translation": "translated",
                "where": d["where"],
                "summary": summary(d),
            })
        })
        .collect();
    let Some(name) = project else {
        return Ok(json!({ "projects": projects }));
    };
    let Some(d) = docs.iter().find(|d| d["project"] == name) else {
        return Ok(json!({ "projects": projects, "state": "not translated" }));
    };
    let r = &d["report"];
    let models: Vec<serde_json::Value> = r["models"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let mut m = m.clone();
            m["why"] = json!(m["gaps"].as_array().and_then(|g| g.first()).cloned());
            m
        })
        .collect();
    let components: Vec<serde_json::Value> = r["components"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let mut c = c.clone();
            c["notes_text"] = json!(c["notes"].as_array().map(|n| {
                n.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            }));
            c
        })
        .collect();
    // The data flow, component by component: an edge from each input.
    let nodes: Vec<serde_json::Value> = components
        .iter()
        .map(|c| json!({ "id": c["id"], "label": c["component"], "group": c["job"], "fidelity": c["fidelity"] }))
        .collect();
    let edges: Vec<serde_json::Value> = components
        .iter()
        .flat_map(|c| {
            c["inputs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| json!({ "from": i, "to": c["id"] }))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut out = json!({
        "projects": projects,
        "state": "translated",
        "graph": { "nodes": nodes, "edges": edges },
        "project": d["project"],
        "where": d["where"],
        "summary": summary(d),
        "models": models,
        "components": components,
        "patterns": r["patterns"],
        "questions": r["questions"],
        "statements": r["statements"],
        "proposal": d["proposal"],
        // The latest differential run's comparison, when there is one (RFC 0268 phase 3).
        "verification": verification_of(dir, name),
    });
    if let Some(id) = component {
        let Some(c) = components.iter().find(|c| c["id"] == id) else {
            anyhow::bail!("`{name}` has no component `{id}` (as JOB:COMPONENT)");
        };
        let sql: Vec<String> = c["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let m = m.as_str()?;
                Some(format!("-- {m}\n{}", d["sql"][m].as_str()?))
            })
            .collect();
        let mut c = c.clone();
        c["sql"] = json!(sql.join("\n\n"));
        out["component"] = c;
    }
    Ok(out)
}

/// Options of `lakehouse talend verify`.
pub struct VerifyOptions {
    pub project: String,
    /// The runner's output directory (`scripts/talend-sandbox/run.sh`).
    pub run: PathBuf,
    /// A Trino used only for this: its `memory` catalog holds the landed files and the models.
    pub trino_url: String,
    pub faithful: std::collections::BTreeSet<String>,
    pub locale: Option<String>,
    pub json: bool,
}

/// A value from Trino as text (`None` for null).
fn trino_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn sql_str(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// `name='value'` in a model's config line.
fn config_value(sql: &str, name: &str) -> Option<String> {
    let at = sql.find(&format!("{name}="))? + name.len() + 1;
    let rest = &sql[at..];
    if let Some(r) = rest.strip_prefix('\'') {
        return Some(r[..r.find('\'')?].to_string());
    }
    let r = rest.strip_prefix('[')?;
    Some(r[..r.find(']')?].to_string())
}

/// `lakehouse talend verify <project> --run <dir>`: EKOS RFC 0268 phase 3's comparison. The models'
/// first build, on the same input as the sandbox run, against the tables the jobs wrote.
pub fn verify(
    settings: &Settings,
    ctx: &CommandContext<'_>,
    o: &VerifyOptions,
) -> anyhow::Result<()> {
    use ekos_lakehouse_talend_dbt::verify as v;
    let all = projects(ctx);
    let Some(dir) = all.iter().find(|p| p.name == o.project) else {
        anyhow::bail!("no Talend project `{}`", o.project);
    };
    let out_dir = o.run.join("out");
    let manifest: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(out_dir.join("manifest.json"))?.trim_start_matches('\u{feff}'),
    )?;
    let plan_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.run.join("plan.json"))?)?;
    let locale = match o.locale.as_deref() {
        None => None,
        Some(tag) => Some(
            ekos_lakehouse_talend_dbt::Locale::parse(tag)
                .ok_or_else(|| anyhow::anyhow!("unknown locale `{tag}`"))?,
        ),
    };
    let project = read_dir(&dir.path, &ctx.config.redaction_config());
    let s = ekos_lakehouse_talend_dbt::Settings {
        project: o.project.clone(),
        source_database: settings.source_database.clone(),
        faithful: o.faithful.clone(),
        locale,
        // The drafts `translate` kept, so what is verified is what it produced.
        drafts: saved_drafts(&state_dir(ctx), &o.project),
    };
    // An empty plan: a model reading a lakehouse table is not verified, by name.
    let plan = ekos_lakehouse_bronze::plan::plan(&[], &Default::default());
    let options = ekos_lakehouse_talend_rel::Options {
        locale,
        drafts: s.drafts.clone(),
    };
    let (t, specs) = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(scope, || {
                (
                    ekos_lakehouse_talend_dbt::translate(&project, &plan, &s),
                    v::file_specs(&project, &options),
                )
            })
            .map(|h| h.join())
    })
    .map_err(|e| anyhow::anyhow!("cannot start the translation: {e}"))?
    .map_err(|_| anyhow::anyhow!("the translation failed"))?;

    let client = ekos_lakehouse_trino::Client::new(ekos_lakehouse_trino::Connection {
        base_url: o.trino_url.clone(),
        user: "ekos".into(),
        catalog: None,
        schema: None,
    })?;
    let id = &sha256(&format!("{}{:?}", o.project, std::time::SystemTime::now()))[..8];
    let schema = |s: &str| format!("\"memory\".\"v{id}_{s}\"");
    let exec = |sql: &str| client.execute(sql).map_err(|e| e.to_string());
    let mut created: Vec<String> = Vec::new();
    for sc in [
        "talend_files",
        "silver_talend",
        "gold_talend",
        "snapshots_talend",
    ] {
        exec(&format!("CREATE SCHEMA {}", schema(sc)))
            .map_err(|e| anyhow::anyhow!("Trino: {e}"))?;
        created.push(schema(sc));
    }
    let prefix = format!("v{id}_");
    let result = verify_in(
        &t, &specs, &manifest, &plan_doc, &o.run, &out_dir, &plan, &exec, &schema, &prefix,
    );
    // Everything this verification made goes, whatever happened.
    for sc in &created {
        if let Ok(r) = client.execute(&format!("SHOW TABLES FROM {sc}")) {
            for row in r.rows {
                if let Some(tname) = row.first().and_then(|x| x.as_str()) {
                    let _ = client.execute(&format!("DROP TABLE {sc}.\"{tname}\""));
                }
            }
        }
        let _ = client.execute(&format!("DROP SCHEMA {sc}"));
    }
    let report = result?;
    let pkg = ekos_lakehouse_etl_dbt::slug(&o.project);
    let dir_state = state_dir(ctx);
    std::fs::create_dir_all(&dir_state)?;
    std::fs::write(
        dir_state.join(format!("{pkg}.verification.json")),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    if o.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for c in report["models"].as_array().into_iter().flatten() {
            println!(
                "{:<12} {} → {}{}",
                c["result"].as_str().unwrap_or_default(),
                c["model"].as_str().unwrap_or_default(),
                c["table"].as_str().unwrap_or_default(),
                c["reason"]
                    .as_str()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            );
        }
        println!("run 1: {}", report["summary"].as_str().unwrap_or_default());
        if let Some(second) = report["second_run"].as_object() {
            for c in second["models"].as_array().into_iter().flatten() {
                if c["result"] == "not_verified" {
                    continue;
                }
                println!(
                    "run 2: {:<12} {} → {}",
                    c["result"].as_str().unwrap_or_default(),
                    c["model"].as_str().unwrap_or_default(),
                    c["table"].as_str().unwrap_or_default()
                );
            }
            println!("run 2: {}", second["summary"].as_str().unwrap_or_default());
        }
    }
    Ok(())
}

/// A table as read: its header and rows.
type Table = (Vec<String>, Vec<Vec<Option<String>>>);

/// The model the comparer's control plants a difference in: its columns, and both sides.
type Control = (Vec<String>, Table, Table);

/// Where each model is built: `(schema, table)` by model name.
type Locations = std::collections::BTreeMap<String, (String, String)>;

/// What every step of a verification shares.
struct Verify<'a> {
    t: &'a ekos_lakehouse_talend_dbt::Translation,
    specs: &'a [ekos_lakehouse_talend_dbt::verify::FileSpec],
    plan: &'a ekos_lakehouse_bronze::plan::Plan,
    exec: &'a dyn Fn(&str) -> Result<ekos_lakehouse_trino::QueryResult, String>,
    schema: &'a dyn Fn(&str) -> String,
    prefix: &'a str,
}

fn refs_in(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = sql;
    while let Some(i) = rest.find("{{ ref('") {
        let after = &rest[i + 8..];
        let Some(j) = after.find("') }}") else { break };
        out.push(after[..j].to_string());
        rest = &after[j..];
    }
    out
}

/// A model's incremental run: each inline `{% if is_incremental() %}A{% else %}B{% endif %}` is A,
/// and `{{ this }}` is the model's table from the run before (the ref `__this__`).
fn incremental_branch(sql: &str) -> String {
    let mut out = String::new();
    let mut rest = sql;
    let (open, els, end) = ("{% if is_incremental() %}", "{% else %}", "{% endif %}");
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i + open.len()..];
        let (Some(e), Some(f)) = (after.find(els), after.find(end)) else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        out.push_str(&after[..e.min(f)]);
        rest = &after[f + end.len()..];
    }
    out.push_str(rest);
    out.replace("{{ this }}", "{{ ref('__this__') }}")
}

impl Verify<'_> {
    fn sql_with(&self, sql: &str, locations: &Locations) -> String {
        ekos_lakehouse_etl_dbt::engine_sql(
            &ekos_lakehouse_talend_dbt::gates::sources_as_tables(sql),
            locations,
            self.plan,
        )
        .replace(
            "\"talend_files\".\"",
            &format!("{}.\"", (self.schema)("talend_files")),
        )
        .replace("\"iceberg\".\"", &format!("\"memory\".\"{}", self.prefix))
    }

    fn table(&self, loc: &(String, String)) -> String {
        format!("{}.{}", (self.schema)(&loc.0), q(&loc.1))
    }

    fn run(&self, sql: &str) -> Result<ekos_lakehouse_trino::QueryResult, String> {
        (self.exec)(sql)
    }

    /// Lands each file source from `inputs` (`(host copy, the VM directory it was in)`), replacing
    /// what an earlier run landed.
    fn land(
        &self,
        inputs: &[(PathBuf, String)],
        replace: bool,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        use ekos_lakehouse_talend_dbt::verify as v;
        let mut landed = Vec::new();
        for (name, file) in &self.t.report.files {
            let Some(spec) = self.specs.iter().find(|s| &s.file == file) else {
                landed.push(json!({"source": name, "landed": false, "reason": "no file component reads it"}));
                continue;
            };
            let mut rows = Vec::new();
            let mut matched = Vec::new();
            for (from, to) in inputs {
                let candidates: Vec<PathBuf> = if from.is_dir() {
                    std::fs::read_dir(from)?
                        .flatten()
                        .map(|e| e.path())
                        .collect()
                } else {
                    vec![from.clone()]
                };
                for f in candidates {
                    let fname = f
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                        .to_string();
                    let hit = if file.contains('*') || file.contains('?') {
                        v::mask_matches(file, &fname)
                    } else {
                        file.rsplit(['/', '\\']).next() == Some(fname.as_str())
                    };
                    if !hit {
                        continue;
                    }
                    // `$path` as the job saw it: the VM's absolute path (Java's `getAbsolutePath`).
                    let vm_path = format!("{to}\\{fname}");
                    rows.extend(
                        v::parse_file(&std::fs::read(&f)?, spec, &vm_path)
                            .map_err(|e| anyhow::anyhow!(e))?,
                    );
                    matched.push(fname);
                }
            }
            let target = format!("{}.{}", (self.schema)("talend_files"), q(name));
            if replace {
                let _ = self.run(&format!("DROP TABLE IF EXISTS {target}"));
            }
            let cols: Vec<String> = spec
                .columns
                .iter()
                .map(|(c, ty)| format!("{} {}", q(c), trino_type_name(*ty)))
                .collect();
            self.run(&format!("CREATE TABLE {target} ({})", cols.join(", ")))
                .map_err(|e| anyhow::anyhow!("Trino: {e}"))?;
            for chunk in rows.chunks(200) {
                let values: Vec<String> = chunk
                    .iter()
                    .map(|r| {
                        let vals: Vec<String> = r
                            .iter()
                            .zip(&spec.columns)
                            .map(|(x, (_, ty))| match x {
                                None => format!("CAST(NULL AS {})", trino_type_name(*ty)),
                                Some(s) => format!(
                                    "CAST({} AS {})",
                                    sql_str(s.trim_end()),
                                    trino_type_name(*ty)
                                ),
                            })
                            .collect();
                        format!("({})", vals.join(", "))
                    })
                    .collect();
                self.run(&format!(
                    "INSERT INTO {target} VALUES {}",
                    values.join(", ")
                ))
                .map_err(|e| anyhow::anyhow!("Trino, landing {name}: {e}"))?;
            }
            landed.push(json!({"source": name, "file": file, "component": format!("{}:{}", spec.job, spec.component), "files": matched, "rows": rows.len(), "landed": true}));
        }
        Ok(landed)
    }

    /// Builds every enabled model in ref order with `build` (model, its SQL); a model reading one that
    /// did not build does not build either.
    fn build_all(
        &self,
        build: &dyn Fn(&ekos_lakehouse_talend_dbt::ModelReport, &str) -> Result<(), String>,
    ) -> (
        std::collections::BTreeSet<String>,
        std::collections::BTreeMap<String, String>,
    ) {
        let mut built = std::collections::BTreeSet::new();
        let mut failed = std::collections::BTreeMap::new();
        let pending: Vec<&ekos_lakehouse_talend_dbt::ModelReport> =
            self.t.report.models.iter().filter(|m| m.enabled).collect();
        loop {
            let mut progress = false;
            for m in &pending {
                if built.contains(&m.name) || failed.contains_key(&m.name) {
                    continue;
                }
                let sql = self.t.files.get(&m.path).cloned().unwrap_or_default();
                let deps = refs_in(&sql);
                if let Some(d) = deps.iter().find(|d| failed.contains_key(*d)) {
                    failed.insert(m.name.clone(), format!("reads `{d}`, which did not build"));
                    progress = true;
                    continue;
                }
                if !deps.iter().all(|d| built.contains(d)) {
                    continue;
                }
                match build(m, &sql) {
                    Ok(()) => {
                        built.insert(m.name.clone());
                    }
                    Err(e) => {
                        failed.insert(m.name.clone(), format!("Trino: {e}"));
                    }
                }
                progress = true;
            }
            if !progress {
                break;
            }
        }
        (built, failed)
    }

    /// Compares each gold model that writes a table with the table the jobs left in `dumps`.
    fn compare(
        &self,
        locations: &Locations,
        failed: &std::collections::BTreeMap<String, String>,
        dumps: &[PathBuf],
        jobs_ran: &std::collections::BTreeMap<String, bool>,
        job_errors: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> anyhow::Result<(
        Vec<ekos_lakehouse_talend_dbt::verify::ModelCheck>,
        Option<Control>,
    )> {
        use ekos_lakehouse_talend_dbt::Layer;
        use ekos_lakehouse_talend_dbt::verify as v;
        let surrogates: std::collections::BTreeSet<String> = self
            .t
            .report
            .models
            .iter()
            .filter_map(|m| m.surrogate.as_ref().map(|s| s.to_lowercase()))
            .collect();
        let mut checks = Vec::new();
        let mut control_rows = None;
        for m in self
            .t
            .report
            .models
            .iter()
            .filter(|m| m.layer == Layer::Gold && m.target.is_some())
        {
            let table = m.target.clone().unwrap_or_default();
            let not = |reason: String| v::ModelCheck {
                model: m.name.clone(),
                table: table.clone(),
                columns: vec![],
                excluded: vec![],
                outcome: v::Outcome::NotVerified { reason },
                job_errors: vec![],
            };
            if !m.enabled {
                checks.push(not(format!(
                    "disabled: {}",
                    m.gaps.first().cloned().unwrap_or_default()
                )));
                continue;
            }
            if let Some(why) = failed.get(&m.name) {
                checks.push(not(why.clone()));
                continue;
            }
            let jobs: Vec<&str> = m.job.split(", ").collect();
            if let Some(j) = jobs
                .iter()
                .find(|j| !jobs_ran.get(**j).copied().unwrap_or(false))
            {
                checks.push(not(format!(
                    "the job {j} did not run to its end in the sandbox"
                )));
                continue;
            }
            let Some(dump) = dumps.iter().find(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n.to_lowercase()
                        .ends_with(&format!(".{}.csv", table.to_lowercase()))
                })
            }) else {
                checks.push(not(format!("the run dumped no table `{table}`")));
                continue;
            };
            let (th, trows) = v::parse_pg_csv(&std::fs::read_to_string(dump)?);
            let r = self
                .run(&format!(
                    "SELECT * FROM {}",
                    self.table(&locations[&m.name])
                ))
                .map_err(|e| anyhow::anyhow!("Trino: {e}"))?;
            let dh: Vec<String> = r.columns.iter().map(|c| c.name.clone()).collect();
            let drows: Vec<Vec<Option<String>>> = r
                .rows
                .iter()
                .map(|row| row.iter().map(trino_text).collect())
                .collect();
            let (cols, excluded) = v::columns_to_compare(&th, &dh, &m.clock, &surrogates);
            let outcome = v::compare(&cols, (&th, &trows), (&dh, &drows))
                .unwrap_or_else(|e| v::Outcome::NotVerified { reason: e });
            if control_rows.is_none() && matches!(outcome, v::Outcome::Equal { rows } if rows > 0) {
                control_rows = Some((
                    cols.clone(),
                    (th.clone(), trows.clone()),
                    (dh.clone(), drows.clone()),
                ));
            }
            let job_errors: Vec<String> = jobs
                .iter()
                .flat_map(|j| job_errors.get(*j).cloned().unwrap_or_default())
                .collect();
            checks.push(v::ModelCheck {
                model: m.name.clone(),
                table,
                columns: cols,
                excluded,
                outcome,
                job_errors,
            });
        }
        Ok((checks, control_rows))
    }
}

/// The run's dumps after its last job: the latest step directory of run `run` (`r1_…`), or of
/// any step for a run made before runs were named.
fn last_dumps(out_dir: &Path, run: &str) -> anyhow::Result<Option<Vec<PathBuf>>> {
    let steps: Vec<PathBuf> = std::fs::read_dir(out_dir.join("dumps"))?
        .flatten()
        .map(|e| e.path())
        .collect();
    let named = steps.iter().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("r1_"))
    });
    let last = steps
        .iter()
        .filter(|p| {
            !named
                || p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&format!("{run}_")))
        })
        .max();
    Ok(match last {
        Some(d) => Some(std::fs::read_dir(d)?.flatten().map(|e| e.path()).collect()),
        None => None,
    })
}

fn checks_summary(
    checks: &[ekos_lakehouse_talend_dbt::verify::ModelCheck],
    built: usize,
    failed: usize,
) -> String {
    let count = |r: &str| {
        checks
            .iter()
            .filter(|c| {
                serde_json::to_value(&c.outcome)
                    .ok()
                    .and_then(|v| v["result"].as_str().map(|s| s == r))
                    .unwrap_or(false)
            })
            .count()
    };
    format!(
        "{} equal, {} differ, {} not verified; {built} models built, {failed} failed to build",
        count("equal"),
        count("differs"),
        count("not_verified")
    )
}

/// Lands, builds and compares, for the first run and, when the run has one, the second; returns
/// the report.
#[allow(clippy::too_many_arguments)]
fn verify_in(
    t: &ekos_lakehouse_talend_dbt::Translation,
    specs: &[ekos_lakehouse_talend_dbt::verify::FileSpec],
    manifest: &serde_json::Value,
    plan_doc: &serde_json::Value,
    run: &Path,
    out_dir: &Path,
    plan: &ekos_lakehouse_bronze::plan::Plan,
    exec: &dyn Fn(&str) -> Result<ekos_lakehouse_trino::QueryResult, String>,
    schema: &dyn Fn(&str) -> String,
    prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    use ekos_lakehouse_talend_dbt::Layer;
    use ekos_lakehouse_talend_dbt::verify as v;
    let ctx = Verify {
        t,
        specs,
        plan,
        exec,
        schema,
        prefix,
    };
    let inputs_of = |dir: &str| -> Vec<(PathBuf, String)> {
        plan_doc["inputs"]
            .as_array()
            .into_iter()
            .flatten()
            .zip(
                manifest["sandbox_settings"]["inputs"]
                    .as_array()
                    .into_iter()
                    .flatten(),
            )
            .map(|(p, m)| {
                (
                    run.join(dir).join(p["from"].as_str().unwrap_or_default()),
                    m["to"].as_str().unwrap_or_default().replace('/', "\\"),
                )
            })
            .collect()
    };
    let jobs_ran = |name: &str| -> std::collections::BTreeMap<String, bool> {
        manifest["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|j| j["run"].as_str().is_none_or(|r| r == name))
            .map(|j| {
                (
                    j["job"].as_str().unwrap_or_default().to_string(),
                    j["exit"] == json!(0),
                )
            })
            .collect()
    };
    // The lines a job wrote to its error output on a run (the runner keeps the first three).
    let job_errors = |name: &str| -> std::collections::BTreeMap<String, Vec<String>> {
        manifest["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|j| j["run"].as_str().is_none_or(|r| r == name))
            .map(|j| {
                (
                    j["job"].as_str().unwrap_or_default().to_string(),
                    j["exception"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.as_str().map(|t| t.chars().take(400).collect()))
                        .collect(),
                )
            })
            .collect()
    };
    let alias_of = |m: &ekos_lakehouse_talend_dbt::ModelReport| -> (String, String) {
        let sql = t.files.get(&m.path).map(String::as_str).unwrap_or_default();
        match m.layer {
            Layer::Silver => (
                "silver_talend".into(),
                config_value(sql, "alias").unwrap_or(m.name.clone()),
            ),
            Layer::Gold => (
                "gold_talend".into(),
                config_value(sql, "alias").unwrap_or(m.name.clone()),
            ),
            Layer::Snapshot => ("snapshots_talend".into(), m.name.clone()),
        }
    };
    let locations: Locations = t
        .report
        .models
        .iter()
        .map(|m| (m.name.clone(), alias_of(m)))
        .collect();

    // Run 1: each model's first build.
    let landed = ctx.land(&inputs_of("inputs"), false)?;
    let (built, failed) = ctx.build_all(&|m, sql| {
        let body = ctx.sql_with(sql, &locations);
        let body = if m.layer == Layer::Snapshot {
            // dbt's first version: from now, open, with a version id over the key and its start.
            let key: Vec<String> = config_value(sql, "unique_key")
                .unwrap_or_default()
                .split(',')
                .map(|k| format!("CAST(b.{} AS varchar)", q(k.trim().trim_matches('\''))))
                .collect();
            format!(
                "SELECT b.*, CAST(current_timestamp AS timestamp(6)) AS dbt_valid_from, CAST(NULL AS timestamp(6)) AS dbt_valid_to, to_hex(md5(to_utf8(concat_ws('|', {}, CAST(current_timestamp AS varchar))))) AS dbt_scd_id FROM (\n{body}\n) AS b",
                key.join(", ")
            )
        } else {
            body
        };
        ctx.run(&format!("CREATE TABLE {} AS {body}", ctx.table(&locations[&m.name]))).map(|_| ())
    });
    let dumps =
        last_dumps(out_dir, "r1")?.ok_or_else(|| anyhow::anyhow!("the run has no dumps"))?;
    let (checks, control_rows) = ctx.compare(
        &locations,
        &failed,
        &dumps,
        &jobs_ran("r1"),
        &job_errors("r1"),
    )?;
    // The comparer's control: one dbt row removed must be found.
    let control = match &control_rows {
        Some((cols, (th, trows), (dh, drows))) => {
            let found = matches!(
                v::compare(cols, (th, trows), (dh, &drows[1..])),
                Ok(v::Outcome::Differs {
                    only_talend_count: 1,
                    ..
                })
            );
            json!({"ran": true, "found_the_planted_difference": found})
        }
        None => {
            json!({"ran": false, "why": "no model compared equal with rows to plant a difference in"})
        }
    };
    let control_word = if control["found_the_planted_difference"] == json!(true) {
        "passed"
    } else if control["ran"] == json!(true) {
        "FAILED: the result is not trusted"
    } else {
        "not run"
    };
    let summary = format!(
        "{}; comparer control: {control_word}",
        checks_summary(&checks, built.len(), failed.len())
    );

    // Run 2: the jobs again on the tables run 1 left, the models as dbt's second run builds them.
    let second = if run.join("inputs-r2").is_dir() {
        Some(second_run(
            &ctx,
            &locations,
            &built,
            &inputs_of("inputs-r2"),
            out_dir,
            &jobs_ran("r2"),
            &job_errors("r2"),
        )?)
    } else {
        None
    };
    Ok(json!({
        "project": t.report.project,
        "run": run.display().to_string(),
        "run_day": manifest["run_day"],
        "scope": if second.is_some() {
            "run 1 from empty tables against each model's first build; run 2 on the tables run 1 left against each model's second (incremental) build"
        } else {
            "one Talend run from empty tables against each model's first build; incremental runs are not compared"
        },
        "landed": landed,
        "build_failures": failed,
        "models": checks,
        "control": control,
        "summary": summary,
        "second_run": second,
    }))
}

/// Run 2: re-land the run-2 inputs, build each model as dbt's incremental run would on run 1's
/// tables, and compare with the tables after the jobs' second run. Every table is new (`…__r2`):
/// Trino's memory catalog has no MERGE.
fn second_run(
    ctx: &Verify<'_>,
    first: &Locations,
    built_first: &std::collections::BTreeSet<String>,
    inputs: &[(PathBuf, String)],
    out_dir: &Path,
    jobs_ran: &std::collections::BTreeMap<String, bool>,
    job_errors: &std::collections::BTreeMap<String, Vec<String>>,
) -> anyhow::Result<serde_json::Value> {
    use ekos_lakehouse_talend_dbt::Layer;
    let landed = ctx.land(inputs, true)?;
    let second: Locations = first
        .iter()
        .map(|(k, (s, a))| (k.clone(), (s.clone(), format!("{a}__r2"))))
        .collect();
    let (built, mut failed) = ctx.build_all(&|m, sql| {
        if !built_first.contains(&m.name) {
            return Err("it did not build on run 1".into());
        }
        let old = ctx.table(&first[&m.name]);
        let new = ctx.table(&second[&m.name]);
        let mut locs = second.clone();
        locs.insert("__this__".into(), first[&m.name].clone());
        let keys = || -> Vec<String> {
            config_value(sql, "unique_key")
                .unwrap_or_default()
                .split(',')
                .map(|k| k.trim().trim_matches('\'').to_string())
                .filter(|k| !k.is_empty())
                .collect()
        };
        let same = |a: &str, b: &str| {
            keys().iter().map(|k| format!("{a}.{} = {b}.{}", q(k), q(k))).collect::<Vec<_>>().join(" AND ")
        };
        if m.layer == Layer::Snapshot {
            // dbt's check strategy: close the current version of a key whose compared columns changed,
            // insert the new versions and new keys; keys gone from the source are left as they are.
            let src = format!("{}.{}", (ctx.schema)(&first[&m.name].0), q(&format!("{}__src", first[&m.name].1)));
            ctx.run(&format!("CREATE TABLE {src} AS {}", ctx.sql_with(sql, &locs)))?;
            let cols: Vec<String> = ctx
                .run(&format!("SELECT * FROM {src} WHERE false"))?
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect();
            let check: Vec<String> = config_value(sql, "check_cols")
                .unwrap_or_default()
                .split(',')
                .map(|k| k.trim().trim_matches('\'').to_string())
                .filter(|k| !k.is_empty())
                .collect();
            let changed = |a: &str, b: &str| {
                check.iter().map(|c| format!("{a}.{} IS DISTINCT FROM {b}.{}", q(c), q(c))).collect::<Vec<_>>().join(" OR ")
            };
            let key_text: Vec<String> = keys().iter().map(|k| format!("CAST(s.{} AS varchar)", q(k))).collect();
            let o_cols: Vec<String> = cols.iter().map(|c| format!("o.{}", q(c))).collect();
            let s_cols: Vec<String> = cols.iter().map(|c| format!("s.{}", q(c))).collect();
            let sql2 = format!(
                "SELECT {oc}, o.dbt_valid_from, CASE WHEN o.dbt_valid_to IS NULL AND EXISTS (SELECT 1 FROM {src} s WHERE {k} AND ({ch})) THEN CAST(current_timestamp AS timestamp(6)) ELSE o.dbt_valid_to END AS dbt_valid_to, o.dbt_scd_id FROM {old} o \
                 UNION ALL \
                 SELECT {sc}, CAST(current_timestamp AS timestamp(6)), CAST(NULL AS timestamp(6)), to_hex(md5(to_utf8(concat_ws('|', {kt}, CAST(current_timestamp AS varchar))))) FROM {src} s \
                 WHERE NOT EXISTS (SELECT 1 FROM {old} o WHERE o.dbt_valid_to IS NULL AND {k}) \
                    OR EXISTS (SELECT 1 FROM {old} o WHERE o.dbt_valid_to IS NULL AND {k} AND ({ch}))",
                oc = o_cols.join(", "),
                sc = s_cols.join(", "),
                k = same("o", "s"),
                ch = changed("o", "s"),
                kt = key_text.join(", "),
            );
            return ctx.run(&format!("CREATE TABLE {new} AS {sql2}")).map(|_| ());
        }
        let materialization = m.materialization.as_str();
        if !materialization.starts_with("incremental") {
            // A table or a view: built again.
            return ctx.run(&format!("CREATE TABLE {new} AS {}", ctx.sql_with(sql, &locs))).map(|_| ());
        }
        let inc = ctx.sql_with(&incremental_branch(sql), &locs);
        if materialization.contains("append") {
            return ctx
                .run(&format!("CREATE TABLE {new} AS SELECT * FROM {old} UNION ALL SELECT * FROM (\n{inc}\n) AS n"))
                .map(|_| ());
        }
        // A merge: the incremental rows replace the rows with their keys.
        let tmp = format!("{}.{}", (ctx.schema)(&second[&m.name].0), q(&format!("{}__inc", first[&m.name].1)));
        ctx.run(&format!("CREATE TABLE {tmp} AS {inc}"))?;
        ctx.run(&format!(
            "CREATE TABLE {new} AS SELECT * FROM {old} o WHERE NOT EXISTS (SELECT 1 FROM {tmp} n WHERE {}) UNION ALL SELECT * FROM {tmp}",
            same("o", "n")
        ))
        .map(|_| ())
    });
    for m in ctx
        .t
        .report
        .models
        .iter()
        .filter(|m| m.enabled && !built.contains(&m.name))
    {
        failed
            .entry(m.name.clone())
            .or_insert_with(|| "not built".into());
    }
    let dumps =
        last_dumps(out_dir, "r2")?.ok_or_else(|| anyhow::anyhow!("the run has no run-2 dumps"))?;
    let (checks, _) = ctx.compare(&second, &failed, &dumps, jobs_ran, job_errors)?;
    let summary = checks_summary(&checks, built.len(), failed.len());
    Ok(json!({ "landed": landed, "build_failures": failed, "models": checks, "summary": summary }))
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn trino_type_name(t: ekos_lakehouse_etl_expr::KType) -> &'static str {
    use ekos_lakehouse_etl_expr::KType;
    match t {
        KType::Integer => "bigint",
        KType::Number => "double",
        KType::BigNumber => "decimal(38,10)",
        KType::Timestamp => "timestamp(6)",
        KType::Date => "date",
        KType::Boolean => "boolean",
        _ => "varchar",
    }
}

/// The saved verification of `project`, flattened for a table: one row per model.
fn verification_of(dir: &Path, project: &str) -> serde_json::Value {
    let pkg = ekos_lakehouse_etl_dbt::slug(project);
    let Ok(text) = std::fs::read_to_string(dir.join(format!("{pkg}.verification.json"))) else {
        return serde_json::Value::Null;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return serde_json::Value::Null;
    };
    let rows: Vec<serde_json::Value> = v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            json!({
                "model": m["model"],
                "table": m["table"],
                "result": m["result"],
                "rows": m["rows"].as_u64().or(m["talend_rows"].as_u64()),
                "only_talend": m["only_talend_count"],
                "only_dbt": m["only_dbt_count"],
                "excluded": m["excluded"].as_array().map(|x| x.iter().filter_map(|e| e[0].as_str()).collect::<Vec<_>>().join(", ")),
                "reason": m["reason"],
            })
        })
        .collect();
    json!({
        "summary": v["summary"],
        "run": v["run"],
        "run_day": v["run_day"],
        "scope": v["scope"],
        "control": v["control"],
        "models": rows,
    })
}

/// `lakehouse talend translate <project>`.
pub async fn translate(
    settings: &Settings,
    ctx: &CommandContext<'_>,
    o: &TranslateOptions,
    repo: &Path,
) -> anyhow::Result<()> {
    let all = projects(ctx);
    let Some(dir) = all.iter().find(|p| p.name == o.project) else {
        anyhow::bail!(
            "no Talend project `{}` (have: {})",
            o.project,
            all.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let plan = read_lock(repo).map_err(|e| {
        anyhow::anyhow!(
            "{e}: run `lakehouse generate` first — Talend tables resolve to the deployed silver models"
        )
    })?;
    let locale = match o.locale.as_deref() {
        None => None,
        Some(tag) => Some(
            ekos_lakehouse_talend_dbt::Locale::parse(tag).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown locale `{tag}` (known: {})",
                    ekos_lakehouse_talend_dbt::Locale::KNOWN.join(", ")
                )
            })?,
        ),
    };
    let unknown: Vec<&String> = o
        .faithful
        .iter()
        .filter(|j| *j != "*" && !project_jobs(&dir.path).contains(*j))
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!("--faithful names no job of `{}`: {unknown:?}", o.project);
    }
    let project = read_dir(&dir.path, &ctx.config.redaction_config());
    let mut s = ekos_lakehouse_talend_dbt::Settings {
        project: o.project.clone(),
        source_database: settings.source_database.clone(),
        faithful: o.faithful.clone(),
        locale,
        drafts: Default::default(),
    };
    // A long Java concatenation is a deep SQL expression: the parser needs more stack than an
    // async worker thread has.
    let run = |s: &ekos_lakehouse_talend_dbt::Settings| {
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(scope, || {
                    let options = ekos_lakehouse_talend_rel::Options {
                        locale: s.locale,
                        drafts: s.drafts.clone(),
                    };
                    (
                        ekos_lakehouse_talend_dbt::translate(&project, &plan, s),
                        ekos_lakehouse_talend_dbt::draft::requests(&project, &options),
                    )
                })
                .map(|h| h.join())
        })
        .map_err(|e| anyhow::anyhow!("cannot start the translation: {e}"))?
        .map_err(|_| anyhow::anyhow!("the translation failed"))
    };
    let (mut t, requests) = run(&s)?;
    // RFC 0205 for Talend (RFC 0268 phase 3): the gaps a model may draft, by the configured
    // `translate` route. An Ollama route needs no permission; any other needs --allow-metered.
    let mut draft_outcomes = Vec::new();
    if !requests.is_empty() {
        let router = ekos_recovery::LlmRouter::new(&ctx.config.llm, &ctx.config.ekos_dir(ctx.cwd));
        let route = router.route(ekos_recovery::Task::Translate);
        if route.provider != "ollama" && !o.allow_metered {
            println!(
                "{} gaps could be drafted by {} ({}, metered): rerun with --allow-metered",
                requests.len(),
                route.models.join(" → "),
                route.provider
            );
        } else {
            let provider = router
                .provider(ekos_recovery::Task::Translate)
                .map_err(|e| anyhow::anyhow!("the translate model: {e}"))?;
            let (drafts, outcomes) = crate::pentaho_cmd::draft_all_with(
                &requests,
                provider.as_ref(),
                3,
                &ekos_lakehouse_talend_dbt::draft::prompt,
                ekos_lakehouse_talend_dbt::draft::PROMPT_VERSION,
            )
            .await;
            println!(
                "{} of {} gaps drafted by {}",
                drafts.len(),
                requests.len(),
                provider.model_name()
            );
            s.drafts = drafts
                .into_iter()
                .map(
                    |((job, step), d): ((String, String), ekos_lakehouse_etl_rel::Draft)| {
                        (ekos_lakehouse_talend_dbt::draft::key_of(&job, &step), d)
                    },
                )
                .collect();
            draft_outcomes = outcomes;
            t = run(&s)?.0;
        }
    }
    let pkg = ekos_lakehouse_etl_dbt::slug(&o.project);
    // Gate 4: Trino plans every enabled model; one it refuses is disabled with its message. A
    // model reading a landed file is not planned (where the file lands is a question).
    let mut explained = 0usize;
    let mut not_planned = 0usize;
    if o.explain && settings.trino_url.is_some() {
        let names: Vec<String> = t
            .report
            .models
            .iter()
            .filter(|m| m.enabled)
            .map(|m| m.name.clone())
            .collect();
        let mut checks = Vec::new();
        for n in &names {
            let path = &t
                .report
                .models
                .iter()
                .find(|m| &m.name == n)
                .expect("listed")
                .path;
            if t.files[path].contains("{{ source('talend_files'") {
                not_planned += 1;
                continue;
            }
            checks.push((
                n.clone(),
                ekos_lakehouse_etl_dbt::plannable_sql(n, &t.files, &plan),
            ));
        }
        let conn = settings.trino_connection()?;
        let results = tokio::task::spawn_blocking(move || {
            let client = ekos_lakehouse_trino::Client::new(conn).map_err(|e| e.to_string())?;
            Ok::<_, String>(
                checks
                    .into_iter()
                    .map(|(n, sql)| {
                        let r = sql.and_then(|s| {
                            client
                                .execute(&format!("EXPLAIN {s}"))
                                .map(|_| ())
                                .map_err(|e| e.to_string())
                        });
                        (n, r)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .await?
        .map_err(|e| anyhow::anyhow!("Trino: {e}"))?;
        let mut refused = false;
        for (n, r) in results {
            match r {
                Ok(()) => explained += 1,
                Err(e) => {
                    refused = true;
                    ekos_lakehouse_talend_dbt::disable_model(
                        &mut t,
                        &n,
                        &format!("Trino cannot plan it: {e}"),
                    );
                }
            }
        }
        if refused {
            ekos_lakehouse_talend_dbt::rewrite_report(&mut t);
        }
    }
    let (where_, outcome) = match &o.out {
        Some(out) => {
            let w = write_translation_of(out, "talend", &pkg, &t.files)?;
            (out.display().to_string(), w)
        }
        None => propose(repo, &pkg, &t)?,
    };
    // The latest translation, for `talend report` and the console, before anyone merges it.
    save_report(&state_dir(ctx), &o.project, &where_, &t)?;
    save_drafts(&state_dir(ctx), &o.project, &s.drafts, &draft_outcomes)?;
    let enabled = t.report.models.iter().filter(|m| m.enabled).count();
    if o.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "project": o.project,
                "where": where_,
                "jobs": t.report.jobs.len(),
                "models": t.report.models.len(),
                "enabled": enabled,
                "questions": t.report.questions.len(),
                "statements": t.report.statements.len(),
                "watermarks": t.report.watermarks.len(),
                "explained": explained,
                "not_planned": not_planned,
                "write": outcome,
            }))?
        );
    } else {
        println!(
            "{}: {} jobs; {} models ({enabled} enabled), {} questions, {} statements → {where_}",
            o.project,
            t.report.jobs.len(),
            t.report.models.len(),
            t.report.questions.len(),
            t.report.statements.len()
        );
        if o.explain && settings.trino_url.is_some() {
            println!(
                "  Trino planned {explained} models; {not_planned} read a landed file and were not planned"
            );
        }
        for k in &outcome.kept {
            println!("  kept (edited by hand): {k}");
        }
    }
    Ok(())
}

/// Writes the translation on branch `ekos/talend/<project>/<id>` (a worktree, as `pentaho
/// translate` does).
fn propose(
    repo: &Path,
    pkg: &str,
    t: &ekos_lakehouse_talend_dbt::Translation,
) -> anyhow::Result<(String, WriteOutcome)> {
    anyhow::ensure!(
        repo.join(".git").exists(),
        "{} is not a git repository: run `lakehouse generate` first",
        repo.display()
    );
    let id = &sha256(&t.files.values().cloned().collect::<Vec<_>>().join("\u{0}"))[..12];
    let branch = format!("ekos/talend/{pkg}/{id}");
    for other in git(
        repo,
        &[
            "branch",
            "--list",
            "--format=%(refname:short)",
            &format!("ekos/talend/{pkg}/*"),
        ],
    )?
    .lines()
    {
        if other != branch {
            git(repo, &["branch", "-D", other])?;
        }
    }
    if git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
    {
        return Ok((
            format!("branch {branch} (unchanged)"),
            WriteOutcome::default(),
        ));
    }
    let wt = std::env::temp_dir().join(format!("ekos-lh-talend-{pkg}-{id}"));
    let _ = std::fs::remove_dir_all(&wt);
    git(repo, &["worktree", "prune"])?;
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            wt.to_str().expect("utf-8"),
            "HEAD",
        ],
    )?;
    let built = (|| -> anyhow::Result<(bool, WriteOutcome)> {
        let w = write_translation_of(&wt, "talend", pkg, &t.files)?;
        let changed = crate::sync::commit_all_pub(
            &wt,
            &format!(
                "lakehouse: translate Talend project {pkg}\n\nOpen proposals/talend-{pkg}.md. Merging is the approval."
            ),
        )?;
        Ok((changed, w))
    })();
    git(
        repo,
        &["worktree", "remove", "--force", wt.to_str().expect("utf-8")],
    )?;
    let (changed, w) = built?;
    if !changed {
        git(repo, &["branch", "-D", &branch])?;
        return Ok((
            "nothing new: the repository already has this translation".into(),
            w,
        ));
    }
    Ok((format!("branch {branch}"), w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn talend_paths_are_the_translations_own() {
        assert!(is_talend_path("talend/shop/translation.json"));
        assert!(is_talend_path("proposals/talend-shop.md"));
        assert!(is_talend_path("dbt/models/gold/talend/shop/x.sql"));
        assert!(!is_talend_path("dbt/models/gold/pentaho/shop/x.sql"));
    }

    /// A one-job project: a file into a cleared table.
    fn shop() -> ekos_lakehouse_talend_dbt::Translation {
        let item = r#"<talendfile:ProcessType xmlns:talendfile="platform:/resource/org.talend.model/model/TalendFile.xsd" defaultContext="Default"><context name="Default"/><node componentName="tFileInputDelimited"><elementParameter field="TEXT" name="UNIQUE_NAME" value="tFileInputDelimited_1"/><elementParameter field="TEXT" name="FILENAME" value="&quot;/in/a.csv&quot;"/><metadata connector="FLOW" name="x"><column name="id" type="id_Integer" key="true"/></metadata></node><node componentName="tPostgresqlOutput"><elementParameter field="TEXT" name="UNIQUE_NAME" value="tDBOutput_1"/><elementParameter field="TEXT" name="TABLE" value="&quot;t&quot;"/><elementParameter field="TEXT" name="DATA_ACTION" value="INSERT"/><elementParameter field="TEXT" name="TABLE_ACTION" value="CLEAR"/><metadata connector="FLOW" name="x"><column name="id" type="id_Integer" key="true"/></metadata></node><connection connectorName="FLOW" label="row1" metaname="row1" source="tFileInputDelimited_1" target="tDBOutput_1"/></talendfile:ProcessType>"#;
        let p = read_project(
            "",
            &[ProjectFile {
                path: "process/jLoad_0.1.item".into(),
                text: item.into(),
            }],
        );
        let plan = ekos_lakehouse_bronze::plan::plan(&[], &Default::default());
        ekos_lakehouse_talend_dbt::translate(
            &p,
            &plan,
            &ekos_lakehouse_talend_dbt::Settings {
                project: "Shop".into(),
                ..Default::default()
            },
        )
    }

    #[test]
    fn the_report_serves_the_saved_translation_by_project_and_component() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            report_json(dir.path(), None, None).unwrap()["projects"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        save_report(dir.path(), "Shop", "ekos/talend/shop/x", &shop()).unwrap();
        let all = report_json(dir.path(), None, None).unwrap();
        assert_eq!(all["projects"][0]["project"], "Shop");
        assert_eq!(all["projects"][0]["where"], "ekos/talend/shop/x");
        let one = report_json(dir.path(), Some("Shop"), None).unwrap();
        assert_eq!(one["state"], "translated");
        assert!(!one["models"].as_array().unwrap().is_empty());
        assert!(
            one["graph"]["edges"]
                .as_array()
                .is_some_and(|e| !e.is_empty()),
            "{}",
            one["graph"]
        );
        assert!(one["proposal"].as_str().unwrap().contains("Talend project"));
        let c = report_json(dir.path(), Some("Shop"), Some("jLoad:tDBOutput_1")).unwrap();
        assert!(
            c["component"]["sql"]
                .as_str()
                .unwrap()
                .contains("materialized='table'"),
            "{}",
            c["component"]
        );
        assert!(report_json(dir.path(), Some("Shop"), Some("jLoad:tX")).is_err());
        assert_eq!(
            report_json(dir.path(), Some("Other"), None).unwrap()["state"],
            "not translated"
        );
        // A verification beside it is attached, never read as a translation.
        std::fs::write(
            dir.path().join("shop.verification.json"),
            r#"{"summary":"1 equal","models":[{"model":"m","table":"T","result":"equal","rows":3,"excluded":[["DT","set from the clock"]]}]}"#,
        )
        .unwrap();
        let all = report_json(dir.path(), None, None).unwrap();
        assert_eq!(all["projects"].as_array().unwrap().len(), 1);
        let one = report_json(dir.path(), Some("Shop"), None).unwrap();
        assert_eq!(one["verification"]["models"][0]["result"], "equal");
        assert_eq!(one["verification"]["models"][0]["excluded"], "DT");
    }
}
