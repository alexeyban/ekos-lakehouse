//! EKOS RFC 0260 §2.1: a source change's proposal names the gold marts whose requirement reads the
//! changed column and the dashboards whose charts read those marts.

use ekos_lakehouse_changes::{Action, Change, Class};
use ekos_lakehouse_ext::{dashboards, sync};
use ekos_lakehouse_health::dashboard::{ChartRecord, DashboardRecord};
use ekos_ledger::KnowledgeStore;
use std::path::Path;
use std::process::Command;

const SALES: &str = "---
mart: sales_by_category_month
grain:
- month
- product_category
dimensions:
  month: month of c_invoice.dateinvoiced via c_invoiceline.c_invoice_id
  product_category: m_product_category.name via c_invoiceline.m_product_id, m_product.m_product_category_id
measures:
  revenue: sum of c_invoiceline.linenetamt
filters:
- c_invoice.docstatus = 'CO'
---

Revenue per month and product category.
";

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo_with_mart(dir: &Path) {
    std::fs::create_dir_all(dir.join("marts")).unwrap();
    std::fs::write(dir.join("marts/sales_by_category_month.md"), SALES).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "mart"]);
}

fn change(table: &str, column: &str, class: Class) -> Change {
    Change {
        class,
        table: table.into(),
        column: Some(column.into()),
        from: Some("varchar(60)".into()),
        to: None,
        evidence: vec![],
        action: Action::Propose,
    }
}

fn dashboard(store: &dyn KnowledgeStore, removed: bool, id: i64) {
    let chart = |id: i64, name: &str, reads: &str| ChartRecord {
        chart_id: id,
        name: name.into(),
        dataset: "gold_idem.ai_1".into(),
        reads: vec![reads.into()],
        ..Default::default()
    };
    dashboards::record(
        store,
        "t1",
        &DashboardRecord {
            dashboard_id: id,
            title: if removed {
                "Old".into()
            } else {
                "CFO Dashboard".into()
            },
            url: format!("/superset/dashboard/{id}/"),
            charts: vec![
                chart(
                    11,
                    "Revenue by Product Category",
                    "gold_idem.sales_by_category_month",
                ),
                chart(
                    12,
                    "Purchase Spend by Vendor",
                    "gold_idem.purchase_spend_by_vendor",
                ),
            ],
            removed,
            ..Default::default()
        },
    )
    .unwrap();
}

#[test]
fn a_dropped_column_lists_the_mart_term_and_the_dashboards_chart() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("pipeline");
    repo_with_mart(&repo);
    let store = ekos_ledger::Ledger::open(&dir.path().join("ledger.db")).unwrap();
    dashboard(&store, false, 1);
    dashboard(&store, true, 2);

    let lines = sync::downstream(
        &repo,
        &store,
        &change("adempiere.m_product_category", "name", Class::Drop),
    )
    .unwrap();
    assert_eq!(
        lines,
        [
            "gold `sales_by_category_month`: dimension `product_category` = m_product_category.name \
             via c_invoiceline.m_product_id, m_product.m_product_category_id",
            "dashboard `CFO Dashboard` (1): chart `Revenue by Product Category` reads gold \
             `sales_by_category_month`",
        ],
        "the removed dashboard and the chart on another mart are not listed"
    );
}

#[test]
fn a_column_no_mart_reads_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("pipeline");
    repo_with_mart(&repo);
    let store = ekos_ledger::Ledger::open(&dir.path().join("ledger.db")).unwrap();
    dashboard(&store, false, 1);
    let lines = sync::downstream(
        &repo,
        &store,
        &change("adempiere.c_order", "poreference", Class::Rename),
    )
    .unwrap();
    assert_eq!(lines, ["no gold mart reads it"]);
}
