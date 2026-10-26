//! EKOS RFC 0228 (step 7): marts — each approved requirement's way through a proposal, a merge,
//! a build and a reconciliation, and what the Marts page shows of it. The pipeline repository
//! (git) and the state directory are read here; the ledger keeps the facts.

use crate::requirements as rq;
use crate::sync::git;
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_gold::{self as gold, MartSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where dbt builds gold (`schema='gold'` in the generated model).
pub const GOLD_SCHEMA: &str = "gold";

pub fn state_dir(config: &ekos_compiler_core::EkosConfig, cwd: &Path) -> PathBuf {
    config.ekos_dir(cwd).join("lakehouse").join("marts")
}

/// The latest gold run of a mart, as `run-report` recorded it (RFC 0196).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    pub run_id: String,
    pub at: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub measure: String,
    pub mart_total: String,
    pub silver_total: String,
    pub equal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconciliation {
    pub mart: String,
    pub at: String,
    /// The gold run it checked, when one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub measures: Vec<Totals>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().expect("a parent"))?;
    std::fs::write(path, serde_json::to_string_pretty(v)? + "\n")?;
    Ok(())
}

pub fn read_run(dir: &Path, mart: &str) -> Option<LastRun> {
    read_json(&dir.join(format!("{mart}.run.json")))
}

pub fn write_run(dir: &Path, mart: &str, r: &LastRun) -> anyhow::Result<()> {
    write_json(&dir.join(format!("{mart}.run.json")), r)
}

pub fn read_reconciliation(dir: &Path, mart: &str) -> Option<Reconciliation> {
    read_json(&dir.join(format!("{mart}.reconcile.json")))
}

pub fn write_reconciliation(dir: &Path, r: &Reconciliation) -> anyhow::Result<()> {
    write_json(&dir.join(format!("{}.reconcile.json", r.mart)), r)
}

/// `gold.<mart>` (RFC 0196's table name for a gold model) → the mart.
pub fn mart_of(table: &str) -> Option<&str> {
    table.strip_prefix("gold.")
}

// ─────────────────────────────── the repository ───────────────────────────────

pub fn proposal_branch(repo: &Path, mart: &str) -> Option<String> {
    git(
        repo,
        &[
            "branch",
            "--list",
            "--format=%(refname:short)",
            &format!("ekos/gold/{mart}/*"),
        ],
    )
    .ok()
    .and_then(|s| s.lines().last().map(str::to_string))
}

/// A file as it is at `rev` (a branch, `HEAD`), if it is there — byte for byte: its digest is
/// compared with the approval's, so `git`'s trimmed output would not do.
pub fn file_at(repo: &Path, rev: &str, path: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{rev}:{path}")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn model_path(mart: &str) -> String {
    format!("dbt/models/gold/gold__{mart}.sql")
}

pub fn description_path(mart: &str) -> String {
    format!("marts/{mart}.md")
}

/// Every mart with a requirement or a description in the repository.
pub fn list(req_dir: &Path, repo: &Path) -> Vec<String> {
    let mut out: BTreeSet<String> = rq::list(req_dir).into_iter().collect();
    for e in std::fs::read_dir(repo.join("marts"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let n = e.file_name().to_string_lossy().into_owned();
        if let Some(m) = n.strip_suffix(".md")
            && !m.ends_with(".questions")
        {
            out.insert(m.to_string());
        }
    }
    out.into_iter().collect()
}

// ─────────────────────────────── the state ───────────────────────────────

/// What the state is computed from (pure inputs, so it is tested without git or a ledger).
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The approval's digest, when the requirement is approved and still valid.
    pub approved: Option<String>,
    /// The digest of the description the proposal branch carries.
    pub proposed: Option<String>,
    /// The model is on the repository's current branch.
    pub merged: bool,
    pub run: Option<LastRun>,
    pub reconciliation: Option<Reconciliation>,
}

/// RFC 0228 §2.1, the first that holds.
pub fn state(f: &Facts) -> &'static str {
    let Some(approved) = &f.approved else {
        return "requirement";
    };
    if !f.merged {
        return if f.proposed.as_deref() == Some(approved.as_str()) {
            "proposed"
        } else {
            "to propose"
        };
    }
    let Some(run) = &f.run else { return "merged" };
    if run.status != "ok" {
        return "failed";
    }
    match &f.reconciliation {
        // A reconciliation of an earlier build says nothing about this one.
        Some(r) if r.run_id.as_deref().is_none_or(|id| id == run.run_id) => {
            if r.measures.iter().all(|m| m.equal) {
                "reconciled"
            } else {
                "diverged"
            }
        }
        _ => "built",
    }
}

