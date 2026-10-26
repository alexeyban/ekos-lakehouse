//! EKOS RFC 0226: step 5 — the extraction plan with step 3's decisions in, the operator's
//! changes per table, and the approval the build is held to.
//!
//! State, under `<state>/lakehouse/extraction/`: `proposed.json` (written by the commands that
//! read the source: `plan`, `generate`, `sync`), `changes.json` and `approved.json`. The approval
//! is also a ledger fact, `LakehouseApproval`, one id per project whose versions are the
//! approvals; `generate` under `require-plan-approval` compares against it.

use crate::plan_cmd::sha256;
use ekos_lakehouse_bronze::extraction::{Applied, TableChange, apply_all};
use ekos_lakehouse_bronze::{Cadence, LoadMode, Lossiness, Plan, WriteFacts};
use ekos_lakehouse_silver::MaskMethod;
use ekos_semantic::review::{Item, SignState, Subject};
use ekos_semantic::workflow::{Step, StepState, SubjectStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The step 3 subjects the plan reads.
const SUBJECTS: [Subject; 3] = [Subject::Scope, Subject::Pii, Subject::WriteBehaviour];

/// Whether a step 3 subject's decisions were used, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectUse {
    pub subject: String,
    pub used: bool,
    pub why: String,
    /// The signed digest the decisions came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// Step 3's decisions as the plan applies them; only from subjects with a valid sign-off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step3 {
    pub subjects: Vec<SubjectUse>,
    /// Lowercase table names (the review's keys).
    out_of_scope: BTreeSet<String>,
    /// Lowercase `table.column` → the mask; `None` is a confirmed column left unmasked.
    masks: BTreeMap<String, Option<MaskMethod>>,
    /// Lowercase table → the overridden write-behaviour fields.
    overrides: BTreeMap<String, serde_json::Map<String, Value>>,
}

/// A table step 3 put out of scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Excluded {
    pub table: String,
    pub reason: String,
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// The review's keys for a table: its name, or `schema.table`, lowercase.
fn keys(schema: &str, table: &str) -> [String; 2] {
    [
        table.to_ascii_lowercase(),
        format!("{schema}.{table}").to_ascii_lowercase(),
    ]
}

