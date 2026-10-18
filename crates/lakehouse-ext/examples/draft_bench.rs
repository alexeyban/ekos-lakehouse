//! RFC 0205 model bake-off for drafting Pentaho code steps.
//!
//! Set A: the code steps the translator leaves as gaps — does a model's draft pass the checks,
//! and does Trino plan it? Set B: the code steps the translator *does* follow — ask the model
//! anyway and compare its answer with the deterministic SQL, row by row on Trino, over synthetic
//! rows (nulls, empty strings, edge values). B is the correctness score; A is the reach.
//!
//! ```text
//! cargo run -p ekos-lakehouse-ext --example draft_bench -- \
//!   --config ekos.toml --models claude-haiku-5-5,minimax-m3 --budget 1.0 \
//!   --trino http://127.0.0.1:58080 --state /tmp/bench crates/lakehouse-ext/examples/bench-gold <corpus dir>...
//! ```
//! `--dry` lists the requests and calls no model. Every call is metered by the RFC 0197 router;
//! the budget stops the run.

use ekos_lakehouse_kettle::Category;
use ekos_lakehouse_kettle_expr::{Fidelity, KType};
use ekos_lakehouse_kettle_rel::{Field, Op};
use ekos_lakehouse_pentaho_dbt::draft::DraftRequest;
use std::path::PathBuf;

struct Truth {
    columns: Vec<(String, String)>,
    keep_when: Option<String>,
}

fn values(t: KType) -> &'static [&'static str] {
    match t {
        KType::String => &["NULL", "'paid'", "'Hello World'", "'a'", "'void'"],
        KType::Integer => &["NULL", "0", "7", "-3", "1200"],
        KType::Number => &["NULL", "0.0", "2.5", "-1.75", "1500.0"],
        KType::BigNumber => &["NULL", "0", "1.25", "-2.50", "999.99"],
        KType::Date | KType::Timestamp => &[
            "NULL",
            "TIMESTAMP '2024-02-29 13:45:00'",
            "TIMESTAMP '2023-12-31 23:59:59'",
            "TIMESTAMP '2024-01-07 00:00:00'",
            "TIMESTAMP '2020-06-15 08:00:00'",
        ],
        KType::Boolean => &["NULL", "true", "false", "true", "false"],
        _ => &["NULL"],
    }
}

/// `(VALUES …) AS t(ekos_row, cols…)` with five synthetic rows.
fn rows(inputs: &[Field]) -> String {
    let mut out = Vec::new();
    for i in 0..5 {
        let mut cells = vec![i.to_string()];
        for f in inputs {
            let vs = values(f.ty);
            cells.push(format!(
                "CAST({} AS {})",
                vs[i % vs.len()],
                f.ty.trino(f.length, f.precision)
            ));
        }
        out.push(format!("({})", cells.join(", ")));
    }
    let cols: Vec<String> = std::iter::once("\"ekos_row\"".to_string())
        .chain(
            inputs
                .iter()
                .map(|f| format!("\"{}\"", f.name.replace('"', "\"\""))),
        )
        .collect();
    format!("(VALUES {}) AS t({})", out.join(", "), cols.join(", "))
}

