//! The live suite (RFC 0190 M0): Trino against the golden canonical forms, the Rust hashes and
//! a real PostgreSQL → Iceberg copy. Needs the sandbox (`deploy/docker-compose.lakehouse.yml`)
//! and `EKOS_LAKEHOUSE_LIVE=1`; every test here is a no-op without it, except the coverage guard.

use ekos_lakehouse_trino::{Client, Connection, TrinoReader};
use ekos_migrate_validate::dialect::{
    ColumnRule as R, Dialect, bucket_checksum_query, canon_expr, canon_expr_of, hash_hex_expr,
    prefix60_expr,
};
use ekos_migrate_validate::tiers::{UnitPlan, run_v1, run_v2, run_v3};
use ekos_migrate_validate::{EngineReader, ReadError, fixtures};
use std::cell::RefCell;

fn live() -> bool {
    std::env::var("EKOS_LAKEHOUSE_LIVE").is_ok_and(|v| v == "1")
}

fn trino_url() -> String {
    std::env::var("EKOS_LH_TRINO_URL").unwrap_or_else(|_| "http://127.0.0.1:58080".into())
}

fn conn(catalog: Option<&str>) -> Connection {
    Connection {
        base_url: trino_url(),
        user: "ekos".into(),
        catalog: catalog.map(str::to_string),
        schema: None,
    }
}

/// Trino typed literals for the shared fixture cases. Hand-written, as the PostgreSQL and
/// ClickHouse ones in `ekos_migrate_validate::fixtures` are: generating them from the code under
/// test would make the check circular. `None` carries the reason in [`UNMAPPED`].
fn trino_literal(name: &str) -> Option<&'static str> {
    Some(match name {
        "null" => "CAST(NULL AS varchar)",
        "empty_string" => "CAST('' AS varchar)",
        // Trino literals take a backslash literally.
        "literal_backslash_n" => "CAST('\\N' AS varchar)",
        "double_backslash" => "CAST('\\\\' AS varchar)",
        "embedded_unit_separator" => "('a' || chr(31) || 'b')",
        "non_ascii" => "CAST('Ünïcödé' AS varchar)",
        "emoji" => "CAST('🦀' AS varchar)",
        "newline" => "('a' || chr(10) || 'b')",
        // Iceberg has no `char`; a char(n) source lands as varchar with its padding.
        "char_trailing_spaces" => "CAST('ab   ' AS varchar)",
        "bool_true" => "true",
        "bool_false" => "false",
        "int_zero" => "BIGINT '0'",
        "int_negative" => "BIGINT '-42'",
        "int_max" => "BIGINT '9223372036854775807'",
        "decimal_trailing_zeros" => "DECIMAL '1.50'",
        "decimal_widened" => "DECIMAL '1'",
        "decimal_leading_zero" => "DECIMAL '0.05'",
        "decimal_negative" => "DECIMAL '-1.50'",
        "decimal_negative_zero" => "DECIMAL '-0.00'",
        "decimal_large" => "DECIMAL '1234567890123456789012.34'",
        "timestamp_utc_whole_second" => "TIMESTAMP '2026-09-24 12:30:00.000000 UTC'",
        "timestamp_utc_microseconds" => "TIMESTAMP '2026-09-24 12:30:00.123456 UTC'",
        "timestamp_naive" => "TIMESTAMP '1999-12-31 23:59:59.999999'",
        "date_pre_1900" => "DATE '1850-06-15'",
        "date_epoch" => "DATE '1970-01-01'",
        "time_microseconds" => "TIME '01:02:03.000004'",
        "uuid_uppercase_input" => "UUID 'A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11'",
        "bytes" => "X'000FFF'",
        "bytes_empty" => "X''",
        // `inet` has no Iceberg type; it lands as varchar.
        "inet_v4" => "CAST('192.168.0.1/32' AS varchar)",
        "inet_v6" => "CAST('2001:db8::1/128' AS varchar)",
        _ => return None,
    })
}

