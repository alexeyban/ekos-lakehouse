//! EKOS RFC 0259 §5 (L1–L5), live: a plan with `engine = "duckdb"` generates a dbt-duckdb project
//! over a silver table in the Iceberg catalog; dbt-duckdb builds the mart into
//! `marts.next.duckdb`, its tests run, each measure is reconciled with silver in Trino, and the
//! file is published. A planted one-cent difference is reported and leaves the published file
//! byte for byte as it was.
//!
//! Needs `EKOS_LH_DUCKDB_LIVE=1` and a stack: Trino with an Iceberg catalog `iceberg` holding
//! `silver.c_invoice` (the RFC 0259 spike's table), the Polaris REST catalog, S3. Settings, all
//! with the spike stack's defaults:
//!
//! - `EKOS_LH_DUCKDB_TRINO` (`http://127.0.0.1:59080`), `EKOS_LH_DUCKDB_CATALOG_URL`
//!   (`http://127.0.0.1:59181/api/catalog`), `EKOS_LH_DUCKDB_S3` (`127.0.0.1:59333`);
//! - `EKOS_LH_DBT` — a `dbt` with dbt-duckdb (default `dbt`), `EKOS_LH_DUCKDB_PYTHON` — a Python
//!   with `duckdb` (default `python3`);
//! - the credentials in the variables the generated profile names: `AWS_ACCESS_KEY_ID`,
//!   `AWS_SECRET_ACCESS_KEY`, `EKOS_LH_POLARIS_CLIENT_ID`, `EKOS_LH_POLARIS_CLIENT_SECRET`.
//!
//! It creates `iceberg.silver_rfc0259.c_invoice` from `iceberg.silver.c_invoice` once (a scratch
//! schema), and nothing else in the catalog.

use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{GoldEngine, SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_ext::gold_duckdb::{
    Build, DuckDbRunner, LiveSteps, MartToCheck, MartTotals, Paths, Steps, publish,
};
use ekos_lakehouse_gold::{self as gold, duckdb::DuckDbGold};
use std::collections::BTreeMap;
use std::path::Path;

fn live() -> bool {
    std::env::var("EKOS_LH_DUCKDB_LIVE").is_ok_and(|v| v == "1")
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn trino() -> ekos_lakehouse_trino::Connection {
    ekos_lakehouse_trino::Connection {
        base_url: env_or("EKOS_LH_DUCKDB_TRINO", "http://127.0.0.1:59080"),
        user: "ekos".into(),
        catalog: None,
        schema: None,
    }
}

fn col(name: &str, trino_type: &str) -> SourceColumn {
    SourceColumn {
        name: name.into(),
        pg_type: "x".into(),
        kind: TypeKind::Base,
        not_null: false,
        trino_type: Some(trino_type.into()),
        numeric_used: None,
        comment: None,
        enum_labels: vec![],
        pii_candidate: None,
        infinite_values: None,
        attnum: 0,
        has_default: false,
    }
}

const MART: &str = "---
mart: invoices_by_status
grain: [status, month]
dimensions:
  status: c_invoice.docstatus
  month: month of c_invoice.dateinvoiced
measures:
  total: sum of c_invoice.grandtotal
  invoices: count of c_invoice
filters:
  - c_invoice.issotrx is true
---
Sales invoices per document status and month.
";

/// The live steps, with a one-cent difference written into the built mart before it is
/// reconciled: the reconciliation must report it, and the published file must stay as it was.
struct Planted<'a> {
    inner: LiveSteps,
    python: &'a str,
}

impl Steps for Planted<'_> {
    fn build(&mut self, next: &Path) -> anyhow::Result<Build> {
        let b = self.inner.build(next)?;
        let out = std::process::Command::new(self.python)
            .args(["-I", "-c"])
            .arg(
                "import duckdb, sys\n\
                 c = duckdb.connect(sys.argv[1])\n\
                 c.execute(\"UPDATE gold.invoices_by_status SET total = total + 0.01 \
                 WHERE rowid = (SELECT min(rowid) FROM gold.invoices_by_status)\")\n\
                 c.execute('CHECKPOINT')\n\
                 c.close()\n",
            )
            .arg(next)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(b)
    }
    fn reconcile(&mut self, next: &Path) -> anyhow::Result<Vec<MartTotals>> {
        self.inner.reconcile(next)
    }
}

fn sha(p: &Path) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()))
}

