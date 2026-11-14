use super::*;
use crate::tests::ledger;
use crate::{
    GoldEngine, check, check_draft, check_draft_on, check_on, compile, mart_files, model_yaml,
    parse, proposal_markdown, reconciliation_tests, resolve,
};

const MART: &str = "---
mart: receivables_by_customer_month
grain: [customer, month]
dimensions:
  customer: entity_credit_account.meta_number
  month: month of transactions.transdate
measures:
  invoiced: sum of ar.amount_bc
  invoices: count of ar
  average_invoice: the amount_bc total divided by the number of invoices
filters:
  - transactions.approved is true
---
Receivables per customer and month, approved transactions only.
";

fn compiled() -> (MartSpec, Vec<Resolved>, Compiled, BTreeMap<String, String>) {
    let s = parse(MART).unwrap();
    let r = resolve(&s, &ledger());
    let drafted = BTreeMap::from([(
        "average_invoice".to_string(),
        "sum(\"ar\".\"amount_bc\") / count(*)".to_string(),
    )]);
    let c = compile(&s, &ledger(), &r, &drafted).unwrap();
    (s, r, c, drafted)
}

#[test]
fn iceberg_types_map_to_duckdb_and_unknown_ones_are_not_guessed() {
    for (iceberg, duck) in [
        ("boolean", "BOOLEAN"),
        ("integer", "INTEGER"),
        ("bigint", "BIGINT"),
        ("smallint", "SMALLINT"),
        ("real", "FLOAT"),
        ("double", "DOUBLE"),
        ("decimal(18,2)", "DECIMAL(18,2)"),
        ("decimal(38, 0)", "DECIMAL(38,0)"),
        ("date", "DATE"),
        ("varchar", "VARCHAR"),
        ("varchar(20)", "VARCHAR"),
        ("timestamp(6)", "TIMESTAMP"),
        ("timestamp(6) with time zone", "TIMESTAMPTZ"),
        ("time(6)", "TIME"),
        ("uuid", "UUID"),
        ("varbinary", "BLOB"),
    ] {
        assert_eq!(duckdb_type(iceberg).as_deref(), Some(duck), "{iceberg}");
    }
    for unknown in [
        "map(varchar, varchar)",
        "row(a integer)",
        "decimal(40,2)",
        "json",
    ] {
        assert_eq!(duckdb_type(unknown), None, "{unknown}");
    }
}

/// The mart for DuckDB reads silver as sources from the attached catalog, keeps a gold ref as a
/// ref, and reads — refs and sources replaced by names — as the Trino one does.
#[test]
fn a_mart_is_lowered_to_sources_and_reads_the_same() {
    let (_, _, c, _) = compiled();
    let duck = lower(&c.dbt_sql, &ledger());
    assert!(!duck.contains("ref('public__"), "{duck}");
    assert!(
        duck.contains("from {{ source('silver_public', 'public__ar') }} as \"ar\""),
        "{duck}"
    );
    assert!(duck.contains("left join {{ source('silver_public', 'public__transactions') }}"));
    assert_eq!(
        lower("select * from {{ ref('gold__m') }}", &ledger()),
        "select * from {{ ref('gold__m') }}"
    );
    let names = |s: &str| {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find("{{ ") {
            out.push_str(&rest[..i]);
            let after = &rest[i..];
            let j = after.find(" }}").unwrap() + 3;
            let call = &after[..j];
            let name = call.rsplit('\'').nth(1).unwrap();
            out.push_str(name);
            rest = &after[j..];
        }
        out.push_str(rest);
        out
    };
    assert_eq!(names(&duck), names(&c.dbt_sql));
}

