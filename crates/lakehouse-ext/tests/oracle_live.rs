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
