//! `lakehouse gold ask <request.json>` (EKOS RFC 0248): triage a question's terms against the
//! deployed plan and the marts that exist, and — when the request resolves — show the SQL it
//! compiles to and the anti-invention verdict. Read-only: nothing is written, nothing is built.
//!
//! The request is the structured answer a model gives for a question (terms only, no SQL); this
//! command is also how a person checks one by hand.

use crate::plan_cmd::read_lock;
use ekos_lakehouse_gold::ask::{self, AskRequest, Triage};
use ekos_lakehouse_gold::{self as gold, MartSpec};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// The marts the pipeline repository already describes: `marts/*.md`, in file-name order. A file
/// that does not parse is reported, not hidden — a mart that cannot be read cannot be matched.
pub fn existing_marts(repo: &Path) -> (Vec<MartSpec>, Vec<String>) {
    let mut specs = Vec::new();
    let mut unreadable = Vec::new();
    let Ok(dir) = std::fs::read_dir(repo.join("marts")) else {
        return (specs, unreadable);
    };
    let mut files: Vec<_> = dir
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    files.sort();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&f)
            .map_err(|e| e.to_string())
            .and_then(|t| gold::parse(&t))
        {
            Ok(s) => specs.push(s),
            Err(e) => unreadable.push(format!("{name}: {e}")),
        }
    }
    (specs, unreadable)
}

/// The report for one request: the triage, and the SQL when the terms resolve.
pub fn report(repo: &Path, request_json: &str) -> anyhow::Result<Value> {
    let req: AskRequest = serde_json::from_str(request_json)
        .map_err(|e| anyhow::anyhow!("not a valid request (terms only, no SQL): {e}"))?;
    let plan = read_lock(repo)?;
    let (marts, unreadable) = existing_marts(repo);
    let triage = ask::triage(&req, &plan, &marts).map_err(|e| anyhow::anyhow!(e))?;

    let answerable = matches!(
        triage,
        Triage::PossibleNow { .. }
            | Triage::PossibleWithAddition { .. }
            | Triage::PossibleWithChange { .. }
    );
    let mut out = json!({ "triage": triage, "unreadable_marts": unreadable });
    if answerable {
        let spec = req.to_spec();
        let resolved = gold::resolve(&spec, &plan);
        match gold::compile(&spec, &plan, &resolved, &BTreeMap::new()) {
            Ok(c) => {
                let verdict = gold::check(&c);
                out["sql"] = json!(c.plain_sql);
                out["mart"] = json!(spec.mart);
                out["invention"] = json!({
                    "accepted": verdict.accepted(),
                    "checked_atoms": verdict.checked,
                    "unmapped": verdict.unmapped.len(),
                    "parse_error": verdict.parse_error,
                });
            }
            Err(qs) => out["questions"] = json!(qs),
        }
    }
    Ok(out)
}

