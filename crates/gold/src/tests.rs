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

#[test]
fn prose_that_names_no_column_is_asked_not_drafted() {
    // The runbook's case: an undefined business term with nothing of the description in it.
    let s = parse(&RECEIVABLES.replace(
        "average_invoice: the amount_bc total divided by the number of invoices",
        "lifetime_value: customer lifetime value",
    ))
    .unwrap();
    let r = resolve(&s, &ledger());
    let qs = questions(&r);
    assert_eq!(qs.len(), 1, "{qs:?}");
    assert_eq!(qs[0].0.term, "lifetime_value");
    assert!(qs[0].1.text.contains("names no column"), "{:?}", qs[0].1);
}

#[test]
fn a_draft_is_one_expression_over_its_own_columns() {
    let s = parse(RECEIVABLES).unwrap();
    let r = resolve(&s, &ledger());
    let avg = r
        .iter()
        .find(|x| x.term == "average_invoice")
        .unwrap()
        .resolution
        .as_ref()
        .unwrap();
    assert_eq!(
        check_draft(avg, "sum(\"ar\".\"amount_bc\") / count(*)"),
        Ok(())
    );
    // A comma splices another column into the SELECT and still plans (found live on Pagila:
    // `"store"."store_id", count(*)`).
    let e = check_draft(avg, "\"ar\".\"amount_bc\", count(*)").unwrap_err();
    assert!(e.contains("not one SQL expression"), "{e}");
    // A column the description states, but for another term.
    let e = check_draft(
        avg,
        "sum(\"ar\".\"amount_bc\") / count(\"transactions\".\"transdate\")",
    )
    .unwrap_err();
    assert!(e.contains("`transdate`"), "{e}");
    // A resolved (non-prose) term is not the drafter's.
    let invoiced = r
        .iter()
        .find(|x| x.term == "invoiced")
        .unwrap()
        .resolution
        .as_ref()
        .unwrap();
    assert_eq!(check_draft(invoiced, "anything, at all"), Ok(()));
}

/// Pagila's shape: a rental reaches `store` through its customer and through its inventory.
fn rentals() -> Plan {
    plan(
        &[
            table(
                "rental",
                vec![
                    col("id", "integer", "integer"),
                    col("customer_id", "integer", "integer"),
                    col("inventory_id", "integer", "integer"),
                ],
                vec![
                    ("customer_id", "customer", "id"),
                    ("inventory_id", "inventory", "id"),
                ],
            ),
            table(
                "customer",
                vec![
                    col("id", "integer", "integer"),
                    col("store_id", "integer", "integer"),
                ],
                vec![("store_id", "store", "id")],
            ),
            table(
                "inventory",
                vec![
                    col("id", "integer", "integer"),
                    col("store_id", "integer", "integer"),
                ],
                vec![("store_id", "store", "id")],
            ),
            table(
                "store",
                vec![
                    col("id", "integer", "integer"),
                    col("name", "text", "varchar"),
                ],
                vec![],
            ),
        ],
        &PlanSettings::default(),
    )
}

#[test]
fn a_table_reached_along_several_paths_is_asked_and_a_column_picks_one() {
    let s = parse(
        "---\nmart: m\ngrain: [store]\ndimensions:\n  store: store.name\n\
         measures:\n  n: count of rental.id\n---\n",
    )
    .unwrap();
    let r = resolve(&s, &rentals());
    let qs = compile(&s, &rentals(), &r, &BTreeMap::new()).unwrap_err();
    assert!(qs[0].text.contains("2 foreign-key paths"), "{qs:?}");
    assert_eq!(
        qs[0].candidates,
        ["customer.store_id", "inventory.store_id"]
    );
    // Naming the column on the path meant resolves it.
    let s = parse(
        "---\nmart: m\ngrain: [store]\ndimensions:\n  store: inventory.store_id\n\
         measures:\n  n: count of rental.id\n---\n",
    )
    .unwrap();
    let r = resolve(&s, &rentals());
    let c = compile(&s, &rentals(), &r, &BTreeMap::new()).unwrap();
    assert_eq!(c.joins.len(), 1, "{:?}", c.joins);
    assert!(check(&c).accepted());
}