fn select(inputs: &[Field], columns: &[(String, String)], keep: Option<&str>) -> String {
    let mut items: Vec<String> = columns
        .iter()
        .map(|(n, e)| format!("CAST(({e}) AS varchar) AS \"o_{}\"", n.replace('"', "")))
        .collect();
    items.push(format!(
        "CASE WHEN ({}) THEN 1 ELSE 0 END AS ekos_keep",
        keep.unwrap_or("TRUE")
    ));
    format!(
        "SELECT \"ekos_row\", {} FROM {} ORDER BY 1",
        items.join(", "),
        rows(inputs)
    )
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut config, mut models, mut budget, mut trino, mut state, mut dry, mut dirs) = (
        PathBuf::from("ekos.toml"),
        Vec::<String>::new(),
        1.0f64,
        String::new(),
        PathBuf::from("/tmp/ekos-draft-bench"),
        false,
        Vec::<PathBuf>::new(),
    );
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => config = args.next().expect("value").into(),
            "--models" => {
                models = args
                    .next()
                    .expect("value")
                    .split(',')
                    .map(str::to_string)
                    .collect()
            }
            "--budget" => budget = args.next().expect("value").parse()?,
            "--trino" => trino = args.next().expect("value"),
            "--state" => state = args.next().expect("value").into(),
            "--dry" => dry = true,
            d => dirs.push(d.into()),
        }
    }
    let plan = ekos_lakehouse_bronze::plan::plan(
        &[],
        &ekos_lakehouse_bronze::plan::PlanSettings::default(),
    );
    let mut set_a: Vec<DraftRequest> = Vec::new();
    let mut set_b: Vec<(DraftRequest, Truth)> = Vec::new();
    for dir in &dirs {
        let package = ekos_lakehouse_ext::pentaho_cmd::read_dir(dir, &Default::default());
        let s = ekos_lakehouse_pentaho_dbt::Settings {
            package: "bench".into(),
            empty_string_is_null: true,
            ..Default::default()
        };
        set_a.extend(ekos_lakehouse_pentaho_dbt::draft::requests(
            &package, &plan, &s,
        ));
        for (t, ir) in ekos_lakehouse_pentaho_dbt::build_irs(&package, &plan, &s) {
            for n in &ir.nodes {
                let Op::Project { items } = &n.op else {
                    continue;
                };
                let Some(step) = t.step(&n.step) else {
                    continue;
                };
                if step.category != Category::Code
                    || !matches!(n.fidelity, Fidelity::Exact | Fidelity::Rewritten)
                    || n.inputs.len() != 1
                {
                    continue;
                }
                let input = ir.node(n.inputs[0]);
                let outputs = ekos_lakehouse_kettle_rel::declared_outputs(step);
                if outputs.is_empty()
                    || input.is_gap()
                    || input.schema.iter().any(|f| f.ty == KType::Unknown)
                {
                    continue;
                }
                let columns: Vec<(String, String)> = outputs
                    .iter()
                    .filter_map(|o| {
                        items
                            .iter()
                            .find(|(nm, _)| nm == &o.name)
                            .map(|(nm, e)| (nm.clone(), e.sql()))
                    })
                    .collect();
                if columns.len() != outputs.len() {
                    continue;
                }
                let keep_when = ir
                    .nodes
                    .iter()
                    .find(|m| m.step == n.step && m.inputs == vec![n.id])
                    .and_then(|m| match &m.op {
                        Op::Filter { predicate } => Some(predicate.sql()),
                        _ => None,
                    });
                set_b.push((
                    DraftRequest {
                        transformation: t.path.clone(),
                        step: n.step.clone(),
                        step_type: n.step_type.clone(),
                        original: ekos_lakehouse_pentaho_dbt::original_text(step),
                        inputs: input.schema.clone(),
                        outputs,
                        reason: "(benchmark) translate the step".into(),
                    },
                    Truth { columns, keep_when },
                ));
            }
        }
    }
    println!(
        "set A (gaps): {} requests; set B (ground truth): {} requests",
        set_a.len(),
        set_b.len()
    );
    if dry {
        for r in set_a.iter().chain(set_b.iter().map(|(r, _)| r)) {
            println!(
                "  {} / {} [{}] — {}",
                r.transformation,
                r.step,
                r.step_type,
                r.reason.chars().take(80).collect::<String>()
            );
        }
        return Ok(());
    }
    let cfg = ekos_compiler_core::EkosConfig::from_file(&config)?;
    std::fs::create_dir_all(&state)?;
    let budget_handle = ekos_recovery::usage::Budget::new(Some(budget));
    let router = ekos_recovery::LlmRouter::new(&cfg.llm, &state)
        .uncached()
        .with_budget(budget_handle.clone());
    let client = ekos_lakehouse_trino::Client::new(ekos_lakehouse_trino::Connection {
        base_url: trino,
        user: "ekos".into(),
        catalog: None,
        schema: None,
    })?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    println!(
        "\n| model | A drafted | A planned | B drafted | B equal | cost $ |\n|---|---|---|---|---|---|"
    );
    for model in &models {
        let before = budget_handle.spent_usd();
        let provider = router.provider_for_models(
            ekos_recovery::Task::Translate,
            Some(std::slice::from_ref(model)),
        )?;
        let (drafts_a, outcomes_a) = rt.block_on(ekos_lakehouse_ext::pentaho_cmd::draft_all(
            &set_a,
            provider.as_ref(),
            3,
        ));
        for o in outcomes_a.iter().filter(|o| !o.drafted).take(40) {
            eprintln!(
                "  {model} A {} / {}: {}",
                o.transformation,
                o.step,
                o.findings.join("; ")
            );
        }
        let b_reqs: Vec<DraftRequest> = set_b.iter().map(|(r, _)| r.clone()).collect();
        let (drafts_b, outcomes_b) = rt.block_on(ekos_lakehouse_ext::pentaho_cmd::draft_all(
            &b_reqs,
            provider.as_ref(),
            3,
        ));
        for o in outcomes_b.iter().filter(|o| !o.drafted) {
            eprintln!(
                "  {model} B {} / {}: {}",
                o.transformation,
                o.step,
                o.findings.join("; ")
            );
        }
        let mut planned = 0;
        for r in &set_a {
            if let Some(d) = drafts_a.get(&r.key())
                && client
                    .execute(&format!(
                        "EXPLAIN {}",
                        select(&r.inputs, &d.columns, d.keep_when.as_deref())
                    ))
                    .is_ok()
            {
                planned += 1;
            }
        }
        let mut equal = 0;
        for (r, truth) in &set_b {
            let Some(d) = drafts_b.get(&r.key()) else {
                continue;
            };
            let mut cols = d.columns.clone();
            cols.sort();
            let mut tcols = truth.columns.clone();
            tcols.sort();
            let want = client.execute(&select(&r.inputs, &tcols, truth.keep_when.as_deref()));
            let got = client.execute(&select(&r.inputs, &cols, d.keep_when.as_deref()));
            match (want, got) {
                (Ok(w), Ok(g)) if w.rows == g.rows => equal += 1,
                (Ok(w), Ok(g)) => eprintln!(
                    "  {model} B differs: {} / {}\n    want {:?}\n    got  {:?}",
                    r.transformation, r.step, w.rows, g.rows
                ),
                (Err(e), _) => eprintln!("  truth failed on Trino for {}: {e}", r.step),
                (_, Err(e)) => eprintln!("  {model} B draft failed on Trino for {}: {e}", r.step),
            }
        }
        println!(
            "| {model} | {}/{} | {planned}/{} | {}/{} | {equal}/{} | {:.4} |",
            drafts_a.len(),
            set_a.len(),
            set_a.len(),
            drafts_b.len(),
            set_b.len(),
            set_b.len(),
            budget_handle.spent_usd() - before
        );
    }
    println!(
        "\ntotal spent: ${:.4} of ${budget:.2}",
        budget_handle.spent_usd()
    );
    Ok(())
}
