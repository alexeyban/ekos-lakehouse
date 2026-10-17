//! Gold marts from written descriptions (RFC 0195). A description's header is parsed into
//! terms; each term resolves against the deployed plan's silver models or becomes a question;
//! resolved terms compile to a dbt model with many-to-one joins only; prose terms are drafted by
//! a model elsewhere and handed back here; the whole statement is checked against the atoms the
//! description itself states. Pure: no connection, no model, no git.

use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_bronze::typemap::ident;
use ekos_lakehouse_silver::model_name;
use ekos_migrate_logic::invention::{InventionReport, check_candidate_with, sql_atoms};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

// ─────────────────────────────── the description ───────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MartSpec {
    pub mart: String,
    pub grain: Vec<String>,
    /// In the order written.
    pub dimensions: Vec<(String, String)>,
    pub measures: Vec<(String, String)>,
    pub filters: Vec<String>,
    /// The prose after the header.
    pub prose: String,
}

fn pairs(v: Option<&serde_yaml::Value>) -> Result<Vec<(String, String)>, String> {
    let Some(v) = v else { return Ok(vec![]) };
    let map = v
        .as_mapping()
        .ok_or("expected a mapping of name: definition")?;
    map.iter()
        .map(|(k, v)| {
            let k = k.as_str().ok_or("a name must be text")?;
            let v = match v {
                serde_yaml::Value::String(s) => s.clone(),
                other => serde_yaml::to_string(other)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
            Ok((k.to_string(), v))
        })
        .collect()
}

/// Parse `marts/<name>.md`: a YAML header between `---` lines, then prose.
pub fn parse(text: &str) -> Result<MartSpec, String> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or("a mart description starts with a `---` YAML header")?;
    let (header, prose) = rest
        .split_once("\n---")
        .ok_or("the YAML header is not closed by `---`")?;
    let doc: serde_yaml::Value = serde_yaml::from_str(header).map_err(|e| e.to_string())?;
    let mart = doc["mart"]
        .as_str()
        .ok_or("`mart:` names the mart")?
        .to_string();
    if !mart
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "mart name {mart:?}: lowercase letters, digits and _ only"
        ));
    }
    let list = |k: &str| -> Vec<String> {
        doc[k]
            .as_sequence()
            .map(|s| {
                s.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(MartSpec {
        mart,
        grain: list("grain"),
        dimensions: pairs(doc.get("dimensions"))?,
        measures: pairs(doc.get("measures"))?,
        filters: list("filters"),
        prose: prose.trim_start_matches('\n').trim().to_string(),
    })
}

// ─────────────────────────────── resolution ───────────────────────────────

/// A silver table EKOS can read: its source name, model and columns.
#[derive(Debug, Clone)]
struct Model<'a> {
    plan: &'a TablePlan,
}

impl Model<'_> {
    fn has(&self, col: &str) -> bool {
        self.plan.columns.iter().any(|m| m.column == col)
    }
}

/// A resolved column: `alias."col"` in the SQL, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColRef {
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Term {
    Column(ColRef),
    /// `count of ar`: the table itself.
    Table(String),
    Aggregate {
        func: String,
        distinct: bool,
        arg: Option<ColRef>,
        table: String,
    },
    TimeGrain {
        unit: String,
        col: ColRef,
    },
    Predicate {
        col: ColRef,
        op: String,
        literal: Option<String>,
    },
    /// Prose: a model drafts it; the columns it names (exact column names of tables the mart
    /// already reads) are allowed to it.
    Prose {
        text: String,
        mentions: Vec<ColRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub term: String,
    pub section: &'static str,
    pub written: String,
    pub resolution: Result<Term, Question>,
    /// How it resolved, for the proposal's table.
    pub how: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    pub text: String,
    pub candidates: Vec<String>,
}

fn q(text: impl Into<String>, candidates: Vec<String>) -> Question {
    Question {
        text: text.into(),
        candidates,
    }
}

struct Catalog<'a> {
    by_table: BTreeMap<String, Vec<Model<'a>>>,
}