/// The facts for one mart, from the requirement's state, the repository and the state dir.
pub fn facts(req_dir: &Path, dir: &Path, repo: &Path, plan: Option<&Plan>, mart: &str) -> Facts {
    let approved = (|| {
        let d = rq::read(req_dir, mart).ok()??;
        let a = rq::read_approval(req_dir, mart).ok()??;
        let p = plan?;
        let ev = rq::evaluate(&d.spec, p);
        (rq::state(&d.text, &ev, Some(&a), rq::committed(repo, mart).as_deref()) == "approved")
            .then_some(a.digest)
    })();
    let proposed = proposal_branch(repo, mart)
        .and_then(|b| file_at(repo, &b, &description_path(mart)))
        .map(|t| rq::sha256(&t));
    Facts {
        approved,
        proposed,
        merged: file_at(repo, "HEAD", &model_path(mart)).is_some(),
        run: read_run(dir, mart),
        reconciliation: read_reconciliation(dir, mart),
    }
}

/// The mart's description from the repository, resolved and compiled against the plan.
pub fn compiled(
    repo: &Path,
    plan: &Plan,
    mart: &str,
) -> anyhow::Result<(MartSpec, Vec<gold::Resolved>, gold::Compiled)> {
    let text = file_at(repo, "HEAD", &description_path(mart))
        .ok_or_else(|| anyhow::anyhow!("{} is not committed", description_path(mart)))?;
    let spec = gold::parse(&text).map_err(|e| anyhow::anyhow!("{mart}: {e}"))?;
    let resolved = gold::resolve(&spec, plan);
    let c = gold::compile(&spec, plan, &resolved, &Default::default()).map_err(|qs| {
        anyhow::anyhow!(
            "{mart} no longer resolves against the plan: {}",
            qs.iter()
                .map(|q| q.text.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    Ok((spec, resolved, c))
}

// ─────────────────────────────── show ───────────────────────────────

fn selected(repo: &Path, plan: &Plan, mart: &str, st: &str, f: &Facts) -> Value {
    let sql = file_at(repo, "HEAD", &model_path(mart))
        .or_else(|| proposal_branch(repo, mart).and_then(|b| file_at(repo, &b, &model_path(mart))));
    let mut md = format!("### {mart} — {st}\n\n");
    md.push_str(
        "Generated from the approved requirement. The SQL is a template: terms marked \
         `drafted` were written by a model and need a person to read them.\n\n",
    );
    let Ok((spec, resolved, c)) = compiled(repo, plan, mart) else {
        md.push_str("The description is not committed yet: approve the requirement (step 4).\n");
        return json!({"mart": mart, "state": st, "markdown": md});
    };
    let terms: Vec<Value> = resolved
        .iter()
        .map(|r| {
            json!({
                "section": r.section,
                "term": r.term,
                "written": r.written,
                "fidelity": gold::fidelity(r),
                "how": r.how,
            })
        })
        .collect();
    let mut tests = vec![json!({
        "test": "grain is unique",
        "what": format!("one row per {}", spec.grain.join(", ")),
    })];
    for (name, _) in gold::reconciliation_tests(&spec, &resolved, &c) {
        tests.push(json!({"test": name, "what": "the measure's total equals silver's"}));
    }
    let recon: Vec<Value> = f
        .reconciliation
        .iter()
        .flat_map(|r| r.measures.iter())
        .map(|m| {
            json!({
                "measure": m.measure,
                "mart_total": m.mart_total,
                "silver_total": m.silver_total,
                "equal": if m.equal { "equal" } else { "differs" },
            })
        })
        .collect();
    let (nodes, edges) = gold::lineage(&spec, &c);
    if let Some(r) = &f.run {
        md.push_str(&format!(
            "**Last build:** {} at {} (run `{}`{}){}.\n\n",
            r.status,
            r.at,
            r.run_id,
            r.rows.map(|n| format!(", {n} rows")).unwrap_or_default(),
            r.message
                .as_ref()
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        ));
    }
    if let Some(r) = &f.reconciliation {
        md.push_str(&format!("**Last reconciliation:** {}.\n\n", r.at));
    }
    let drafted = resolved
        .iter()
        .filter(|r| gold::fidelity(r) == "drafted")
        .count();
    md.push_str(&format!(
        "{} term(s), {} exact, {drafted} drafted; reads {}.\n\n",
        resolved.len(),
        resolved.len() - drafted,
        c.tables.join(", ")
    ));
    json!({
        "mart": mart,
        "state": st,
        "markdown": md,
        "sql": sql.unwrap_or_default(),
        "description": file_at(repo, "HEAD", &description_path(mart)).unwrap_or_default(),
        "terms": terms,
        "tests": tests,
        "reconciliation": recon,
        "graph": {
            "nodes": nodes.iter().map(|(id, label, group)| json!({"id": id, "label": label, "group": group, "fidelity": group})).collect::<Vec<_>>(),
            "edges": edges.iter().map(|(from, to)| json!({"from": from, "to": to})).collect::<Vec<_>>(),
        },
    })
}

pub fn show(
    req_dir: &Path,
    dir: &Path,
    repo: &Path,
    plan: Option<&Plan>,
    want: Option<&str>,
) -> Value {
    let mut marts = vec![];
    let mut sel = Value::Null;
    for mart in list(req_dir, repo) {
        let f = facts(req_dir, dir, repo, plan, &mart);
        let st = state(&f);
        marts.push(json!({
            "mart": mart,
            "state": st,
            "branch": proposal_branch(repo, &mart),
            "last_build": f.run.as_ref().map(|r| format!("{} {}", r.status, r.at)),
            "reconciled_at": f.reconciliation.as_ref().map(|r| r.at.clone()),
        }));
        if want == Some(mart.as_str())
            && let Some(p) = plan
        {
            sel = selected(repo, p, &mart, st, &f);
        }
    }
    json!({"marts": marts, "selected": sel})
}

pub fn render(v: &Value) -> String {
    let mut s = String::new();
    let marts = v["marts"].as_array().cloned().unwrap_or_default();
    if marts.is_empty() {
        s.push_str("no marts: approve a requirement first (`lakehouse requirements approve`)\n");
    }
    for m in &marts {
        s.push_str(&format!(
            "{:<32} {:<12} {}\n",
            m["mart"].as_str().unwrap_or_default(),
            m["state"].as_str().unwrap_or_default(),
            m["last_build"].as_str().unwrap_or_default(),
        ));
    }
    let sel = &v["selected"];
    if let Some(md) = sel["markdown"].as_str() {
        s.push('\n');
        s.push_str(md);
        for r in sel["reconciliation"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "  {}: mart {} / silver {} — {}\n",
                r["measure"].as_str().unwrap_or_default(),
                r["mart_total"].as_str().unwrap_or_default(),
                r["silver_total"].as_str().unwrap_or_default(),
                r["equal"].as_str().unwrap_or_default()
            ));
        }
    }
    s
}

/// Pure: the two totals compared as numbers when both are numbers (`10.50` = `10.5`), else as
/// text; NULL (an empty cell) only equals NULL.
pub fn totals_equal(a: &str, b: &str) -> bool {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Step 7 on the bar (RFC 0228 §2.6), from the states of the marts with an approved requirement.
pub fn step(states: &[&str]) -> Option<(bool, String)> {
    let live: Vec<&&str> = states.iter().filter(|s| **s != "requirement").collect();
    if live.is_empty() {
        return None;
    }
    let n = |x: &str| live.iter().filter(|s| ***s == x).count();
    let done = n("reconciled");
    let mut parts = vec![format!("{done} of {} mart(s) reconciled", live.len())];
    for x in [
        "diverged",
        "failed",
        "built",
        "merged",
        "proposed",
        "to propose",
    ] {
        if n(x) > 0 {
            parts.push(format!("{} {x}", n(x)));
        }
    }
    Some((done == live.len(), parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, status: &str) -> LastRun {
        LastRun {
            run_id: id.into(),
            at: "t".into(),
            status: status.into(),
            rows: None,
            message: None,
        }
    }

    fn recon(run: Option<&str>, equal: bool) -> Reconciliation {
        Reconciliation {
            mart: "m".into(),
            at: "t".into(),
            run_id: run.map(str::to_string),
            measures: vec![Totals {
                measure: "total".into(),
                mart_total: "1".into(),
                silver_total: "1".into(),
                equal,
            }],
        }
    }

    #[test]
    fn a_mart_goes_from_requirement_to_reconciled() {
        let mut f = Facts::default();
        assert_eq!(state(&f), "requirement");
        f.approved = Some("d1".into());
        assert_eq!(state(&f), "to propose");
        f.proposed = Some("d0".into());
        assert_eq!(state(&f), "to propose", "a proposal of another description");
        f.proposed = Some("d1".into());
        assert_eq!(state(&f), "proposed");
        f.merged = true;
        assert_eq!(state(&f), "merged");
        f.run = Some(run("r1", "failed"));
        assert_eq!(state(&f), "failed");
        f.run = Some(run("r2", "ok"));
        assert_eq!(state(&f), "built");
        f.reconciliation = Some(recon(Some("r1"), true));
        assert_eq!(state(&f), "built", "an earlier build's reconciliation");
        f.reconciliation = Some(recon(Some("r2"), false));
        assert_eq!(state(&f), "diverged");
        f.reconciliation = Some(recon(Some("r2"), true));
        assert_eq!(state(&f), "reconciled");
    }

    #[test]
    fn totals_compare_as_numbers_and_the_bar_counts_marts() {
        assert!(totals_equal("10.50", "10.5"));
        assert!(!totals_equal("10.5", "10.51"));
        assert!(totals_equal("", ""));
        assert!(!totals_equal("", "0"));
        assert_eq!(step(&["requirement"]), None);
        assert_eq!(
            step(&["reconciled", "proposed", "requirement"]),
            Some((false, "1 of 2 mart(s) reconciled, 1 proposed".into()))
        );
        assert_eq!(
            step(&["reconciled"]),
            Some((true, "1 of 1 mart(s) reconciled".into()))
        );
        assert_eq!(mart_of("gold.sales"), Some("sales"));
    }
}

#[cfg(test)]
mod repo_tests {
    use super::*;

    #[test]
    fn a_committed_file_is_read_byte_for_byte() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path();
        git(repo, &["init", "-q"]).unwrap();
        std::fs::create_dir_all(repo.join("marts")).unwrap();
        std::fs::write(repo.join("marts/m.md"), "---\nmart: m\n---\n").unwrap();
        git(repo, &["add", "."]).unwrap();
        git(
            repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "x",
            ],
        )
        .unwrap();
        assert_eq!(
            file_at(repo, "HEAD", "marts/m.md").as_deref(),
            Some("---\nmart: m\n---\n"),
            "the trailing newline is part of the digest"
        );
        assert!(file_at(repo, "HEAD", "marts/none.md").is_none());
        assert_eq!(list(&repo.join("none"), repo), ["m"]);
    }
}
