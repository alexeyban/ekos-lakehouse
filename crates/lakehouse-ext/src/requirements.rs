//! EKOS RFC 0215 (step 4): mart requirements — drafted from text by the configured model or
//! written as a description, resolved against the deployed plan, edited by the operator, and
//! approved into the pipeline repository. The model drafts; it never answers a question.

use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_gold::requirements::{self as req, Edit};
use ekos_lakehouse_gold::{self as gold, MartSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn state_dir(config: &ekos_compiler_core::EkosConfig, cwd: &Path) -> PathBuf {
    config.ekos_dir(cwd).join("lakehouse").join("requirements")
}

/// One model answer: what came back and why it was not used, if it was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub answer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Logged {
    pub at: String,
    pub by: String,
    pub change: String,
}

/// What is kept beside a draft: where it came from and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub mart: String,
    /// `text` (drafted by a model) or `file` (written as a description).
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<Attempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<Logged>,
    /// As of the last command that touched it, for the workflow bar.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub questions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub mart: String,
    pub by: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// SHA-256 of the approved description, as committed to `marts/<mart>.md`.
    pub digest: String,
    /// The silver tables the mart reads.
    pub needs: Vec<String>,
}

pub fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn valid_name(mart: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !mart.is_empty()
            && mart
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "mart name {mart:?}: lowercase letters, digits and _ only"
    );
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).map_err(|e| {
            anyhow::anyhow!("{} is not readable: {e}", path.display())
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_json<T: Serialize>(path: &Path, v: &T) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().expect("a parent"))?;
    std::fs::write(path, serde_json::to_string_pretty(v)? + "\n")?;
    Ok(())
}

/// A draft: its description text, the spec it parses to, and its record.
pub struct Draft {
    pub text: String,
    pub spec: MartSpec,
    pub record: Record,
}

pub fn read(dir: &Path, mart: &str) -> anyhow::Result<Option<Draft>> {
    let md = dir.join(format!("{mart}.md"));
    let text = match std::fs::read_to_string(&md) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let spec = gold::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", md.display()))?;
    let record = read_json(&dir.join(format!("{mart}.json")))?.unwrap_or(Record {
        mart: mart.into(),
        source: "file".into(),
        request: None,
        model: None,
        attempts: vec![],
        log: vec![],
        state: String::new(),
        questions: 0,
    });
    Ok(Some(Draft { text, spec, record }))
}

pub fn write(dir: &Path, spec: &MartSpec, record: &Record) -> anyhow::Result<String> {
    let text = req::render(spec);
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{}.md", spec.mart)), &text)?;
    write_json(&dir.join(format!("{}.json", spec.mart)), record)?;
    Ok(text)
}

pub fn read_approval(dir: &Path, mart: &str) -> anyhow::Result<Option<Approval>> {
    read_json(&dir.join(format!("{mart}.approved.json")))
}

pub fn write_approval(dir: &Path, a: &Approval) -> anyhow::Result<()> {
    write_json(&dir.join(format!("{}.approved.json", a.mart)), a)
}

/// Every mart with a draft, by name.
pub fn list(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_suffix(".md").map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

/// A draft resolved against the plan: the terms, the questions, the tables it reads.
#[derive(Debug, Clone, Serialize)]
pub struct Evaluation {
    pub resolved: Vec<gold::Resolved>,
    /// `(term, question)`.
    pub questions: Vec<(String, gold::Question)>,
    pub needs: Vec<String>,
}

pub fn evaluate(spec: &MartSpec, plan: &Plan) -> Evaluation {
    let resolved = gold::resolve(spec, plan);
    let mut questions: Vec<(String, gold::Question)> = gold::questions(&resolved)
        .into_iter()
        .map(|(r, q)| (r.term.clone(), q.clone()))
        .collect();
    let mut needs = vec![];
    if questions.is_empty() {
        match gold::compile(spec, plan, &resolved, &Default::default()) {
            Ok(c) => needs = c.tables,
            Err(qs) => questions.extend(qs.into_iter().map(|q| ("the joins".to_string(), q))),
        }
    }
    Evaluation {
        resolved,
        questions,
        needs,
    }
}

/// `questions`, `resolved` (ready to approve), `approved`, `changed` (edited since its
/// approval) or `stale` (the committed description is no longer the approved one).
pub fn state(
    draft_text: &str,
    ev: &Evaluation,
    approval: Option<&Approval>,
    committed: Option<&str>,
) -> &'static str {
    if !ev.questions.is_empty() {
        return "questions";
    }
    match approval {
        None => "resolved",
        Some(a) if sha256(draft_text) != a.digest => "changed",
        Some(a) if committed.map(sha256).as_deref() != Some(a.digest.as_str()) => "stale",
        Some(_) => "approved",
    }
}

