//! `lakehouse plan` and `lakehouse generate` (RFC 0192 §5).

use crate::settings::Settings;
use crate::source;
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::{Cadence, LoadMode, Lossiness, render};
use std::path::Path;

/// Read the source and plan bronze. Synchronous (the source and Trino clients both block): the
/// caller runs it off the async runtime.
pub fn build_plan(
    s: &Settings,
    redaction: &ekos_common::redaction::RedactionConfig,
) -> anyhow::Result<Plan> {
    build_plan_with(s, redaction, &Default::default())
}

/// [`build_plan`] with EKOS's write-behaviour facts by `schema.table` (RFC 0225).
pub fn build_plan_with(
    s: &Settings,
    redaction: &ekos_common::redaction::RedactionConfig,
    facts: &std::collections::BTreeMap<String, ekos_lakehouse_bronze::WriteFacts>,
) -> anyhow::Result<Plan> {
    Ok(build_plan_checked(s, redaction, facts)?.0)
}

/// What [`build_plan_checked`] returns: the plan, the engine's capabilities, and each read
/// table's row estimate by `schema.table` (the advice of EKOS RFC 0256 §3.6 weighs it).
pub type CheckedPlan = (
    Plan,
    ekos_kir::source_capabilities::SourceCapabilities,
    std::collections::BTreeMap<String, i64>,
);

/// [`build_plan_with`], and the source engine's capabilities as checked against what the plan
/// read, with the conformance kit run on the source (RFC 0255 §3.5): the fact `plan` records.
pub fn build_plan_checked(
    s: &Settings,
    redaction: &ekos_common::redaction::RedactionConfig,
    facts: &std::collections::BTreeMap<String, ekos_lakehouse_bronze::WriteFacts>,
) -> anyhow::Result<CheckedPlan> {
    let provider = s.engines.get(&s.engine())?.clone();
    let src = connect_source(s, "lakehouse-plan")?;
    let trino = ekos_lakehouse_trino::TrinoReader::new(s.trino_connection()?)?;
    let ps = s.plan_settings();
    let sources = src.read_tables(
        &trino,
        &ps.source_catalog,
        &s.schemas(),
        redaction,
        ps.modes.full_refresh_max_rows,
    )?;
    let spec = provider.spec();
    let mut plan = ekos_lakehouse_bronze::plan_for(spec.as_ref(), &sources, &ps, facts);
    ekos_lakehouse_bronze::attach_checks(&mut plan, &src.value_lists(&s.schemas(), redaction)?);
    let report = crate::capabilities::conformance(provider.as_ref(), src.as_ref());
    let caps = crate::capabilities::record(provider.as_ref(), s, &sources, report.as_ref());
    let rows = sources
        .iter()
        .map(|t| (format!("{}.{}", t.schema, t.name), t.row_estimate))
        .collect();
    Ok((plan, caps, rows))
}

/// The source engine, connected read-only (RFC 0216 §2.1), through its provider (RFC 0255 §3.3).
pub fn connect_source(s: &Settings, purpose: &str) -> anyhow::Result<Box<dyn source::Source>> {
    s.engines.get(&s.engine())?.connect(s, purpose)
}

/// The deployed plan: `lakehouse.lock.json` in the pipeline repository.
pub fn read_lock(dir: &Path) -> anyhow::Result<Plan> {
    let path = dir.join("lakehouse.lock.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("{}: {e} (run `lakehouse generate` first)", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

/// The deployed plan, refused when it names a source engine this build lacks (RFC 0255 §3.1): a
/// pipeline planned by a build with a sold engine, opened by the public build. For every command
/// that reads the source or renders engine SQL from the lock.
pub fn read_lock_for(dir: &Path, s: &Settings) -> anyhow::Result<Plan> {
    let plan = read_lock(dir)?;
    s.engines
        .get(&plan.settings.source_engine)
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.join("lakehouse.lock.json").display()))?;
    Ok(plan)
}

pub fn render_summary(p: &Plan) -> String {
    let mut out = String::new();
    for t in &p.tables {
        let mode = match &t.decision.mode {
            LoadMode::Full => "full".to_string(),
            LoadMode::Incremental {
                watermark,
                keyset_deletes,
            } => format!(
                "incremental by {watermark}{}",
                match (&t.decision.deletes, keyset_deletes) {
                    (Some(d), _) => match d.cadence {
                        Cadence::Run => format!(" + {}s", d.kind),
                        Cadence::Audit { days } => format!(" + audit {days}d"),
                    },
                    (None, true) => " + deletes".to_string(),
                    (None, false) => String::new(),
                }
            ),
        };
        let count = |l: Lossiness| t.columns.iter().filter(|c| c.lossiness == l).count();
        let uncompared = t.columns.iter().filter(|c| c.compare.is_none()).count();
        out.push_str(&format!(
            "{:<32} → {}.{:<24} {:<34} {} cols ({} widened, {} as text, {} measured){}\n",
            format!("{}.{}", t.schema, t.table),
            t.namespace,
            t.table,
            mode,
            t.columns.len(),
            count(Lossiness::Widening),
            count(Lossiness::AsText),
            count(Lossiness::NarrowingSafe),
            if uncompared > 0 {
                format!(", {uncompared} not compared")
            } else {
                String::new()
            }
        ));
        for n in &t.decision.notes {
            out.push_str(&format!("    note: {n}\n"));
        }
    }
    for r in &p.refused {
        out.push_str(&format!("REFUSED {}.{}\n", r.schema, r.table));
        for reason in &r.reasons {
            out.push_str(&format!("    {reason}\n"));
        }
    }
    out.push_str(&format!(
        "{} tables planned, {} refused\n",
        p.tables.len(),
        p.refused.len()
    ));
    out
}

