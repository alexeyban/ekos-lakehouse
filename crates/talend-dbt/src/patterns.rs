//! EKOS RFC 0268 phase 2: the Talend patterns that keep state in a target, recognised by
//! structure in one job's IR. Each recognised pattern replaces the sinks it covers by one
//! virtual sink over a virtual projection, appended to the IR; the caller writes that sink's
//! model by the pattern's rule. A shape that looks like a pattern but fails a condition is left
//! as it is, and the failed condition is returned as a question.
//!
//! - **SCD2 by hand:** a lookup on the target's own rows and three sinks (insert the unmatched,
//!   update the changed to close them, insert their new version) → a dbt snapshot.
//! - **Keyed replace:** delete the target rows a left join matched, insert every staged row →
//!   one merge on the deleted key.
//! - **Write-back:** an update of a flag on the job's own source → no model.

use std::collections::BTreeSet;

use ekos_lakehouse_etl_expr::{BinOp, Expr, Fidelity, Func, Lit};
use ekos_lakehouse_etl_rel::{
    Field, Ir, JoinKind, NodeId, Op, RelNode, Sink, SinkMode, SourceQuery, TableRef,
};

/// A snapshot's configuration and the columns its gold model gives back.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Target columns of the lookup key (the business key).
    pub unique_key: Vec<String>,
    /// Target columns the change comparison reads.
    pub check_cols: Vec<String>,
    /// The flag column, its value on a current row and on a closed one.
    pub flag: (String, Lit, Lit),
    /// Columns the inserts set to the clock: the version's start.
    pub valid_from: Vec<String>,
    /// Columns the closing update sets to the clock: the version's end.
    pub valid_to: Vec<String>,
    /// The key the closing update matches on, when no insert writes it (a `SERIAL`).
    pub surrogate: Option<String>,
}

/// What a recognised group becomes.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Snapshot(Snapshot),
    /// Merge on the deleted key.
    KeyedReplace {
        key: String,
    },
    /// No model.
    WriteBack,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub kind: Kind,
    pub table: TableRef,
    /// The job's sinks the group replaces.
    pub members: Vec<NodeId>,
    /// The virtual sink appended to the IR (none for a write-back).
    pub sink: Option<NodeId>,
    pub notes: Vec<String>,
    /// `(component, kind, question)`.
    pub questions: Vec<(String, String, String)>,
}

#[derive(Debug, Default)]
pub struct Recognised {
    pub groups: Vec<Group>,
    /// Shapes refused, as `(component, kind, question)`.
    pub refused: Vec<(String, String, String)>,
}

fn same_table(a: &TableRef, b: &TableRef) -> bool {
    let k = |t: &TableRef| {
        (
            t.schema.clone().unwrap_or_default().to_lowercase(),
            t.table.to_lowercase(),
        )
    };
    let db = |t: &TableRef| {
        t.connection
            .as_ref()
            .and_then(|c| c.database.clone())
            .unwrap_or_default()
            .to_lowercase()
    };
    k(a) == k(b) && (db(a).is_empty() || db(b).is_empty() || db(a) == db(b))
}

/// Every node `id` reads, itself included.
pub fn upstream(ir: &Ir, id: NodeId) -> BTreeSet<NodeId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![id];
    while let Some(i) = stack.pop() {
        if seen.insert(i) {
            stack.extend(ir.node(i).inputs.iter().copied());
        }
    }
    seen
}

/// Whether node `id` reads table `t` through a source.
pub fn reads(ir: &Ir, id: NodeId, t: &TableRef) -> bool {
    upstream(ir, id).into_iter().any(|i| {
        matches!(&ir.node(i).op, Op::Source { query: SourceQuery::Sql { tables, .. } }
            if tables.iter().any(|x| same_table(x, t)))
    })
}

/// Whether the main flow into `id` (first inputs, back to its source) reads table `t`: the job
/// reads the table as its data, not through a lookup.
pub fn main_reads(ir: &Ir, id: NodeId, t: &TableRef) -> bool {
    let mut cur = id;
    loop {
        let n = ir.node(cur);
        if let Op::Source {
            query: SourceQuery::Sql { tables, .. },
        } = &n.op
        {
            return tables.iter().any(|x| same_table(x, t));
        }
        match n.inputs.first() {
            Some(i) => cur = *i,
            None => return false,
        }
    }
}

fn sink_of(n: &RelNode) -> Option<&Sink> {
    match &n.op {
        Op::Sink { sink } => Some(sink),
        _ => None,
    }
}

fn project_items(n: &RelNode) -> Option<&Vec<(String, Expr)>> {
    match &n.op {
        Op::Project { items } => Some(items),
        _ => None,
    }
}

fn is_now(e: &Expr) -> bool {
    matches!(e, Expr::Call { func: Func::Now | Func::Today, args } if args.is_empty())
}

fn lit(e: &Expr) -> Option<&Lit> {
    match e {
        Expr::Lit { value } => Some(value),
        _ => None,
    }
}

/// A sink's branch: its projection, the filters under it, and the lookup or join they read.
struct Branch {
    project: NodeId,
    filters: Vec<NodeId>,
    base: NodeId,
}

fn branch(ir: &Ir, sink: &RelNode) -> Option<Branch> {
    let project = *sink.inputs.first()?;
    project_items(ir.node(project))?;
    let mut cur = *ir.node(project).inputs.first()?;
    let mut filters = Vec::new();
    while let Op::Filter { .. } = ir.node(cur).op {
        filters.push(cur);
        cur = *ir.node(cur).inputs.first()?;
    }
    matches!(ir.node(cur).op, Op::Lookup { .. } | Op::Join { .. }).then_some(Branch {
        project,
        filters,
        base: cur,
    })
}

fn predicate(ir: &Ir, id: NodeId) -> Option<&Expr> {
    match &ir.node(id).op {
        Op::Filter { predicate } => Some(predicate),
        _ => None,
    }
}