#[test]
fn a_duckdb_mart_builds_reconciles_and_publishes() {
    if !live() {
        eprintln!("skipped: EKOS_LH_DUCKDB_LIVE=1 runs it against the spike stack");
        return;
    }
    // Silver, as the dbt-trino project would have written it: a scratch copy of the spike table.
    let client = ekos_lakehouse_trino::Client::new(trino()).unwrap();
    client
        .execute("CREATE SCHEMA IF NOT EXISTS iceberg.silver_rfc0259")
        .unwrap();
    client
        .execute(
            "CREATE TABLE IF NOT EXISTS iceberg.silver_rfc0259.c_invoice AS \
             SELECT * FROM iceberg.silver.c_invoice",
        )
        .unwrap();

    let sources = [SourceTable {
        schema: "rfc0259".into(),
        oid: 0,
        name: "c_invoice".into(),
        columns: vec![
            col("c_invoice_id", "bigint"),
            col("documentno", "varchar"),
            col("docstatus", "varchar"),
            col("dateinvoiced", "date"),
            col("grandtotal", "decimal(18,2)"),
            col("issotrx", "boolean"),
            col("updated", "timestamp(6)"),
            col("updated_tz", "timestamp(6) with time zone"),
        ],
        primary_key: vec!["c_invoice_id".into()],
        row_estimate: 6667,
        update_triggers: vec![],
        foreign_keys: vec![],
        comment: None,
    }];
    let p = plan(
        &sources,
        &PlanSettings {
            gold_engine: GoldEngine::Duckdb,
            ..Default::default()
        },
    );
    assert_eq!(p.tables.len(), 1, "{:?}", p.refused);

    let repo = std::env::temp_dir().join(format!("ekos-lh-duckdb-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    let duck = DuckDbGold {
        catalog_url: env_or(
            "EKOS_LH_DUCKDB_CATALOG_URL",
            "http://127.0.0.1:59181/api/catalog",
        ),
        s3_endpoint: env_or("EKOS_LH_DUCKDB_S3", "127.0.0.1:59333"),
        ..Default::default()
    };
    ekos_lakehouse_ext::plan_cmd::write(&p, &Default::default(), &duck, &repo).unwrap();
    assert!(repo.join("gold-duckdb/profiles.yml").is_file());

    let spec = gold::parse(MART).unwrap();
    let resolved = gold::resolve(&spec, &p);
    assert!(gold::questions(&resolved).is_empty());
    let c = gold::compile(&spec, &p, &resolved, &BTreeMap::new()).unwrap();
    let report = gold::check_on(GoldEngine::Duckdb, &c);
    assert!(report.accepted(), "{report:?}");
    for (path, body) in gold::mart_files(
        GoldEngine::Duckdb,
        &p,
        &spec,
        &resolved,
        &c,
        &report,
        &BTreeMap::new(),
    ) {
        let f = repo.join(path);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, body).unwrap();
    }

    let python = env_or("EKOS_LH_DUCKDB_PYTHON", "python3");
    let steps = |target: &str| LiveSteps {
        repo: repo.clone(),
        dbt: env_or("EKOS_LH_DBT", "dbt"),
        duckdb: DuckDbRunner {
            python: python.clone(),
        },
        trino: trino(),
        plan: p.clone(),
        marts: vec![MartToCheck {
            spec: spec.clone(),
            resolved: resolved.clone(),
            compiled: c.clone(),
        }],
        target: repo.join(target),
    };
    let paths = Paths::of(&repo.join(".lakehouse/marts/marts.duckdb"));

    // Build, test, reconcile, publish.
    let first = publish(&paths, &mut steps("target-1"));
    assert!(first.published, "{:?}", first.refused);
    let run = first
        .reports
        .iter()
        .find(|r| r.table == "gold.invoices_by_status")
        .unwrap();
    assert_eq!(run.status, "ok");
    let totals = &first.reconciliations[0].totals;
    for t in totals {
        assert!(t.equal, "{t:?}");
    }
    eprintln!(
        "published {} ({} bytes): mart rows {:?}; {}",
        paths.published.display(),
        first.bytes.unwrap(),
        run.rows,
        totals
            .iter()
            .map(|t| format!(
                "{} mart {} / silver {}",
                t.measure, t.mart_total, t.silver_total
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    let total = totals.iter().find(|t| t.measure == "total").unwrap();
    assert_eq!(total.silver_total, "20838167.00");
    let invoices = totals.iter().find(|t| t.measure == "invoices").unwrap();
    assert_eq!(invoices.mart_total, "3333");

    // Nothing secret is in the published file: the secrets were temporary, named in profiles.yml.
    let bytes = std::fs::read(&paths.published).unwrap();
    for var in ["AWS_SECRET_ACCESS_KEY", "EKOS_LH_POLARIS_CLIENT_SECRET"] {
        if let Ok(v) = std::env::var(var) {
            assert!(
                !bytes.windows(v.len()).any(|w| w == v.as_bytes()),
                "{var}'s value is in the marts file"
            );
        }
    }
    let profiles = std::fs::read_to_string(repo.join("gold-duckdb/profiles.yml")).unwrap();
    for var in ["AWS_SECRET_ACCESS_KEY", "EKOS_LH_POLARIS_CLIENT_SECRET"] {
        if let Ok(v) = std::env::var(var) {
            assert!(!profiles.contains(&v), "{var}'s value is in profiles.yml");
        }
    }

    // The published file answers read-only, with external access off.
    let rows = DuckDbRunner {
        python: python.clone(),
    }
    .first_rows(
        &paths.published,
        &["SELECT count(*), CAST(sum(total) AS VARCHAR) FROM gold.invoices_by_status".into()],
    )
    .unwrap();
    eprintln!("published mart: {:?}", rows[0]);
    assert_eq!(run.rows, Some(20));
    assert_eq!(
        rows[0],
        [Some("20".to_string()), Some("20838167.00".to_string())]
    );

    // A planted one-cent difference: reported, and the published file is left as it was.
    let before = sha(&paths.published);
    let mut planted = Planted {
        inner: steps("target-2"),
        python: &python,
    };
    let second = publish(&paths, &mut planted);
    assert!(!second.published);
    let (step, why) = second.refused.clone().unwrap();
    assert_eq!(step, "reconcile", "{why}");
    assert!(why.contains("invoices_by_status.total"), "{why}");
    eprintln!("planted difference refused: {why}");
    assert_eq!(sha(&paths.published), before);
    assert!(
        second
            .reports
            .iter()
            .filter(|r| r.layer == "gold")
            .all(|r| r.status == "failed")
    );
    let _ = std::fs::remove_dir_all(&repo);
}
