//! `lakehouse gold draft` (RFC 0195): a mart description → questions, or a proposal branch.

use crate::plan_cmd::read_lock;
use crate::sync::git;
use ekos_lakehouse_gold::{self as gold, Question, Term};
use ekos_recovery::{LlmProvider, LlmRequest, OllamaProvider};
use std::collections::BTreeMap;
use std::path::Path;

pub struct DraftOutcome {
    pub mart: String,
    /// `(term, question)` when the mart could not be drafted.
    pub questions: Vec<(String, Question)>,
    pub branch: Option<String>,
    pub checked_atoms: usize,
    /// For each prose term: how many drafts the model needed.
    pub drafts: BTreeMap<String, usize>,
}

const SYSTEM: &str = "You write one SQL expression for Trino, nothing else: no SELECT, no FROM, \
no explanation, no code fence. Use only the columns, written exactly as listed (\"table\".\"column\"), \
and only the functions sum, count, avg, min, max. Count rows with count(*). Do not add constants, \
conditions or columns the request does not state.";

fn context(
    resolved: &[gold::Resolved],
    plan: &ekos_lakehouse_bronze::plan::Plan,
    tables: &[String],
) -> String {
    let mut s = String::from("Columns you may use:\n");
    for t in plan.tables.iter().filter(|t| tables.contains(&t.table)) {
        for m in &t.columns {
            let desc = t
                .column_facts(&m.column)
                .and_then(|f| f.comment.clone())
                .unwrap_or_default();
            s.push_str(&format!(
                "- \"{}\".\"{}\" ({}) {desc}\n",
                t.table, m.column, m.iceberg_type
            ));
        }
    }
    s.push_str("\nThe other terms of this mart:\n");
    for r in resolved {
        if let Ok(t) = &r.resolution
            && !matches!(t, Term::Prose { .. })
        {
            s.push_str(&format!("- {} = {}\n", r.term, r.written));
        }
    }
    s
}

fn clean(answer: &str) -> String {
    let a = answer.trim();
    let a = a
        .trim_start_matches("```sql")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    a.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(';')
        .to_string()
}