/// `x IS NULL` on one column.
fn is_null_of(e: &Expr) -> Option<(&str, bool)> {
    match e {
        Expr::IsNull { arg, negated } => match arg.as_ref() {
            Expr::Col { name } => Some((name, *negated)),
            _ => None,
        },
        _ => None,
    }
}

/// The main-side columns a comparison of main and lookup columns reads (`NOT (a IS NOT DISTINCT
/// FROM b)` and the like, any nesting).
fn compared(e: &Expr, main: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    e.walk(&mut |x| {
        if let Expr::Binary { op, left, right } = x
            && matches!(
                op,
                BinOp::NotDistinct | BinOp::Eq | BinOp::NotEq | BinOp::Distinct
            )
        {
            for side in [left, right] {
                if let Expr::Col { name } = side.as_ref()
                    && main.contains(name)
                {
                    out.insert(name.clone());
                }
            }
        }
    });
    out
}

fn names(n: &RelNode) -> BTreeSet<String> {
    n.schema.iter().map(|f| f.name.clone()).collect()
}

fn st(ir: &Ir, i: NodeId) -> String {
    ir.node(i).step.clone()
}

/// Appends a node; returns its id.
fn push(
    ir: &mut Ir,
    like: &RelNode,
    op: Op,
    inputs: Vec<NodeId>,
    schema: Vec<Field>,
    branch: &str,
) -> NodeId {
    let id = ir.nodes.len();
    ir.nodes.push(RelNode {
        id,
        step: like.step.clone(),
        step_type: like.step_type.clone(),
        branch: Some(branch.to_string()),
        op,
        inputs,
        schema,
        fidelity: Fidelity::Rewritten,
        notes: Vec::new(),
        line: like.line,
    });
    id
}

/// The target column an insert's projection writes from main column `col`.
fn target_of(items: &[(String, Expr)], sink: &Sink, col: &str) -> Option<String> {
    let stream = items
        .iter()
        .find(|(_, e)| matches!(e, Expr::Col { name } if name == col))
        .map(|(n, _)| n.clone())?;
    sink.columns
        .iter()
        .find(|(s, _)| *s == stream)
        .map(|(_, c)| c.clone())
}

/// Recognises the job's patterns, appending virtual nodes to `ir`.
pub fn recognise(ir: &mut Ir) -> Recognised {
    let mut out = Recognised::default();
    let mut by_table: Vec<(TableRef, Vec<NodeId>)> = Vec::new();
    for n in &ir.nodes {
        let Some(s) = sink_of(n) else { continue };
        if s.target.schema.as_deref() == Some("files") {
            continue;
        }
        match by_table.iter_mut().find(|(t, _)| same_table(t, &s.target)) {
            Some((_, v)) => v.push(n.id),
            None => by_table.push((s.target.clone(), vec![n.id])),
        }
    }
    let mut taken: BTreeSet<NodeId> = BTreeSet::new();
    for (table, sinks) in &by_table {
        // Write-back first: it is not a writer of the table's data.
        for &s in sinks {
            if let Some(g) = write_back(ir, s, table) {
                taken.insert(s);
                out.groups.push(g);
            }
        }
        let rest: Vec<NodeId> = sinks
            .iter()
            .copied()
            .filter(|s| !taken.contains(s))
            .collect();
        if rest.len() < 2 || !rest.iter().any(|s| reads(ir, *s, table)) {
            continue;
        }
        let result = if rest.len() == 3 {
            snapshot(ir, &rest, table)
        } else if rest.len() == 2 {
            keyed_replace(ir, &rest, table)
        } else {
            Err((
                ir.node(rest[0]).step.clone(),
                format!(
                    "{} components write `{}` and read it",
                    rest.len(),
                    table.table
                ),
            ))
        };
        match result {
            Ok(g) => {
                taken.extend(g.members.iter().copied());
                out.groups.push(g);
            }
            Err((component, why)) => out.refused.push((
                component,
                "pattern".into(),
                format!(
                    "`{}` is written by {} components of the job, which also read it, but they are not a recognised pattern: {why}. Which load is it?",
                    table.table,
                    rest.len()
                ),
            )),
        }
    }
    out
}

/// An `UPDATE` of a table the job reads, setting literals only.
fn write_back(ir: &Ir, s: NodeId, table: &TableRef) -> Option<Group> {
    let n = ir.node(s);
    let sink = sink_of(n)?;
    if sink.mode != SinkMode::Update || sink.keys.is_empty() || !main_reads(ir, s, table) {
        return None;
    }
    let items = project_items(ir.node(*n.inputs.first()?))?;
    let keys: BTreeSet<&str> = sink.keys.iter().map(|(st, _)| st.as_str()).collect();
    let set: Vec<(&String, &Expr)> = sink
        .columns
        .iter()
        .filter(|(st, _)| !keys.contains(st.as_str()))
        .filter_map(|(st, col)| items.iter().find(|(x, _)| x == st).map(|(_, e)| (col, e)))
        .collect();
    if set.is_empty() || !set.iter().all(|(_, e)| lit(e).is_some()) {
        return None;
    }
    let cols: Vec<String> = set.iter().map(|(c, _)| format!("`{c}`")).collect();
    Some(Group {
        kind: Kind::WriteBack,
        table: table.clone(),
        members: vec![s],
        sink: None,
        notes: vec![format!(
            "{} marks the rows it read as done ({} set to a constant): no model",
            n.step,
            cols.join(", ")
        )],
        questions: vec![(
            n.step.clone(),
            "write-back".into(),
            format!(
                "{} sets {} on `{}` for the rows the job has read, so that the next run skips them. In dbt the reading model's incremental filter does that; the flag is not written. Does anything else read it?",
                n.step,
                cols.join(", "),
                table.table
            ),
        )],
    })
}

type Refusal = (String, String);

