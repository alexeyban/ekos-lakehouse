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

fn det_id(seed: &str) -> KirId {
    KirId(Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes()))
}

pub fn run_id(table: &str, run: &str) -> KirId {
    det_id(&format!("lakehouse-run:{table}:{run}"))
}

pub fn metric_id(table: &str, run: &str, metric: &str) -> KirId {
    det_id(&format!("lakehouse-metric:{table}:{run}:{metric}"))
}

/// What the orchestrator reports after a load.
#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub table: String,
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
            format!("{} run {}", r.table, r.run_id),
            ObjectKind::Custom(RUN_KIND.into()),
        );
        obj.id = run_id(&r.table, &r.run_id);
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
