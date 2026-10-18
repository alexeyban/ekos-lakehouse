//! Runs and reconciliations as ledger facts (RFC 0192 §7). Counts, verdicts and the queries that
//! measured them — never a row value: a divergence names a bucket and its checksums, V2 names a
//! column and an aggregate.

use crate::reconcile::Reconciliation;
use ekos_kir::{KirId, KirObject, ObjectKind};
use ekos_ledger::KnowledgeStore;
use ekos_ledger::provenance::WriteContext;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

pub const RUN_KIND: &str = "LakehouseRun";
pub const METRIC_KIND: &str = "LakehouseMetric";
pub const CHANGE_KIND: &str = "LakehouseChange";
pub const MART_KIND: &str = "LakehouseMart";
pub const TRANSLATION_KIND: &str = "PentahoTranslation";

fn det_id(seed: &str) -> KirId {
    KirId(Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes()))
}

pub fn run_id(table: &str, layer: &str, run: &str) -> KirId {
    det_id(&format!("lakehouse-run:{layer}:{table}:{run}"))
}

pub fn metric_id(table: &str, run: &str, metric: &str) -> KirId {
    det_id(&format!("lakehouse-metric:{table}:{run}:{metric}"))
}

/// What the orchestrator reports after a load.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub table: String,
    /// `bronze`, `silver` or `gold`.
    pub layer: String,
    pub run_id: String,
    pub status: String,
    pub rows: Option<i64>,
    pub duration_ms: Option<u64>,
    pub snapshot_id: Option<String>,
    pub message: Option<String>,
}

fn with_context<T>(
    store: &dyn KnowledgeStore,
    run: &str,
    stage: &str,
    f: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    store.set_write_context(Some(WriteContext {
        run_id: run.to_string(),
        stage: format!("lakehouse:{stage}"),
        source_artifact_id: None,
    }));
    let out = f();
    store.set_write_context(None);
    out
}

pub fn record_run(store: &dyn KnowledgeStore, r: &RunReport) -> anyhow::Result<KirId> {
    with_context(store, &r.run_id, "run-report", || {
        let mut obj = KirObject::new(
            format!("{} {} run {}", r.table, r.layer, r.run_id),
            ObjectKind::Custom(RUN_KIND.into()),
        );
        obj.id = run_id(&r.table, &r.layer, &r.run_id);
        let serde_json::Value::Object(map) = serde_json::to_value(r)? else {
            unreachable!("a struct serializes to an object")
        };
        obj.properties.extend(map);
        store.append_object(&obj)?;
        Ok(obj.id)
    })
}

/// One `LakehouseMetric` per table and run: the reconciliation, with the queries it ran.
pub fn record_reconciliation(
    store: &dyn KnowledgeStore,
    run: &str,
    r: &Reconciliation,
    queries: &[String],
) -> anyhow::Result<KirId> {
    with_context(store, run, "reconcile", || {
        let mut obj = KirObject::new(
            format!("{} reconciliation {run}", r.table),
            ObjectKind::Custom(METRIC_KIND.into()),
        );
        obj.id = metric_id(&r.table, run, "reconciliation");
        obj.properties.insert("table".into(), json!(r.table));
        obj.properties.insert("run_id".into(), json!(run));
        obj.properties
            .insert("metric".into(), json!("reconciliation"));
        obj.properties.insert("verdict".into(), json!(r.verdict));
        obj.properties.insert("tiers".into(), json!(r.tiers));
        obj.properties
            .insert("not_compared".into(), json!(r.not_compared));
        obj.properties
            .insert("control_fired".into(), json!(r.control_fired));
        obj.properties
            .insert("control".into(), json!(r.control_detail));
        obj.properties
            .insert("evidence_queries".into(), json!(queries));
        store.append_object(&obj)?;
        Ok(obj.id)
    })
}