/// What `generate` last wrote: path → SHA-256 of the content. A file whose content no longer
/// matches was edited by a person (RFC 0190: "drift from generated").
pub const MANIFEST: &str = ".generated.json";

pub fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn read_manifest(out: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(out.join(MANIFEST))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The generated files whose content on disk is not what `generate` last wrote there.
pub fn edited_files(out: &Path) -> Vec<String> {
    read_manifest(out)
        .into_iter()
        .filter(|(path, hash)| {
            std::fs::read_to_string(out.join(path)).is_ok_and(|have| &sha256(&have) != hash)
        })
        .map(|(path, _)| path)
        .collect()
}

pub struct Written {
    pub files: usize,
    /// Edited by a person since the last `generate`: left as they are.
    pub kept: Vec<String>,
    /// Generated before, no longer part of the plan, and unedited: removed.
    pub removed: Vec<String>,
    pub silver: ekos_lakehouse_silver::SilverReport,
}

/// Every generated file of the plan: bronze, the Dagster definitions, silver's dbt-trino project
/// and — when gold is built on DuckDB (EKOS RFC 0259) — the dbt-duckdb project's own files.
pub fn files(
    p: &Plan,
    silver: &ekos_lakehouse_silver::SilverSettings,
    gold: &ekos_lakehouse_gold::duckdb::DuckDbGold,
) -> (
    std::collections::BTreeMap<String, String>,
    ekos_lakehouse_silver::SilverReport,
) {
    let mut files = render::files(p);
    let (silver_files, report) = ekos_lakehouse_silver::files(p, silver);
    files.extend(silver_files);
    if p.settings.gold_engine == ekos_lakehouse_bronze::GoldEngine::Duckdb {
        files.extend(ekos_lakehouse_gold::duckdb::project_files(p, gold));
    }
    (files, report)
}

/// Write the plan's files under `out`. A file edited by hand since the last `generate` is kept
/// and reported, never overwritten; a file that exists but was never generated here is refused.
pub fn write(
    p: &Plan,
    silver: &ekos_lakehouse_silver::SilverSettings,
    gold: &ekos_lakehouse_gold::duckdb::DuckDbGold,
    out: &Path,
) -> anyhow::Result<Written> {
    let (files, report) = files(p, silver, gold);
    let mut manifest = read_manifest(out);
    let mut kept = Vec::new();
    let mut foreign = Vec::new();
    for (path, content) in &files {
        // The lock records what the SQL beside it was rendered from: kept apart from that SQL
        // it would describe another pipeline, so it is always rewritten (EKOS RFC 0227).
        if path == "lakehouse.lock.json" {
            continue;
        }
        let Ok(have) = std::fs::read_to_string(out.join(path)) else {
            continue;
        };
        match manifest.get(path) {
            Some(hash) if &sha256(&have) == hash => {}
            _ if &have == content => {}
            Some(_) => kept.push(path.clone()),
            None => foreign.push(path.clone()),
        }
    }
    if !foreign.is_empty() {
        anyhow::bail!(
            "these files exist but were not generated here; move them or remove them: {}",
            foreign.join(", ")
        );
    }
    for (path, content) in &files {
        if kept.contains(path) {
            continue;
        }
        let full = out.join(path);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&full, content)?;
        manifest.insert(path.clone(), sha256(content));
    }
    // A file generated before that the plan no longer produces (a table refused or dropped)
    // would keep feeding the old state to dbt: remove it, unless a person has edited it.
    let mut removed = Vec::new();
    let stale: Vec<String> = manifest
        .keys()
        .filter(|p| !files.contains_key(*p) && !crate::pentaho_cmd::is_pentaho_path(p))
        .cloned()
        .collect();
    for path in stale {
        let full = out.join(&path);
        match std::fs::read_to_string(&full) {
            Ok(have) if sha256(&have) == manifest[&path] => {
                std::fs::remove_file(&full)?;
                manifest.remove(&path);
                removed.push(path);
            }
            Ok(_) => kept.push(path),
            Err(_) => {
                manifest.remove(&path);
            }
        }
    }
    std::fs::write(
        out.join(MANIFEST),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;
    Ok(Written {
        files: files.len() - kept.len(),
        kept,
        removed,
        silver: report,
    })
}

pub fn render_silver_report(r: &ekos_lakehouse_silver::SilverReport) -> String {
    let mut out = String::new();
    let section = |out: &mut String, title: &str, items: &[String]| {
        if !items.is_empty() {
            out.push_str(&format!("{title} ({}):\n", items.len()));
            for i in items {
                out.push_str(&format!("    {i}\n"));
            }
        }
    };
    section(&mut out, "masked", &r.masked);
    section(
        &mut out,
        "PII candidates, not masked until listed in [extensions.lakehouse.pii]",
        &r.pii_candidates,
    );
    section(
        &mut out,
        "foreign keys without a relationship test",
        &r.skipped_foreign_keys,
    );
    out
}
