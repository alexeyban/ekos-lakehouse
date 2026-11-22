//! The engine-neutral half of translating an ETL tool's jobs to dbt for the lakehouse (EKOS RFC
//! 0268, moved from `pentaho-dbt`): resolving a job's tables against the lakehouse plan, rewriting
//! PostgreSQL source SQL for Trino, and the checks every generated model goes through.

pub mod catalog;
pub mod draft;
pub mod pgsql;

use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_etl_expr::quote_ident as q;
use std::collections::BTreeMap;

/// A dbt-safe slug.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "x".into() } else { out }
}

/// A model's SQL as the engine runs it: config and comments dropped, every `ref()` replaced by
/// its physical table (silver source models, and the package's own models).
pub fn engine_sql(
    sql: &str,
    locations: &BTreeMap<String, (String, String)>,
    plan: &Plan,
) -> String {
    let mut out = String::new();
    let mut rest = sql;
    while let Some(i) = rest.find("{{ ref('") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "{{ ref('".len()..];
        let Some(j) = after.find("') }}") else { break };
        let model = &after[..j];
        let (schema, table) = match locations.get(model) {
            Some((s, a)) => (s.clone(), a.clone()),
            None => match plan
                .tables
                .iter()
                .find(|t| ekos_lakehouse_silver::model_name(t) == model)
            {
                Some(t) => (ekos_lakehouse_silver::silver_schema(t), t.table.clone()),
                None => ("unknown".into(), model.to_string()),
            },
        };
        out.push_str(&format!("{}.{}.{}", q("iceberg"), q(&schema), q(&table)));
        rest = &after[j + "') }}".len()..];
    }
    out.push_str(rest);
    out.lines()
        .filter(|l| !l.starts_with("--") && !l.starts_with("{{ config(") && !l.starts_with("{%"))
        .map(|l| {
            // Jinja inside a line (Update's incremental filter): keep the first-build branch.
            if let Some(i) = l.find("{% if is_incremental() %}") {
                let tail = l[i..]
                    .split("{% else %}")
                    .nth(1)
                    .unwrap_or("")
                    .replace("{% endif %}", "");
                format!("{}{}", &l[..i], tail)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `sql` parses with the Trino grammar.
pub fn parses_as_trino(sql: &str) -> Result<(), String> {
    use ekos_sql_dialect_sdk::SqlDialectParser;
    let trino = ekos_plugin_sql_dialect_trino::TrinoDialectParser;
    let pre = trino.preprocess(sql);
    sqlparser::parser::Parser::parse_sql(&*trino.sqlparser_dialect(), &pre)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A model's SQL that Trino can plan (and run) before any model of the package is built: every
/// `ref()` to a model of the package is inlined as a sub-query — a snapshot as its first build
/// would be (one open version per key) — and every other `ref()` is the silver table it names.
/// For `EXPLAIN`, and for running a translation against a sandbox without dbt.
pub fn plannable_sql(
    name: &str,
    files: &BTreeMap<String, String>,
    plan: &Plan,
) -> Result<String, String> {
    let by_name: BTreeMap<String, &String> = files
        .iter()
        .filter(|(p, _)| {
            p.ends_with(".sql")
                && (p.starts_with("dbt/models/")
                    || p.starts_with("dbt/snapshots/")
                    || p.starts_with("dbt/tests/"))
        })
        .map(|(p, body)| {
            (
                p.rsplit('/')
                    .next()
                    .unwrap_or(p)
                    .trim_end_matches(".sql")
                    .to_string(),
                body,
            )
        })
        .collect();
    fn body_of(sql: &str) -> String {
        let mut lines: Vec<&str> = sql
            .lines()
            .filter(|l| {
                !l.starts_with("--")
                    && !l.trim_start().starts_with("{{ config(")
                    && !l.starts_with("{% snapshot")
                    && !l.starts_with("{% endsnapshot")
            })
            .collect();
        while lines.first().is_some_and(|l| l.trim().is_empty()) {
            lines.remove(0);
        }
        lines.join("\n")
    }
    fn expand(
        sql: &str,
        by_name: &BTreeMap<String, &String>,
        plan: &Plan,
        depth: usize,
    ) -> Result<String, String> {
        if depth > 16 {
            return Err("models reference each other too deeply".into());
        }
        let mut out = String::new();
        let mut rest = sql;
        while let Some(i) = rest.find("{{ ref('") {
            out.push_str(&rest[..i]);
            let after = &rest[i + "{{ ref('".len()..];
            let j = after.find("') }}").ok_or("an unclosed ref")?;
            let model = &after[..j];
            match by_name.get(model) {
                Some(body) if body.contains("{% snapshot") => {
                    let inner = expand(&body_of(body), by_name, plan, depth + 1)?;
                    out.push_str(&format!(
                        "(SELECT x.*, x.ekos_key AS dbt_scd_id, TIMESTAMP '1900-01-01 00:00:00.000000' AS dbt_valid_from, CAST(NULL AS timestamp(6)) AS dbt_valid_to FROM ({inner}) AS x)"
                    ));
                }
                Some(body) => {
                    if body.contains("enabled=false") {
                        return Err(format!("it reads `{model}`, which is disabled"));
                    }
                    out.push_str(&format!(
                        "({})",
                        expand(&body_of(body), by_name, plan, depth + 1)?
                    ));
                }
                None => match plan
                    .tables
                    .iter()
                    .find(|t| ekos_lakehouse_silver::model_name(t) == model)
                {
                    Some(t) => out.push_str(&format!(
                        "{}.{}.{}",
                        q(&plan.settings.target_catalog),
                        q(&ekos_lakehouse_silver::silver_schema(t)),
                        q(&ekos_lakehouse_bronze::iceberg_name(&t.table))
                    )),
                    None => {
                        return Err(format!(
                            "`{model}` is neither a package model nor a silver model"
                        ));
                    }
                },
            }
            rest = &after[j + "') }}".len()..];
        }
        out.push_str(rest);
        Ok(out)
    }
    let body = by_name
        .get(name)
        .ok_or_else(|| format!("no model {name}"))?;
    let sql = expand(&body_of(body), &by_name, plan, 0)?;
    // Update's incremental filter: plan the first build.
    let sql = match (
        sql.find("{% if is_incremental() %}"),
        sql.find("{% else %}"),
    ) {
        (Some(i), Some(e)) => {
            let end = sql.find("{% endif %}").unwrap_or(sql.len());
            format!("{}{}", &sql[..i], &sql[e + "{% else %}".len()..end])
        }
        _ => sql,
    };
    Ok(sql)
}