pub fn committed(repo: &Path, mart: &str) -> Option<String> {
    std::fs::read_to_string(repo.join("marts").join(format!("{mart}.md"))).ok()
}

/// RFC 0215 §2.4: `gold draft` under require-plan-approval builds only an approved description.
pub fn require_approved(dir: &Path, description_text: &str) -> anyhow::Result<()> {
    let spec = gold::parse(description_text).map_err(|e| anyhow::anyhow!("{e}"))?;
    match read_approval(dir, &spec.mart)? {
        Some(a) if a.digest == sha256(description_text) => Ok(()),
        Some(a) => anyhow::bail!(
            "mart {} differs from the description {} approved at {}: approve it again \
             (`lakehouse requirements approve --mart {}`)",
            spec.mart,
            a.by,
            a.at,
            spec.mart
        ),
        None => anyhow::bail!(
            "mart {} has no approved requirement: draft and approve it first \
             (`lakehouse requirements draft|approve --mart {}`)",
            spec.mart,
            spec.mart
        ),
    }
}

// ─────────────────────────────── the model ───────────────────────────────

pub const SYSTEM: &str = "You turn a request for a report into mart metadata. Answer with only \
a YAML header between two lines of three dashes (---), nothing before or after it, no code \
fence. Keys: mart, grain, dimensions, measures, filters. Write every table and column exactly as \
listed (table.column). The only forms allowed:\n\
- a dimension: table.column, or day|week|month|quarter|year of table.column\n\
- a measure: sum|count|average|min|max of table.column, count distinct table.column, or count of \
table\n\
- a filter: table.column is true|false|null|not null, or table.column = <> < <= > >= a number or \
a 'quoted' code from the list\n\
No SQL functions, no dates relative to today, no filter the request does not ask for. Every grain \
entry names a dimension. An example for another database:\n\
---\n\
mart: rentals_by_store\n\
grain: [month, store]\n\
dimensions:\n  month: month of rental.rental_date\n  store: store.name\n\
measures:\n  rentals: count of rental\n  revenue: sum of payment.amount\n\
filters:\n  - rental.returned is true\n\
---";

pub fn prompt(excerpt: &str, request: &str, feedback: &str) -> String {
    format!("Tables you may use:\n\n{excerpt}\nRequest: {request}\n{feedback}")
}

/// Draft `request` with `provider`: up to three answers, each one that does not parse as a
/// header sent back with the reason. Returns the spec and every attempt.
pub async fn draft_text(
    provider: &dyn ekos_recovery::LlmProvider,
    mart: &str,
    request: &str,
    excerpt: &str,
) -> anyhow::Result<(Option<MartSpec>, Vec<Attempt>)> {
    let mut attempts = vec![];
    let mut feedback = String::new();
    for _ in 0..3 {
        let user = prompt(excerpt, request, &feedback);
        let resp = provider
            .complete(&ekos_recovery::LlmRequest {
                system: SYSTEM,
                user: &user,
                prompt_version: "lakehouse-requirements-1",
                max_tokens: 600,
                history: &[],
            })
            .await
            .map_err(|e| anyhow::anyhow!("drafting {mart} with {}: {e}", provider.model_name()))?;
        match req::from_answer(&resp.content, mart, request) {
            Ok(spec) => {
                attempts.push(Attempt {
                    answer: resp.content,
                    rejected: None,
                });
                return Ok((Some(spec), attempts));
            }
            Err(why) => {
                feedback = format!(
                    "\nYour last answer could not be read: {why}. Answer again with only the \
                     header between `---` lines."
                );
                attempts.push(Attempt {
                    answer: resp.content,
                    rejected: Some(why),
                });
            }
        }
    }
    Ok((None, attempts))
}

// ─────────────────────────────── edits ───────────────────────────────

pub struct EditArgs<'a> {
    pub term: Option<&'a String>,
    pub value: Option<&'a String>,
    pub dimension: bool,
    pub measure: bool,
    pub drop_term: Option<&'a String>,
    pub filter: Option<&'a String>,
    pub drop_filter: Option<&'a String>,
}

