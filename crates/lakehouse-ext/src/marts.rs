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

/// The mart's description from the repository, resolved — with the instructions in force on
/// the tables it reads (EKOS RFC 0257 §3.4) — and compiled against the plan.
pub fn compiled(
    repo: &Path,
    plan: &Plan,
    mart: &str,
    instructions: &[gold::Instruction],
) -> anyhow::Result<(MartSpec, Vec<gold::Resolved>, gold::Compiled)> {
    let text = file_at(repo, "HEAD", &description_path(mart))
        .ok_or_else(|| anyhow::anyhow!("{} is not committed", description_path(mart)))?;
    let spec = gold::parse(&text).map_err(|e| anyhow::anyhow!("{mart}: {e}"))?;
    let (spec, resolved, _) = gold::resolve_with(&spec, plan, instructions);
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

// ─────────────────────────────── notes on silver tables (EKOS RFC 0257 §3.4) ───────────────────────────────

/// RFC 0247 annotations on the silver tables a project's marts read, from its ledger.
#[derive(Debug, Clone, Default)]
pub struct Notes {
    /// The instructions that apply to every mart: shared, approved, still valid, and from the
    /// rules that state a predicate (`default_filter`, `exclude`).
    pub instructions: Vec<gold::Instruction>,
    /// Every current annotation per table (lower-case source name), as the steps show them.
    pub by_table: std::collections::BTreeMap<String, Vec<Value>>,
    /// Annotation view per annotation id.
    pub by_id: std::collections::BTreeMap<String, Value>,
}

impl Notes {
    /// The instruction each instruction-made filter term came from, by the term.
    pub fn by_term(&self) -> std::collections::BTreeMap<String, Vec<Value>> {
        let mut out: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
        for i in &self.instructions {
            for f in i.filters() {
                if let Some(v) = self.by_id.get(&i.id) {
                    out.entry(f).or_default().push(v.clone());
                }
            }
        }
        out
    }
}

fn op_sql(op: ekos_semantic::annotations::Op) -> &'static str {
    use ekos_semantic::annotations::Op;
    match op {
        Op::Eq => "=",
        Op::Ne => "<>",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
    }
}

/// The plan table an annotated object is: a `Table` whose name's last part is a planned table.
fn plan_table<'a>(plan: &'a Plan, o: &ekos_kir::KirObject) -> Option<&'a str> {
    let last = o
        .name
        .rsplit('.')
        .next()
        .unwrap_or(&o.name)
        .trim_matches('"');
    plan.tables
        .iter()
        .find(|t| t.table.eq_ignore_ascii_case(last))
        .map(|t| t.table.as_str())
}

/// The annotations on the plan's tables and the instructions among them that apply to marts.
/// Only a shared, approved instruction applies (a mart is everyone's); a personal one and a
/// comment are shown, never applied. Pure over the ledger's objects.
pub fn notes(objects: &[ekos_kir::KirObject], plan: &Plan) -> Notes {
    use ekos_semantic::annotations::{self as an, Rule};
    let digest = |id: &str| -> Option<String> {
        objects
            .iter()
            .find(|o| o.id.0.to_string() == id)
            .map(an::digest_of_object)
    };
    let all = an::current(objects);
    let applies: Vec<String> = an::applicable(&all, "", &digest)
        .iter()
        .map(|a| a.id.0.to_string())
        .collect();
    let mut out = Notes::default();
    for a in &all {
        let Some(target) = objects.iter().find(|o| o.id.0.to_string() == a.target) else {
            continue;
        };
        let Some(table) = plan_table(plan, target) else {
            continue;
        };
        let state = match an::effective(a, digest(&a.target).as_deref()) {
            an::Effective::Active => "active",
            an::Effective::Stale => "stale",
            an::Effective::Withdrawn => "withdrawn",
        };
        let id = a.id.0.to_string();
        let rule = match &a.rule {
            Some(Rule::DefaultFilter { column, op, value }) => {
                Some(gold::InstructionRule::Filter {
                    column: column.clone(),
                    op: op_sql(*op).into(),
                    value: value.clone(),
                })
            }
            Some(Rule::Exclude { column, values }) => Some(gold::InstructionRule::Exclude {
                column: column.clone(),
                values: values.clone(),
            }),
            _ => None,
        };
        let applied = applies.contains(&id) && rule.is_some();
        let view = json!({
            "id": id,
            "target": a.target,
            "table": table,
            "kind": a.kind.as_str(),
            "text": a.text,
            "rule": a.rule,
            "author": a.author,
            "approved_by": a.approved_by,
            "state": state,
            "applies_to_marts": applied,
        });
        if let (true, Some(rule)) = (applied, rule) {
            out.instructions.push(gold::Instruction {
                id: id.clone(),
                author: a.author.clone(),
                table: table.to_string(),
                rule,
            });
        }
        out.by_table
            .entry(table.to_ascii_lowercase())
            .or_default()
            .push(view.clone());
        out.by_id.insert(id, view);
    }
    out
}

// ─────────────────────────────── the chain (EKOS RFC 0257) ───────────────────────────────