/// The project's ledger, writable. EKOS allows one writer at a time and an orchestrator runs
/// several tables at once, so a held write lock is waited for (writes here are short), up to
/// two minutes, rather than failing the run.
pub fn open_store_waiting(
    config: &ekos_compiler_core::EkosConfig,
    cwd: &std::path::Path,
) -> anyhow::Result<Box<dyn KnowledgeStore>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut pause = std::time::Duration::from_millis(100);
    loop {
        match ekos::commands::store::open_store(config, cwd) {
            Ok(s) => return Ok(s),
            Err(e)
                if e.to_string().contains("write lock") && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(pause);
                pause = (pause * 2).min(std::time::Duration::from_secs(2));
            }
            Err(e) => return Err(e),
        }
    }
}

/// One code-quality metric of the pipeline repository (RFC 0193 §6), with how it was measured.
pub fn record_check(
    store: &dyn KnowledgeStore,
    run: &str,
    m: &crate::check::Metric,
) -> anyhow::Result<KirId> {
    with_context(store, run, "check", || {
        let mut obj = KirObject::new(
            format!("pipeline {} {run}", m.metric),
            ObjectKind::Custom(METRIC_KIND.into()),
        );
        obj.id = metric_id("pipeline", run, m.metric);
        obj.properties.insert("table".into(), json!("pipeline"));
        obj.properties.insert("run_id".into(), json!(run));
        let serde_json::Value::Object(map) = serde_json::to_value(m)? else {
            unreachable!("a struct serializes to an object")
        };
        obj.properties.extend(map);
        store.append_object(&obj)?;
        Ok(obj.id)
    })
}

/// One detected schema change and what was done with it (RFC 0194).
pub fn record_change(
    store: &dyn KnowledgeStore,
    run: &str,
    c: &ekos_lakehouse_changes::Change,
    proposal: Option<&str>,
) -> anyhow::Result<KirId> {
    with_context(store, run, "sync", || {
        let what = c.column.as_deref().unwrap_or("");
        let mut obj = KirObject::new(
            format!("{} {} {what}", c.table, c.class.name()),
            ObjectKind::Custom(CHANGE_KIND.into()),
        );
        obj.id = det_id(&format!(
            "lakehouse-change:{}:{}:{what}:{}:{}",
            c.table,
            c.class.name(),
            c.from.as_deref().unwrap_or(""),
            c.to.as_deref().unwrap_or("")
        ));
        let serde_json::Value::Object(map) = serde_json::to_value(c)? else {
            unreachable!("a struct serializes to an object")
        };
        obj.properties.extend(map);
        obj.properties.insert("run_id".into(), json!(run));
        obj.properties.insert("proposal".into(), json!(proposal));
        store.append_object(&obj)?;
        Ok(obj.id)
    })
}

/// A gold mart drafted from a description: proposed, or asked back (RFC 0195).
pub fn record_mart(
    store: &dyn KnowledgeStore,
    run: &str,
    o: &crate::gold_cmd::DraftOutcome,
) -> anyhow::Result<KirId> {
    with_context(store, run, "gold", || {
        let mut obj = KirObject::new(
            format!("gold mart {}", o.mart),
            ObjectKind::Custom(MART_KIND.into()),
        );
        obj.id = det_id(&format!("lakehouse-mart:{}", o.mart));
        obj.properties.insert("mart".into(), json!(o.mart));
        obj.properties.insert(
            "status".into(),
            json!(if o.branch.is_some() {
                "proposed"
            } else {
                "questions"
            }),
        );
        obj.properties.insert("branch".into(), json!(o.branch));
        obj.properties.insert(
            "questions".into(),
            json!(
                o.questions
                    .iter()
                    .map(|(t, q)| format!("{t}: {}", q.text))
                    .collect::<Vec<_>>()
            ),
        );
        obj.properties.insert(
            "anti_invention_atoms_checked".into(),
            json!(o.checked_atoms),
        );
        obj.properties.insert("drafts".into(), json!(o.drafts));
        obj.properties.insert("run_id".into(), json!(run));
        store.append_object(&obj)?;
        Ok(obj.id)
    })
}

