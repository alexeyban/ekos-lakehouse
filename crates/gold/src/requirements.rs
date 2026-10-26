//! EKOS RFC 0215 (step 4): a mart requirement as an editable description, and the catalogue
//! excerpt a model drafts one from. Pure: no model, no file, no connection.

use crate::MartSpec;
use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_silver::{SilverSettings, flag};
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The description `parse` reads back: the header in the order written, then the prose.
pub fn render(spec: &MartSpec) -> String {
    let mut h = Mapping::new();
    h.insert("mart".into(), spec.mart.clone().into());
    h.insert(
        "grain".into(),
        Value::Sequence(spec.grain.iter().map(|g| g.as_str().into()).collect()),
    );
    for (key, items) in [
        ("dimensions", &spec.dimensions),
        ("measures", &spec.measures),
    ] {
        let mut m = Mapping::new();
        for (n, v) in items {
            m.insert(n.as_str().into(), v.as_str().into());
        }
        h.insert(key.into(), Value::Mapping(m));
    }
    if !spec.filters.is_empty() {
        h.insert(
            "filters".into(),
            Value::Sequence(spec.filters.iter().map(|f| f.as_str().into()).collect()),
        );
    }
    let header = serde_yaml::to_string(&Value::Mapping(h)).expect("a mapping serializes");
    let mut out = format!("---\n{header}---\n");
    if !spec.prose.is_empty() {
        out.push('\n');
        out.push_str(&spec.prose);
        out.push('\n');
    }
    out
}

/// One change an operator makes to a requirement (RFC 0215 §2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Replace a dimension or measure, or add one to `section` (default: where it already is).
    Term {
        name: String,
        value: String,
        section: Option<String>,
    },
    /// Remove a dimension or measure (and the dimension from the grain).
    DropTerm(String),
    AddFilter(String),
    DropFilter(String),
}

pub fn apply(spec: &mut MartSpec, e: &Edit) -> Result<String, String> {
    match e {
        Edit::Term {
            name,
            value,
            section,
        } => {
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err("a term needs a definition".into());
            }
            let place = |items: &mut Vec<(String, String)>| -> bool {
                match items.iter_mut().find(|(n, _)| n == name) {
                    Some(t) => {
                        t.1 = value.clone();
                        true
                    }
                    None => false,
                }
            };
            match section.as_deref() {
                None => {
                    if place(&mut spec.dimensions) || place(&mut spec.measures) {
                        Ok(format!("`{name}` is now `{value}`"))
                    } else {
                        Err(format!(
                            "`{name}` is not a term of this mart: add it with a section \
                             (dimensions or measures)"
                        ))
                    }
                }
                Some(s @ ("dimensions" | "measures")) => {
                    let items = if s == "dimensions" {
                        &mut spec.dimensions
                    } else {
                        &mut spec.measures
                    };
                    if place(items) {
                        return Ok(format!("`{name}` is now `{value}`"));
                    }
                    items.push((name.clone(), value.clone()));
                    if s == "dimensions" && !spec.grain.contains(name) {
                        spec.grain.push(name.clone());
                    }
                    Ok(format!("{s}: `{name}` added as `{value}`"))
                }
                Some(other) => Err(format!(
                    "no section `{other}`: dimensions or measures (filters have their own edit)"
                )),
            }
        }
        Edit::DropTerm(name) => {
            let before = spec.dimensions.len() + spec.measures.len();
            spec.dimensions.retain(|(n, _)| n != name);
            spec.measures.retain(|(n, _)| n != name);
            spec.grain.retain(|g| g != name);
            if spec.dimensions.len() + spec.measures.len() == before {
                Err(format!("`{name}` is not a term of this mart"))
            } else {
                Ok(format!("`{name}` removed"))
            }
        }
        Edit::AddFilter(f) => {
            let f = f.trim().to_string();
            if f.is_empty() || spec.filters.contains(&f) {
                return Err(format!("the filter `{f}` is empty or already there"));
            }
            spec.filters.push(f.clone());
            Ok(format!("filter `{f}` added"))
        }
        Edit::DropFilter(f) => {
            let before = spec.filters.len();
            spec.filters.retain(|x| x != f.trim());
            if spec.filters.len() == before {
                Err(format!("there is no filter `{f}`"))
            } else {
                Ok(format!("filter `{f}` removed"))
            }
        }
    }
}

