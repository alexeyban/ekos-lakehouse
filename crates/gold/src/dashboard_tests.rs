use super::*;
use crate::parse;
use crate::tests::ledger;

fn t(n: &str, d: &str) -> NamedTerm {
    NamedTerm {
        name: n.into(),
        definition: d.into(),
    }
}

const MART: &str = "---
mart: receivables
grain: [month, customer]
dimensions:
  month: month of transactions.transdate
  customer: entity_credit_account.meta_number
measures:
  invoiced: sum of ar.amount_bc
  invoices: count of ar
filters:
  - transactions.approved is true
---
Approved receivables per month and customer.
";

fn marts() -> Vec<Mart> {
    marts_of(&[parse(MART).unwrap()], &ledger())
}

fn target() -> Target {
    Target {
        catalog: "iceberg".into(),
        gold_schema: "gold".into(),
    }
}

fn by_month() -> ChartRequest {
    ChartRequest {
        measures: vec![t("invoiced", "sum of ar.amount_bc")],
        dimensions: vec![t("month", "month of transactions.transdate")],
        filters: vec![],
        period: None,
    }
}

/// A chart as published: compiled over `receivables`, the mart's filter inherited.
fn published() -> BundleChart {
    let req = by_month();
    let s = compile_chart(&req, &ledger(), &marts(), &target(), None, true).unwrap();
    BundleChart {
        key: "c1".into(),
        presentation: Presentation {
            title: "Invoiced by month".into(),
            viz: "bar".into(),
            x: Some("month".into()),
            group_by: None,
            metric: Some("invoiced".into()),
            order: 0,
            number_format: None,
            color_scheme: None,
        },
        request: with_inherited(&req, &s),
        mart: s.mart.clone(),
        marts: vec![MartDigest {
            mart: s.mart.clone(),
            digest: s.digest.clone(),
        }],
        terms: s.terms.clone(),
        sql: s.sql.clone(),
        reconciled: None,
        superset: None,
        params: None,
    }
}

fn bundle(charts: Vec<BundleChart>) -> Bundle {
    Bundle {
        format: BUNDLE_FORMAT,
        dashboard: "dash-7".into(),
        title: "Receivables".into(),
        author: "alice".into(),
        request_text: "receivables by month".into(),
        superset: Some(SupersetDashboard {
            dashboard_id: 7,
            url: "/superset/dashboard/7/".into(),
        }),
        origin: None,
        charts,
        unadopted: vec![],
    }
}

fn change(c: &BundleChart, d: ChartDelta) -> Classified {
    let (p, r) = apply_chart_delta(c, &d).unwrap();
    classify(c, p, r, &ledger(), &marts(), &target())
}

#[test]
fn a_new_chart_rolls_up_a_finer_mart_and_inherits_its_filter() {
    let c = published();
    assert_eq!(c.mart, "receivables");
    assert_eq!(
        c.request.filters,
        ["transactions.approved is true"],
        "the mart's filter is stated on the chart"
    );
    assert!(
        c.sql.contains("sum(\"invoiced\") as \"invoiced\""),
        "{}",
        c.sql
    );
    assert!(c.sql.contains("from \"iceberg\".\"gold\".\"receivables\""));
    assert!(c.sql.contains("group by 1"));
    let rule = c.terms.iter().find(|r| r.name == "invoiced").unwrap();
    assert_eq!(rule.served_by, "sum of gold.receivables.invoiced");
    assert!(rule.rule.contains("sum"), "{rule:?}");
    let f = c.terms.iter().find(|r| r.section == "filters").unwrap();
    assert_eq!(f.served_by, "built into gold.receivables");
    // Without inheriting, a chart that does not state the mart's filter is not served by it.
    let e = compile_chart(&by_month(), &ledger(), &marts(), &target(), None, false).unwrap_err();
    assert!(
        matches!(e, Unserved::NoMart(ref w) if w[0].1.contains("keeps only rows")),
        "{e:?}"
    );
}

