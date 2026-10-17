//! Source schema changes (RFC 0194): the live plan against the deployed one, by PostgreSQL's own
//! identities (table OID, column `attnum`), classified into RFC 0190's nine classes, each with an
//! action from policy. Pure: no connection, no git.

use ekos_lakehouse_bronze::LoadMode;
use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    NewTable,
    NewNullableColumn,
    Widening,
    NewRequiredColumn,
    Rename,
    Drop,
    Narrowing,
    KeyChange,
    RoutineChange,
}

impl Class {
    pub const ALL: [Class; 9] = [
        Class::NewTable,
        Class::NewNullableColumn,
        Class::Widening,
        Class::NewRequiredColumn,
        Class::Rename,
        Class::Drop,
        Class::Narrowing,
        Class::KeyChange,
        Class::RoutineChange,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Class::NewTable => "new_table",
            Class::NewNullableColumn => "new_nullable_column",
            Class::Widening => "widening",
            Class::NewRequiredColumn => "new_required_column",
            Class::Rename => "rename",
            Class::Drop => "drop",
            Class::Narrowing => "narrowing",
            Class::KeyChange => "key_change",
            Class::RoutineChange => "routine_change",
        }
    }

    /// RFC 0190 §5's defaults.
    pub fn default_action(self) -> Action {
        match self {
            Class::NewTable | Class::NewNullableColumn | Class::Widening => Action::Auto,
            _ => Action::Propose,
        }
    }

    /// While a proposal is open: does the old load still work against the new source?
    /// A load that names a column the source no longer has, or casts a type it can no longer
    /// read, does not — so the table is paused rather than failed every run.
    pub fn pauses_bronze(self) -> bool {
        matches!(self, Class::Rename | Class::Drop | Class::Narrowing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Auto,
    Propose,
}

/// `[extensions.lakehouse.policy]`: a class's name → its action.
pub type Policy = BTreeMap<String, Action>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub class: Class,
    /// `schema.table` as deployed (or as live, for a new table).
    pub table: String,
    pub column: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    /// The catalog facts the classification rests on.
    pub evidence: Vec<String>,
    pub action: Action,
}

fn qualified(t: &TablePlan) -> String {
    format!("{}.{}", t.schema, t.table)
}

/// Iceberg-level widening: every value of `from` fits `to` and Iceberg can promote in place.
fn widens(from: &str, to: &str) -> bool {
    if from == to {
        return true; // a source-only change (varchar(20) → varchar(50)): Iceberg is unaffected
    }
    if matches!((from, to), ("integer", "bigint") | ("real", "double")) {
        return true;
    }
    let dec = |t: &str| -> Option<(u32, u32)> {
        let inner = t.strip_prefix("decimal(")?.strip_suffix(')')?;
        let (p, s) = inner.split_once(',')?;
        Some((p.trim().parse().ok()?, s.trim().parse().ok()?))
    };
    matches!((dec(from), dec(to)), (Some((p1, s1)), Some((p2, s2))) if s1 == s2 && p2 >= p1)
}

/// The watermark's proof alone — the trigger and its function body (RFC 0192 §3). Other
/// evidence (the key's sentence) changes with renames, which are their own class.
fn proof(t: &TablePlan) -> Option<Vec<&String>> {
    matches!(t.decision.mode, LoadMode::Incremental { .. }).then(|| {
        t.decision
            .evidence
            .iter()
            .filter(|e| e.starts_with("trigger ") || e.contains(" body sha256 "))
            .collect()
    })
}

fn same_proof(a: &TablePlan, b: &TablePlan) -> bool {
    proof(a) == proof(b)
}