// ─────────────────────────────── the catalogue excerpt ───────────────────────────────

const STOP: &[&str] = &[
    "the", "and", "for", "with", "from", "per", "all", "each", "which", "that", "this", "are",
    "was", "were", "show", "give", "want", "need", "report", "over", "into", "their", "them",
];

fn stem(w: &str) -> String {
    let w = w.to_ascii_lowercase();
    if let Some(s) = w.strip_suffix("ies")
        && s.len() >= 3
    {
        return format!("{s}y");
    }
    match w.strip_suffix('s') {
        Some(s) if s.len() >= 3 && !s.ends_with('s') => s.to_string(),
        _ => w,
    }
}

fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(stem)
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect()
}

/// The silver tables a request's words point at, scored: a word in the table's name counts 3,
/// in its comment 2, in a column's name 1. Among equal scores, a table whose name says less
/// besides comes first (`M_PRODUCT_CATEGORY` before `M_PRODUCT_CATEGORY_TRL`). Pure and
/// deterministic.
pub fn rank(plan: &Plan, request: &str) -> Vec<(String, u32)> {
    let asked = words(request);
    let extra = |table: &str| {
        words(&table.replace('_', " "))
            .iter()
            .filter(|w| !asked.contains(*w))
            .count()
    };
    let mut scored: Vec<(String, u32)> = plan
        .tables
        .iter()
        .map(|t| {
            let name = words(&t.table.replace('_', " "));
            let comment = words(t.facts.comment.as_deref().unwrap_or_default());
            let cols: BTreeSet<String> = t
                .columns
                .iter()
                .flat_map(|c| words(&c.column.replace('_', " ")))
                .collect();
            let score = asked
                .iter()
                .map(|w| {
                    3 * u32::from(name.contains(w))
                        + 2 * u32::from(comment.contains(w))
                        + u32::from(cols.contains(w))
                })
                .sum();
            (t.table.clone(), score)
        })
        .collect();
    // A table the best matches point at by a key is a little more likely meant.
    let mut top: Vec<&(String, u32)> = scored.iter().filter(|(_, s)| *s > 0).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let referenced: BTreeSet<String> = top
        .iter()
        .take(3)
        .filter_map(|(n, _)| plan.tables.iter().find(|t| &t.table == n))
        .flat_map(|t| t.facts.foreign_keys.iter().map(|fk| fk.ref_table.clone()))
        .collect();
    for (n, s) in &mut scored {
        if *s > 0 && referenced.contains(n) {
            *s += 1;
        }
    }
    scored.retain(|(_, s)| *s > 0);
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(extra(&a.0).cmp(&extra(&b.0)))
            .then(a.0.cmp(&b.0))
    });
    scored
}

