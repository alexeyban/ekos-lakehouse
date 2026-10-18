//! `lakehouse pentaho …` (RFC 0200–0204): Pentaho packages read into the deep Kettle model.
//!
//! This module is a raw-content entry point (RFC 0043): every file is checked against the
//! excluded-path globs and redacted before the model sees it.

use crate::plan_cmd::{read_lock, read_manifest, sha256};
use crate::settings::Settings;
use crate::sync::git;
use ekos::extension::CommandContext;
use ekos_common::redaction::{RedactionConfig, is_excluded_path, redact};
use ekos_lakehouse_kettle::{
    Category, Package, SourceFile, is_package_file, is_plugin_jar, read_package,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A file larger than this is skipped with a diagnostic (a `.ktr` is rarely over 1 MB).
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// A package to read: its name and directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDir {
    pub name: String,
    pub path: PathBuf,
}

/// The packages of a project: `[extensions.lakehouse] pentaho-packages` when set, otherwise
/// every observed directory (one per `[[sources]]` entry) that holds a `.ktr` or `.kjb`.
pub fn packages(settings: &Settings, ctx: &CommandContext<'_>) -> Vec<PackageDir> {
    if !settings.pentaho_packages.is_empty() {
        return settings
            .pentaho_packages
            .iter()
            .map(|(name, p)| PackageDir {
                name: name.clone(),
                path: ctx.cwd.join(p),
            })
            .collect();
    }
    ctx.config
        .observe_bases(ctx.cwd)
        .into_iter()
        .filter(|b| has_kettle_files(&b.path))
        .map(|b| PackageDir {
            name: b.key,
            path: b.path,
        })
        .collect()
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn has_kettle_files(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if p.is_dir() {
                if !skip_dir(&name) {
                    stack.push(p);
                }
            } else if name.ends_with(".ktr") || name.ends_with(".kjb") {
                return true;
            }
        }
    }
    false
}

/// Reads the package under `root`: package files redacted, jars listed by path only.
pub fn read_dir(root: &Path, redaction: &RedactionConfig) -> Package {
    let mut files = Vec::new();
    let mut jars = Vec::new();
    let mut skipped = Vec::new();
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
            if is_excluded_path(&rel, redaction) {
                continue;
            }
            if is_plugin_jar(&rel) {
                jars.push(rel);
                continue;
            }
            if !is_package_file(&rel) {
                continue;
            }
            if e.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                skipped.push((rel, "larger than 32 MB; not read".to_string()));
                continue;
            }
            match std::fs::read_to_string(&p) {
                Ok(text) => files.push(SourceFile {
                    path: rel,
                    text: redact(&text, redaction),
                }),
                Err(err) => skipped.push((rel, format!("not readable as UTF-8 text: {err}"))),
            }
        }
    }
    let mut package = read_package(&files, &jars);
    for (path, message) in skipped {
        package
            .diagnostics
            .push(ekos_lakehouse_kettle::Diagnostic { path, message });
    }
    package.diagnostics.sort();
    package
}

/// One package's summary for `pentaho scan` (and the console's Packages panel).
pub fn summary(name: &str, path: &Path, package: &Package) -> Value {
    let c = package.coverage();
    let unknown: Vec<Value> = c
        .step_types
        .iter()
        .filter(|(_, (cat, _))| *cat == Category::Unknown)
        .map(|(t, (_, n))| json!({"type": t, "steps": n}))
        .collect();
    let categories: serde_json::Map<String, Value> = c
        .categories
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), json!(v)))
        .collect();
    json!({
        "package": name,
        "path": path.display().to_string(),
        "transformations": c.transformations,
        "jobs": c.jobs,
        "steps": c.steps,
        "step_types": c.step_types.len(),
        "entries": c.entries,
        "categories": categories,
        "unknown_types": unknown,
        "unknown_count": unknown.len(),
        "diagnostic_count": package.diagnostics.len(),
        "links": c.links,
        "unresolved_links": c.unresolved_links,
        "plugins": package.plugins.len(),
        "diagnostics": package.diagnostics,
    })
}

