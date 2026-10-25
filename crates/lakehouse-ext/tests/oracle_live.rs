//! RFC 0216 live: Oracle as a lakehouse source, on EKOS's iDempiere testbed
//! (`docker-compose.oracle.yml`). First the dialect — Oracle reproduces every canonical form,
//! hash, prefix and bucket the Rust implementation computes — then the pipeline: plan from the
//! live catalog, run the generated loads through Trino for a set of business tables, reconcile
//! each at V1–V3 with the planted control.
//!
//! Needs the lakehouse stack with the `idempiere` catalog (`deploy/trino/catalog/
//! idempiere.properties`), the testbed, Instant Client on `LD_LIBRARY_PATH`, the crate feature
//! `oracle`, `EKOS_ORACLE_PASSWORD` and `EKOS_LAKEHOUSE_LIVE=1 EKOS_ORACLE_LIVE=1`. Reads only:
//! the source user cannot write, and bronze goes to this test's own namespace.

use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_bronze::{Lossiness, SourceEngine, render};
use ekos_lakehouse_ext::reconcile::{Verdict, reconcile};
use ekos_lakehouse_ext::settings::Settings;
use ekos_lakehouse_ext::source::Source;
use ekos_lakehouse_ext::source_oracle::Oracle;
use ekos_lakehouse_trino::{Client, TrinoReader};
use ekos_migrate_validate::dialect::{
    ColumnRule as R, Dialect, bucket_checksum_query, canon_expr, canon_expr_of, hash_hex_expr,
    prefix60_expr,
};
use ekos_migrate_validate::fixtures;

fn live() -> bool {
    ["EKOS_LAKEHOUSE_LIVE", "EKOS_ORACLE_LIVE"]
        .iter()
        .all(|v| std::env::var(v).is_ok_and(|v| v == "1"))
}

fn database() -> ekos_compiler_core::DatabaseConfig {
    toml::from_str(
        r#"
        name = "idempiere-db"
        kind = "oracle"
        connect = "localhost:51521/XEPDB1"
        user = "ekos_ro"
        password-env = "EKOS_ORACLE_PASSWORD"
        owners = ["ADEMPIERE"]
        "#,
    )
    .unwrap()
}

fn settings() -> Settings {
    Settings {
        trino_url: Some("http://127.0.0.1:58080".into()),
        source_engine: Some(SourceEngine::Oracle),
        source_database: Some("idempiere-db".into()),
        source_catalog: Some("idempiere".into()),
        namespace_prefix: Some("bronze_o216_".into()),
        database: Some(database()),
        ..Default::default()
    }
}

fn oracle() -> Oracle {
    Oracle::connect(&database(), "lakehouse-test").unwrap()
}

/// Oracle literals for the shared fixture cases, hand-written as the Trino ones are. `None`
/// carries the reason in [`UNMAPPED`].
fn oracle_literal(name: &str) -> Option<&'static str> {
    Some(match name {
        "null" => "CAST(NULL AS VARCHAR2(10))",
        "literal_backslash_n" => "'\\N'",
        "double_backslash" => "'\\\\'",
        "embedded_unit_separator" => "('a' || CHR(31) || 'b')",
        "non_ascii" => "'Ünïcödé'",
        "emoji" => "'🦀'",
        "newline" => "('a' || CHR(10) || 'b')",
        "char_trailing_spaces" => "CAST('ab' AS CHAR(5))",
        // A NUMBER(1) flag: Oracle tables have no boolean before 23ai.
        "bool_true" => "1",
        "bool_false" => "0",
        "int_zero" => "0",
        "int_negative" => "-42",
        "int_max" => "9223372036854775807",
        "decimal_trailing_zeros" => "CAST(1.50 AS NUMBER(10,2))",
        "decimal_widened" => "1",
        "decimal_leading_zero" => "0.05",
        "decimal_negative" => "-1.50",
        "decimal_negative_zero" => "-0.00",
        "decimal_large" => "1234567890123456789012.34",
        "timestamp_utc_whole_second" => "TIMESTAMP '2026-09-24 12:30:00.000000 +00:00'",
        "timestamp_utc_microseconds" => "TIMESTAMP '2026-09-24 12:30:00.123456 +00:00'",
        "timestamp_naive" => "TIMESTAMP '1999-12-31 23:59:59.999999'",
        "date_pre_1900" => "DATE '1850-06-15'",
        "date_epoch" => "DATE '1970-01-01'",
        "time_microseconds" => "TIMESTAMP '1970-01-01 01:02:03.000004'",
        "uuid_uppercase_input" => "'A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11'",
        "bytes" => "HEXTORAW('000FFF')",
        "inet_v4" => "'192.168.0.1/32'",
        "inet_v6" => "'2001:db8::1/128'",
        _ => return None,
    })
}

