//! RFC 0204 end to end: a package with a dimension, a fact that looks it up, an Abort, and a
//! branch reading a database that is not the lakehouse source.

use ekos_lakehouse_bronze::plan::{Plan, PlanSettings, plan};
use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_kettle::{SourceFile, read_package};
use ekos_lakehouse_kettle_expr::Fidelity;
use ekos_lakehouse_pentaho_dbt::{Layer, Settings, engine_sql, parses_as_trino, translate};
use std::collections::BTreeMap;

fn col(n: &str, pg: &str, trino: &str) -> SourceColumn {
    SourceColumn {
        name: n.into(),
        pg_type: pg.into(),
        kind: TypeKind::Base,
        not_null: false,
        trino_type: Some(trino.into()),
        numeric_used: None,
        comment: None,
        enum_labels: vec![],
        pii_candidate: None,
        infinite_values: None,
        attnum: 0,
        has_default: false,
    }
}

fn table(name: &str, cols: Vec<SourceColumn>) -> SourceTable {
    SourceTable {
        schema: "public".into(),
        oid: 0,
        name: name.into(),
        columns: cols,
        primary_key: vec!["id".into()],
        row_estimate: 10,
        update_triggers: vec![],
        foreign_keys: vec![],
        comment: None,
    }
}

fn shop() -> Plan {
    plan(
        &[
            table(
                "customers",
                vec![
                    col("id", "integer", "integer"),
                    col("name", "text", "varchar"),
                    col("city", "text", "varchar"),
                ],
            ),
            table(
                "orders",
                vec![
                    col("id", "integer", "integer"),
                    col("customer_id", "integer", "integer"),
                    col("ordered_at", "timestamp", "timestamp(6)"),
                    col("qty", "integer", "integer"),
                    col("price", "numeric(10,2)", "decimal(10,2)"),
                    col("status", "text", "varchar"),
                ],
            ),
        ],
        &PlanSettings::default(),
    )
}

const CONN: &str = "<connection><name>src</name><type>POSTGRESQL</type><server>replica</server><database>shop</database></connection>\
                    <connection><name>dwh</name><type>POSTGRESQL</type><database>dwh</database></connection>\
                    <connection><name>crm</name><type>MSSQLNATIVE</type><database>crm</database></connection>";

fn ktr(name: &str, body: &str, hops: &[(&str, &str)]) -> String {
    let hops: String = hops
        .iter()
        .map(|(a, b)| format!("<hop><from>{a}</from><to>{b}</to><enabled>Y</enabled></hop>"))
        .collect();
    format!(
        "<transformation><info><name>{name}</name></info>{CONN}<order>{hops}</order>{body}</transformation>"
    )
}

fn package() -> Vec<SourceFile> {
    let dim = ktr(
        "dim_customer",
        r#"<step><name>customers</name><type>TableInput</type><connection>src</connection><sql>SELECT id, name, city FROM customers</sql></step>
           <step><name>rename</name><type>SelectValues</type><fields><field><name>id</name><rename>customer_id</rename></field><field><name>name</name></field><field><name>city</name></field></fields></step>
           <step><name>dim</name><type>DimensionLookup</type><connection>dwh</connection><table>dim_customer</table><update>Y</update>
             <fields><key><name>customer_id</name><lookup>customer_id</lookup></key><date><name/><from>date_from</from><to>date_to</to></date>
               <field><name>name</name><lookup>name</lookup><update>Insert</update></field>
               <field><name>city</name><lookup>city</lookup><update>Update</update></field>
               <return><name>customer_tk</name><version>version</version></return></fields></step>"#,
        &[("customers", "rename"), ("rename", "dim")],
    );
    let fact = ktr(
        "fact_orders",
        r#"<step><name>orders</name><type>TableInput</type><connection>src</connection><sql>SELECT id, customer_id, ordered_at, qty, price, status FROM public.orders</sql></step>
           <step><name>amount</name><type>Calculator</type><distribute>N</distribute>
             <calculation><field_name>amount</field_name><calc_type>MULTIPLY</calc_type><field_a>qty</field_a><field_b>price</field_b><value_type>BigNumber</value_type></calculation></step>
           <step><name>not void</name><type>FilterRows</type>
             <compare><condition><leftvalue>status</leftvalue><function>&lt;&gt;</function><value><type>String</type><text>void</text></value></condition></compare></step>
           <step><name>customer key</name><type>DimensionLookup</type><connection>dwh</connection><table>dim_customer</table><update>N</update>
             <fields><key><name>customer_id</name><lookup>customer_id</lookup></key><date><name>ordered_at</name><from>date_from</from><to>date_to</to></date>
               <return><name>customer_tk</name></return></fields></step>
           <step><name>out</name><type>TableOutput</type><connection>dwh</connection><table>fact_orders</table><truncate>Y</truncate></step>
           <step><name>negative</name><type>FilterRows</type>
             <compare><condition><leftvalue>amount</leftvalue><function>&lt;</function><value><type>Number</type><text>0</text></value></condition></compare></step>
           <step><name>stop</name><type>Abort</type><message>negative amount</message></step>"#,
        &[
            ("orders", "amount"),
            ("amount", "not void"),
            ("not void", "customer key"),
            ("customer key", "out"),
            ("amount", "negative"),
            ("negative", "stop"),
        ],
    );
    let crm = ktr(
        "crm_leads",
        r#"<step><name>leads</name><type>TableInput</type><connection>crm</connection><sql>SELECT id FROM leads</sql></step>
           <step><name>out</name><type>TableOutput</type><connection>dwh</connection><table>leads</table><truncate>Y</truncate></step>"#,
        &[("leads", "out")],
    );
    vec![
        SourceFile {
            path: "etl/dim_customer.ktr".into(),
            text: dim,
        },
        SourceFile {
            path: "etl/fact_orders.ktr".into(),
            text: fact,
        },
        SourceFile {
            path: "etl/crm_leads.ktr".into(),
            text: crm,
        },
    ]
}

