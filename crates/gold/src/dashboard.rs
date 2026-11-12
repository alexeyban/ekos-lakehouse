//! Dashboards from terms (EKOS RFC 0248 §2.4, RFC 0258 §3.1–§3.3): the bundle a published
//! dashboard keeps, the chart SQL compiled from a chart's terms over the gold marts, a change as a
//! **delta** to those terms and its class, the silver check a chart is reconciled with, and the
//! term-level diff and three-way merge of two versions.
//!
//! Pure: no connection, no model, no git, no Superset. The model's only output is terms (a request
//! or a delta, schema-checked here); every term resolves against the deployed plan through the
//! same resolver a mart description uses (RFC 0195), and the SQL is compiled here.

use crate::ask::{Change, NamedTerm};
use crate::{MartSpec, Question, Resolved, Term, compile, engine_sql, mart_id, questions, resolve};
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::typemap::ident;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The bundle format this module reads and writes.
pub const BUNDLE_FORMAT: u32 = 1;
/// Where bundles live in the pipeline repository: `dashboards/<key>.yaml`.
pub const BUNDLE_DIR: &str = "dashboards";
/// Chart types (RFC 0258 §2: styling is chart type, title, order, number format, colour scheme).
pub const VIZ: &[&str] = &["bar", "line", "pie", "big_number", "table"];
/// Colour schemes by name, as Superset 4.1 ships them.
pub const COLOR_SCHEMES: &[&str] = &[
    "supersetColors",
    "bnbColors",
    "googleCategory10c",
    "googleCategory20c",
    "d3Category10",
    "d3Category20",
    "d3Category20b",
    "d3Category20c",
    "echarts4Colors",
    "echarts5Colors",
    "lyftColors",
    "presetColors",
    "blueToGreen",
];
const MAX_CHARTS: usize = 12;
const MAX_TERMS: usize = crate::ask::MAX_TERMS;
const MAX_TITLE: usize = 80;

// ─────────────────────────────── the bundle ───────────────────────────────

/// A period on a time dimension of the chart: `[from, to)`, each `YYYY-MM-DD` on the start of the
/// dimension's unit, so filtering the truncated value equals filtering the raw one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub dimension: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// What a chart is compiled from: terms only. A field that could carry SQL does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartRequest {
    pub measures: Vec<NamedTerm>,
    #[serde(default)]
    pub dimensions: Vec<NamedTerm>,
    /// In the description's vocabulary: `c_order.issotrx is true`, `c_bpartner.name <> 'Internal'`.
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<Period>,
}

/// How a chart is drawn. `x`/`group_by` name dimensions, `metric` a measure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presentation {
    pub title: String,
    pub viz: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default)]
    pub order: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_scheme: Option<String>,
}

/// One resolved term and the rule that applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TermRecord {
    pub section: String,
    pub name: String,
    pub written: String,
    /// The resolver's rule (`sum over \`C_ORDER.GRANDTOTAL\``).
    pub rule: String,
    /// Where the chart reads it: `gold.<mart>.<column>`, or how a filter is applied.
    pub served_by: String,
}

/// A mart a chart reads, by the digest of its compiled SQL when the chart was compiled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MartDigest {
    pub mart: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureCheck {
    pub measure: String,
    pub chart_total: String,
    pub silver_total: String,
    pub equal: bool,
}

/// RFC 0248 §2.4: the chart's totals against the same terms compiled over silver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconciled {
    pub at: String,
    pub ok: bool,
    #[serde(default)]
    pub measures: Vec<MeasureCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersetChart {
    pub chart_id: i64,
    pub dataset_id: i64,
    /// The dataset's name in Superset.
    pub dataset: String,
}

/// One chart of a bundle: what it was compiled from and what was published.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleChart {
    pub key: String,
    pub presentation: Presentation,
    pub request: ChartRequest,
    /// The mart it reads.
    pub mart: String,
    pub marts: Vec<MartDigest>,
    pub terms: Vec<TermRecord>,
    pub sql: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciled: Option<Reconciled>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superset: Option<SupersetChart>,
    /// The form data as published, compared with Superset's for drift (RFC 0258 §3.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Where a copy came from (RFC 0258 §3.3: someone else's dashboard is changed as a copy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub dashboard: String,
    /// The original's version the branch was made from.
    pub commit: String,
    pub superset_dashboard_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersetDashboard {
    pub dashboard_id: i64,
    pub url: String,
}

/// `dashboards/<key>.yaml`: one published dashboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub format: u32,
    pub dashboard: String,
    pub title: String,
    pub author: String,
    /// The person's words this version came from (a question or a change).
    #[serde(default)]
    pub request_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superset: Option<SupersetDashboard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    pub charts: Vec<BundleChart>,
    /// RFC 0258 §3.6: charts of an adopted dashboard EKOS could not read, with the atoms it could
    /// not resolve. They are not changed by request; drift does not judge them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unadopted: Vec<Unadopted>,
}

/// A chart adoption could not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unadopted {
    pub chart_id: i64,
    pub title: String,
    /// Each atom that did not resolve, as a question to the user.
    pub atoms: Vec<String>,
}

impl Bundle {
    pub fn path(key: &str) -> String {
        format!("{BUNDLE_DIR}/{key}.yaml")
    }

    pub fn to_yaml(&self) -> String {
        serde_yaml::to_string(self).unwrap_or_default()
    }

    pub fn from_yaml(text: &str) -> Result<Bundle, String> {
        let b: Bundle = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
        if b.format != BUNDLE_FORMAT {
            return Err(format!(
                "bundle format {} (this build reads {BUNDLE_FORMAT})",
                b.format
            ));
        }
        if !valid_key(&b.dashboard) {
            return Err(format!("{:?} is not a dashboard key", b.dashboard));
        }
        Ok(b)
    }

    pub fn chart(&self, key_or_title: &str) -> Option<&BundleChart> {
        let k = key_or_title.trim();
        self.charts.iter().find(|c| c.key == k).or_else(|| {
            self.charts
                .iter()
                .find(|c| c.presentation.title.eq_ignore_ascii_case(k))
        })
    }

    /// Every mart the dashboard reads.
    pub fn marts(&self) -> BTreeSet<String> {
        self.charts.iter().map(|c| c.mart.clone()).collect()
    }
}

/// A dashboard or chart key: lowercase letters, digits, `-` and `_`.
pub fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

// ─────────────────────────────── checking terms ───────────────────────────────

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 40
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !n.starts_with(|c: char| c.is_ascii_digit())
}

