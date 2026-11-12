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

/// The sandbox's Trino, or `EKOS_LH_TRINO_URL`.
fn trino_url() -> String {
    std::env::var("EKOS_LH_TRINO_URL").unwrap_or_else(|_| "http://127.0.0.1:58080".into())
}

fn source_addr() -> String {
    std::env::var("EKOS_LH_SOURCE").unwrap_or_else(|_| "127.0.0.1:55433".into())
}

/// The end-to-end settings: the fixture engine registered, its source database `fixture` and the
/// Trino catalog of the same name over it, bronze in this test's own namespace.
fn e2e_settings() -> Settings {
    Settings {
        trino_url: Some(trino_url()),
        source_engine: Some(ENGINE),
        engines: lakehouse().engines().clone(),
        source: Some(source_addr()),
        source_database: Some("fixture".into()),
        source_user: Some("ekos".into()),
        source_password_env: Some("EKOS_LH_PG_PASSWORD".into()),
        source_catalog: Some("fixture".into()),
        namespace_prefix: Some("bronze_fx_".into()),
        watermarks: BTreeMap::from([("public.orders".into(), "updated".into())]),
        ..Default::default()
    }
}

/// A writing session on the source, for the test's own setup and changes only: the engine
/// itself connects read-only.
fn source_writer() -> postgres::Client {
    let (host, port) = source_addr()
        .rsplit_once(':')
        .map(|(h, p)| (h.to_string(), p.parse::<u16>().unwrap()))
        .unwrap();
    let pw = std::env::var("EKOS_LH_PG_PASSWORD").unwrap();
    postgres::Client::connect(
        &format!("host={host} port={port} user=ekos dbname=fixture password={pw}"),
        postgres::NoTls,
    )
    .expect("the source database `fixture` (CREATE DATABASE fixture) and its Trino catalog")
}

/// What the orchestrator does for one table and run: create + watermark (incremental), load,
/// deletes.
fn run_table(
    admin: &ekos_lakehouse_trino::Client,
    t: &ekos_lakehouse_bronze::plan::TablePlan,
    s: &PlanSettings,
    run: &str,
) {
    let mut watermark = "NULL".to_string();
    if let ekos_lakehouse_bronze::LoadMode::Incremental { watermark: w, .. } = &t.decision.mode {
        for f in [render::create_sql(t, s), render::schema_sql(t, s)]
            .into_iter()
            .flatten()
        {
            for stmt in render::statements(&f) {
                admin
                    .execute(&stmt)
                    .unwrap_or_else(|e| panic!("{stmt}: {e}"));
            }
        }
        let q = render::statements(&render::watermark_sql(t, s).unwrap()).remove(0);
        let r = admin.execute(&q).unwrap();
        let ty = &t
            .columns
            .iter()
            .find(|m| &m.column == w)
            .unwrap()
            .iceberg_type;
        watermark = render::watermark_literal(r.rows[0][0].as_str(), ty);
    }
    for file in [Some(render::load_sql(t, s)), render::deletes_sql(t, s)]
        .into_iter()
        .flatten()
    {
        for stmt in render::statements(&render::fill(&file, run, "", &watermark)) {
            admin
                .execute(&stmt)
                .unwrap_or_else(|e| panic!("{}: {e}\n{stmt}", t.table));
        }
    }
}

/// Reconcile one table with the fixture's own dialect on the source side; the verdict and
/// whether the planted control fired.
fn reconcile_one(
    t: &ekos_lakehouse_bronze::plan::TablePlan,
    s: &PlanSettings,
) -> (ekos_lakehouse_ext::reconcile::Verdict, Option<bool>) {
    let src = FixtureProvider
        .connect(&e2e_settings(), "fixture-e2e")
        .unwrap();
    assert_eq!(src.reader().dialect().name(), "fixture");
    let tgt =
        ekos_lakehouse_trino::TrinoReader::new(e2e_settings().trino_connection().unwrap()).unwrap();
    let r = ekos_lakehouse_ext::reconcile::reconcile(t, s, src.reader(), &tgt)
        .unwrap_or_else(|e| panic!("{}: {e}", t.table));
    eprintln!(
        "{:<10} {:?} control={:?} not compared={:?}",
        t.table, r.verdict, r.control_fired, r.not_compared
    );
    (r.verdict, r.control_fired)
}