/// `lakehouse pentaho scan`.
pub fn scan(settings: &Settings, ctx: &CommandContext<'_>, as_json: bool) -> anyhow::Result<()> {
    let redaction = ctx.config.redaction_config();
    let repo = ctx
        .cwd
        .join(settings.pipeline_dir.as_deref().unwrap_or("pipeline"));
    let rows: Vec<Value> = packages(settings, ctx)
        .iter()
        .map(|p| {
            let mut row = summary(&p.name, &p.path, &read_dir(&p.path, &redaction));
            let pkg = ekos_lakehouse_pentaho_dbt::slug(&p.name);
            match load_report(&repo, &pkg) {
                Some((r, state)) => {
                    row["translation"] = json!(state);
                    row["translated"] = counts_json(&r);
                }
                None => row["translation"] = json!("not translated"),
            }
            row
        })
        .collect();
    if as_json {
        println!("{}", serde_json::to_string(&json!({ "packages": rows }))?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no Pentaho package found: no observed directory holds a .ktr or .kjb \
             (set [extensions.lakehouse] pentaho-packages to name one)"
        );
    }
    for r in &rows {
        println!(
            "{}: {} transformations, {} jobs, {} steps of {} types, {} links ({} unresolved)",
            r["package"].as_str().unwrap_or(""),
            r["transformations"],
            r["jobs"],
            r["steps"],
            r["step_types"],
            r["links"],
            r["unresolved_links"]
        );
        for (cat, n) in r["categories"].as_object().into_iter().flatten() {
            println!("  {cat:<20} {n}");
        }
        for d in r["diagnostics"].as_array().into_iter().flatten() {
            println!(
                "  ! {}: {}",
                d["path"].as_str().unwrap_or(""),
                d["message"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

/// Whether a pipeline-repository path belongs to a Pentaho translation (RFC 0204): `generate`
/// leaves these alone, `pentaho translate` owns them per package.
pub fn is_pentaho_path(path: &str) -> bool {
    path.starts_with("pentaho/")
        || path.starts_with("proposals/pentaho-")
        || path.contains("/pentaho/")
}

fn owned_by(path: &str, pkg: &str) -> bool {
    path.starts_with(&format!("pentaho/{pkg}/"))
        || path == format!("proposals/pentaho-{pkg}.md")
        || path.contains(&format!("/pentaho/{pkg}/"))
}

/// What writing a translation did.
#[derive(Debug, Default, serde::Serialize)]
pub struct WriteOutcome {
    pub written: usize,
    /// Edited by a person since the last translation: left as they are.
    pub kept: Vec<String>,
    pub removed: Vec<String>,
}

/// Writes `files` under `out` with the `.generated.json` rules of `generate`: a file a person
/// edited is kept, a file this package no longer produces is removed when unedited.
pub fn write_translation(
    out: &Path,
    pkg: &str,
    files: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<WriteOutcome> {
    let mut manifest = read_manifest(out);
    let mut o = WriteOutcome::default();
    for (path, content) in files {
        let full = out.join(path);
        if let Ok(have) = std::fs::read_to_string(&full) {
            let ours = manifest.get(path).is_some_and(|h| *h == sha256(&have));
            if !ours && &have != content {
                o.kept.push(path.clone());
                continue;
            }
        }
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&full, content)?;
        manifest.insert(path.clone(), sha256(content));
        o.written += 1;
    }
    let stale: Vec<String> = manifest
        .keys()
        .filter(|p| owned_by(p, pkg) && !files.contains_key(*p))
        .cloned()
        .collect();
    for path in stale {
        let full = out.join(&path);
        match std::fs::read_to_string(&full) {
            Ok(have) if sha256(&have) == manifest[&path] => {
                std::fs::remove_file(&full)?;
                manifest.remove(&path);
                o.removed.push(path);
            }
            Ok(_) => o.kept.push(path),
            Err(_) => {
                manifest.remove(&path);
            }
        }
    }
    std::fs::write(
        out.join(crate::plan_cmd::MANIFEST),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;
    Ok(o)
}

/// Options of `lakehouse pentaho translate`.
pub struct TranslateOptions {
    pub package: String,
    /// Write into this directory instead of proposing a branch.
    pub out: Option<PathBuf>,
    /// Plan every enabled model on Trino (when a Trino URL is configured).
    pub explain: bool,
    pub json: bool,
}

/// Disables model `name` in `t` with `reason` (a model Trino refused).
fn disable_model(t: &mut ekos_lakehouse_pentaho_dbt::Translation, name: &str, reason: &str) {
    let Some(m) = t.report.models.iter_mut().find(|m| m.name == name) else {
        return;
    };
    m.enabled = false;
    m.fidelity = ekos_lakehouse_kettle_expr::Fidelity::Gap;
    m.gaps.push(reason.to_string());
    if let Some(sql) = t.files.get_mut(&m.path) {
        let mut body = format!(
            "{}\n-- DISABLED: {}\n{{{{ config(enabled=false) }}}}\n",
            ekos_lakehouse_pentaho_dbt::HEADER,
            reason.replace('\n', " ")
        );
        for line in sql
            .lines()
            .filter(|l| !l.starts_with("--") && !l.contains("config("))
        {
            body.push_str(&format!("-- {line}\n"));
        }
        *sql = body;
    }
}

/// `lakehouse pentaho translate <package>`.
pub async fn translate(
    settings: &Settings,
    ctx: &CommandContext<'_>,
    o: &TranslateOptions,
    repo: &Path,
) -> anyhow::Result<()> {
    let pkgs = packages(settings, ctx);
    let Some(dir) = pkgs.iter().find(|p| p.name == o.package) else {
        anyhow::bail!(
            "no Pentaho package `{}` (have: {})",
            o.package,
            pkgs.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let plan = read_lock(repo).map_err(|e| anyhow::anyhow!("{e}: run `lakehouse generate` first — Pentaho tables resolve to the deployed silver models"))?;
    let redaction = ctx.config.redaction_config();
    let package = read_dir(&dir.path, &redaction);
    let mut t = ekos_lakehouse_pentaho_dbt::translate(
        &package,
        &plan,
        &ekos_lakehouse_pentaho_dbt::Settings {
            package: o.package.clone(),
            source_database: settings.source_database.clone(),
            empty_string_is_null: settings.pentaho_empty_string_is_null.unwrap_or(true),
            plugin_code: plugin_code(&dir.path, &package, ctx),
        },
    );
    let pkg = ekos_lakehouse_pentaho_dbt::slug(&o.package);
    // Trino plans every enabled model; one it refuses is disabled with its message.
    let mut explained = 0usize;
    if o.explain && settings.trino_url.is_some() {
        let names: Vec<String> = t
            .report
            .models
            .iter()
            .filter(|m| m.enabled)
            .map(|m| m.name.clone())
            .collect();
        let mut refused = Vec::new();
        let conn = settings.trino_connection()?;
        let checks: Vec<(String, Result<String, String>)> = names
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    ekos_lakehouse_pentaho_dbt::plannable_sql(n, &t.files, &plan),
                )
            })
            .collect();
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
        for (n, r) in results {
            match r {
                Ok(()) => explained += 1,
                Err(e) => refused.push((n, e)),
            }
        }
        for (n, e) in &refused {
            disable_model(&mut t, n, &format!("Trino cannot plan it: {e}"));
        }
        if !refused.is_empty() {
            t.files.insert(
                format!("pentaho/{pkg}/translation.json"),
                serde_json::to_string_pretty(&t.report)? + "\n",
            );
            t.files.insert(
                format!("proposals/pentaho-{pkg}.md"),
                ekos_lakehouse_pentaho_dbt::proposal::markdown(&t.report),
            );
        }
    }
    let (where_, outcome) = match &o.out {
        Some(out) => {
            let w = write_translation(out, &pkg, &t.files)?;
            (out.display().to_string(), w)
        }
        None => propose(repo, &pkg, &t)?,
    };
    if let Ok(store) = crate::facts::open_store_waiting(ctx.config, ctx.cwd) {
        crate::facts::record_translation(store.as_ref(), &t.report)?;
    }
    let counts = t.report.counts();
    if o.json {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "package": o.package,
                "where": where_,
                "counts": counts.iter().map(|(k, v)| (k.as_str(), v)).collect::<std::collections::BTreeMap<_, _>>(),
                "models": t.report.models.len(),
                "enabled": t.report.models.iter().filter(|m| m.enabled).count(),
                "questions": t.report.questions.len(),
                "explained": explained,
                "write": outcome,
            }))?
        );
    } else {
        println!(
            "{}: {} steps — {}; {} models ({} enabled), {} questions → {where_}",
            o.package,
            t.report.steps.len(),
            counts
                .iter()
                .map(|(k, v)| format!("{v} {}", k.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
            t.report.models.len(),
            t.report.models.iter().filter(|m| m.enabled).count(),
            t.report.questions.len()
        );
        if o.explain && settings.trino_url.is_some() {
            println!("  Trino planned {explained} models");
        }
        for k in &outcome.kept {
            println!("  kept (edited by hand): {k}");
        }
    }
    Ok(())
}

/// Decompiled `processRow` source for every plugin step type of `package`, when this binary
/// has a `JavaDecompiler` (the private ekos-binary build, RFC 0200/0203). The public build has
/// none: plugin steps stay gaps that say so.
fn plugin_code(
    root: &Path,
    package: &Package,
    ctx: &CommandContext<'_>,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Some(decompiler) = ctx.extensions.java_decompiler() else {
        return out;
    };
    for plugin in &package.plugins {
        let base = plugin.class.trim_end_matches("Meta").to_string();
        let candidates = [base.clone(), format!("{base}Step"), plugin.class.clone()];
        'jars: for jar in &plugin.jars {
            let Ok(bytes) = std::fs::read(root.join(jar)) else {
                continue;
            };
            if bytes.len() as u64 > MAX_FILE_BYTES * 8 {
                continue;
            }
            for class in &candidates {
                let binary_name = class.replace('.', "/");
                let Ok(c) = decompiler.decompile(&bytes, &binary_name) else {
                    continue;
                };
                if let Some(m) = c.methods.iter().find(|m| m.name == "processRow") {
                    let source = format!(
                        "public boolean processRow(StepMetaInterface smi, StepDataInterface sdi) {{\n{}\n}}",
                        m.source
                    );
                    out.insert(plugin.id.clone(), source);
                    break 'jars;
                }
            }
        }
    }
    out
}

/// Writes the translation on branch `ekos/pentaho/<pkg>/<id>` (a worktree, as RFC 0194/0195).
fn propose(
    repo: &Path,
    pkg: &str,
    t: &ekos_lakehouse_pentaho_dbt::Translation,
) -> anyhow::Result<(String, WriteOutcome)> {
    anyhow::ensure!(
        repo.join(".git").exists(),
        "{} is not a git repository: run `lakehouse generate` first",
        repo.display()
    );
    let id = &sha256(&t.files.values().cloned().collect::<Vec<_>>().join("\u{0}"))[..12];
    let branch = format!("ekos/pentaho/{pkg}/{id}");
    for other in git(
        repo,
        &[
            "branch",
            "--list",
            "--format=%(refname:short)",
            &format!("ekos/pentaho/{pkg}/*"),
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
    let wt = std::env::temp_dir().join(format!("ekos-lh-pentaho-{pkg}-{id}"));
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
        let w = write_translation(&wt, pkg, &t.files)?;
        let changed = crate::sync::commit_all_pub(
            &wt,
            &format!(
                "lakehouse: translate Pentaho package {pkg}\n\nOpen proposals/pentaho-{pkg}.md. Merging is the approval."
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

/// The newest translation report of a package: the working tree's, else the proposal branch's.
pub fn load_report(repo: &Path, pkg: &str) -> Option<(ekos_lakehouse_pentaho_dbt::Report, String)> {
    let rel = format!("pentaho/{pkg}/translation.json");
    let branch = git(
        repo,
        &[
            "branch",
            "--list",
            "--format=%(refname:short)",
            &format!("ekos/pentaho/{pkg}/*"),
        ],
    )
    .ok()
    .and_then(|b| b.lines().next().map(str::to_string));
    if let Some(b) = &branch
        && let Ok(text) = git(repo, &["show", &format!("{b}:{rel}")])
        && let Ok(r) = serde_json::from_str(&text)
    {
        return Some((r, format!("proposed on {b}")));
    }
    let text = std::fs::read_to_string(repo.join(&rel)).ok()?;
    Some((serde_json::from_str(&text).ok()?, "merged".into()))
}

/// The graph the console draws for one translation: steps grouped by transformation, coloured
/// by fidelity, with an edge per hop (RFC 0207).
pub fn graph(r: &ekos_lakehouse_pentaho_dbt::Report) -> Value {
    let mut nodes: Vec<Value> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut edges = std::collections::BTreeSet::new();
    for s in r.steps.iter().filter(|s| s.category != "model") {
        if seen.insert(s.id.clone()) {
            nodes.push(json!({
                "id": s.id,
                "label": s.step,
                "group": s.transformation,
                "fidelity": s.fidelity.as_str(),
            }));
        } else if let Some(n) = nodes.iter_mut().find(|n| n["id"] == s.id.as_str()) {
            // Several nodes of one step (routed outputs): the weakest label shows.
            let have = n["fidelity"].as_str().unwrap_or("exact").to_string();
            let rank = |f: &str| {
                ["exact", "rewritten", "llm-drafted", "gap"]
                    .iter()
                    .position(|x| *x == f)
                    .unwrap_or(0)
            };
            if rank(s.fidelity.as_str()) > rank(&have) {
                n["fidelity"] = json!(s.fidelity.as_str());
            }
        }
        for i in &s.inputs {
            edges.insert((i.clone(), s.id.clone()));
        }
    }
    let edges: Vec<Value> = edges
        .into_iter()
        .map(|(f, t)| json!({"from": f, "to": t}))
        .collect();
    json!({ "nodes": nodes, "edges": edges })
}

fn counts_json(r: &ekos_lakehouse_pentaho_dbt::Report) -> Value {
    let c = r.counts();
    let n = |f: ekos_lakehouse_kettle_expr::Fidelity| c.get(&f).copied().unwrap_or(0);
    use ekos_lakehouse_kettle_expr::Fidelity as F;
    json!({
        "exact": n(F::Exact),
        "rewritten": n(F::Rewritten),
        "llm_drafted": n(F::LlmDrafted),
        "gap": n(F::Gap),
        "models": r.models.len(),
        "enabled": r.models.iter().filter(|m| m.enabled).count(),
        "questions": r.questions.len(),
    })
}

/// `lakehouse pentaho report`: without `--package`, one row per package; with it, that
/// translation's models, steps, questions, graph and proposal; with `--step` too, that step's
/// original text and SQL (the console's Pentaho tab, RFC 0207).
pub fn report(
    settings: &Settings,
    ctx: &CommandContext<'_>,
    repo: &Path,
    package: Option<&str>,
    step: Option<&str>,
    as_json: bool,
) -> anyhow::Result<()> {
    let Some(name) = package else {
        let rows: Vec<Value> = packages(settings, ctx)
            .into_iter()
            .map(|p| {
                let pkg = ekos_lakehouse_pentaho_dbt::slug(&p.name);
                match load_report(repo, &pkg) {
                    Some((r, state)) => {
                        json!({"package": p.name, "state": state, "summary": counts_json(&r)})
                    }
                    None => json!({"package": p.name, "state": "not translated", "summary": {}}),
                }
            })
            .collect();
        if as_json {
            println!(
                "{}",
                serde_json::to_string(&json!({ "translations": rows }))?
            );
        } else {
            for r in &rows {
                println!(
                    "{}: {} {}",
                    r["package"].as_str().unwrap_or(""),
                    r["state"].as_str().unwrap_or(""),
                    r["summary"]
                );
            }
        }
        return Ok(());
    };
    let pkg = ekos_lakehouse_pentaho_dbt::slug(name);
    let Some((r, state)) = load_report(repo, &pkg) else {
        let out = json!({"package": name, "state": "not translated", "models": [], "steps": [], "questions": []});
        if as_json {
            println!("{}", serde_json::to_string(&out)?);
        } else {
            println!("{name}: not translated (run `lakehouse pentaho translate {name}`)");
        }
        return Ok(());
    };
    let selected = step.map(|id| r.steps.iter().find(|s| s.id == id || s.step == id).cloned());
    if let Some(None) = selected {
        anyhow::bail!(
            "no step `{}` in the translation of {name}",
            step.unwrap_or_default()
        );
    }
    let lean: Vec<Value> = r
        .steps
        .iter()
        .filter(|s| s.category != "model")
        .map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.remove("original");
                o.remove("sql");
                o.insert("notes_text".into(), json!(s.notes.join("; ")));
                o.insert("models_text".into(), json!(s.models.join(", ")));
            }
            v
        })
        .collect();
    let mut out = json!({
        "package": name,
        "state": state,
        "summary": counts_json(&r),
        "models": r.models,
        "steps": lean,
        "questions": r.questions,
        "graph": graph(&r),
        "proposal": ekos_lakehouse_pentaho_dbt::proposal::markdown(&r),
    });
    if let Some(Some(s)) = selected {
        out["step"] = serde_json::to_value(&s)?;
        out["step"]["notes_text"] = json!(s.notes.join("\n"));
    }
    if as_json {
        println!("{}", serde_json::to_string(&out)?);
        return Ok(());
    }
    println!("{name}: {state} {}", out["summary"]);
    for m in &r.models {
        println!(
            "  {:<60} {:<6} {:<12} {}",
            m.name,
            format!("{:?}", m.layer).to_lowercase(),
            m.fidelity.as_str(),
            if m.enabled { "" } else { "DISABLED" }
        );
    }
    if let Some(s) = out.get("step") {
        println!(
            "\n{}\n--- original ---\n{}\n--- SQL ---\n{}",
            s["id"].as_str().unwrap_or(""),
            s["original"].as_str().unwrap_or(""),
            s["sql"].as_str().unwrap_or("(not translated)")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reader_redacts_and_honours_excluded_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("etl/.git")).unwrap();
        std::fs::write(
            dir.path().join("etl/load.ktr"),
            "<transformation><info><name>load</name></info>\
             <connection><name>src</name><type>POSTGRESQL</type>\
             <password>Encrypted 2be98afc86aa7b88bb81cbe788fc0fce5</password></connection>\
             <step><name>JS</name><type>ScriptValueMod</type>\
             <jsScript_script>var token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\";</jsScript_script></step>\
             </transformation>",
        )
        .unwrap();
        std::fs::write(dir.path().join("etl/.git/x.ktr"), "<transformation/>").unwrap();
        std::fs::write(dir.path().join("etl/.env"), "PASSWORD=x").unwrap();
        let p = read_dir(dir.path(), &RedactionConfig::default());
        assert_eq!(p.transformations.len(), 1, "hidden directories are skipped");
        let all = serde_json::to_string(&p).unwrap();
        assert!(!all.contains("2be98afc"), "{all}");
        assert!(!all.contains("ghp_abcdefghij"), "{all}");
        let s = summary("etl", dir.path(), &p);
        assert_eq!(s["categories"]["code"], 1);
    }
}