/// A term's text: no statement separators, comments, backslashes, backticks, double quotes or
/// control characters; single quotes only as one `'value'` literal at the end of a filter.
fn text_problem(d: &str, filter: bool) -> Option<String> {
    if d.trim().is_empty() {
        return Some("is empty".into());
    }
    if d.chars().count() > 200 {
        return Some("is longer than 200 characters".into());
    }
    let bad = d
        .chars()
        .any(|c| c.is_control() || matches!(c, ';' | '"' | '`' | '\\'))
        || d.contains("--")
        || d.contains("/*");
    if bad {
        return Some("contains characters a term may not hold (`;`, comments, quotes)".into());
    }
    let quotes = d.matches('\'').count();
    if quotes == 0 {
        return None;
    }
    // One literal, last, with nothing inside it that ends it.
    let ok = filter
        && quotes == 2
        && d.trim_end().ends_with('\'')
        && d.find('\'').is_some_and(|i| {
            let inner = &d.trim_end()[i + 1..d.trim_end().len() - 1];
            inner
                .chars()
                .all(|c| c.is_alphanumeric() || " _.-/:&()".contains(c))
        });
    (!ok).then(|| {
        "a quoted value is allowed only at the end of a filter, without quotes inside".into()
    })
}

fn valid_date(s: &str) -> Option<(i32, u32, u32)> {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() != 3 || p[0].len() != 4 || p[1].len() != 2 || p[2].len() != 2 {
        return None;
    }
    let (y, m, d) = (p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

fn unit_of(definition: &str) -> Option<&'static str> {
    let lower = definition.trim().to_ascii_lowercase();
    ["day", "week", "month", "quarter", "year"]
        .into_iter()
        .find(|u| lower.starts_with(&format!("{u} of ")))
}

fn aligned(unit: &str, (_, m, d): (i32, u32, u32)) -> bool {
    match unit {
        "month" => d == 1,
        "quarter" => d == 1 && matches!(m, 1 | 4 | 7 | 10),
        "year" => d == 1 && m == 1,
        _ => true,
    }
}

impl ChartRequest {
    /// Why this request cannot be used, if it cannot. Checked before anything else sees it.
    pub fn check(&self) -> Result<(), String> {
        if self.measures.is_empty() {
            return Err("a chart needs at least one measure".into());
        }
        let mut names = BTreeSet::new();
        for (what, terms) in [("measure", &self.measures), ("dimension", &self.dimensions)] {
            if terms.len() > MAX_TERMS {
                return Err(format!("at most {MAX_TERMS} {what}s"));
            }
            for t in terms {
                if !valid_name(&t.name) {
                    return Err(format!(
                        "{what} name {:?}: lowercase letters, digits and _ only, starting with a letter",
                        t.name
                    ));
                }
                if !names.insert(t.name.clone()) {
                    return Err(format!("{:?} is named twice", t.name));
                }
                if let Some(p) = text_problem(&t.definition, false) {
                    return Err(format!("{what} {}: definition {p}", t.name));
                }
            }
        }
        if self.filters.len() > MAX_TERMS {
            return Err(format!("at most {MAX_TERMS} filters"));
        }
        for f in &self.filters {
            if let Some(p) = text_problem(f, true) {
                return Err(format!("filter {f:?} {p}"));
            }
        }
        if let Some(p) = &self.period {
            let Some(d) = self.dimensions.iter().find(|d| d.name == p.dimension) else {
                return Err(format!(
                    "the period's dimension {:?} is not a dimension",
                    p.dimension
                ));
            };
            let Some(unit) = unit_of(&d.definition) else {
                return Err(format!(
                    "the period's dimension {} is not a time dimension (`month of …`)",
                    d.name
                ));
            };
            if p.from.is_none() && p.to.is_none() {
                return Err("a period needs `from`, `to` or both".into());
            }
            for s in [&p.from, &p.to].into_iter().flatten() {
                let Some(date) = valid_date(s) else {
                    return Err(format!("{s:?} is not a date (YYYY-MM-DD)"));
                };
                if !aligned(unit, date) {
                    return Err(format!("{s} is not the start of a {unit}"));
                }
            }
        }
        Ok(())
    }

    /// The request as a mart description named `mart` (grain = its dimensions). The period is not
    /// part of it: it is applied on the time dimension's values.
    pub fn to_spec(&self, mart: &str) -> MartSpec {
        MartSpec {
            mart: mart.to_string(),
            grain: self.dimensions.iter().map(|d| d.name.clone()).collect(),
            dimensions: pairs(&self.dimensions),
            measures: pairs(&self.measures),
            filters: self.filters.clone(),
            prose: String::new(),
        }
    }

    /// Equal as terms: order, case and spacing do not count.
    pub fn same_terms(&self, other: &ChartRequest) -> bool {
        let key = |r: &ChartRequest| {
            (
                set(&r.measures),
                set(&r.dimensions),
                r.filters.iter().map(|f| norm(f)).collect::<BTreeSet<_>>(),
                r.period.clone(),
            )
        };
        key(self) == key(other)
    }
}

fn pairs(terms: &[NamedTerm]) -> Vec<(String, String)> {
    terms
        .iter()
        .map(|t| (t.name.clone(), t.definition.clone()))
        .collect()
}

fn set(terms: &[NamedTerm]) -> BTreeSet<(String, String)> {
    terms
        .iter()
        .map(|t| (t.name.clone(), norm(&t.definition)))
        .collect()
}

/// Compared without case or spacing (quoted values keep their case).
pub fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut space = false;
    for c in s.trim().chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if !quoted && c.is_whitespace() {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(if quoted { c } else { c.to_ascii_lowercase() });
    }
    out
}

