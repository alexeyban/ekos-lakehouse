use super::*;
use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind, UpdateTrigger};

fn col(n: &str, attnum: i32, pg: &str, trino: &str) -> SourceColumn {
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
        attnum,
        has_default: false,
    }
}

fn orders() -> SourceTable {
    let mut id = col("id", 1, "integer", "integer");
    id.not_null = true;
    SourceTable {
        schema: "public".into(),
        oid: 100,
        name: "orders".into(),
        columns: vec![
            id,
            col("amount", 2, "numeric(10,2)", "decimal(10,2)"),
            col("note", 3, "text", "varchar"),
            col(
                "updated",
                4,
                "timestamp with time zone",
                "timestamp(6) with time zone",
            ),
        ],
        primary_key: vec!["id".into()],
        row_estimate: 10,
        update_triggers: vec![UpdateTrigger {
            trigger: "touch".into(),
            function: "touch()".into(),
            assigns: vec!["updated".into()],
            body_sha: "aaaa".into(),
        }],
        foreign_keys: vec![],
        comment: None,
    }
}

fn settings() -> PlanSettings {
    let mut s = PlanSettings::default();
    s.modes
        .watermarks
        .insert("public.orders".into(), "updated".into());
    s
}

fn classes(
    before: Vec<SourceTable>,
    after: Vec<SourceTable>,
) -> Vec<(Class, Option<String>, Action)> {
    let s = settings();
    diff(&plan(&before, &s), &plan(&after, &s), &Policy::new())
        .into_iter()
        .map(|c| (c.class, c.column, c.action))
        .collect()
}

#[test]
fn no_change_is_no_change() {
    assert!(classes(vec![orders()], vec![orders()]).is_empty());
}

#[test]
fn a_new_table_is_automatic() {
    let mut other = orders();
    other.oid = 200;
    other.name = "items".into();
    other.update_triggers.clear();
    assert_eq!(
        classes(vec![orders()], vec![orders(), other]),
        [(Class::NewTable, None, Action::Auto)]
    );
}

#[test]
fn new_columns_split_on_nullability_and_default() {
    let mut a = orders();
    a.columns.push(col("tag", 5, "text", "varchar"));
    assert_eq!(
        classes(vec![orders()], vec![a.clone()]),
        [(Class::NewNullableColumn, Some("tag".into()), Action::Auto)]
    );
    a.columns[4].has_default = true;
    assert_eq!(
        classes(vec![orders()], vec![a.clone()])[0].0,
        Class::NewRequiredColumn
    );
    a.columns[4].has_default = false;
    a.columns[4].not_null = true;
    assert_eq!(
        classes(vec![orders()], vec![a])[0],
        (
            Class::NewRequiredColumn,
            Some("tag".into()),
            Action::Propose
        )
    );
}

#[test]
fn a_rename_is_known_by_attnum_not_guessed() {
    let mut a = orders();
    a.columns[2].name = "remark".into();
    assert_eq!(
        classes(vec![orders()], vec![a]),
        [(Class::Rename, Some("note".into()), Action::Propose)]
    );
    let mut t = orders();
    t.name = "purchase_orders".into();
    t.update_triggers.clear(); // the watermark declaration names the old table
    let c = classes(vec![orders()], vec![t]);
    assert!(c.contains(&(Class::Rename, None, Action::Propose)), "{c:?}");
}

#[test]
fn drops_of_columns_and_tables_are_proposed() {
    let mut a = orders();
    a.columns.remove(2);
    assert_eq!(
        classes(vec![orders()], vec![a]),
        [(Class::Drop, Some("note".into()), Action::Propose)]
    );
    assert_eq!(
        classes(vec![orders()], vec![]),
        [(Class::Drop, None, Action::Propose)]
    );
}

#[test]
fn types_widen_automatically_and_narrow_by_proposal() {
    let mut w = orders();
    w.columns[1] = col("amount", 2, "numeric(14,2)", "decimal(14,2)");
    assert_eq!(
        classes(vec![orders()], vec![w]),
        [(Class::Widening, Some("amount".into()), Action::Auto)]
    );
    let mut n = orders();
    n.columns[1] = col("amount", 2, "numeric(10,4)", "decimal(10,4)"); // scale changed
    assert_eq!(classes(vec![orders()], vec![n])[0].0, Class::Narrowing);
    let mut t = orders();
    t.columns[2] = col("note", 3, "integer", "integer");
    assert_eq!(classes(vec![orders()], vec![t])[0].0, Class::Narrowing);
    let mut same = orders();
    same.columns[2] = col("note", 3, "character varying(50)", "varchar(50)");
    assert!(
        classes(vec![orders()], vec![same]).is_empty(),
        "Iceberg is unaffected"
    );
}