#[test]
fn a_non_additive_measure_is_not_totalled_over_a_finer_grain() {
    let mut req = by_month();
    req.measures = vec![t("largest", "max of ar.amount_bc")];
    let spec = "---\nmart: m2\ngrain: [month, customer]\ndimensions:\n  month: month of transactions.transdate\n  customer: entity_credit_account.meta_number\nmeasures:\n  largest: max of ar.amount_bc\n---\n";
    let ms = marts_of(&[parse(spec).unwrap()], &ledger());
    let e = compile_chart(&req, &ledger(), &ms, &target(), None, true).unwrap_err();
    assert!(format!("{e:?}").contains("not a sum or a count"), "{e:?}");
}

#[test]
fn a_filter_on_a_carried_column_is_a_query_change() {
    let c = published();
    let out = change(
        &c,
        ChartDelta {
            chart: "c1".into(),
            add_dimensions: vec![t("customer", "entity_credit_account.meta_number")],
            add_filters: vec!["entity_credit_account.meta_number <> 'INTERNAL'".into()],
            group_by: Some("customer".into()),
            ..Default::default()
        },
    );
    assert_eq!(out.class, ChangeClass::Query, "{out:?}");
    let s = out.served.unwrap();
    assert!(
        s.sql.contains("where \"customer\" <> 'INTERNAL'"),
        "{}",
        s.sql
    );
    assert!(
        !s.sql.contains("group by"),
        "the mart's own grain: no roll-up\n{}",
        s.sql
    );
    let f = s
        .terms
        .iter()
        .find(|r| r.written.contains("INTERNAL"))
        .unwrap();
    assert_eq!(f.served_by, "applied on gold.receivables.customer");
}

#[test]
fn a_period_filters_the_time_dimension_on_both_sides() {
    let c = published();
    let out = change(
        &c,
        ChartDelta {
            chart: "Invoiced by month".into(),
            period: Some(Period {
                dimension: "month".into(),
                from: Some("2025-01-01".into()),
                to: Some("2026-01-01".into()),
            }),
            ..Default::default()
        },
    );
    assert_eq!(out.class, ChangeClass::Query);
    let sql = out.served.clone().unwrap().sql;
    assert!(
        sql.contains("\"month\" >= timestamp '2025-01-01 00:00:00'"),
        "{sql}"
    );
    assert!(
        sql.contains("\"month\" < timestamp '2026-01-01 00:00:00'"),
        "{sql}"
    );
    // The silver side applies the same period, over silver tables only.
    let served = compile_chart(&out.request, &ledger(), &marts(), &target(), None, false).unwrap();
    let q = reconcile_queries(&out.request, &served, &ledger(), "iceberg").unwrap();
    assert_eq!(q.len(), 1);
    let (m, chart, silver) = &q[0];
    assert_eq!(m, "invoiced");
    assert!(chart.contains("\"gold\".\"receivables\""));
    assert!(
        !silver.contains("\"gold\""),
        "silver is not read through gold: {silver}"
    );
    assert!(silver.contains("silver_public"), "{silver}");
    assert!(silver.contains("where \"month\" >= timestamp '2025-01-01 00:00:00'"));
    // A period must start a unit.
    let bad = apply_chart_delta(
        &c,
        &ChartDelta {
            chart: "c1".into(),
            period: Some(Period {
                dimension: "month".into(),
                from: Some("2025-01-15".into()),
                to: None,
            }),
            ..Default::default()
        },
    );
    assert!(bad.unwrap_err().contains("not the start of a month"));
}

#[test]
fn presentation_only_keeps_the_sql() {
    let c = published();
    let out = change(
        &c,
        ChartDelta {
            chart: "c1".into(),
            viz: Some("line".into()),
            title: Some("Invoiced, monthly".into()),
            number_format: Some(",.2f".into()),
            color_scheme: Some("bnbColors".into()),
            ..Default::default()
        },
    );
    assert_eq!(out.class, ChangeClass::Presentation);
    assert!(out.served.is_none());
    assert_eq!(out.presentation.viz, "line");
}