/// iDempiere's shape (EKOS RFC 0215): upper-case names, an order pointing at its partner along
/// two keys (customer, bill-to) and at a location that has a region, and a long chain.
fn orders() -> Plan {
    let mut p = plan(
        &[
            table(
                "C_ORDER",
                vec![
                    col("id", "NUMBER(10,0)", "decimal(10,0)"),
                    col("C_BPARTNER_ID", "NUMBER(10,0)", "decimal(10,0)"),
                    col("BILL_BPARTNER_ID", "NUMBER(10,0)", "decimal(10,0)"),
                    col("C_LOCATION_ID", "NUMBER(10,0)", "decimal(10,0)"),
                    col("A_ID", "NUMBER(10,0)", "decimal(10,0)"),
                    col("GRANDTOTAL", "NUMBER", "decimal(38,10)"),
                    col("DOCSTATUS", "VARCHAR2(2)", "varchar(2)"),
                    col("DATEORDERED", "DATE", "timestamp(0)"),
                ],
                vec![
                    ("C_BPARTNER_ID", "C_BPARTNER", "id"),
                    ("BILL_BPARTNER_ID", "C_BPARTNER", "id"),
                    ("C_LOCATION_ID", "C_LOCATION", "id"),
                    ("A_ID", "A", "id"),
                ],
            ),
            table(
                "C_BPARTNER",
                vec![
                    col("id", "NUMBER(10,0)", "decimal(10,0)"),
                    col("NAME", "VARCHAR2(60)", "varchar(60)"),
                ],
                vec![],
            ),
            table(
                "C_LOCATION",
                vec![
                    col("id", "NUMBER(10,0)", "decimal(10,0)"),
                    col("C_REGION_ID", "NUMBER(10,0)", "decimal(10,0)"),
                ],
                vec![("C_REGION_ID", "C_REGION", "id")],
            ),
            table(
                "C_REGION",
                vec![
                    col("id", "NUMBER(10,0)", "decimal(10,0)"),
                    col("NAME", "VARCHAR2(60)", "varchar(60)"),
                ],
                vec![],
            ),
            table(
                "A",
                vec![col("id", "x", "integer"), col("B_ID", "x", "integer")],
                vec![("B_ID", "B", "id")],
            ),
            table(
                "B",
                vec![col("id", "x", "integer"), col("C_ID", "x", "integer")],
                vec![("C_ID", "C", "id")],
            ),
            table(
                "C",
                vec![col("id", "x", "integer"), col("D_ID", "x", "integer")],
                vec![("D_ID", "D", "id")],
            ),
            table(
                "D",
                vec![col("id", "x", "integer"), col("E_ID", "x", "integer")],
                vec![("E_ID", "E", "id")],
            ),
            table(
                "E",
                vec![col("id", "x", "integer"), col("LABEL", "x", "varchar")],
                vec![],
            ),
        ],
        &PlanSettings::default(),
    );
    for t in &mut p.tables {
        if t.table == "C_ORDER" {
            t.facts.comment = Some("Sales & Purchase Order".into());
        }
        if t.table == "C_REGION" {
            t.facts.comment = Some("Identifies a geographical Region".into());
        }
    }
    p
}

fn spec(body: &str) -> MartSpec {
    parse(&format!("---\nmart: m\n{body}---\n")).unwrap()
}

#[test]
fn oracle_names_resolve_whatever_their_case() {
    let s = spec(
        "grain: [status]\ndimensions:\n  status: c_order.docstatus\n\
         measures:\n  total: sum of c_order.grandtotal\nfilters:\n  - c_order.docstatus = 'CO'\n",
    );
    let r = resolve(&s, &orders());
    assert!(questions(&r).is_empty(), "{r:?}");
    let c = compile(&s, &orders(), &r, &BTreeMap::new()).unwrap();
    assert!(
        c.plain_sql.contains("\"C_ORDER\".\"GRANDTOTAL\""),
        "{}",
        c.plain_sql
    );
    assert!(check(&c).accepted());
}

