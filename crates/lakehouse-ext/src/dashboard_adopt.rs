//! EKOS RFC 0258 §3.6 (phase 5): adopting a Superset dashboard EKOS did not publish. Each chart's
//! dataset (its SQL, or the physical table) and the chart's own metrics, dimensions and filters are
//! resolved to terms over the gold marts:
//!
//! - every atom resolves → the chart gets a bundle entry and can be changed by request;
//! - some do not → the chart is listed with each unresolved atom, as a question;
//! - a dataset reading tables outside gold and silver → the adoption is refused, naming them.
//!
//! Adoption publishes nothing and changes nothing in Superset: it records what EKOS understood, as
//! a bundle committed to the pipeline repository, and reports the share it understood.

use crate::dashboard_change::{Ctx, bundle_now, keys};
use ekos_lakehouse_gold::ask::NamedTerm;
use ekos_lakehouse_gold::dashboard::{
    self as dash, Bundle, BundleChart, ChartRequest, MartDigest, Presentation, SupersetChart,
    SupersetDashboard, Unadopted,
};
use ekos_lakehouse_gold::{ColRef, Term};
use serde_json::{Value, json};
use sqlparser::ast::{
    BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, GroupByExpr, SelectItem,
    SetExpr, Statement, TableFactor, Value as SqlValue,
};

/// What one chart's dataset reads.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// One gold mart, with what the dataset's SQL selects from it (`None`: the table itself).
    Mart { mart: String, sql: Option<Parsed> },
    /// Tables outside gold and silver: the adoption is refused.
    Outside(Vec<String>),
    /// Something EKOS cannot read as one mart (a join, silver, SQL it does not parse).
    Unreadable(Vec<String>),
}

/// A dataset query over one mart, as far as it was understood.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// Output column → the mart column it is, with `sum`/`count` when aggregated.
    pub columns: Vec<(String, String, Option<String>)>,
    /// `(mart column, op, literal)`.
    pub filters: Vec<(String, String, String)>,
    pub atoms: Vec<String>,
}

fn name_parts(n: &sqlparser::ast::ObjectName) -> Vec<String> {
    n.0.iter().map(|i| i.value.to_ascii_lowercase()).collect()
}

fn literal(v: &SqlValue) -> Option<String> {
    match v {
        SqlValue::Number(n, _) => Some(n.clone()),
        SqlValue::SingleQuotedString(s)
            if s.chars()
                .all(|c| c.is_alphanumeric() || " _.-/:&()".contains(c)) =>
        {
            Some(format!("'{s}'"))
        }
        SqlValue::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn ident_of(e: &Expr) -> Option<String> {
    match e {
        Expr::Identifier(i) => Some(i.value.to_ascii_lowercase()),
        Expr::CompoundIdentifier(p) => p.last().map(|i| i.value.to_ascii_lowercase()),
        Expr::Nested(x) => ident_of(x),
        _ => None,
    }
}

fn op_text(op: &BinaryOperator) -> Option<&'static str> {
    Some(match op {
        BinaryOperator::Eq => "=",
        BinaryOperator::NotEq => "<>",
        BinaryOperator::Lt => "<",
        BinaryOperator::LtEq => "<=",
        BinaryOperator::Gt => ">",
        BinaryOperator::GtEq => ">=",
        _ => return None,
    })
}

fn conjuncts<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            conjuncts(left, out);
            conjuncts(right, out);
        }
        Expr::Nested(x) => conjuncts(x, out),
        other => out.push(other),
    }
}

/// `sum(col)` / `count(col)` / `count(*)` over a plain column: `(func, column)`.
fn aggregate(e: &Expr) -> Option<(String, Option<String>)> {
    let Expr::Function(f) = e else { return None };
    let func = f.name.to_string().to_ascii_lowercase();
    if func != "sum" && func != "count" {
        return None;
    }
    let FunctionArguments::List(list) = &f.args else {
        return None;
    };
    if list.duplicate_treatment.is_some() || list.args.len() != 1 {
        return None;
    }
    match &list.args[0] {
        FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => Some((func, None)),
        FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) => ident_of(x).map(|c| (func, Some(c))),
        _ => None,
    }
}

