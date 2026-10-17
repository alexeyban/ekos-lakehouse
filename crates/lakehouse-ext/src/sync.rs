//! `lakehouse sync` (RFC 0194): the live source against the deployed plan. Safe changes are
//! committed; the rest become proposal branches a person merges. The checkout the orchestrator
//! reads is never switched: proposals are built in a separate `git worktree`.

use crate::plan_cmd;
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_changes::{
    Action, Change, Policy, apply_auto, change_id, diff, proposal_markdown, with_table,
};
use ekos_lakehouse_silver::SilverSettings;
use ekos_ledger::KnowledgeStore;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Commits EKOS makes are its own, so a reviewer can tell them from a person's.
const AUTHOR: [&str; 4] = [
    "-c",
    "user.name=ekos-lakehouse",
    "-c",
    "user.email=ekos-lakehouse@localhost",
];

pub fn git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn commit_all(repo: &Path, message: &str) -> anyhow::Result<bool> {
    git(repo, &["add", "-A"])?;
    if git(repo, &["status", "--porcelain"])?.is_empty() {
        return Ok(false);
    }
    let mut args: Vec<&str> = AUTHOR.to_vec();
    args.extend(["commit", "-q", "-m", message]);
    git(repo, &args)?;
    Ok(true)
}

/// [`commit_all`] for the other commands that build branches (gold, RFC 0195).
pub fn commit_all_pub(repo: &Path, message: &str) -> anyhow::Result<bool> {
    commit_all(repo, message)
}