#[test]
fn via_chooses_one_of_several_paths_and_one_table_is_joined_once() {
    let s = spec(
        "grain: [customer]\ndimensions:\n  customer: c_bpartner.name\n\
         measures:\n  total: sum of c_order.grandtotal\n",
    );
    let r = resolve(&s, &orders());
    let qs = compile(&s, &orders(), &r, &BTreeMap::new()).unwrap_err();
    assert!(qs[0].text.contains("2 foreign-key paths"), "{qs:?}");
    assert_eq!(
        qs[0].candidates,
        ["C_ORDER.BILL_BPARTNER_ID", "C_ORDER.C_BPARTNER_ID"]
    );
    let s = spec(
        "grain: [customer]\ndimensions:\n  customer: c_bpartner.name via c_order.c_bpartner_id\n\
         measures:\n  total: sum of c_order.grandtotal\n",
    );
    let r = resolve(&s, &orders());
    assert!(
        r[0].how.contains("joined via `C_ORDER.C_BPARTNER_ID`"),
        "{r:?}"
    );
    let c = compile(&s, &orders(), &r, &BTreeMap::new()).unwrap();
    assert_eq!(c.joins.len(), 1);
    assert_eq!(c.joins[0].1, "C_BPARTNER_ID");
    assert!(check(&c).accepted());
    // A `via` that is not a key is a question, not ignored.
    let s = spec(
        "grain: [customer]\ndimensions:\n  customer: c_bpartner.name via c_order.grandtotal\n\
         measures:\n  total: sum of c_order.grandtotal\n",
    );
    let r = resolve(&s, &orders());
    assert!(
        questions(&r)[0].1.text.contains("is not a foreign key"),
        "{r:?}"
    );
    // Two terms reading one table along two paths: one mart, one path per table.
    let s = spec(
        "grain: [customer, bill_to]\ndimensions:\n  customer: c_bpartner.name via c_order.c_bpartner_id\n  \
         bill_to: c_bpartner.name via c_order.bill_bpartner_id\n\
         measures:\n  total: sum of c_order.grandtotal\n",
    );
    let r = resolve(&s, &orders());
    let qs = compile(&s, &orders(), &r, &BTreeMap::new()).unwrap_err();
    assert!(qs[0].text.contains("passes through"), "{qs:?}");
}

#[test]
fn a_join_longer_than_four_keys_is_a_question() {
    let s =
        spec("grain: [d]\ndimensions:\n  d: d.id\nmeasures:\n  total: sum of c_order.grandtotal\n");
    let r = resolve(&s, &orders());
    assert!(
        compile(&s, &orders(), &r, &BTreeMap::new()).is_ok(),
        "four keys: A, B, C, D"
    );
    let s = spec(
        "grain: [e]\ndimensions:\n  e: e.label\nmeasures:\n  total: sum of c_order.grandtotal\n",
    );
    let r = resolve(&s, &orders());
    let qs = compile(&s, &orders(), &r, &BTreeMap::new()).unwrap_err();
    assert!(qs[0].text.contains("at most 4 foreign keys"), "{qs:?}");
}

#[test]
fn a_requirement_is_edited_and_written_back_as_it_reads() {
    use crate::requirements::{Edit, apply, render};
    let mut s = spec(
        "grain: [customer]\ndimensions:\n  customer: c_bpartner.name\n\
         measures:\n  total: sum of c_order.grandtotal\nfilters:\n  - c_order.docstatus = 'CO'\n",
    );
    s.prose = "Sales per customer.".into();
    apply(
        &mut s,
        &Edit::Term {
            name: "customer".into(),
            value: "c_bpartner.name via c_order.c_bpartner_id".into(),
            section: None,
        },
    )
    .unwrap();
    apply(
        &mut s,
        &Edit::Term {
            name: "month".into(),
            value: "month of c_order.dateordered".into(),
            section: Some("dimensions".into()),
        },
    )
    .unwrap();
    assert_eq!(s.grain, ["customer", "month"]);
    assert!(
        apply(
            &mut s,
            &Edit::Term {
                name: "x".into(),
                value: "y".into(),
                section: None
            }
        )
        .is_err()
    );
    apply(&mut s, &Edit::DropFilter("c_order.docstatus = 'CO'".into())).unwrap();
    apply(&mut s, &Edit::AddFilter("c_order.docstatus <> 'VO'".into())).unwrap();
    let text = render(&s);
    assert_eq!(parse(&text).unwrap(), s, "{text}");
    let r = resolve(&s, &orders());
    assert!(questions(&r).is_empty(), "{r:?}");
}