fn snapshot(ir: &mut Ir, sinks: &[NodeId], table: &TableRef) -> Result<Group, Refusal> {
    let refuse = |c: String, why: String| -> Result<Group, Refusal> { Err((c, why)) };
    let mut new = None;
    let mut update = None;
    let mut version = None;
    for &s in sinks {
        let n = ir.node(s);
        let sink = sink_of(n).expect("a sink");
        let Some(b) = branch(ir, n) else {
            return refuse(
                st(ir, s),
                format!("{} is not fed by a lookup's output", n.step),
            );
        };
        let Op::Lookup { keys, inner, .. } = &ir.node(b.base).op else {
            return refuse(st(ir, s), format!("{} does not read a lookup", n.step));
        };
        let lk_input = ir.node(b.base).inputs.get(1).copied();
        if !lk_input.is_some_and(|i| reads(ir, i, table)) {
            return refuse(
                st(ir, s),
                format!("{}'s lookup does not read `{}`", n.step, table.table),
            );
        }
        let unmatched = !*inner
            && b.filters.len() == 1
            && predicate(ir, b.filters[0])
                .and_then(is_null_of)
                .is_some_and(|(c, neg)| !neg && keys.iter().any(|(_, lk)| lk == c));
        match (sink.mode, unmatched) {
            (SinkMode::Insert, true) => new = Some((s, b)),
            (SinkMode::Update, false) => update = Some((s, b)),
            (SinkMode::Insert, false) => {
                // A closing "insert": every column NULL but a constant (fixture B's jOdsClient).
                let items = project_items(ir.node(b.project)).expect("a projection");
                let data: Vec<&Expr> = items
                    .iter()
                    .filter(|(n, _)| n != "$path")
                    .map(|(_, e)| e)
                    .collect();
                let nulls = data.iter().filter(|e| lit(e) == Some(&Lit::Null)).count();
                if nulls + 1 >= data.len() && data.len() > 2 {
                    return refuse(
                        st(ir, s),
                        format!(
                            "{} inserts a row whose columns are all NULL but one, where an SCD2 updates the current version to close it",
                            n.step
                        ),
                    );
                }
                version = Some((s, b));
            }
            _ => {
                return refuse(st(ir, s), format!("{} is a {:?}", n.step, sink.mode));
            }
        }
    }
    let (Some((ns, nb)), Some((us, ub)), Some((vs, vb))) = (new, update, version) else {
        return refuse(
            st(ir, sinks[0]),
            "not one insert of unmatched rows, one update and one insert of changed rows".into(),
        );
    };
    // One lookup (its inner and left forms share their inputs).
    let inputs_of = |b: &Branch| ir.node(b.base).inputs.clone();
    if inputs_of(&nb) != inputs_of(&ub) || inputs_of(&nb) != inputs_of(&vb) {
        return refuse(
            st(ir, us),
            "the three outputs read different lookups".into(),
        );
    }
    let main = inputs_of(&nb)[0];
    let main_cols = names(ir.node(main));
    let new_items = project_items(ir.node(nb.project))
        .expect("projection")
        .clone();
    let ver_items = project_items(ir.node(vb.project))
        .expect("projection")
        .clone();
    let upd_items = project_items(ir.node(ub.project))
        .expect("projection")
        .clone();
    if new_items != ver_items {
        return refuse(
            st(ir, vs),
            format!("{} and {} write different rows", st(ir, ns), st(ir, vs)),
        );
    }
    let reads_main_only = new_items
        .iter()
        .all(|(_, e)| e.columns().iter().all(|c| main_cols.contains(c)));
    if !reads_main_only {
        return refuse(
            st(ir, ns),
            format!("{} reads the lookup's columns", st(ir, ns)),
        );
    }
    let new_sink = sink_of(ir.node(ns)).expect("sink").clone();
    let upd_sink = sink_of(ir.node(us)).expect("sink").clone();
    let col_of = |items: &[(String, Expr)], sink: &Sink, stream: &str| {
        let _ = items;
        sink.columns
            .iter()
            .find(|(s, _)| s == stream)
            .map(|(_, c)| c.clone())
    };
    // The flag: a constant in the inserts, another in the update.
    let flag = new_items.iter().find_map(|(st, e)| {
        let a = lit(e)?;
        let col = col_of(&new_items, &new_sink, st)?;
        let ust = upd_sink
            .columns
            .iter()
            .find(|(_, c)| c.eq_ignore_ascii_case(&col))?
            .0
            .clone();
        let b = upd_items
            .iter()
            .find(|(x, _)| *x == ust)
            .and_then(|(_, e)| lit(e))?;
        (a != b && *a != Lit::Null && *b != Lit::Null).then(|| (col, a.clone(), b.clone()))
    });
    let Some(flag) = flag else {
        return refuse(
            st(ir, us),
            format!(
                "{} sets no column to a constant the inserts set differently",
                st(ir, us)
            ),
        );
    };
    // The key the update matches on: the business key, or a key read from the lookup.
    let Op::Lookup { keys: lkeys, .. } = &ir.node(nb.base).op else {
        unreachable!()
    };
    let unique_key: Vec<String> = lkeys
        .iter()
        .filter_map(|(m, _)| target_of(&new_items, &new_sink, m))
        .collect();
    if unique_key.len() != lkeys.len() {
        return refuse(
            st(ir, ns),
            format!("{} does not write the lookup key", st(ir, ns)),
        );
    }
    let written: BTreeSet<String> = new_sink
        .columns
        .iter()
        .map(|(_, c)| c.to_lowercase())
        .collect();
    let surrogate = match upd_sink.keys.as_slice() {
        [(_, k)] if !written.contains(&k.to_lowercase()) => Some(k.clone()),
        ks if ks
            .iter()
            .map(|(_, k)| k.to_lowercase())
            .collect::<BTreeSet<_>>()
            == unique_key.iter().map(|k| k.to_lowercase()).collect() =>
        {
            None
        }
        _ => {
            return refuse(
                st(ir, us),
                format!(
                    "{} matches on neither the lookup key nor a key the inserts leave out",
                    st(ir, us)
                ),
            );
        }
    };
    // The change comparison: the same filter on the update and on the new version.
    let (up, vp) = (
        ub.filters.first().and_then(|f| predicate(ir, *f)),
        vb.filters.first().and_then(|f| predicate(ir, *f)),
    );
    let (Some(up), Some(vp)) = (up, vp) else {
        let why = match (up, vp) {
            (Some(_), None) => format!(
                "{} inserts a new version of every matched row, changed or not, while {} closes only the changed ones: an unchanged key gains a second current row on each run",
                st(ir, vs),
                st(ir, us)
            ),
            _ => format!("{} closes every matched row, changed or not", st(ir, us)),
        };
        return refuse(st(ir, vs), why);
    };
    if up != vp {
        return refuse(
            st(ir, vs),
            format!("{} and {} compare differently", st(ir, us), st(ir, vs)),
        );
    }
    let check_cols: Vec<String> = compared(up, &main_cols)
        .iter()
        .filter_map(|m| target_of(&new_items, &new_sink, m))
        .collect();
    if check_cols.is_empty() {
        return refuse(
            st(ir, us),
            "the comparison reads no column the inserts write".into(),
        );
    }
    let valid_from: Vec<String> = new_items
        .iter()
        .filter(|(_, e)| is_now(e))
        .filter_map(|(st, _)| col_of(&new_items, &new_sink, st))
        .collect();
    let valid_to: Vec<String> = upd_items
        .iter()
        .filter(|(_, e)| is_now(e))
        .filter_map(|(st, _)| col_of(&upd_items, &upd_sink, st))
        .filter(|c| !valid_from.iter().any(|v| v.eq_ignore_ascii_case(c)))
        .collect();
    // What the closing update also changes: columns the inserts fill from the data.
    let keys_l: BTreeSet<String> = upd_sink
        .keys
        .iter()
        .map(|(_, k)| k.to_lowercase())
        .collect();
    let overwritten: Vec<String> = upd_items
        .iter()
        .filter(|(_, e)| lit(e) == Some(&Lit::Null))
        .filter_map(|(st, _)| col_of(&upd_items, &upd_sink, st))
        .filter(|c| !keys_l.contains(&c.to_lowercase()) && written.contains(&c.to_lowercase()))
        .collect();
    // The snapshot's rows: the first insert's projection over the main input, without the
    // flag and the clock (dbt keeps both).
    let dropped: BTreeSet<String> = std::iter::once(flag.0.to_lowercase())
        .chain(valid_from.iter().map(|c| c.to_lowercase()))
        .collect();
    let keep: Vec<(String, Expr)> = new_items
        .iter()
        .filter(|(st, _)| {
            col_of(&new_items, &new_sink, st).is_some_and(|c| !dropped.contains(&c.to_lowercase()))
        })
        .cloned()
        .collect();
    let schema: Vec<Field> = keep
        .iter()
        .filter_map(|(st, _)| ir.node(nb.project).field(st).cloned())
        .collect();
    let like = ir.node(nb.project).clone();
    let body = push(
        ir,
        &like,
        Op::Project {
            items: keep.clone(),
        },
        vec![main],
        schema.clone(),
        "snapshot",
    );
    let columns: Vec<(String, String)> = keep
        .iter()
        .filter_map(|(st, _)| col_of(&new_items, &new_sink, st).map(|c| (st.clone(), c)))
        .collect();
    let keys: Vec<(String, String)> = columns
        .iter()
        .filter(|(_, c)| unique_key.iter().any(|k| k.eq_ignore_ascii_case(c)))
        .cloned()
        .collect();
    let sink_like = ir.node(ns).clone();
    let vsink = push(
        ir,
        &sink_like,
        Op::Sink {
            sink: Sink {
                mode: SinkMode::Insert,
                target: table.clone(),
                columns,
                keys,
                truncate: false,
                scd: None,
            },
        },
        vec![body],
        schema,
        "snapshot",
    );
    let (sn, su, sv) = (st(ir, ns), st(ir, us), st(ir, vs));
    let mut notes = vec![format!(
        "SCD2 by hand: {sn} inserts unseen keys, {su} closes a changed row ({} = {}), {sv} inserts its new version: a dbt snapshot (check {}), the flag from `dbt_valid_to`",
        flag.0,
        lit_text(&flag.2),
        check_cols.join(", ")
    )];
    let mut questions = Vec::new();
    if !overwritten.is_empty() {
        let scope = if surrogate.is_some() {
            "the closed version"
        } else {
            "every version under the key"
        };
        notes.push(format!(
            "{su} also writes NULL into {} of {scope}; the snapshot keeps the closed version's values",
            overwritten.join(", ")
        ));
        questions.push((
            su.clone(),
            "snapshot".into(),
            format!(
                "{su} closes a version by updating `{}` on {}, and sets {} to NULL there too. The snapshot keeps the old values in its history: is that what the table is for?",
                table.table,
                upd_sink.keys.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>().join(", "),
                overwritten.join(", ")
            ),
        ));
    }
    if let Some(k) = &surrogate {
        questions.push((
            su.clone(),
            "surrogate-key".into(),
            format!(
                "`{}`.{k} is a key the database assigns (no insert writes it). The model gives `dbt_scd_id` instead, a hash of the business key and the version's start: the key values change. Do readers keep `{k}` anywhere?",
                table.table
            ),
        ));
    }
    Ok(Group {
        kind: Kind::Snapshot(Snapshot {
            unique_key,
            check_cols,
            flag,
            valid_from,
            valid_to,
            surrogate,
        }),
        table: table.clone(),
        members: vec![ns, us, vs],
        sink: Some(vsink),
        notes,
        questions,
    })
}