/// Every change between the deployed plan and the live one.
pub fn diff(deployed: &Plan, live: &Plan, policy: &Policy) -> Vec<Change> {
    let mut out = Vec::new();
    let action = |c: Class| policy.get(c.name()).copied().unwrap_or(c.default_action());
    let mut push = |class: Class,
                    table: String,
                    column: Option<String>,
                    from: Option<String>,
                    to: Option<String>,
                    evidence: Vec<String>| {
        out.push(Change {
            class,
            table,
            column,
            from,
            to,
            evidence,
            action: action(class),
        })
    };
    let by_oid = |p: &Plan| -> BTreeMap<u32, TablePlan> {
        p.tables.iter().map(|t| (t.oid, t.clone())).collect()
    };
    let (dep, liv) = (by_oid(deployed), by_oid(live));
    // A table refused live (RFC 0192 §2) is a finding of the plan, not a schema change.
    let refused_live: std::collections::BTreeSet<u32> =
        live.refused.iter().map(|r| r.oid).collect();

    for (oid, t) in &liv {
        if !dep.contains_key(oid) {
            push(
                Class::NewTable,
                qualified(t),
                None,
                None,
                None,
                vec![format!(
                    "table {} (oid {oid}) is not in the deployed plan",
                    qualified(t)
                )],
            );
        }
    }
    for (oid, d) in &dep {
        let Some(l) = liv.get(oid) else {
            if !refused_live.contains(oid) {
                push(
                    Class::Drop,
                    qualified(d),
                    None,
                    None,
                    None,
                    vec![format!("table oid {oid} is no longer in the source")],
                );
            }
            continue;
        };
        if (&d.schema, &d.table) != (&l.schema, &l.table) {
            push(
                Class::Rename,
                qualified(d),
                None,
                Some(qualified(d)),
                Some(qualified(l)),
                vec![format!("table oid {oid} is now named {}", qualified(l))],
            );
        }
        let attnums = |t: &TablePlan| -> BTreeMap<i32, (String, bool, bool, String)> {
            t.facts
                .columns
                .iter()
                .map(|f| {
                    let ty = t
                        .columns
                        .iter()
                        .find(|m| m.column == f.name)
                        .map(|m| m.iceberg_type.clone())
                        .unwrap_or_default();
                    (f.attnum, (f.name.clone(), f.not_null, f.has_default, ty))
                })
                .collect()
        };
        let (dc, lc) = (attnums(d), attnums(l));
        for (n, (name, not_null, has_default, ty)) in &lc {
            match dc.get(n) {
                None => {
                    let class = if !not_null && !has_default {
                        Class::NewNullableColumn
                    } else {
                        Class::NewRequiredColumn
                    };
                    push(
                        class,
                        qualified(d),
                        Some(name.clone()),
                        None,
                        Some(ty.clone()),
                        vec![format!(
                            "column {name} (attnum {n}) is new: {}{}",
                            if *not_null { "NOT NULL" } else { "nullable" },
                            if *has_default {
                                ", with a default"
                            } else {
                                ", no default"
                            }
                        )],
                    );
                }
                Some((dname, _, _, dty)) => {
                    if dname != name {
                        push(
                            Class::Rename,
                            qualified(d),
                            Some(dname.clone()),
                            Some(dname.clone()),
                            Some(name.clone()),
                            vec![format!("attnum {n} was {dname}, is now {name}")],
                        );
                    }
                    if dty != ty {
                        let class = if widens(dty, ty) {
                            Class::Widening
                        } else {
                            Class::Narrowing
                        };
                        push(
                            class,
                            qualified(d),
                            Some(name.clone()),
                            Some(dty.clone()),
                            Some(ty.clone()),
                            vec![format!("{name} lands as {ty}, deployed as {dty}")],
                        );
                    }
                }
            }
        }
        for (n, (name, ..)) in &dc {
            if !lc.contains_key(n) {
                push(
                    Class::Drop,
                    qualified(d),
                    Some(name.clone()),
                    None,
                    None,
                    vec![format!(
                        "column {name} (attnum {n}) is no longer in the source"
                    )],
                );
            }
        }
        // Keys by attnum, so a renamed key column is a rename, not a key change.
        let key_nums = |t: &TablePlan| -> Vec<i32> {
            t.primary_key
                .iter()
                .filter_map(|k| {
                    t.facts
                        .columns
                        .iter()
                        .find(|f| &f.name == k)
                        .map(|f| f.attnum)
                })
                .collect()
        };
        if key_nums(d) != key_nums(l) {
            push(
                Class::KeyChange,
                qualified(d),
                None,
                Some(d.primary_key.join(", ")),
                Some(l.primary_key.join(", ")),
                vec![format!(
                    "primary key ({}) is now ({})",
                    d.primary_key.join(", "),
                    l.primary_key.join(", ")
                )],
            );
        }
        if matches!(d.decision.mode, LoadMode::Incremental { .. }) && !same_proof(d, l) {
            let mut evidence = vec!["the deployed watermark's proof differs".to_string()];
            evidence.extend(d.decision.evidence.iter().map(|e| format!("deployed: {e}")));
            evidence.extend(l.decision.evidence.iter().map(|e| format!("now: {e}")));
            evidence.extend(l.decision.notes.iter().map(|e| format!("now: {e}")));
            push(
                Class::RoutineChange,
                qualified(d),
                None,
                Some(format!("{:?}", d.decision.mode)),
                Some(format!("{:?}", l.decision.mode)),
                evidence,
            );
        }
    }
    out.sort_by(|a, b| (&a.table, a.class, &a.column).cmp(&(&b.table, b.class, &b.column)));
    out
}