/// RFC 0259 L1: the anti-invention check and the drafting check read the statement in the
/// engine's dialect: an aggregate's `FILTER (WHERE …)` clause, which the DuckDB plugin's parser
/// reads and the Trino plugin's does not, passes for DuckDB only.
#[test]
fn the_checks_parse_in_the_engines_dialect() {
    let (_, r, c, _) = compiled();
    assert!(check_on(GoldEngine::Duckdb, &c).accepted());
    assert_eq!(check_on(GoldEngine::Trino, &c), check(&c));
    let prose = r
        .iter()
        .find(|x| x.term == "average_invoice")
        .unwrap()
        .resolution
        .clone()
        .unwrap();
    let duck_only = "sum(\"ar\".\"amount_bc\") / count(*) FILTER (WHERE true)";
    assert!(check_draft_on(GoldEngine::Duckdb, &prose, duck_only).is_ok());
    assert!(check_draft(&prose, duck_only).is_err());
    assert_eq!(
        check_draft_on(GoldEngine::Trino, &prose, duck_only),
        check_draft(&prose, duck_only)
    );
    // Borrowing another term's column is refused whatever the engine.
    let borrowed = "sum(\"ar\".\"netamount_bc\") / count(*)";
    assert!(check_draft_on(GoldEngine::Duckdb, &prose, borrowed).is_err());
}

/// For Trino, a proposal writes exactly the files RFC 0195 always wrote, byte for byte.
#[test]
fn trino_mart_files_are_unchanged() {
    let (s, r, c, d) = compiled();
    let report = check(&c);
    let files = mart_files(GoldEngine::Trino, &ledger(), &s, &r, &c, &report, &d);
    let mut want = vec![
        (
            "dbt/models/gold/gold__receivables_by_customer_month.sql".to_string(),
            c.dbt_sql.clone(),
        ),
        (
            "dbt/models/gold/_gold__receivables_by_customer_month.yml".to_string(),
            model_yaml(&s, &r),
        ),
    ];
    for (name, sql) in reconciliation_tests(&s, &r, &c) {
        want.push((format!("dbt/tests/gold/{name}.sql"), sql));
    }
    want.push((
        "proposals/gold-receivables_by_customer_month.md".to_string(),
        proposal_markdown(&s, &r, &c, &report, &d),
    ));
    assert_eq!(files, want);
}

#[test]
fn duckdb_mart_files_go_to_the_duckdb_project() {
    let (s, r, c, d) = compiled();
    let report = check_on(GoldEngine::Duckdb, &c);
    let files: BTreeMap<String, String> =
        mart_files(GoldEngine::Duckdb, &ledger(), &s, &r, &c, &report, &d)
            .into_iter()
            .collect();
    let model = &files["gold-duckdb/models/gold/gold__receivables_by_customer_month.sql"];
    assert!(
        model.contains("source('silver_public', 'public__ar')"),
        "{model}"
    );
    assert!(model.contains("schema='gold', alias='receivables_by_customer_month'"));
    let test = files
        .iter()
        .find(|(k, _)| k.starts_with("gold-duckdb/tests/gold/"))
        .unwrap()
        .1;
    assert!(test.contains("ref('gold__receivables_by_customer_month')"));
    assert!(
        test.contains("source('silver_public', 'public__ar')"),
        "{test}"
    );
    assert!(!files.keys().any(|k| k.starts_with("dbt/")), "{files:?}");
    let proposal = &files["proposals/gold-receivables_by_customer_month.md"];
    assert!(proposal.contains("Built on **DuckDB**"));
    assert!(proposal.contains("source('silver_public', 'public__ar')"));
}

