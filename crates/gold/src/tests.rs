use super::*;
use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{ForeignKey, SourceColumn, SourceTable, TypeKind};

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

fn table(name: &str, cols: Vec<SourceColumn>, fks: Vec<(&str, &str, &str)>) -> SourceTable {
    SourceTable {
        schema: "public".into(),
        oid: 0,
        name: name.into(),
        columns: cols,
        primary_key: vec!["id".into()],
        row_estimate: 10,
        update_triggers: vec![],
        foreign_keys: fks
            .into_iter()
            .map(|(c, t, rc)| ForeignKey {
                name: format!("{name}_{c}_fkey"),
                columns: vec![c.into()],
                ref_schema: "public".into(),
                ref_table: t.into(),
                ref_columns: vec![rc.into()],
            })
            .collect(),
        comment: None,
    }
}

/// LedgerSMB's shape: ar → transactions (date, approval) and ar → entity_credit_account.
fn ledger() -> Plan {
    plan(
        &[
            table(
                "ar",
                vec![
                    col("id", "integer", "integer"),
                    col("trans_id", "integer", "integer"),
                    col("entity_credit_account", "integer", "integer"),
                    col("amount_bc", "numeric(20,2)", "decimal(20,2)"),
                    col("netamount_bc", "numeric(20,2)", "decimal(20,2)"),
                ],
                vec![
                    ("trans_id", "transactions", "id"),
                    ("entity_credit_account", "entity_credit_account", "id"),
                ],
            ),
            table(
                "transactions",
                vec![
                    col("id", "integer", "integer"),
                    col("transdate", "date", "date"),
                    col("approved", "boolean", "boolean"),
                ],
                vec![],
            ),
            table(
                "entity_credit_account",
                vec![
                    col("id", "integer", "integer"),
                    col("meta_number", "text", "varchar"),
                ],
                vec![],
            ),
        ],
        &PlanSettings::default(),
    )
}

const RECEIVABLES: &str = "---
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

#[test]
fn a_description_parses_in_order() {
    let s = parse(RECEIVABLES).unwrap();
    assert_eq!(s.mart, "receivables_by_customer_month");
    assert_eq!(s.dimensions[0].0, "customer");
    assert_eq!(s.measures.len(), 3);
    assert_eq!(s.filters, ["transactions.approved is true"]);
    assert!(s.prose.starts_with("Receivables"));
    assert!(parse("no header").is_err());
}

#[test]
fn terms_resolve_or_become_questions() {
    let s = parse(RECEIVABLES).unwrap();
    let r = resolve(&s, &ledger());
    assert!(questions(&r).is_empty(), "{:?}", questions(&r));
    let avg = r.iter().find(|x| x.term == "average_invoice").unwrap();
    match &avg.resolution {
        Ok(Term::Prose { mentions, .. }) => {
            assert_eq!(mentions[0].column, "amount_bc", "{mentions:?}")
        }
        other => panic!("{other:?}"),
    }
    let bad = parse(
        &RECEIVABLES
            .replace("entity_credit_account.meta_number", "region")
            .replace("ar.amount_bc", "ar.amount"),
    )
    .unwrap();
    let qs = questions(&resolve(&bad, &ledger()))
        .into_iter()
        .map(|(r, q)| (r.term.clone(), q.text.clone()))
        .collect::<Vec<_>>();
    assert!(
        qs.iter()
            .any(|(t, q)| t == "customer" && q.contains("no silver column is named `region`")),
        "{qs:?}"
    );
    assert!(
        qs.iter()
            .any(|(t, q)| t == "invoiced" && q.contains("`ar` has no column `amount`")),
        "{qs:?}"
    );
    // A bare word that names a column in several tables is asked, with the candidates.
    let ambiguous = parse(&RECEIVABLES.replace("entity_credit_account.meta_number", "id")).unwrap();
    let resolved = resolve(&ambiguous, &ledger());
    let (_, q) = questions(&resolved)[0];
    assert_eq!(q.candidates.len(), 3, "{q:?}");
}

#[test]
fn a_grain_term_that_is_not_a_dimension_is_asked() {
    let s = parse(&RECEIVABLES.replace("grain: [customer, month]", "grain: [customer, region]"))
        .unwrap();
    assert!(
        questions(&resolve(&s, &ledger()))
            .iter()
            .any(|(r, _)| r.term == "region")
    );
}