/// Pure: step 3 from the review's statuses and items.
pub fn step3(statuses: &[SubjectStatus], items: &BTreeMap<String, Vec<Item>>) -> Step3 {
    let mut out = Step3::default();
    for s in SUBJECTS {
        let name = s.as_str();
        let status = statuses.iter().find(|x| x.subject == name);
        let (used, why) = match status.map(|x| &x.signoff) {
            Some(SignState::Valid {
                signed_by,
                signed_at,
            }) => (true, format!("signed off by {signed_by} at {signed_at}")),
            Some(SignState::Stale { .. }) => (
                false,
                "its sign-off no longer matches the facts: sign it off again".into(),
            ),
            Some(SignState::Revoked { .. }) => (false, "its sign-off was revoked".into()),
            Some(SignState::None) | None => (false, "not signed off".into()),
        };
        out.subjects.push(SubjectUse {
            subject: name.into(),
            used,
            why,
            digest: status.filter(|_| used).map(|x| x.digest.clone()),
        });
        if !used {
            continue;
        }
        for i in items.get(name).map(Vec::as_slice).unwrap_or(&[]) {
            let table = i
                .facts
                .get("table")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let decision = i.decision.as_ref();
            match s {
                Subject::Scope => {
                    let scope = decision
                        .and_then(|d| text(d, "scope"))
                        .or(i.proposed.as_deref());
                    if scope == Some("out") {
                        out.out_of_scope.insert(table);
                    }
                }
                Subject::Pii => {
                    let Some(d) = decision.filter(|d| text(d, "pii") == Some("confirm")) else {
                        continue;
                    };
                    let column = i
                        .facts
                        .get("column")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let mask = match text(d, "mask") {
                        Some("hash") => Some(MaskMethod::Hmac),
                        Some("none") => None,
                        _ => Some(MaskMethod::Null),
                    };
                    out.masks.insert(format!("{table}.{column}"), mask);
                }
                Subject::WriteBehaviour => {
                    if let Some(f) = decision
                        .filter(|d| text(d, "behaviour") == Some("override"))
                        .and_then(|d| d.get("fields"))
                        .and_then(Value::as_object)
                    {
                        out.overrides.insert(table, f.clone());
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn classes(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::Array(a) => Some(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        Value::String(s) => Some(
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
        ),
        _ => None,
    }
}

impl Step3 {
    /// Step 3 when there is no review to read.
    pub fn unavailable(why: &str) -> Self {
        Self {
            subjects: SUBJECTS
                .iter()
                .map(|s| SubjectUse {
                    subject: s.as_str().into(),
                    used: false,
                    why: why.into(),
                    digest: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn override_for(&self, schema: &str, table: &str) -> Option<&serde_json::Map<String, Value>> {
        keys(schema, table)
            .iter()
            .find_map(|k| self.overrides.get(k))
    }

    fn is_out(&self, schema: &str, table: &str) -> bool {
        keys(schema, table)
            .iter()
            .any(|k| self.out_of_scope.contains(k))
    }

    /// The overridden write-behaviour fields in place, before the plan is chosen from them.
    pub fn apply_facts(&self, facts: &mut BTreeMap<String, WriteFacts>) {
        for (q, f) in facts.iter_mut() {
            let (schema, table) = q.split_once('.').unwrap_or(("", q));
            let Some(o) = self.override_for(schema, table) else {
                continue;
            };
            if let Some(u) = o.get("update_class").and_then(Value::as_str) {
                f.update_class = u.to_string();
            }
            if let Some(d) = o.get("delete_classes").and_then(classes) {
                f.delete_classes = d;
            }
        }
    }

    /// Out-of-scope tables leave the plan; an override is named in its table's evidence.
    pub fn apply_plan(&self, plan: &mut Plan) -> Vec<Excluded> {
        let mut excluded = vec![];
        let reason = "out of scope (step 3)".to_string();
        plan.tables.retain(|t| {
            let out = self.is_out(&t.schema, &t.table);
            if out {
                excluded.push(Excluded {
                    table: format!("{}.{}", t.schema, t.table),
                    reason: reason.clone(),
                });
            }
            !out
        });
        plan.refused.retain(|t| {
            let out = self.is_out(&t.schema, &t.table);
            if out {
                excluded.push(Excluded {
                    table: format!("{}.{}", t.schema, t.table),
                    reason: reason.clone(),
                });
            }
            !out
        });
        excluded.sort_by(|a, b| a.table.cmp(&b.table));
        for t in &mut plan.tables {
            if let Some(o) = self.override_for(&t.schema, &t.table) {
                let fields: Vec<String> = o.iter().map(|(k, v)| format!("{k} = {v}")).collect();
                t.decision.evidence.push(format!(
                    "write behaviour overridden in the review (step 3): {}",
                    fields.join(", ")
                ));
            }
        }
        excluded
    }

    /// `schema.table.column` (the plan's spelling) → mask, for the plan's columns.
    pub fn masks_for(&self, plan: &Plan) -> BTreeMap<String, MaskMethod> {
        let mut out = BTreeMap::new();
        for t in &plan.tables {
            for c in &t.columns {
                let found = keys(&t.schema, &t.table).iter().find_map(|k| {
                    self.masks
                        .get(&format!("{k}.{}", c.column.to_ascii_lowercase()))
                });
                if let Some(Some(m)) = found {
                    out.insert(format!("{}.{}.{}", t.schema, t.table, c.column), *m);
                }
            }
        }
        out
    }
}

/// What `lakehouse plan` proposes: the plan with step 3 in, before any operator change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub at: String,
    pub plan: Plan,
    /// The write-behaviour facts the plan was chosen from (overrides applied), by `OWNER.TABLE`.
    pub facts: BTreeMap<String, WriteFacts>,
    /// Step 3's masks.
    pub masks: BTreeMap<String, MaskMethod>,
    pub excluded: Vec<Excluded>,
    pub step3: Vec<SubjectUse>,
}

/// The plan as it would be built: the proposal with the changes, and its digest.
#[derive(Debug, Clone, PartialEq)]
pub struct Final {
    pub plan: Plan,
    /// `ekos.toml`'s masks, then step 3's for the columns it does not name.
    pub masks: BTreeMap<String, MaskMethod>,
    pub excluded: Vec<Excluded>,
    pub applied: Vec<Applied>,
    pub digest: String,
    /// `schema.table` → its part of the digest.
    pub tables: BTreeMap<String, String>,
    pub notes: Vec<String>,
}

impl Final {
    pub fn stale(&self) -> Vec<&Applied> {
        self.applied.iter().filter(|a| a.stale.is_some()).collect()
    }
}

/// Pure: the digest over what a build depends on (RFC 0226 §2.4) — evidence text and notes
/// left out, since they carry row estimates.
pub fn digest(
    plan: &Plan,
    masks: &BTreeMap<String, MaskMethod>,
    excluded: &[Excluded],
) -> (String, BTreeMap<String, String>) {
    let mut tables = BTreeMap::new();
    for t in &plan.tables {
        let mut t = t.clone();
        t.decision.evidence.clear();
        t.decision.notes.clear();
        let body = serde_json::to_string(&t).unwrap_or_default();
        tables.insert(format!("{}.{}", t.schema, t.table), sha256(&body));
    }
    let whole = json!({
        "format": plan.format,
        "settings": plan.settings,
        "tables": tables,
        "refused": plan.refused,
        "excluded": excluded,
        "masks": masks,
    });
    (sha256(&whole.to_string()), tables)
}

/// Pure: the proposal with `changes` applied and `configured` (`ekos.toml`) masks first.
pub fn finalize(
    p: &Proposal,
    changes: &[TableChange],
    configured: &BTreeMap<String, MaskMethod>,
) -> Final {
    let mut plan = p.plan.clone();
    let applied = apply_all(&mut plan, &p.facts, changes);
    let mut masks = configured.clone();
    let mut notes = vec![];
    for (k, m) in &p.masks {
        match configured.iter().find(|(c, _)| c.eq_ignore_ascii_case(k)) {
            Some((c, cm)) if cm != m => notes.push(format!(
                "{c}: ekos.toml's mask ({cm:?}) wins over step 3's ({m:?})"
            )),
            Some(_) => {}
            None => {
                masks.insert(k.clone(), *m);
            }
        }
    }
    let (digest, tables) = digest(&plan, &masks, &p.excluded);
    Final {
        plan,
        masks,
        excluded: p.excluded.clone(),
        applied,
        digest,
        tables,
        notes,
    }
}

/// The counts an approval records and the page shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub incremental_every_run: usize,
    pub incremental_audited: usize,
    /// Incremental without a delete plan (a trigger-proven watermark with a key-set diff).
    pub incremental_other: usize,
    pub full: usize,
    pub refused: usize,
    pub excluded: usize,
    pub masked_columns: usize,
}

pub fn counts(f: &Final) -> Counts {
    let mut c = Counts {
        refused: f.plan.refused.len(),
        excluded: f.excluded.len(),
        masked_columns: f.masks.len(),
        ..Default::default()
    };
    for t in &f.plan.tables {
        match (
            &t.decision.mode,
            t.decision.deletes.as_ref().map(|d| d.cadence),
        ) {
            (LoadMode::Full, _) => c.full += 1,
            (_, Some(Cadence::Run)) => c.incremental_every_run += 1,
            (_, Some(Cadence::Audit { .. })) => c.incremental_audited += 1,
            (_, None) => c.incremental_other += 1,
        }
    }
    c
}

/// An approval: who agreed to which plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub by: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub digest: String,
    pub proposed_at: String,
    pub counts: Counts,
    pub changes: Vec<TableChange>,
    pub step3: Vec<SubjectUse>,
    /// Per-table digests, to name what changed later (kept in the state file only).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tables: BTreeMap<String, String>,
}

/// Pure: the approval of `f`, or why it cannot be approved.
pub fn approve(
    p: &Proposal,
    f: &Final,
    changes: &[TableChange],
    by: &str,
    at: &str,
    note: Option<String>,
) -> anyhow::Result<Approval> {
    let stale = f.stale();
    if !stale.is_empty() {
        let list: Vec<String> = stale
            .iter()
            .map(|a| {
                format!(
                    "{} ({}): {}",
                    a.change.table,
                    a.change.change.describe(),
                    a.stale.as_deref().unwrap_or_default()
                )
            })
            .collect();
        anyhow::bail!(
            "these changes no longer fit the plan; undo or redo them first: {}",
            list.join("; ")
        );
    }
    Ok(Approval {
        by: by.into(),
        at: at.into(),
        note,
        digest: f.digest.clone(),
        proposed_at: p.at.clone(),
        counts: counts(f),
        changes: changes.to_vec(),
        step3: p.step3.clone(),
        tables: f.tables.clone(),
    })
}

/// Pure: which tables differ between an approval's digests and a plan's.
pub fn differences(approved: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> String {
    let mut changed = vec![];
    let mut added = vec![];
    let mut removed = vec![];
    for (t, d) in now {
        match approved.get(t) {
            Some(a) if a == d => {}
            Some(_) => changed.push(t.as_str()),
            None => added.push(t.as_str()),
        }
    }
    for t in approved.keys() {
        if !now.contains_key(t) {
            removed.push(t.as_str());
        }
    }
    let list = |what: &str, v: &[&str]| {
        let shown: Vec<&str> = v.iter().take(10).copied().collect();
        let more = if v.len() > 10 {
            format!(" and {} more", v.len() - 10)
        } else {
            String::new()
        };
        format!("{what} {}{more}", shown.join(", "))
    };
    let mut parts = vec![];
    if !changed.is_empty() {
        parts.push(list("changed:", &changed));
    }
    if !added.is_empty() {
        parts.push(list("new:", &added));
    }
    if !removed.is_empty() {
        parts.push(list("gone:", &removed));
    }
    if parts.is_empty() {
        "no table differs: the settings, masks, refused or excluded tables do".into()
    } else {
        parts.join("; ")
    }
}

// ---- state files ----

pub fn state_dir(config: &ekos_compiler_core::EkosConfig, cwd: &Path) -> PathBuf {
    config.ekos_dir(cwd).join("lakehouse").join("extraction")
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
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(v)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_proposal(dir: &Path) -> anyhow::Result<Option<Proposal>> {
    read_json(&dir.join("proposed.json"))
}

pub fn write_proposal(dir: &Path, p: &Proposal) -> anyhow::Result<()> {
    write_json(&dir.join("proposed.json"), p)
}

pub fn read_changes(dir: &Path) -> anyhow::Result<Vec<TableChange>> {
    Ok(read_json(&dir.join("changes.json"))?.unwrap_or_default())
}

pub fn write_changes(dir: &Path, c: &[TableChange]) -> anyhow::Result<()> {
    write_json(&dir.join("changes.json"), &c)
}

pub fn read_approved(dir: &Path) -> anyhow::Result<Option<Approval>> {
    read_json(&dir.join("approved.json"))
}

pub fn write_approved(dir: &Path, a: &Approval) -> anyhow::Result<()> {
    write_json(&dir.join("approved.json"), a)
}

// ---- where step 5 stands ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    NoProposal,
    NotApproved,
    /// Approved, but the plan as it would be built now differs.
    ChangedSinceApproval,
    Approved,
}

impl State {
    pub fn of(f: Option<&Final>, a: Option<&Approval>) -> Self {
        match (f, a) {
            (None, _) => State::NoProposal,
            (Some(_), None) => State::NotApproved,
            (Some(f), Some(a)) if f.digest == a.digest => State::Approved,
            (Some(_), Some(_)) => State::ChangedSinceApproval,
        }
    }

    pub fn words(&self) -> &'static str {
        match self {
            State::NoProposal => "no plan yet",
            State::NotApproved => "not approved",
            State::ChangedSinceApproval => "changed since the approval",
            State::Approved => "approved",
        }
    }
}

fn plan_words(c: &Counts) -> String {
    format!(
        "{} incremental, {} full, {} refused, {} out of scope",
        c.incremental_every_run + c.incremental_audited + c.incremental_other,
        c.full,
        c.refused,
        c.excluded
    )
}

/// Pure: workflow step 5 (RFC 0226 §2.7).
pub fn step(p: Option<&Proposal>, f: Option<&Final>, a: Option<&Approval>) -> Step {
    match (State::of(f, a), p, f, a) {
        (State::Approved, _, Some(f), Some(a)) => Step::new(
            5,
            StepState::Done,
            format!(
                "plan approved by {} at {}: {}",
                a.by,
                a.at,
                plan_words(&counts(f))
            ),
        ),
        (State::ChangedSinceApproval, _, _, Some(a)) => Step::new(
            5,
            StepState::Partial,
            format!(
                "approved by {} at {}, but the plan changed since: review and approve it again",
                a.by, a.at
            ),
        ),
        (State::NotApproved, Some(p), Some(f), _) => Step::new(
            5,
            StepState::Partial,
            format!(
                "planned at {}: {}; not approved yet",
                p.at,
                plan_words(&counts(f))
            ),
        ),
        _ => Step::new(
            5,
            StepState::Todo,
            "plan the extraction (`lakehouse plan`), then review and approve it",
        ),
    }
}

// ---- the page's JSON ----

fn mode_words(t: &ekos_lakehouse_bronze::TablePlan) -> (&'static str, Option<String>) {
    match &t.decision.mode {
        LoadMode::Full => ("full", None),
        LoadMode::Incremental { watermark, .. } => ("incremental", Some(watermark.clone())),
    }
}

fn cadence_words(c: Cadence) -> String {
    match c {
        Cadence::Run => "every run".into(),
        Cadence::Audit { days } => format!("every {days} days (audit)"),
    }
}

fn bullets(out: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("### {title}\n\n"));
    for i in items {
        out.push_str(&format!("- {i}\n"));
    }
    out.push('\n');
}

/// Pure: `lakehouse extraction show --json`.
pub fn show(
    p: Option<&Proposal>,
    f: Option<&Final>,
    a: Option<&Approval>,
    selected: Option<&str>,
) -> Value {
    let state = State::of(f, a);
    let (Some(p), Some(f)) = (p, f) else {
        return json!({
            "proposed": false,
            "summary": {"state": state.words()},
            "step3": [], "tables": [], "refused": [], "excluded": [], "changes": [],
        });
    };
    let c = counts(f);
    let since: Option<&BTreeMap<String, String>> = a.map(|a| &a.tables).filter(|t| !t.is_empty());
    let changes_of = |table: &str| -> Vec<&Applied> {
        f.applied
            .iter()
            .filter(|x| x.change.table == table)
            .collect()
    };
    let tables: Vec<Value> = f
        .plan
        .tables
        .iter()
        .map(|t| {
            let q = format!("{}.{}", t.schema, t.table);
            let (mode, watermark) = mode_words(t);
            let d = t.decision.deletes.as_ref();
            let masked = f
                .masks
                .keys()
                .filter(|k| k.starts_with(&format!("{q}.")))
                .count();
            let change: Vec<String> = changes_of(&q)
                .iter()
                .map(|x| {
                    let mut s = x.change.change.describe();
                    if x.stale.is_some() {
                        s.push_str(" (stale)");
                    }
                    s
                })
                .collect();
            let since_approval = match since {
                None => "",
                Some(m) => match m.get(&q) {
                    None => "new",
                    Some(h) if Some(h) != f.tables.get(&q) => "changed",
                    Some(_) => "",
                },
            };
            json!({
                "table": q,
                "mode": mode,
                "watermark": watermark,
                "deletes": d.map(|d| cadence_words(d.cadence)),
                "kind": d.map(|d| d.kind.clone()),
                "purge_policy": d.and_then(|d| d.purge_policy),
                "contradicts_code": d.is_some_and(|d| d.contradicts_code).then_some("yes"),
                "not_exact": t.columns.iter().filter(|m| m.lossiness != Lossiness::Exact).count(),
                "masked": masked,
                "change": change.join("; "),
                "since_approval": since_approval,
            })
        })
        .collect();
    let selected = selected.and_then(|s| {
        f.plan
            .tables
            .iter()
            .find(|t| format!("{}.{}", t.schema, t.table).eq_ignore_ascii_case(s))
    });
    let selected = selected.map(|t| {
        let q = format!("{}.{}", t.schema, t.table);
        let mut md = format!("## {q}\n\n");
        bullets(&mut md, "Why it loads this way", &t.decision.evidence);
        bullets(&mut md, "Worth knowing", &t.decision.notes);
        let ch: Vec<String> = changes_of(&q)
            .iter()
            .map(|x| {
                format!(
                    "{} — by {} at {}{}{}",
                    x.change.change.describe(),
                    x.change.by,
                    x.change.at,
                    x.change
                        .reason
                        .as_deref()
                        .map(|r| format!(": {r}"))
                        .unwrap_or_default(),
                    x.stale
                        .as_deref()
                        .map(|s| format!(" — **not applied**: {s}"))
                        .unwrap_or_default()
                )
            })
            .collect();
        bullets(&mut md, "Changes", &ch);
        let columns: Vec<Value> = t
            .columns
            .iter()
            .map(|m| {
                json!({
                    "column": m.column,
                    "source": m.pg_type,
                    "iceberg": m.iceberg_type,
                    "lossiness": format!("{:?}", m.lossiness).to_lowercase(),
                    "rule": m.rule,
                    "mask": f.masks.get(&format!("{q}.{}", m.column))
                        .map(|m| format!("{m:?}").to_lowercase()),
                })
            })
            .collect();
        json!({"table": q, "markdown": md, "columns": columns})
    });
    let step3: Vec<Value> = p
        .step3
        .iter()
        .map(|s| json!({"subject": s.subject, "used": if s.used {"used"} else {"ignored"}, "why": s.why}))
        .collect();
    let changes: Vec<Value> = f
        .applied
        .iter()
        .map(|x| {
            json!({
                "table": x.change.table,
                "change": x.change.change.describe(),
                "reason": x.change.reason,
                "by": x.change.by,
                "at": x.change.at,
                "state": if x.stale.is_some() {"stale"} else {"applied"},
                "why_stale": x.stale,
            })
        })
        .collect();
    let refused: Vec<Value> = f
        .plan
        .refused
        .iter()
        .map(|r| json!({"table": format!("{}.{}", r.schema, r.table), "reasons": r.reasons.join("; ")}))
        .collect();
    json!({
        "proposed": true,
        "summary": {
            "state": state.words(),
            "planned_at": p.at,
            "approved_by": a.map(|a| a.by.clone()),
            "approved_at": a.map(|a| a.at.clone()),
            "digest": &f.digest[..f.digest.len().min(16)],
            "incremental_every_run": c.incremental_every_run,
            "incremental_audited": c.incremental_audited,
            "incremental_other": c.incremental_other,
            "full": c.full,
            "refused": c.refused,
            "excluded": c.excluded,
            "masked_columns": c.masked_columns,
            "changes": f.applied.len(),
            "stale_changes": f.stale().len(),
            "notes": f.notes.join("; "),
        },
        "step3": step3,
        "tables": tables,
        "selected": selected,
        "refused": refused,
        "excluded": f.excluded,
        "changes": changes,
    })
}

/// Pure: the text form of [`show`].
pub fn render(v: &Value) -> String {
    let s = &v["summary"];
    let mut out = format!("extraction plan: {}\n", s["state"].as_str().unwrap_or("?"));
    if v["proposed"] != true {
        out.push_str("run `lakehouse plan` first\n");
        return out;
    }
    if let Some(by) = s["approved_by"].as_str() {
        out.push_str(&format!(
            "approved by {by} at {}\n",
            s["approved_at"].as_str().unwrap_or("?")
        ));
    }
    out.push_str(&format!(
        "planned at {}, digest {}\n  incremental: {} deletes every run, {} audited, {} other\n  \
         full: {}   refused: {}   out of scope: {}   masked columns: {}\n  changes: {} ({} stale)\n",
        s["planned_at"].as_str().unwrap_or("?"),
        s["digest"].as_str().unwrap_or("?"),
        s["incremental_every_run"],
        s["incremental_audited"],
        s["incremental_other"],
        s["full"],
        s["refused"],
        s["excluded"],
        s["masked_columns"],
        s["changes"],
        s["stale_changes"],
    ));
    for x in v["step3"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  step 3 {}: {} ({})\n",
            x["subject"].as_str().unwrap_or("?"),
            x["used"].as_str().unwrap_or("?"),
            x["why"].as_str().unwrap_or("?")
        ));
    }
    for x in v["changes"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  change {}: {} by {} ({}){}\n",
            x["table"].as_str().unwrap_or("?"),
            x["change"].as_str().unwrap_or("?"),
            x["by"].as_str().unwrap_or("?"),
            x["state"].as_str().unwrap_or("?"),
            x["reason"]
                .as_str()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default()
        ));
    }
    if let Some(n) = s["notes"].as_str().filter(|n| !n.is_empty()) {
        out.push_str(&format!("  note: {n}\n"));
    }
    if let Some(t) = v["selected"].as_object() {
        out.push('\n');
        out.push_str(t["markdown"].as_str().unwrap_or_default());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekos_lakehouse_bronze::ModeSettings;
    use ekos_lakehouse_bronze::extraction::{Change, ModeChoice};
    use ekos_lakehouse_bronze::model::{SourceColumn, SourceEngine, SourceTable, TypeKind};
    use ekos_lakehouse_bronze::plan::{PlanSettings, plan_with};

    fn item(id: &str, facts: Value, proposed: Option<&str>, decision: Option<Value>) -> Item {
        Item {
            id: id.into(),
            label: id.into(),
            facts: serde_json::from_value(facts).unwrap(),
            info: Default::default(),
            proposed: proposed.map(Into::into),
            decision,
            outdated: false,
            blocking: false,
        }
    }

    fn status(subject: &str, signoff: SignState) -> SubjectStatus {
        SubjectStatus {
            subject: subject.into(),
            items: 0,
            decided: 0,
            outdated: 0,
            blocking: 0,
            digest: format!("d-{subject}"),
            signoff,
        }
    }

    fn valid() -> SignState {
        SignState::Valid {
            signed_by: "ana".into(),
            signed_at: "2026-10-25T10:00:00Z".into(),
        }
    }

    fn review() -> BTreeMap<String, Vec<Item>> {
        [
            (
                "scope".to_string(),
                vec![
                    item("table:t_temp", json!({"table":"t_temp"}), Some("out"), None),
                    item("table:a", json!({"table":"a"}), Some("in"), None),
                    item("table:b", json!({"table":"b"}), Some("in"), Some(json!({"scope":"out"}))),
                ],
            ),
            (
                "pii".to_string(),
                vec![
                    item("c1", json!({"table":"a","column":"email"}), None, Some(json!({"pii":"confirm","mask":"hash"}))),
                    item("c2", json!({"table":"a","column":"phone"}), None, Some(json!({"pii":"reject"}))),
                    item("c3", json!({"table":"a","column":"name"}), None, Some(json!({"pii":"confirm","mask":"none"}))),
                    item("c4", json!({"table":"a","column":"note"}), None, None),
                ],
            ),
            (
                "write-behaviour".to_string(),
                vec![item(
                    "w",
                    json!({"table":"a"}),
                    None,
                    Some(json!({"behaviour":"override","fields":{"update_class":"raw_sql_skips_updated"}})),
                )],
            ),
        ]
        .into()
    }

    fn col(name: &str, ty: &str, trino: &str) -> SourceColumn {
        SourceColumn {
            name: name.into(),
            pg_type: ty.into(),
            kind: TypeKind::Base,
            not_null: true,
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

    fn source(name: &str, rows: i64) -> SourceTable {
        SourceTable {
            schema: "ADEMPIERE".into(),
            name: name.into(),
            oid: 0,
            columns: vec![
                col("ID", "NUMBER(10,0)", "decimal(10,0)"),
                col("UPDATED", "DATE", "timestamp(0)"),
                col("EMAIL", "VARCHAR2(60 CHAR)", "varchar(60)"),
                col("NAME", "VARCHAR2(60 CHAR)", "varchar(60)"),
            ],
            primary_key: vec!["ID".into()],
            row_estimate: rows,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }
    }

    fn proposal(statuses: &[SubjectStatus], rows: i64) -> Proposal {
        let s3 = step3(statuses, &review());
        let settings = PlanSettings {
            source_engine: SourceEngine::Oracle,
            modes: ModeSettings {
                application_watermark: Some("UPDATED".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut facts: BTreeMap<String, WriteFacts> = ["A", "B", "C", "T_TEMP"]
            .iter()
            .map(|t| {
                (
                    format!("ADEMPIERE.{t}"),
                    WriteFacts {
                        update_class: "po_only".into(),
                        delete_classes: vec!["deleted_by_users".into()],
                        change_log: None,
                    },
                )
            })
            .collect();
        s3.apply_facts(&mut facts);
        let mut plan = plan_with(
            &[
                source("A", rows),
                source("B", rows),
                source("C", rows),
                source("T_TEMP", rows),
            ],
            &settings,
            &facts,
        );
        let excluded = s3.apply_plan(&mut plan);
        let masks = s3.masks_for(&plan);
        Proposal {
            at: "2026-10-26T00:00:00Z".into(),
            plan,
            facts,
            masks,
            excluded,
            step3: s3.subjects,
        }
    }

    fn signed() -> Vec<SubjectStatus> {
        vec![
            status("scope", valid()),
            status("pii", valid()),
            status("write-behaviour", valid()),
        ]
    }

    #[test]
    fn signed_subjects_shape_the_plan() {
        let p = proposal(&signed(), 10);
        let names: Vec<&str> = p.plan.tables.iter().map(|t| t.table.as_str()).collect();
        assert_eq!(
            names,
            ["A", "C"],
            "scope out (proposed and decided) leaves the plan"
        );
        assert_eq!(
            p.excluded
                .iter()
                .map(|e| e.table.as_str())
                .collect::<Vec<_>>(),
            ["ADEMPIERE.B", "ADEMPIERE.T_TEMP"]
        );
        // Only the confirmed, masked column; `none` and undecided ones are not masked.
        assert_eq!(
            p.masks,
            [("ADEMPIERE.A.EMAIL".to_string(), MaskMethod::Hmac)].into()
        );
        // The override keeps A in full; C, without one, is incremental.
        let a = &p.plan.tables[0];
        assert_eq!(a.decision.mode, LoadMode::Full);
        assert!(
            a.decision
                .evidence
                .iter()
                .any(|e| e.contains("overridden in the review"))
        );
        assert!(matches!(
            p.plan.tables[1].decision.mode,
            LoadMode::Incremental { .. }
        ));
        assert!(p.step3.iter().all(|s| s.used && s.digest.is_some()));
    }

    #[test]
    fn an_unsigned_or_stale_subject_is_ignored_whole() {
        let statuses = vec![
            status("scope", SignState::None),
            status(
                "pii",
                SignState::Stale {
                    signed_by: "ana".into(),
                    signed_at: "x".into(),
                    added: vec![],
                    removed: vec![],
                    changed: vec![],
                },
            ),
            status("write-behaviour", valid()),
        ];
        let p = proposal(&statuses, 10);
        assert_eq!(p.plan.tables.len(), 4);
        assert!(p.excluded.is_empty() && p.masks.is_empty());
        assert_eq!(p.step3[0].why, "not signed off");
        assert!(p.step3[1].why.contains("no longer matches"));
        assert!(p.step3[2].used);
        let none = Step3::unavailable("no ledger");
        assert!(
            none.subjects
                .iter()
                .all(|s| !s.used && s.why == "no ledger")
        );
    }

    fn change(table: &str, c: Change, reason: Option<&str>) -> TableChange {
        TableChange {
            table: format!("ADEMPIERE.{table}"),
            change: c,
            reason: reason.map(Into::into),
            by: "ana".into(),
            at: "t".into(),
        }
    }

    #[test]
    fn the_digest_ignores_row_estimates_and_follows_decisions() {
        let base = finalize(&proposal(&signed(), 10), &[], &BTreeMap::new());
        let grown = finalize(&proposal(&signed(), 99_000), &[], &BTreeMap::new());
        assert_eq!(
            base.digest, grown.digest,
            "evidence text carries the estimate"
        );
        let p = proposal(&signed(), 10);
        let full = finalize(
            &p,
            &[change("C", Change::Mode(ModeChoice::Full), None)],
            &BTreeMap::new(),
        );
        assert_ne!(base.digest, full.digest);
        assert_eq!(
            differences(&base.tables, &full.tables),
            "changed: ADEMPIERE.C"
        );
        // ekos.toml's mask wins, and says so.
        let configured = [("adempiere.a.email".to_string(), MaskMethod::Null)].into();
        let f = finalize(&p, &[], &configured);
        assert_eq!(f.masks.len(), 1);
        assert!(f.notes[0].contains("wins over step 3"));
        assert_ne!(f.digest, base.digest);
    }

    #[test]
    fn approval_refuses_stale_changes_and_step_5_follows_the_digest() {
        let p = proposal(&signed(), 10);
        let bad = vec![change("B", Change::Mode(ModeChoice::Full), None)];
        let f = finalize(&p, &bad, &BTreeMap::new());
        let e = approve(&p, &f, &bad, "ana", "now", None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("ADEMPIERE.B"), "{e}");
        let ok = vec![change("C", Change::Mode(ModeChoice::Full), None)];
        let f = finalize(&p, &ok, &BTreeMap::new());
        let a = approve(&p, &f, &ok, "ana", "now", Some("first".into())).unwrap();
        assert_eq!(a.counts.full, 2);
        assert_eq!(step(Some(&p), Some(&f), Some(&a)).state, StepState::Done);
        let undone = finalize(&p, &[], &BTreeMap::new());
        let s = step(Some(&p), Some(&undone), Some(&a));
        assert_eq!(s.state, StepState::Partial);
        assert!(s.detail.contains("changed since"));
        assert_eq!(
            step(Some(&p), Some(&undone), None).state,
            StepState::Partial
        );
        assert_eq!(step(None, None, None).state, StepState::Todo);
        let v = show(Some(&p), Some(&undone), Some(&a), Some("adempiere.c"));
        assert_eq!(v["summary"]["state"], "changed since the approval");
        let c = v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["table"] == "ADEMPIERE.C")
            .unwrap();
        assert_eq!(c["since_approval"], "changed");
        assert_eq!(v["selected"]["table"], "ADEMPIERE.C");
        assert!(
            v["selected"]["markdown"]
                .as_str()
                .unwrap()
                .contains("### Why it loads this way")
        );
        let a_row = v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["table"] == "ADEMPIERE.A")
            .unwrap();
        assert_eq!(a_row["masked"], 1);
        assert!(render(&v).contains("step 3 scope: used"));
    }

    #[test]
    fn the_state_files_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("x");
        assert!(read_proposal(&d).unwrap().is_none());
        assert!(read_changes(&d).unwrap().is_empty());
        let p = proposal(&signed(), 10);
        write_proposal(&d, &p).unwrap();
        assert_eq!(read_proposal(&d).unwrap().unwrap(), p);
        let c = vec![change("C", Change::Mode(ModeChoice::Full), Some("small"))];
        write_changes(&d, &c).unwrap();
        assert_eq!(read_changes(&d).unwrap(), c);
    }
}
