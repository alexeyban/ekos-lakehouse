//! EKOS RFC 0257 §3.5 — structured edits of a mart, made on a step of its chain.
//!
//! A mart is edited by changing its requirement, never its SQL. The edits are a closed set
//! ([`StepEdit`]); each becomes a requirement edit (`crate::requirements::Edit`), is classified
//! additive or modifying (RFC 0248 §2.2), and must pass the static gates here — the term
//! resolves, the edit's own constraint holds, the compiled mart invents nothing — before the
//! engine gates (the grain stays unique, every additive measure reconciles with silver) and a
//! person's confirmation of a modifying change. Pure: no engine, no file.

use crate::requirements::{self as req, Edit};
use crate::{MartSpec, Term, check, compile, questions, resolve_with};
use ekos_lakehouse_bronze::plan::Plan;
use serde::{Deserialize, Serialize};

/// The closed set of edits (RFC 0257 §3.5). Anything else is refused with the reason, and the
/// person is offered a comment instead. The JSON shape is what the assistant (§3.6) must return.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepEdit {
    /// `table.column <op> literal`, `… is true|false|null|not null`.
    AddFilter {
        filter: String,
    },
    ChangeFilter {
        from: String,
        to: String,
    },
    DropFilter {
        filter: String,
    },
    /// Choose the join path of a term's table by a declared foreign key (`table.column`).
    ChangeJoin {
        term: String,
        via: String,
    },
    /// `table.column` or `day|week|month|quarter|year of table.column`; joins the grain.
    AddDimension {
        name: String,
        definition: String,
    },
    DropDimension {
        name: String,
    },
    /// `sum of table.column` or `count of table`: an additive measure, which gets its
    /// reconciliation test.
    AddMeasure {
        name: String,
        definition: String,
    },
    DropMeasure {
        name: String,
    },
}

impl StepEdit {
    /// The kind, as the console and the log name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AddFilter { .. } => "add_filter",
            Self::ChangeFilter { .. } => "change_filter",
            Self::DropFilter { .. } => "drop_filter",
            Self::ChangeJoin { .. } => "change_join",
            Self::AddDimension { .. } => "add_dimension",
            Self::DropDimension { .. } => "drop_dimension",
            Self::AddMeasure { .. } => "add_measure",
            Self::DropMeasure { .. } => "drop_measure",
        }
    }

    /// The step kinds an edit may be made on (§3.5's "On" column).
    pub fn on(&self) -> &'static [&'static str] {
        match self {
            Self::AddFilter { .. } => &["Filter", "Source"],
            Self::ChangeFilter { .. } | Self::DropFilter { .. } => &["Filter"],
            Self::ChangeJoin { .. } => &["Join"],
            _ => &["Aggregate"],
        }
    }
}

/// One gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gate {
    pub gate: &'static str,
    pub passed: bool,
    pub detail: String,
}

/// RFC 0248 §2.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Class {
    /// `additive` or `modifying`.
    pub class: &'static str,
    pub reasons: Vec<String>,
}

/// The requirement after the edit, or why the edit cannot be expressed.
pub fn apply(spec: &MartSpec, e: &StepEdit) -> Result<(MartSpec, String), String> {
    let mut s = spec.clone();
    let said = match e {
        StepEdit::AddFilter { filter } => req::apply(&mut s, &Edit::AddFilter(filter.clone()))?,
        StepEdit::DropFilter { filter } => req::apply(&mut s, &Edit::DropFilter(filter.clone()))?,
        StepEdit::ChangeFilter { from, to } => {
            let pos = s
                .filters
                .iter()
                .position(|f| f == from.trim())
                .ok_or_else(|| format!("there is no filter `{from}`"))?;
            if to.trim().is_empty() {
                return Err("the new filter is empty".into());
            }
            s.filters[pos] = to.trim().to_string();
            format!("filter `{from}` is now `{to}`")
        }
        StepEdit::ChangeJoin { term, via } => {
            let current = s
                .dimensions
                .iter()
                .chain(s.measures.iter())
                .find(|(n, _)| n == term)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| format!("`{term}` is not a term of this mart"))?;
            let lower = current.to_ascii_lowercase();
            let base = match lower.rfind(" via ") {
                Some(i) => current[..i].to_string(),
                None => current.clone(),
            };
            req::apply(
                &mut s,
                &Edit::Term {
                    name: term.clone(),
                    value: format!("{base} via {}", via.trim()),
                    section: None,
                },
            )?
        }
        StepEdit::AddDimension { name, definition } | StepEdit::AddMeasure { name, definition } => {
            let section = if matches!(e, StepEdit::AddDimension { .. }) {
                "dimensions"
            } else {
                "measures"
            };
            if s.dimensions
                .iter()
                .chain(s.measures.iter())
                .any(|(n, _)| n == name)
            {
                return Err(format!(
                    "`{name}` is already a term: change it, or pick a new name"
                ));
            }
            req::apply(
                &mut s,
                &Edit::Term {
                    name: name.clone(),
                    value: definition.clone(),
                    section: Some(section.into()),
                },
            )?
        }
        StepEdit::DropDimension { name } => {
            if !s.dimensions.iter().any(|(n, _)| n == name) {
                return Err(format!("`{name}` is not a dimension of this mart"));
            }
            req::apply(&mut s, &Edit::DropTerm(name.clone()))?
        }
        StepEdit::DropMeasure { name } => {
            if !s.measures.iter().any(|(n, _)| n == name) {
                return Err(format!("`{name}` is not a measure of this mart"));
            }
            if s.measures.len() == 1 {
                return Err("a mart needs at least one measure".into());
            }
            req::apply(&mut s, &Edit::DropTerm(name.clone()))?
        }
    };
    Ok((s, said))
}

