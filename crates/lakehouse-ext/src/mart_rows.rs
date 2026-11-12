//! EKOS RFC 0257 §3.3 — how many rows pass each step of a mart's chain.
//!
//! Each step's count is one aggregate query on the engine over the chain up to that step, built
//! from the same Transformation IR graph the chain is drawn from (so a hand-edited model is
//! counted as it runs). RFC 0254's probe rules apply: aggregates only (no value leaves the
//! engine), a budget of queries per request, small cells hidden, a step that reads a personal
//! column withheld with everything after it, and nothing is persisted.
//!
//! A join shows rows in from each side, rows out, and unmatched rows on each side — where fan-out
//! and silent row loss become visible. Pure: [`plan`] writes the SQL, [`present`] applies the
//! rules to what the caller ran.

use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::typemap::ident;
use ekos_semantic::transform_ir::{TransformGraph, TransformNode};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// The most queries one request runs (RFC 0257 open question 2: all steps at once, behind one
/// button, with this stated budget).
pub const BUDGET: usize = 60;

/// One query of one step: what it measures and its SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub step: u32,
    /// `out`, `matched` or `unmatched_right`.
    pub measure: &'static str,
    pub sql: String,
}

/// What [`plan`] decided for each step besides its queries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan2 {
    pub queries: Vec<Query>,
    /// Steps whose rows in come from these steps' rows out.
    pub inputs: BTreeMap<u32, Vec<u32>>,
    /// Steps whose count equals another's (a calculation, the mart), by that step.
    pub same_as: BTreeMap<u32, u32>,
    /// Steps withheld, with the reason (a personal column; budget).
    pub withheld: BTreeMap<u32, String>,
    /// Steps that are joins.
    pub joins: Vec<u32>,
    /// Steps that are filters (they remove rows; an aggregate groups them).
    pub filters: Vec<u32>,
    /// Steps whose rows out are known without a query.
    pub constants: BTreeMap<u32, i64>,
}

/// `"ADEMPIERE__C_ORDER"` → `"iceberg"."silver_<tag>ADEMPIERE"."C_ORDER"` (as
/// `ekos_lakehouse_gold::engine_sql` reads a ref).
pub fn engine_table(model: &str, catalog: &str, tag: &str) -> String {
    let model = model.trim_matches('"');
    let (schema, table) = model.split_once("__").unwrap_or(("public", model));
    format!(
        "{}.{}.{}",
        ident(catalog),
        ident(&format!("silver_{tag}{schema}")),
        ident(table)
    )
}

/// Columns a predicate or key list names (`"T"."C"` → `C`), outside string literals.
fn columns_named(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut quoted = false;
    let mut word = String::new();
    let flush = |w: &mut String, out: &mut Vec<String>| {
        if !w.is_empty() {
            out.push(
                w.rsplit('.')
                    .next()
                    .unwrap_or(w)
                    .trim_matches('"')
                    .to_string(),
            );
            w.clear();
        }
    };
    for c in text.chars() {
        if c == '\'' {
            quoted = !quoted;
            flush(&mut word, &mut out);
        } else if !quoted && (c.is_alphanumeric() || matches!(c, '_' | '"' | '.')) {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
        }
    }
    flush(&mut word, &mut out);
    out.retain(|w| w.chars().next().is_some_and(|c| c.is_alphabetic()));
    out
}

/// Whether a column holds personal data: flagged in the plan, or named like it (the shared
/// RFC 0043/0211 rules).
pub fn is_personal(plan: &Plan, column: &str) -> bool {
    let flagged = plan.tables.iter().any(|t| {
        t.facts
            .columns
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(column) && c.pii_candidate.is_some())
    });
    flagged || ekos_common::pii::classify_name(column).is_some()
}

fn personal_in(plan: &Plan, text: &str) -> Option<String> {
    columns_named(text)
        .into_iter()
        .find(|c| is_personal(plan, c))
}