/// A table's schema state after `sync` (RFC 0196 §1): one fixed id per table, so the ledger's
/// current version is the current state.
pub fn record_schema_state(
    store: &dyn KnowledgeStore,
    run: &str,
    table: &str,
    state: &str,
    proposal: Option<&str>,
) -> anyhow::Result<KirId> {
    with_context(store, run, "sync", || {
        let mut obj = KirObject::new(
            format!("{table} schema state"),
            ObjectKind::Custom(METRIC_KIND.into()),
        );
        obj.id = det_id(&format!("lakehouse-schema-state:{table}"));
        obj.properties.insert("table".into(), json!(table));
        obj.properties
            .insert("metric".into(), json!("schema_state"));
        obj.properties.insert("state".into(), json!(state));
        obj.properties.insert("proposal".into(), json!(proposal));
        obj.properties.insert("run_id".into(), json!(run));
        store.append_object(&obj)?;
        Ok(obj.id)
    })
}

/// RFC 0204: one `PentahoTranslation` per model of a translation, keyed by package, model and the
/// translation's content (a new translation is a new fact; the old ones stay, append-only).
pub fn record_translation(
    store: &dyn KnowledgeStore,
    r: &ekos_lakehouse_pentaho_dbt::Report,
) -> anyhow::Result<usize> {
    let run = {
        use sha2::{Digest, Sha256};
        let text = serde_json::to_string(r)?;
        format!("{:x}", Sha256::digest(text.as_bytes()))[..12].to_string()
    };
    with_context(store, &run, "pentaho-translate", || {
        let mut n = 0;
        for m in &r.models {
            let mut obj = KirObject::new(
                format!(
                    "{} → {} ({})",
                    m.transformation,
                    m.name,
                    m.fidelity.as_str()
                ),
                ObjectKind::Custom(TRANSLATION_KIND.into()),
            );
            obj.id = det_id(&format!(
                "pentaho-translation:{}:{}:{run}",
                r.package, m.name
            ));
            let serde_json::Value::Object(map) = serde_json::to_value(m)? else {
                unreachable!("a struct serializes to an object")
            };
            obj.properties.extend(map);
            obj.properties.insert("package".into(), json!(r.package));
            obj.properties.insert("translation".into(), json!(run));
            store.append_object(&obj)?;
            n += 1;
        }
        // The whole package in one fact: what `ekos_pentaho_translation` serves.
        let mut obj = KirObject::new(
            format!("Pentaho package {} translation", r.package),
            ObjectKind::Custom(TRANSLATION_KIND.into()),
        );
        obj.id = det_id(&format!("pentaho-translation:{}:{run}", r.package));
        obj.properties.insert("record".into(), json!("package"));
        obj.properties.insert("package".into(), json!(r.package));
        obj.properties.insert("translation".into(), json!(run));
        obj.properties.insert(
            "counts".into(),
            json!(
                r.counts()
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .collect::<std::collections::BTreeMap<_, _>>()
            ),
        );
        obj.properties
            .insert("report".into(), serde_json::to_value(r)?);
        store.append_object(&obj)?;
        Ok(n + 1)
    })
}

/// The latest translation report of each package in the ledger (RFC 0204), newest first.
pub fn translations(store: &dyn KnowledgeStore) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut latest: std::collections::BTreeMap<String, (i64, serde_json::Value)> =
        Default::default();
    for o in store.all_objects()? {
        if o.kind != ObjectKind::Custom(TRANSLATION_KIND.into()) {
            continue;
        }
        if o.properties.get("record").and_then(|v| v.as_str()) != Some("package") {
            continue;
        }
        let pkg = o
            .properties
            .get("package")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let at = o.created_at.timestamp();
        let value = serde_json::to_value(&o.properties)?;
        if latest.get(&pkg).is_none_or(|(t, _)| at >= *t) {
            latest.insert(pkg, (at, value));
        }
    }
    let mut out: Vec<_> = latest.into_values().collect();
    out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    Ok(out.into_iter().map(|(_, v)| v).collect())
}
