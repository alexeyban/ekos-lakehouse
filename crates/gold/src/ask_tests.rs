use super::*;
use crate::tests::ledger;
use crate::{check, compile, parse};
use std::collections::BTreeMap;

fn term(n: &str, d: &str) -> NamedTerm {
    NamedTerm {
        name: n.into(),
        definition: d.into(),
    }
}

fn request() -> AskRequest {
    AskRequest {
        measures: vec![term("invoiced", "sum of ar.amount_bc")],
        dimensions: vec![term("customer", "entity_credit_account.meta_number")],
        filters: vec!["transactions.approved is true".into()],
        target_mart: None,
    }
}

/// The existing mart a request like `request()` is answered by.
fn existing() -> MartSpec {
    parse(
        "---
mart: receivables_by_customer
grain: [customer]
dimensions:
  customer: entity_credit_account.meta_number
measures:
  invoiced: sum of ar.amount_bc
filters:
  - transactions.approved is true
---
Receivables per customer.
",
    )
    .unwrap()
}

#[test]
fn a_model_answer_is_terms_only_and_unknown_fields_are_refused() {
    let ok =
        r#"{"measures":[{"name":"n","definition":"count of ar"}],"dimensions":[],"filters":[]}"#;
    assert!(serde_json::from_str::<AskRequest>(ok).is_ok());
    for bad in [
        r#"{"measures":[{"name":"n","definition":"count of ar"}],"sql":"select 1"}"#,
        r#"{"measures":[{"name":"n","definition":"count of ar","expr":"1"}]}"#,
    ] {
        assert!(serde_json::from_str::<AskRequest>(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_request_must_be_data_in_the_descriptions_vocabulary() {
    assert!(request().check().is_ok());
    let mut r = request();
    r.measures.clear();
    assert!(r.check().unwrap_err().contains("at least one measure"));
    for bad in [
        "x'; drop table ar; --",
        "a;b",
        "a\"b",
        "a`b",
        "a--b",
        "a/*b",
        "a\\b",
        "a\nb",
        "  ",
    ] {
        let mut r = request();
        r.measures[0].definition = bad.into();
        assert!(r.check().is_err(), "{bad:?}");
        let mut r = request();
        r.filters = vec![bad.into()];
        assert!(r.check().is_err(), "filter {bad:?}");
    }
    for bad in ["", "Revenue", "1st", "a b", "a-b", &"x".repeat(41)] {
        let mut r = request();
        r.measures[0].name = bad.into();
        assert!(r.check().is_err(), "{bad:?}");
    }
    let mut r = request();
    r.measures.push(term("invoiced", "count of ar"));
    assert!(r.check().unwrap_err().contains("named twice"));
    let mut r = request();
    r.dimensions[0].name = "invoiced".into();
    assert!(r.check().unwrap_err().contains("share a name"));
    let mut r = request();
    r.filters = (0..=MAX_TERMS).map(|i| format!("ar.id is {i}")).collect();
    assert!(r.check().is_err());
}

#[test]
fn the_same_terms_make_the_same_mart_whatever_the_spacing_or_order() {
    let a = request();
    let mut b = request();
    b.measures[0].definition = "SUM  of ar.amount_bc".into();
    b.filters = vec!["transactions.approved   is true".into()];
    assert_eq!(a.to_spec().mart, b.to_spec().mart);
    let mut c = request();
    c.measures[0].definition = "sum of ar.netamount_bc".into();
    assert_ne!(a.to_spec().mart, c.to_spec().mart);
    assert!(a.to_spec().mart.starts_with("ask_") && a.to_spec().mart.len() == 16);
}

#[test]
fn a_mart_that_answers_exactly_is_possible_now() {
    let t = triage(&request(), &ledger(), &[existing()]).unwrap();
    assert_eq!(
        t,
        Triage::PossibleNow {
            mart: "receivables_by_customer".into()
        }
    );
    // A measure the mart already has, asked under another name, is the same measure.
    let mut r = request();
    r.measures[0].name = "total".into();
    assert!(matches!(
        triage(&r, &ledger(), &[existing()]).unwrap(),
        Triage::PossibleNow { .. }
    ));
}

#[test]
fn a_new_measure_on_the_same_shape_extends_a_mart_without_changing_it() {
    let mut r = request();
    r.measures.push(term("invoices", "count of ar"));
    match triage(&r, &ledger(), &[existing()]).unwrap() {
        Triage::PossibleWithAddition { extend, mart } => {
            assert_eq!(extend.as_deref(), Some("receivables_by_customer"));
            assert!(mart.starts_with("ask_"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_different_shape_is_a_new_mart() {
    let mut r = request();
    r.filters.clear(); // the existing mart is approved-only; this is not
    match triage(&r, &ledger(), &[existing()]).unwrap() {
        Triage::PossibleWithAddition { extend, .. } => assert_eq!(extend, None),
        other => panic!("{other:?}"),
    }
    // No marts at all: also an addition.
    assert!(matches!(
        triage(&request(), &ledger(), &[]).unwrap(),
        Triage::PossibleWithAddition { extend: None, .. }
    ));
}

#[test]
fn changing_an_existing_mart_is_only_ever_an_explicit_request() {
    // The same names with other definitions, no target: nothing existing is touched.
    let mut r = request();
    r.measures[0].definition = "sum of ar.netamount_bc".into();
    assert!(matches!(
        triage(&r, &ledger(), &[existing()]).unwrap(),
        Triage::PossibleWithAddition { .. }
    ));
    // The same request aimed at the mart is a change, and says what changes.
    r.target_mart = Some("receivables_by_customer".into());
    r.filters.clear();
    match triage(&r, &ledger(), &[existing()]).unwrap() {
        Triage::PossibleWithChange { mart, changes } => {
            assert_eq!(mart, "receivables_by_customer");
            assert!(
                changes
                    .iter()
                    .any(|c| matches!(c, Change::Measure { name, .. } if name == "invoiced"))
            );
            assert!(changes.iter().any(|c| matches!(c, Change::Filters { .. })));
        }
        other => panic!("{other:?}"),
    }
    // Aimed at a mart that is not there: refused, not guessed.
    r.target_mart = Some("no_such_mart".into());
    assert!(
        triage(&r, &ledger(), &[existing()])
            .unwrap_err()
            .contains("no mart named")
    );
    // Aimed at a mart that already says the same: nothing to change.
    let mut same = request();
    same.target_mart = Some("receivables_by_customer".into());
    assert!(matches!(
        triage(&same, &ledger(), &[existing()]).unwrap(),
        Triage::PossibleNow { .. }
    ));
}

#[test]
fn what_does_not_resolve_is_a_question_or_impossible_never_a_guess() {
    // A name near real columns: ask which.
    let mut r = request();
    r.measures[0].definition = "sum of ar.amount".into();
    match triage(&r, &ledger(), &[]).unwrap() {
        Triage::NeedsClarification { questions } => {
            assert!(
                questions
                    .iter()
                    .any(|q| q.candidates.iter().any(|c| c.contains("amount_bc")))
            );
        }
        other => panic!("{other:?}"),
    }
    // Nothing near it: impossible, with the reason.
    let mut r = request();
    r.dimensions[0].definition = "weather.forecast".into();
    match triage(&r, &ledger(), &[]).unwrap() {
        Triage::Impossible { reasons } => assert!(reasons[0].contains("customer"), "{reasons:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_answerable_request_compiles_to_sql_that_states_nothing_it_was_not_given() {
    let spec = request().to_spec();
    let resolved = resolve(&spec, &ledger());
    let c = compile(&spec, &ledger(), &resolved, &BTreeMap::new()).expect("compiles");
    let report = check(&c);
    assert!(report.accepted(), "{report:?}");
    assert!(
        c.plain_sql.to_lowercase().contains("sum("),
        "{}",
        c.plain_sql
    );
    assert!(!c.plain_sql.to_lowercase().contains("weather"));
}

#[test]
fn prose_is_asked_back_because_a_question_never_gets_model_written_sql() {
    let mut r = request();
    r.measures.push(term(
        "average_invoice",
        "the amount_bc total divided by the number of invoices",
    ));
    match triage(&r, &ledger(), &[]).unwrap() {
        Triage::NeedsClarification { questions } => {
            assert!(questions[0].text.contains("prose"), "{questions:?}");
        }
        other => panic!("{other:?}"),
    }
}
