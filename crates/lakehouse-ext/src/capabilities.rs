//! RFC 0255 §3.5 in the Lakehouse: the source engine's declared capabilities, checked against
//! what the build measured, recorded at `plan` as one `SourceCapabilities` fact per source and
//! printed by `lakehouse doctor`.
//!
//! The one capability that needs the engine to prove something is `reconcile`: it is provided
//! only after the public conformance kit (`ekos_migrate_validate::conformance`) passes on the
//! connected source in the engine's own dialect. The kit's checks are read-only `SELECT`s.

use crate::engines::SourceProvider;
use crate::settings::Settings;
use crate::source::Source;
use ekos_kir::source_capabilities::{
    CAPABILITIES_VERSION, Capability, Evidence, SourceCapabilities, checked,
};
use ekos_lakehouse_bronze::SourceTable;
use ekos_migrate_validate::conformance::{self, Report};

/// The conformance kit on the connected source, with the provider's literal table; `None` when
/// the engine brings none (its `reconcile` is then absent).
pub fn conformance(provider: &dyn SourceProvider, source: &dyn Source) -> Option<Report> {
    let literals = provider.conformance_literals()?;
    Some(conformance::run(source.reader(), literals.as_ref()))
}

/// What the plan read, as the downgrade rules need it.
pub fn evidence(tables: &[SourceTable], report: Option<&Report>) -> Evidence {
    Evidence {
        conformance: report.map(|r| (r.id.clone(), r.passed())),
        tables_read: tables.len(),
        keys_read: tables
            .iter()
            .map(|t| usize::from(!t.primary_key.is_empty()) + t.foreign_keys.len())
            .sum(),
    }
}

/// The source's key in the ledger: its `source-database` (for Oracle, the `[[databases]]` name).
pub fn source_key(s: &Settings) -> String {
    s.source_database
        .clone()
        .unwrap_or_else(|| crate::NAME.to_string())
}

/// The declaration, checked (RFC 0255 §3.5 rules), as the fact to record.
pub fn record(
    provider: &dyn SourceProvider,
    s: &Settings,
    tables: &[SourceTable],
    report: Option<&Report>,
) -> SourceCapabilities {
    let spec = provider.spec();
    let declared = spec.capabilities();
    let (capabilities, mut downgrades) = checked(&declared, &evidence(tables, report));
    if report.is_none() && !declared.get(Capability::Reconcile).is_absent() {
        downgrades.push("the engine brings no conformance literals: the kit cannot run".into());
    }
    SourceCapabilities {
        source: source_key(s),
        engine: spec.engine().to_string(),
        capabilities_version: CAPABILITIES_VERSION,
        capabilities,
        conformance_run: report.map(|r| r.id.clone()),
        downgrades,
    }
}

/// The capability table for the terminal.
pub fn render(sc: &SourceCapabilities) -> String {
    let mut out = format!(
        "source {} (engine {}, capability list v{}){}\n",
        sc.source,
        sc.engine,
        sc.capabilities_version,
        sc.conformance_run
            .as_deref()
            .map(|r| format!(", conformance run {}", &r[..r.len().min(12)]))
            .unwrap_or_default()
    );
    for (c, s) in sc.capabilities.all() {
        out.push_str(&format!(
            "  {:<16} {:<9} {}\n",
            c.name(),
            s.word(),
            if s.is_provided() {
                String::new()
            } else if s.detail().is_empty() {
                c.when_absent().to_string()
            } else {
                format!("{} — {}", s.detail(), c.when_absent())
            }
        ));
    }
    for d in &sc.downgrades {
        out.push_str(&format!("  downgraded: {d}\n"));
    }
    out
}

/// Whether `sc` lets a reconciliation be called healthy: only with `reconcile` provided.
pub fn reconcile_trusted(sc: &SourceCapabilities) -> bool {
    sc.capabilities.get(Capability::Reconcile).is_provided()
}

/// The note a reconciliation carries when the engine's `reconcile` is not provided: agreement
/// in a dialect nobody proved is not evidence (RFC 0255 §3.5, RFC 0196 `unverified`).
pub fn untrusted_note(sc: &SourceCapabilities) -> String {
    format!(
        "source engine {} has reconcile {} ({}): the result cannot be healthy",
        sc.engine,
        sc.capabilities.get(Capability::Reconcile).word(),
        sc.capabilities.get(Capability::Reconcile).detail()
    )
}

/// A reconciliation over an engine whose `reconcile` is not provided: a green result becomes
/// unverified, saying why (RFC 0255 §3.5; RFC 0196 reports it `unverified`).
pub fn apply_trust(r: &mut crate::reconcile::Reconciliation, sc: &SourceCapabilities) {
    if reconcile_trusted(sc) {
        return;
    }
    if r.verdict == crate::reconcile::Verdict::Green {
        r.verdict = crate::reconcile::Verdict::Unverified;
    }
    r.control_detail = if r.control_detail.is_empty() {
        untrusted_note(sc)
    } else {
        format!("{}; {}", r.control_detail, untrusted_note(sc))
    };
}

