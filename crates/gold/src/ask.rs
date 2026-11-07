//! Questions to marts (EKOS RFC 0248): the model turns a question into **terms**; this module
//! checks them, triages the request, and hands the terms to the same resolution and compilation a
//! written mart description gets (RFC 0195). The model never writes SQL here: what it returns is
//! a list of `name: definition` terms in the description's own vocabulary (`sum of ar.amount_bc`),
//! and every term must resolve against the deployed silver model or becomes a question back.
//!
//! Pure: no connection, no model, no git. The model call, Superset and the console are elsewhere.

use crate::{MartSpec, Plan, Question, Resolved, Term, questions, resolve};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most terms of any one kind a request may carry.
pub const MAX_TERMS: usize = 8;
const MAX_NAME: usize = 40;
const MAX_DEFINITION: usize = 200;

/// One `name: definition` term, as the description header writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedTerm {
    pub name: String,
    pub definition: String,
}

/// What a model returns for a question. Terms only — a field that could carry SQL does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskRequest {
    pub measures: Vec<NamedTerm>,
    #[serde(default)]
    pub dimensions: Vec<NamedTerm>,
    /// Filters in the description's vocabulary: `transactions.approved is true`.
    #[serde(default)]
    pub filters: Vec<String>,
    /// Set only when the request asks to change an existing mart (it must exist).
    #[serde(default)]
    pub target_mart: Option<String>,
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_NAME
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !n.starts_with(|c: char| c.is_ascii_digit())
}

/// A definition is data in the description's vocabulary, never SQL: no statement separators,
/// quotes, comment markers, backticks, backslashes or control characters.
fn definition_problem(d: &str) -> Option<String> {
    if d.trim().is_empty() {
        return Some("is empty".into());
    }
    if d.chars().count() > MAX_DEFINITION {
        return Some(format!("is longer than {MAX_DEFINITION} characters"));
    }
    let bad = d
        .chars()
        .any(|c| c.is_control() || matches!(c, ';' | '\'' | '"' | '`' | '\\'))
        || d.contains("--")
        || d.contains("/*");
    bad.then(|| "contains characters a term may not hold (quotes, `;`, comments)".into())
}

impl AskRequest {
    /// Why this request cannot be used, if it cannot. Checked before anything else sees it.
    pub fn check(&self) -> Result<(), String> {
        if self.measures.is_empty() {
            return Err(
                "a question needs at least one measure (what is counted, summed, averaged)".into(),
            );
        }
        for (what, terms) in [("measure", &self.measures), ("dimension", &self.dimensions)] {
            if terms.len() > MAX_TERMS {
                return Err(format!("at most {MAX_TERMS} {what}s"));
            }
            let mut seen = BTreeSet::new();
            for t in terms {
                if !valid_name(&t.name) {
                    return Err(format!(
                        "{what} name {:?}: lowercase letters, digits and _ only, starting with a letter",
                        t.name
                    ));
                }
                if !seen.insert(&t.name) {
                    return Err(format!("{what} {:?} is named twice", t.name));
                }
                if let Some(p) = definition_problem(&t.definition) {
                    return Err(format!("{what} {}: definition {p}", t.name));
                }
            }
        }
        if self.filters.len() > MAX_TERMS {
            return Err(format!("at most {MAX_TERMS} filters"));
        }
        for f in &self.filters {
            if let Some(p) = definition_problem(f) {
                return Err(format!("filter {f:?} {p}"));
            }
        }
        let clash = self
            .measures
            .iter()
            .any(|m| self.dimensions.iter().any(|d| d.name == m.name));
        if clash {
            return Err("a measure and a dimension share a name".into());
        }
        Ok(())
    }

    /// The mart description this request is, named by its content: the same terms always give the
    /// same name, so asking twice builds nothing twice.
    pub fn to_spec(&self) -> MartSpec {
        let canonical = serde_json::to_string(&(
            sorted(&self.measures),
            sorted(&self.dimensions),
            self.filters
                .iter()
                .map(|f| norm(f))
                .collect::<BTreeSet<_>>(),
        ))
        .unwrap_or_default();
        MartSpec {
            mart: format!("ask_{}", fingerprint(&canonical)),
            grain: self.dimensions.iter().map(|d| d.name.clone()).collect(),
            dimensions: self
                .dimensions
                .iter()
                .map(|d| (d.name.clone(), d.definition.clone()))
                .collect(),
            measures: self
                .measures
                .iter()
                .map(|m| (m.name.clone(), m.definition.clone()))
                .collect(),
            filters: self.filters.clone(),
            prose: String::new(),
        }
    }
}

/// The first 12 hex digits of the SHA-256 of `text`.
fn fingerprint(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sorted(terms: &[NamedTerm]) -> BTreeSet<(String, String)> {
    terms
        .iter()
        .map(|t| (t.name.clone(), norm(&t.definition)))
        .collect()
}

/// A definition compared without case or spacing: `Sum  of AR.amount` is `sum of ar.amount`.
fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Something an existing mart would have to change to serve the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Change {
    /// The mart defines this measure differently.
    Measure {
        name: String,
        was: String,
        now: String,
    },
    /// The mart defines this dimension differently.
    Dimension {
        name: String,
        was: String,
        now: String,
    },
    /// The mart's filters are not the request's.
    Filters { was: Vec<String>, now: Vec<String> },
    /// The mart's grain is not the request's.
    Grain { was: Vec<String>, now: Vec<String> },
}

/// The status of a request (EKOS RFC 0248, "Triage"). `unsafe` is RFC 0249's and decided before
/// this runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Triage {
    /// An existing mart answers it exactly (same measures, dimensions and filters).
    PossibleNow { mart: String },
    /// It resolves in silver but no mart has it: a new mart, or `extend` an existing one with new
    /// measures — nothing existing changes.
    PossibleWithAddition {
        mart: String,
        extend: Option<String>,
    },
    /// Answering it means changing the existing mart the request targets.
    PossibleWithChange { mart: String, changes: Vec<Change> },
    /// A term can be read more than one way (or names something near): ask, never guess.
    NeedsClarification { questions: Vec<Question> },
    /// A term names nothing in silver: not in gold, silver or the source.
    Impossible { reasons: Vec<String> },
}