impl<'a> Catalog<'a> {
    fn new(plan: &'a Plan) -> Self {
        let mut by_table: BTreeMap<String, Vec<Model<'a>>> = BTreeMap::new();
        for t in &plan.tables {
            by_table
                .entry(t.table.clone())
                .or_default()
                .push(Model { plan: t });
        }
        Self { by_table }
    }

    /// `table` or `schema.table` → the one planned model.
    fn table(&self, name: &str) -> Result<&Model<'a>, Question> {
        let (schema, table) = match name.split_once('.') {
            Some((s, t)) => (Some(s), t),
            None => (None, name),
        };
        let found: Vec<&Model> = self
            .by_table
            .get(table)
            .into_iter()
            .flatten()
            .filter(|m| schema.is_none_or(|s| m.plan.schema == s))
            .collect();
        match found.as_slice() {
            [one] => Ok(one),
            [] => Err(q(
                format!("there is no silver table `{name}`"),
                self.near(table),
            )),
            many => Err(q(
                format!("`{name}` is in several schemas; write schema.table"),
                many.iter()
                    .map(|m| format!("{}.{}", m.plan.schema, m.plan.table))
                    .collect(),
            )),
        }
    }

    fn near(&self, word: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for (t, ms) in &self.by_table {
            if t.contains(word) {
                out.push(t.clone());
            }
            for m in ms {
                for c in &m.plan.columns {
                    if c.column.contains(word) {
                        out.push(format!("{t}.{}", c.column));
                    }
                }
            }
        }
        out.sort();
        out.truncate(12);
        out
    }

    /// `table.column` → a column that exists; a bare word → the one column or table so named.
    fn column(&self, written: &str) -> Result<ColRef, Question> {
        let w = written.trim();
        if let Some((t, c)) = w.rsplit_once('.') {
            let m = self.table(t)?;
            return if m.has(c) {
                Ok(ColRef {
                    table: m.plan.table.clone(),
                    column: c.to_string(),
                })
            } else {
                Err(q(
                    format!("`{}` has no column `{c}`", m.plan.table),
                    m.plan
                        .columns
                        .iter()
                        .map(|x| format!("{}.{}", m.plan.table, x.column))
                        .collect(),
                ))
            };
        }
        let mut hits = vec![];
        for (t, ms) in &self.by_table {
            for m in ms {
                if m.has(w) {
                    hits.push(ColRef {
                        table: t.clone(),
                        column: w.to_string(),
                    });
                }
            }
        }
        match hits.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(q(format!("no silver column is named `{w}`"), self.near(w))),
            many => Err(q(
                format!("`{w}` names a column in {} tables; which one?", many.len()),
                many.iter()
                    .map(|c| format!("{}.{}", c.table, c.column))
                    .collect(),
            )),
        }
    }
}

const AGGREGATES: &[(&str, &str)] = &[
    ("sum of ", "sum"),
    ("count distinct ", "count"),
    ("count of ", "count"),
    ("average of ", "avg"),
    ("min of ", "min"),
    ("max of ", "max"),
];
const UNITS: &[&str] = &["day", "week", "month", "quarter", "year"];