/// Record `sc` in the project's ledger (one fixed id per source: the latest version is current).
pub fn write(
    store: &dyn ekos_ledger::KnowledgeStore,
    run: &str,
    sc: &SourceCapabilities,
) -> anyhow::Result<()> {
    store.set_write_context(Some(ekos_ledger::provenance::WriteContext {
        run_id: run.to_string(),
        stage: "lakehouse:plan".into(),
        source_artifact_id: None,
    }));
    let r = store.append_object(&ekos_kir::source_capabilities::object(sc));
    store.set_write_context(None);
    r.map(|_| ()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekos_kir::source_capabilities::Support;
    use ekos_lakehouse_bronze::{EngineSpec, ForeignKey, Mapping, SourceColumn, SourceEngine};
    use std::sync::Arc;

    fn table(pk: bool, fks: usize) -> SourceTable {
        SourceTable {
            schema: "S".into(),
            name: "T".into(),
            oid: 1,
            columns: vec![],
            primary_key: if pk { vec!["ID".into()] } else { vec![] },
            row_estimate: 1,
            update_triggers: vec![],
            foreign_keys: (0..fks)
                .map(|i| ForeignKey {
                    name: format!("FK{i}"),
                    columns: vec!["X".into()],
                    ref_schema: "S".into(),
                    ref_table: "U".into(),
                    ref_columns: vec!["ID".into()],
                })
                .collect(),
            comment: None,
        }
    }

    /// A planted engine that declares everything and brings no conformance literals.
    struct Boastful;

    impl EngineSpec for Boastful {
        fn engine(&self) -> SourceEngine {
            SourceEngine::from_static("boastful")
        }
        fn map_column(&self, c: &SourceColumn) -> Mapping {
            ekos_lakehouse_bronze::typemap::map_column(c)
        }
        fn orderable(&self, _: &str) -> bool {
            false
        }
        fn capabilities(&self) -> ekos_kir::source_capabilities::Capabilities {
            ekos_kir::source_capabilities::Capabilities::of(
                Capability::ALL.iter().map(|c| (*c, Support::Provided)),
            )
        }
    }

    impl SourceProvider for Boastful {
        fn spec(&self) -> Arc<dyn EngineSpec> {
            Arc::new(Boastful)
        }
        fn connect(&self, _: &Settings, _: &str) -> anyhow::Result<Box<dyn Source>> {
            anyhow::bail!("planted")
        }
        fn dialect(&self) -> ekos_migrate_validate::Dialect {
            ekos_migrate_validate::Dialect::Postgres
        }
        fn conformance_literals(
            &self,
        ) -> Option<Box<dyn ekos_migrate_validate::conformance::Literals>> {
            None
        }
    }

    /// RFC 0255 §4 phase 4 exit: an engine declaring `reconcile: provided` without a run is
    /// downgraded, and its reconciliations can never be healthy.
    #[test]
    fn a_planted_engine_claiming_reconcile_without_a_run_is_downgraded() {
        let sc = record(
            &Boastful,
            &Settings::default(),
            &[table(false, 0), table(false, 0)],
            None,
        );
        assert_eq!(sc.engine, "boastful");
        assert_eq!(sc.source, "lakehouse");
        assert!(!reconcile_trusted(&sc));
        assert_eq!(
            sc.capabilities.get(Capability::Reconcile),
            Support::absent("no conformance run recorded")
        );
        // Two tables, no key at all: declared keys are partial, with the count.
        assert_eq!(
            sc.capabilities.get(Capability::DeclaredKeys),
            Support::partial("0 keys read from 2 tables")
        );
        assert_eq!(sc.downgrades.len(), 3, "{:?}", sc.downgrades);
        let text = render(&sc);
        assert!(
            text.contains("reconcile        absent    no conformance run recorded"),
            "{text}"
        );
        assert!(untrusted_note(&sc).contains("reconcile absent"));
    }

    /// RFC 0255 §5: an absent `reconcile` never yields healthy — a green reconciliation becomes
    /// unverified, which `lakehouse status` reports as such (RFC 0196).
    #[test]
    fn an_absent_reconcile_never_yields_green() {
        let sc = record(&Boastful, &Settings::default(), &[], None);
        let mut r = crate::reconcile::Reconciliation {
            table: "S.T".into(),
            tiers: vec![],
            not_compared: vec![],
            control_fired: Some(true),
            control_detail: "control fired".into(),
            verdict: crate::reconcile::Verdict::Green,
        };
        apply_trust(&mut r, &sc);
        assert_eq!(r.verdict, crate::reconcile::Verdict::Unverified);
        assert!(
            r.control_detail
                .starts_with("control fired; source engine boastful has reconcile absent"),
            "{}",
            r.control_detail
        );
        let v = serde_json::json!({"verdict": r.verdict});
        assert_eq!(v["verdict"], "unverified", "the fact health reads");
    }

    #[test]
    fn keys_are_counted_from_primary_and_foreign_keys() {
        let e = evidence(&[table(true, 2), table(false, 1)], None);
        assert_eq!((e.tables_read, e.keys_read), (2, 4));
    }

    /// The built-in engines declare every capability, the required ones provided.
    #[test]
    fn the_builtin_engines_declare_every_capability() {
        for p in crate::engines::Engines::builtin().providers() {
            let c = p.spec().capabilities();
            assert!(c.missing_required().is_empty(), "{}", p.spec().engine());
            assert_eq!(c.0.len(), Capability::ALL.len(), "{}", p.spec().engine());
            assert!(p.conformance_literals().is_some());
        }
    }
}