const UNMAPPED: &[(&str, &str)] = &[
    (
        "interval_months_and_days",
        "Iceberg has no interval type; the mapping registry (M1) decides its landing shape",
    ),
    (
        "array_with_null_element",
        "array rendering needs the M1 type mapping",
    ),
    ("array_nested", "array rendering needs the M1 type mapping"),
    (
        "array_empty",
        "an untyped empty array literal has no element type",
    ),
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

/// Not live: every shared fixture case is either rendered on Trino or explained.
#[test]
fn every_fixture_has_a_trino_literal_or_a_reason() {
    for (name, _, _) in fixtures::cases() {
        let mapped = trino_literal(name).is_some();
        let explained = UNMAPPED.iter().any(|(n, _)| *n == name);
        assert!(
            mapped ^ explained,
            "{name}: needs exactly one of a Trino literal or a reason"
        );
    }
}

#[test]
fn trino_reproduces_every_canonical_form() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 with the sandbox up");
        return;
    }
    let reader = TrinoReader::new(conn(None)).unwrap();
    let mut wrong = Vec::new();
    let mut checked = 0;
    for (name, _, _) in fixtures::cases() {
        let Some(lit) = trino_literal(name) else {
            continue;
        };
        let expr = canon_expr_of(Dialect::Trino, lit, rule_of(name));
        let got = reader.one_row(&format!("SELECT {expr}")).unwrap();
        if got[0] != expected(name) {
            wrong.push(format!(
                "{name}: trino {:?}, expected {:?}",
                got[0],
                expected(name)
            ));
        }
        checked += 1;
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(checked, 31, "the trino live-case count changed");
}