#[test]
fn removing_a_filter_built_into_the_mart_changes_the_mart() {
    let c = published();
    let out = change(
        &c,
        ChartDelta {
            chart: "c1".into(),
            remove_filters: vec!["transactions.approved IS TRUE".into()],
            ..Default::default()
        },
    );
    assert_eq!(out.class, ChangeClass::MartChange, "{out:?}");
    let m = out.mart.unwrap();
    assert_eq!(m.mart, "receivables");
    assert!(m.filters.is_empty());
    assert!(
        matches!(&out.changes[0], Change::Filters { was, now } if was.len() == 1 && now.is_empty())
    );
}

#[test]
fn a_new_measure_is_an_addition_extending_a_mart_of_the_same_shape() {
    let c = published();
    let out = change(
        &c,
        ChartDelta {
            chart: "c1".into(),
            add_measures: vec![t("net", "sum of ar.netamount_bc")],
            add_dimensions: vec![t("customer", "entity_credit_account.meta_number")],
            viz: Some("table".into()),
            ..Default::default()
        },
    );
    assert_eq!(out.class, ChangeClass::MartAddition, "{out:?}");
    assert_eq!(out.extend.as_deref(), Some("receivables"));
    let m = out.mart.unwrap();
    assert!(m.mart.starts_with("dash_"));
    let md = m.markdown("For a chart.");
    let back = parse(&md).unwrap();
    assert_eq!(back.measures.len(), 2);
    assert_eq!(back.filters, ["transactions.approved is true"]);
}