/// Live, end to end (RFC 0255 §9 "not done" until now): the out-of-tree engine plans its own
/// source through Trino, refuses its `money` table by name, lands bronze, follows an
/// incremental change and reconciles every table green at V1–V3 with its own canonical dialect
/// — and a value changed in bronze is caught. Needs a source database `fixture` and a Trino
/// catalog `fixture` over it, plus `EKOS_LH_TRINO_URL` / `EKOS_LH_SOURCE` /
/// `EKOS_LH_PG_PASSWORD`. Writes only to `fixture` and to its own bronze namespace.
#[test]
fn live_the_fixture_lands_bronze_and_reconciles_end_to_end() {
    use ekos_lakehouse_ext::reconcile::Verdict;
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 with the sandbox and a `fixture` catalog up");
        return;
    }
    let mut db = source_writer();
    db.batch_execute(
        "DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;
         CREATE TABLE orders (id bigint PRIMARY KEY, amount numeric(10,2), status text,
                              placed date, updated timestamptz NOT NULL DEFAULT now());
         -- The watermark is proven, not declared alone (RFC 0192): a trigger moves it.
         CREATE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS
           $$ BEGIN NEW.updated := clock_timestamp(); RETURN NEW; END $$;
         CREATE TRIGGER touch BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION touch();
         CREATE TABLE customers (id integer PRIMARY KEY, name text, vip boolean,
                                 since timestamp);
         CREATE TABLE ledger (id bigint PRIMARY KEY, total money);
         INSERT INTO orders SELECT i, i * 1.25, CASE WHEN i % 3 = 0 THEN 'open' ELSE 'sent' END,
                DATE '2026-01-01' + i, now() - interval '1 day'
           FROM generate_series(1, 300) i;
         INSERT INTO customers SELECT i, 'c' || i || E'\\\\ ' || chr(31), i % 2 = 0,
                TIMESTAMP '2026-01-01 00:00:00.000001' + i * interval '1 minute'
           FROM generate_series(1, 120) i;
         INSERT INTO ledger SELECT i, i FROM generate_series(1, 10) i;
         ANALYZE;",
    )
    .unwrap();

    let s = e2e_settings();
    let plan = ekos_lakehouse_ext::plan_cmd::build_plan(&s, &Default::default()).unwrap();
    let names: Vec<&str> = plan.tables.iter().map(|t| t.table.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?} refused {:?}", plan.refused);
    assert!(names.contains(&"orders") && names.contains(&"customers"));
    assert_eq!(plan.refused.len(), 1);
    assert_eq!(plan.refused[0].table, "ledger");
    assert!(
        plan.refused[0].reasons[0].starts_with("total (money)"),
        "{:?}",
        plan.refused
    );
    assert_eq!(plan.settings.source_engine, ENGINE);
    let orders = plan.tables.iter().find(|t| t.table == "orders").unwrap();
    assert!(
        matches!(
            orders.decision.mode,
            ekos_lakehouse_bronze::LoadMode::Incremental { .. }
        ),
        "{:?}",
        orders.decision
    );

    let admin = ekos_lakehouse_trino::Client::new(s.trino_connection().unwrap()).unwrap();
    for t in &plan.tables {
        admin
            .execute(&format!(
                "DROP TABLE IF EXISTS {}",
                render::target(t, &plan.settings)
            ))
            .unwrap();
    }
    admin
        .execute(&render::statements(&render::namespace_sql("bronze_fx_public", &plan.settings))[0])
        .unwrap();

    // First load: every table green, the planted control fired.
    for t in &plan.tables {
        run_table(&admin, t, &plan.settings, "fx-1");
        assert_eq!(
            reconcile_one(t, &plan.settings),
            (Verdict::Green, Some(true)),
            "{}",
            t.table
        );
    }

    // Incremental: an update and an insert, picked up by the watermark.
    db.batch_execute(
        "UPDATE orders SET amount = amount + 1 WHERE id = 7;
         INSERT INTO orders (id, amount, status, placed) VALUES (301, 9.99, 'open', DATE '2026-06-01');",
    )
    .unwrap();
    run_table(&admin, orders, &plan.settings, "fx-2");
    assert_eq!(
        reconcile_one(orders, &plan.settings),
        (Verdict::Green, Some(true)),
        "after an update and an insert"
    );

    // A value changed in bronze behind the pipeline's back: reconciliation is not vacuous.
    let customers = plan.tables.iter().find(|t| t.table == "customers").unwrap();
    admin
        .execute(&format!(
            "UPDATE {} SET name = 'tampered' WHERE id = 60",
            render::target(customers, &plan.settings)
        ))
        .unwrap();
    let (verdict, _) = reconcile_one(customers, &plan.settings);
    assert_ne!(
        verdict,
        Verdict::Green,
        "a changed value must not reconcile green"
    );
}