/// Triage a request against the deployed plan and the marts that already exist.
pub fn triage(req: &AskRequest, plan: &Plan, marts: &[MartSpec]) -> Result<Triage, String> {
    req.check()?;
    if let Some(target) = &req.target_mart
        && !marts.iter().any(|m| &m.mart == target)
    {
        return Err(format!(
            "no mart named {target}: a change targets an existing mart"
        ));
    }
    let spec = req.to_spec();
    let resolved = resolve(&spec, plan);
    if let Some(t) = unresolved(&resolved).or_else(|| prose(&resolved)) {
        return Ok(t);
    }

    let mut ordered: Vec<&MartSpec> = marts.iter().collect();
    ordered.sort_by(|a, b| a.mart.cmp(&b.mart));

    if let Some(target) = &req.target_mart {
        let mart = ordered
            .iter()
            .find(|m| &m.mart == target)
            .expect("checked above");
        let changes = changes(req, mart);
        return Ok(if changes.is_empty() {
            Triage::PossibleNow {
                mart: mart.mart.clone(),
            }
        } else {
            Triage::PossibleWithChange {
                mart: mart.mart.clone(),
                changes,
            }
        });
    }

    if let Some(m) = ordered.iter().find(|m| covers(req, m)) {
        return Ok(Triage::PossibleNow {
            mart: m.mart.clone(),
        });
    }
    let extend = ordered
        .iter()
        .find(|m| same_shape(req, m))
        .map(|m| m.mart.clone());
    Ok(Triage::PossibleWithAddition {
        mart: spec.mart,
        extend,
    })
}

/// A prose term is one a model would have to turn into SQL. A question never gets that: the model
/// names measures in the description's vocabulary (`sum of ar.amount_bc`) or is asked to.
fn prose(resolved: &[Resolved]) -> Option<Triage> {
    let qs: Vec<Question> = resolved
        .iter()
        .filter(|r| matches!(r.resolution, Ok(Term::Prose { .. })))
        .map(|r| Question {
            text: format!(
                "`{}` is prose, not a column or an aggregate of one: write it as \
                 `sum|count|… of table.column` (a model does not write SQL for a question)",
                r.written
            ),
            candidates: vec![],
        })
        .collect();
    (!qs.is_empty()).then_some(Triage::NeedsClarification { questions: qs })
}

/// Terms that did not resolve: nothing near them is `Impossible`; something near is a question.
fn unresolved(resolved: &[Resolved]) -> Option<Triage> {
    let qs = questions(resolved);
    if qs.is_empty() {
        return None;
    }
    let hopeless: Vec<String> = qs
        .iter()
        .filter(|(_, q)| q.candidates.is_empty())
        .map(|(r, q)| format!("{} ({}): {}", r.term, r.section, q.text))
        .collect();
    if !hopeless.is_empty() {
        return Some(Triage::Impossible { reasons: hopeless });
    }
    Some(Triage::NeedsClarification {
        questions: qs.into_iter().map(|(_, q)| q.clone()).collect(),
    })
}

fn defs(terms: &[NamedTerm]) -> BTreeSet<String> {
    terms.iter().map(|t| norm(&t.definition)).collect()
}

fn mart_defs(pairs: &[(String, String)]) -> BTreeSet<String> {
    pairs.iter().map(|(_, d)| norm(d)).collect()
}

fn filter_set(f: &[String]) -> BTreeSet<String> {
    f.iter().map(|x| norm(x)).collect()
}

/// The mart answers the request exactly: the same dimensions (its grain), the same filters, and
/// every requested measure among its own.
fn covers(req: &AskRequest, m: &MartSpec) -> bool {
    same_shape(req, m) && defs(&req.measures).is_subset(&mart_defs(&m.measures))
}

/// Same dimensions and same filters: a mart that adding measures to would answer the request.
fn same_shape(req: &AskRequest, m: &MartSpec) -> bool {
    defs(&req.dimensions) == mart_defs(&m.dimensions)
        && filter_set(&req.filters) == filter_set(&m.filters)
}

fn changes(req: &AskRequest, m: &MartSpec) -> Vec<Change> {
    let mut out = Vec::new();
    for (terms, mart_terms, measure) in [
        (&req.measures, &m.measures, true),
        (&req.dimensions, &m.dimensions, false),
    ] {
        for t in terms {
            if let Some((_, was)) = mart_terms.iter().find(|(n, _)| n == &t.name)
                && norm(was) != norm(&t.definition)
            {
                let (name, was, now) = (t.name.clone(), was.clone(), t.definition.clone());
                out.push(if measure {
                    Change::Measure { name, was, now }
                } else {
                    Change::Dimension { name, was, now }
                });
            }
        }
    }
    if filter_set(&req.filters) != filter_set(&m.filters) {
        out.push(Change::Filters {
            was: m.filters.clone(),
            now: req.filters.clone(),
        });
    }
    let grain: Vec<String> = req.dimensions.iter().map(|d| d.name.clone()).collect();
    if BTreeSet::from_iter(grain.iter()) != BTreeSet::from_iter(m.grain.iter()) {
        out.push(Change::Grain {
            was: m.grain.clone(),
            now: grain,
        });
    }
    out
}

#[cfg(test)]
#[path = "ask_tests.rs"]
mod ask_tests;
