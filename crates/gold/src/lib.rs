//! Gold marts from written descriptions (RFC 0195). A description's header is parsed into
//! terms; each term resolves against the deployed plan's silver models or becomes a question;
//! resolved terms compile to a dbt model with many-to-one joins only; prose terms are drafted by
//! a model elsewhere and handed back here; the whole statement is checked against the atoms the
//! description itself states. Pure: no connection, no model, no git.

use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_bronze::typemap::ident;
use ekos_lakehouse_silver::model_name;
use ekos_migrate_logic::invention::{AtomKind, InventionReport, check_candidate_with, sql_atoms};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub mod requirements;

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
        dimensions: pairs(doc.get("dimensions"))
            .map_err(|e| format!("dimensions: {e} (e.g. `month: month of orders.ordered_at`)"))?,
        measures: pairs(doc.get("measures"))
            .map_err(|e| format!("measures: {e} (e.g. `orders: count of orders`)"))?,
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
    /// The column so named: exactly, or (EKOS RFC 0215: Oracle's names are upper case, people
    /// write lower case) the one column whose name differs only in case.
    fn find(&self, col: &str) -> Result<String, Vec<String>> {
        if self.plan.columns.iter().any(|m| m.column == col) {
            return Ok(col.to_string());
        }
        let near: Vec<String> = self
            .plan
            .columns
            .iter()
            .filter(|m| m.column.eq_ignore_ascii_case(col))
            .map(|m| m.column.clone())
            .collect();
        match near.as_slice() {
            [one] => Ok(one.clone()),
            _ => Err(near),
        }
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
    /// EKOS RFC 0215: `… via table.column`: the foreign keys the joins to this term's tables
    /// must pass through, when there are several paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<ColRef>,
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
        // Exactly, or else regardless of case (EKOS RFC 0215); several that differ only in case
        // are listed below as a question.
        let exact = self.by_table.contains_key(table);
        let found: Vec<&Model> = self
            .by_table
            .iter()
            .filter(|(k, _)| {
                if exact {
                    k.as_str() == table
                } else {
                    k.eq_ignore_ascii_case(table)
                }
            })
            .flat_map(|(_, ms)| ms)
            .filter(|m| schema.is_none_or(|s| m.plan.schema.eq_ignore_ascii_case(s)))
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
        let word = word.to_ascii_lowercase();
        let word = word.as_str();
        for (t, ms) in &self.by_table {
            if t.to_ascii_lowercase().contains(word) {
                out.push(t.clone());
            }
            for m in ms {
                for c in &m.plan.columns {
                    if c.column.to_ascii_lowercase().contains(word) {
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
            return match m.find(c) {
                Ok(column) => Ok(ColRef {
                    table: m.plan.table.clone(),
                    column,
                }),
                Err(same) if !same.is_empty() => Err(q(
                    format!(
                        "`{}` has columns differing only in case: which one?",
                        m.plan.table
                    ),
                    same.iter()
                        .map(|x| format!("{}.{x}", m.plan.table))
                        .collect(),
                )),
                Err(_) => Err(q(
                    format!("`{}` has no column `{c}`", m.plan.table),
                    similar(c, m.plan.columns.iter().map(|x| x.column.as_str()))
                        .into_iter()
                        .map(|x| format!("{}.{x}", m.plan.table))
                        .collect(),
                )),
            };
        }
        let mut hits = vec![];
        for (t, ms) in &self.by_table {
            for m in ms {
                if let Ok(column) = m.find(w) {
                    hits.push(ColRef {
                        table: t.clone(),
                        column,
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

/// EKOS RFC 0215: the names like `word` (one contains the other, ignoring case and a leading
/// `is`): `total` → `GRANDTOTAL`, `TOTALLINES`; `isprocessed` → `PROCESSED`. At most twelve; a
/// table's every column is no help to the person asked.
fn similar<'a>(word: &str, names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let norm = |s: &str| {
        let l = s.to_ascii_lowercase().replace('_', "");
        match l.strip_prefix("is") {
            Some(r) if r.len() >= 3 => r.to_string(),
            _ => l,
        }
    };
    let w = norm(word);
    if w.len() < 3 {
        return vec![];
    }
    names
        .filter(|n| {
            let n = norm(n);
            n.len() >= 3 && (n.contains(&w) || w.contains(&n))
        })
        .take(12)
        .map(str::to_string)
        .collect()
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
                // EKOS RFC 0215: `= true` reads as `is true` (silver flags are booleans).
                let lit = if matches!(op, "=" | "<>") && lit.eq_ignore_ascii_case("true") {
                    "true"
                } else if matches!(op, "=" | "<>") && lit.eq_ignore_ascii_case("false") {
                    "false"
                } else {
                    lit
                };
                let literal_ok = lit.parse::<f64>().is_ok()
                    || matches!(lit, "true" | "false")
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

/// `value via t.c[, t2.c2]`: the value, and the foreign-key columns that choose the join path.
fn resolve_term(cat: &Catalog, section: &'static str, name: &str, written: &str) -> Resolved {
    let lower = written.to_ascii_lowercase();
    let (value, via_text) = match lower.rfind(" via ") {
        Some(i) => (&written[..i], Some(&written[i + 5..])),
        None => (written, None),
    };
    let mut via = vec![];
    let mut via_question = None;
    for v in via_text
        .into_iter()
        .flat_map(|t| t.split(','))
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        match cat.column(v) {
            Ok(c) => {
                let is_key = cat.table(&c.table).is_ok_and(|m| {
                    m.plan
                        .facts
                        .foreign_keys
                        .iter()
                        .any(|fk| fk.columns.len() == 1 && fk.columns[0] == c.column)
                });
                if is_key {
                    via.push(c);
                } else {
                    via_question = Some(q(
                        format!(
                            "`via {v}`: `{}.{}` is not a foreign key, so it chooses no join",
                            c.table, c.column
                        ),
                        vec![],
                    ));
                }
            }
            Err(e) => via_question = Some(e),
        }
    }
    let (resolution, how) = match via_question {
        Some(e) => (Err(e), String::new()),
        None => {
            let (r, how) = resolve_value(cat, section, value);
            let how = if via.is_empty() {
                how
            } else {
                format!(
                    "{how}, joined via {}",
                    via.iter()
                        .map(|c| format!("`{}.{}`", c.table, c.column))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            (r, how)
        }
    };
    Resolved {
        term: name.to_string(),
        section,
        written: written.to_string(),
        resolution,
        how,
        via,
    }
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
                via: vec![],
            });
        }
    }
    for (section, items) in [
        ("dimensions", &spec.dimensions),
        ("measures", &spec.measures),
    ] {
        for (name, written) in items {
            out.push(resolve_term(&cat, section, name, written));
        }
    }
    for f in &spec.filters {
        out.push(resolve_term(&cat, "filters", f, f));
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
    // A prose term that names none of them gives a model nothing of the description's to write
    // it from: whatever it drafted (`count(*)` for "customer lifetime value") would be a meaning
    // the author never stated. It is asked back instead.
    for r in &mut out {
        let unanchored =
            matches!(&r.resolution, Ok(Term::Prose { mentions, .. }) if mentions.is_empty());
        if unanchored {
            r.resolution = Err(q(
                format!(
                    "`{}` names no column of the tables this mart reads ({}): write it as \
                     `sum|count|… of table.column`, or name the columns it is computed from",
                    r.written,
                    tables.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
                vec![],
            ));
            r.how = String::new();
        }
    }
    out
}

/// RFC 0195: what a model drafted for one prose term, judged before it joins the mart. It must
/// be one SQL expression — a comma would splice another column into the SELECT, and still plan
/// — and use only the columns its own term names: the description's other columns belong to
/// its other terms, and borrowing them states a meaning this term does not have.
pub fn check_draft(term: &Term, expr: &str) -> Result<(), String> {
    use ekos_sql_dialect_sdk::SqlDialectParser;
    let Term::Prose { mentions, .. } = term else {
        return Ok(());
    };
    let trino = ekos_plugin_sql_dialect_trino::TrinoDialectParser;
    let sql = trino.preprocess(&format!("select {expr}"));
    let statements = sqlparser::parser::Parser::parse_sql(&*trino.sqlparser_dialect(), &sql)
        .map_err(|e| format!("unparseable SQL ({e})"))?;
    let single = match statements.as_slice() {
        [sqlparser::ast::Statement::Query(q)] => match q.body.as_ref() {
            sqlparser::ast::SetExpr::Select(s) => {
                s.projection.len() == 1 && s.from.is_empty() && s.selection.is_none()
            }
            _ => false,
        },
        _ => false,
    };
    if !single {
        return Err(
            "not one SQL expression (one expression only: no list of columns, no SELECT or FROM)"
                .into(),
        );
    }
    let allowed: BTreeSet<String> = mentions.iter().map(|m| m.column.to_lowercase()).collect();
    let foreign: Vec<String> = sql_atoms(&sql)
        .map_err(|e| format!("unparseable SQL ({e})"))?
        .into_iter()
        .filter(|a| a.kind == AtomKind::Column && !allowed.contains(&a.text))
        .map(|a| format!("`{}`", a.text))
        .collect();
    if !foreign.is_empty() {
        return Err(format!(
            "columns this term does not name ({}; it names {})",
            foreign.join(", "),
            if allowed.is_empty() {
                "none".to_string()
            } else {
                allowed
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
    }
    Ok(())
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

type Step = (String, String, String, String);

/// EKOS RFC 0215: a join path longer than this means something nobody wrote, and in a dense key
/// graph (every iDempiere table points at `AD_Client` and `AD_Org`) the search over all simple
/// paths does not finish without a bound.
pub const MAX_HOPS: usize = 4;

/// The many-to-one join path from `base` to every table in `need`, along declared single-column
/// foreign keys (child → parent only: a parent → child join would repeat the base's rows and
/// inflate every total). `via` (EKOS RFC 0215): per table, the `(table, column)` keys its path
/// must pass through.
fn joins(
    plan: &Plan,
    base: &str,
    need: &BTreeSet<String>,
    via: &BTreeMap<String, Vec<(String, String)>>,
) -> Result<Vec<Step>, Question> {
    let cat = Catalog::new(plan);
    // child table → [(child col, parent table, parent col)]
    let mut edges: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for t in &plan.tables {
        for fk in &t.facts.foreign_keys {
            if let ([c], [rc]) = (fk.columns.as_slice(), fk.ref_columns.as_slice())
                && let Ok(parent) = cat.table(&fk.ref_table)
            {
                edges.entry(t.table.clone()).or_default().push((
                    c.clone(),
                    parent.plan.table.clone(),
                    rc.clone(),
                ));
            }
        }
    }
    let mut out: Vec<Step> = vec![];
    for t in need {
        if t == base {
            continue;
        }
        let mut paths = paths_to(&edges, base, t, 64);
        if let Some(keys) = via.get(t) {
            paths.retain(|p| {
                keys.iter()
                    .all(|(vt, vc)| p.iter().any(|(c, cc, ..)| c == vt && cc == vc))
            });
            if paths.is_empty() {
                return Err(q(
                    format!(
                        "no foreign-key path from `{base}` to `{t}` passes through {}",
                        keys.iter()
                            .map(|(a, b)| format!("`{a}.{b}`"))
                            .collect::<Vec<_>>()
                            .join(" and ")
                    ),
                    vec![],
                ));
            }
        }
        let path = match paths.as_slice() {
            [] => {
                return Err(q(
                    format!(
                        "`{t}` cannot be joined to `{base}` along at most {MAX_HOPS} foreign keys \
                         from `{base}` towards it; joining the other way would repeat rows of `{base}`"
                    ),
                    vec![],
                ));
            }
            [one] => one.clone(),
            many => {
                // More than one foreign-key path to a table is more than one meaning (the
                // customer's store, the rented copy's store, the clerk's store): the author picks
                // with `via`, naming a key on the path they mean.
                let show = |p: &Vec<Step>| {
                    p.iter()
                        .map(|(c, cc, parent, _)| format!("{c}.{cc} → {parent}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                // Per path, its last key the other paths do not all share: the one that says which.
                let shared = |st: &Step| many.iter().all(|p| p.contains(st));
                let mut keys: Vec<String> = many
                    .iter()
                    .filter_map(|p| {
                        p.iter()
                            .rev()
                            .find(|st| !shared(st))
                            .map(|(c, cc, ..)| format!("{c}.{cc}"))
                    })
                    .collect();
                keys.sort();
                keys.dedup();
                let shown: Vec<String> = many.iter().take(6).map(show).collect();
                return Err(q(
                    format!(
                        "`{t}` is reached from `{base}` along {}{} foreign-key paths, each a \
                         different meaning ({}{}): add `via` and the key that says which, e.g. \
                         `… via {}`",
                        if many.len() >= 64 { "at least " } else { "" },
                        many.len(),
                        shown.join("; "),
                        if many.len() > 6 { "; …" } else { "" },
                        keys[0]
                    ),
                    keys,
                ));
            }
        };
        for step in path {
            // One table joined along two paths would be two meanings under one name.
            if let Some(other) = out.iter().find(|o| o.2 == step.2 && **o != step) {
                return Err(q(
                    format!(
                        "`{}` would be joined twice, from `{}.{}` and from `{}.{}`: one mart \
                         reads a table along one path; choose one with `via`",
                        step.2, other.0, other.1, step.0, step.1
                    ),
                    vec![
                        format!("{}.{}", other.0, other.1),
                        format!("{}.{}", step.0, step.1),
                    ],
                ));
            }
            if !out.contains(&step) {
                out.push(step);
            }
        }
    }
    Ok(out)
}

/// Every simple many-to-one path from `base` to `target` of at most [`MAX_HOPS`] keys (up to
/// `limit` of them). Only tables that can still reach `target` in the hops left are walked.
fn paths_to(
    edges: &BTreeMap<String, Vec<(String, String, String)>>,
    base: &str,
    target: &str,
    limit: usize,
) -> Vec<Vec<Step>> {
    // Hops from each table to `target`, walking the keys backwards.
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (child, es) in edges {
        for (_, parent, _) in es {
            children
                .entry(parent.as_str())
                .or_default()
                .push(child.as_str());
        }
    }
    let mut dist: BTreeMap<&str, usize> = BTreeMap::from([(target, 0)]);
    let mut queue = VecDeque::from([target]);
    while let Some(t) = queue.pop_front() {
        let d = dist[t];
        if d == MAX_HOPS {
            continue;
        }
        for c in children.get(t).into_iter().flatten() {
            if !dist.contains_key(c) {
                dist.insert(c, d + 1);
                queue.push_back(c);
            }
        }
    }
    struct Walk<'a> {
        edges: &'a BTreeMap<String, Vec<(String, String, String)>>,
        dist: &'a BTreeMap<&'a str, usize>,
        target: &'a str,
        limit: usize,
        on_path: Vec<String>,
        path: Vec<Step>,
        out: Vec<Vec<Step>>,
    }
    fn walk(w: &mut Walk, at: &str) {
        if w.out.len() >= w.limit {
            return;
        }
        if at == w.target {
            w.out.push(w.path.clone());
            return;
        }
        for (c, parent, rc) in w.edges.get(at).into_iter().flatten() {
            let Some(d) = w.dist.get(parent.as_str()) else {
                continue;
            };
            if w.path.len() + 1 + d > MAX_HOPS || w.on_path.contains(parent) {
                continue;
            }
            w.on_path.push(parent.clone());
            w.path
                .push((at.to_string(), c.clone(), parent.clone(), rc.clone()));
            walk(w, parent);
            w.path.pop();
            w.on_path.pop();
        }
    }
    let mut w = Walk {
        edges,
        dist: &dist,
        target,
        limit,
        on_path: vec![base.to_string()],
        path: vec![],
        out: vec![],
    };
    walk(&mut w, base);
    w.out
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
    let mut via: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for r in resolved {
        if r.via.is_empty() {
            continue;
        }
        for t in term_tables(r.resolution.as_ref().expect("no questions")) {
            let keys = via.entry(t).or_default();
            for c in &r.via {
                let k = (c.table.clone(), c.column.clone());
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
    }
    let path = joins(plan, &base, &need, &via).map_err(|e| vec![e])?;
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

/// EKOS RFC 0228 §2.5: per additive measure, one query returning the mart's total and the same
/// aggregate over silver under the mart's joins and filters, both as text (exact decimals).
/// `gold_schema` is where dbt built the mart.
pub fn reconcile_sql(
    spec: &MartSpec,
    resolved: &[Resolved],
    c: &Compiled,
    catalog: &str,
    gold_schema: &str,
) -> Vec<(String, String)> {
    let engine = engine_sql(c, catalog);
    let from_where = engine
        .split_once("\nfrom ")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    let from_where = from_where.split("\ngroup by").next().unwrap_or(from_where);
    let gold = format!(
        "{}.{}.{}",
        ident(catalog),
        ident(gold_schema),
        ident(&spec.mart)
    );
    let mut out = vec![];
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
        out.push((
            r.term.clone(),
            format!(
                "select cast((select sum({m}) from {gold}) as varchar) as mart_total,\n       \
                 cast((select {silver}\n        from {from_where}) as varchar) as silver_total",
                m = ident(&r.term),
            ),
        ));
    }
    out
}

/// EKOS RFC 0228 §2.1: how faithfully each term was carried over — `exact` (a pattern compiled
/// over declared keys) or `drafted` (prose a model wrote: a person should read it).
pub fn fidelity(r: &Resolved) -> &'static str {
    match &r.resolution {
        Ok(Term::Prose { .. }) => "drafted",
        Ok(_) => "exact",
        Err(_) => "question",
    }
}

/// A lineage graph: nodes `(id, label, group)` and edges `(from, to)`.
pub type Lineage = (Vec<(String, String, String)>, Vec<(String, String)>);

/// EKOS RFC 0228 §2.1: the mart's lineage — the silver tables it reads, the keys joining them,
/// the mart.
pub fn lineage(spec: &MartSpec, c: &Compiled) -> Lineage {
    let mart = format!("gold.{}", spec.mart);
    let mut nodes: Vec<(String, String, String)> = c
        .tables
        .iter()
        .map(|t| (t.clone(), t.clone(), "silver".to_string()))
        .collect();
    nodes.push((mart.clone(), spec.mart.clone(), "gold".to_string()));
    let mut edges: Vec<(String, String)> = c
        .joins
        .iter()
        .map(|(child, _, parent, _)| (parent.clone(), child.clone()))
        .collect();
    edges.push((c.base.clone(), mart));
    edges.dedup();
    (nodes, edges)
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
