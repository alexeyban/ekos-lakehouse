//! RFC 0192 live: Pagila → bronze on the sandbox. Plan from the live catalogs, run the generated
//! files the way the orchestrator does, reconcile every table (control included), then drive an
//! incremental cycle — an update, an insert, a delete — through `film`.
//!
//! Needs the stack, Pagila (`deploy/fixtures/pagila.sh`) and `EKOS_LAKEHOUSE_LIVE=1`. It writes
//! to the sandbox source (one film) and puts it back.

use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_bronze::{LoadMode, render};
use ekos_lakehouse_ext::reconcile::{Verdict, reconcile};
use ekos_lakehouse_ext::settings::Settings;
use ekos_lakehouse_trino::{Client, TrinoReader};
use std::collections::BTreeMap;

fn live() -> bool {
    std::env::var("EKOS_LAKEHOUSE_LIVE").is_ok_and(|v| v == "1")
}

fn settings() -> Settings {
    Settings {
        trino_url: Some("http://127.0.0.1:58080".into()),
        source: Some("127.0.0.1:55433".into()),
        source_database: Some("pagila".into()),
        source_user: Some("ekos".into()),
        source_password_env: Some("EKOS_LH_PG_PASSWORD".into()),
        source_catalog: Some("pagila".into()),
        namespace_prefix: Some("bronze_m1_".into()),
        watermarks: BTreeMap::from([
            ("public.film".into(), "last_update".into()),
            ("public.rental".into(), "last_update".into()),
        ]),
        ..Default::default()
    }
}

fn source_reader() -> ekos_pg_live::PgSource {
    let conn = ekos_migrate::connection::ConnectionRef::parse("postgres://127.0.0.1/pagila")
        .unwrap()
        .with_secret_env(Some("EKOS_LH_PG_PASSWORD".into()));
    ekos_pg_live::PgSource::connect(
        &conn,
        "127.0.0.1",
        55433,
        "ekos",
        "lakehouse-test",
        &ekos_pg_live::SessionPolicy::default(),
    )
    .unwrap()
}

fn pg_writer() -> postgres::Client {
    let pw = std::env::var("EKOS_LH_PG_PASSWORD").unwrap();
    postgres::Client::connect(
        &format!("host=127.0.0.1 port=55433 user=ekos dbname=pagila password={pw}"),
        postgres::NoTls,
    )
    .unwrap()
}

/// What the orchestrator does for one table and run: watermark, load, deletes.
fn run_table(admin: &Client, t: &TablePlan, plan: &Plan, run_id: &str, lsn: &str) {
    let s = &plan.settings;
    let mut watermark = "NULL".to_string();
    if let LoadMode::Incremental { watermark: w, .. } = &t.decision.mode {
        for stmt in render::statements(&render::create_sql(t, s).unwrap()) {
            admin
                .execute(&stmt)
                .unwrap_or_else(|e| panic!("{stmt}: {e}"));
        }
        let q = render::statements(&render::watermark_sql(t, s).unwrap()).remove(0);
        let r = admin.execute(&q).unwrap();
        let text = r.rows[0][0].as_str().map(str::to_string);
        let ty = &t
            .columns
            .iter()
            .find(|m| &m.column == w)
            .unwrap()
            .iceberg_type;
        watermark = render::watermark_literal(text.as_deref(), ty);
    }
    for file in [Some(render::load_sql(t, s)), render::deletes_sql(t, s)]
        .into_iter()
        .flatten()
    {
        for stmt in render::statements(&render::fill(&file, run_id, lsn, &watermark)) {
            admin
                .execute(&stmt)
                .unwrap_or_else(|e| panic!("{}.{}: {e}\n{stmt}", t.schema, t.table));
        }
    }
}

fn reconcile_all(plan: &Plan, only: Option<&str>) -> Vec<String> {
    let src = source_reader();
    let tgt = TrinoReader::new(settings().trino_connection().unwrap()).unwrap();
    let mut bad = Vec::new();
    for t in &plan.tables {
        if only.is_some_and(|o| o != t.table) {
            continue;
        }
        let r =
            reconcile(t, &plan.settings, &src, &tgt).unwrap_or_else(|e| panic!("{}: {e}", t.table));
        eprintln!(
            "{:<16} {:?} control={:?} not compared={:?}",
            t.table, r.verdict, r.control_fired, r.not_compared
        );
        if r.verdict != Verdict::Green {
            bad.push(format!("{}: {:?} {:?}", t.table, r.verdict, r.tiers));
        }
        if r.control_fired != Some(true) {
            bad.push(format!("{}: control {:?}", t.table, r.control_fired));
        }
    }
    bad
}

#[test]
fn pagila_lands_in_bronze_reconciles_and_follows_changes() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 with the sandbox and Pagila up");
        return;
    }
    let plan = ekos_lakehouse_ext::plan_cmd::build_plan(&settings(), &Default::default()).unwrap();
    assert_eq!(plan.tables.len(), 16, "{:?}", plan.refused);
    assert!(plan.refused.is_empty(), "{:?}", plan.refused);
    let admin = Client::new(settings().trino_connection().unwrap()).unwrap();
    let lsn = source_reader().current_lsn().unwrap();

    // A clean start: this test's own namespace only.
    for t in &plan.tables {
        admin
            .execute(&format!(
                "DROP TABLE IF EXISTS {}",
                render::target(t, &plan.settings)
            ))
            .unwrap();
    }
    admin
        .execute(&render::statements(&render::namespace_sql("bronze_m1_public", &plan.settings))[0])
        .unwrap();

    for t in &plan.tables {
        run_table(&admin, t, &plan, "m1-run-1", &lsn);
    }
    let bad = reconcile_all(&plan, None);
    assert!(bad.is_empty(), "first load:\n{}", bad.join("\n"));

    // Incremental: change one film, add one, then delete the added one.
    let film = plan.tables.iter().find(|t| t.table == "film").unwrap();
    let mut pg = pg_writer();
    pg.batch_execute(
        "UPDATE film SET rental_rate = rental_rate + 1 WHERE film_id = 1; \
         INSERT INTO film (film_id, title, language_id, fulltext) \
         VALUES (900001, 'EKOS TEST FILM', 1, to_tsvector('ekos'))",
    )
    .unwrap();
    run_table(&admin, film, &plan, "m1-run-2", &lsn);
    let bad = reconcile_all(&plan, Some("film"));
    assert!(bad.is_empty(), "after update + insert:\n{}", bad.join("\n"));

    pg.batch_execute("DELETE FROM film WHERE film_id = 900001")
        .unwrap();
    run_table(&admin, film, &plan, "m1-run-3", &lsn);
    let bad = reconcile_all(&plan, Some("film"));
    pg.batch_execute("UPDATE film SET rental_rate = rental_rate - 1 WHERE film_id = 1")
        .unwrap();
    assert!(bad.is_empty(), "after delete:\n{}", bad.join("\n"));

    let tombstones = admin
        .execute(&format!(
            "SELECT count(*) FROM {} WHERE \"_ekos_op\" = 'delete'",
            render::target(film, &plan.settings)
        ))
        .unwrap();
    assert_eq!(tombstones.rows[0][0].as_i64(), Some(1));

    // Maintenance runs on every table.
    for t in &plan.tables {
        for stmt in render::statements(&render::maintenance_sql(t, &plan.settings)) {
            admin
                .execute(&stmt)
                .unwrap_or_else(|e| panic!("{stmt}: {e}"));
        }
    }
}