/// Make the pipeline repository a git repository on first generate, and commit.
pub fn commit_generated(repo: &Path, message: &str) -> anyhow::Result<bool> {
    if !repo.join(".git").exists() {
        git(repo, &["init", "-q", "-b", "main"])?;
    }
    commit_all(repo, message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableState {
    Active,
    /// A proposal is open; loads continue on the deployed schema.
    Pending,
    /// A proposal is open; the deployed load cannot run against the source.
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub table: String,
    pub branch: String,
    pub changes: Vec<Change>,
    pub new: bool,
    /// Rebuilt on the current HEAD because another proposal was merged first.
    pub rebuilt: bool,
    pub pull_request: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub applied: Vec<Change>,
    pub committed: bool,
    pub proposals: Vec<Proposal>,
    pub status: BTreeMap<String, (TableState, Option<String>)>,
}

/// Who reads the changed bronze column: silver models and their tests, from the ledger EKOS
/// compiled from this repository (RFC 0193 §6).
pub fn impact(store: &dyn KnowledgeStore, c: &Change) -> anyhow::Result<Vec<String>> {
    let table = c.table.rsplit('.').next().unwrap_or(&c.table);
    let mut out = vec![];
    for o in store.all_objects()? {
        let silver = o
            .properties
            .get("source_path")
            .and_then(|v| v.as_str())
            .is_some_and(|p| p.starts_with("models/silver/"));
        if !silver {
            continue;
        }
        let Some(lineage) = o
            .properties
            .get("column_lineage")
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        let tests_of = |col: &str| -> Vec<String> {
            o.properties
                .get("columns")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .find(|x| x["name"] == col)
                .and_then(|x| x["dbt_tests"].as_array())
                .map(|t| {
                    t.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        // A model that passes any column of this table through reads the table; a same-named
        // column it does not pass through is computed from it (a mask, a cast).
        let reads_table = lineage.values().any(|src| src[0] == table);
        if let Some(col) = c.column.as_deref()
            && reads_table
            && !lineage.contains_key(col)
            && o.properties
                .get("columns")
                .and_then(|v| v.as_array())
                .is_some_and(|cols| cols.iter().any(|x| x["name"] == col))
        {
            let tests = tests_of(col);
            out.push(format!(
                "silver `{}.{col}` is computed from it (masked or cast){}",
                o.name,
                if tests.is_empty() {
                    String::new()
                } else {
                    format!(" — tests: {}", tests.join(", "))
                }
            ));
        }
        for (out_col, src) in lineage {
            let reads = src[0] == table && c.column.as_deref().is_none_or(|col| src[1] == col);
            if reads {
                let tests = tests_of(out_col);
                out.push(format!(
                    "silver `{}.{out_col}` reads it{}",
                    o.name,
                    if tests.is_empty() {
                        String::new()
                    } else {
                        format!(" — tests: {}", tests.join(", "))
                    }
                ));
            }
        }
    }
    out.sort();
    if out.is_empty() && c.column.is_some() {
        out.push(
            "no compiled silver column reads it (compile the repository for a fresh view)".into(),
        );
    }
    Ok(out)
}

pub struct SyncInputs<'a> {
    pub repo: &'a Path,
    pub live: &'a Plan,
    pub silver: &'a SilverSettings,
    pub policy: &'a Policy,
    pub store: &'a dyn KnowledgeStore,
    /// Push proposal branches here and open pull requests with `gh` (RFC 0194 §4).
    pub git_remote: Option<&'a str>,
}

pub fn sync(i: SyncInputs<'_>) -> anyhow::Result<Outcome> {
    let repo = i.repo;
    if !repo.join(".git").exists() {
        anyhow::bail!(
            "{} is not a git repository: run `lakehouse generate` first",
            repo.display()
        );
    }
    let deployed = plan_cmd::read_lock(repo)?;
    let changes = diff(&deployed, i.live, i.policy);

    // Safe changes: regenerate and commit on the checked-out branch.
    let next = apply_auto(&deployed, i.live, &changes);
    let mut committed = false;
    if next != deployed {
        plan_cmd::write(&next, i.silver, repo)?;
        let applied: Vec<String> = changes
            .iter()
            .filter(|c| c.action == Action::Auto)
            .map(|c| {
                format!(
                    "{} {}{}",
                    c.class.name(),
                    c.table,
                    c.column
                        .as_ref()
                        .map(|x| format!(".{x}"))
                        .unwrap_or_default()
                )
            })
            .collect();
        committed = commit_all(
            repo,
            &format!(
                "lakehouse: apply {} safe change(s)\n\n{}",
                applied.len(),
                applied.join("\n")
            ),
        )?;
    }
    let applied: Vec<Change> = changes
        .iter()
        .filter(|c| {
            c.action == Action::Auto
                && !changes
                    .iter()
                    .any(|o| o.table == c.table && o.action == Action::Propose)
        })
        .cloned()
        .collect();

    // Everything else: one proposal per table.
    let mut by_table: BTreeMap<String, Vec<&Change>> = BTreeMap::new();
    for c in &changes {
        if changes
            .iter()
            .any(|o| o.table == c.table && o.action == Action::Propose)
        {
            by_table.entry(c.table.clone()).or_default().push(c);
        }
    }
    let base_branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let ours = |table: &str| -> anyhow::Result<Vec<String>> {
        Ok(git(
            repo,
            &[
                "branch",
                "--list",
                "--format=%(refname:short)",
                &format!("ekos/change/{table}/*"),
            ],
        )?
        .lines()
        .map(str::to_string)
        .collect())
    };
    let mut proposals = vec![];
    for (table, cs) in &by_table {
        let id = change_id(cs);
        let branch = format!("ekos/change/{table}/{id}");
        // EKOS owns its proposal branches: one superseded by a new set of changes is deleted,
        // and one whose base is no longer HEAD (another proposal was merged) is rebuilt, so
        // every open proposal merges cleanly. Its content is generated, so rebuilding is exact.
        for other in ours(table)? {
            if other != branch {
                git(repo, &["branch", "-D", &other])?;
            }
        }
        let exists = git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok();
        let current =
            exists && git(repo, &["merge-base", "--is-ancestor", "HEAD", &branch]).is_ok();
        if exists && !current {
            git(repo, &["branch", "-D", &branch])?;
        }
        let mut pull_request = None;
        if !current {
            let mut impact_lines = vec![];
            for c in cs {
                for line in impact(i.store, c)? {
                    if !impact_lines.contains(&line) {
                        impact_lines.push(line);
                    }
                }
            }
            let wt: PathBuf = std::env::temp_dir().join(format!("ekos-lh-proposal-{id}"));
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
                    wt.to_str().expect("utf-8 temp path"),
                    "HEAD",
                ],
            )?;
            let built = (|| -> anyhow::Result<()> {
                let proposed = with_table(&next, i.live, table);
                plan_cmd::write(&proposed, i.silver, &wt)?;
                std::fs::create_dir_all(wt.join("proposals"))?;
                std::fs::write(
                    wt.join(format!("proposals/{id}.md")),
                    proposal_markdown(&id, table, cs, &impact_lines),
                )?;
                commit_all(
                    &wt,
                    &format!(
                        "lakehouse: propose {} in {table}\n\nOpen proposals/{id}.md. Merging this branch is the approval.",
                        cs.iter()
                            .map(|c| c.class.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )?;
                Ok(())
            })();
            git(
                repo,
                &[
                    "worktree",
                    "remove",
                    "--force",
                    wt.to_str().expect("utf-8 temp path"),
                ],
            )?;
            built?;
            if let Some(remote) = i.git_remote {
                git(repo, &["push", "-q", "--force", "-u", remote, &branch])?;
                if !exists {
                    let out = Command::new("gh")
                        .current_dir(repo)
                        .args(["pr", "create", "--base", &base_branch, "--head", &branch])
                        .args(["--title", &format!("Schema change in {table} ({id})")])
                        .args(["--body-file", &format!("proposals/{id}.md")])
                        .output()?;
                    pull_request = Some(if out.status.success() {
                        String::from_utf8_lossy(&out.stdout).trim().to_string()
                    } else {
                        format!(
                            "not opened: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        )
                    });
                }
            }
        }
        proposals.push(Proposal {
            id,
            table: table.clone(),
            branch,
            changes: cs.iter().map(|c| (*c).clone()).collect(),
            new: !exists,
            rebuilt: exists && !current,
            pull_request,
        });
    }
    // A table with no open change keeps no proposal branch (its proposal was merged, or the
    // source went back).
    for b in git(
        repo,
        &[
            "branch",
            "--list",
            "--format=%(refname:short)",
            "ekos/change/*",
        ],
    )?
    .lines()
    {
        let table = b
            .trim_start_matches("ekos/change/")
            .rsplit_once('/')
            .map(|(t, _)| t);
        if table.is_some_and(|t| !by_table.contains_key(t)) {
            git(repo, &["branch", "-D", b])?;
        }
    }

    // Run-time state for the orchestrator: derived, never committed.
    let mut status = BTreeMap::new();
    for t in &next.tables {
        status.insert(
            format!("{}.{}", t.schema, t.table),
            (TableState::Active, None),
        );
    }
    for p in &proposals {
        let state = if p.changes.iter().any(|c| c.class.pauses_bronze()) {
            TableState::Paused
        } else {
            TableState::Pending
        };
        status.insert(p.table.clone(), (state, Some(p.branch.clone())));
    }
    let dir = repo.join(".lakehouse");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("status.json"),
        serde_json::to_string_pretty(&status)? + "\n",
    )?;

    Ok(Outcome {
        applied,
        committed,
        proposals,
        status,
    })
}
