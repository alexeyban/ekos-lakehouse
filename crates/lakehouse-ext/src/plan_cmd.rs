//! `lakehouse plan` and `lakehouse generate` (RFC 0192 §5).

use crate::settings::Settings;
use crate::source;
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_bronze::{LoadMode, Lossiness, plan, render};
use std::path::Path;

/// Read the source and plan bronze. Synchronous (PostgreSQL and Trino clients both block): the
/// caller runs it off the async runtime.
pub fn build_plan(
    s: &Settings,
    redaction: &ekos_common::redaction::RedactionConfig,
) -> anyhow::Result<Plan> {
    let pg = connect_source(s, "lakehouse-plan")?;
    let trino = ekos_lakehouse_trino::TrinoReader::new(s.trino_connection()?)?;
    let ps = s.plan_settings();
    let sources = source::read(
        &pg,
        &trino,
        &ps.source_catalog,
        &s.schemas(),
        redaction,
        ps.modes.full_refresh_max_rows,
    )?;
    Ok(plan(&sources, &ps))
}

/// The source, through EKOS's read-only session (RFC 0157).
pub fn connect_source(s: &Settings, purpose: &str) -> anyhow::Result<ekos_pg_live::PgSource> {
    let (host, port) = s.source_host_port()?;
    let database = s
        .source_database
        .clone()
        .ok_or_else(|| anyhow::anyhow!("set `source-database` in [extensions.lakehouse]"))?;
    let conn =
        ekos_migrate::connection::ConnectionRef::parse(&format!("postgres://{host}/{database}"))?
            .with_secret_env(s.source_password_env.clone())
            .with_secret_file(
                s.source_password_file
                    .as_ref()
                    .map(std::path::PathBuf::from),
            );
    Ok(ekos_pg_live::PgSource::connect(
        &conn,
        &host,
        port,
        s.source_user.as_deref().unwrap_or("ekos"),
        purpose,
        &ekos_pg_live::SessionPolicy::default(),
    )?)
}

/// The deployed plan: `lakehouse.lock.json` in the pipeline repository.
pub fn read_lock(dir: &Path) -> anyhow::Result<Plan> {
    let path = dir.join("lakehouse.lock.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("{}: {e} (run `lakehouse generate` first)", path.display()))?;
    Ok(serde_json::from_str(&text)?)
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
                if *keyset_deletes { " + deletes" } else { "" }
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

/// Write the plan's files under `out`. A file edited by hand since the last `generate` is kept
/// and reported, never overwritten; a file that exists but was never generated here is refused.
pub fn write(
    p: &Plan,
    silver: &ekos_lakehouse_silver::SilverSettings,
    out: &Path,
) -> anyhow::Result<Written> {
    let mut files = render::files(p);
    let (silver_files, report) = ekos_lakehouse_silver::files(p, silver);
    files.extend(silver_files);
    let mut manifest = read_manifest(out);
    let mut kept = Vec::new();
    let mut foreign = Vec::new();
    for (path, content) in &files {
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