/// RFC 0248 §2.2: adding a column is additive — every existing column keeps its values — unless
/// it changes the grain, which reclassifies it modifying (a new dimension always does); every
/// other edit changes what existing columns hold, or which rows there are, and is modifying.
pub fn classify(before: &MartSpec, after: &MartSpec, e: &StepEdit) -> Class {
    let mut reasons = vec![];
    // A new column, of either kind, leaves the existing ones as they are …
    let mut additive = matches!(
        e,
        StepEdit::AddMeasure { .. } | StepEdit::AddDimension { .. }
    );
    if additive {
        reasons.push("a new column: existing columns keep their values".into());
    } else {
        reasons.push(format!(
            "{}: existing rows or values change",
            e.kind().replace('_', " ")
        ));
    }
    // … unless it changes the grain: then every existing row is split or merged (RFC 0257 §5).
    if before.grain != after.grain {
        additive = false;
        reasons.push(format!(
            "the grain changes from one row per {} to one row per {}",
            before.grain.join(", "),
            after.grain.join(", ")
        ));
    }
    Class {
        class: if additive { "additive" } else { "modifying" },
        reasons,
    }
}

/// The edit's own constraint (§3.5's third column), on the resolved requirement.
fn constraint(after: &MartSpec, resolved: &[crate::Resolved], e: &StepEdit) -> Gate {
    let term = |section: &str, name: &str| {
        resolved
            .iter()
            .find(|r| r.section == section && r.term == name)
            .and_then(|r| r.resolution.as_ref().ok())
    };
    let (passed, detail) = match e {
        StepEdit::AddFilter { filter } | StepEdit::ChangeFilter { to: filter, .. } => {
            match term("filters", filter.trim()) {
                Some(Term::Predicate { .. }) => (true, "a predicate on a resolved column".into()),
                Some(_) => (
                    false,
                    format!(
                        "`{filter}` is not a predicate the description grammar states (column op value)"
                    ),
                ),
                None => (false, format!("`{filter}` does not resolve")),
            }
        }
        StepEdit::AddMeasure { name, .. } => match term("measures", name) {
            Some(Term::Aggregate {
                func,
                distinct: false,
                ..
            }) if func == "sum" || func == "count" => {
                (true, format!("{func}: additive, reconciled with silver"))
            }
            Some(Term::Aggregate { func, .. }) => (
                false,
                format!("`{func}` is not additive: a new measure is a sum or a count"),
            ),
            _ => (
                false,
                format!("`{name}` is not `sum of table.column` or `count of table`"),
            ),
        },
        StepEdit::AddDimension { name, .. } => match term("dimensions", name) {
            Some(Term::Column(_) | Term::TimeGrain { .. }) => (
                true,
                format!(
                    "a resolved column; the grain becomes {}",
                    after.grain.join(", ")
                ),
            ),
            _ => (
                false,
                format!("`{name}` is not a column or a time grain of one"),
            ),
        },
        StepEdit::ChangeJoin { term: t, via } => {
            let r = resolved.iter().find(|r| &r.term == t);
            match r {
                Some(r) if r.resolution.is_ok() && !r.via.is_empty() => (
                    true,
                    format!("joined along the declared key `{}`", via.trim()),
                ),
                _ => (
                    false,
                    format!("`{via}` is not a declared foreign key on a path to `{t}`'s table"),
                ),
            }
        }
        StepEdit::DropFilter { .. }
        | StepEdit::DropDimension { .. }
        | StepEdit::DropMeasure { .. } => (true, "a removal".into()),
    };
    Gate {
        gate: "constraint",
        passed,
        detail,
    }
}

/// The static gates of RFC 0248 §2.3 on the edited requirement: every term resolves; the edit's
/// constraint holds; the compiled mart invents nothing. The engine gates are the caller's.
pub fn static_gates(
    after: &MartSpec,
    plan: &Plan,
    instructions: &[crate::Instruction],
    e: &StepEdit,
) -> (Vec<Gate>, Option<crate::Compiled>) {
    let (spec, resolved, _) = resolve_with(after, plan, instructions);
    let qs = questions(&resolved);
    let mut gates = vec![Gate {
        gate: "resolution",
        passed: qs.is_empty(),
        detail: if qs.is_empty() {
            format!("{} term(s) resolve", resolved.len())
        } else {
            qs.iter()
                .map(|(r, q)| format!("`{}`: {}", r.term, q.text))
                .collect::<Vec<_>>()
                .join("; ")
        },
    }];
    gates.push(constraint(&spec, &resolved, e));
    if !qs.is_empty() {
        return (gates, None);
    }
    match compile(&spec, plan, &resolved, &Default::default()) {
        Ok(c) => {
            let report = check(&c);
            gates.push(Gate {
                gate: "invention",
                passed: report.accepted(),
                detail: format!(
                    "{} atoms checked, {} not stated by the requirement",
                    report.checked,
                    report.unmapped.len()
                ),
            });
            (gates, Some(c))
        }
        Err(qs) => {
            gates.push(Gate {
                gate: "compile",
                passed: false,
                detail: qs
                    .iter()
                    .map(|q| q.text.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            });
            (gates, None)
        }
    }
}