/// The tables to show: first, for each word of the request in turn, the best-ranked table whose
/// name has it (so "region" is not crowded out by tables that match "product category" twice),
/// then the rest by rank, at most `max`.
pub fn choose(plan: &Plan, request: &str, max: usize) -> Vec<String> {
    let ranked = rank(plan, request);
    let named = |t: &str, w: &str| words(&t.replace('_', " ")).contains(w);
    let mut asked: Vec<String> = request
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(stem)
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect();
    asked.dedup();
    let mut out: Vec<String> = vec![];
    for w in &asked {
        if out.len() >= max || out.iter().any(|t| named(t, w)) {
            continue;
        }
        if let Some((t, _)) = ranked.iter().find(|(t, _)| named(t, w)) {
            out.push(t.clone());
        }
    }
    for (t, _) in &ranked {
        if out.len() >= max {
            break;
        }
        if !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

pub struct Excerpt {
    pub tables: Vec<String>,
    pub text: String,
}

fn describe(t: &TablePlan, shown: &BTreeSet<&str>, silver: &SilverSettings) -> String {
    let mut s = format!("## {}", t.table);
    if let Some(c) = &t.facts.comment {
        s.push_str(&format!(" — {}", c.replace('\n', " ")));
    }
    s.push('\n');
    let cols: Vec<String> = t
        .columns
        .iter()
        .map(|c| {
            let facts = t.column_facts(&c.column);
            let ty = if flag(t, &c.column, silver).is_some() {
                "boolean".to_string()
            } else {
                c.iceberg_type.clone()
            };
            let mut x = format!("{} {ty}", c.column);
            let codes = facts
                .and_then(|f| f.code.as_ref())
                .map(|k| k.values.clone())
                .or_else(|| {
                    facts
                        .and_then(|f| f.check.as_ref())
                        .map(|k| k.values.clone())
                })
                .unwrap_or_default();
            if !codes.is_empty() && codes.len() <= 20 && ty != "boolean" {
                x.push_str(&format!(" (codes {})", codes.join(" ")));
            }
            x
        })
        .collect();
    s.push_str(&format!("columns: {}\n", cols.join(", ")));
    let keys: Vec<String> = t
        .facts
        .foreign_keys
        .iter()
        .filter(|fk| fk.columns.len() == 1 && shown.contains(fk.ref_table.as_str()))
        .map(|fk| format!("{} → {}", fk.columns[0], fk.ref_table))
        .collect();
    if !keys.is_empty() {
        s.push_str(&format!("keys: {}\n", keys.join(", ")));
    }
    s
}

/// The only schema a model sees (RFC 0215 §2.3): the best-ranked tables, at most `max`, with
/// their columns (silver types: flags are booleans), code lists and the keys among them.
pub fn excerpt(plan: &Plan, request: &str, max: usize, silver: &SilverSettings) -> Excerpt {
    let tables = choose(plan, request, max);
    let shown: BTreeSet<&str> = tables.iter().map(String::as_str).collect();
    let by_name: BTreeMap<&str, &TablePlan> =
        plan.tables.iter().map(|t| (t.table.as_str(), t)).collect();
    let text = tables
        .iter()
        .filter_map(|t| by_name.get(t.as_str()))
        .map(|t| describe(t, &shown, silver))
        .collect::<Vec<_>>()
        .join("\n");
    Excerpt { tables, text }
}

/// A model may list dimensions or measures without names (`- count of c_order`): each gets
/// a name made from its definition (`count_of_c_order`). A name is only a label; what it means
/// is still resolved from the definition. Anything else is left for `parse` to judge.
fn name_listed_terms(header: &str) -> String {
    let Ok(Value::Mapping(mut doc)) = serde_yaml::from_str::<Value>(header) else {
        return header.to_string();
    };
    let mut changed = false;
    for key in ["dimensions", "measures"] {
        let Some(Value::Sequence(items)) = doc.get(key).cloned() else {
            continue;
        };
        let defs: Vec<String> = items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if defs.len() != items.len() {
            continue;
        }
        let mut m = Mapping::new();
        for d in defs {
            let base: String = d
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("_");
            let base: String = base.chars().take(40).collect();
            let mut name = base.clone();
            let mut n = 2;
            while m.contains_key(name.as_str()) {
                name = format!("{base}_{n}");
                n += 1;
            }
            m.insert(name.into(), d.into());
        }
        doc.insert(key.into(), Value::Mapping(m));
        changed = true;
    }
    if changed {
        serde_yaml::to_string(&Value::Mapping(doc)).unwrap_or_else(|_| header.to_string())
    } else {
        header.to_string()
    }
}

/// A model's answer, cleaned to the description it should be: code fences and talk around the
/// header dropped, the mart name the operator gave, the request as the prose.
pub fn from_answer(answer: &str, mart: &str, request: &str) -> Result<MartSpec, String> {
    let a = answer.replace("\r\n", "\n");
    let body = a
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    // The header between `---` lines, or (models drop them) from a `mart:` line on.
    let rest = match body.find("---\n") {
        Some(start) => &body[start + 4..],
        None => {
            let start = body
                .find("mart:")
                .ok_or("the answer has no `---` header and no `mart:` line")?;
            &body[start..]
        }
    };
    let end = rest.find("\n---").map(|e| e + 1).unwrap_or(rest.len());
    let named = name_listed_terms(&rest[..end]);
    let header = named.as_str();
    let mut spec = crate::parse(&format!("---\n{}\n---\n", header.trim_end())).or_else(|_| {
        // The model may name the mart its own way: the operator's name replaces it.
        let fixed: String = header
            .lines()
            .map(|l| {
                if l.starts_with("mart:") {
                    format!("mart: {mart}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        crate::parse(&format!("---\n{}\n---\n", fixed.trim_end()))
    })?;
    spec.mart = mart.to_string();
    spec.prose = request.trim().to_string();
    if spec.measures.is_empty() {
        return Err("the header has no measures".into());
    }
    Ok(spec)
}
