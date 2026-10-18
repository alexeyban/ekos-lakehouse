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
        "links": c.links,
        "unresolved_links": c.unresolved_links,
        "plugins": package.plugins.len(),
        "diagnostics": package.diagnostics,
    })
}

/// `lakehouse pentaho scan`.
pub fn scan(settings: &Settings, ctx: &CommandContext<'_>, as_json: bool) -> anyhow::Result<()> {
    let redaction = ctx.config.redaction_config();
    let rows: Vec<Value> = packages(settings, ctx)
        .iter()
        .map(|p| summary(&p.name, &p.path, &read_dir(&p.path, &redaction)))
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

/// `lakehouse pentaho report`: packages, models and steps of the latest translations.
pub fn report(
    settings: &Settings,
    ctx: &CommandContext<'_>,
    repo: &Path,
    package: Option<&str>,
    as_json: bool,
) -> anyhow::Result<()> {
    let names: Vec<String> = match package {
        Some(p) => vec![p.to_string()],
        None => packages(settings, ctx)
            .into_iter()
            .map(|p| p.name)
            .collect(),
    };
    let mut out = Vec::new();
    for n in names {
        let pkg = ekos_lakehouse_pentaho_dbt::slug(&n);
        match load_report(repo, &pkg) {
            Some((r, state)) => out.push(json!({"package": n, "state": state, "counts": r.counts().iter().map(|(k, v)| (k.as_str(), v)).collect::<std::collections::BTreeMap<_, _>>(), "report": r})),
            None => out.push(json!({"package": n, "state": "not translated", "counts": {}, "report": null})),
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string(&json!({ "translations": out }))?
        );
        return Ok(());
    }
    for t in &out {
        println!(
            "{}: {} {}",
            t["package"].as_str().unwrap_or(""),
            t["state"].as_str().unwrap_or(""),
            t["counts"]
        );
        for m in t["report"]["models"].as_array().into_iter().flatten() {
            println!(
                "  {:<60} {:<6} {:<12} {}",
                m["name"].as_str().unwrap_or(""),
                m["layer"].as_str().unwrap_or(""),
                m["fidelity"].as_str().unwrap_or(""),
                if m["enabled"].as_bool() == Some(true) {
                    ""
                } else {
                    "DISABLED"
                }
            );
        }
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