/// The project names every secret by its variable and writes no value; the catalog is attached
/// before any model, with the options the spike proved.
#[test]
fn the_project_names_secrets_and_attaches_the_catalog() {
    let s = DuckDbGold {
        s3_key_id_env: "MY_KEY".into(),
        s3_secret_env: "MY_SECRET".into(),
        ..Default::default()
    };
    let f = project_files(&ledger(), &s);
    let profiles = &f["gold-duckdb/profiles.yml"];
    for name in [
        "MY_KEY",
        "MY_SECRET",
        "EKOS_LH_POLARIS_CLIENT_ID",
        "EKOS_LH_POLARIS_CLIENT_SECRET",
    ] {
        assert!(
            profiles.contains(&format!("\"{{{{ env_var('{name}') }}}}\"")),
            "{name}: {profiles}"
        );
    }
    assert!(profiles.contains("env_var('EKOS_LH_MARTS_NEXT', 'marts.next.duckdb')"));
    assert!(profiles.contains("- httpfs\n        - iceberg"));
    assert!(profiles.contains("name: ekos_lh_iceberg"));
    assert!(profiles.contains("/v1/oauth/tokens"));
    let yaml: serde_yaml::Value = serde_yaml::from_str(profiles).unwrap();
    assert_eq!(
        yaml["ekos_lakehouse_gold"]["outputs"]["marts"]["type"],
        serde_yaml::Value::from("duckdb")
    );
    let project = &f["gold-duckdb/dbt_project.yml"];
    assert!(project.contains("on-run-start:\n  - \"{{ ekos_attach_silver() }}\""));
    serde_yaml::from_str::<serde_yaml::Value>(project).unwrap();
    let attach = &f["gold-duckdb/macros/ekos_attach_silver.sql"];
    assert!(attach.contains("AS lh ("));
    assert!(attach.contains("TYPE iceberg"));
    assert!(attach.contains("SECRET ekos_lh_iceberg"));
    assert!(attach.contains("ACCESS_DELEGATION_MODE 'none'"));
    let sources: serde_yaml::Value =
        serde_yaml::from_str(&f["gold-duckdb/models/_silver_sources.yml"]).unwrap();
    let src = &sources["sources"][0];
    assert_eq!(src["name"], serde_yaml::Value::from("silver_public"));
    assert_eq!(src["database"], serde_yaml::Value::from("lh"));
    let ar = src["tables"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "public__ar")
        .unwrap();
    assert_eq!(ar["identifier"], serde_yaml::Value::from("ar"));
}

#[test]
fn an_s3_url_becomes_duckdbs_endpoint() {
    assert_eq!(
        s3_endpoint_of("http://127.0.0.1:58333"),
        ("127.0.0.1:58333".into(), false)
    );
    assert_eq!(
        s3_endpoint_of("https://s3.example.com/"),
        ("s3.example.com".into(), true)
    );
}

/// L5: each additive measure in the canonical form of each engine — the DuckDB side over the
/// marts file, the Trino side over silver under the mart's joins and filters. A drafted measure
/// is not reconciled (its silver aggregate is not known).
#[test]
fn measures_reconcile_across_engines_in_canonical_form() {
    let (s, r, c, _) = compiled();
    let checks = cross_checks(&s, &r, &c, &ledger(), "iceberg");
    let names: Vec<&str> = checks.iter().map(|c| c.measure.as_str()).collect();
    assert_eq!(names, ["invoiced", "invoices"]);
    let invoiced = &checks[0];
    assert_eq!(invoiced.compare, "canonical");
    assert!(
        invoiced
            .mart_sql
            .contains("CAST(CAST(sum(\"invoiced\") AS DECIMAL(38, 2)) AS VARCHAR)"),
        "{}",
        invoiced.mart_sql
    );
    assert!(
        invoiced
            .mart_sql
            .ends_with("FROM \"gold\".\"receivables_by_customer_month\""),
        "{}",
        invoiced.mart_sql
    );
    assert!(
        invoiced
            .silver_sql
            .contains("CAST(CAST(sum(\"ar\".\"amount_bc\") AS decimal(38, 2)) AS varchar)"),
        "{}",
        invoiced.silver_sql
    );
    assert!(
        invoiced
            .silver_sql
            .contains("\"iceberg\".\"silver_public\".\"ar\"")
    );
    assert!(
        invoiced
            .silver_sql
            .contains("where \"transactions\".\"approved\" = true")
    );
    assert!(!invoiced.silver_sql.contains("group by"));
    let invoices = &checks[1];
    assert!(invoices.silver_sql.contains("CAST(count(*) AS varchar)"));
    assert!(agrees(invoiced, "41675667.00", "41675667.00"));
    // A planted one-cent difference is reported.
    assert!(!agrees(invoiced, "41675667.01", "41675667.00"));
    let float = CrossCheck {
        compare: "numeric",
        ..invoiced.clone()
    };
    assert!(agrees(&float, "1.0", "1"));
    assert!(!agrees(&float, "1.5", "1"));
}

/// The pin every DuckDB consumer shares (RFC 0259 §6 open question 2): the Dagster image that
/// writes the marts file pins the same version.
#[test]
fn the_dagster_image_pins_the_shared_duckdb_version() {
    let dockerfile = include_str!("../../../../deploy/dagster/Dockerfile");
    assert!(
        dockerfile.contains(&format!("duckdb=={DUCKDB_VERSION}")),
        "{dockerfile}"
    );
}