fn resolve_value(
    cat: &Catalog,
    section: &'static str,
    written: &str,
) -> (Result<Term, Question>, String) {
    let w = written.trim();
    let lower = w.to_ascii_lowercase();
    if section == "measures" {
        for (prefix, func) in AGGREGATES {
            if lower.starts_with(prefix) {
                let arg = &w[prefix.len()..];
                let distinct = prefix.starts_with("count distinct");
                // `count of ar`: a table.
                if *func == "count" && !distinct && !arg.contains('.') && cat.table(arg).is_ok() {
                    let t = cat
                        .table(arg)
                        .map(|m| m.plan.table.clone())
                        .unwrap_or_default();
                    return (
                        Ok(Term::Aggregate {
                            func: "count".into(),
                            distinct: false,
                            arg: None,
                            table: t.clone(),
                        }),
                        format!("count of the rows of `{t}`"),
                    );
                }
                return match cat.column(arg) {
                    Ok(c) => (
                        Ok(Term::Aggregate {
                            func: func.to_string(),
                            distinct,
                            arg: Some(c.clone()),
                            table: c.table.clone(),
                        }),
                        format!(
                            "{func}{} over `{}.{}`",
                            if distinct { " distinct" } else { "" },
                            c.table,
                            c.column
                        ),
                    ),
                    Err(e) => (Err(e), String::new()),
                };
            }
        }
    }
    if section == "dimensions" {
        for unit in UNITS {
            if lower.starts_with(&format!("{unit} of ")) {
                let arg = &w[unit.len() + 4..];
                return match cat.column(arg) {
                    Ok(c) => (
                        Ok(Term::TimeGrain {
                            unit: unit.to_string(),
                            col: c.clone(),
                        }),
                        format!("`{}.{}` truncated to the {unit}", c.table, c.column),
                    ),
                    Err(e) => (Err(e), String::new()),
                };
            }
        }
    }
    if section == "filters" {
        for (suffix, op, lit) in [
            (" is not null", "is not null", None),
            (" is null", "is null", None),
            (" is true", "=", Some("true")),
            (" is false", "=", Some("false")),
        ] {
            if let Some(arg) = lower.strip_suffix(suffix) {
                let arg = &w[..arg.len()];
                return match cat.column(arg) {
                    Ok(c) => (
                        Ok(Term::Predicate {
                            col: c.clone(),
                            op: op.into(),
                            literal: lit.map(str::to_string),
                        }),
                        format!("`{}.{}`{suffix}", c.table, c.column),
                    ),
                    Err(e) => (Err(e), String::new()),
                };
            }
        }
        for op in ["<>", "<=", ">=", "=", "<", ">"] {
            if let Some((l, r)) = w.split_once(&format!(" {op} ")) {
                let lit = r.trim();
                let literal_ok = lit.parse::<f64>().is_ok()
                    || (lit.starts_with('\'') && lit.ends_with('\'') && lit.len() >= 2);
                if !literal_ok {
                    return (
                        Err(q(
                            format!("`{lit}` is not a number or a 'quoted' value"),
                            vec![],
                        )),
                        String::new(),
                    );
                }
                return match cat.column(l) {
                    Ok(c) => (
                        Ok(Term::Predicate {
                            col: c.clone(),
                            op: op.into(),
                            literal: Some(lit.to_string()),
                        }),
                        format!("`{}.{}` {op} {lit}", c.table, c.column),
                    ),
                    Err(e) => (Err(e), String::new()),
                };
            }
        }
    }
    // A plain reference.
    let looks_like_ref = !w.contains(' ');
    if looks_like_ref {
        return match cat.column(w) {
            Ok(c) => (
                Ok(Term::Column(c.clone())),
                format!("column `{}.{}`", c.table, c.column),
            ),
            Err(e) => (Err(e), String::new()),
        };
    }
    (
        Ok(Term::Prose {
            text: w.to_string(),
            mentions: vec![],
        }),
        "prose: drafted by a model, checked against the description's atoms".into(),
    )
}