#[test]
fn the_excerpt_ranks_tables_by_the_request_words_and_answers_are_cleaned() {
    use crate::requirements::{excerpt, from_answer, rank};
    let p = orders();
    let ranked = rank(&p, "Total of sales orders per region");
    assert_eq!(ranked[0].0, "C_ORDER", "{ranked:?}");
    assert!(ranked.iter().any(|(t, _)| t == "C_REGION"), "{ranked:?}");
    let e = excerpt(
        &p,
        "Total of sales orders per region",
        2,
        &Default::default(),
    );
    assert_eq!(e.tables.len(), 2);
    assert!(
        e.text.contains("## C_ORDER — Sales & Purchase Order"),
        "{}",
        e.text
    );
    let answer = "Here is the mart:\n```yaml\n---\nmart: sales_by_region\ngrain: [region]\n\
                  dimensions:\n  region: c_region.name\nmeasures:\n  total: sum of c_order.grandtotal\n---\n```\n";
    let s = from_answer(answer, "regional_sales", "Total of sales orders per region").unwrap();
    assert_eq!(s.mart, "regional_sales");
    assert_eq!(s.prose, "Total of sales orders per region");
    assert_eq!(
        s.dimensions,
        [("region".to_string(), "c_region.name".to_string())]
    );
    assert!(from_answer("no header here", "m", "x").is_err());
}

#[test]
fn every_word_of_the_request_gets_its_table_before_the_rest() {
    use crate::requirements::{choose, from_answer};
    let mut p = orders();
    let extra = |n: &str| {
        table(
            n,
            vec![col("id", "x", "integer"), col("NAME", "x", "varchar")],
            vec![],
        )
    };
    for n in ["C_ORDER_TRL", "C_ORDER_ACCT", "C_ORDERLINE_ORDER"] {
        p.tables
            .extend(plan(&[extra(n)], &PlanSettings::default()).tables);
    }
    let t = choose(&p, "sales orders by region", 2);
    assert_eq!(
        t,
        ["C_ORDER", "C_REGION"],
        "the plain name first, and region not crowded out"
    );
    // A header without its `---` lines, inside a code fence.
    let s = from_answer(
        "```\nmart: x\ngrain: [region]\ndimensions:\n  region: c_region.name\nmeasures:\n  n: count of c_order\n```",
        "m",
        "r",
    )
    .unwrap();
    assert_eq!(s.measures.len(), 1);
}

#[test]
fn a_missing_column_is_asked_with_the_similar_ones_only() {
    let s = spec(
        "grain: [m]\ndimensions:\n  m: month of c_order.dateordered\nmeasures:\n  t: sum of c_order.total\n",
    );
    let r = resolve(&s, &orders());
    let (_, q) = questions(&r)[0];
    assert!(q.text.contains("has no column `total`"), "{q:?}");
    assert_eq!(q.candidates, ["C_ORDER.GRANDTOTAL"]);
}

#[test]
fn a_model_listing_terms_without_names_gets_names_from_their_definitions() {
    let s = crate::requirements::from_answer(
        "---\nmart: x\ngrain: [month]\ndimensions:\n  month: month of c_order.dateordered\n\
         measures:\n  - count of c_order\n  - sum of c_order.grandtotal\n---\n",
        "m",
        "r",
    )
    .unwrap();
    assert_eq!(
        s.measures,
        [
            (
                "count_of_c_order".to_string(),
                "count of c_order".to_string()
            ),
            (
                "sum_of_c_order_grandtotal".to_string(),
                "sum of c_order.grandtotal".to_string()
            )
        ]
    );
    let e = parse("---\nmart: m\nmeasures:\n  - count of c_order\n---\n").unwrap_err();
    assert!(
        e.starts_with("measures:") && e.contains("orders: count of orders"),
        "{e}"
    );
}

#[test]
fn equals_true_reads_as_is_true() {
    let s = spec(
        "grain: [m]\ndimensions:\n  m: month of c_order.dateordered\nmeasures:\n  t: sum of c_order.grandtotal\nfilters:\n  - c_order.docstatus = TRUE\n",
    );
    let r = resolve(&s, &orders());
    assert!(questions(&r).is_empty(), "{r:?}");
    let c = compile(&s, &orders(), &r, &BTreeMap::new()).unwrap();
    assert!(
        c.plain_sql.contains("\"DOCSTATUS\" = true"),
        "{}",
        c.plain_sql
    );
}