impl Presentation {
    /// The presentation against the request it draws.
    pub fn check(&self, req: &ChartRequest) -> Result<(), String> {
        if self.title.trim().is_empty() || self.title.chars().count() > MAX_TITLE {
            return Err(format!("a chart title is 1 to {MAX_TITLE} characters"));
        }
        if self
            .title
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '`' | '\\'))
        {
            return Err("a chart title may not hold < > ` or \\".into());
        }
        if !VIZ.contains(&self.viz.as_str()) {
            return Err(format!(
                "chart type {:?}: one of {}",
                self.viz,
                VIZ.join(", ")
            ));
        }
        let dims: BTreeSet<&str> = req.dimensions.iter().map(|d| d.name.as_str()).collect();
        let measures: BTreeSet<&str> = req.measures.iter().map(|m| m.name.as_str()).collect();
        for (what, v) in [("x", &self.x), ("group_by", &self.group_by)] {
            if let Some(v) = v
                && !dims.contains(v.as_str())
            {
                return Err(format!("{what} {v:?} is not a dimension of the chart"));
            }
        }
        if let Some(m) = &self.metric
            && !measures.contains(m.as_str())
        {
            return Err(format!("metric {m:?} is not a measure of the chart"));
        }
        if let Some(f) = &self.number_format
            && (f.len() > 16
                || !f
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ",.$%~+-".contains(c)))
        {
            return Err(format!(
                "number format {f:?} is not a d3 format (e.g. `,.2f`)"
            ));
        }
        if let Some(c) = &self.color_scheme
            && !COLOR_SCHEMES.contains(&c.as_str())
        {
            return Err(format!(
                "colour scheme {c:?}: one of {}",
                COLOR_SCHEMES.join(", ")
            ));
        }
        match self.viz.as_str() {
            "table" => Ok(()),
            "big_number" => {
                if dims.is_empty() {
                    Ok(())
                } else {
                    Err(
                        "a big number has no dimension: remove them or use another chart type"
                            .into(),
                    )
                }
            }
            _ => {
                if self.x.is_none() {
                    return Err(format!("a {} chart needs `x`", self.viz));
                }
                let drawn: BTreeSet<&str> = [&self.x, &self.group_by]
                    .into_iter()
                    .flatten()
                    .map(String::as_str)
                    .collect();
                if drawn != dims {
                    return Err(format!(
                        "a {} chart draws `x` and `group_by`; the chart's dimensions are {} — \
                         use a table for more",
                        self.viz,
                        dims.into_iter().collect::<Vec<_>>().join(", ")
                    ));
                }
                Ok(())
            }
        }
    }
}

// ─────────────────────────────── the marts ───────────────────────────────

/// Where gold lives on the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub catalog: String,
    pub gold_schema: String,
}

/// A mart the pipeline repository describes, resolved and compiled once.
#[derive(Debug, Clone)]
pub struct Mart {
    pub spec: MartSpec,
    pub digest: String,
    pub resolved: Vec<Resolved>,
}

/// Every described mart that compiles; the others are skipped (a mart that does not compile is
/// not built, so no chart reads it). The digest is `mart_id` of its compiled SQL.
pub fn marts_of(specs: &[MartSpec], plan: &Plan) -> Vec<Mart> {
    let mut out: Vec<Mart> = specs
        .iter()
        .filter_map(|s| {
            let resolved = resolve(s, plan);
            let c = compile(s, plan, &resolved, &BTreeMap::new()).ok()?;
            Some(Mart {
                spec: s.clone(),
                digest: mart_id(&c),
                resolved,
            })
        })
        .collect();
    out.sort_by(|a, b| a.spec.mart.cmp(&b.spec.mart));
    out
}

/// What a resolved term means — its term and the keys its joins pass through.
type Meaning = Option<(Term, Vec<crate::ColRef>)>;

/// One chart rebuilt from merged terms: its key, presentation and request.
pub type MergedChart = (String, Presentation, ChartRequest);

/// What a resolved term means, for comparing a chart's terms with a mart's.
fn meaning(r: &Resolved) -> Meaning {
    r.resolution.clone().ok().map(|t| (t, r.via.clone()))
}

fn additive(t: &Term) -> bool {
    matches!(t, Term::Aggregate { func, distinct: false, .. } if func == "sum" || func == "count")
}

/// A chart compiled over one mart.
#[derive(Debug, Clone, PartialEq)]
pub struct Served {
    pub mart: String,
    pub digest: String,
    /// The SQL without `order by`: what totals are computed over.
    pub core_sql: String,
    /// The dataset's SQL.
    pub sql: String,
    pub terms: Vec<TermRecord>,
    /// The mart's filters the request did not state, added to it (`inherit`).
    pub inherited: Vec<String>,
    /// Whether the chart totals a finer mart (its grain has more dimensions than the chart).
    pub rollup: bool,
    /// The number of dimensions of the mart's grain.
    pub grain: usize,
}

fn sql_literal(l: &Option<String>) -> String {
    l.clone().unwrap_or_default()
}

/// The chart's terms resolved, or the questions they raise.
pub fn resolve_chart(req: &ChartRequest, plan: &Plan) -> Result<Vec<Resolved>, Vec<Question>> {
    let resolved = resolve(&req.to_spec("chart"), plan);
    let qs: Vec<Question> = questions(&resolved)
        .into_iter()
        .map(|(_, q)| q.clone())
        .collect();
    if !qs.is_empty() {
        return Err(qs);
    }
    let prose: Vec<Question> = resolved
        .iter()
        .filter(|r| matches!(r.resolution, Ok(Term::Prose { .. })))
        .map(|r| Question {
            text: format!(
                "`{}` is prose, not a column or an aggregate of one: write it as \
                 `sum|count|… of table.column`",
                r.written
            ),
            candidates: vec![],
        })
        .collect();
    if prose.is_empty() {
        Ok(resolved)
    } else {
        Err(prose)
    }
}

struct Mapping<'a> {
    dims: Vec<(&'a str, &'a str)>,
    measures: Vec<(&'a str, &'a str, bool)>,
    /// (chart filter as written, the SQL over the mart) for filters applied on a dimension.
    applied: Vec<(String, String, String)>,
    inherited: Vec<String>,
}

fn by_section<'a>(r: &'a [Resolved], s: &str) -> impl Iterator<Item = &'a Resolved> {
    let s = s.to_string();
    r.iter().filter(move |x| x.section == s)
}