pub fn edit_from(a: EditArgs<'_>) -> anyhow::Result<Edit> {
    let (term, value, dimension, measure, filter, drop_filter) = (
        a.term,
        a.value,
        a.dimension,
        a.measure,
        a.filter,
        a.drop_filter,
    );
    if let Some(t) = a.drop_term {
        anyhow::ensure!(
            term.is_none() && value.is_none() && filter.is_none() && drop_filter.is_none(),
            "--drop-term alone"
        );
        return Ok(Edit::DropTerm(t.trim().to_string()));
    }
    Ok(match (term, value, filter, drop_filter) {
        (Some(t), Some(v), None, None) => Edit::Term {
            name: t.trim().to_string(),
            value: v.clone(),
            section: match (dimension, measure) {
                (true, false) => Some("dimensions".into()),
                (false, true) => Some("measures".into()),
                (false, false) => None,
                _ => anyhow::bail!("--dimension or --measure, not both"),
            },
        },
        (None, None, Some(f), None) => Edit::AddFilter(f.clone()),
        (None, None, None, Some(f)) => Edit::DropFilter(f.clone()),
        _ => anyhow::bail!("--term with --value, --drop-term, --filter or --drop-filter"),
    })
}

// ─────────────────────────────── show ───────────────────────────────

fn terms_json(ev: &Evaluation) -> Vec<Value> {
    ev.resolved
        .iter()
        .map(|r| {
            let (question, candidates) = match &r.resolution {
                Err(q) => (q.text.clone(), q.candidates.join(", ")),
                Ok(_) => (String::new(), String::new()),
            };
            json!({
                "section": r.section,
                "term": r.term,
                "written": r.written,
                "resolved": if r.resolution.is_ok() { "yes" } else { "no" },
                "how": r.how,
                "question": question,
                "candidates": candidates,
            })
        })
        .collect()
}

fn markdown(d: &Draft, ev: &Evaluation, state: &str, a: Option<&Approval>) -> String {
    let mut s = format!("### {} — {state}\n\n", d.spec.mart);
    if let Some(r) = &d.record.request {
        s.push_str(&format!("**Request:** {r}\n\n"));
    }
    if let Some(m) = &d.record.model {
        s.push_str(&format!(
            "Drafted by `{m}` in {} attempt(s); the model's draft is resolved by EKOS, never \
             trusted as written.\n\n",
            d.record.attempts.len()
        ));
    }
    s.push_str(&format!("```yaml\n{}```\n\n", d.text));
    if ev.questions.is_empty() {
        s.push_str("Every term resolves.\n\n");
    } else {
        s.push_str("**Questions** (answer them by changing a term):\n\n");
        for (t, q) in &ev.questions {
            s.push_str(&format!("- `{t}`: {}", q.text));
            if !q.candidates.is_empty() {
                s.push_str(&format!(" Candidates: {}.", q.candidates.join(", ")));
            }
            s.push('\n');
        }
        s.push('\n');
    }
    if !ev.needs.is_empty() {
        s.push_str(&format!(
            "**Silver tables it reads:** {}\n\n",
            ev.needs.join(", ")
        ));
    }
    if let Some(a) = a {
        s.push_str(&format!(
            "Approved by {} at {} (digest `{}`){}.\n\n",
            a.by,
            a.at,
            &a.digest[..16],
            a.note
                .as_ref()
                .map(|n| format!(": {n}"))
                .unwrap_or_default()
        ));
    }
    if !d.record.log.is_empty() {
        s.push_str("**Changes:**\n\n");
        for l in &d.record.log {
            s.push_str(&format!("- {} {}: {}\n", l.at, l.by, l.change));
        }
    }
    s
}

/// The page's JSON: every mart with its state, the tables all of them read, and the selected
/// mart in full.
pub fn show(
    dir: &Path,
    repo: &Path,
    plan: Option<&Plan>,
    selected: Option<&str>,
) -> anyhow::Result<Value> {
    let mut marts = vec![];
    let mut all_needs: std::collections::BTreeSet<String> = Default::default();
    let mut sel = Value::Null;
    for mart in list(dir) {
        let Some(d) = read(dir, &mart)? else { continue };
        let a = read_approval(dir, &mart)?;
        let ev = match plan {
            Some(p) => evaluate(&d.spec, p),
            None => Evaluation {
                resolved: vec![],
                questions: vec![(
                    "plan".into(),
                    gold::Question {
                        text: "no deployed plan: run `lakehouse generate` first".into(),
                        candidates: vec![],
                    },
                )],
                needs: vec![],
            },
        };
        let st = state(&d.text, &ev, a.as_ref(), committed(repo, &mart).as_deref());
        all_needs.extend(ev.needs.iter().cloned());
        marts.push(json!({
            "mart": mart,
            "state": st,
            "source": d.record.source,
            "terms": ev.resolved.len(),
            "questions": ev.questions.len(),
            "needs": ev.needs.len(),
            "approved_by": a.as_ref().map(|a| a.by.clone()),
            "approved_at": a.as_ref().map(|a| a.at.clone()),
        }));
        if selected == Some(mart.as_str()) {
            sel = json!({
                "mart": mart,
                "state": st,
                "markdown": markdown(&d, &ev, st, a.as_ref()),
                "terms": terms_json(&ev),
                "needs": ev.needs,
            });
        }
    }
    Ok(json!({
        "marts": marts,
        "needs_rows": all_needs.iter().map(|t| json!({"table": t})).collect::<Vec<_>>(),
        "needs": all_needs.into_iter().collect::<Vec<_>>(),
        "selected": sel,
    }))
}

