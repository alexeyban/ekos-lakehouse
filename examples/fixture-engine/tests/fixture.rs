//! RFC 0255 phase 5 acceptance: an engine built out of tree plans, renders bronze, passes the
//! type-map kit and — live — the conformance kit, through the public crates only.

use ekos_kir::source_capabilities::Capability;
use ekos_lakehouse_bronze::plan::PlanSettings;
use ekos_lakehouse_bronze::typemap_kit::{Sample, check, postgres_samples};
use ekos_lakehouse_bronze::{
    EngineSpec, Lossiness, SourceColumn, SourceTable, TypeKind, plan_for, render,
};
use ekos_lakehouse_ext::engines::{Engines, SourceProvider};
use ekos_lakehouse_ext::settings::{Settings, load_with};
use ekos_lakehouse_fixture_engine::{ENGINE, FixtureProvider, FixtureSpec, lakehouse};
use std::collections::BTreeMap;

fn col(name: &str, ty: &str, trino: &str) -> SourceColumn {
    SourceColumn {
        name: name.into(),
        pg_type: ty.into(),
        kind: TypeKind::Base,
        not_null: false,
        trino_type: Some(trino.into()),
        numeric_used: None,
        comment: None,
        enum_labels: vec![],
        pii_candidate: None,
        infinite_values: None,
        attnum: 1,
        has_default: false,
    }
}

fn tables() -> Vec<SourceTable> {
    let t = |name: &str, columns| SourceTable {
        schema: "public".into(),
        name: name.into(),
        oid: 1,
        columns,
        primary_key: vec!["id".into()],
        row_estimate: 10_000,
        update_triggers: vec![],
        foreign_keys: vec![],
        comment: None,
    };
    vec![
        t(
            "orders",
            vec![
                col("id", "bigint", "bigint"),
                col("amount", "numeric(10,2)", "decimal(10,2)"),
                col(
                    "updated",
                    "timestamp with time zone",
                    "timestamp(6) with time zone",
                ),
            ],
        ),
        t(
            "ledger",
            vec![
                col("id", "bigint", "bigint"),
                col("total", "money", "varchar"),
            ],
        ),
    ]
}

#[test]
fn the_fixture_registers_and_reads_its_own_settings() {
    let ext = lakehouse();
    assert_eq!(ext.engines().names().last(), Some(&ENGINE));
    let config: ekos_compiler_core::EkosConfig = toml::from_str(
        "[extensions.lakehouse]\nsource-engine = \"fixture\"\nsource = \"db:5432\"\nsource-database = \"shop\"\n",
    )
    .unwrap();
    let s = load_with(&config, ext.engines()).unwrap();
    assert_eq!(s.engine(), ENGINE);
    // The public build has no such engine and says so by name.
    let e = load_with(&config, &Engines::builtin())
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("source engine `fixture` is not in this build"),
        "{e}"
    );
    // The provider's refusal names the key and the engine.
    let bad: ekos_compiler_core::EkosConfig = toml::from_str(
        "[extensions.lakehouse]\nsource-engine = \"fixture\"\nsource-database = \"shop\"\nsource-password-file = \"pw\"\n",
    )
    .unwrap();
    let e = load_with(&bad, ext.engines()).unwrap_err().to_string();
    assert_eq!(e, "source-password-file is not read by the fixture engine");
}

#[test]
fn the_fixture_plans_and_renders_bronze() {
    let mut s = PlanSettings {
        source_engine: ENGINE,
        ..Default::default()
    };
    s.modes
        .watermarks
        .insert("public.orders".into(), "updated".into());
    let plan = plan_for(&FixtureSpec, &tables(), &s, &BTreeMap::new());
    assert_eq!(plan.tables.len(), 1, "{:?}", plan.refused);
    assert_eq!(plan.refused[0].table, "ledger");
    assert!(
        plan.refused[0].reasons[0].starts_with("total (money)"),
        "{:?}",
        plan.refused
    );
    let files = render::files(&plan);
    let lock = &files["lakehouse.lock.json"];
    assert!(lock.contains("\"source-engine\": \"fixture\""), "{lock}");
    assert!(files.contains_key("bronze/public/orders/load.sql"));
    // The lock reads back in a build that has the engine, and is refused in one that has not.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lakehouse.lock.json"), lock).unwrap();
    let with = Settings {
        engines: lakehouse().engines().clone(),
        ..Default::default()
    };
    assert!(ekos_lakehouse_ext::plan_cmd::read_lock_for(dir.path(), &with).is_ok());
    let e = ekos_lakehouse_ext::plan_cmd::read_lock_for(dir.path(), &Settings::default())
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("source engine `fixture` is not in this build"),
        "{e}"
    );
}

#[test]
fn the_fixture_type_map_passes_the_kit() {
    let mut samples = postgres_samples();
    samples.push(Sample::new("money", Some("varchar")));
    let r = check(&FixtureSpec, &samples);
    assert!(r.passed(), "{:#?}", r.failures);
    let m = FixtureSpec.map_column(&col("m", "money", "varchar"));
    assert_eq!(m.lossiness, Lossiness::Refused);
}

#[test]
fn without_a_run_the_fixture_cannot_reconcile() {
    let sc = ekos_lakehouse_ext::capabilities::record(
        &FixtureProvider,
        &Settings::default(),
        &tables(),
        None,
    );
    assert!(sc.capabilities.get(Capability::Reconcile).is_absent());
    assert!(sc.capabilities.get(Capability::DeclaredKeys).is_absent());
    assert_eq!(sc.engine, "fixture");
}

fn live() -> bool {
    std::env::var("EKOS_LAKEHOUSE_LIVE").is_ok_and(|v| v == "1")
}

/// Live: the fixture's own canonical dialect passes the public conformance kit on a real
/// PostgreSQL-compatible engine, through the provider's read-only connection, so its
/// `reconcile` capability holds. `EKOS_LH_SOURCE` (`host:port`, default the sandbox's) and
/// `EKOS_LH_PG_PASSWORD`.
#[test]
fn live_the_fixture_dialect_passes_the_conformance_kit() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 with the sandbox up");
        return;
    }
    let s = Settings {
        source: Some(std::env::var("EKOS_LH_SOURCE").unwrap_or_else(|_| "127.0.0.1:55433".into())),
        source_database: Some("pagila".into()),
        source_user: Some("ekos".into()),
        source_password_env: Some("EKOS_LH_PG_PASSWORD".into()),
        ..Default::default()
    };
    let src = FixtureProvider.connect(&s, "fixture-test").unwrap();
    assert_eq!(src.reader().dialect().name(), "fixture");
    let report = ekos_lakehouse_ext::capabilities::conformance(&FixtureProvider, src.as_ref())
        .expect("the fixture brings literals");
    eprintln!(
        "RFC 0255 conformance fixture: {} cases, {} hashes, buckets {}, passed {}, id {}",
        report.cases_checked,
        report.hashes_checked,
        report.buckets_checked,
        report.passed(),
        report.id
    );
    assert!(report.passed(), "{report:#?}");
    let sc = ekos_lakehouse_ext::capabilities::record(&FixtureProvider, &s, &[], Some(&report));
    assert!(sc.capabilities.get(Capability::Reconcile).is_provided());
    eprintln!("{}", ekos_lakehouse_ext::capabilities::render(&sc));
}