fn map_terms<'a>(
    chart: &'a [Resolved],
    mart: &'a Mart,
    inherit: bool,
) -> Result<Mapping<'a>, String> {
    let m = &mart.spec.mart;
    let mut dims = vec![];
    for d in by_section(chart, "dimensions") {
        let want = meaning(d);
        let Some(md) = by_section(&mart.resolved, "dimensions").find(|x| meaning(x) == want) else {
            return Err(format!("{m} has no dimension `{}`", d.written));
        };
        dims.push((d.term.as_str(), md.term.as_str()));
    }
    let mut measures = vec![];
    for x in by_section(chart, "measures") {
        let want = meaning(x);
        let Some(mm) = by_section(&mart.resolved, "measures").find(|y| meaning(y) == want) else {
            return Err(format!("{m} has no measure `{}`", x.written));
        };
        let add = x.resolution.as_ref().is_ok_and(additive);
        measures.push((x.term.as_str(), mm.term.as_str(), add));
    }
    let chart_filters: Vec<(&Resolved, Meaning)> = by_section(chart, "filters")
        .map(|f| (f, meaning(f)))
        .collect();
    let mut built_in = vec![];
    let mut inherited = vec![];
    for mf in by_section(&mart.resolved, "filters") {
        if chart_filters.iter().any(|(_, w)| *w == meaning(mf)) {
            built_in.push(norm(&mf.written));
        } else if inherit {
            inherited.push(mf.written.clone());
        } else {
            return Err(format!("{m} keeps only rows where `{}`", mf.written));
        }
    }
    let mut applied = vec![];
    for (f, w) in &chart_filters {
        if built_in.contains(&norm(&f.written)) {
            continue;
        }
        let Some((Term::Predicate { col, op, literal }, _)) = w else {
            return Err(format!("`{}` cannot be applied over {m}", f.written));
        };
        let on = by_section(&mart.resolved, "dimensions")
            .find(|d| matches!(&d.resolution, Ok(Term::Column(c)) if c == col));
        let Some(on) = on else {
            return Err(format!(
                "`{}` filters a column {m} does not carry as a dimension",
                f.written
            ));
        };
        let sql = match literal {
            None => format!("{} {op}", ident(&on.term)),
            l => format!("{} {op} {}", ident(&on.term), sql_literal(l)),
        };
        applied.push((f.written.clone(), on.term.clone(), sql));
    }
    Ok(Mapping {
        dims,
        measures,
        applied,
        inherited,
    })
}

fn period_sql(p: &Period, column: &str) -> Vec<String> {
    let mut out = vec![];
    if let Some(f) = &p.from {
        out.push(format!("{} >= timestamp '{f} 00:00:00'", ident(column)));
    }
    if let Some(t) = &p.to {
        out.push(format!("{} < timestamp '{t} 00:00:00'", ident(column)));
    }
    out
}