pub fn lit_text(l: &Lit) -> String {
    match l {
        Lit::Null => "NULL".into(),
        Lit::Str(s) => ekos_lakehouse_etl_expr::quote_str(s),
        Lit::Int(i) => i.to_string(),
        Lit::Num(n) => n.clone(),
        Lit::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
    }
}

/// The columns a node reads; `None` for a kind this rule does not look into (it refuses then).
fn reads_columns(n: &RelNode) -> Option<Vec<String>> {
    Some(match &n.op {
        Op::Project { items } => items.iter().flat_map(|(_, e)| e.columns()).collect(),
        Op::Filter { predicate } => predicate.columns(),
        Op::Join {
            keys, condition, ..
        } => keys
            .iter()
            .flat_map(|(a, b)| [a.clone(), b.clone()])
            .chain(condition.iter().flat_map(Expr::columns))
            .collect(),
        Op::Lookup { keys, .. } => keys
            .iter()
            .flat_map(|(a, b)| [a.clone(), b.clone()])
            .collect(),
        Op::Aggregate { group_by, aggs } => group_by
            .iter()
            .cloned()
            .chain(aggs.iter().filter_map(|a| a.subject.clone()))
            .collect(),
        Op::Window {
            partition, order, ..
        } => partition
            .iter()
            .cloned()
            .chain(order.iter().map(|k| k.field.clone()))
            .collect(),
        Op::Distinct { keys, .. } => keys.clone(),
        Op::Sort { keys } => keys.iter().map(|k| k.field.clone()).collect(),
        Op::Union => Vec::new(),
        _ => return None,
    })
}