pub fn render(v: &Value) -> String {
    let mut s = String::new();
    let marts = v["marts"].as_array().cloned().unwrap_or_default();
    if marts.is_empty() {
        s.push_str("no requirements yet: `lakehouse requirements draft --mart <name> --text …`\n");
    }
    for m in &marts {
        s.push_str(&format!(
            "{:<32} {:<10} {} term(s), {} question(s), reads {} table(s)\n",
            m["mart"].as_str().unwrap_or_default(),
            m["state"].as_str().unwrap_or_default(),
            m["terms"],
            m["questions"],
            m["needs"]
        ));
    }
    if let Some(md) = v["selected"]["markdown"].as_str() {
        s.push('\n');
        s.push_str(md);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> MartSpec {
        gold::parse(text).unwrap()
    }

    #[test]
    fn a_requirement_moves_from_questions_to_approved_and_back() {
        let ev_ok = Evaluation {
            resolved: vec![],
            questions: vec![],
            needs: vec!["C_ORDER".into()],
        };
        let ev_q = Evaluation {
            questions: vec![(
                "x".into(),
                gold::Question {
                    text: "?".into(),
                    candidates: vec![],
                },
            )],
            ..ev_ok.clone()
        };
        let text = "---\nmart: m\n---\n";
        assert_eq!(state(text, &ev_q, None, None), "questions");
        assert_eq!(state(text, &ev_ok, None, None), "resolved");
        let a = Approval {
            mart: "m".into(),
            by: "op".into(),
            at: "t".into(),
            note: None,
            digest: sha256(text),
            needs: vec![],
        };
        assert_eq!(state(text, &ev_ok, Some(&a), Some(text)), "approved");
        assert_eq!(
            state("---\nmart: m2\n---\n", &ev_ok, Some(&a), Some(text)),
            "changed"
        );
        assert_eq!(state(text, &ev_ok, Some(&a), Some("other")), "stale");
        assert_eq!(state(text, &ev_q, Some(&a), Some(text)), "questions");
    }

    #[test]
    fn drafts_are_kept_apart_from_the_repository_and_gold_needs_an_approval() {
        let dir = tempfile::tempdir().unwrap();
        let s = spec(
            "---\nmart: sales\ngrain: [m]\ndimensions:\n  m: month of o.d\nmeasures:\n  t: sum of o.x\n---\n\nSales.\n",
        );
        let rec = Record {
            mart: "sales".into(),
            source: "text".into(),
            request: Some("Sales.".into()),
            model: Some("llama3".into()),
            attempts: vec![],
            log: vec![],
            state: String::new(),
            questions: 0,
        };
        let text = write(dir.path(), &s, &rec).unwrap();
        assert_eq!(list(dir.path()), ["sales"]);
        let back = read(dir.path(), "sales").unwrap().unwrap();
        assert_eq!(back.spec, s);
        assert_eq!(back.record, rec);
        assert!(require_approved(dir.path(), &text).is_err());
        write_approval(
            dir.path(),
            &Approval {
                mart: "sales".into(),
                by: "op".into(),
                at: "t".into(),
                note: None,
                digest: sha256(&text),
                needs: vec![],
            },
        )
        .unwrap();
        assert!(require_approved(dir.path(), &text).is_ok());
        assert!(require_approved(dir.path(), &text.replace("Sales.", "Other.")).is_err());
        assert!(valid_name("Sales").is_err());
    }

    #[test]
    fn edits_come_from_the_flags() {
        let t = "t".to_string();
        let v = "c.x".to_string();
        let args = |term, value, dimension, filter, drop_filter, drop_term| EditArgs {
            term,
            value,
            dimension,
            measure: false,
            drop_term,
            filter,
            drop_filter,
        };
        assert_eq!(
            edit_from(args(Some(&t), Some(&v), true, None, None, None)).unwrap(),
            Edit::Term {
                name: "t".into(),
                value: "c.x".into(),
                section: Some("dimensions".into())
            }
        );
        assert!(edit_from(args(Some(&t), None, false, None, None, None)).is_err());
        assert!(edit_from(args(None, None, false, Some(&v), Some(&v), None)).is_err());
        assert_eq!(
            edit_from(args(None, None, false, None, None, Some(&t))).unwrap(),
            Edit::DropTerm("t".into())
        );
        assert!(edit_from(args(Some(&t), None, false, None, None, Some(&t))).is_err());
    }
}