/// The chart over one mart, or why that mart cannot serve it. `inherit`: the mart's own filters
/// the request does not state are added to it (a new chart); otherwise they must be stated.
pub fn serve(
    req: &ChartRequest,
    chart: &[Resolved],
    mart: &Mart,
    target: &Target,
    inherit: bool,
) -> Result<Served, String> {
    let map = map_terms(chart, mart, inherit)?;
    let grain: BTreeSet<&str> = mart.spec.grain.iter().map(String::as_str).collect();
    let used: BTreeSet<&str> = map.dims.iter().map(|(_, md)| *md).collect();
    let rollup = used != grain;
    if rollup && let Some((n, _, _)) = map.measures.iter().find(|(_, _, add)| !add) {
        return Err(format!(
            "`{n}` is not a sum or a count, so it cannot be totalled over {}'s finer grain",
            mart.spec.mart
        ));
    }
    let col = |mart_name: &str, chart_name: &str| {
        if mart_name == chart_name {
            ident(mart_name)
        } else {
            format!("{} as {}", ident(mart_name), ident(chart_name))
        }
    };
    let mut select: Vec<String> = map.dims.iter().map(|(c, md)| col(md, c)).collect();
    for (c, mm, _) in &map.measures {
        select.push(if rollup {
            format!("sum({}) as {}", ident(mm), ident(c))
        } else {
            col(mm, c)
        });
    }
    let mut wheres: Vec<String> = map.applied.iter().map(|(_, _, s)| s.clone()).collect();
    if let Some(p) = &req.period {
        let md = map
            .dims
            .iter()
            .find(|(c, _)| *c == p.dimension)
            .map(|(_, md)| *md)
            .unwrap_or(p.dimension.as_str());
        wheres.extend(period_sql(p, md));
    }
    let from = format!(
        "{}.{}.{}",
        ident(&target.catalog),
        ident(&target.gold_schema),
        ident(&mart.spec.mart)
    );
    let mut core = format!("select {}\nfrom {from}", select.join(",\n       "));
    if !wheres.is_empty() {
        core.push_str(&format!("\nwhere {}", wheres.join("\n  and ")));
    }
    let n = map.dims.len();
    if rollup && n > 0 {
        core.push_str(&format!(
            "\ngroup by {}",
            (1..=n)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let sql = if n > 0 {
        format!(
            "{core}\norder by {}",
            (1..=n)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        core.clone()
    };
    let m = &mart.spec.mart;
    let mut terms = vec![];
    for r in chart {
        let served_by = match r.section {
            "dimensions" => map
                .dims
                .iter()
                .find(|(c, _)| *c == r.term)
                .map(|(_, md)| format!("gold.{m}.{md}"))
                .unwrap_or_default(),
            "measures" => map
                .measures
                .iter()
                .find(|(c, _, _)| *c == r.term)
                .map(|(_, mm, add)| {
                    if rollup && *add {
                        format!("sum of gold.{m}.{mm}")
                    } else {
                        format!("gold.{m}.{mm}")
                    }
                })
                .unwrap_or_default(),
            _ => match map.applied.iter().find(|(w, _, _)| *w == r.written) {
                Some((_, on, _)) => format!("applied on gold.{m}.{on}"),
                None => format!("built into gold.{m}"),
            },
        };
        terms.push(TermRecord {
            section: r.section.to_string(),
            name: r.term.clone(),
            written: r.written.clone(),
            rule: r.how.clone(),
            served_by,
        });
    }
    for f in by_section(&mart.resolved, "filters").filter(|f| map.inherited.contains(&f.written)) {
        terms.push(TermRecord {
            section: "filters".into(),
            name: f.term.clone(),
            written: f.written.clone(),
            rule: f.how.clone(),
            served_by: format!("built into gold.{m}"),
        });
    }
    Ok(Served {
        mart: m.clone(),
        digest: mart.digest.clone(),
        core_sql: core,
        sql,
        terms,
        inherited: map.inherited,
        rollup,
        grain: mart.spec.grain.len(),
    })
}

/// The chart over the first mart that serves it (`prefer` first, then by name), or why none does.
pub fn compile_chart(
    req: &ChartRequest,
    plan: &Plan,
    marts: &[Mart],
    target: &Target,
    prefer: Option<&str>,
    inherit: bool,
) -> Result<Served, Unserved> {
    req.check().map_err(Unserved::Invalid)?;
    let chart = resolve_chart(req, plan).map_err(Unserved::Questions)?;
    // Every mart that serves it; the preferred one first, then one at the chart's own grain, then
    // the coarsest, then by name — never the order the descriptions were read in.
    let mut served = vec![];
    let mut why = vec![];
    for m in marts {
        match serve(req, &chart, m, target, inherit) {
            Ok(mut s) => {
                s.inherited.sort();
                served.push(s);
            }
            Err(e) => why.push((m.spec.mart.clone(), e)),
        }
    }
    served.sort_by_key(|s| {
        (
            Some(s.mart.as_str()) != prefer,
            s.rollup,
            s.grain,
            s.mart.clone(),
        )
    });
    match served.into_iter().next() {
        Some(s) => Ok(s),
        None => {
            why.sort();
            Err(Unserved::NoMart(why))
        }
    }
}

/// Why a chart has no SQL over the existing marts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Unserved {
    Invalid(String),
    Questions(Vec<Question>),
    /// Every mart, with why it does not serve the chart.
    NoMart(Vec<(String, String)>),
}

/// The request with the filters `served` inherited, as recorded in the bundle.
pub fn with_inherited(req: &ChartRequest, served: &Served) -> ChartRequest {
    let mut r = req.clone();
    for f in &served.inherited {
        if !r.filters.iter().any(|x| norm(x) == norm(f)) {
            r.filters.push(f.clone());
        }
    }
    r
}

// ─────────────────────────────── the silver check ───────────────────────────────

/// RFC 0248 §2.4: per additive measure, the chart's total and the same terms compiled over
/// silver (not through gold), each returning one text value. `(measure, chart_sql, silver_sql)`.
pub fn reconcile_queries(
    req: &ChartRequest,
    served: &Served,
    plan: &Plan,
    catalog: &str,
) -> Result<Vec<(String, String, String)>, String> {
    let spec = req.to_spec("chart_check");
    let resolved = resolve(&spec, plan);
    let c = compile(&spec, plan, &resolved, &BTreeMap::new())
        .map_err(|qs| format!("the chart's terms do not compile over silver: {}", qs.len()))?;
    let silver = engine_sql(&c, catalog);
    let mut period = vec![];
    if let Some(p) = &req.period {
        period = period_sql(p, &p.dimension);
    }
    let silver_where = if period.is_empty() {
        String::new()
    } else {
        format!(" where {}", period.join(" and "))
    };
    let mut out = vec![];
    for r in resolved.iter().filter(|r| r.section == "measures") {
        if !r.resolution.as_ref().is_ok_and(additive) {
            continue;
        }
        let m = ident(&r.term);
        out.push((
            r.term.clone(),
            format!(
                "select cast(sum({m}) as varchar) from (\n{}\n) as chart",
                served.core_sql
            ),
            format!("select cast(sum({m}) as varchar) from (\n{silver}\n) as silver{silver_where}"),
        ));
    }
    Ok(out)
}

/// Two totals as text are equal as decimals (`12.50` = `12.5`; both empty = equal).
pub fn totals_equal(a: &str, b: &str) -> bool {
    fn canon(s: &str) -> String {
        let s = s.trim();
        if !s.contains('.') {
            return s.trim_start_matches('+').to_string();
        }
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
    canon(a) == canon(b)
}

// ─────────────────────────────── a change ───────────────────────────────

/// A change to one chart, as the model returns it: a delta to the stored terms and presentation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartDelta {
    /// The chart's key or title.
    pub chart: String,
    #[serde(default)]
    pub add_measures: Vec<NamedTerm>,
    #[serde(default)]
    pub remove_measures: Vec<String>,
    #[serde(default)]
    pub add_dimensions: Vec<NamedTerm>,
    #[serde(default)]
    pub remove_dimensions: Vec<String>,
    #[serde(default)]
    pub add_filters: Vec<String>,
    #[serde(default)]
    pub remove_filters: Vec<String>,
    #[serde(default)]
    pub period: Option<Period>,
    #[serde(default)]
    pub clear_period: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub viz: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub clear_group_by: bool,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub order: Option<u32>,
    #[serde(default)]
    pub number_format: Option<String>,
    #[serde(default)]
    pub color_scheme: Option<String>,
}

/// A change request: deltas to some charts and, optionally, the dashboard's title.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub charts: Vec<ChartDelta>,
}

/// A delta from the model's JSON. Anything outside the schema drops it whole.
pub fn parse_delta(json_text: &str) -> Result<Delta, String> {
    let d: Delta = serde_json::from_str(json_text)
        .map_err(|e| format!("not a valid change (terms only, no SQL): {e}"))?;
    if d.charts.is_empty() && d.title.is_none() {
        return Err("the change changes nothing".into());
    }
    if d.charts.len() > MAX_CHARTS {
        return Err(format!("at most {MAX_CHARTS} charts"));
    }
    if let Some(t) = &d.title
        && (t.trim().is_empty() || t.chars().count() > MAX_TITLE || t.contains(['<', '>', '`']))
    {
        return Err("a dashboard title is 1 to 80 characters without < > `".into());
    }
    Ok(d)
}

fn remove_named(terms: &mut Vec<NamedTerm>, names: &[String], what: &str) -> Result<(), String> {
    for n in names {
        let before = terms.len();
        terms.retain(|t| &t.name != n);
        if terms.len() == before {
            return Err(format!("the chart has no {what} {n:?}"));
        }
    }
    Ok(())
}

/// The chart's new presentation and request after `d`. Both are checked.
pub fn apply_chart_delta(
    c: &BundleChart,
    d: &ChartDelta,
) -> Result<(Presentation, ChartRequest), String> {
    let mut req = c.request.clone();
    let mut p = c.presentation.clone();
    remove_named(&mut req.measures, &d.remove_measures, "measure")?;
    remove_named(&mut req.dimensions, &d.remove_dimensions, "dimension")?;
    for f in &d.remove_filters {
        let before = req.filters.len();
        req.filters.retain(|x| norm(x) != norm(f));
        if req.filters.len() == before {
            return Err(format!("the chart has no filter {f:?}"));
        }
    }
    req.measures.extend(d.add_measures.iter().cloned());
    req.dimensions.extend(d.add_dimensions.iter().cloned());
    for f in &d.add_filters {
        if !req.filters.iter().any(|x| norm(x) == norm(f)) {
            req.filters.push(f.clone());
        }
    }
    if d.clear_period {
        req.period = None;
    }
    if let Some(per) = &d.period {
        req.period = Some(per.clone());
    }
    if req
        .period
        .as_ref()
        .is_some_and(|per| d.remove_dimensions.contains(&per.dimension))
    {
        req.period = None;
    }
    // A removed dimension or measure leaves the presentation unless it is replaced.
    let gone = |v: &Option<String>, names: &[String]| v.as_ref().is_some_and(|x| names.contains(x));
    if gone(&p.x, &d.remove_dimensions) {
        p.x = None;
    }
    if gone(&p.group_by, &d.remove_dimensions) {
        p.group_by = None;
    }
    if gone(&p.metric, &d.remove_measures) {
        p.metric = req.measures.first().map(|m| m.name.clone());
    }
    if let Some(t) = &d.title {
        p.title = t.clone();
    }
    if let Some(v) = &d.viz {
        p.viz = v.clone();
    }
    if let Some(x) = &d.x {
        p.x = Some(x.clone());
    }
    if d.clear_group_by {
        p.group_by = None;
    }
    if let Some(g) = &d.group_by {
        p.group_by = Some(g.clone());
    }
    if let Some(m) = &d.metric {
        p.metric = Some(m.clone());
    }
    if let Some(o) = d.order {
        p.order = o;
    }
    if let Some(f) = &d.number_format {
        p.number_format = Some(f.clone());
    }
    if let Some(cs) = &d.color_scheme {
        p.color_scheme = Some(cs.clone());
    }
    req.check()?;
    p.check(&req)?;
    Ok((p, req))
}

/// RFC 0258 §3.2: what a change needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeClass {
    Presentation,
    Query,
    MartAddition,
    MartChange,
    NeedsClarification,
    Impossible,
}

impl ChangeClass {
    pub fn name(self) -> &'static str {
        match self {
            ChangeClass::Presentation => "presentation",
            ChangeClass::Query => "query",
            ChangeClass::MartAddition => "mart_addition",
            ChangeClass::MartChange => "mart_change",
            ChangeClass::NeedsClarification => "needs_clarification",
            ChangeClass::Impossible => "impossible",
        }
    }
}

