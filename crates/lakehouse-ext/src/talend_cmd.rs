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
    };
    // An empty plan: a model reading a lakehouse table is not verified, by name.
    let plan = ekos_lakehouse_bronze::plan::plan(&[], &Default::default());
    let options = ekos_lakehouse_talend_rel::Options { locale };
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
        println!("{}", report["summary"]);
    }
    Ok(())
}

/// A table as read: its header and rows.
type Table = (Vec<String>, Vec<Vec<Option<String>>>);

/// Lands, builds and compares; returns the report.
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
    let mut landed: Vec<serde_json::Value> = Vec::new();
    // 1. Land each file source from the run's inputs, as its component reads them.
    let inputs: Vec<(PathBuf, String)> = plan_doc["inputs"]
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
                run.join("inputs")
                    .join(p["from"].as_str().unwrap_or_default()),
                m["to"].as_str().unwrap_or_default().replace('/', "\\"),
            )
        })
        .collect();
    for (name, file) in &t.report.files {
        let Some(spec) = specs.iter().find(|s| &s.file == file) else {
            landed.push(
                json!({"source": name, "landed": false, "reason": "no file component reads it"}),
            );
            continue;
        };
        let mut rows = Vec::new();
        let mut matched = Vec::new();
        for (from, to) in &inputs {
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
        let cols: Vec<String> = spec
            .columns
            .iter()
            .map(|(c, ty)| format!("{} {}", q(c), trino_type_name(*ty)))
            .collect();
        let target = format!("{}.{}", schema("talend_files"), q(name));
        exec(&format!("CREATE TABLE {target} ({})", cols.join(", ")))
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
            exec(&format!(
                "INSERT INTO {target} VALUES {}",
                values.join(", ")
            ))
            .map_err(|e| anyhow::anyhow!("Trino, landing {name}: {e}"))?;
        }
        landed.push(json!({"source": name, "file": file, "component": format!("{}:{}", spec.job, spec.component), "files": matched, "rows": rows.len(), "landed": true}));
    }

    // 2. Build every enabled model's first version, in ref order.
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
    let locations: std::collections::BTreeMap<String, (String, String)> = t
        .report
        .models
        .iter()
        .map(|m| (m.name.clone(), alias_of(m)))
        .collect();
    let refs = |sql: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = sql;
        while let Some(i) = rest.find("{{ ref('") {
            let after = &rest[i + 8..];
            let Some(j) = after.find("') }}") else { break };
            out.push(after[..j].to_string());
            rest = &after[j..];
        }
        out
    };
    let mut built: std::collections::BTreeSet<String> = Default::default();
    let mut failed: std::collections::BTreeMap<String, String> = Default::default();
    let pending: Vec<&ekos_lakehouse_talend_dbt::ModelReport> =
        t.report.models.iter().filter(|m| m.enabled).collect();
    loop {
        let mut progress = false;
        for m in &pending {
            if built.contains(&m.name) || failed.contains_key(&m.name) {
                continue;
            }
            let sql = t.files.get(&m.path).cloned().unwrap_or_default();
            let deps = refs(&sql);
            if let Some(d) = deps.iter().find(|d| failed.contains_key(*d)) {
                failed.insert(m.name.clone(), format!("reads `{d}`, which did not build"));
                progress = true;
                continue;
            }
            if !deps.iter().all(|d| built.contains(d)) {
                continue;
            }
            let body = ekos_lakehouse_etl_dbt::engine_sql(
                &ekos_lakehouse_talend_dbt::gates::sources_as_tables(&sql),
                &locations,
                plan,
            )
            .replace(
                "\"talend_files\".\"",
                &format!("{}.\"", schema("talend_files")),
            )
            .replace("\"iceberg\".\"", &format!("\"memory\".\"{prefix}"));
            let body = if m.layer == Layer::Snapshot {
                // dbt's first version: from now, open, with a version id over the key and its start.
                let key: Vec<String> = config_value(&sql, "unique_key")
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
            let (sc, alias) = &locations[&m.name];
            match exec(&format!(
                "CREATE TABLE {}.{} AS {body}",
                schema(sc),
                q(alias)
            )) {
                Ok(_) => {
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

    // 3. Compare each gold model that writes a table with the table the jobs left.
    let last = std::fs::read_dir(out_dir.join("dumps"))?
        .flatten()
        .map(|e| e.path())
        .max()
        .ok_or_else(|| anyhow::anyhow!("the run has no dumps"))?;
    let dumps: Vec<PathBuf> = std::fs::read_dir(&last)?
        .flatten()
        .map(|e| e.path())
        .collect();
    let surrogates: std::collections::BTreeSet<String> = t
        .report
        .models
        .iter()
        .filter_map(|m| m.surrogate.as_ref().map(|s| s.to_lowercase()))
        .collect();
    let jobs_ran: std::collections::BTreeMap<String, bool> = manifest["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|j| {
            (
                j["job"].as_str().unwrap_or_default().to_string(),
                j["exit"] == json!(0),
            )
        })
        .collect();
    let mut checks: Vec<v::ModelCheck> = Vec::new();
    // The first model compared equal: its columns, and each side's header and rows.
    let mut control_rows: Option<(Vec<String>, Table, Table)> = None;
    for m in t
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
        let (sc, alias) = &locations[&m.name];
        let r = exec(&format!("SELECT * FROM {}.{}", schema(sc), q(alias)))
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
        checks.push(v::ModelCheck {
            model: m.name.clone(),
            table,
            columns: cols,
            excluded,
            outcome,
        });
    }
    // 4. The comparer's control: one dbt row removed must be found.
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
    let summary = format!(
        "{} equal, {} differ, {} not verified; {} models built, {} failed to build; comparer control: {}",
        count("equal"),
        count("differs"),
        count("not_verified"),
        built.len(),
        failed.len(),
        if control["found_the_planted_difference"] == json!(true) {
            "passed"
        } else if control["ran"] == json!(true) {
            "FAILED: the result is not trusted"
        } else {
            "not run"
        }
    );
    Ok(json!({
        "project": t.report.project,
        "run": run.display().to_string(),
        "run_day": manifest["run_day"],
        "scope": "one Talend run from empty tables against each model's first build; incremental runs are not compared",
        "landed": landed,
        "build_failures": failed,
        "models": checks,
        "control": control,
        "summary": summary,
    }))
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
    let s = ekos_lakehouse_talend_dbt::Settings {
        project: o.project.clone(),
        source_database: settings.source_database.clone(),
        faithful: o.faithful.clone(),
        locale,
    };
    // A long Java concatenation is a deep SQL expression: the parser needs more stack than an
    // async worker thread has.
    let mut t = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(scope, || {
                ekos_lakehouse_talend_dbt::translate(&project, &plan, &s)
            })
            .map(|h| h.join())
    })
    .map_err(|e| anyhow::anyhow!("cannot start the translation: {e}"))?
    .map_err(|_| anyhow::anyhow!("the translation failed"))?;
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
    }
}
