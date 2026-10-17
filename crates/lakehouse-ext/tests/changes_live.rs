//! RFC 0194 live: every change class driven by real DDL on the sandbox's `m3` database, each
//! checked for its documented action, then every proposal merged as a person would and the
//! loads reconciled green. Needs the stack and `EKOS_LAKEHOUSE_LIVE=1`.

use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_bronze::{LoadMode, render};
use ekos_lakehouse_changes::{Class, Policy};
use ekos_lakehouse_ext::plan_cmd::{build_plan, read_lock, write};
use ekos_lakehouse_ext::reconcile::{Verdict, reconcile};
use ekos_lakehouse_ext::settings::Settings;
use ekos_lakehouse_ext::sync::{SyncInputs, TableState, commit_generated, git, sync};
use ekos_lakehouse_silver::SilverSettings;
use ekos_lakehouse_trino::{Client, TrinoReader};
use std::collections::BTreeMap;
use std::path::Path;

fn live() -> bool {
    std::env::var("EKOS_LAKEHOUSE_LIVE").is_ok_and(|v| v == "1")
}

fn settings() -> Settings {
    Settings {
        trino_url: Some("http://127.0.0.1:58080".into()),
        source: Some("127.0.0.1:55433".into()),
        source_database: Some("m3".into()),
        source_user: Some("ekos".into()),
        source_password_env: Some("EKOS_LH_PG_PASSWORD".into()),
        source_catalog: Some("m3".into()),
        namespace_prefix: Some("bronze_m3_".into()),
        watermarks: BTreeMap::from([("public.orders".into(), "updated".into())]),
        ..Default::default()
    }
}

fn pg() -> postgres::Client {
    let pw = std::env::var("EKOS_LH_PG_PASSWORD").unwrap();
    postgres::Client::connect(
        &format!("host=127.0.0.1 port=55433 user=ekos dbname=m3 password={pw}"),
        postgres::NoTls,
    )
    .unwrap()
}

fn plan_now() -> Plan {
    build_plan(&settings(), &Default::default()).unwrap()
}

/// What the orchestrator does for one table: create + schema (incremental), then load.
fn load(admin: &Client, t: &TablePlan, p: &Plan, run: &str) -> Result<(), String> {
    let s = &p.settings;
    let mut watermark = "NULL".to_string();
    if let LoadMode::Incremental { watermark: w, .. } = &t.decision.mode {
        for f in [render::create_sql(t, s), render::schema_sql(t, s)]
            .into_iter()
            .flatten()
        {
            for stmt in render::statements(&f) {
                admin.execute(&stmt).map_err(|e| format!("{stmt}: {e}"))?;
            }
        }
        let q = render::statements(&render::watermark_sql(t, s).unwrap()).remove(0);
        let r = admin.execute(&q).map_err(|e| e.to_string())?;
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
                .map_err(|e| format!("{}: {e}", t.table))?;
        }
    }
    Ok(())
}

fn green(t: &TablePlan, p: &Plan) {
    let conn = ekos_migrate::connection::ConnectionRef::parse("postgres://127.0.0.1/m3")
        .unwrap()
        .with_secret_env(Some("EKOS_LH_PG_PASSWORD".into()));
    let src = ekos_pg_live::PgSource::connect(
        &conn,
        "127.0.0.1",
        55433,
        "ekos",
        "m3-test",
        &Default::default(),
    )
    .unwrap();
    let tgt = TrinoReader::new(settings().trino_connection().unwrap()).unwrap();
    let r = reconcile(t, &p.settings, &src, &tgt).unwrap();
    assert_eq!(r.verdict, Verdict::Green, "{}: {:?}", t.table, r.tiers);
    assert_eq!(r.control_fired, Some(true), "{}", t.table);
}

fn table<'a>(p: &'a Plan, name: &str) -> &'a TablePlan {
    p.tables
        .iter()
        .find(|t| t.table == name)
        .unwrap_or_else(|| panic!("{name} not planned"))
}

fn run_sync(
    repo: &Path,
    store: &dyn ekos_ledger::KnowledgeStore,
) -> ekos_lakehouse_ext::sync::Outcome {
    sync(SyncInputs {
        repo,
        live: &plan_now(),
        silver: &SilverSettings::default(),
        policy: &Policy::new(),
        store,
        git_remote: None,
    })
    .unwrap()
}

