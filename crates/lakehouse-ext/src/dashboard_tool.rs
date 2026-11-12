//! MCP `ekos_dashboard_compile` (EKOS RFC 0248 §2.1/§2.4): what the Superset fork's "Ask your
//! data" calls to publish a dashboard from **terms**. Without charts it returns the vocabulary —
//! each gold mart's dimensions, measures and filters as terms; with charts (terms only, no SQL) it
//! returns, per chart, the SQL compiled over the gold marts, the resolved terms with the rule
//! that applied, the mart's digest and the two queries the chart is reconciled with. Read-only:
//! it reads the pipeline repository's plan and mart descriptions; it publishes nothing.

use crate::dashboard_change::NewChart;
use ekos_lakehouse_gold::dashboard::{self as dash, Target, Unserved};
use serde_json::{Value, json};
use std::path::Path;

pub const NAME: &str = "ekos_dashboard_compile";
pub const DESCRIPTION: &str = "EKOS Lakehouse (RFC 0248): compile dashboard charts from terms. \
Call it without `charts` for the vocabulary (each gold mart's dimensions, measures and filters as \
terms); call it with `charts` (each a presentation and a request of measures, dimensions, filters \
and an optional period — terms only, never SQL) for each chart's SQL over the gold marts, its \
resolved terms with the rule that applied, the mart's digest and the chart/silver queries that \
must agree before the chart is published. A term that does not resolve is returned as a question. \
Read-only: it publishes nothing.";

pub fn schema() -> Value {
    json!({"type": "object", "properties": {
        "charts": {"type": "array", "maxItems": 12, "items": {"type": "object",
            "required": ["presentation", "request"],
            "properties": {
                "presentation": {"type": "object", "description": "title, viz (bar|line|pie|big_number|table), x, group_by, metric, order, number_format, color_scheme"},
                "request": {"type": "object", "description": "measures [{name, definition}], dimensions [{name, definition}], filters [text], period {dimension, from, to}"}
            }}}
    }})
}

pub fn answer(repo: &Path, args: &Value) -> anyhow::Result<Value> {
    let plan = crate::plan_cmd::read_lock(repo)?;
    let (specs, unreadable) = crate::ask_cmd::existing_marts(repo);
    let marts = dash::marts_of(&specs, &plan);
    let target = Target {
        catalog: plan.settings.target_catalog.clone(),
        gold_schema: ekos_lakehouse_bronze::plan::gold_schema(&plan.settings.namespace_prefix),
    };
    let Some(charts) = args.get("charts").and_then(Value::as_array) else {
        return Ok(json!({
            "marts": marts.iter().map(|m| json!({
                "mart": m.spec.mart, "grain": m.spec.grain,
                "dimensions": m.spec.dimensions.iter().map(|(n, d)| json!({"name": n, "definition": d})).collect::<Vec<_>>(),
                "measures": m.spec.measures.iter().map(|(n, d)| json!({"name": n, "definition": d})).collect::<Vec<_>>(),
                "filters": m.spec.filters, "digest": m.digest,
            })).collect::<Vec<_>>(),
            "gold": format!("{}.{}", target.catalog, target.gold_schema),
            "unreadable_marts": unreadable,
            "how": "A chart's request names measures and dimensions with these definitions (a \
                    measure of a finer mart is totalled); a filter on a dimension column narrows it.",
        }));
    };
    anyhow::ensure!(charts.len() <= 12, "at most 12 charts");
    let mut out = vec![];
    for (i, c) in charts.iter().enumerate() {
        let chart: NewChart = match serde_json::from_value(c.clone()) {
            Ok(x) => x,
            Err(e) => {
                out.push(json!({"chart": i + 1, "status": "refused", "reason": format!("not a chart of terms: {e}")}));
                continue;
            }
        };
        out.push(match dash::compile_chart(&chart.request, &plan, &marts, &target, None, true) {
            Ok(s) => {
                let req = dash::with_inherited(&chart.request, &s);
                if let Err(e) = chart.presentation.check(&req) {
                    json!({"chart": i + 1, "status": "refused", "reason": e})
                } else {
                    let checks = dash::reconcile_queries(&req, &s, &plan, &target.catalog)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    json!({
                        "chart": i + 1, "status": "ok", "mart": s.mart, "digest": s.digest,
                        "sql": s.sql, "request": req, "presentation": chart.presentation,
                        "terms": s.terms, "inherited_filters": s.inherited,
                        "reconcile": checks.iter().map(|(m, a, b)| json!({"measure": m, "chart_sql": a, "silver_sql": b})).collect::<Vec<_>>(),
                    })
                }
            }
            Err(Unserved::Questions(qs)) => json!({"chart": i + 1, "status": "needs_clarification", "questions": qs}),
            Err(Unserved::Invalid(e)) => json!({"chart": i + 1, "status": "refused", "reason": e}),
            Err(Unserved::NoMart(why)) => json!({"chart": i + 1, "status": "not_in_gold",
                "reason": "no gold mart serves it today (a mart addition or change is needed)",
                "marts": why.iter().map(|(m, w)| json!({"mart": m, "why": w})).collect::<Vec<_>>()}),
        });
    }
    Ok(json!({"charts": out, "gold": format!("{}.{}", target.catalog, target.gold_schema)}))
}