const UNMAPPED: &[(&str, &str)] = &[
    ("empty_string", "Oracle stores '' as NULL"),
    ("bytes_empty", "Oracle stores an empty RAW as NULL"),
    ("interval_months_and_days", "an interval lands as text"),
    ("array_with_null_element", "Oracle tables have no arrays"),
    ("array_nested", "Oracle tables have no arrays"),
    ("array_empty", "Oracle tables have no arrays"),
];

fn rule_of(name: &str) -> R {
    fixtures::live_cases()
        .into_iter()
        .find(|c| c.name == name)
        .map(|c| c.rule)
        .unwrap_or(R::Text)
}

fn expected(name: &str) -> String {
    let (_, value, spec) = fixtures::cases()
        .into_iter()
        .find(|(n, _, _)| *n == name)
        .unwrap();
    ekos_migrate_validate::canon(&value, &spec).unwrap()
}

/// Not live: every shared fixture case is either rendered on Oracle or explained.
#[test]
fn every_fixture_has_an_oracle_literal_or_a_reason() {
    for (name, _, _) in fixtures::cases() {
        let mapped = oracle_literal(name).is_some();
        let explained = UNMAPPED.iter().any(|(n, _)| *n == name);
        assert!(
            mapped ^ explained,
            "{name}: needs exactly one of an Oracle literal or a reason"
        );
    }
}

#[test]
fn oracle_reproduces_every_canonical_form_hash_and_bucket() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 EKOS_ORACLE_LIVE=1 with the testbed up");
        return;
    }
    let o = oracle();
    let r = o.reader();
    let mut wrong = Vec::new();
    let mut checked = 0;
    for (name, _, _) in fixtures::cases() {
        let Some(lit) = oracle_literal(name) else {
            continue;
        };
        let expr = canon_expr_of(Dialect::Oracle, lit, rule_of(name));
        let got = r.one_row(&format!("SELECT {expr} FROM dual")).unwrap();
        if got[0] != expected(name) {
            wrong.push(format!(
                "{name}: oracle {:?}, expected {:?}",
                got[0],
                expected(name)
            ));
        }
        checked += 1;
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(checked, 29, "the oracle live-case count changed");

    // A DATE carries a time; the naive-timestamp rule keeps it.
    let date = canon_expr_of(
        Dialect::Oracle,
        "TO_DATE('2003-11-01 10:20:30', 'YYYY-MM-DD HH24:MI:SS')",
        R::TimestampNaive,
    );
    assert_eq!(
        r.one_row(&format!("SELECT {date} FROM dual")).unwrap()[0],
        "2003-11-01T10:20:30.000000"
    );

    // A value with more digits than the approved scale is never rounded into agreement.
    let over = canon_expr_of(Dialect::Oracle, "1.005", R::Decimal(2));
    assert_eq!(
        r.one_row(&format!("SELECT {over} FROM dual")).unwrap()[0],
        "1.005"
    );

    for text in ["abc", "Ünïcödé\u{1f}🦀", "\\N"] {
        let lit = format!("'{}'", text.replace('\'', "''")).replace('\u{1f}', "' || CHR(31) || '");
        let h = hash_hex_expr(Dialect::Oracle, &lit);
        let row = r
            .one_row(&format!(
                "SELECT {h}, {} FROM dual",
                prefix60_expr(Dialect::Oracle, &h)
            ))
            .unwrap();
        let rust = ekos_migrate_validate::row_hash(text);
        assert_eq!(row[0], rust, "hash of {text:?}");
        assert_eq!(
            row[1],
            ekos_migrate_validate::hash::prefix60(&rust).to_string(),
            "prefix of {text:?}"
        );
    }

    let table = "(SELECT LEVEL AS id, CASE WHEN MOD(LEVEL, 7) = 0 THEN 'seven' ELSE 'other' END \
                 AS txt FROM dual CONNECT BY LEVEL <= 200) t";
    let pk = canon_expr(Dialect::Oracle, "ID", R::Int);
    let cols = vec![pk.clone(), canon_expr(Dialect::Oracle, "TXT", R::Text)];
    let got = r
        .query(&bucket_checksum_query(
            Dialect::Oracle,
            table,
            &pk,
            &cols,
            16,
        ))
        .unwrap();
    let canon: Vec<(String, String)> = (1..=200)
        .map(|i| {
            let s = if i % 7 == 0 { "seven" } else { "other" };
            (i.to_string(), format!("{i}\u{1f}{s}"))
        })
        .collect();
    let rust: Vec<Vec<String>> = ekos_migrate_validate::hash::checksum_buckets(
        canon.iter().map(|(k, r)| (k.as_str(), r.as_str())),
        16,
    )
    .iter()
    .map(|(b, c)| vec![b.to_string(), c.count.to_string(), c.sum.to_string()])
    .collect();
    assert_eq!(got, rust);
}