/// Delete the target rows a left join on the target matched, insert every staged row: one merge
/// on the deleted key. The join may sit anywhere in a chain (fixture E joins dates and other
/// tables after it): the insert's path from the join is rebuilt on the join's staged side, which
/// holds when nothing on that path reads the target's columns.
fn keyed_replace(ir: &mut Ir, sinks: &[NodeId], table: &TableRef) -> Result<Group, Refusal> {
    let mode = |i: NodeId| sink_of(ir.node(i)).map(|s| s.mode);
    let (d, i) = match (mode(sinks[0]), mode(sinks[1])) {
        (Some(SinkMode::Delete), Some(SinkMode::Insert)) => (sinks[0], sinks[1]),
        (Some(SinkMode::Insert), Some(SinkMode::Delete)) => (sinks[1], sinks[0]),
        _ => {
            return Err((st(ir, sinks[0]), "not one delete and one insert".into()));
        }
    };
    let (Some(&dproj), Some(&iproj)) = (ir.node(d).inputs.first(), ir.node(i).inputs.first())
    else {
        return Err((st(ir, d), "an output with no input".into()));
    };
    let (Some(del_items), Some(_)) = (project_items(ir.node(dproj)), project_items(ir.node(iproj)))
    else {
        return Err((
            st(ir, d),
            "the delete or the insert is not fed by a projection".into(),
        ));
    };
    let bcol = match del_items.as_slice() {
        [(_, Expr::Col { name })] => name,
        [_] => {
            return Err((
                st(ir, d),
                format!(
                    "{} deletes a key it computes, not one read from the target",
                    st(ir, d)
                ),
            ));
        }
        _ => {
            return Err((
                st(ir, d),
                format!("{} deletes more than one key column", st(ir, d)),
            ));
        }
    };
    let bcol = bcol.clone();
    // The left join on the target both outputs depend on.
    let (up_d, up_i) = (upstream(ir, dproj), upstream(ir, iproj));
    let join = up_d.intersection(&up_i).copied().find(|j| {
        let n = ir.node(*j);
        matches!(
            n.op,
            Op::Join {
                kind: JoinKind::Left,
                ..
            }
        ) && n.inputs.len() == 2
            && reads(ir, n.inputs[1], table)
            && !reads(ir, n.inputs[0], table)
            && names(ir.node(n.inputs[1])).contains(&bcol)
    });
    let Some(join) = join else {
        return Err((
            st(ir, d),
            format!(
                "no left join of the staged rows with `{}` feeds both outputs, keyed by what the delete removes",
                table.table
            ),
        ));
    };
    let (a, b) = (ir.node(join).inputs[0], ir.node(join).inputs[1]);
    let Op::Join { keys: jkeys, .. } = &ir.node(join).op else {
        unreachable!()
    };
    let jkeys = jkeys.clone();
    let after = |id: NodeId| upstream(ir, id).contains(&join) && id != join;
    // The delete removes the matched rows: `bcol IS NOT NULL` after the join, or no filter at all
    // (an unmatched row's `bcol` is NULL, which equals no key).
    let filters: Vec<NodeId> = up_d
        .iter()
        .copied()
        .filter(|x| after(*x) && matches!(ir.node(*x).op, Op::Filter { .. }))
        .collect();
    let matched = filters.is_empty()
        || filters.iter().any(|x| {
            predicate(ir, *x)
                .and_then(is_null_of)
                .is_some_and(|(c, neg)| neg && c == bcol)
        });
    if !matched {
        return Err((
            st(ir, d),
            format!("{} does not delete the matched rows", st(ir, d)),
        ));
    }
    let dsink = sink_of(ir.node(d)).expect("sink");
    let [(_, key)] = dsink.columns.as_slice() else {
        return Err((
            st(ir, d),
            format!("{} matches on more than one column", st(ir, d)),
        ));
    };
    let key = key.clone();
    let isink = sink_of(ir.node(i)).expect("sink").clone();
    let Some((kstream, _)) = isink
        .columns
        .iter()
        .find(|(_, c)| c.eq_ignore_ascii_case(&key))
    else {
        return Err((st(ir, i), format!("{} does not write `{key}`", st(ir, i))));
    };
    let kstream = kstream.clone();
    // The insert's path from the join reads none of the target's columns.
    let b_cols = names(ir.node(b));
    let mut path: Vec<NodeId> = up_i.iter().copied().filter(|x| after(*x)).collect();
    path.sort_unstable();
    // A projection item that only passes a target column on is not a read: it is dropped below.
    let passes =
        |k: &str, e: &Expr| matches!(e, Expr::Col { name } if name == k) && b_cols.contains(k);
    let without_passes = |op: &Op| match op {
        Op::Project { items } => Op::Project {
            items: items
                .iter()
                .filter(|(k, e)| !passes(k, e))
                .cloned()
                .collect(),
        },
        other => other.clone(),
    };
    for x in &path {
        let n = &RelNode {
            op: without_passes(&ir.node(*x).op),
            ..ir.node(*x).clone()
        };
        let Some(cols) = reads_columns(n) else {
            return Err((
                st(ir, i),
                format!(
                    "{}'s path holds a {} this rule does not read",
                    st(ir, i),
                    n.step_type
                ),
            ));
        };
        if let Some(c) = cols.iter().find(|c| b_cols.contains(*c)) {
            return Err((
                st(ir, i),
                format!("{} reads `{}`'s column `{c}`", st(ir, i), table.table),
            ));
        }
    }
    let ins_items = project_items(ir.node(iproj)).expect("projection").clone();
    let kexpr = ins_items
        .iter()
        .find(|(n, _)| *n == kstream)
        .map(|(_, e)| format!("`{}`", e.sql()))
        .unwrap_or_default();
    // Rebuild the path on the staged side.
    let mut map: std::collections::BTreeMap<NodeId, NodeId> = std::collections::BTreeMap::new();
    map.insert(join, a);
    for x in &path {
        let like = ir.node(*x).clone();
        let inputs = like
            .inputs
            .iter()
            .map(|q| *map.get(q).unwrap_or(q))
            .collect();
        let schema = like
            .schema
            .iter()
            .filter(|f| !b_cols.contains(&f.name))
            .cloned()
            .collect();
        let id = push(
            ir,
            &like,
            without_passes(&like.op),
            inputs,
            schema,
            "keyed replace",
        );
        map.insert(*x, id);
    }
    let body = map[&iproj];
    let schema = ir.node(body).schema.clone();
    let sink_like = ir.node(i).clone();
    let vsink = push(
        ir,
        &sink_like,
        Op::Sink {
            sink: Sink {
                mode: SinkMode::Upsert,
                target: table.clone(),
                columns: isink.columns.clone(),
                keys: vec![(kstream.clone(), key.clone())],
                truncate: false,
                scd: None,
            },
        },
        vec![body],
        schema,
        "keyed replace",
    );
    let on: Vec<String> = jkeys.iter().map(|(x, y)| format!("{x} = {y}")).collect();
    let (sd, si) = (st(ir, d), st(ir, i));
    Ok(Group {
        kind: Kind::KeyedReplace { key: key.clone() },
        table: table.clone(),
        members: vec![d, i],
        sink: Some(vsink),
        notes: vec![format!(
            "keyed replace: {sd} deletes the rows of `{}` the join on {} matched, {si} inserts every staged row: one merge on `{key}`",
            table.table,
            on.join(", ")
        )],
        questions: vec![(
            si.clone(),
            "keyed-replace".into(),
            format!(
                "{sd} deletes the rows the staging rows match on {}, and {si} inserts the staging rows with `{key}` computed as {kexpr}. One merge on `{key}` gives the same table when every `{key}` in it was computed that way from its row, and when the staged rows match at most one target row each: was it?",
                on.join(", ")
            ),
        )],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekos_lakehouse_etl_expr::KType;

    /// A small IR, written by hand: nodes in order, each `(step, op, inputs, schema)`.
    struct B(Ir);

    impl B {
        fn new() -> B {
            B(Ir {
                transformation: "t".into(),
                nodes: Vec::new(),
            })
        }
        fn add(&mut self, step: &str, op: Op, inputs: Vec<NodeId>, cols: &[&str]) -> NodeId {
            let id = self.0.nodes.len();
            self.0.nodes.push(RelNode {
                id,
                step: step.into(),
                step_type: "t".into(),
                branch: None,
                op,
                inputs,
                schema: cols.iter().map(|c| Field::new(c, KType::String)).collect(),
                fidelity: Fidelity::Exact,
                notes: Vec::new(),
                line: 0,
            });
            id
        }
        fn source(&mut self, step: &str, table: &str, cols: &[&str]) -> NodeId {
            let op = Op::Source {
                query: SourceQuery::Sql {
                    text: format!("SELECT * FROM {table}"),
                    tables: vec![t(table)],
                    limit: None,
                },
            };
            self.add(step, op, vec![], cols)
        }
        fn project(&mut self, step: &str, input: NodeId, items: &[(&str, Expr)]) -> NodeId {
            let cols: Vec<&str> = items.iter().map(|(n, _)| *n).collect();
            let items = items
                .iter()
                .map(|(n, e)| (n.to_string(), e.clone()))
                .collect();
            self.add(step, Op::Project { items }, vec![input], &cols)
        }
        fn filter(&mut self, input: NodeId, p: Expr) -> NodeId {
            let cols: Vec<String> = self
                .0
                .node(input)
                .schema
                .iter()
                .map(|f| f.name.clone())
                .collect();
            let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
            self.add("tMap_1", Op::Filter { predicate: p }, vec![input], &cols)
        }
        fn sink(
            &mut self,
            step: &str,
            input: NodeId,
            table: &str,
            mode: SinkMode,
            cols: &[&str],
            keys: &[&str],
        ) -> NodeId {
            let pair = |c: &&str| (c.to_string(), c.to_string());
            let op = Op::Sink {
                sink: Sink {
                    mode,
                    target: t(table),
                    columns: cols.iter().map(pair).collect(),
                    keys: keys.iter().map(pair).collect(),
                    truncate: false,
                    scd: None,
                },
            };
            self.add(step, op, vec![input], cols)
        }
    }

    fn t(table: &str) -> TableRef {
        TableRef {
            connection: None,
            schema: Some("ods".into()),
            table: table.into(),
        }
    }

    fn c(n: &str) -> Expr {
        Expr::col(n)
    }

    fn changed(a: &str, b: &str) -> Expr {
        Expr::Unary {
            op: ekos_lakehouse_etl_expr::UnOp::Not,
            arg: Box::new(Expr::bin(BinOp::NotDistinct, c(a), c(b))),
        }
    }

    /// Fixture B's ODS shape: staged rows looked up on the target's current rows; insert the
    /// unseen, close the changed (`close` builds that output), insert their new version.
    fn scd2(close: &dyn Fn(&mut B, NodeId) -> NodeId, version_compares: bool) -> Ir {
        let mut b = B::new();
        let staged = b.source("tFileInputDelimited_1", "staged", &["CD", "LB"]);
        let main = b.project("tMap_1", staged, &[("s__CD", c("CD")), ("s__LB", c("LB"))]);
        let cur = b.source("tDBInput_1", "target", &["CD", "LB", "ACTIVE"]);
        let lk = b.project("tMap_1", cur, &[("t__CD", c("CD")), ("t__LB", c("LB"))]);
        let cols = ["s__CD", "s__LB", "t__CD", "t__LB"];
        let lookup = |inner| Op::Lookup {
            table: None,
            keys: vec![("s__CD".into(), "t__CD".into())],
            values: vec![],
            valid_at: None,
            inner,
        };
        let matched = b.add("tMap_1", lookup(true), vec![main, lk], &cols);
        let left = b.add("tMap_1", lookup(false), vec![main, lk], &cols);
        let unseen = b.filter(
            left,
            Expr::IsNull {
                arg: Box::new(c("t__CD")),
                negated: false,
            },
        );
        let row = |b: &mut B, input| {
            b.project(
                "tMap_1",
                input,
                &[
                    ("CD", c("s__CD")),
                    ("LB", c("s__LB")),
                    ("DT", Expr::call(Func::Now, vec![])),
                    ("ACTIVE", Expr::int(1)),
                ],
            )
        };
        let new = row(&mut b, unseen);
        b.sink(
            "tDBOutput_3",
            new,
            "target",
            SinkMode::Insert,
            &["CD", "LB", "DT", "ACTIVE"],
            &["CD"],
        );
        let ch = b.filter(matched, changed("s__LB", "t__LB"));
        let closing = close(&mut b, ch);
        let _ = closing;
        let vin = if version_compares {
            b.filter(matched, changed("s__LB", "t__LB"))
        } else {
            matched
        };
        let ver = row(&mut b, vin);
        b.sink(
            "tDBOutput_2",
            ver,
            "target",
            SinkMode::Insert,
            &["CD", "LB", "DT", "ACTIVE"],
            &["CD"],
        );
        b.0
    }

    fn update_close(b: &mut B, ch: NodeId) -> NodeId {
        let p = b.project(
            "tMap_1",
            ch,
            &[
                ("CD", c("s__CD")),
                ("LB", Expr::null()),
                ("DT", Expr::null()),
                ("ACTIVE", Expr::int(0)),
            ],
        );
        b.sink(
            "tDBOutput_1",
            p,
            "target",
            SinkMode::Update,
            &["CD", "LB", "DT", "ACTIVE"],
            &["CD"],
        )
    }

    #[test]
    fn an_scd2_by_hand_is_a_snapshot_and_its_null_overwrite_is_asked_about() {
        let mut ir = scd2(&update_close, true);
        let r = recognise(&mut ir);
        assert!(r.refused.is_empty(), "{:?}", r.refused);
        let g = &r.groups[0];
        let Kind::Snapshot(s) = &g.kind else {
            panic!("{:?}", g.kind)
        };
        assert_eq!(s.unique_key, vec!["CD"]);
        assert_eq!(s.check_cols, vec!["LB"]);
        assert_eq!(s.flag, ("ACTIVE".into(), Lit::Int(1), Lit::Int(0)));
        assert_eq!(s.valid_from, vec!["DT"]);
        assert_eq!(s.surrogate, None);
        assert_eq!(g.members.len(), 3);
        // The snapshot's rows: the first insert's projection over the staged rows, without the
        // flag and the clock.
        let v = ir.node(g.sink.unwrap());
        let body = ir.node(v.inputs[0]);
        assert_eq!(body.inputs, vec![1]);
        let Op::Project { items } = &body.op else {
            panic!()
        };
        assert_eq!(
            items.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["CD", "LB"]
        );
        // The update writes NULL into LB on every version of the key: asked.
        assert!(
            g.questions
                .iter()
                .any(|(_, k, q)| k == "snapshot" && q.contains("LB")),
            "{:?}",
            g.questions
        );
    }

    #[test]
    fn a_close_that_inserts_a_null_row_is_refused_by_name() {
        let insert_close = |b: &mut B, ch: NodeId| {
            let p = b.project(
                "tMap_1",
                ch,
                &[
                    ("CD", Expr::null()),
                    ("LB", Expr::null()),
                    ("DT", Expr::null()),
                    ("ACTIVE", Expr::int(0)),
                ],
            );
            b.sink(
                "tDBOutput_1",
                p,
                "target",
                SinkMode::Insert,
                &["CD", "LB", "DT", "ACTIVE"],
                &["CD"],
            )
        };
        let mut ir = scd2(&insert_close, true);
        let r = recognise(&mut ir);
        assert!(r.groups.is_empty());
        assert!(
            r.refused[0].2.contains("all NULL but one"),
            "{:?}",
            r.refused
        );
    }

    #[test]
    fn a_new_version_of_every_matched_row_is_refused_by_name() {
        let mut ir = scd2(&update_close, false);
        let r = recognise(&mut ir);
        assert!(r.groups.is_empty());
        assert!(r.refused[0].2.contains("changed or not"), "{:?}", r.refused);
    }

    #[test]
    fn delete_the_matched_then_insert_every_staged_row_is_a_merge_on_the_key() {
        let mut b = B::new();
        let a = b.source("tELTInput_1", "stg", &["MATNR"]);
        let a = b.project("tELTMap_1", a, &[("A__MATNR", c("MATNR"))]);
        let t_ = b.source("tELTInput_2", "target", &["MATNR", "PK"]);
        let t_ = b.project(
            "tELTMap_1",
            t_,
            &[("B__MATNR", c("MATNR")), ("B__PK", c("PK"))],
        );
        let j = b.add(
            "tELTMap_1",
            Op::Join {
                kind: JoinKind::Left,
                keys: vec![("A__MATNR".into(), "B__MATNR".into())],
                condition: None,
            },
            vec![a, t_],
            &["A__MATNR", "B__MATNR", "B__PK"],
        );
        let m = b.filter(
            j,
            Expr::IsNull {
                arg: Box::new(c("B__PK")),
                negated: true,
            },
        );
        let d = b.project("tELTMap_1", m, &[("PK", c("B__PK"))]);
        b.sink("tELTOutput_2", d, "target", SinkMode::Delete, &["PK"], &[]);
        let i = b.project(
            "tELTMap_1",
            j,
            &[
                ("MATNR", c("A__MATNR")),
                ("PK", Expr::call(Func::Md5Hex, vec![c("A__MATNR")])),
            ],
        );
        b.sink(
            "tELTOutput_1",
            i,
            "target",
            SinkMode::Insert,
            &["MATNR", "PK"],
            &[],
        );
        let mut ir = b.0;
        let r = recognise(&mut ir);
        assert!(r.refused.is_empty(), "{:?}", r.refused);
        let g = &r.groups[0];
        assert_eq!(g.kind, Kind::KeyedReplace { key: "PK".into() });
        let v = ir.node(g.sink.unwrap());
        let Op::Sink { sink } = &v.op else { panic!() };
        assert_eq!(
            (sink.mode, sink.keys.clone()),
            (SinkMode::Upsert, vec![("PK".into(), "PK".into())])
        );
        // The join on the target is gone: the body reads the staged rows only.
        assert_eq!(ir.node(v.inputs[0]).inputs, vec![a]);
        assert!(g.questions[0].2.contains("md5"), "{:?}", g.questions);
    }

    /// Fixture E's chained form: the join on the target, then an inner join on a dates table and a
    /// pass-through of the target's columns; the delete has no filter (a NULL key deletes nothing).
    #[test]
    fn a_keyed_replace_whose_join_sits_in_a_chain_is_rebuilt_on_the_staged_side() {
        let mut b = B::new();
        let a = b.source("tELTInput_1", "stg", &["K", "D"]);
        let a = b.project("tELTMap_1", a, &[("A__K", c("K")), ("A__D", c("D"))]);
        let t_ = b.source("tELTInput_2", "target", &["K", "PK"]);
        let t_ = b.project("tELTMap_1", t_, &[("B__K", c("K")), ("B__PK", c("PK"))]);
        let dates = b.source("tELTInput_3", "dates", &["FROM"]);
        let dates = b.project("tELTMap_1", dates, &[("DATES__FROM", c("FROM"))]);
        let j = b.add(
            "tELTMap_1",
            Op::Join {
                kind: JoinKind::Left,
                keys: vec![("A__K".into(), "B__K".into())],
                condition: None,
            },
            vec![a, t_],
            &["A__K", "A__D", "B__K", "B__PK"],
        );
        let pass = b.project(
            "tELTMap_1",
            j,
            &[
                ("A__K", c("A__K")),
                ("A__D", c("A__D")),
                ("B__PK", c("B__PK")),
            ],
        );
        let j2 = b.add(
            "tELTMap_1",
            Op::Join {
                kind: JoinKind::Inner,
                keys: vec![],
                condition: Some(Expr::bin(BinOp::GtEq, c("A__D"), c("DATES__FROM"))),
            },
            vec![pass, dates],
            &["A__K", "A__D", "B__PK", "DATES__FROM"],
        );
        let d = b.project("tELTMap_1", j2, &[("PK", c("B__PK"))]);
        b.sink("tELTOutput_2", d, "target", SinkMode::Delete, &["PK"], &[]);
        let i = b.project(
            "tELTMap_1",
            j2,
            &[
                ("K", c("A__K")),
                ("PK", Expr::call(Func::Md5Hex, vec![c("A__K")])),
            ],
        );
        b.sink(
            "tELTOutput_1",
            i,
            "target",
            SinkMode::Insert,
            &["K", "PK"],
            &[],
        );
        let mut ir = b.0;
        let r = recognise(&mut ir);
        assert!(r.refused.is_empty(), "{:?}", r.refused);
        let g = &r.groups[0];
        assert_eq!(g.kind, Kind::KeyedReplace { key: "PK".into() });
        // The rebuilt path reads the staged rows and the dates, never the target.
        let v = ir.node(g.sink.unwrap());
        let up = upstream(&ir, v.id);
        assert!(!up.contains(&j) && up.contains(&a) && up.contains(&dates));
        assert!(
            up.iter()
                .all(|x| !ir.node(*x).schema.iter().any(|f| f.name.starts_with("B__")))
        );
    }

    #[test]
    fn a_flag_set_on_the_jobs_own_source_is_a_write_back_and_a_closing_update_is_not() {
        let mut b = B::new();
        let src = b.source("tDBInput_1", "sales", &["ID", "DONE"]);
        let p = b.project(
            "tMap_3",
            src,
            &[("ID", c("ID")), ("DONE", Expr::bool(true))],
        );
        b.sink(
            "tDBOutput_2",
            p,
            "sales",
            SinkMode::Update,
            &["ID", "DONE"],
            &["ID"],
        );
        let mut ir = b.0;
        let r = recognise(&mut ir);
        assert_eq!(r.groups[0].kind, Kind::WriteBack);
        assert_eq!(r.groups[0].sink, None);
        // The SCD2's closing update reads its table through the lookup, not as its data.
        let mut ir = scd2(&update_close, true);
        let r = recognise(&mut ir);
        assert!(!r.groups.iter().any(|g| g.kind == Kind::WriteBack));
    }
}
