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