/// A full load, as the orchestrator runs one.
fn load(admin: &Client, t: &TablePlan, plan: &Plan) {
    let file = render::load_sql(t, &plan.settings);
    for stmt in render::statements(&render::fill(&file, "o216-run-1", "", "NULL")) {
        admin
            .execute(&stmt)
            .unwrap_or_else(|e| panic!("{}.{}: {e}\n{stmt}", t.schema, t.table));
    }
}

/// Business documents and their lines, the master data they reference, a wide table, the
/// dictionary's largest table, a table whose amounts are all NULL and an empty one.
const TABLES: &[&str] = &[
    "AD_CLIENT",
    "AD_ORG",
    "C_BPARTNER",
    "M_PRODUCT",
    "C_ORDER",
    "C_ORDERLINE",
    "C_INVOICE",
    "C_INVOICELINE",
    "C_PAYMENT",
    "M_INOUT",
    "AD_COLUMN",
    "A_ASSET_ADDITION",
];

#[test]
fn idempiere_plans_lands_and_reconciles() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 EKOS_ORACLE_LIVE=1 with the testbed up");
        return;
    }
    let started = std::time::Instant::now();
    let plan = ekos_lakehouse_ext::plan_cmd::build_plan(&settings(), &Default::default()).unwrap();
    eprintln!(
        "plan: {} tables planned, {} refused, {:.1}s",
        plan.tables.len(),
        plan.refused.len(),
        started.elapsed().as_secs_f64()
    );
    assert!(
        plan.tables.len() + plan.refused.len() >= 900,
        "the testbed's ADEMPIERE has 903 tables"
    );
    for r in &plan.refused {
        assert!(
            r.reasons.iter().all(|x| x.contains("(NUMBER)")),
            "{}: {:?}",
            r.table,
            r.reasons
        );
    }
    let admin = Client::new(settings().trino_connection().unwrap()).unwrap();
    let ns = format!("{}ADEMPIERE", plan.settings.namespace_prefix);
    admin
        .execute(&render::statements(&render::namespace_sql(&ns, &plan.settings))[0])
        .unwrap();

    let o = oracle();
    let target = TrinoReader::new(settings().trino_connection().unwrap()).unwrap();
    let mut bad = Vec::new();
    for name in TABLES {
        let t = plan
            .tables
            .iter()
            .find(|t| t.table == *name)
            .unwrap_or_else(|| panic!("{name} is not planned"));
        let started = std::time::Instant::now();
        load(&admin, t, &plan);
        let loaded = started.elapsed();
        let r = reconcile(t, &plan.settings, o.reader(), &target)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let lossy = t
            .columns
            .iter()
            .filter(|m| m.lossiness != Lossiness::Exact)
            .count();
        eprintln!(
            "{name:<18} {} cols ({lossy} not exact), load {:.1}s, reconcile {:.1}s: {:?} \
             control={:?} not compared={:?}",
            t.columns.len(),
            loaded.as_secs_f64(),
            (started.elapsed() - loaded).as_secs_f64(),
            r.verdict,
            r.control_fired,
            r.not_compared
        );
        if r.verdict != Verdict::Green {
            bad.push(format!("{name}: {:?} {:?}", r.verdict, r.tiers));
        }
        let empty = name == &"A_ASSET_ADDITION";
        if r.control_fired != Some(true) && !(empty && r.control_fired.is_none()) {
            bad.push(format!("{name}: control {:?}", r.control_fired));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// SQL as the schema owner, through the testbed container's SQL*Plus: the source user cannot
/// write, by design. Only rows this test planted (ids above 9,000,000) are touched.
fn as_owner(sql: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            "ekos-oracle-xe-1",
            "sqlplus",
            "-s",
            "-L",
            "adempiere/adempiere@localhost:1521/XEPDB1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("WHENEVER SQLERROR EXIT FAILURE\n{sql}\nCOMMIT;\nEXIT\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{sql}\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

fn plant(from: u32, to: u32) {
    as_owner(&format!(
        "INSERT INTO AD_MESSAGE (AD_MESSAGE_ID, AD_CLIENT_ID, AD_ORG_ID, ISACTIVE, CREATED, \
         CREATEDBY, UPDATED, UPDATEDBY, VALUE, MSGTEXT, MSGTYPE, ENTITYTYPE, AD_MESSAGE_UU) \
         SELECT 9000000 + LEVEL, 0, 0, 'Y', SYSDATE, 100, SYSDATE, 100, 'EKOS_TEST_' || LEVEL, \
         'planted by the RFC 0225 live test', 'I', 'U', 'ekos-test-' || LEVEL FROM dual \
         WHERE LEVEL >= {from} CONNECT BY LEVEL <= {to};"
    ));
}

fn unplant(range: &str) {
    as_owner(&format!(
        "DELETE FROM AD_MESSAGE WHERE AD_MESSAGE_ID > 9000000 AND {range};"
    ));
}

/// An incremental run as the orchestrator does it: create, watermark, load.
fn incremental(admin: &Client, t: &TablePlan, plan: &Plan, run: &str) {
    let s = &plan.settings;
    for stmt in render::statements(&render::create_sql(t, s).unwrap()) {
        admin.execute(&stmt).unwrap();
    }
    let q = render::statements(&render::watermark_sql(t, s).unwrap()).remove(0);
    let r = admin.execute(&q).unwrap();
    let text = r.rows[0][0].as_str().map(str::to_string);
    let ty = &t
        .columns
        .iter()
        .find(|m| m.column == "UPDATED")
        .unwrap()
        .iceberg_type;
    let wm = render::watermark_literal(text.as_deref(), ty);
    for stmt in render::statements(&render::fill(&render::load_sql(t, s), run, "", &wm)) {
        admin
            .execute(&stmt)
            .unwrap_or_else(|e| panic!("{e}\n{stmt}"));
    }
}

/// `lakehouse deletes` without the CLI around it: find, guard, tombstone. Returns what was found
/// and whether the guard stopped it.
fn deletes(
    admin: &Client,
    t: &TablePlan,
    plan: &Plan,
    run: &str,
    accept: bool,
) -> (usize, Option<String>) {
    use ekos_lakehouse_ext::deletes;
    let target = TrinoReader::new(settings().trino_connection().unwrap()).unwrap();
    let f = deletes::find(t, &plan.settings, oracle().reader(), &target).unwrap();
    let guard = if accept {
        None
    } else {
        deletes::guard(f.deleted.len(), f.bronze_keys, 5, 10_000)
    };
    eprintln!(
        "deletes {run}: {} of {} buckets differ, {} found, {} unloaded, guard {guard:?}",
        f.differing.len(),
        f.buckets,
        f.deleted.len(),
        f.unloaded
    );
    if guard.is_none() {
        let op = if render::retains_purges(t) {
            "purge"
        } else {
            "delete"
        };
        for sql in deletes::tombstone_statements(t, &plan.settings, op, &f.deleted).unwrap() {
            for stmt in render::statements(&render::fill(&sql, run, "", "NULL")) {
                admin.execute(&stmt).unwrap();
            }
        }
    }
    (f.deleted.len(), guard)
}

fn green(t: &TablePlan, plan: &Plan) -> bool {
    let target = TrinoReader::new(settings().trino_connection().unwrap()).unwrap();
    let r = reconcile(t, &plan.settings, oracle().reader(), &target).unwrap();
    eprintln!("reconcile: {:?} control={:?}", r.verdict, r.control_fired);
    r.verdict == Verdict::Green && r.control_fired == Some(true)
}

fn message_plan(prefix: &str, classes: &[&str]) -> (Plan, TablePlan) {
    let mut s = settings();
    s.namespace_prefix = Some(prefix.into());
    s.application_watermark = Some("UPDATED".into());
    let facts = std::collections::BTreeMap::from([(
        "ADEMPIERE.AD_MESSAGE".to_string(),
        ekos_lakehouse_bronze::WriteFacts {
            update_class: "po_only".into(),
            delete_classes: classes.iter().map(|c| c.to_string()).collect(),
            change_log: None,
        },
    )]);
    let plan =
        ekos_lakehouse_ext::plan_cmd::build_plan_with(&s, &Default::default(), &facts).unwrap();
    let t = plan
        .tables
        .iter()
        .find(|t| t.table == "AD_MESSAGE")
        .unwrap()
        .clone();
    (plan, t)
}

/// RFC 0225 Phase 3: hard deletes with no marker, found by the bucketed key diff. The classes
/// are the testbed's own for `AD_MESSAGE` (`po_only`, `deleted_by_users`); the purge path is
/// driven by declaring the table purged.
#[test]
fn idempiere_deletes_are_found_guarded_and_tombstoned() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 EKOS_ORACLE_LIVE=1 with the testbed up");
        return;
    }
    unplant("1 = 1");
    plant(1, 150);
    let (plan, t) = message_plan("bronze_o225_", &["deleted_by_users"]);
    let dp = t.decision.deletes.clone().expect("a delete plan");
    assert_eq!(dp.kind, "delete");
    assert!(matches!(
        t.decision.mode,
        ekos_lakehouse_bronze::LoadMode::Incremental { .. }
    ));
    let admin = Client::new(settings().trino_connection().unwrap()).unwrap();
    admin
        .execute(&render::statements(&render::namespace_sql(&t.namespace, &plan.settings))[0])
        .unwrap();
    admin
        .execute(&format!(
            "DROP TABLE IF EXISTS {}",
            render::target(&t, &plan.settings)
        ))
        .unwrap();
    incremental(&admin, &t, &plan, "o225-1");
    assert!(green(&t, &plan), "first load");

    // A single delete.
    unplant("AD_MESSAGE_ID = 9000002");
    incremental(&admin, &t, &plan, "o225-2");
    assert_eq!(deletes(&admin, &t, &plan, "o225-2", false), (1, None));
    assert!(green(&t, &plan), "after a single delete");

    // Delete, then the same key again: a new version.
    unplant("AD_MESSAGE_ID = 9000001");
    incremental(&admin, &t, &plan, "o225-3");
    assert_eq!(deletes(&admin, &t, &plan, "o225-3", false), (1, None));
    std::thread::sleep(std::time::Duration::from_secs(1));
    plant(1, 1);
    incremental(&admin, &t, &plan, "o225-4");
    assert_eq!(deletes(&admin, &t, &plan, "o225-4", false), (0, None));
    assert!(green(&t, &plan), "after delete + re-insert");

    // A mass delete: the guard stops it, nothing is applied, until a person accepts it.
    unplant("AD_MESSAGE_ID BETWEEN 9000003 AND 9000150");
    incremental(&admin, &t, &plan, "o225-5");
    let (found, guard) = deletes(&admin, &t, &plan, "o225-5", false);
    assert_eq!(found, 148);
    assert!(
        guard.as_deref().unwrap_or("").contains("148 of"),
        "{guard:?}"
    );
    assert!(
        !green(&t, &plan),
        "the guard applied nothing, so bronze still has the rows"
    );
    assert_eq!(deletes(&admin, &t, &plan, "o225-5", true), (148, None));
    assert!(green(&t, &plan), "after the accepted mass delete");

    // A purge, retained: it leaves bronze's current state as `purge`, history keeps the rows.
    let (pplan, pt) = message_plan("bronze_o225p_", &["purged"]);
    assert!(render::retains_purges(&pt));
    admin
        .execute(&render::statements(&render::namespace_sql(&pt.namespace, &pplan.settings))[0])
        .unwrap();
    admin
        .execute(&format!(
            "DROP TABLE IF EXISTS {}",
            render::target(&pt, &pplan.settings)
        ))
        .unwrap();
    plant(201, 203);
    incremental(&admin, &pt, &pplan, "o225p-1");
    unplant("AD_MESSAGE_ID = 9000201");
    incremental(&admin, &pt, &pplan, "o225p-2");
    assert_eq!(deletes(&admin, &pt, &pplan, "o225p-2", false), (1, None));
    assert!(green(&pt, &pplan), "after a retained purge");
    let purged = admin
        .execute(&format!(
            "SELECT count(*) FROM {} WHERE \"_ekos_op\" = 'purge'",
            render::target(&pt, &pplan.settings)
        ))
        .unwrap();
    assert_eq!(purged.rows[0][0].as_i64(), Some(1));

    unplant("1 = 1");
}