#[test]
fn compiled_sql_joins_many_to_one_and_invents_nothing() {
    let s = parse(RECEIVABLES).unwrap();
    let r = resolve(&s, &ledger());
    let drafted = BTreeMap::from([(
        "average_invoice".to_string(),
        "sum(\"ar\".\"amount_bc\") / count(*)".to_string(),
    )]);
    let c = compile(&s, &ledger(), &r, &drafted).unwrap();
    assert_eq!(c.base, "ar");
    assert!(
        c.dbt_sql.contains("from {{ ref('public__ar') }} as \"ar\""),
        "{}",
        c.dbt_sql
    );
    assert!(c.dbt_sql.contains("left join {{ ref('public__transactions') }} as \"transactions\"\n    on \"ar\".\"trans_id\" = \"transactions\".\"id\""), "{}", c.dbt_sql);
    assert!(
        c.dbt_sql
            .contains("date_trunc('month', \"transactions\".\"transdate\") as \"month\"")
    );
    assert!(
        c.dbt_sql
            .contains("where \"transactions\".\"approved\" = true")
    );
    assert!(
        c.dbt_sql
            .contains("group by \"entity_credit_account\".\"meta_number\", date_trunc('month'"),
        "{}",
        c.dbt_sql
    );
    let report = check(&c);
    assert!(report.accepted(), "{report:#?}\n{}", c.source_sql);
    assert!(report.checked > 10);
}

#[test]
fn a_draft_that_invents_is_rejected_and_named() {
    let s = parse(RECEIVABLES).unwrap();
    let r = resolve(&s, &ledger());
    for (invented, atom) in [
        ("sum(\"ar\".\"amount_bc\") / count(*) * 1.2", "1.2"),
        (
            "sum(case when \"ar\".\"amount_bc\" > 0 then \"ar\".\"amount_bc\" end) / count(*)",
            "amount_bc",
        ),
        ("sum(\"ar\".\"netamount_bc\") / count(*)", "netamount_bc"),
    ] {
        let drafted = BTreeMap::from([("average_invoice".to_string(), invented.to_string())]);
        let c = compile(&s, &ledger(), &r, &drafted).unwrap();
        let report = check(&c);
        assert!(!report.accepted(), "{invented}");
        assert!(
            report.unmapped.iter().any(|a| a.text.contains(atom)),
            "{invented}: {:?}",
            report.unmapped
        );
    }
}

#[test]
fn a_join_that_would_repeat_rows_is_a_question() {
    // From transactions, ar is reachable only parent → child: totals would be multiplied.
    let s = parse(
        "---\nmart: m\ngrain: [month]\ndimensions:\n  month: month of transactions.transdate\n\
         measures:\n  n: count of transactions\n  x: max of ar.amount_bc\n---\n",
    )
    .unwrap();
    let r = resolve(&s, &ledger());
    let qs = compile(&s, &ledger(), &r, &BTreeMap::new()).unwrap_err();
    assert!(qs[0].text.contains("would repeat rows"), "{qs:?}");
}

#[test]
fn reconciliation_tests_cover_additive_measures_with_the_same_filters() {
    let s = parse(RECEIVABLES).unwrap();
    let r = resolve(&s, &ledger());
    let drafted = BTreeMap::from([(
        "average_invoice".to_string(),
        "sum(\"ar\".\"amount_bc\") / count(*)".to_string(),
    )]);
    let c = compile(&s, &ledger(), &r, &drafted).unwrap();
    let tests = reconciliation_tests(&s, &r, &c);
    let names: Vec<&str> = tests.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "gold__receivables_by_customer_month__invoiced_reconciles",
            "gold__receivables_by_customer_month__invoices_reconciles"
        ]
    );
    assert!(tests[0].1.contains("select sum(\"ar\".\"amount_bc\") as v"));
    assert!(
        tests[0]
            .1
            .contains("where \"transactions\".\"approved\" = true")
    );
    let yaml = model_yaml(&s, &r);
    assert!(
        yaml.contains("ekos_unique_combination") && yaml.contains("- '\"customer\"'"),
        "{yaml}"
    );
    let md = proposal_markdown(&s, &r, &c, &check(&c), &drafted);
    assert!(
        md.contains("| invoiced (measures) | sum of ar.amount_bc |"),
        "{md}"
    );
    assert!(md.contains("0 not stated by the description"));
}

#[test]
fn the_engine_reads_silver_tables_directly() {
    let s = parse(RECEIVABLES).unwrap();
    let r = resolve(&s, &ledger());
    let drafted = BTreeMap::from([(
        "average_invoice".to_string(),
        "sum(\"ar\".\"amount_bc\") / count(*)".to_string(),
    )]);
    let c = compile(&s, &ledger(), &r, &drafted).unwrap();
    let e = engine_sql(&c, "iceberg");
    assert!(
        e.contains("from \"iceberg\".\"silver_public\".\"ar\" as \"ar\""),
        "{e}"
    );
    assert!(!e.contains("{{") && !e.contains("--"), "{e}");
}