#[test]
fn a_key_change_is_proposed_and_a_renamed_key_is_not_one() {
    let mut k = orders();
    k.primary_key = vec!["id".into(), "note".into()];
    assert_eq!(classes(vec![orders()], vec![k])[0].0, Class::KeyChange);
    let mut r = orders();
    r.columns[0].name = "order_id".into();
    r.primary_key = vec!["order_id".into()];
    assert_eq!(
        classes(vec![orders()], vec![r]),
        [(Class::Rename, Some("id".into()), Action::Propose)]
    );
}

#[test]
fn a_changed_or_missing_watermark_proof_is_a_routine_change() {
    let mut body = orders();
    body.update_triggers[0].body_sha = "bbbb".into();
    assert_eq!(
        classes(vec![orders()], vec![body])[0].0,
        Class::RoutineChange
    );
    let mut gone = orders();
    gone.update_triggers.clear();
    let c = diff(
        &plan(&[orders()], &settings()),
        &plan(&[gone], &settings()),
        &Policy::new(),
    );
    assert_eq!(c[0].class, Class::RoutineChange);
    assert!(
        c[0].evidence
            .iter()
            .any(|e| e.contains("no BEFORE UPDATE row trigger")),
        "{c:?}"
    );
}

#[test]
fn policy_overrides_a_class() {
    let mut a = orders();
    a.columns[2].name = "remark".into();
    let policy: Policy = [("rename".to_string(), Action::Auto)].into();
    let c = diff(
        &plan(&[orders()], &settings()),
        &plan(&[a], &settings()),
        &policy,
    );
    assert_eq!(c[0].action, Action::Auto);
}

#[test]
fn auto_changes_merge_and_proposed_ones_wait() {
    let s = settings();
    let mut other = orders();
    other.oid = 200;
    other.name = "items".into();
    other.update_triggers.clear();
    let mut a = orders();
    a.columns[2].name = "remark".into(); // proposed
    let (dep, live) = (plan(&[orders()], &s), plan(&[a, other], &s));
    let changes = diff(&dep, &live, &Policy::new());
    let next = apply_auto(&dep, &live, &changes);
    let names: Vec<&str> = next.tables.iter().map(|t| t.table.as_str()).collect();
    assert_eq!(names, ["items", "orders"]);
    let orders_next = next.tables.iter().find(|t| t.table == "orders").unwrap();
    assert!(
        orders_next.columns.iter().any(|m| m.column == "note"),
        "the rename waits"
    );
    let proposed = with_table(&next, &live, "public.orders");
    let o = proposed
        .tables
        .iter()
        .find(|t| t.table == "orders")
        .unwrap();
    assert!(o.columns.iter().any(|m| m.column == "remark"));
    assert_eq!(
        o.renames,
        [ekos_lakehouse_bronze::ColumnRename {
            from: "note".into(),
            to: "remark".into()
        }],
        "the merged plan renames bronze's column in place"
    );
    assert!(
        diff(&proposed, &live, &Policy::new()).is_empty(),
        "after the merge the source matches"
    );
}

#[test]
fn an_auto_rename_by_policy_also_renames_in_place() {
    let mut a = orders();
    a.columns[2].name = "remark".into();
    let policy: Policy = [("rename".to_string(), Action::Auto)].into();
    let (dep, live) = (plan(&[orders()], &settings()), plan(&[a], &settings()));
    let next = apply_auto(&dep, &live, &diff(&dep, &live, &policy));
    assert_eq!(
        next.tables[0].renames.len(),
        1,
        "{:?}",
        next.tables[0].renames
    );
    assert_eq!(next.tables[0].renames[0].to, "remark");
}

#[test]
fn every_class_has_a_name_and_ids_are_stable() {
    assert_eq!(Class::ALL.len(), 9);
    let mut a = orders();
    a.columns.remove(2);
    let c = diff(
        &plan(&[orders()], &settings()),
        &plan(&[a], &settings()),
        &Policy::new(),
    );
    let refs: Vec<&Change> = c.iter().collect();
    assert_eq!(change_id(&refs), change_id(&refs));
    let md = proposal_markdown(
        &change_id(&refs),
        "public.orders",
        &refs,
        &["public__orders.note: 1 test".into()],
    );
    assert!(
        md.contains("| `drop` | note |") && md.contains("bronze paused"),
        "{md}"
    );
}