/// Read a dataset's SQL: one SELECT from one table, plain columns and sums or counts of them,
/// `AND`ed comparisons with literals. Every other construct is an unresolved atom.
pub fn read_sql(sql: &str, gold_schema: &str, marts: &[String]) -> Source {
    let dialect = sqlparser::dialect::GenericDialect {};
    let stmts = match sqlparser::parser::Parser::parse_sql(&dialect, sql) {
        Ok(s) => s,
        Err(e) => return Source::Unreadable(vec![format!("the SQL does not parse: {e}")]),
    };
    let [Statement::Query(q)] = stmts.as_slice() else {
        return Source::Unreadable(vec!["more than one statement, or not a query".into()]);
    };
    if q.with.is_some() {
        return Source::Unreadable(vec!["a WITH clause".into()]);
    }
    let SetExpr::Select(s) = q.body.as_ref() else {
        return Source::Unreadable(vec!["a set operation (UNION, …)".into()]);
    };
    let mut tables = vec![];
    let mut atoms = vec![];
    for f in &s.from {
        if !f.joins.is_empty() {
            atoms.push(format!("a join of {} table(s)", f.joins.len() + 1));
        }
        for t in std::iter::once(&f.relation).chain(f.joins.iter().map(|j| &j.relation)) {
            match t {
                TableFactor::Table { name, .. } => tables.push(name_parts(name)),
                other => atoms.push(format!("a derived table: {other}")),
            }
        }
    }
    let outside: Vec<String> = tables
        .iter()
        .filter(|p| {
            let schema = p.iter().rev().nth(1).map(String::as_str).unwrap_or("");
            schema != gold_schema && !schema.starts_with("silver")
        })
        .map(|p| p.join("."))
        .collect();
    if !outside.is_empty() {
        return Source::Outside(outside);
    }
    if tables.len() != 1 {
        atoms.push(format!("{} tables", tables.len()));
    }
    let Some(mart) = tables
        .first()
        .filter(|p| p.iter().rev().nth(1).map(String::as_str) == Some(gold_schema))
        .and_then(|p| p.last().cloned())
        .filter(|m| marts.contains(m))
    else {
        atoms.push(format!(
            "reads {} — not one of the gold marts",
            tables
                .iter()
                .map(|p| p.join("."))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        return Source::Unreadable(atoms);
    };
    let mut p = Parsed::default();
    for item in &s.projection {
        let (e, alias) = match item {
            SelectItem::UnnamedExpr(e) => (e, None),
            SelectItem::ExprWithAlias { expr, alias } => {
                (expr, Some(alias.value.to_ascii_lowercase()))
            }
            other => {
                p.atoms.push(format!("the select item `{other}`"));
                continue;
            }
        };
        if let Some(c) = ident_of(e) {
            p.columns
                .push((alias.unwrap_or_else(|| c.clone()), c, None));
        } else if let Some((func, col)) = aggregate(e) {
            let out =
                alias.unwrap_or_else(|| format!("{func}_{}", col.clone().unwrap_or("rows".into())));
            match col {
                Some(c) => p.columns.push((out, c, Some(func))),
                None => p.atoms.push(format!(
                    "`{e}`: counts rows of the mart, not a measure of it"
                )),
            }
        } else {
            p.atoms.push(format!("the expression `{e}`"));
        }
    }
    if let Some(w) = &s.selection {
        let mut parts = vec![];
        conjuncts(w, &mut parts);
        for c in parts {
            match c {
                Expr::BinaryOp { left, op, right } => {
                    match (
                        ident_of(left),
                        op_text(op),
                        match right.as_ref() {
                            Expr::Value(v) => literal(v),
                            _ => None,
                        },
                    ) {
                        (Some(col), Some(o), Some(l)) => p.filters.push((col, o.into(), l)),
                        _ => p.atoms.push(format!("the condition `{c}`")),
                    }
                }
                other => p.atoms.push(format!("the condition `{other}`")),
            }
        }
    }
    if s.having.is_some() {
        p.atoms.push("a HAVING clause".into());
    }
    if let GroupByExpr::Expressions(g, _) = &s.group_by {
        for e in g {
            if ident_of(e).is_none() && !matches!(e, Expr::Value(SqlValue::Number(..))) {
                p.atoms.push(format!("the grouping `{e}`"));
            }
        }
    }
    p.atoms.extend(atoms);
    Source::Mart { mart, sql: Some(p) }
}

fn viz_back(viz_type: &str) -> Option<&'static str> {
    Some(match viz_type {
        "echarts_timeseries_bar" | "dist_bar" | "bar" => "bar",
        "echarts_timeseries_line" | "line" | "echarts_timeseries" => "line",
        "pie" => "pie",
        "big_number_total" => "big_number",
        "table" => "table",
        _ => return None,
    })
}

/// A mart column's term: its definition, and whether it is a dimension or an additive measure.
fn column_term(m: &dash::Mart, col: &str) -> Option<(&'static str, String, Option<Term>)> {
    let lower = col.to_ascii_lowercase();
    for r in &m.resolved {
        if r.term.to_ascii_lowercase() == lower
            && (r.section == "dimensions" || r.section == "measures")
        {
            return Some((r.section, r.written.clone(), r.resolution.clone().ok()));
        }
    }
    None
}

fn col_ref_text(c: &ColRef) -> String {
    format!("{}.{}", c.table, c.column)
}

/// One chart, adopted or with what was not understood.
struct ChartRead {
    entry: Option<BundleChart>,
    atoms: Vec<String>,
}

struct Builder<'a> {
    mart: &'a dash::Mart,
    measures: Vec<NamedTerm>,
    dimensions: Vec<NamedTerm>,
    filters: Vec<String>,
    atoms: Vec<String>,
}