#[test]
fn hashes_and_prefixes_agree_with_rust() {
    if !live() {
        return;
    }
    let reader = TrinoReader::new(conn(None)).unwrap();
    for text in ["", "abc", "Ünïcödé\u{1f}🦀", "\\N"] {
        let lit = format!("'{}'", text.replace('\'', "''"));
        let lit = lit.replace('\u{1f}', "' || chr(31) || '");
        let h = hash_hex_expr(Dialect::Trino, &lit);
        let row = reader
            .one_row(&format!(
                "SELECT {h}, {}",
                prefix60_expr(Dialect::Trino, &h)
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
}

#[test]
fn bucket_checksums_agree_with_rust() {
    if !live() {
        return;
    }
    let reader = TrinoReader::new(conn(None)).unwrap();
    let rows: Vec<(i64, &str)> = (1..=200)
        .map(|i| (i, if i % 7 == 0 { "seven" } else { "other" }))
        .collect();
    let values = rows
        .iter()
        .map(|(i, s)| format!("(BIGINT '{i}', '{s}')"))
        .collect::<Vec<_>>()
        .join(", ");
    let table = format!("(VALUES {values}) AS t(id, txt)");
    let pk = canon_expr(Dialect::Trino, "id", R::Int);
    let cols = vec![pk.clone(), canon_expr(Dialect::Trino, "txt", R::Text)];
    let got = reader
        .query(&bucket_checksum_query(
            Dialect::Trino,
            &table,
            &pk,
            &cols,
            16,
        ))
        .unwrap();

    let canon: Vec<(String, String)> = rows
        .iter()
        .map(|(i, s)| (i.to_string(), format!("{i}\u{1f}{s}")))
        .collect();
    let rust = ekos_migrate_validate::hash::checksum_buckets(
        canon.iter().map(|(k, r)| (k.as_str(), r.as_str())),
        16,
    );
    let rust: Vec<Vec<String>> = rust
        .iter()
        .map(|(b, c)| vec![b.to_string(), c.count.to_string(), c.sum.to_string()])
        .collect();
    assert_eq!(got, rust);
}

/// PostgreSQL read directly — the source side of the oracle.
struct Pg(RefCell<postgres::Client>);

impl EngineReader for Pg {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }
    fn label(&self) -> &str {
        "postgres:direct"
    }
    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, ReadError> {
        let msgs = self
            .0
            .borrow_mut()
            .simple_query(sql)
            .map_err(|e| ReadError::Query {
                engine: "postgres".into(),
                message: format!("{e:?}"),
            })?;
        Ok(msgs
            .into_iter()
            .filter_map(|m| match m {
                postgres::SimpleQueryMessage::Row(r) => Some(
                    (0..r.len())
                        .map(|i| r.get(i).unwrap_or_default().to_string())
                        .collect(),
                ),
                _ => None,
            })
            .collect())
    }
}

fn pg() -> Pg {
    let password =
        std::env::var("EKOS_LH_PG_PASSWORD").unwrap_or_else(|_| "ekos-local-only".into());
    let client = postgres::Client::connect(
        &format!("host=127.0.0.1 port=55433 user=ekos dbname=source password={password}"),
        postgres::NoTls,
    )
    .expect("the sandbox PostgreSQL on 55433");
    Pg(RefCell::new(client))
}

/// The M0 exit: a PostgreSQL table copied to Iceberg through Trino reconciles at V1–V3, and one
/// changed value in the copy fails V3 (and only V2/V3: the count is unchanged).
#[test]
fn v3_catches_a_planted_one_row_defect() {
    if !live() {
        return;
    }
    // Setup goes through the plain client: it writes, which the read path refuses.
    let admin = Client::new(conn(None)).unwrap();
    for sql in [
        "DROP TABLE IF EXISTS postgresql.public.m0_orders",
        "CREATE TABLE postgresql.public.m0_orders \
         (id bigint, status varchar, amount decimal(12,2), placed timestamp(6))",
        "INSERT INTO postgresql.public.m0_orders \
         SELECT i, CASE WHEN i % 3 = 0 THEN 'open' ELSE 'sent' END, \
                CAST(i AS decimal(12,2)) / 4, \
                TIMESTAMP '2026-01-01 00:00:00.000001' + i * INTERVAL '1' MINUTE \
         FROM UNNEST(sequence(1, 2000)) AS t(i)",
        "CREATE SCHEMA IF NOT EXISTS iceberg.bronze",
        "DROP TABLE IF EXISTS iceberg.bronze.m0_orders",
        "CREATE TABLE iceberg.bronze.m0_orders AS SELECT * FROM postgresql.public.m0_orders",
    ] {
        admin.execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    let rules = [
        ("id", R::Int),
        ("status", R::Text),
        ("amount", R::Decimal(2)),
        ("placed", R::TimestampNaive),
    ];
    let plan = UnitPlan {
        unit: "public.m0_orders".into(),
        source_table: "public.m0_orders".into(),
        target_table: "iceberg.bronze.m0_orders".into(),
        source_pk: canon_expr(Dialect::Postgres, "id", R::Int),
        target_pk: canon_expr(Dialect::Trino, "id", R::Int),
        source_columns: rules
            .iter()
            .map(|(c, r)| canon_expr(Dialect::Postgres, c, *r))
            .collect(),
        target_columns: rules
            .iter()
            .map(|(c, r)| canon_expr(Dialect::Trino, c, *r))
            .collect(),
        buckets: 64,
        digest_columns: false,
    };
    let source = pg();
    let target = TrinoReader::new(conn(None)).unwrap();
    let names: Vec<String> = rules.iter().map(|(c, _)| c.to_string()).collect();

    for (tier, outcome) in [
        ("V1", run_v1(&plan, &source, &target).unwrap()),
        ("V2", run_v2(&plan, &source, &target, &names).unwrap()),
        ("V3", run_v3(&plan, &source, &target).unwrap()),
    ] {
        assert!(
            outcome.divergences.is_empty(),
            "{tier} on a faithful copy: {:?}",
            outcome.divergences
        );
    }

    admin
        .execute("UPDATE iceberg.bronze.m0_orders SET status = 'opem' WHERE id = 1500")
        .unwrap();
    let v1 = run_v1(&plan, &source, &target).unwrap();
    assert!(v1.divergences.is_empty(), "the count did not change");
    let v3 = run_v3(&plan, &source, &target).unwrap();
    assert!(
        !v3.divergences.is_empty(),
        "V3 missed a changed value in one row of 2000"
    );
    eprintln!("V3 on the planted defect: {:?}", v3.divergences);
}