pub fn run(repo: &Path, request_file: &Path, json_out: bool) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(request_file)
        .map_err(|e| anyhow::anyhow!("{}: {e}", request_file.display()))?;
    let r = report(repo, &text)?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }
    let status = r["triage"]["status"].as_str().unwrap_or("?");
    println!("status: {status}");
    match status {
        "possible_now" => println!("answered by the existing mart {}", r["triage"]["mart"]),
        "possible_with_addition" => match r["triage"]["extend"].as_str() {
            Some(m) => println!(
                "add measures to {m} (nothing existing changes), or build {}",
                r["triage"]["mart"]
            ),
            None => println!(
                "a new mart, {}; nothing existing changes",
                r["triage"]["mart"]
            ),
        },
        "possible_with_change" => {
            println!("answering it changes {}:", r["triage"]["mart"]);
            for c in r["triage"]["changes"].as_array().into_iter().flatten() {
                println!("  - {c}");
            }
        }
        "needs_clarification" => {
            for q in r["triage"]["questions"].as_array().into_iter().flatten() {
                println!("? {}", q["text"].as_str().unwrap_or(""));
                if let Some(c) = q["candidates"].as_array().filter(|c| !c.is_empty()) {
                    println!(
                        "  one of: {}",
                        c.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        "impossible" => {
            for reason in r["triage"]["reasons"].as_array().into_iter().flatten() {
                println!("- {}", reason.as_str().unwrap_or(""));
            }
        }
        _ => {}
    }
    if let Some(sql) = r["sql"].as_str() {
        println!("\n{sql}\n");
        let inv = &r["invention"];
        println!(
            "anti-invention: {} ({} atoms checked)",
            if inv["accepted"] == json!(true) {
                "accepted"
            } else {
                "REJECTED"
            },
            inv["checked_atoms"]
        );
    }
    for u in r["unreadable_marts"].as_array().into_iter().flatten() {
        println!(
            "note: could not read a mart description: {}",
            u.as_str().unwrap_or("")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
    use ekos_lakehouse_bronze::{ForeignKey, SourceColumn, SourceTable, TypeKind};

    fn col(n: &str, pg: &str, trino: &str) -> SourceColumn {
        SourceColumn {
            name: n.into(),
            pg_type: pg.into(),
            kind: TypeKind::Base,
            not_null: false,
            trino_type: Some(trino.into()),
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 0,
            has_default: false,
        }
    }

    fn table(name: &str, cols: Vec<SourceColumn>, fk: Option<(&str, &str)>) -> SourceTable {
        SourceTable {
            schema: "public".into(),
            oid: 0,
            name: name.into(),
            columns: cols,
            primary_key: vec!["id".into()],
            row_estimate: 10,
            update_triggers: vec![],
            foreign_keys: fk
                .into_iter()
                .map(|(c, t)| ForeignKey {
                    name: format!("{name}_{c}_fkey"),
                    columns: vec![c.into()],
                    ref_schema: "public".into(),
                    ref_table: t.into(),
                    ref_columns: vec!["id".into()],
                })
                .collect(),
            comment: None,
        }
    }

    /// A pipeline repository with a deployed plan and one described mart.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = plan(
            &[
                table(
                    "orders",
                    vec![
                        col("id", "integer", "integer"),
                        col("customer_id", "integer", "integer"),
                        col("total", "numeric(20,2)", "decimal(20,2)"),
                    ],
                    Some(("customer_id", "customers")),
                ),
                table(
                    "customers",
                    vec![
                        col("id", "integer", "integer"),
                        col("name", "text", "varchar"),
                    ],
                    None,
                ),
            ],
            &PlanSettings::default(),
        );
        std::fs::write(
            dir.path().join("lakehouse.lock.json"),
            serde_json::to_string(&p).unwrap(),
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("marts")).unwrap();
        std::fs::write(
            dir.path().join("marts/revenue_by_customer.md"),
            "---\nmart: revenue_by_customer\ngrain: [customer]\ndimensions:\n  customer: customers.name\nmeasures:\n  revenue: sum of orders.total\n---\nRevenue per customer.\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("marts/broken.md"), "no header").unwrap();
        dir
    }

    const REQUEST: &str = r#"{"measures":[{"name":"revenue","definition":"sum of orders.total"}],
        "dimensions":[{"name":"customer","definition":"customers.name"}]}"#;

    #[test]
    fn an_existing_mart_answers_and_an_unreadable_one_is_reported() {
        let dir = repo();
        let r = report(dir.path(), REQUEST).unwrap();
        assert_eq!(r["triage"]["status"], "possible_now");
        assert_eq!(r["triage"]["mart"], "revenue_by_customer");
        assert_eq!(r["invention"]["accepted"], true, "{r}");
        assert!(
            r["sql"].as_str().unwrap().to_lowercase().contains("sum("),
            "{r}"
        );
        let unreadable = r["unreadable_marts"].as_array().unwrap();
        assert_eq!(unreadable.len(), 1);
        assert!(unreadable[0].as_str().unwrap().starts_with("broken.md"));
    }

    #[test]
    fn a_new_measure_is_an_addition_and_still_compiles_without_invention() {
        let dir = repo();
        let req = r#"{"measures":[{"name":"orders","definition":"count of orders"}],
            "dimensions":[{"name":"customer","definition":"customers.name"}]}"#;
        let r = report(dir.path(), req).unwrap();
        assert_eq!(r["triage"]["status"], "possible_with_addition");
        assert_eq!(r["triage"]["extend"], "revenue_by_customer");
        assert_eq!(r["invention"]["accepted"], true, "{r}");
    }

    #[test]
    fn an_unknown_thing_is_impossible_and_returns_no_sql() {
        let dir = repo();
        let req = r#"{"measures":[{"name":"x","definition":"sum of weather.forecast"}]}"#;
        let r = report(dir.path(), req).unwrap();
        assert_eq!(r["triage"]["status"], "impossible");
        assert!(r.get("sql").is_none());
    }

    #[test]
    fn sql_in_a_request_is_refused_before_anything_reads_the_plan() {
        let dir = repo();
        let bad = r#"{"measures":[{"name":"x","definition":"1; drop table orders"}]}"#;
        assert!(
            report(dir.path(), bad)
                .unwrap_err()
                .to_string()
                .contains("may not hold")
        );
        let extra =
            r#"{"measures":[{"name":"x","definition":"count of orders"}],"sql":"select 1"}"#;
        assert!(
            report(dir.path(), extra)
                .unwrap_err()
                .to_string()
                .contains("terms only")
        );
        // No plan deployed: say so, do not guess.
        let empty = tempfile::tempdir().unwrap();
        assert!(
            report(empty.path(), REQUEST)
                .unwrap_err()
                .to_string()
                .contains("lakehouse generate")
        );
    }
}