impl Builder<'_> {
    /// A column of the dataset as the chart uses it: a dimension, or an additive measure.
    fn dimension(&mut self, out_col: &str, mart_col: &str) -> Option<String> {
        match column_term(self.mart, mart_col) {
            Some(("dimensions", def, _)) => {
                if !self.dimensions.iter().any(|d| d.name == out_col) {
                    self.dimensions.push(NamedTerm {
                        name: out_col.into(),
                        definition: def,
                    });
                }
                Some(out_col.to_string())
            }
            _ => {
                self.atoms.push(format!(
                    "`{out_col}` is not a dimension of {}",
                    self.mart.spec.mart
                ));
                None
            }
        }
    }

    fn measure(&mut self, out_col: &str, mart_col: &str, agg: &str) -> Option<String> {
        match column_term(self.mart, mart_col) {
            Some(("measures", def, Some(t)))
                if matches!(&t, Term::Aggregate { func, distinct: false, .. } if func == "sum" || func == "count")
                    && agg == "sum" =>
            {
                if !self.measures.iter().any(|d| d.name == out_col) {
                    self.measures.push(NamedTerm {
                        name: out_col.into(),
                        definition: def,
                    });
                }
                Some(out_col.to_string())
            }
            Some(("measures", _, _)) => {
                self.atoms.push(format!(
                    "`{agg}` of {mart_col}: only a sum of a sum or a count is a measure of {}",
                    self.mart.spec.mart
                ));
                None
            }
            _ => {
                self.atoms.push(format!(
                    "`{mart_col}` is not a measure of {}",
                    self.mart.spec.mart
                ));
                None
            }
        }
    }

    fn filter(&mut self, mart_col: &str, op: &str, lit: &str) {
        match column_term(self.mart, mart_col) {
            Some(("dimensions", _, Some(Term::Column(c)))) => {
                self.filters
                    .push(format!("{} {op} {lit}", col_ref_text(&c)));
            }
            _ => self.atoms.push(format!(
                "the filter on `{mart_col}` (not a plain column dimension of {})",
                self.mart.spec.mart
            )),
        }
    }
}