/// One chart's change, classified.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Classified {
    pub chart: String,
    pub class: ChangeClass,
    pub presentation: Presentation,
    pub request: ChartRequest,
    /// The chart's SQL over existing marts (`presentation`, `query`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub served: Option<ServedView>,
    /// The mart to add, or the mart as changed (`mart_addition`, `mart_change`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mart: Option<MartSpecView>,
    /// `mart_addition`: an existing mart of the same shape that new measures would extend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extend: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<Change>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// [`Served`] for output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServedView {
    pub mart: String,
    pub digest: String,
    pub sql: String,
    pub terms: Vec<TermRecord>,
}

impl From<&Served> for ServedView {
    fn from(s: &Served) -> Self {
        ServedView {
            mart: s.mart.clone(),
            digest: s.digest.clone(),
            sql: s.sql.clone(),
            terms: s.terms.clone(),
        }
    }
}

/// A mart description for output and for writing `marts/<name>.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MartSpecView {
    pub mart: String,
    pub grain: Vec<String>,
    pub dimensions: Vec<(String, String)>,
    pub measures: Vec<(String, String)>,
    pub filters: Vec<String>,
}

impl From<&MartSpec> for MartSpecView {
    fn from(s: &MartSpec) -> Self {
        MartSpecView {
            mart: s.mart.clone(),
            grain: s.grain.clone(),
            dimensions: s.dimensions.clone(),
            measures: s.measures.clone(),
            filters: s.filters.clone(),
        }
    }
}

impl MartSpecView {
    /// `marts/<name>.md` as RFC 0195 reads it.
    pub fn markdown(&self, prose: &str) -> String {
        let mut s = format!("---\nmart: {}\ngrain:\n", self.mart);
        for g in &self.grain {
            s.push_str(&format!("- {g}\n"));
        }
        s.push_str("dimensions:\n");
        for (n, d) in &self.dimensions {
            s.push_str(&format!("  {n}: {d}\n"));
        }
        s.push_str("measures:\n");
        for (n, d) in &self.measures {
            s.push_str(&format!("  {n}: {d}\n"));
        }
        if !self.filters.is_empty() {
            s.push_str("filters:\n");
            for f in &self.filters {
                s.push_str(&format!("- {f}\n"));
            }
        }
        s.push_str(&format!("---\n\n{}\n", prose.trim()));
        s
    }
}