/// Whether Trino can plan the mart: `EXPLAIN` plans a statement without running it.
fn engine_accepts(trino: &ekos_lakehouse_trino::Client, sql: &str) -> Result<(), String> {
    trino
        .execute(&format!("EXPLAIN {sql}"))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub async fn draft(
    repo: &Path,
    description: &Path,
    model: &str,
    ollama_url: &str,
    trino: &ekos_lakehouse_trino::Connection,
) -> anyhow::Result<(DraftOutcome, Option<(String, BTreeMap<String, String>)>)> {
    let text = std::fs::read_to_string(description)
        .map_err(|e| anyhow::anyhow!("{}: {e}", description.display()))?;
    let spec = gold::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", description.display()))?;
    // A mart is built from a committed description: the approval then cites text a reviewer
    // can read in history, and the proposal branch never carries a copy that collides with the
    // author's working file.
    let in_repo = description
        .canonicalize()
        .ok()
        .zip(repo.canonicalize().ok())
        .and_then(|(d, r)| {
            d.strip_prefix(&r)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        });
    if let Some(rel) = &in_repo {
        let tracked = git(repo, &["ls-files", "--error-unmatch", rel]).is_ok();
        let clean = git(repo, &["diff", "--quiet", "HEAD", "--", rel]).is_ok();
        anyhow::ensure!(
            tracked && clean,
            "commit {rel} first: a gold mart is drafted from a committed description"
        );
    }
    let plan = read_lock(repo)?;
    let resolved = gold::resolve(&spec, &plan);
    let mut outcome = DraftOutcome {
        mart: spec.mart.clone(),
        questions: gold::questions(&resolved)
            .into_iter()
            .map(|(r, q)| (r.term.clone(), q.clone()))
            .collect(),
        branch: None,
        checked_atoms: 0,
        drafts: BTreeMap::new(),
    };
    if !outcome.questions.is_empty() {
        return Ok((outcome, None));
    }

    // Prose terms: a local model drafts, the anti-invention check judges, its findings go back.
    let provider = OllamaProvider::new(model, ollama_url);
    let mut drafted: BTreeMap<String, String> = BTreeMap::new();
    let tables: Vec<String> = match gold::compile(&spec, &plan, &resolved, &drafted) {
        Ok(c) => c.tables,
        Err(qs) => {
            outcome.questions = qs.into_iter().map(|q| ("mart".to_string(), q)).collect();
            return Ok((outcome, None));
        }
    };
    let ctx = context(&resolved, &plan, &tables);
    for r in &resolved {
        let Ok(Term::Prose { text, .. }) = &r.resolution else {
            continue;
        };
        let mut feedback = String::new();
        let mut accepted = false;
        for attempt in 1..=3 {
            let user = format!(
                "{ctx}\nWrite the SQL expression for the {} `{}`: {text}\n{feedback}",
                if r.section == "filters" {
                    "condition"
                } else {
                    "measure"
                },
                r.term
            );
            let resp = provider
                .complete(&LlmRequest {
                    system: SYSTEM,
                    user: &user,
                    prompt_version: "lakehouse-gold-2",
                    max_tokens: 200,
                    history: &[],
                })
                .await
                .map_err(|e| anyhow::anyhow!("drafting `{}` with {model}: {e}", r.term))?;
            let expr = clean(&resp.content);
            outcome.drafts.insert(r.term.clone(), attempt);
            if let Err(why) = gold::check_draft(r.resolution.as_ref().expect("prose"), &expr) {
                feedback = format!(
                    "\nYour last answer `{expr}` was rejected: {why}. Answer again with one SQL \
                     expression over the columns this term names."
                );
                continue;
            }
            let mut trial = drafted.clone();
            trial.insert(r.term.clone(), expr.clone());
            let c = gold::compile(&spec, &plan, &resolved, &trial)
                .map_err(|q| anyhow::anyhow!("{q:?}"))?;
            let report = gold::check(&c);
            if report.accepted() {
                // Nothing invented; now it must also run (RFC 0195: found when Trino refused
                // `COUNT("ar".*)`, which every atom of passed).
                let catalog = plan.settings.target_catalog.clone();
                let sql = gold::engine_sql(&c, &catalog);
                let conn = trino.clone();
                let verdict = tokio::task::spawn_blocking(move || {
                    let client =
                        ekos_lakehouse_trino::Client::new(conn).map_err(|e| e.to_string())?;
                    engine_accepts(&client, &sql)
                })
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
                match verdict {
                    Ok(()) => {
                        drafted.insert(r.term.clone(), expr);
                        accepted = true;
                        break;
                    }
                    Err(e) => {
                        feedback = format!(
                            "\nYour last answer `{expr}` was rejected by the engine: {e}. Answer again \
                             with the same meaning in SQL Trino accepts."
                        );
                        continue;
                    }
                }
            }
            feedback = format!(
                "\nYour last answer `{expr}` was rejected: it states {} that the description does not. \
                 Answer again without them.",
                match &report.parse_error {
                    Some(e) => format!("unparseable SQL ({e})"),
                    None => report
                        .unmapped
                        .iter()
                        .map(|a| format!("`{}`", a.text))
                        .collect::<Vec<_>>()
                        .join(", "),
                }
            );
        }
        if !accepted {
            outcome.questions.push((
                r.term.clone(),
                Question {
                    text: format!(
                        "`{text}` could not be written without stating something the description does not \
                         (the model's last attempt was rejected: {}). Write it as `sum|count|… of table.column`, \
                         or name the columns and conditions it needs.",
                        feedback.trim().trim_start_matches("Your last answer ")
                    ),
                    candidates: vec![],
                },
            ));
        }
    }
    if !outcome.questions.is_empty() {
        return Ok((outcome, None));
    }
    let c =
        gold::compile(&spec, &plan, &resolved, &drafted).map_err(|q| anyhow::anyhow!("{q:?}"))?;
    let report = gold::check(&c);
    anyhow::ensure!(
        report.accepted(),
        "the compiled mart invents {:?}",
        report.unmapped
    );
    outcome.checked_atoms = report.checked;
    // The whole mart plans on the engine: a compiled mart that does not is a bug here.
    let sql = gold::engine_sql(&c, &plan.settings.target_catalog);
    let conn = trino.clone();
    tokio::task::spawn_blocking(move || {
        let client = ekos_lakehouse_trino::Client::new(conn).map_err(|e| e.to_string())?;
        engine_accepts(&client, &sql)
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?
    .map_err(|e| anyhow::anyhow!("Trino cannot plan the mart: {e}"))?;

    // The proposal branch (RFC 0195 §6), built in a worktree like RFC 0194's.
    let id = gold::mart_id(&c);
    let branch = format!("ekos/gold/{}/{id}", spec.mart);
    for other in git(
        repo,
        &[
            "branch",
            "--list",
            "--format=%(refname:short)",
            &format!("ekos/gold/{}/*", spec.mart),
        ],
    )?
    .lines()
    {
        if other != branch {
            git(repo, &["branch", "-D", other])?;
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
    if !exists {
        let wt = std::env::temp_dir().join(format!("ekos-lh-gold-{id}"));
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
        let built = (|| -> anyhow::Result<()> {
            let write = |rel: &str, body: &str| -> anyhow::Result<()> {
                let p = wt.join(rel);
                std::fs::create_dir_all(p.parent().expect("a parent"))?;
                std::fs::write(p, body)?;
                Ok(())
            };
            if in_repo.is_none() {
                // A description kept outside the repository travels with its proposal.
                write(&format!("marts/{}.md", spec.mart), &text)?;
            }
            write(
                &format!("dbt/models/gold/gold__{}.sql", spec.mart),
                &c.dbt_sql,
            )?;
            write(
                &format!("dbt/models/gold/_gold__{}.yml", spec.mart),
                &gold::model_yaml(&spec, &resolved),
            )?;
            for (name, sql) in gold::reconciliation_tests(&spec, &resolved, &c) {
                write(&format!("dbt/tests/gold/{name}.sql"), &sql)?;
            }
            write(
                &format!("proposals/gold-{}.md", spec.mart),
                &gold::proposal_markdown(&spec, &resolved, &c, &report, &drafted),
            )?;
            crate::sync::commit_all_pub(
                &wt,
                &format!(
                    "lakehouse: propose gold mart {}\n\nOpen proposals/gold-{}.md. Merging is the approval.",
                    spec.mart, spec.mart
                ),
            )?;
            Ok(())
        })();
        git(
            repo,
            &["worktree", "remove", "--force", wt.to_str().expect("utf-8")],
        )?;
        built?;
    }
    outcome.branch = Some(branch);
    Ok((outcome, Some((c.dbt_sql, drafted))))
}