#[test]
fn every_change_class_produces_its_action() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 with the sandbox up");
        return;
    }
    let mut db = pg();
    db.batch_execute(
        "DROP SCHEMA public CASCADE; CREATE SCHEMA public;
         CREATE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.updated := now(); RETURN NEW; END $$;
         CREATE TABLE orders (id int PRIMARY KEY, amount numeric(10,2), qty integer, updated timestamptz NOT NULL DEFAULT now());
         CREATE TRIGGER touch BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION touch();
         INSERT INTO orders SELECT i, i * 1.5, i, now() FROM generate_series(1, 50) i;
         CREATE TABLE t_required (id int PRIMARY KEY, v text);
         CREATE TABLE t_rename (id int PRIMARY KEY, label text);
         CREATE TABLE t_drop (id int PRIMARY KEY, keep text, gone text);
         CREATE TABLE t_narrow (id int PRIMARY KEY, amount numeric(10,2));
         CREATE TABLE t_key (id int PRIMARY KEY, code text NOT NULL);
         INSERT INTO t_required SELECT i, 'v' || i FROM generate_series(1, 20) i;
         INSERT INTO t_rename SELECT i, 'l' || i FROM generate_series(1, 20) i;
         INSERT INTO t_drop SELECT i, 'k' || i, 'g' || i FROM generate_series(1, 20) i;
         INSERT INTO t_narrow SELECT i, i * 0.25 FROM generate_series(1, 20) i;
         INSERT INTO t_key SELECT i, 'c' || i FROM generate_series(1, 20) i;
         ANALYZE;",
    )
    .unwrap();

    let repo = std::env::temp_dir().join(format!("ekos-lh-m3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let ws = repo.with_extension("ws");
    let _ = std::fs::remove_dir_all(&ws);
    std::fs::create_dir_all(&ws).unwrap();
    let store = ekos::commands::store::open_store(&Default::default(), &ws).unwrap();
    let admin = Client::new(settings().trino_connection().unwrap()).unwrap();
    admin
        .execute("CREATE SCHEMA IF NOT EXISTS iceberg.bronze_m3_public")
        .unwrap();

    // ── Round 0: deploy, load, reconcile. ──
    let p0 = plan_now();
    for t in &p0.tables {
        admin
            .execute(&format!(
                "DROP TABLE IF EXISTS {}",
                render::target(t, &p0.settings)
            ))
            .unwrap();
    }
    write(&p0, &SilverSettings::default(), &repo).unwrap();
    commit_generated(&repo, "lakehouse: generate").unwrap();
    for t in &p0.tables {
        load(&admin, t, &p0, "r0").unwrap();
        green(t, &p0);
    }

    // ── Round 1: the three safe classes, applied and committed. ──
    db.batch_execute(
        "CREATE TABLE t_new (id int PRIMARY KEY, label text);
         INSERT INTO t_new SELECT i, 'n' || i FROM generate_series(1, 10) i;
         ALTER TABLE orders ADD COLUMN channel text;
         ALTER TABLE orders ALTER COLUMN qty TYPE bigint;
         UPDATE orders SET channel = 'web', qty = qty + 5000000000 WHERE id <= 5;
         ANALYZE;",
    )
    .unwrap();
    let out = run_sync(&repo, store.as_ref());
    let applied: Vec<Class> = out.applied.iter().map(|c| c.class).collect();
    assert_eq!(
        applied,
        [Class::NewNullableColumn, Class::Widening, Class::NewTable],
        "{:?}",
        out.applied
    );
    assert!(out.proposals.is_empty() && out.committed);
    let p1 = read_lock(&repo).unwrap();
    assert!(
        git(&repo, &["log", "-1", "--format=%an %s"])
            .unwrap()
            .starts_with("ekos-lakehouse lakehouse: apply 3 safe change(s)")
    );
    for name in ["orders", "t_new"] {
        load(&admin, table(&p1, name), &p1, "r1").unwrap();
        green(table(&p1, name), &p1);
    }

    // ── Round 2: the six proposed classes. ──
    db.batch_execute(
        "ALTER TABLE t_required ADD COLUMN tier text NOT NULL DEFAULT 'std';
         ALTER TABLE t_rename RENAME COLUMN label TO title;
         ALTER TABLE t_drop DROP COLUMN gone;
         ALTER TABLE t_narrow ALTER COLUMN amount TYPE numeric(10,4);
         ALTER TABLE t_key DROP CONSTRAINT t_key_pkey, ADD PRIMARY KEY (id, code);
         CREATE OR REPLACE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS
           $$ BEGIN NEW.updated := clock_timestamp(); RETURN NEW; END $$;
         ANALYZE;",
    )
    .unwrap();
    let out = run_sync(&repo, store.as_ref());
    assert!(
        out.applied.is_empty() && !out.committed,
        "nothing is applied: {:?}",
        out.applied
    );
    let mut by_class: BTreeMap<Class, String> = BTreeMap::new();
    for p in &out.proposals {
        for c in &p.changes {
            by_class.insert(c.class, p.table.clone());
        }
    }
    let expected = [
        (
            Class::NewRequiredColumn,
            "public.t_required",
            TableState::Pending,
        ),
        (Class::Rename, "public.t_rename", TableState::Paused),
        (Class::Drop, "public.t_drop", TableState::Paused),
        (Class::Narrowing, "public.t_narrow", TableState::Paused),
        (Class::KeyChange, "public.t_key", TableState::Pending),
        (Class::RoutineChange, "public.orders", TableState::Pending),
    ];
    for (class, t, state) in expected {
        assert_eq!(
            by_class.get(&class).map(String::as_str),
            Some(t),
            "{class:?}: {by_class:?}"
        );
        assert_eq!(out.status[t].0, state, "{t}");
    }
    let deployed = read_lock(&repo).unwrap();
    assert!(
        table(&deployed, "t_rename")
            .columns
            .iter()
            .any(|m| m.column == "label"),
        "main waits for approval"
    );
    // Why paused: the deployed load cannot run against the renamed source.
    assert!(load(&admin, table(&deployed, "t_rename"), &deployed, "r2").is_err());
    // Pending tables keep loading on the deployed schema, and stay green.
    for name in ["t_required", "t_key", "orders"] {
        load(&admin, table(&deployed, name), &deployed, "r2").unwrap();
        green(table(&deployed, name), &deployed);
    }

    // ── Round 3: a person merges each proposal; the rest are rebuilt on the new HEAD. ──
    let mut rebuilt = 0;
    loop {
        let branches = git(
            &repo,
            &[
                "branch",
                "--list",
                "--format=%(refname:short)",
                "ekos/change/*",
            ],
        )
        .unwrap();
        let Some(next) = branches.lines().next().map(str::to_string) else {
            break;
        };
        git(
            &repo,
            &[
                "-c",
                "user.name=a reviewer",
                "-c",
                "user.email=r@localhost",
                "merge",
                "--no-ff",
                "--no-edit",
                &next,
            ],
        )
        .unwrap();
        let out = run_sync(&repo, store.as_ref());
        rebuilt += out.proposals.iter().filter(|p| p.rebuilt).count();
    }
    assert!(
        rebuilt > 0,
        "a merge moves HEAD; the open proposals follow it"
    );
    let out = run_sync(&repo, store.as_ref());
    assert!(
        out.applied.is_empty() && out.proposals.is_empty(),
        "the source matches the deployed plan"
    );
    assert!(out.status.values().all(|(s, _)| *s == TableState::Active));
    let p3 = read_lock(&repo).unwrap();
    for name in [
        "t_required",
        "t_rename",
        "t_drop",
        "t_narrow",
        "t_key",
        "orders",
    ] {
        load(&admin, table(&p3, name), &p3, "r3").unwrap();
        green(table(&p3, name), &p3);
    }
    assert!(
        table(&p3, "t_rename")
            .columns
            .iter()
            .any(|m| m.column == "title")
    );
    assert!(
        table(&p3, "t_narrow")
            .columns
            .iter()
            .any(|m| m.iceberg_type == "decimal(10,4)")
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&ws);
}