fn metric_of(
    b: &mut Builder<'_>,
    metric: &Value,
    dataset: &[(String, String, Option<String>)],
) -> Option<String> {
    if metric.is_string() {
        b.atoms.push(format!("the saved metric {metric}"));
        return None;
    }
    if metric["expressionType"] != json!("SIMPLE") {
        b.atoms.push(format!(
            "the SQL metric `{}`",
            metric["sqlExpression"].as_str().unwrap_or_default()
        ));
        return None;
    }
    let col = metric["column"]["column_name"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let agg = metric["aggregate"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some((_, mart_col, pre)) = dataset.iter().find(|(o, _, _)| *o == col) else {
        b.atoms
            .push(format!("the metric's column `{col}` is not in the dataset"));
        return None;
    };
    // A dataset column that is already `sum(x)` summed again is still `x`'s sum; a count summed
    // is still the count.
    let agg = if agg == "sum" && pre.is_some() {
        "sum".to_string()
    } else {
        agg
    };
    let (mart_col, out) = (mart_col.clone(), col.clone());
    b.measure(&out, &mart_col, &agg)
}

fn read_chart(
    cx: &Ctx<'_>,
    marts: &[dash::Mart],
    chart: &Value,
    dataset: &Value,
) -> anyhow::Result<(ChartRead, Option<Vec<String>>)> {
    let title = chart["slice_name"].as_str().unwrap_or_default().to_string();
    let fd = &chart["form_data"];
    let names: Vec<String> = marts.iter().map(|m| m.spec.mart.clone()).collect();
    let gold = cx.target.gold_schema.to_ascii_lowercase();
    let source = match dataset["sql"].as_str().filter(|s| !s.trim().is_empty()) {
        Some(sql) => read_sql(sql, &gold, &names),
        None => {
            let schema = dataset["schema"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let table = dataset["table_name"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if schema == gold && names.contains(&table) {
                Source::Mart {
                    mart: table,
                    sql: None,
                }
            } else if schema.starts_with("silver") {
                Source::Unreadable(vec![format!(
                    "reads silver table {schema}.{table} directly"
                )])
            } else {
                Source::Outside(vec![format!("{schema}.{table}")])
            }
        }
    };
    let (mart_name, parsed) = match source {
        Source::Outside(t) => {
            return Ok((
                ChartRead {
                    entry: None,
                    atoms: vec![],
                },
                Some(t),
            ));
        }
        Source::Unreadable(a) => {
            return Ok((
                ChartRead {
                    entry: None,
                    atoms: a,
                },
                None,
            ));
        }
        Source::Mart { mart, sql } => (mart, sql),
    };
    let mart = marts
        .iter()
        .find(|m| m.spec.mart == mart_name)
        .expect("named from marts");
    let mut b = Builder {
        mart,
        measures: vec![],
        dimensions: vec![],
        filters: vec![],
        atoms: vec![],
    };
    // The dataset's own columns: each output column and the mart column it is.
    let dataset_cols: Vec<(String, String, Option<String>)> = match &parsed {
        Some(p) => {
            b.atoms.extend(p.atoms.iter().cloned());
            for (c, op, l) in &p.filters {
                b.filter(c, op, l);
            }
            p.columns.clone()
        }
        None => mart
            .spec
            .dimensions
            .iter()
            .chain(mart.spec.measures.iter())
            .map(|(n, _)| (n.to_ascii_lowercase(), n.to_ascii_lowercase(), None))
            .collect(),
    };
    let dim = |b: &mut Builder<'_>, name: &Value| -> Option<String> {
        let n = name.as_str()?.to_ascii_lowercase();
        match dataset_cols.iter().find(|(o, _, _)| *o == n) {
            Some((o, mc, None)) => b.dimension(o, mc),
            _ => {
                b.atoms
                    .push(format!("`{n}` is not a dimension column of the dataset"));
                None
            }
        }
    };
    let viz_type = fd["viz_type"]
        .as_str()
        .or(chart["viz_type"].as_str())
        .unwrap_or_default();
    let Some(viz) = viz_back(viz_type) else {
        b.atoms.push(format!("the chart type `{viz_type}`"));
        return Ok((
            ChartRead {
                entry: None,
                atoms: b.atoms,
            },
            None,
        ));
    };
    let mut x = None;
    let mut group_by = None;
    let mut metric = None;
    match viz {
        "table" => {
            for c in fd["all_columns"].as_array().into_iter().flatten() {
                let n = c.as_str().unwrap_or_default().to_ascii_lowercase();
                match dataset_cols.iter().find(|(o, _, _)| *o == n).cloned() {
                    Some((o, mc, None)) => match column_term(mart, &mc) {
                        Some(("measures", _, _)) => {
                            metric = metric.or(b.measure(&o, &mc, "sum"));
                        }
                        _ => {
                            b.dimension(&o, &mc);
                        }
                    },
                    Some((o, mc, Some(_))) => {
                        metric = metric.or(b.measure(&o, &mc, "sum"));
                    }
                    None => b.atoms.push(format!("the column `{n}`")),
                }
            }
        }
        "big_number" => metric = metric_of(&mut b, &fd["metric"], &dataset_cols),
        "pie" => {
            x = fd["groupby"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|g| dim(&mut b, g));
            metric = metric_of(&mut b, &fd["metric"], &dataset_cols);
        }
        _ => {
            x = dim(&mut b, &fd["x_axis"]);
            for g in fd["groupby"].as_array().into_iter().flatten() {
                group_by = dim(&mut b, g).or(group_by);
            }
            for m in fd["metrics"].as_array().into_iter().flatten() {
                metric = metric_of(&mut b, m, &dataset_cols).or(metric);
            }
        }
    }
    for f in fd["adhoc_filters"].as_array().into_iter().flatten() {
        let col = f["subject"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let op = f["operator"].as_str().unwrap_or_default();
        let op = match op {
            "==" => "=",
            "!=" => "<>",
            o @ ("<" | "<=" | ">" | ">=") => o,
            other => {
                b.atoms.push(format!("the chart filter `{col} {other}`"));
                continue;
            }
        };
        let lit = match &f["comparator"] {
            Value::String(s)
                if s.chars()
                    .all(|c| c.is_alphanumeric() || " _.-/:&()".contains(c)) =>
            {
                format!("'{s}'")
            }
            Value::Number(n) => n.to_string(),
            other => {
                b.atoms.push(format!("the chart filter value {other}"));
                continue;
            }
        };
        if f["expressionType"] != json!("SIMPLE") {
            b.atoms.push("a custom SQL chart filter".into());
            continue;
        }
        match dataset_cols.iter().find(|(o, _, _)| *o == col) {
            Some((_, mc, None)) => {
                let mc = mc.clone();
                b.filter(&mc, op, &lit);
            }
            _ => b.atoms.push(format!("the chart filter on `{col}`")),
        }
    }
    if b.measures.is_empty() && b.atoms.is_empty() {
        b.atoms
            .push("the chart shows no measure EKOS can name".into());
    }
    if !b.atoms.is_empty() {
        return Ok((
            ChartRead {
                entry: None,
                atoms: b.atoms,
            },
            None,
        ));
    }
    let request = ChartRequest {
        measures: b.measures,
        dimensions: b.dimensions,
        filters: b.filters,
        period: None,
    };
    let presentation = Presentation {
        title: title.clone(),
        viz: viz.into(),
        x,
        group_by,
        metric,
        order: 0,
        number_format: fd["y_axis_format"]
            .as_str()
            .or(fd["number_format"].as_str())
            .map(str::to_string)
            .filter(|f| f != "SMART_NUMBER"),
        color_scheme: fd["color_scheme"].as_str().map(str::to_string),
    };
    let served = match dash::compile_chart(
        &request,
        &cx.plan,
        marts,
        &cx.target,
        Some(&mart_name),
        true,
    ) {
        Ok(s) => s,
        Err(u) => {
            return Ok((
                ChartRead {
                    entry: None,
                    atoms: vec![format!("its terms do not compile over {mart_name}: {u:?}")],
                },
                None,
            ));
        }
    };
    let request = dash::with_inherited(&request, &served);
    if let Err(e) = presentation.check(&request) {
        return Ok((
            ChartRead {
                entry: None,
                atoms: vec![e],
            },
            None,
        ));
    }
    let chart_id = chart["id"].as_i64().unwrap_or_default();
    let dataset_id = dataset["id"].as_i64().unwrap_or_default();
    Ok((
        ChartRead {
            entry: Some(BundleChart {
                key: format!("c{chart_id}"),
                presentation,
                request,
                mart: mart_name.clone(),
                marts: vec![MartDigest {
                    mart: mart_name,
                    digest: served.digest,
                }],
                terms: served.terms,
                // The dataset as it is in Superset: adoption changes nothing there.
                sql: dataset["sql"].as_str().unwrap_or_default().to_string(),
                reconciled: None,
                superset: Some(SupersetChart {
                    chart_id,
                    dataset_id,
                    dataset: dataset["table_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                }),
                params: chart["params"]
                    .as_str()
                    .and_then(|p| serde_json::from_str(p).ok()),
            }),
            atoms: vec![],
        },
        None,
    ))
}

/// `lakehouse dashboards adopt --superset-id <id>`.
pub fn adopt(
    cx: &Ctx<'_>,
    superset_id: i64,
    key: Option<&str>,
    user: &str,
    dry_run: bool,
) -> anyhow::Result<Value> {
    for k in keys(&cx.repo) {
        if let Ok(b) = bundle_now(&cx.repo, &k)
            && b.superset
                .as_ref()
                .is_some_and(|s| s.dashboard_id == superset_id)
        {
            anyhow::bail!("Superset dashboard {superset_id} already has a bundle: {k}");
        }
    }
    let d = cx.sup.read(&format!("/api/v1/dashboard/{superset_id}"))?["result"].clone();
    let charts = cx
        .sup
        .read(&format!("/api/v1/dashboard/{superset_id}/charts"))?["result"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let datasets = cx
        .sup
        .read(&format!("/api/v1/dashboard/{superset_id}/datasets"))?["result"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let marts = cx.marts();
    let mut adopted = vec![];
    let mut unadopted = vec![];
    let mut outside = vec![];
    let mut sorted = charts.clone();
    sorted.sort_by_key(|c| c["id"].as_i64());
    for c in &sorted {
        // `/dashboard/<id>/charts` gives form data; `/chart/<id>` the saved params (for drift).
        let id = c["id"].as_i64().unwrap_or_default();
        let full = cx.sup.read(&format!("/api/v1/chart/{id}"))?["result"].clone();
        let mut chart = c.clone();
        chart["params"] = full["params"].clone();
        let ds_id = c["form_data"]["datasource"]
            .as_str()
            .and_then(|d| d.split("__").next())
            .and_then(|n| n.parse::<i64>().ok());
        let Some(ds) = ds_id.and_then(|i| datasets.iter().find(|d| d["id"].as_i64() == Some(i)))
        else {
            unadopted.push(Unadopted {
                chart_id: id,
                title: c["slice_name"].as_str().unwrap_or_default().into(),
                atoms: vec!["its dataset is not among the dashboard's".into()],
            });
            continue;
        };
        let (r, out) = read_chart(cx, &marts, &chart, ds)?;
        if let Some(o) = out {
            outside.extend(o);
            continue;
        }
        match r.entry {
            Some(e) => adopted.push(e),
            None => unadopted.push(Unadopted {
                chart_id: id,
                title: c["slice_name"].as_str().unwrap_or_default().into(),
                atoms: r.atoms,
            }),
        }
    }
    if !outside.is_empty() {
        outside.sort();
        outside.dedup();
        return Ok(json!({
            "status": "refused",
            "reason": format!("the dashboard reads tables outside gold and silver: {}", outside.join(", ")),
            "outside": outside,
        }));
    }
    let total = adopted.len() + unadopted.len();
    let share = if total == 0 {
        0.0
    } else {
        adopted.len() as f64 / total as f64
    };
    let questions: Vec<Value> = unadopted
        .iter()
        .map(|u| json!({"chart": u.title, "chart_id": u.chart_id,
                        "questions": u.atoms.iter().map(|a| format!("What does {a} mean in terms of the gold marts?")).collect::<Vec<_>>(),
                        "atoms": u.atoms}))
        .collect();
    let key = key
        .map(str::to_string)
        .unwrap_or_else(|| format!("adopted-{superset_id}"));
    anyhow::ensure!(dash::valid_key(&key), "{key:?} is not a dashboard key");
    let mut out = json!({
        "dashboard": key, "superset_id": superset_id,
        "adopted": adopted.iter().map(|c| json!({"chart": c.presentation.title, "chart_id": c.superset.as_ref().map(|s| s.chart_id), "mart": c.mart, "terms": c.terms})).collect::<Vec<_>>(),
        "not_adopted": questions,
        "share": format!("{}/{total} ({:.0}%)", adopted.len(), share * 100.0),
        "superset_changed": false,
    });
    if adopted.is_empty() {
        out["status"] = json!("not adopted");
        out["reason"] = json!("no chart could be read as terms over the gold marts");
        return Ok(out);
    }
    let b = Bundle {
        format: dash::BUNDLE_FORMAT,
        dashboard: key.clone(),
        title: d["dashboard_title"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        author: user.to_string(),
        request_text: format!("adopted from Superset dashboard {superset_id}"),
        superset: Some(SupersetDashboard {
            dashboard_id: superset_id,
            url: d["url"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("/superset/dashboard/{superset_id}/")),
        }),
        origin: None,
        charts: adopted,
        unadopted,
    };
    if dry_run {
        out["status"] = json!("dry run: nothing written");
        return Ok(out);
    }
    let path = cx.repo.join(Bundle::path(&key));
    std::fs::create_dir_all(path.parent().expect("dashboards/"))?;
    std::fs::write(&path, b.to_yaml())?;
    let rel = Bundle::path(&key);
    let git = |args: &[&str]| -> anyhow::Result<String> {
        let o = std::process::Command::new("git")
            .arg("-C")
            .arg(&cx.repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", user)
            .env("GIT_COMMITTER_NAME", user)
            .env("GIT_AUTHOR_EMAIL", format!("{user}@ekos.local"))
            .env("GIT_COMMITTER_EMAIL", format!("{user}@ekos.local"))
            .output()?;
        anyhow::ensure!(
            o.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    git(&["add", "--", &rel])?;
    git(&[
        "commit",
        "-q",
        "-m",
        &format!("lakehouse: adopt Superset dashboard {superset_id} as {key} (by {user})"),
        "--",
        &rel,
    ])?;
    out["version"] = json!(git(&["rev-parse", "HEAD"])?);
    out["status"] = json!(if b.unadopted.is_empty() {
        "adopted"
    } else {
        "partly adopted"
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARTS: &[&str] = &["sales_by_month"];

    fn marts() -> Vec<String> {
        MARTS.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_plain_query_over_one_mart_is_read() {
        let s = read_sql(
            "select month, sum(total) as total from iceberg.gold_sales.sales_by_month \
             where customer <> 'Internal A' group by month",
            "gold_sales",
            &marts(),
        );
        let Source::Mart { mart, sql: Some(p) } = s else {
            panic!("{s:?}")
        };
        assert_eq!(mart, "sales_by_month");
        assert!(p.atoms.is_empty(), "{:?}", p.atoms);
        assert_eq!(
            p.columns[1],
            ("total".into(), "total".into(), Some("sum".into()))
        );
        assert_eq!(
            p.filters,
            [("customer".into(), "<>".into(), "'Internal A'".into())]
        );
    }

    #[test]
    fn custom_sql_is_reported_atom_by_atom() {
        let s = read_sql(
            "select month, sum(total) / nullif(sum(orders), 0) as avg_order, upper(customer) as c \
             from gold_sales.sales_by_month where customer like '%Internal%' group by month, upper(customer)",
            "gold_sales",
            &marts(),
        );
        let Source::Mart { sql: Some(p), .. } = s else {
            panic!("{s:?}")
        };
        let atoms = p.atoms.join(" | ");
        assert!(atoms.contains("nullif"), "{atoms}");
        assert!(
            atoms.contains("UPPER(customer)") || atoms.contains("upper(customer)"),
            "{atoms}"
        );
        assert!(atoms.contains("LIKE"), "{atoms}");
    }

    #[test]
    fn a_table_outside_gold_and_silver_refuses_and_a_join_is_not_one_mart() {
        assert_eq!(
            read_sql("select * from adempiere.c_order", "gold_sales", &marts()),
            Source::Outside(vec!["adempiere.c_order".into()])
        );
        let j = read_sql(
            "select a.month from gold_sales.sales_by_month a join gold_sales.invoices_by_month b on a.month = b.month",
            "gold_sales",
            &["sales_by_month".into(), "invoices_by_month".into()],
        );
        let atoms = match &j {
            Source::Unreadable(a) => a.clone(),
            Source::Mart { sql: Some(p), .. } => p.atoms.clone(),
            other => panic!("{other:?}"),
        };
        assert!(atoms.iter().any(|x| x.contains("join")), "{j:?}");
    }
}