/// Which SQL a chain is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// The model on the repository's current branch, else on its proposal branch: what runs.
    Committed,
    /// What the compiler writes from the committed description now.
    Proposed,
}

/// The mart's chain of steps (`ekos.transformation-graph/1`), every step joined to its terms.
pub fn chain(
    repo: &Path,
    plan: &Plan,
    mart: &str,
    version: Version,
    notes: &Notes,
) -> anyhow::Result<(
    ekos_semantic::transform_chain::ChainGraph,
    ekos_semantic::transform_ir::TransformGraph,
)> {
    let (spec, resolved, c) = compiled(repo, plan, mart, &notes.instructions)?;
    let path = model_path(mart);
    let (sql, label) = match version {
        Version::Committed => (
            file_at(repo, "HEAD", &path)
                .or_else(|| proposal_branch(repo, mart).and_then(|b| file_at(repo, &b, &path)))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "{path} is neither merged nor proposed: `lakehouse marts propose`"
                    )
                })?,
            "committed",
        ),
        Version::Proposed => (c.dbt_sql.clone(), "proposed"),
    };
    crate::mart_graph::chain_ir(&crate::mart_graph::MartSql {
        mart,
        path: &path,
        dbt_sql: &sql,
        version: label,
        grain: &spec.grain,
        resolved: &resolved,
        clauses: &c.clauses,
        notes_by_table: notes.by_table.clone(),
        notes_by_term: notes.by_term(),
    })
}

// ─────────────────────────────── show ───────────────────────────────

