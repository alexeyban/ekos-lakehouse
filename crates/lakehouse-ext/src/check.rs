//! `lakehouse check` (RFC 0193 §6): the generated project, compiled back into the ledger by EKOS's
//! own dbt analyzer, and measured. Lineage comes from the ledger; test and documentation coverage
//! from the YAML actually in the repository (a person may have edited it); drift by regenerating
//! in memory and comparing; lint from `sqlfluff` when it is installed.

use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::render;
use ekos_lakehouse_silver::{MaskMethod, SilverSettings, model_name};
use ekos_ledger::KnowledgeStore;
use serde::Serialize;
use serde_yaml::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Metric {
    pub metric: &'static str,
    /// What was measured, out of what it could be.
    pub measured: usize,
    pub of: usize,
    pub passed: bool,
    /// How it was measured.
    pub method: String,
    /// What fell short, named.
    pub gaps: Vec<String>,
}

fn metric(
    name: &'static str,
    measured: usize,
    of: usize,
    method: &str,
    gaps: Vec<String>,
) -> Metric {
    Metric {
        metric: name,
        measured,
        of,
        passed: gaps.is_empty(),
        method: method.into(),
        gaps,
    }
}

/// Lineage: every silver column resolves to its bronze column in the compiled ledger, except
/// the masked ones, which are expected to be computed.
pub fn lineage(
    plan: &Plan,
    silver: &SilverSettings,
    store: &dyn KnowledgeStore,
) -> anyhow::Result<Metric> {
    let mut models: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for o in store.all_objects()? {
        let is_silver = o
            .properties
            .get("source_path")
            .and_then(|v| v.as_str())
            .is_some_and(|p| p.starts_with("models/silver/"));
        if is_silver {
            models.insert(
                o.name.clone(),
                o.properties
                    .get("column_lineage")
                    .cloned()
                    .unwrap_or_default(),
            );
        }
    }
    let (mut ok, mut of, mut gaps) = (0, 0, vec![]);
    for t in &plan.tables {
        let name = model_name(t);
        let Some(lineage) = models.get(&name) else {
            of += t.columns.len() + 1;
            gaps.push(format!(
                "{name}: not in the ledger (run the EKOS pipeline over the repository)"
            ));
            continue;
        };
        let expect = t
            .columns
            .iter()
            .map(|m| (m.column.clone(), m.column.clone()))
            .chain([(
                "_ekos_bronze_loaded_at".to_string(),
                "_ekos_loaded_at".to_string(),
            )]);
        for (out, src) in expect {
            let key = format!("{}.{}.{}", t.schema, t.table, out);
            if silver.pii.contains_key(&key) {
                continue; // computed by the mask, and expected to be
            }
            of += 1;
            match lineage.get(&out).and_then(|v| v.as_array()) {
                Some(pair)
                    if pair.first().and_then(|r| r.as_str()) == Some(t.table.as_str())
                        && pair.get(1).and_then(|c| c.as_str()) == Some(src.as_str()) =>
                {
                    ok += 1
                }
                other => gaps.push(format!(
                    "{name}.{out}: resolves to {}",
                    other.map_or("nothing".to_string(), |p| serde_json::Value::Array(
                        p.clone()
                    )
                    .to_string())
                )),
            }
        }
    }
    Ok(metric(
        "lineage",
        ok,
        of,
        "each silver column's column_lineage in the ledger (EKOS dbt analyzer) equals its bronze \
         column; masked columns excluded as computed",
        gaps,
    ))
}

