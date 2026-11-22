//! EKOS RFC 0268 phase 1 on a small project written for the purpose: each data action's rule, a
//! table one job writes read by another as a `ref()`, a job that reads what it writes, a
//! statement, and a source outside the lakehouse.

use ekos_lakehouse_bronze::plan::{Plan, PlanSettings, plan};
use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_talend_dbt::{Rule, Settings, Translation, translate};
use ekos_talend::{ProjectFile, read_project};

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

fn lakehouse() -> Plan {
    plan(
        &[SourceTable {
            schema: "public".into(),
            oid: 0,
            name: "orders".into(),
            columns: vec![
                col("id", "integer", "integer"),
                col("qty", "integer", "integer"),
            ],
            primary_key: vec!["id".into()],
            row_estimate: 10,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }],
        &PlanSettings::default(),
    )
}

fn esc(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn param(name: &str, value: &str) -> String {
    format!(
        r#"<elementParameter field="TEXT" name="{name}" value="{}"/>"#,
        esc(value)
    )
}

fn meta(cols: &[(&str, &str, bool)]) -> String {
    let cols: String = cols
        .iter()
        .map(|(c, t, k)| format!(r#"<column name="{c}" type="{t}" key="{k}"/>"#))
        .collect();
    format!(r#"<metadata connector="FLOW" name="x">{cols}</metadata>"#)
}

fn node(kind: &str, name: &str, extra: &str) -> String {
    format!(
        r#"<node componentName="{kind}"><elementParameter field="TEXT" name="UNIQUE_NAME" value="{name}"/>{extra}</node>"#
    )
}

fn link(label: &str, from: &str, to: &str) -> String {
    format!(
        r#"<connection connectorName="FLOW" label="{label}" metaname="{label}" source="{from}" target="{to}"/>"#
    )
}

fn input(name: &str, table: &str) -> String {
    node(
        "tPostgresqlInput",
        name,
        &format!(
            "{}{}{}",
            param("TABLE", &format!("\"{table}\"")),
            param("QUERY", &format!("\"SELECT id, qty FROM {table}\"")),
            meta(&[("id", "id_Integer", false), ("qty", "id_Integer", false)])
        ),
    )
}

fn output(name: &str, table: &str, action: &str, clear: bool) -> String {
    node(
        "tPostgresqlOutput",
        name,
        &format!(
            "{}{}{}{}",
            param("TABLE", &format!("\"{table}\"")),
            param("DATA_ACTION", action),
            param("TABLE_ACTION", if clear { "CLEAR" } else { "NONE" }),
            meta(&[("id", "id_Integer", true), ("qty", "id_Integer", false)])
        ),
    )
}

fn job(path: &str, body: &str) -> ProjectFile {
    ProjectFile {
        path: path.into(),
        text: format!(
            r#"<talendfile:ProcessType xmlns:talendfile="platform:/resource/org.talend.model/model/TalendFile.xsd" defaultContext="Default"><context name="Default"/>{body}</talendfile:ProcessType>"#
        ),
    }
}

fn shop() -> Translation {
    let files = [
        // Reload: the lakehouse source into a cleared table.
        job(
            "process/jStage_0.1.item",
            &[
                input("tDBInput_1", "orders"),
                output("tDBOutput_1", "stage_orders", "INSERT", true),
                link("row1", "tDBInput_1", "tDBOutput_1"),
            ]
            .concat(),
        ),
        // Merge: what jStage wrote, upserted on its key; an append; a delete; a statement.
        job(
            "process/jLoad_0.1.item",
            &[
                input("tDBInput_1", "stage_orders"),
                output("tDBOutput_1", "orders_dwh", "INSERT_OR_UPDATE", false),
                output("tDBOutput_2", "orders_log", "INSERT", false),
                output("tDBOutput_3", "orders_gone", "DELETE", false),
                node(
                    "tPostgresqlRow",
                    "tDBRow_1",
                    &param(
                        "QUERY",
                        "\"UPDATE cnf.run_control SET last_run = now() WHERE job = 'jLoad'\"",
                    ),
                ),
                link("row1", "tDBInput_1", "tDBOutput_1"),
                link("row2", "tDBInput_1", "tDBOutput_2"),
                link("row3", "tDBInput_1", "tDBOutput_3"),
            ]
            .concat(),
        ),
        // A job that reads the table it writes.
        job(
            "process/jSelf_0.1.item",
            &[
                input("tDBInput_1", "orders_self"),
                output("tDBOutput_1", "orders_self", "INSERT", false),
                link("row1", "tDBInput_1", "tDBOutput_1"),
            ]
            .concat(),
        ),
        // A source the lakehouse does not hold.
        job(
            "process/jOther_0.1.item",
            &[
                input("tDBInput_1", "invoices"),
                output("tDBOutput_1", "invoices_dwh", "INSERT", true),
                link("row1", "tDBInput_1", "tDBOutput_1"),
            ]
            .concat(),
        ),
    ];
    let p = read_project("", &files);
    translate(
        &p,
        &lakehouse(),
        &Settings {
            project: "Shop".into(),
            source_database: None,
        },
    )
}

fn model<'a>(t: &'a Translation, name: &str) -> &'a ekos_lakehouse_talend_dbt::ModelReport {
    t.report
        .models
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| {
            panic!(
                "{name}: {:?}",
                t.report.models.iter().map(|m| &m.name).collect::<Vec<_>>()
            )
        })
}

#[test]
fn each_data_action_has_its_rule() {
    let t = shop();
    let reload = model(&t, "talend__shop__jstage__tdboutput_1");
    assert_eq!(
        (reload.rule, reload.enabled),
        (Rule::Reload, true),
        "{:?}",
        reload.gaps
    );
    assert!(t.files[&reload.path].contains("materialized='table'"));
    let merge = model(&t, "talend__shop__jload__tdboutput_1");
    assert_eq!(
        (merge.rule, merge.enabled),
        (Rule::Merge, true),
        "{:?}",
        merge.gaps
    );
    assert!(t.files[&merge.path].contains("unique_key=['id']"));
    let append = model(&t, "talend__shop__jload__tdboutput_2");
    assert_eq!((append.rule, append.enabled), (Rule::Append, true));
    let delete = model(&t, "talend__shop__jload__tdboutput_3");
    assert_eq!((delete.rule, delete.enabled), (Rule::Delete, false));
    for kind in ["append", "delete"] {
        assert!(t.report.questions.iter().any(|q| q.kind == kind), "{kind}");
    }
    // The key's tests, for enabled models only.
    let yml = &t.files["dbt/models/gold/talend/shop/_jload.yml"];
    assert!(yml.contains("tests: [unique, not_null]"), "{yml}");
    assert!(!yml.contains("tdboutput_3"), "{yml}");
}

#[test]
fn a_table_another_job_writes_is_a_ref_to_its_gold_model() {
    let t = shop();
    let silver = model(&t, "talend__shop__jload__tdbinput_1");
    assert!(silver.enabled, "{:?}", silver.gaps);
    assert!(
        t.files[&silver.path].contains("{{ ref('talend__shop__jstage__tdboutput_1') }}"),
        "{}",
        t.files[&silver.path]
    );
    let source = model(&t, "talend__shop__jstage__tdbinput_1");
    assert!(source.enabled, "{:?}", source.gaps);
}

#[test]
fn a_job_reading_what_it_writes_and_an_unknown_source_are_disabled_with_a_question() {
    let t = shop();
    assert!(!model(&t, "talend__shop__jself__tdboutput_1").enabled);
    assert!(t.report.questions.iter().any(|q| q.kind == "self-read"));
    let other = model(&t, "talend__shop__jother__tdbinput_1");
    assert!(!other.enabled);
    assert!(
        other.gaps[0].contains("not a table of the lakehouse plan"),
        "{:?}",
        other.gaps
    );
    assert!(t.report.questions.iter().any(|q| q.kind == "source"));
}

#[test]
fn a_statement_is_listed_with_no_model_and_bookkeeping_is_asked_about() {
    let t = shop();
    let s = &t.report.statements[0];
    assert_eq!(
        (s.verb.as_str(), s.table.as_deref()),
        ("UPDATE", Some("cnf.run_control"))
    );
    assert!(s.bookkeeping);
    assert!(t.report.questions.iter().any(|q| q.kind == "bookkeeping"));
    assert!(!t.report.models.iter().any(|m| m.name.contains("tdbrow")));
}

#[test]
fn every_enabled_model_has_the_header_and_no_jinja_outside_dbt_forms() {
    let t = shop();
    for m in t.report.models.iter().filter(|m| m.enabled) {
        let sql = &t.files[&m.path];
        assert!(sql.starts_with(ekos_lakehouse_talend_dbt::HEADER));
        assert!(ekos_lakehouse_talend_dbt::gates::raw_java(sql).is_none());
    }
    assert!(t.files.contains_key("talend/shop/translation.json"));
    assert!(t.files["proposals/talend-shop.md"].contains("not verified by execution"));
}