fn fingerprint(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The name a mart added for a chart gets: by its terms, so the same terms add one mart.
pub fn added_mart_name(req: &ChartRequest) -> String {
    let canon = serde_json::to_string(&(
        set(&req.measures),
        set(&req.dimensions),
        req.filters.iter().map(|f| norm(f)).collect::<BTreeSet<_>>(),
    ))
    .unwrap_or_default();
    format!("dash_{}", fingerprint(&canon))
}

/// Classify one chart's change (RFC 0258 §3.2). `marts` are the described marts; the chart's own
/// mart is preferred when it still serves.
pub fn classify(
    old: &BundleChart,
    presentation: Presentation,
    request: ChartRequest,
    plan: &Plan,
    marts: &[Mart],
    target: &Target,
) -> Classified {
    let mut out = Classified {
        chart: old.key.clone(),
        class: ChangeClass::Presentation,
        presentation,
        request,
        served: None,
        mart: None,
        extend: None,
        changes: vec![],
        questions: vec![],
        reasons: vec![],
    };
    if out.request.same_terms(&old.request) {
        return out;
    }
    let chart = match resolve_chart(&out.request, plan) {
        Ok(c) => c,
        Err(qs) => {
            let hopeless: Vec<String> = qs
                .iter()
                .filter(|q| q.candidates.is_empty() && !q.text.contains("prose"))
                .map(|q| q.text.clone())
                .collect();
            if hopeless.is_empty() {
                out.class = ChangeClass::NeedsClarification;
                out.questions = qs;
            } else {
                out.class = ChangeClass::Impossible;
                out.reasons = hopeless;
            }
            return out;
        }
    };
    match compile_chart(&out.request, plan, marts, target, Some(&old.mart), false) {
        Ok(s) => {
            out.class = ChangeClass::Query;
            out.served = Some((&s).into());
            return out;
        }
        Err(Unserved::NoMart(why)) => {
            out.reasons = why.into_iter().map(|(m, w)| format!("{m}: {w}")).collect()
        }
        Err(Unserved::Invalid(e)) => {
            out.class = ChangeClass::Impossible;
            out.reasons = vec![e];
            return out;
        }
        Err(Unserved::Questions(qs)) => {
            out.class = ChangeClass::NeedsClarification;
            out.questions = qs;
            return out;
        }
    }
    if let Some(base) = marts.iter().find(|m| m.spec.mart == old.mart)
        && let Some((spec, changes)) = as_mart_change(&chart, base)
    {
        out.class = ChangeClass::MartChange;
        out.mart = Some((&spec).into());
        out.changes = changes;
        return out;
    }
    out.class = ChangeClass::MartAddition;
    let spec = out.request.to_spec(&added_mart_name(&out.request));
    let shape = |m: &Mart| {
        let keyed = |r: &[Resolved], s: &str| {
            by_section(r, s)
                .map(|x| format!("{:?}", meaning(x)))
                .collect::<BTreeSet<_>>()
        };
        keyed(&m.resolved, "dimensions") == keyed(&chart, "dimensions")
            && keyed(&m.resolved, "filters") == keyed(&chart, "filters")
    };
    out.extend = marts.iter().find(|m| shape(m)).map(|m| m.spec.mart.clone());
    out.mart = Some((&spec).into());
    out
}

/// The chart's own mart with its filters changed so it serves the chart, when the chart's
/// dimensions and measures are all the mart's: the filters a dimension cannot carry move into
/// the mart. `None` when more than filters would change (that is an addition).
fn as_mart_change(chart: &[Resolved], base: &Mart) -> Option<(MartSpec, Vec<Change>)> {
    let has = |section: &str| {
        by_section(chart, section)
            .all(|x| by_section(&base.resolved, section).any(|y| meaning(y) == meaning(x)))
    };
    if !has("dimensions") || !has("measures") {
        return None;
    }
    // A filter on a plain column the mart carries stays at chart level; the rest go into it.
    let mut into_mart = vec![];
    for f in by_section(chart, "filters") {
        let on_dim = matches!(&f.resolution, Ok(Term::Predicate { col, .. })
            if by_section(&base.resolved, "dimensions")
                .any(|d| matches!(&d.resolution, Ok(Term::Column(c)) if c == col)));
        if !on_dim {
            into_mart.push(f.written.clone());
        }
    }
    let was: BTreeSet<String> = base.spec.filters.iter().map(|f| norm(f)).collect();
    let now: BTreeSet<String> = into_mart.iter().map(|f| norm(f)).collect();
    if was == now {
        return None;
    }
    let mut spec = base.spec.clone();
    spec.filters = into_mart.clone();
    Some((
        spec,
        vec![Change::Filters {
            was: base.spec.filters.clone(),
            now: into_mart,
        }],
    ))
}

// ─────────────────────────────── diff and merge ───────────────────────────────

/// A bundle as terms: one entry per chart, measure, dimension, filter, period and presentation
/// field, and the dashboard's title. Merges and diffs work on this, never on YAML text.
pub fn term_map(b: &Bundle) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("title".to_string(), json!(b.title));
    for c in &b.charts {
        let k = &c.key;
        m.insert(format!("chart {k}"), json!(true));
        for t in &c.request.measures {
            m.insert(
                format!("chart {k} measure {}", t.name),
                json!(norm(&t.definition)),
            );
        }
        for t in &c.request.dimensions {
            m.insert(
                format!("chart {k} dimension {}", t.name),
                json!(norm(&t.definition)),
            );
        }
        for f in &c.request.filters {
            m.insert(format!("chart {k} filter {}", norm(f)), json!(f));
        }
        if let Some(p) = &c.request.period {
            m.insert(format!("chart {k} period"), json!(p));
        }
        if let Value::Object(fields) = json!(c.presentation) {
            for (f, v) in fields {
                if !v.is_null() {
                    m.insert(format!("chart {k} presentation {f}"), v);
                }
            }
        }
    }
    m
}

/// One term that differs between two versions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TermChange {
    pub term: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

/// Terms first: what differs between `a` and `b`.
pub fn term_diff(a: &Bundle, b: &Bundle) -> Vec<TermChange> {
    let (ma, mb) = (term_map(a), term_map(b));
    let keys: BTreeSet<&String> = ma.keys().chain(mb.keys()).collect();
    keys.into_iter()
        .filter(|k| ma.get(*k) != mb.get(*k))
        .map(|k| TermChange {
            term: k.clone(),
            before: ma.get(k).cloned(),
            after: mb.get(k).cloned(),
        })
        .collect()
}