/// The queries for every step of `graph` (split filters, the mart as its last step).
pub fn plan(graph: &TransformGraph, catalog: &str, tag: &str, plan: &Plan) -> Plan2 {
    let mut p = Plan2::default();
    let mut from = String::new();
    let mut wheres: Vec<String> = vec![];
    // The step whose rows out are the relation built so far.
    let mut current: Option<u32> = None;
    let mut last_count: Option<u32> = None;
    let mut personal: Option<String> = None;
    let table_of = |i: usize| match &graph.nodes[i] {
        TransformNode::Source {
            object_name, alias, ..
        } => Some((
            engine_table(object_name, catalog, tag),
            alias
                .clone()
                .unwrap_or_else(|| object_name.trim_matches('"').to_string()),
        )),
        _ => None,
    };
    let push = |p: &mut Plan2, step: u32, measure, sql: String| {
        if p.queries.len() >= BUDGET {
            p.withheld
                .entry(step)
                .or_insert_with(|| format!("not measured: over the budget of {BUDGET} queries"));
        } else {
            p.queries.push(Query { step, measure, sql });
        }
    };
    for (i, node) in graph.nodes.iter().enumerate() {
        let step = i as u32;
        if let Some(why) = &personal {
            p.withheld.insert(step, why.clone());
        }
        match node {
            TransformNode::Source { .. } => {
                let (t, a) = table_of(i).expect("a source");
                if from.is_empty() {
                    from = format!("{t} as {}", ident(&a));
                    current = Some(step);
                }
                push(&mut p, step, "out", format!("select count(*) from {t}"));
                last_count = Some(step);
            }
            TransformNode::Join {
                left, right, keys, ..
            } => {
                let (rt, ra) = table_of(right.0 as usize).unwrap_or_default();
                let on = keys
                    .iter()
                    .map(|(a, b)| format!("{a} = {b}"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                if let Some(c) = personal_in(plan, &on) {
                    personal.get_or_insert(format!("withheld: the join key `{c}` is personal"));
                    p.withheld
                        .insert(step, personal.clone().unwrap_or_default());
                }
                let before = from.clone();
                from = format!("{from}\nleft join {rt} as {} on {on}", ident(&ra));
                // The right side's key: the one of each pair qualified by its alias.
                let right_key = keys
                    .iter()
                    .map(|(a, b)| {
                        let q = format!("{}.", ident(&ra)).to_ascii_lowercase();
                        if b.to_ascii_lowercase().starts_with(&q) {
                            b
                        } else {
                            a
                        }
                    })
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "1".into());
                push(&mut p, step, "out", format!("select count(*) from {from}"));
                push(
                    &mut p,
                    step,
                    "matched",
                    format!("select count({right_key}) from {from}"),
                );
                push(
                    &mut p,
                    step,
                    "unmatched_right",
                    format!(
                        "select count(*) from {rt} as {} where not exists (select 1 from {before} \
                         where {on})",
                        ident(&ra)
                    ),
                );
                let ins: Vec<u32> = [current.unwrap_or(left.0), right.0].into();
                p.inputs.insert(step, ins);
                p.joins.push(step);
                current = Some(step);
                last_count = Some(step);
            }
            TransformNode::Filter { condition } => {
                if let Some(c) = personal_in(plan, condition) {
                    personal.get_or_insert(format!("withheld: the filter reads `{c}`, personal"));
                    p.withheld
                        .insert(step, personal.clone().unwrap_or_default());
                }
                wheres.push(condition.clone());
                p.filters.push(step);
                push(
                    &mut p,
                    step,
                    "out",
                    format!(
                        "select count(*) from {from}\nwhere {}",
                        wheres.join("\n  and ")
                    ),
                );
                p.inputs.insert(step, current.into_iter().collect());
                current = Some(step);
                last_count = Some(step);
            }
            TransformNode::Aggregate { group_by, .. } => {
                if let Some(c) = personal_in(plan, &group_by.join(", ")) {
                    personal.get_or_insert(format!("withheld: grouped by `{c}`, personal"));
                    p.withheld
                        .insert(step, personal.clone().unwrap_or_default());
                }
                let w = if wheres.is_empty() {
                    String::new()
                } else {
                    format!("\nwhere {}", wheres.join("\n  and "))
                };
                if group_by.is_empty() {
                    // An aggregate without groups returns one row, whatever comes in.
                    p.constants.insert(step, 1);
                } else {
                    let sql = format!(
                        "select count(*) from (select 1 from {from}{w}\ngroup by {}) t",
                        group_by.join(", ")
                    );
                    push(&mut p, step, "out", sql);
                }
                p.inputs.insert(step, current.into_iter().collect());
                current = Some(step);
                last_count = Some(step);
            }
            TransformNode::Calculate { .. } | TransformNode::Sink { .. } => {
                if let Some(c) = last_count {
                    p.same_as.insert(step, c);
                }
                p.inputs.insert(step, current.into_iter().collect());
                current = Some(step);
            }
            TransformNode::Unmapped { .. } => {
                p.withheld.insert(
                    step,
                    "not measured: a step the parser did not understand".into(),
                );
            }
        }
    }
    p
}

/// A count as shown: below `min_cell` (and above zero) hidden.
fn cell(n: Option<i64>, min_cell: i64) -> Value {
    match n {
        None => Value::Null,
        Some(n) if n > 0 && n < min_cell => json!(format!("<{min_cell}")),
        Some(n) => json!(n),
    }
}

/// What a step shows, from the counts the caller ran (`step → measure → count`).
///
/// A count below `min_cell` (and above zero) is hidden. A difference that is itself a small cell
/// is hidden too, so two shown counts never give it away: a filter that removes fewer than
/// `min_cell` rows shows its rows out as `<in> − <k`, and every later step reads that display as
/// its rows in. Downstream of a hidden count, a step shows no `removed` and a row-preserving
/// step (a many-to-one join) shows its input's display rather than an exact number that would
/// undo it.
pub fn present(
    p: &Plan2,
    counts: &BTreeMap<u32, BTreeMap<&'static str, i64>>,
    steps: u32,
    min_cell: i64,
) -> BTreeMap<u32, Value> {
    let raw = |s: u32| -> Option<i64> {
        let s = p.same_as.get(&s).copied().unwrap_or(s);
        p.constants
            .get(&s)
            .copied()
            .or_else(|| counts.get(&s).and_then(|m| m.get("out")).copied())
    };
    let small = |n: i64| n > 0 && n < min_cell;
    // The display of each step's rows out, and whether it is exact.
    let mut shown: BTreeMap<u32, (Value, bool)> = BTreeMap::new();
    let mut res = BTreeMap::new();
    for step in 0..steps {
        if let Some(why) = p.withheld.get(&step) {
            res.insert(step, json!({ "withheld": why }));
            shown.insert(step, (Value::Null, false));
            continue;
        }
        let inputs: Vec<u32> = p.inputs.get(&step).cloned().unwrap_or_default();
        let ins: Vec<(Value, bool)> = inputs
            .iter()
            .map(|i| shown.get(i).cloned().unwrap_or((Value::Null, false)))
            .collect();
        let exact_in = ins.iter().all(|(_, e)| *e);
        let out = raw(step);
        let mut display = (cell(out, min_cell), out.is_some_and(|o| !small(o)));
        let mut v = json!({ "in": ins.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>() });
        let first_in = inputs.first().and_then(|i| raw(*i));
        if p.filters.contains(&step) {
            match (first_in, out) {
                (Some(i), Some(o)) if exact_in => {
                    let removed = i - o;
                    if small(removed) && !small(i) {
                        display = (json!(format!("{i} − <{min_cell}")), false);
                        v["removed"] = json!(format!("<{min_cell}"));
                    } else {
                        v["removed"] = cell(Some(removed), min_cell);
                    }
                }
                _ => {}
            }
        } else if let Some(s) = p.same_as.get(&step) {
            display = shown.get(s).cloned().unwrap_or(display);
        }
        if p.joins.contains(&step) {
            let m = counts.get(&step);
            let matched = m.and_then(|m| m.get("matched")).copied();
            v["unmatched_left"] = cell(out.zip(matched).map(|(o, k)| o - k), min_cell);
            v["unmatched_right"] =
                cell(m.and_then(|m| m.get("unmatched_right")).copied(), min_cell);
            // A many-to-one join never repeats a row: more out than in is fan-out.
            if let (Some(i), Some(o)) = (first_in, out) {
                v["fan_out"] = json!(o > i);
                if !exact_in && o == i {
                    display = ins[0].clone();
                }
            }
        }
        v["out"] = display.0.clone();
        shown.insert(step, display);
        res.insert(step, v);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mart_graph::{MartSql, mart_ir};
    use ekos_lakehouse_bronze::plan::PlanSettings;

    fn empty_plan() -> Plan {
        ekos_lakehouse_bronze::plan::plan(&[], &PlanSettings::default())
    }

    fn graph() -> TransformGraph {
        let m = MartSql {
            mart: "sales_by_month",
            path: "m.sql",
            dbt_sql: crate::mart_graph::tests::SALES,
            version: "committed",
            grain: &[],
            resolved: &[],
            clauses: &[],
            notes_by_table: Default::default(),
            notes_by_term: Default::default(),
        };
        mart_ir(&m).unwrap().0
    }

    #[test]
    fn each_step_counts_the_chain_up_to_it() {
        let p = plan(&graph(), "iceberg", "idem_", &empty_plan());
        let sql = |step: u32, m: &str| {
            p.queries
                .iter()
                .find(|q| q.step == step && q.measure == m)
                .map(|q| q.sql.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            sql(0, "out"),
            "select count(*) from \"iceberg\".\"silver_idem_ADEMPIERE\".\"C_ORDER\""
        );
        assert!(sql(2, "out").contains(
            "left join \"iceberg\".\"silver_idem_ADEMPIERE\".\"C_BPARTNER\" as \"C_BPARTNER\""
        ));
        assert!(sql(2, "matched").starts_with("select count(\"C_BPARTNER\".\"C_BPARTNER_ID\")"));
        assert!(sql(2, "unmatched_right").contains("where not exists (select 1 from"));
        assert!(sql(3, "out").ends_with("where \"C_ORDER\".\"ISSOTRX\" = true"));
        assert!(sql(4, "out").contains("and \"C_ORDER\".\"DOCSTATUS\" = 'CO'"));
        assert!(sql(5, "out").contains("group by date_trunc('month'"));
        assert_eq!(p.same_as[&6], 5);
        assert_eq!(p.same_as[&7], 5);
        assert_eq!(p.inputs[&2], [0, 1]);
        assert!(p.queries.len() <= BUDGET);
        // Only aggregates: every query is a count.
        assert!(p.queries.iter().all(|q| q.sql.starts_with("select count(")));
    }

    #[test]
    fn small_cells_are_hidden_and_differences_never_give_them_away() {
        let p = plan(&graph(), "iceberg", "", &empty_plan());
        let mut counts: BTreeMap<u32, BTreeMap<&'static str, i64>> = BTreeMap::new();
        let mut put = |s: u32, m: &'static str, n: i64| {
            counts.entry(s).or_default().insert(m, n);
        };
        put(0, "out", 100);
        put(1, "out", 3);
        put(2, "out", 100);
        put(2, "matched", 98);
        put(2, "unmatched_right", 0);
        put(3, "out", 97);
        put(4, "out", 40);
        put(5, "out", 12);
        let r = present(&p, &counts, 8, 5);
        assert_eq!(r[&1]["out"], "<5", "a small cell");
        assert_eq!(r[&2]["in"], json!([100, "<5"]));
        assert_eq!(r[&2]["unmatched_left"], "<5", "100 − 98");
        assert_eq!(r[&2]["unmatched_right"], 0);
        assert_eq!(r[&2]["fan_out"], false);
        assert_eq!(r[&3]["out"], "100 − <5", "97 of 100 would give the 3 away");
        assert_eq!(r[&3]["removed"], "<5");
        assert_eq!(
            r[&4]["in"],
            json!(["100 − <5"]),
            "the next step reads the display"
        );
        assert_eq!(r[&4]["out"], 40);
        assert_eq!(r[&4]["removed"], Value::Null, "57 would give the 3 away");
        assert_eq!(r[&7]["out"], 12, "the mart has the aggregate's rows");
        assert!(
            r[&5].get("removed").is_none(),
            "an aggregate groups rows, it removes none"
        );
        // Under a small cell, everything is one.
        counts.get_mut(&3).unwrap().insert("out", 4);
        counts.get_mut(&4).unwrap().insert("out", 2);
        let r = present(&p, &counts, 8, 5);
        assert_eq!(r[&3]["removed"], 96);
        assert_eq!(r[&4]["in"], json!(["<5"]));
        assert_eq!(r[&4]["out"], "<5");
        assert_eq!(r[&4]["removed"], Value::Null);
    }

    #[test]
    fn a_step_reading_a_personal_column_is_withheld_with_everything_after_it() {
        let sql = crate::mart_graph::tests::SALES.replace(
            "where \"C_ORDER\".\"ISSOTRX\" = true",
            "where \"C_BPARTNER\".\"EMAIL\" = 'x@y'\n    and \"C_ORDER\".\"ISSOTRX\" = true",
        );
        let m = MartSql {
            mart: "m",
            path: "m.sql",
            dbt_sql: &sql,
            version: "committed",
            grain: &[],
            resolved: &[],
            clauses: &[],
            notes_by_table: Default::default(),
            notes_by_term: Default::default(),
        };
        let g = mart_ir(&m).unwrap().0;
        let p = plan(&g, "iceberg", "", &empty_plan());
        assert!(p.withheld[&3].contains("EMAIL"), "{:?}", p.withheld);
        for s in 3..g.nodes.len() as u32 {
            assert!(p.withheld.contains_key(&s), "step {s}");
        }
        assert!(!p.withheld.contains_key(&2));
        let r = present(&p, &BTreeMap::new(), g.nodes.len() as u32, 5);
        assert!(r[&5]["withheld"].as_str().unwrap().contains("personal"));
    }
}