fn tests_of(v: &Value) -> Vec<String> {
    v.get("data_tests")
        .and_then(Value::as_sequence)
        .map(|s| {
            s.iter()
                .filter_map(|t| match t {
                    Value::String(n) => Some(n.clone()),
                    Value::Mapping(m) => {
                        m.keys().next().and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Test and documentation coverage, from the model YAML in the repository.
pub fn coverage(plan: &Plan, silver: &SilverSettings, repo: &Path) -> anyhow::Result<Vec<Metric>> {
    let mut docs: BTreeMap<String, Value> = BTreeMap::new();
    let mut schemas: BTreeSet<&str> = BTreeSet::new();
    for t in &plan.tables {
        schemas.insert(&t.schema);
    }
    for schema in schemas {
        let path = repo.join(format!("dbt/models/silver/{schema}/_{schema}__models.yml"));
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let doc: Value = serde_yaml::from_str(&text)?;
        for m in doc
            .get("models")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
        {
            if let Some(n) = m.get("name").and_then(Value::as_str) {
                docs.insert(n.to_string(), m.clone());
            }
        }
    }
    let planned: BTreeSet<(String, String)> = plan
        .tables
        .iter()
        .map(|t| (t.schema.clone(), t.table.clone()))
        .collect();
    let (mut keys, mut keyed, mut key_gaps) = (0, 0, vec![]);
    let (mut nn, mut nn_of, mut nn_gaps) = (0, 0, vec![]);
    let (mut fk, mut fk_of, mut fk_gaps) = (0, 0, vec![]);
    let (mut md, mut cd, mut cols) = (0, 0, 0);
    for t in &plan.tables {
        let name = model_name(t);
        let empty = Value::Null;
        let model = docs.get(&name).unwrap_or(&empty);
        let columns: BTreeMap<String, &Value> = model
            .get("columns")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(|c| {
                c.get("name")
                    .and_then(Value::as_str)
                    .map(|n| (n.to_string(), c))
            })
            .collect();
        let col_tests = |c: &str| columns.get(c).map(|v| tests_of(v)).unwrap_or_default();
        let masked = |c: &str| silver.pii.get(&format!("{}.{}.{}", t.schema, t.table, c));

        if model.get("description").is_some() {
            md += 1;
        }
        for c in columns.values() {
            cols += 1;
            if c.get("description").is_some() {
                cd += 1;
            }
        }
        if !t.primary_key.is_empty() {
            keyed += 1;
            let covered = match t.primary_key.as_slice() {
                [one] => col_tests(one).iter().any(|x| x == "unique"),
                _ => tests_of(model)
                    .iter()
                    .any(|x| x == "ekos_unique_combination"),
            };
            if covered {
                keys += 1;
            } else {
                key_gaps.push(format!("{name}: no uniqueness test on its key"));
            }
        }
        for f in &t.facts.columns {
            if (f.not_null || t.primary_key.contains(&f.name))
                && masked(&f.name) != Some(&MaskMethod::Null)
            {
                nn_of += 1;
                if col_tests(&f.name).iter().any(|x| x == "not_null") {
                    nn += 1;
                } else {
                    nn_gaps.push(format!("{name}.{}: no not_null test", f.name));
                }
            }
        }
        for k in &t.facts.foreign_keys {
            if k.columns.len() == 1
                && planned.contains(&(k.ref_schema.clone(), k.ref_table.clone()))
                && masked(&k.columns[0]).is_none()
            {
                fk_of += 1;
                if col_tests(&k.columns[0])
                    .iter()
                    .any(|x| x == "relationships")
                {
                    fk += 1;
                } else {
                    fk_gaps.push(format!(
                        "{name}.{}: no relationships test ({})",
                        k.columns[0], k.name
                    ));
                }
            }
        }
    }
    let yaml = "the model YAML in dbt/models/silver/";
    Ok(vec![
        metric(
            "test_coverage.keys",
            keys,
            keyed,
            &format!("models with a key that test it unique, from {yaml}"),
            key_gaps,
        ),
        metric(
            "test_coverage.not_null",
            nn,
            nn_of,
            &format!("NOT NULL and key columns with a not_null test, from {yaml}"),
            nn_gaps,
        ),
        metric(
            "test_coverage.relationships",
            fk,
            fk_of,
            &format!(
                "single-column foreign keys to planned tables with a relationships test, from {yaml}"
            ),
            fk_gaps,
        ),
        // Documentation is reported, not gated: it can only come from the source's comments.
        Metric {
            metric: "documentation",
            measured: md + cd,
            of: plan.tables.len() + cols,
            passed: true,
            method: format!("models and columns with a description, from {yaml}"),
            gaps: vec![],
        },
    ])
}

/// Drift (RFC 0190): generated files a person has edited since the last `generate`, and
/// generated files that are out of date with the deployed plan. Reported, not a failure: the
/// edits are kept, and `generate` works around them.
pub fn drift(plan: &Plan, silver: &SilverSettings, repo: &Path) -> Metric {
    let mut files = render::files(plan);
    files.extend(ekos_lakehouse_silver::files(plan, silver).0);
    let edited = crate::plan_cmd::edited_files(repo);
    let mut gaps: Vec<String> = edited
        .iter()
        .map(|p| format!("{p}: edited by hand"))
        .collect();
    for (path, want) in &files {
        if edited.contains(path) {
            continue;
        }
        match std::fs::read_to_string(repo.join(path)) {
            Ok(have) if &have == want => {}
            Ok(_) => gaps.push(format!("{path}: out of date (run generate)")),
            Err(_) => gaps.push(format!("{path}: missing (run generate)")),
        }
    }
    let n = files.len();
    Metric {
        metric: "drift",
        measured: n.saturating_sub(gaps.len()),
        of: n,
        passed: true,
        method: "each generated file against the manifest of what generate wrote (edits) and \
                 against what the deployed plan generates now (staleness)"
            .into(),
        gaps,
    }
}

/// Lint with `sqlfluff` (the generated `.sqlfluff`), when it is on PATH.
pub fn lint(repo: &Path) -> Metric {
    let dbt = repo.join("dbt");
    let out = std::process::Command::new("sqlfluff")
        .args(["lint", "models", "macros", "tests", "--format", "json"])
        .current_dir(&dbt)
        .output();
    let method = "sqlfluff lint with the generated .sqlfluff (dialect trino, core rules)";
    let Ok(out) = out else {
        return Metric {
            metric: "lint",
            measured: 0,
            of: 0,
            passed: false,
            method: method.into(),
            gaps: vec!["sqlfluff is not installed here: not measured".into()],
        };
    };
    let files: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let mut gaps = vec![];
    for f in &files {
        let path = f["filepath"].as_str().unwrap_or("?");
        for v in f["violations"].as_array().into_iter().flatten() {
            gaps.push(format!(
                "{path}:{}: {} {}",
                v["start_line_no"],
                v["code"].as_str().unwrap_or(""),
                v["description"].as_str().unwrap_or("")
            ));
        }
    }
    let n = files.len();
    let bad: BTreeSet<&str> = files
        .iter()
        .filter(|f| f["violations"].as_array().is_some_and(|v| !v.is_empty()))
        .filter_map(|f| f["filepath"].as_str())
        .collect();
    metric("lint", n - bad.len(), n, method, gaps)
}