#[test]
fn a_package_becomes_silver_gold_snapshot_and_test_files() {
    let p = read_package(&package(), &[]);
    let plan = shop();
    let t = translate(
        &p,
        &plan,
        &Settings {
            package: "shop etl".into(),
            source_database: Some("shop".into()),
            empty_string_is_null: true,
        },
    );
    let names: Vec<&str> = t.files.keys().map(String::as_str).collect();
    for want in [
        "dbt/models/silver/pentaho/shop_etl/pentaho__shop_etl__fact_orders__orders.sql",
        "dbt/models/gold/pentaho/shop_etl/gold__fact_orders.sql",
        "dbt/models/gold/pentaho/shop_etl/gold__dim_customer.sql",
        "dbt/models/gold/pentaho/shop_etl/pentaho__shop_etl__dim_customer_stage.sql",
        "dbt/snapshots/pentaho/shop_etl/pentaho__shop_etl__dim_customer_snapshot.sql",
        "dbt/tests/pentaho/shop_etl/assert_shop_etl__fact_orders__stop.sql",
        "pentaho/shop_etl/translation.json",
        "proposals/pentaho-shop_etl.md",
    ] {
        assert!(names.contains(&want), "missing {want}; have {names:#?}");
    }
    let fact = &t.files["dbt/models/gold/pentaho/shop_etl/gold__fact_orders.sql"];
    assert!(
        fact.contains("{{ ref('gold__dim_customer') }}"),
        "the fact reads the package's own dimension:\n{fact}"
    );
    assert!(
        fact.contains("{{ ref('pentaho__shop_etl__fact_orders__orders') }}"),
        "{fact}"
    );
    assert!(
        fact.contains("m.\"ordered_at\" >= lk.\"date_from\""),
        "lookup valid at the stream date:\n{fact}"
    );
    assert!(!fact.contains("enabled=false"), "{fact}");
    let silver =
        &t.files["dbt/models/silver/pentaho/shop_etl/pentaho__shop_etl__fact_orders__orders.sql"];
    assert!(
        silver.contains("{{ ref('public__orders') }}") && silver.contains("nullif(\"status\", '')"),
        "{silver}"
    );
    // Every enabled model parses as Trino once refs are resolved.
    let locations: BTreeMap<String, (String, String)> = t
        .report
        .models
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                (
                    if m.layer == Layer::Silver {
                        "silver_pentaho".into()
                    } else {
                        "gold".into()
                    },
                    m.name.clone(),
                ),
            )
        })
        .collect();
    for m in t
        .report
        .models
        .iter()
        .filter(|m| m.enabled && m.layer != Layer::Test)
    {
        let sql = engine_sql(&t.files[&m.path], &locations, &plan);
        parses_as_trino(&sql).unwrap_or_else(|e| panic!("{}: {e}\n{sql}", m.name));
    }
    // The CRM branch is a question, and its gold model is disabled.
    assert!(
        t.report
            .questions
            .iter()
            .any(|q| q.question.contains("not the lakehouse source")),
        "{:#?}",
        t.report.questions
    );
    let leads = t
        .report
        .models
        .iter()
        .find(|m| m.name == "gold__leads")
        .unwrap();
    assert!(!leads.enabled);
    assert!(t.files[&leads.path].contains("config(enabled=false)"));
    // The report has one entry per step, with fidelity.
    let counts = t.report.counts();
    assert!(
        counts.get(&Fidelity::Exact).copied().unwrap_or(0) > 0,
        "{counts:?}"
    );
    let md = &t.files["proposals/pentaho-shop_etl.md"];
    assert!(
        md.contains("merging it is the approval") && md.contains("## Questions"),
        "{md}"
    );
}

#[test]
fn translation_is_deterministic() {
    let p = read_package(&package(), &[]);
    let s = Settings {
        package: "shop".into(),
        source_database: Some("shop".into()),
        empty_string_is_null: true,
    };
    let a = translate(&p, &shop(), &s).files;
    let b = translate(&p, &shop(), &s).files;
    assert_eq!(a, b);
}

#[test]
#[ignore = "prints generated files; run with --ignored --nocapture"]
fn show_generated_files() {
    let p = read_package(&package(), &[]);
    let t = translate(
        &p,
        &shop(),
        &Settings {
            package: "shop".into(),
            source_database: Some("shop".into()),
            empty_string_is_null: true,
        },
    );
    for (path, body) in &t.files {
        if path.ends_with(".sql") {
            println!("==== {path}\n{body}");
        }
    }
}

#[test]
fn every_enabled_model_has_plannable_sql_over_silver_tables_only() {
    let p = read_package(&package(), &[]);
    let plan = shop();
    let t = translate(
        &p,
        &plan,
        &Settings {
            package: "shop".into(),
            source_database: Some("shop".into()),
            empty_string_is_null: true,
        },
    );
    for m in t.report.models.iter().filter(|m| m.enabled) {
        let sql = ekos_lakehouse_pentaho_dbt::plannable_sql(&m.name, &t.files, &plan)
            .unwrap_or_else(|e| panic!("{}: {e}", m.name));
        assert!(!sql.contains("{{"), "{}: jinja left:\n{sql}", m.name);
        parses_as_trino(&sql).unwrap_or_else(|e| panic!("{}: {e}\n{sql}", m.name));
        assert!(
            !sql.contains("silver_pentaho"),
            "package models are inlined:\n{sql}"
        );
    }
}