/// Resolve every term of a description against the deployed plan.
pub fn resolve(spec: &MartSpec, plan: &Plan) -> Vec<Resolved> {
    let cat = Catalog::new(plan);
    let mut out = vec![];
    for g in &spec.grain {
        if !spec.dimensions.iter().any(|(n, _)| n == g) {
            out.push(Resolved {
                term: g.clone(),
                section: "grain",
                written: g.clone(),
                resolution: Err(q(
                    format!("the grain names `{g}`, which is not one of the dimensions"),
                    spec.dimensions.iter().map(|(n, _)| n.clone()).collect(),
                )),
                how: String::new(),
            });
        }
    }
    for (section, items) in [
        ("dimensions", &spec.dimensions),
        ("measures", &spec.measures),
    ] {
        for (name, written) in items {
            let (resolution, how) = resolve_value(&cat, section, written);
            out.push(Resolved {
                term: name.clone(),
                section,
                written: written.clone(),
                resolution,
                how,
            });
        }
    }
    for f in &spec.filters {
        let (resolution, how) = resolve_value(&cat, "filters", f);
        out.push(Resolved {
            term: f.clone(),
            section: "filters",
            written: f.clone(),
            resolution,
            how,
        });
    }
    // Prose may name exact columns of the tables the mart reads: those are allowed to the draft.
    let tables: BTreeSet<String> = out
        .iter()
        .filter_map(|r| r.resolution.as_ref().ok())
        .flat_map(term_tables)
        .collect();
    for r in &mut out {
        if let Ok(Term::Prose { text, mentions }) = &mut r.resolution {
            for t in &tables {
                if let Ok(m) = cat.table(t) {
                    for c in &m.plan.columns {
                        let word = c.column.as_str();
                        if text
                            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                            .any(|w| w == word)
                        {
                            mentions.push(ColRef {
                                table: t.clone(),
                                column: word.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    out
}

fn term_tables(t: &Term) -> Vec<String> {
    match t {
        Term::Column(c) | Term::TimeGrain { col: c, .. } | Term::Predicate { col: c, .. } => {
            vec![c.table.clone()]
        }
        Term::Table(t) => vec![t.clone()],
        Term::Aggregate { table, .. } => vec![table.clone()],
        Term::Prose { mentions, .. } => mentions.iter().map(|c| c.table.clone()).collect(),
    }
}

pub fn questions(r: &[Resolved]) -> Vec<(&Resolved, &Question)> {
    r.iter()
        .filter_map(|x| x.resolution.as_ref().err().map(|q| (x, q)))
        .collect()
}

// ─────────────────────────────── compilation ───────────────────────────────

/// The many-to-one join path from `base` to every table in `need`, along declared single-column
/// foreign keys (child → parent only: a parent → child join would repeat the base's rows and
/// inflate every total).
fn joins(
    plan: &Plan,
    base: &str,
    need: &BTreeSet<String>,
) -> Result<Vec<(String, String, String, String)>, Question> {
    let cat = Catalog::new(plan);
    // child table → [(child col, parent table, parent col)]
    let mut edges: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for t in &plan.tables {
        for fk in &t.facts.foreign_keys {
            if let ([c], [rc]) = (fk.columns.as_slice(), fk.ref_columns.as_slice())
                && cat.table(&fk.ref_table).is_ok()
            {
                edges.entry(t.table.clone()).or_default().push((
                    c.clone(),
                    fk.ref_table.clone(),
                    rc.clone(),
                ));
            }
        }
    }
    let mut prev: BTreeMap<String, (String, String, String)> = BTreeMap::new(); // table → (from, from col, col)
    let mut seen = BTreeSet::from([base.to_string()]);
    let mut queue = VecDeque::from([base.to_string()]);
    while let Some(t) = queue.pop_front() {
        for (c, parent, rc) in edges.get(&t).into_iter().flatten() {
            if seen.insert(parent.clone()) {
                prev.insert(parent.clone(), (t.clone(), c.clone(), rc.clone()));
                queue.push_back(parent.clone());
            }
        }
    }
    let mut out: Vec<(String, String, String, String)> = vec![];
    for t in need {
        if t == base {
            continue;
        }
        if !prev.contains_key(t) {
            return Err(q(
                format!(
                    "`{t}` cannot be joined to `{base}` along a foreign key from `{base}` towards it; \
                     joining the other way would repeat rows of `{base}`"
                ),
                vec![],
            ));
        }
        let mut cur = t.clone();
        let mut path = vec![];
        while let Some((from, fc, tc)) = prev.get(&cur) {
            path.push((from.clone(), fc.clone(), cur.clone(), tc.clone()));
            cur = from.clone();
        }
        for step in path.into_iter().rev() {
            if !out.contains(&step) {
                out.push(step);
            }
        }
    }
    Ok(out)
}

fn col(c: &ColRef) -> String {
    format!("{}.{}", ident(&c.table), ident(&c.column))
}

fn literal(l: &str) -> String {
    l.to_string()
}

fn predicate_sql(col_ref: &ColRef, op: &str, lit: &Option<String>) -> String {
    match lit {
        None => format!("{} {op}", col(col_ref)),
        Some(l) => format!("{} {op} {}", col(col_ref), literal(l)),
    }
}

/// The compiled mart: its SQL as dbt writes it and as the checks read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Compiled {
    /// With `{{ ref('…') }}`.
    pub dbt_sql: String,
    /// With model names in place of refs: what the anti-invention check and EKOS read.
    pub plain_sql: String,
    /// The source atoms: the description's own facts, written as SQL.
    pub source_sql: String,
    pub base: String,
    pub joins: Vec<(String, String, String, String)>,
    pub tables: Vec<String>,
}

/// Compile resolved terms. `drafted` holds the SQL expression a model wrote for each prose term.
pub fn compile(
    spec: &MartSpec,
    plan: &Plan,
    resolved: &[Resolved],
    drafted: &BTreeMap<String, String>,
) -> Result<Compiled, Vec<Question>> {
    let qs: Vec<Question> = questions(resolved)
        .into_iter()
        .map(|(_, q)| q.clone())
        .collect();
    if !qs.is_empty() {
        return Err(qs);
    }
    let terms: BTreeMap<(&str, &str), &Term> = resolved
        .iter()
        .map(|r| {
            (
                (r.section, r.term.as_str()),
                r.resolution.as_ref().expect("no questions"),
            )
        })
        .collect();
    // The base: the table the measures aggregate.
    let base = spec
        .measures
        .iter()
        .find_map(|(n, _)| match terms[&("measures", n.as_str())] {
            Term::Aggregate { table, .. } => Some(table.clone()),
            _ => None,
        })
        .ok_or_else(|| vec![q("a mart needs at least one measure written as `sum|count|… of table.column`; prose-only measures have no table to start from", vec![])])?;
    let mut need: BTreeSet<String> = BTreeSet::new();
    for r in resolved {
        need.extend(term_tables(r.resolution.as_ref().expect("no questions")));
    }
    let path = joins(plan, &base, &need).map_err(|e| vec![e])?;
    let cat = Catalog::new(plan);
    let model_of = |t: &str| cat.table(t).map(|m| model_name(m.plan)).unwrap_or_default();

    let mut select = vec![];
    let mut group = vec![];
    let mut source_cols: BTreeSet<String> = BTreeSet::new();
    // The exact expressions the description's patterns compile to: their functions and
    // literals (`date_trunc('month', …)`) are the description's own.
    let mut source_exprs: Vec<String> = vec![];
    let mut source_funcs: BTreeSet<String> = BTreeSet::from([
        "sum".into(),
        "count".into(),
        "avg".into(),
        "min".into(),
        "max".into(),
    ]);
    for (i, (name, _)) in spec.dimensions.iter().enumerate() {
        let expr = match terms[&("dimensions", name.as_str())] {
            Term::Column(c) => {
                source_cols.insert(col(c));
                col(c)
            }
            Term::TimeGrain { unit, col: c } => {
                source_cols.insert(col(c));
                source_funcs.insert("date_trunc".into());
                format!("date_trunc('{unit}', {})", col(c))
            }
            Term::Prose { .. } => drafted.get(name).cloned().unwrap_or_else(|| "null".into()),
            other => {
                return Err(vec![q(
                    format!("dimension `{name}` must be a column or a time grain, not {other:?}"),
                    vec![],
                )]);
            }
        };
        select.push(format!("{expr} as {}", ident(name)));
        if !matches!(terms[&("dimensions", name.as_str())], Term::Prose { .. }) {
            source_exprs.push(expr.clone());
        }
        group.push(expr);
        let _ = i;
    }
    for (name, _) in &spec.measures {
        let expr = match terms[&("measures", name.as_str())] {
            Term::Aggregate {
                func,
                distinct,
                arg: Some(c),
                ..
            } => {
                source_cols.insert(col(c));
                let e = format!(
                    "{func}({}{})",
                    if *distinct { "distinct " } else { "" },
                    col(c)
                );
                source_exprs.push(e.clone());
                e
            }
            Term::Aggregate {
                func, arg: None, ..
            } => format!("{func}(*)"),
            Term::Prose { mentions, .. } => {
                for m in mentions {
                    source_cols.insert(col(m));
                }
                drafted.get(name).cloned().unwrap_or_else(|| "null".into())
            }
            other => {
                return Err(vec![q(
                    format!("measure `{name}` must aggregate, not {other:?}"),
                    vec![],
                )]);
            }
        };
        select.push(format!("{expr} as {}", ident(name)));
    }
    let mut wheres = vec![];
    for f in &spec.filters {
        match terms[&("filters", f.as_str())] {
            Term::Predicate {
                col: c,
                op,
                literal,
            } => wheres.push(predicate_sql(c, op, literal)),
            Term::Prose { mentions, .. } => {
                for m in mentions {
                    source_cols.insert(col(m));
                }
                if let Some(x) = drafted.get(f) {
                    wheres.push(x.clone());
                }
            }
            other => {
                return Err(vec![q(
                    format!("filter `{f}` must be a condition, not {other:?}"),
                    vec![],
                )]);
            }
        }
    }
    let from = |refs: bool| -> String {
        let r = |t: &str| {
            if refs {
                format!("{{{{ ref('{}') }}}}", model_of(t))
            } else {
                ident(&model_of(t))
            }
        };
        let mut s = format!("from {} as {}", r(&base), ident(&base));
        for (child, cc, parent, pc) in &path {
            s.push_str(&format!(
                "\nleft join {} as {}\n    on {}.{} = {}.{}",
                r(parent),
                ident(parent),
                ident(child),
                ident(cc),
                ident(parent),
                ident(pc)
            ));
        }
        s
    };
    let body = |refs: bool| -> String {
        let mut s = format!("select\n    {}\n{}", select.join(",\n    "), from(refs));
        if !wheres.is_empty() {
            s.push_str(&format!("\nwhere {}", wheres.join("\n    and ")));
        }
        if !group.is_empty() {
            s.push_str(&format!("\ngroup by {}", group.join(", ")));
        }
        s.push('\n');
        s
    };
    // The source atoms: every table, column, join and predicate the description resolved to,
    // and the functions its patterns name. Nothing a model wrote is in here.
    let mut src_cols: Vec<String> = source_cols.iter().cloned().collect();
    for (child, cc, parent, pc) in &path {
        src_cols.push(format!("{}.{}", ident(child), ident(cc)));
        src_cols.push(format!("{}.{}", ident(parent), ident(pc)));
    }
    let src_preds: Vec<String> = spec
        .filters
        .iter()
        .filter_map(|f| match terms[&("filters", f.as_str())] {
            Term::Predicate {
                col: c,
                op,
                literal,
            } => Some(predicate_sql(c, op, literal)),
            _ => None,
        })
        .chain(path.iter().map(|(child, cc, parent, pc)| {
            format!(
                "{}.{} = {}.{}",
                ident(child),
                ident(cc),
                ident(parent),
                ident(pc)
            )
        }))
        .collect();
    // Function names alone, for prose drafts (`sum(*)` names `sum` and states no literal).
    let funcs: Vec<String> = source_funcs.iter().map(|f| format!("{f}(*)")).collect();
    let mut tables: Vec<String> = std::iter::once(base.clone())
        .chain(path.iter().map(|p| p.2.clone()))
        .collect();
    tables.dedup();
    let source_sql = format!(
        "select {}, {} from {}{}",
        src_cols
            .iter()
            .chain(source_exprs.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        funcs.join(", "),
        tables
            .iter()
            .map(|t| format!("{} as {}", ident(&model_of(t)), ident(t)))
            .collect::<Vec<_>>()
            .join(", "),
        if src_preds.is_empty() {
            String::new()
        } else {
            format!(" where {}", src_preds.join(" and "))
        }
    );
    let header = format!(
        "-- Generated by ekos-lakehouse (RFC 0195) from marts/{}.md. Approve by merging; do not edit.\n\
         {{{{ config(materialized='table', schema='gold', alias='{}') }}}}\n\n",
        spec.mart, spec.mart
    );
    Ok(Compiled {
        dbt_sql: format!("{header}{}", body(true)),
        plain_sql: body(false),
        source_sql,
        base,
        joins: path,
        tables,
    })
}

/// RFC 0195 §4: the finished statement against the description's own atoms.
pub fn check(c: &Compiled) -> InventionReport {
    let source = sql_atoms(&c.source_sql).unwrap_or_default();
    let trino = ekos_plugin_sql_dialect_trino::TrinoDialectParser;
    use ekos_sql_dialect_sdk::SqlDialectParser;
    check_candidate_with(
        &source,
        &trino.preprocess(&c.plain_sql),
        &*trino.sqlparser_dialect(),
    )
}

/// The mart as the engine runs it: dbt's refs replaced by the silver tables themselves
/// (`{{ ref('public__ar') }}` → `"iceberg"."silver_public"."ar"`), the config line dropped. For
/// `EXPLAIN`, which plans a statement without running it.
pub fn engine_sql(c: &Compiled, catalog: &str) -> String {
    let mut out = String::new();
    let mut rest = c.dbt_sql.as_str();
    while let Some(i) = rest.find("{{ ref('") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "{{ ref('".len()..];
        let Some(j) = after.find("') }}") else { break };
        let model = &after[..j];
        let (schema, table) = model.split_once("__").unwrap_or(("public", model));
        out.push_str(&format!(
            "{}.{}.{}",
            ident(catalog),
            ident(&format!("silver_{schema}")),
            ident(table)
        ));
        rest = &after[j + "') }}".len()..];
    }
    out.push_str(rest);
    out.lines()
        .filter(|l| !l.starts_with("--") && !l.starts_with("{{ config("))
        .collect::<Vec<_>>()
        .join("\n")
}

// ─────────────────────────────── files ───────────────────────────────

pub fn mart_id(c: &Compiled) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(c.dbt_sql.as_bytes()))[..12].to_string()
}

/// The model's YAML: grain uniqueness over the dimensions; each column described by its term.
pub fn model_yaml(spec: &MartSpec, resolved: &[Resolved]) -> String {
    use serde_yaml::{Mapping, Value};
    let mut cols = vec![];
    for r in resolved
        .iter()
        .filter(|r| r.section != "filters" && r.section != "grain")
    {
        let mut c = Mapping::new();
        c.insert("name".into(), r.term.clone().into());
        c.insert(
            "description".into(),
            format!("{} — {}", r.written, r.how).into(),
        );
        cols.push(Value::Mapping(c));
    }
    let mut test_args = Mapping::new();
    test_args.insert(
        "columns".into(),
        Value::Sequence(spec.grain.iter().map(|g| Value::from(ident(g))).collect()),
    );
    let mut t = Mapping::new();
    let mut body = Mapping::new();
    body.insert("arguments".into(), Value::Mapping(test_args));
    t.insert("ekos_unique_combination".into(), Value::Mapping(body));
    let mut model = Mapping::new();
    model.insert("name".into(), format!("gold__{}", spec.mart).into());
    model.insert("description".into(), spec.prose.clone().into());
    if !spec.grain.is_empty() {
        model.insert(
            "data_tests".into(),
            Value::Sequence(vec![Value::Mapping(t)]),
        );
    }
    model.insert("columns".into(), Value::Sequence(cols));
    let mut root = Mapping::new();
    root.insert("version".into(), 2.into());
    root.insert(
        "models".into(),
        Value::Sequence(vec![Value::Mapping(model)]),
    );
    format!(
        "# Generated by ekos-lakehouse (RFC 0195) from marts/{}.md.\n{}",
        spec.mart,
        serde_yaml::to_string(&Value::Mapping(root)).expect("YAML serializes")
    )
}

/// One reconciliation test per additive measure: the mart's total equals the same aggregate
/// over the base silver model, under the same joins and filters.
pub fn reconciliation_tests(
    spec: &MartSpec,
    resolved: &[Resolved],
    c: &Compiled,
) -> Vec<(String, String)> {
    let mut out = vec![];
    let from = c
        .dbt_sql
        .split_once("\nfrom ")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    let from_where = from.split("\ngroup by").next().unwrap_or(from);
    // Indented one level more inside the CTE (sqlfluff LT02).
    let from_where = from_where.replace('\n', "\n    ");
    for r in resolved.iter().filter(|r| r.section == "measures") {
        let Ok(Term::Aggregate {
            func,
            distinct: false,
            arg,
            ..
        }) = &r.resolution
        else {
            continue;
        };
        if func != "sum" && func != "count" {
            continue;
        }
        let silver = match arg {
            Some(a) => format!("{func}({})", col(a)),
            None => "count(*)".to_string(),
        };
        let sql = format!(
            "-- Generated by ekos-lakehouse (RFC 0195): `{m}` in gold__{mart} reconciles to silver.\n\
             with mart as (\n    select sum({mi}) as v from {{{{ ref('gold__{mart}') }}}}\n),\n\n\
             silver as (\n    select {silver} as v\n    from {from_where}\n)\n\n\
             select mart.v as mart_total, silver.v as silver_total\nfrom mart, silver\n\
             where mart.v is distinct from silver.v\n",
            m = r.term,
            mart = spec.mart,
            mi = ident(&r.term),
        );
        out.push((format!("gold__{}__{}_reconciles", spec.mart, r.term), sql));
    }
    out
}

/// The proposal a stakeholder and a data engineer review (RFC 0195 §6).
pub fn proposal_markdown(
    spec: &MartSpec,
    resolved: &[Resolved],
    c: &Compiled,
    report: &InventionReport,
    drafted: &BTreeMap<String, String>,
) -> String {
    let mut s = format!(
        "# Gold mart `{}` — proposal {}\n\nGenerated by ekos-lakehouse (RFC 0195) from `marts/{}.md`. \
         Merging this branch is the approval (the stakeholder and a data engineer).\n\n\
         ## Terms\n\n| Term | Written | Resolved to |\n|---|---|---|\n",
        spec.mart,
        mart_id(c),
        spec.mart
    );
    for r in resolved.iter().filter(|r| r.section != "grain") {
        let how = if let Some(d) = drafted.get(&r.term) {
            format!("{} → `{d}`", r.how)
        } else {
            r.how.clone()
        };
        s.push_str(&format!(
            "| {} ({}) | {} | {} |\n",
            r.term,
            r.section,
            r.written.replace('|', "\\|"),
            how
        ));
    }
    s.push_str(&format!(
        "\n## Joins\n\nBase: `{}`. {}\n\n## Anti-invention\n\n{} atoms checked, {} not stated by the description{}.\n\n## SQL\n\n```sql\n{}```\n",
        c.base,
        if c.joins.is_empty() {
            "No joins.".to_string()
        } else {
            c.joins
                .iter()
                .map(|(a, ac, b, bc)| format!("`{a}.{ac}` → `{b}.{bc}`"))
                .collect::<Vec<_>>()
                .join(", ")
                + " (many-to-one, left joins)"
        },
        report.checked,
        report.unmapped.len(),
        if report.accepted() { "" } else { " — REJECTED" },
        c.dbt_sql
    ));
    s
}

/// The questions file for the author (RFC 0195 §2).
pub fn questions_markdown(spec: &MartSpec, qs: &[(String, Question)]) -> String {
    let mut s = format!(
        "# Questions about `marts/{}.md`\n\nGenerated by ekos-lakehouse (RFC 0195). No SQL was written: \
         every term must resolve first. Edit the description and run `lakehouse gold draft` again.\n\n",
        spec.mart
    );
    for (term, q) in qs {
        s.push_str(&format!("## `{term}`\n\n{}\n", q.text));
        if !q.candidates.is_empty() {
            s.push_str("\nCandidates:\n\n");
            for c in &q.candidates {
                s.push_str(&format!("- `{c}`\n"));
            }
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests;