#[test]
fn an_unknown_column_is_impossible_and_a_bad_delta_is_dropped() {
    let c = published();
    let out = change(
        &c,
        ChartDelta {
            chart: "c1".into(),
            add_filters: vec!["weather.forecast = 'sunny'".into()],
            ..Default::default()
        },
    );
    assert_eq!(out.class, ChangeClass::Impossible, "{out:?}");
    assert!(parse_delta(r#"{"charts":[{"chart":"c1","sql":"select 1"}]}"#).is_err());
    assert!(parse_delta(r#"{"charts":[]}"#).is_err());
    let d = parse_delta(r#"{"charts":[{"chart":"c1","add_filters":["x.y = 1; drop table t"]}]}"#)
        .unwrap();
    assert!(
        apply_chart_delta(&c, &d.charts[0]).is_err(),
        "`;` is refused"
    );
    let quoted = ChartDelta {
        chart: "c1".into(),
        add_filters: vec!["entity_credit_account.meta_number = 'a' or 'b'".into()],
        ..Default::default()
    };
    assert!(apply_chart_delta(&c, &quoted).is_err(), "one literal, last");
    let unknown_chart = ChartDelta {
        chart: "nope".into(),
        ..Default::default()
    };
    assert!(bundle(vec![c]).chart(&unknown_chart.chart).is_none());
}

#[test]
fn the_bundle_round_trips_and_rejects_unknown_fields() {
    let b = bundle(vec![published()]);
    let y = b.to_yaml();
    assert_eq!(Bundle::from_yaml(&y).unwrap(), b);
    assert!(Bundle::from_yaml(&format!("{y}\nextra: 1\n")).is_err());
    assert!(Bundle::from_yaml(&y.replace("format: 1", "format: 9")).is_err());
}

fn renamed(mut c: BundleChart, key: &str, title: &str) -> BundleChart {
    c.key = key.into();
    c.presentation.title = title.into();
    c
}

#[test]
fn branches_changing_different_charts_merge_and_the_same_term_conflicts() {
    let base = bundle(vec![published(), renamed(published(), "c2", "Second")]);
    // ours (the original, moved on): chart 1 becomes a line.
    let mut ours = base.clone();
    ours.charts[0].presentation.viz = "line".into();
    // theirs (a branch): chart 2 gets a period.
    let mut theirs = base.clone();
    theirs.charts[1].request.period = Some(Period {
        dimension: "month".into(),
        from: Some("2025-01-01".into()),
        to: None,
    });
    let merged = merge3(&base, &ours, &theirs, &BTreeMap::new()).unwrap();
    let (_, charts) = from_terms(&merged, &ours, &theirs).unwrap();
    assert_eq!(charts[0].1.viz, "line");
    assert!(charts[1].2.period.is_some());

    // Both change chart 1's type: a conflict, nothing merged until one is chosen.
    let mut theirs2 = base.clone();
    theirs2.charts[0].presentation.viz = "pie".into();
    let c = merge3(&base, &ours, &theirs2, &BTreeMap::new()).unwrap_err();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].term, "chart c1 presentation viz");
    let mut choose = BTreeMap::new();
    choose.insert(c[0].term.clone(), Side::Theirs);
    let merged = merge3(&base, &ours, &theirs2, &choose).unwrap();
    let (_, charts) = from_terms(&merged, &ours, &theirs2).unwrap();
    assert_eq!(charts[0].1.viz, "pie");

    // A chart removed on one side and untouched on the other is removed.
    let mut theirs3 = base.clone();
    theirs3.charts.remove(1);
    let merged = merge3(&base, &ours, &theirs3, &BTreeMap::new()).unwrap();
    assert_eq!(from_terms(&merged, &ours, &theirs3).unwrap().1.len(), 1);
}

#[test]
fn the_diff_is_terms_first_then_sql() {
    let a = bundle(vec![published()]);
    let mut b = a.clone();
    b.charts[0]
        .request
        .filters
        .push("entity_credit_account.meta_number <> 'X'".into());
    b.charts[0].sql = a.charts[0].sql.replace("from", "from\n-- x\n");
    let d = term_diff(&a, &b);
    assert_eq!(d.len(), 1);
    assert!(d[0].term.starts_with("chart c1 filter"));
    assert!(d[0].before.is_none());
    let s = sql_diff(&a.charts[0].sql, &b.charts[0].sql);
    assert!(s.iter().any(|l| l.starts_with("+ ")));
    assert!(s.iter().any(|l| l.starts_with("  ")));
}

#[test]
fn form_data_and_layout() {
    let c = published();
    let fd = form_data(&c, 42);
    assert_eq!(fd["datasource"], "42__table");
    assert_eq!(fd["viz_type"], "echarts_timeseries_bar");
    assert_eq!(fd["x_axis"], "month");
    assert_eq!(fd["metrics"][0]["aggregate"], "SUM");
    let l: Value = serde_json::from_str(&layout(
        "T",
        &[(1, "a".into()), (2, "b".into()), (3, "c".into())],
    ))
    .unwrap();
    assert_eq!(l["GRID_ID"]["children"], json!(["ROW-0", "ROW-1"]));
    assert_eq!(l["CHART-2"]["meta"]["chartId"], 3);
}

#[test]
fn norm_keeps_the_case_of_quoted_values() {
    assert_eq!(norm("A.B  =  'Ab C'"), "a.b = 'Ab C'");
    assert_eq!(norm("X IS TRUE"), "x is true");
    assert!(totals_equal("12.50", "12.5"));
    assert!(totals_equal("3", "3"));
    assert!(!totals_equal("3", "4"));
}

#[test]
fn the_mart_at_the_charts_own_grain_is_chosen_whatever_the_order() {
    let coarse = "---\nmart: by_month\ngrain: [month]\ndimensions:\n  month: month of transactions.transdate\nmeasures:\n  invoiced: sum of ar.amount_bc\nfilters:\n  - transactions.approved is true\n---\n";
    let specs = [parse(MART).unwrap(), parse(coarse).unwrap()];
    for order in [specs.to_vec(), specs.iter().rev().cloned().collect()] {
        let ms = marts_of(&order, &ledger());
        let s = compile_chart(&by_month(), &ledger(), &ms, &target(), None, true).unwrap();
        assert_eq!(s.mart, "by_month");
        assert!(!s.rollup);
    }
    // A preference wins when it serves.
    let ms = marts_of(&specs, &ledger());
    let s = compile_chart(
        &by_month(),
        &ledger(),
        &ms,
        &target(),
        Some("receivables"),
        true,
    )
    .unwrap();
    assert_eq!(s.mart, "receivables");
}