fn selected(
    repo: &Path,
    plan: &Plan,
    mart: &str,
    st: &str,
    f: &Facts,
    instructions: &[gold::Instruction],
) -> Value {
    let sql = file_at(repo, "HEAD", &model_path(mart))
        .or_else(|| proposal_branch(repo, mart).and_then(|b| file_at(repo, &b, &model_path(mart))));
    let mut md = format!("### {mart} — {st}\n\n");
    md.push_str(
        "Generated from the approved requirement. The SQL is a template: terms marked \
         `drafted` were written by a model and need a person to read them.\n\n",
    );
    let Ok((spec, resolved, c)) = compiled(repo, plan, mart, instructions) else {
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
    instructions: &[gold::Instruction],
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
            sel = selected(repo, p, &mart, st, &f, instructions);
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

/// EKOS RFC 0257 §4 phase 2.
#[cfg(test)]
mod note_tests {
    use super::*;
    use ekos_kir::{KirObject, ObjectKind};
    use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
    use ekos_lakehouse_bronze::{ForeignKey, SourceColumn, SourceTable, TypeKind};
    use ekos_semantic::annotations::{self as an, AnnotationKind, NewAnnotation, Rule, Scope};

    fn col(n: &str, trino: &str) -> SourceColumn {
        SourceColumn {
            name: n.into(),
            pg_type: "x".into(),
            kind: TypeKind::Base,
            not_null: false,
            trino_type: Some(trino.into()),
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 0,
            has_default: false,
        }
    }

    fn orders() -> Plan {
        let t = |name: &str, cols: Vec<SourceColumn>, fks: Vec<(&str, &str, &str)>| SourceTable {
            schema: "ADEMPIERE".into(),
            oid: 0,
            name: name.into(),
            columns: cols,
            primary_key: vec![format!("{name}_ID")],
            row_estimate: 10,
            update_triggers: vec![],
            foreign_keys: fks
                .into_iter()
                .map(|(c, t, rc)| ForeignKey {
                    name: format!("{name}_{c}"),
                    columns: vec![c.into()],
                    ref_schema: "ADEMPIERE".into(),
                    ref_table: t.into(),
                    ref_columns: vec![rc.into()],
                })
                .collect(),
            comment: None,
        };
        plan(
            &[
                t(
                    "C_ORDER",
                    vec![
                        col("C_ORDER_ID", "decimal(10,0)"),
                        col("C_BPARTNER_ID", "decimal(10,0)"),
                        col("GRANDTOTAL", "decimal(38,10)"),
                        col("DOCSTATUS", "varchar(2)"),
                        col("DATEORDERED", "timestamp(0)"),
                    ],
                    vec![("C_BPARTNER_ID", "C_BPARTNER", "C_BPARTNER_ID")],
                ),
                t(
                    "C_BPARTNER",
                    vec![
                        col("C_BPARTNER_ID", "decimal(10,0)"),
                        col("NAME", "varchar(60)"),
                    ],
                    vec![],
                ),
            ],
            &PlanSettings::default(),
        )
    }

    fn table_object() -> KirObject {
        let mut o = KirObject::new("silver_ADEMPIERE.C_ORDER", ObjectKind::Table);
        o.properties.insert(
            "columns".into(),
            json!([{"name": "DOCSTATUS"}, {"name": "GRANDTOTAL"}]),
        );
        o
    }

    fn instruction(t: &KirObject, author: &str, scope: Scope, values: &[&str]) -> NewAnnotation {
        NewAnnotation {
            target: t.id.0.to_string(),
            kind: AnnotationKind::Instruction,
            text: "voided and reversed orders are not sales".into(),
            rule: Some(Rule::Exclude {
                column: "DOCSTATUS".into(),
                values: values.iter().map(|v| v.to_string()).collect(),
            }),
            author: author.into(),
            scope,
            term: None,
            target_digest: an::digest_of_object(t),
            at: "2026-11-12T00:00:00Z".into(),
        }
    }

    const DESCRIPTION: &str = "---\nmart: sales\ngrain: [month]\ndimensions:\n  month: month of \
        c_order.dateordered\nmeasures:\n  total: sum of c_order.grandtotal\nfilters:\n  - \
        c_order.docstatus = 'CO'\n---\n";

    fn draw(notes: &Notes) -> ekos_semantic::transform_chain::ChainGraph {
        let p = orders();
        let spec = gold::parse(DESCRIPTION).unwrap();
        let (spec, resolved, _) = gold::resolve_with(&spec, &p, &notes.instructions);
        let c = gold::compile(&spec, &p, &resolved, &Default::default()).unwrap();
        assert!(gold::check(&c).accepted());
        crate::mart_graph::chain(&crate::mart_graph::MartSql {
            mart: "sales",
            path: "dbt/models/gold/gold__sales.sql",
            dbt_sql: &c.dbt_sql,
            version: "proposed",
            grain: &spec.grain,
            resolved: &resolved,
            clauses: &c.clauses,
            notes_by_table: notes.by_table.clone(),
            notes_by_term: notes.by_term(),
        })
        .unwrap()
    }

    /// An instruction added on a step changes the next resolution of the term it is on, and is
    /// shown on the step.
    #[test]
    fn an_approved_instruction_on_a_step_changes_the_next_resolution_and_is_shown() {
        let p = orders();
        let t = table_object();
        let before = draw(&notes(std::slice::from_ref(&t), &p));
        let filters = |g: &ekos_semantic::transform_chain::ChainGraph| {
            g.nodes
                .iter()
                .filter(|n| n.node_type == "Filter")
                .map(|n| n.term.clone().unwrap_or_default())
                .collect::<Vec<_>>()
        };
        assert_eq!(filters(&before), ["c_order.docstatus = 'CO'"]);

        // Written on the step that reads C_ORDER, shared: shown, not applied until approved.
        let a = an::annotation_object(&instruction(&t, "ana", Scope::Shared, &["VO", "RE"]));
        let mut objects = vec![t.clone(), a.clone()];
        let n = notes(&objects, &p);
        assert!(
            n.instructions.is_empty(),
            "an unapproved instruction applies to nobody"
        );
        let g = draw(&n);
        assert_eq!(filters(&g), ["c_order.docstatus = 'CO'"]);
        let source = g.nodes.iter().find(|n| n.node_type == "Source").unwrap();
        assert_eq!(source.annotations.len(), 1, "shown on the step");
        assert_eq!(source.annotations[0]["applies_to_marts"], false);

        // Approved: the next resolution carries it, each value its own step, mapped and noted.
        let ann = an::current(&objects)[0].clone();
        let approved =
            an::approve_object(&ann, "bo", &ann.target_digest, "2026-11-12T01:00:00Z").unwrap();
        objects.push(approved);
        let n = notes(&objects, &p);
        assert_eq!(n.instructions.len(), 1);
        let g = draw(&n);
        assert_eq!(
            filters(&g),
            [
                "c_order.docstatus = 'CO'",
                "C_ORDER.DOCSTATUS <> 'VO'",
                "C_ORDER.DOCSTATUS <> 'RE'"
            ]
        );
        assert_eq!(g.counts.unmapped, 0);
        let vo = g
            .nodes
            .iter()
            .find(|n| n.term.as_deref() == Some("C_ORDER.DOCSTATUS <> 'VO'"))
            .unwrap();
        assert!(vo.rule.as_deref().unwrap().contains("instruction by ana"));
        assert_eq!(vo.annotations.len(), 1);
        assert_eq!(vo.annotations[0]["applies_to_marts"], true);

        // The table changed since: the instruction is stale and applies nothing.
        let mut changed = t.clone();
        changed
            .properties
            .insert("comment".into(), json!("orders and quotes"));
        let stale: Vec<KirObject> = objects
            .iter()
            .map(|o| {
                if o.id == t.id {
                    changed.clone()
                } else {
                    o.clone()
                }
            })
            .collect();
        let n = notes(&stale, &p);
        assert!(n.instructions.is_empty());
        assert_eq!(n.by_table["c_order"][0]["state"], "stale");
    }

    /// A comment and a personal instruction are shown on the step and never applied.
    #[test]
    fn a_personal_instruction_is_shown_and_not_applied() {
        let p = orders();
        let t = table_object();
        let a = an::annotation_object(&instruction(&t, "ana", Scope::Personal, &["VO"]));
        let n = notes(&[t, a], &p);
        assert!(n.instructions.is_empty());
        assert_eq!(n.by_table["c_order"].len(), 1);
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