/// A stable id for a set of changes: the same changes always give the same proposal branch.
pub fn change_id(changes: &[&Change]) -> String {
    use sha2::{Digest, Sha256};
    let text = serde_json::to_string(changes).expect("changes serialize");
    format!("{:x}", Sha256::digest(text.as_bytes()))[..12].to_string()
}

/// The deployed plan with every table whose changes are all automatic taken from the live plan
/// — new tables added, columns added, types widened. Tables with a proposed change stay as
/// deployed until a person merges their proposal.
pub fn apply_auto(deployed: &Plan, live: &Plan, changes: &[Change]) -> Plan {
    let mut out = deployed.clone();
    let by_table: BTreeMap<&str, Vec<&Change>> =
        changes.iter().fold(BTreeMap::new(), |mut m, c| {
            m.entry(c.table.as_str()).or_insert_with(Vec::new).push(c);
            m
        });
    for (table, cs) in by_table {
        if cs.iter().any(|c| c.action == Action::Propose) {
            continue;
        }
        let live_t = live.tables.iter().find(|t| qualified(t) == table);
        match live_t {
            Some(l) => match out.tables.iter_mut().find(|t| t.oid == l.oid) {
                Some(slot) => *slot = l.clone(),
                None => out.tables.push(l.clone()),
            },
            None => out.tables.retain(|t| qualified(t) != table), // an auto drop by policy
        }
    }
    out.tables
        .sort_by(|a, b| (&a.schema, &a.table).cmp(&(&b.schema, &b.table)));
    out
}

/// What a merged proposal would deploy: `base` with `table` taken from the live plan (or
/// removed, when the live plan no longer has it).
pub fn with_table(base: &Plan, live: &Plan, table: &str) -> Plan {
    let mut out = base.clone();
    let dep_oid = base
        .tables
        .iter()
        .find(|t| qualified(t) == table)
        .map(|t| t.oid);
    let live_t = live
        .tables
        .iter()
        .find(|t| Some(t.oid) == dep_oid || qualified(t) == table)
        .cloned();
    out.tables
        .retain(|t| qualified(t) != table && Some(t.oid) != dep_oid);
    if let Some(l) = live_t {
        out.tables.push(l);
    }
    out.tables
        .sort_by(|a, b| (&a.schema, &a.table).cmp(&(&b.schema, &b.table)));
    out
}

/// The proposal a person reviews: what changed, why EKOS thinks so, and what it reaches.
pub fn proposal_markdown(id: &str, table: &str, changes: &[&Change], impact: &[String]) -> String {
    let mut s = format!(
        "# Schema change in `{table}` — proposal {id}\n\n\
         Generated by ekos-lakehouse (RFC 0194). EKOS does not merge this: merging it is the \
         approval.\n\n## Changes\n\n| Class | Column | From | To | While open |\n|---|---|---|---|---|\n"
    );
    for c in changes {
        s.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            c.class.name(),
            c.column.as_deref().unwrap_or("—"),
            c.from.as_deref().unwrap_or("—"),
            c.to.as_deref().unwrap_or("—"),
            if c.class.pauses_bronze() {
                "bronze paused"
            } else {
                "bronze continues"
            }
        ));
    }
    s.push_str("\n## Evidence\n\n");
    for c in changes {
        for e in &c.evidence {
            s.push_str(&format!("- {e}\n"));
        }
    }
    s.push_str("\n## Impact\n\n");
    if impact.is_empty() {
        s.push_str("- nothing compiled reads it\n");
    }
    for i in impact {
        s.push_str(&format!("- {i}\n"));
    }
    s.push_str(
        "\n## After merging\n\nThe next `lakehouse sync` finds the deployed plan equal to the \
         source for this table and resumes it. A dropped or renamed column's history stays in \
         Iceberg's snapshots.\n",
    );
    s
}

#[cfg(test)]
mod tests;