/// SQL second: a line diff (`-` removed, `+` added, two spaces kept).
pub fn sql_diff(a: &str, b: &str) -> Vec<String> {
    let (x, y): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let (n, m) = (x.len(), y.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if x[i] == y[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, vec![]);
    while i < n || j < m {
        if i < n && j < m && x[i] == y[j] {
            out.push(format!("  {}", x[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(format!("+ {}", y[j]));
            j += 1;
        } else {
            out.push(format!("- {}", x[i]));
            i += 1;
        }
    }
    out
}

/// Two changes to the same term: the merging person chooses one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    pub term: String,
    pub base: Option<Value>,
    pub ours: Option<Value>,
    pub theirs: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Ours,
    Theirs,
}

/// Three-way merge at the level of terms: `ours` is the original now, `theirs` the branch,
/// `base` the version the branch was made from. A term both sides changed differently is a
/// conflict unless `choose` names a side for it. Nothing is merged half-way: any open conflict
/// returns all of them.
pub fn merge3(
    base: &Bundle,
    ours: &Bundle,
    theirs: &Bundle,
    choose: &BTreeMap<String, Side>,
) -> Result<BTreeMap<String, Value>, Vec<Conflict>> {
    let (b, o, t) = (term_map(base), term_map(ours), term_map(theirs));
    let keys: BTreeSet<&String> = b.keys().chain(o.keys()).chain(t.keys()).collect();
    let mut out = BTreeMap::new();
    let mut conflicts = vec![];
    for k in keys {
        let (vb, vo, vt) = (b.get(k), o.get(k), t.get(k));
        let v = if vo == vt || vt == vb {
            vo
        } else if vo == vb {
            vt
        } else {
            match choose.get(k) {
                Some(Side::Ours) => vo,
                Some(Side::Theirs) => vt,
                None => {
                    conflicts.push(Conflict {
                        term: k.clone(),
                        base: vb.cloned(),
                        ours: vo.cloned(),
                        theirs: vt.cloned(),
                    });
                    continue;
                }
            }
        };
        if let Some(v) = v {
            out.insert(k.clone(), v.clone());
        }
    }
    if conflicts.is_empty() {
        Ok(out)
    } else {
        Err(conflicts)
    }
}

/// A merged term map back into charts' presentations and requests, in `ours`' chart order then
/// `theirs`'. Each chart is checked; a chart that no longer holds together is an error.
pub fn from_terms(
    merged: &BTreeMap<String, Value>,
    ours: &Bundle,
    theirs: &Bundle,
) -> Result<(String, Vec<MergedChart>), String> {
    let title = merged
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&ours.title)
        .to_string();
    let mut keys: Vec<String> = vec![];
    for c in ours.charts.iter().chain(theirs.charts.iter()) {
        if !keys.contains(&c.key) && merged.contains_key(&format!("chart {}", c.key)) {
            keys.push(c.key.clone());
        }
    }
    let defs = |b: &Bundle, k: &str| b.charts.iter().find(|c| c.key == k).cloned();
    let mut out = vec![];
    for k in keys {
        let prefix = format!("chart {k} ");
        let src = defs(ours, &k)
            .or_else(|| defs(theirs, &k))
            .expect("listed from them");
        let src_t = defs(theirs, &k);
        // Definitions as written, from whichever side has the merged value.
        let written = |section: &str, name: &str, want: &Value| -> String {
            [Some(&src), src_t.as_ref()]
                .into_iter()
                .flatten()
                .flat_map(|c| {
                    let terms = if section == "measure" {
                        &c.request.measures
                    } else {
                        &c.request.dimensions
                    };
                    terms
                        .iter()
                        .filter(|t| t.name == name)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .find(|t| json!(norm(&t.definition)) == *want)
                .map(|t| t.definition)
                .unwrap_or_else(|| want.as_str().unwrap_or_default().to_string())
        };
        let mut req = ChartRequest {
            measures: vec![],
            dimensions: vec![],
            filters: vec![],
            period: None,
        };
        let mut pres = serde_json::Map::new();
        for (key, v) in merged.range(prefix.clone()..) {
            let Some(rest) = key.strip_prefix(&prefix) else {
                break;
            };
            let (kind, name) = rest.split_once(' ').unwrap_or((rest, ""));
            match kind {
                "measure" => req.measures.push(NamedTerm {
                    name: name.into(),
                    definition: written("measure", name, v),
                }),
                "dimension" => req.dimensions.push(NamedTerm {
                    name: name.into(),
                    definition: written("dimension", name, v),
                }),
                "filter" => req.filters.push(v.as_str().unwrap_or_default().to_string()),
                "period" => req.period = serde_json::from_value(v.clone()).ok(),
                "presentation" => {
                    pres.insert(name.to_string(), v.clone());
                }
                _ => {}
            }
        }
        // Keep the written order of the source chart where the terms are the same.
        let order = |terms: &mut Vec<NamedTerm>, src: &[NamedTerm]| {
            terms.sort_by_key(|t| {
                src.iter()
                    .position(|s| s.name == t.name)
                    .unwrap_or(usize::MAX)
            });
        };
        order(&mut req.measures, &src.request.measures);
        order(&mut req.dimensions, &src.request.dimensions);
        let p: Presentation = serde_json::from_value(Value::Object(pres))
            .map_err(|e| format!("chart {k}: the merged presentation does not hold: {e}"))?;
        req.check().map_err(|e| format!("chart {k}: {e}"))?;
        p.check(&req).map_err(|e| format!("chart {k}: {e}"))?;
        out.push((k, p, req));
    }
    Ok((title, out))
}

// ─────────────────────────────── Superset form data ───────────────────────────────

/// Superset's viz type for a chart type.
pub fn viz_type(viz: &str) -> &'static str {
    match viz {
        "bar" => "echarts_timeseries_bar",
        "line" => "echarts_timeseries_line",
        "pie" => "pie",
        "big_number" => "big_number_total",
        _ => "table",
    }
}

/// The chart's form data over its dataset (the same vocabulary as the Superset fork's
/// `planner.form_data`): the SQL is already at the chart's grain, so the metric is `SUM` of the
/// measure's column.
pub fn form_data(c: &BundleChart, dataset_id: i64) -> Value {
    let p = &c.presentation;
    let viz = viz_type(&p.viz);
    let mut fd = json!({"datasource": format!("{dataset_id}__table"), "viz_type": viz});
    let measure = p
        .metric
        .clone()
        .or_else(|| c.request.measures.first().map(|m| m.name.clone()))
        .unwrap_or_default();
    let metric = json!({
        "expressionType": "SIMPLE", "column": {"column_name": measure},
        "aggregate": "SUM", "label": measure,
    });
    match p.viz.as_str() {
        "table" => {
            let cols: Vec<String> = c
                .request
                .dimensions
                .iter()
                .chain(c.request.measures.iter())
                .map(|t| t.name.clone())
                .collect();
            fd["query_mode"] = json!("raw");
            fd["all_columns"] = json!(cols);
            fd["row_limit"] = json!(1000);
        }
        "big_number" => fd["metric"] = metric,
        "pie" => {
            fd["groupby"] = json!([p.x]);
            fd["metric"] = metric;
            fd["row_limit"] = json!(50);
        }
        _ => {
            fd["x_axis"] = json!(p.x);
            fd["metrics"] = json!([metric]);
            fd["groupby"] = json!(p.group_by.iter().collect::<Vec<_>>());
            fd["row_limit"] = json!(1000);
            fd["stack"] = json!(p.group_by.is_some());
            fd["x_axis_sort_asc"] = json!(true);
        }
    }
    if let Some(f) = &p.number_format {
        fd["number_format"] = json!(f);
        fd["y_axis_format"] = json!(f);
    }
    if let Some(cs) = &p.color_scheme {
        fd["color_scheme"] = json!(cs);
    }
    fd
}

/// position_json: the charts in their `order`, two to a row (the fork's layout).
pub fn layout(title: &str, charts: &[(i64, String)]) -> String {
    let mut doc = json!({
        "DASHBOARD_VERSION_KEY": "v2",
        "ROOT_ID": {"type": "ROOT", "id": "ROOT_ID", "children": ["GRID_ID"]},
        "GRID_ID": {"type": "GRID", "id": "GRID_ID", "children": [], "parents": ["ROOT_ID"]},
        "HEADER_ID": {"id": "HEADER_ID", "type": "HEADER", "meta": {"text": title}},
    });
    for (row_n, pair) in charts.chunks(2).enumerate() {
        let row = format!("ROW-{row_n}");
        let mut kids = vec![];
        for (i, (id, name)) in pair.iter().enumerate() {
            let cid = format!("CHART-{}", row_n * 2 + i);
            doc[&cid] = json!({
                "type": "CHART", "id": cid, "children": [],
                "parents": ["ROOT_ID", "GRID_ID", row],
                "meta": {"width": 6, "height": 50, "chartId": id, "sliceName": name},
            });
            kids.push(json!(cid));
        }
        doc[&row] = json!({
            "type": "ROW", "id": row, "children": kids,
            "parents": ["ROOT_ID", "GRID_ID"], "meta": {"background": "BACKGROUND_TRANSPARENT"},
        });
        doc["GRID_ID"]["children"]
            .as_array_mut()
            .expect("an array")
            .push(json!(row));
    }
    doc.to_string()
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod dashboard_tests;
