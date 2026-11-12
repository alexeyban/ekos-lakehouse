//! RFC 0258 phases 1, 2 and 4 against an in-memory Superset, a scripted engine and a real git
//! repository: versions and revert, a non-author's copy, merges at the level of terms, conflicts,
//! a mart change through its gates and confirmation, an addition on a branch, drift.

use super::*;
use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{ForeignKey, SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_gold::dashboard::parse_delta;
use std::cell::RefCell;

// ─────────────────────────────── fakes ───────────────────────────────

#[derive(Default)]
struct FakeSuperset {
    next: RefCell<i64>,
    dashboards: RefCell<BTreeMap<i64, Value>>,
    charts: RefCell<BTreeMap<i64, Value>>,
    datasets: RefCell<BTreeMap<i64, Value>>,
}

impl FakeSuperset {
    fn id(&self) -> i64 {
        *self.next.borrow_mut() += 1;
        *self.next.borrow()
    }

    fn on(&self, dashboard: i64) -> Vec<Value> {
        self.charts
            .borrow()
            .iter()
            .filter(|(_, c)| {
                c["dashboards"]
                    .as_array()
                    .is_some_and(|d| d.contains(&json!(dashboard)))
            })
            .map(|(id, c)| {
                json!({"id": id, "slice_name": c["slice_name"],
                       "form_data": serde_json::from_str::<Value>(c["params"].as_str().unwrap_or("{}")).unwrap_or_default()})
            })
            .collect()
    }

    /// One dashboard as Superset holds it: itself, its charts and their datasets.
    fn state_of(&self, dashboard: i64) -> String {
        let charts = self.on(dashboard);
        let ds: Vec<Value> = charts
            .iter()
            .filter_map(|c| {
                let id: i64 = c["form_data"]["datasource"]
                    .as_str()?
                    .split("__")
                    .next()?
                    .parse()
                    .ok()?;
                self.datasets.borrow().get(&id).cloned()
            })
            .collect();
        format!(
            "{:?}{charts:?}{ds:?}",
            self.dashboards.borrow().get(&dashboard)
        )
    }
}

fn num(path: &str, prefix: &str) -> Option<i64> {
    path.strip_prefix(prefix)?
        .split(['/', '?'])
        .next()?
        .parse()
        .ok()
}

impl SupersetApi for FakeSuperset {
    fn read(&self, path: &str) -> anyhow::Result<Value> {
        if let Some(id) = num(path, "/api/v1/dashboard/") {
            if path.ends_with("/charts") {
                return Ok(json!({"result": self.on(id)}));
            }
            let d = self.dashboards.borrow().get(&id).cloned();
            return Ok(json!({"result": d.ok_or_else(|| anyhow::anyhow!("404 {path}"))?}));
        }
        if let Some(id) = num(path, "/api/v1/chart/") {
            let mut c = self
                .charts
                .borrow()
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("404"))?;
            let ds: Vec<Value> = c["dashboards"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|d| json!({"id": d}))
                .collect();
            c["dashboards"] = json!(ds);
            return Ok(json!({"result": c}));
        }
        if let Some(id) = num(path, "/api/v1/dataset/") {
            let d = self
                .datasets
                .borrow()
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("404"))?;
            return Ok(json!({"result": d}));
        }
        anyhow::bail!("unexpected read {path}")
    }

    fn write(&self, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
        let b = body.cloned().unwrap_or(Value::Null);
        let merge = |into: &mut Value| {
            if let (Some(o), Some(n)) = (into.as_object_mut(), b.as_object()) {
                for (k, v) in n {
                    o.insert(k.clone(), v.clone());
                }
            }
        };
        let table = |p: &str| -> Option<(&RefCell<BTreeMap<i64, Value>>, i64)> {
            for (prefix, t) in [
                ("/api/v1/dashboard/", &self.dashboards),
                ("/api/v1/chart/", &self.charts),
                ("/api/v1/dataset/", &self.datasets),
            ] {
                if let Some(id) = num(p, prefix) {
                    return Some((t, id));
                }
            }
            None
        };
        match (method, path) {
            ("POST", p) => {
                let t = match p {
                    "/api/v1/dashboard/" => &self.dashboards,
                    "/api/v1/dataset/" => &self.datasets,
                    "/api/v1/chart/" => &self.charts,
                    _ => anyhow::bail!("unexpected POST {p}"),
                };
                let id = self.id();
                t.borrow_mut().insert(id, b.clone());
                Ok(json!({"id": id}))
            }
            ("PUT", p) if p.ends_with("/refresh") => Ok(Value::Null),
            ("PUT", p) => {
                let (t, id) = table(p).ok_or_else(|| anyhow::anyhow!("unexpected PUT {p}"))?;
                merge(
                    t.borrow_mut()
                        .get_mut(&id)
                        .ok_or_else(|| anyhow::anyhow!("404"))?,
                );
                Ok(Value::Null)
            }
            ("DELETE", p) => {
                let (t, id) = table(p).ok_or_else(|| anyhow::anyhow!("unexpected DELETE {p}"))?;
                t.borrow_mut().remove(&id);
                Ok(Value::Null)
            }
            _ => anyhow::bail!("unexpected write {method} {path}"),
        }
    }
}

/// Totals agree unless `silver_off` is set; builds are recorded.
#[derive(Default)]
struct FakeEngine {
    silver_off: RefCell<bool>,
    built: RefCell<Vec<String>>,
}

impl Engine for FakeEngine {
    fn rows(&self, sql: &str) -> anyhow::Result<Vec<Vec<String>>> {
        let off = *self.silver_off.borrow();
        let one = |v: &str| vec![vec![v.to_string()]];
        Ok(if sql.contains("as mart_total") {
            vec![vec!["7".into(), "7".into()]]
        } else if sql.contains("as silver") {
            one(if off { "99" } else { "100.50" })
        } else if sql.contains("as chart") {
            one("100.5")
        } else if sql.contains("having count(*) > 1") {
            one("0")
        } else if sql.starts_with("select cast(count(*)") {
            one("12")
        } else {
            vec![vec!["2025-01-01".into(), "1".into()]]
        })
    }

    fn build(&self, sql: &str) -> anyhow::Result<()> {
        self.built.borrow_mut().push(sql.to_string());
        Ok(())
    }
}

// ─────────────────────────────── the repository ───────────────────────────────

fn col(n: &str, t: &str) -> SourceColumn {
    let (pg, trino) = match t {
        "integer" => ("integer", "integer"),
        "timestamp" => ("timestamp without time zone", "timestamp(6)"),
        "decimal(20,2)" => ("numeric(20,2)", "decimal(20,2)"),
        "boolean" => ("boolean", "boolean"),
        _ => ("text", "varchar"),
    };
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

fn table(name: &str, cols: Vec<SourceColumn>, fk: Option<(&str, &str)>) -> SourceTable {
    SourceTable {
        schema: "public".into(),
        oid: 0,
        name: name.into(),
        columns: cols,
        primary_key: vec!["id".into()],
        row_estimate: 10,
        update_triggers: vec![],
        foreign_keys: fk
            .into_iter()
            .map(|(c, t)| ForeignKey {
                name: format!("{name}_{c}_fkey"),
                columns: vec![c.into()],
                ref_schema: "public".into(),
                ref_table: t.into(),
                ref_columns: vec!["id".into()],
            })
            .collect(),
        comment: None,
    }
}

const SALES: &str = "---\nmart: sales\ngrain: [month, customer]\ndimensions:\n  month: month of orders.ordered\n  customer: customers.name\nmeasures:\n  total: sum of orders.total\n  orders: count of orders\nfilters:\n- orders.done is true\n---\nSales.\n";

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = plan(
        &[
            table(
                "orders",
                vec![
                    col("id", "integer"),
                    col("customer_id", "integer"),
                    col("ordered", "timestamp"),
                    col("total", "decimal(20,2)"),
                    col("net", "decimal(20,2)"),
                    col("currency", "integer"),
                    col("done", "boolean"),
                ],
                Some(("customer_id", "customers")),
            ),
            table(
                "customers",
                vec![col("id", "integer"), col("name", "varchar")],
                None,
            ),
        ],
        &PlanSettings::default(),
    );
    std::fs::write(
        dir.path().join("lakehouse.lock.json"),
        serde_json::to_string(&p).unwrap(),
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("marts")).unwrap();
    std::fs::write(dir.path().join("marts/sales.md"), SALES).unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]).unwrap();
    git_as(dir.path(), "setup", &["add", "-A"]).unwrap();
    git_as(dir.path(), "setup", &["commit", "-q", "-m", "init"]).unwrap();
    dir
}

fn ctx<'a>(repo: &Path, sup: &'a FakeSuperset, engine: &'a FakeEngine, state: &Path) -> Ctx<'a> {
    let plan = crate::plan_cmd::read_lock(repo).unwrap();
    Ctx {
        repo: repo.to_path_buf(),
        target: Target {
            catalog: "iceberg".into(),
            gold_schema: ekos_lakehouse_bronze::plan::gold_schema(&plan.settings.namespace_prefix),
        },
        plan,
        sup,
        engine,
        settings: DashboardsSettings {
            superset_database_id: Some(1),
            reviewers: vec!["rita".into()],
            mart_build: Some("trino".into()),
            ..Default::default()
        },
        state_dir: state.to_path_buf(),
        now: "2026-11-12T10:00:00Z".into(),
    }
}

fn t(n: &str, d: &str) -> ekos_lakehouse_gold::ask::NamedTerm {
    ekos_lakehouse_gold::ask::NamedTerm {
        name: n.into(),
        definition: d.into(),
    }
}

fn new_chart(p: Value, measure: (&str, &str), dimension: (&str, &str)) -> NewChart {
    NewChart {
        presentation: serde_json::from_value(p).unwrap(),
        request: ChartRequest {
            measures: vec![t(measure.0, measure.1)],
            dimensions: vec![t(dimension.0, dimension.1)],
            filters: vec![],
            period: None,
        },
    }
}

fn dashboard(key: &str) -> NewDashboard {
    NewDashboard {
        key: Some(key.into()),
        title: "Sales".into(),
        request_text: "sales by month".into(),
        charts: vec![
            new_chart(
                json!({"title": "Total", "viz": "bar", "x": "month", "metric": "total"}),
                ("total", "sum of orders.total"),
                ("month", "month of orders.ordered"),
            ),
            new_chart(
                json!({"title": "Orders", "viz": "bar", "x": "customer", "metric": "orders", "order": 1}),
                ("orders", "count of orders"),
                ("customer", "customers.name"),
            ),
        ],
    }
}

fn delta(v: Value) -> Delta {
    parse_delta(&v.to_string()).unwrap()
}

fn ask<'a>(target: &'a str, user: &'a str, d: Value) -> ChangeRequest<'a> {
    ChangeRequest {
        target,
        delta: delta(d),
        user,
        text: "a change",
        mode: Mode::Auto,
        preview: false,
        confirm: false,
    }
}

fn head(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"]).unwrap()
}

struct Bed {
    dir: tempfile::TempDir,
    state: tempfile::TempDir,
    sup: FakeSuperset,
    engine: FakeEngine,
}

fn bed() -> Bed {
    Bed {
        dir: repo(),
        state: tempfile::tempdir().unwrap(),
        sup: FakeSuperset::default(),
        engine: FakeEngine::default(),
    }
}

// ─────────────────────────────── phase 1 ───────────────────────────────

#[test]
fn changes_are_versions_and_a_revert_restores() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    let d = publish_new(&cx, dashboard("sales"), "alice").unwrap();
    assert_eq!(
        d.charts[0].request.filters,
        ["orders.done is true"],
        "the mart's filter is stated"
    );
    assert!(d.charts.iter().all(|c| c.reconciled.as_ref().unwrap().ok));
    assert_eq!(versions(b.dir.path(), "sales").unwrap().len(), 1);

    let v = change(
        &cx,
        ask(
            "sales",
            "alice",
            json!({"charts": [{"chart": "c1", "viz": "line", "number_format": ",.2f"}]}),
        ),
    )
    .unwrap();
    assert_eq!(v["status"], "published", "{v}");
    assert_eq!(v["classes"][0]["class"], "presentation");
    let v = change(&cx, ask("sales", "alice", json!({"charts": [{"chart": "Orders", "add_filters": ["customers.name <> 'Internal'"]}]}))).unwrap();
    assert_eq!(v["classes"][0]["class"], "query", "{v}");
    let now = bundle_now(b.dir.path(), "sales").unwrap();
    assert!(now.charts[1].sql.contains("\"customer\" <> 'Internal'"));
    assert!(now.charts[1].reconciled.as_ref().unwrap().ok);
    let ds = now.charts[1].superset.as_ref().unwrap().dataset_id;
    assert_eq!(
        b.sup.datasets.borrow()[&ds]["sql"],
        json!(now.charts[1].sql),
        "republished in place"
    );
    assert_eq!(versions(b.dir.path(), "sales").unwrap().len(), 3);

    let before = bundle_at(b.dir.path(), "HEAD~1", "sales").unwrap();
    assert_eq!(
        revert(&cx, "sales", None, "alice").unwrap()["status"],
        "reverted"
    );
    let after = bundle_now(b.dir.path(), "sales").unwrap();
    assert!(dash::term_diff(&before, &after).is_empty());
    assert_eq!(
        b.sup.datasets.borrow()[&ds]["sql"],
        json!(before.charts[1].sql)
    );
    assert!(drift(&b.sup, &after).unwrap().is_empty());
    assert_eq!(versions(b.dir.path(), "sales").unwrap().len(), 4);
}

#[test]
fn a_second_users_change_is_a_copy_and_leaves_the_original() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    let d = publish_new(&cx, dashboard("sales"), "alice").unwrap();
    let original = d.superset.as_ref().unwrap().dashboard_id;
    let (snap, main) = (b.sup.state_of(original), head(b.dir.path()));
    let v = change(
        &cx,
        ask(
            "sales",
            "bob",
            json!({"charts": [{"chart": "c1", "viz": "line"}]}),
        ),
    )
    .unwrap();
    assert_eq!(v["status"], "published as a copy", "{v}");
    let br = v["branch"].as_str().unwrap();
    assert!(br.starts_with("ekos/dash/sales/"));
    assert_eq!(
        (b.sup.state_of(original), head(b.dir.path())),
        (snap, main.clone())
    );
    let copy = bundle_at(b.dir.path(), br, "sales").unwrap();
    assert_ne!(copy.superset.as_ref().unwrap().dashboard_id, original);
    assert_eq!(copy.origin.as_ref().unwrap().commit, main);
    assert_eq!(copy.author, "bob");
    assert!(superset_title(&copy).contains("copy by bob"));
    // Only the branch's author changes it further.
    assert!(
        change(
            &cx,
            ask(
                br,
                "carol",
                json!({"charts": [{"chart": "c1", "viz": "bar"}]})
            )
        )
        .is_err()
    );
    assert_eq!(
        change(
            &cx,
            ask(
                br,
                "bob",
                json!({"charts": [{"chart": "c1", "viz": "bar"}]})
            )
        )
        .unwrap()["status"],
        "published on the branch"
    );
}

#[test]
fn a_bad_delta_or_a_chart_that_does_not_reconcile_publishes_nothing() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    let d = publish_new(&cx, dashboard("sales"), "alice").unwrap();
    let id = d.superset.as_ref().unwrap().dashboard_id;
    assert!(parse_delta(r#"{"charts":[{"chart":"c1","sql":"select 1"}]}"#).is_err());
    let (snap, main) = (b.sup.state_of(id), head(b.dir.path()));
    *b.engine.silver_off.borrow_mut() = true;
    let v = change(
        &cx,
        ask(
            "sales",
            "alice",
            json!({"charts": [{"chart": "c2", "add_filters": ["customers.name <> 'X'"]}]}),
        ),
    )
    .unwrap();
    assert_eq!(v["status"], "refused", "{v}");
    assert!(v["reason"].as_str().unwrap().contains("does not reconcile"));
    assert_eq!(
        (b.sup.state_of(id), head(b.dir.path())),
        (snap.clone(), main.clone())
    );
    *b.engine.silver_off.borrow_mut() = false;
    let v = change(
        &cx,
        ask(
            "sales",
            "alice",
            json!({"charts": [{"chart": "c2", "add_filters": ["weather.sky = 'blue'"]}]}),
        ),
    )
    .unwrap();
    assert_eq!(v["status"], "impossible", "{v}");
    assert_eq!((b.sup.state_of(id), head(b.dir.path())), (snap, main));
}

// ─────────────────────────────── phase 2 ───────────────────────────────

#[test]
fn branches_merge_by_terms_and_the_same_term_conflicts() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    publish_new(&cx, dashboard("sales"), "alice").unwrap();
    let branch = |user: &str, d: Value| -> String {
        let v = change(&cx, ask("sales", user, d)).unwrap();
        v["branch"].as_str().unwrap().to_string()
    };
    let bob = branch(
        "bob",
        json!({"charts": [{"chart": "c1", "period": {"dimension": "month", "from": "2025-01-01"}}]}),
    );
    let carol = branch("carol", json!({"charts": [{"chart": "c2", "viz": "pie"}]}));
    assert_eq!(
        request_merge(&cx, &bob, "bob").unwrap()["status"],
        "requested"
    );
    assert!(
        request_merge(&cx, &bob, "carol").is_err(),
        "only the branch's author asks"
    );
    assert!(
        merge(&cx, &bob, "bob", &BTreeMap::new(), false).is_err(),
        "not the original's author"
    );
    for br in [&bob, &carol] {
        let v = merge(&cx, br, "alice", &BTreeMap::new(), false).unwrap();
        assert_eq!(v["status"], "merged", "{v}");
    }
    let now = bundle_now(b.dir.path(), "sales").unwrap();
    assert!(now.charts[0].request.period.is_some());
    assert_eq!(now.charts[1].presentation.viz, "pie");
    assert!(
        branches(b.dir.path(), None).unwrap().is_empty(),
        "merged branches are retired"
    );
    assert_eq!(read_requests(&cx)[0].status, "merged");
    assert_eq!(
        b.sup.dashboards.borrow().len(),
        1,
        "the copies were removed"
    );

    let dave = branch(
        "dave",
        json!({"charts": [{"chart": "c2", "number_format": ",.1f"}]}),
    );
    let erin = branch(
        "erin",
        json!({"charts": [{"chart": "c2", "number_format": ",.3f", "title": "Orders per customer"}]}),
    );
    assert_eq!(
        merge(&cx, &dave, "rita", &BTreeMap::new(), false).unwrap()["status"],
        "merged",
        "a reviewer may merge"
    );
    let main = head(b.dir.path());
    let v = merge(&cx, &erin, "alice", &BTreeMap::new(), false).unwrap();
    assert_eq!(v["status"], "conflict");
    assert_eq!(v["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["conflicts"][0]["term"],
        "chart c2 presentation number_format"
    );
    assert_eq!(head(b.dir.path()), main, "nothing merged half-way");
    let choose = BTreeMap::from([(
        "chart c2 presentation number_format".to_string(),
        Side::Theirs,
    )]);
    assert_eq!(
        merge(&cx, &erin, "alice", &choose, false).unwrap()["status"],
        "merged"
    );
    let now = bundle_now(b.dir.path(), "sales").unwrap();
    assert_eq!(
        now.charts[1].presentation.number_format.as_deref(),
        Some(",.3f")
    );
    assert_eq!(
        now.charts[1].presentation.title, "Orders per customer",
        "the other term merged too"
    );
}

#[test]
fn a_mart_change_passes_the_gates_and_the_confirmation_and_lists_the_other_dashboards() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    publish_new(&cx, dashboard("sales"), "alice").unwrap();
    publish_new(&cx, dashboard("other"), "frank").unwrap();
    let usd = json!({"charts": [{"chart": "c1", "add_filters": ["orders.currency = 100"]}]});
    let v = change(&cx, ask("sales", "alice", usd.clone())).unwrap();
    assert_eq!(v["status"], "needs_confirmation", "{v}");
    assert_eq!(v["impact"][0]["dashboard"], "other");
    assert!(
        b.engine.built.borrow().is_empty(),
        "nothing is built before Yes"
    );
    let mut yes = ask("sales", "alice", usd);
    yes.confirm = true;
    let v = change(&cx, yes).unwrap();
    assert_eq!(v["status"], "published", "{v}");
    assert_eq!(v["classes"][0]["class"], "mart_change");
    let gates = v["marts"][0]["gates"].as_array().unwrap();
    assert!(gates.iter().all(|g| g["ok"] == json!(true)), "{gates:?}");
    assert!(
        gates
            .iter()
            .any(|g| g["gate"].as_str().unwrap().starts_with("reconciles"))
    );
    assert!(
        std::fs::read_to_string(b.dir.path().join("marts/sales.md"))
            .unwrap()
            .contains("orders.currency = 100")
    );
    assert!(
        b.engine
            .built
            .borrow()
            .iter()
            .any(|s| s.contains("__candidate"))
    );
    let mut again = ask(
        "sales",
        "alice",
        json!({"charts": [{"chart": "c1", "add_filters": ["orders.net > 0"]}]}),
    );
    again.confirm = true;
    let v = change(&cx, again).unwrap();
    assert_eq!(v["status"], "refused");
    assert!(
        v["reason"]
            .as_str()
            .unwrap()
            .contains("today's one modifying change")
    );
}

#[test]
fn an_addition_on_a_branch_is_built_aside_and_becomes_real_on_merge() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    publish_new(&cx, dashboard("sales"), "alice").unwrap();
    let v = change(&cx, ask("sales", "gina", json!({"charts": [{"chart": "c1", "add_measures": [{"name": "net", "definition": "sum of orders.net"}], "viz": "table"}]}))).unwrap();
    assert_eq!(v["status"], "published as a copy", "{v}");
    let mart = v["marts"][0]["mart"].as_str().unwrap().to_string();
    assert!(mart.contains("__b"), "{mart}");
    assert!(
        !b.dir.path().join(format!("marts/{mart}.md")).exists(),
        "not on the main line"
    );
    let br = v["branch"].as_str().unwrap().to_string();
    assert!(crate::marts::file_at(b.dir.path(), &br, &format!("marts/{mart}.md")).is_some());
    let v = merge(&cx, &br, "alice", &BTreeMap::new(), false).unwrap();
    assert_eq!(v["status"], "merged", "{v}");
    let now = bundle_now(b.dir.path(), "sales").unwrap();
    assert!(now.charts[0].mart.starts_with("dash_") && !now.charts[0].mart.contains("__b"));
    assert!(
        b.dir
            .path()
            .join(format!("marts/{}.md", now.charts[0].mart))
            .exists()
    );
    assert!(
        b.engine
            .built
            .borrow()
            .iter()
            .any(|s| s.starts_with("drop table") && s.contains(&mart)),
        "the branch's mart is dropped"
    );
}

// ─────────────────────────────── phase 4 ───────────────────────────────

#[test]
fn a_hand_edit_is_drift_and_refuses_a_change_until_republished() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    let d = publish_new(&cx, dashboard("sales"), "alice").unwrap();
    let id = d.charts[0].superset.as_ref().unwrap().chart_id;
    {
        let mut charts = b.sup.charts.borrow_mut();
        let c = charts.get_mut(&id).unwrap();
        let mut p: Value = serde_json::from_str(c["params"].as_str().unwrap()).unwrap();
        p["viz_type"] = json!("echarts_area");
        c["params"] = json!(p.to_string());
    }
    let found = drift(&b.sup, &d).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("Total: `viz_type`"));
    let main = head(b.dir.path());
    let v = change(
        &cx,
        ask(
            "sales",
            "alice",
            json!({"charts": [{"chart": "c1", "viz": "line"}]}),
        ),
    )
    .unwrap();
    assert_eq!(v["status"], "refused");
    assert_eq!(head(b.dir.path()), main);
    assert_eq!(republish(&cx, "sales").unwrap()["status"], "republished");
    assert!(drift(&b.sup, &d).unwrap().is_empty());
    let v = change(
        &cx,
        ask(
            "sales",
            "alice",
            json!({"charts": [{"chart": "c1", "viz": "line"}]}),
        ),
    )
    .unwrap();
    assert_eq!(v["status"], "published");
}

#[test]
fn sync_reads_the_bundle_its_drift_and_the_marts_digest() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    let d = publish_new(&cx, dashboard("sales"), "alice").unwrap();
    let (digests, reads) = mart_facts(b.dir.path(), &cx.plan);
    let r = bundle_record(
        &b.sup,
        "dashboards/sales.yaml",
        &d,
        &cx.target,
        &digests,
        &reads,
    )
    .unwrap();
    assert_eq!(r.bundle.as_deref(), Some("dashboards/sales.yaml"));
    assert_eq!(
        r.charts[0].reads,
        [format!("iceberg.{}.sales", cx.target.gold_schema)]
    );
    assert_eq!(
        r.charts[0].mart_reads,
        ["public.orders", "public.customers"]
    );
    let rec = r.charts[0].reconciled.as_ref().unwrap();
    assert_eq!(
        (rec.ok, rec.current_digest.as_deref()),
        (Some(true), Some(rec.mart_digest.as_str()))
    );
    // A hand edit is the chart's drift; a changed title is the dashboard's.
    let id = d.superset.as_ref().unwrap().dashboard_id;
    b.sup.dashboards.borrow_mut().get_mut(&id).unwrap()["dashboard_title"] = json!("Renamed");
    let r = bundle_record(&b.sup, "x", &d, &cx.target, &digests, &reads).unwrap();
    assert!(r.charts.iter().all(|c| c.drift.is_empty()));
    assert_eq!(r.drift.len(), 1, "{:?}", r.drift);
    // A mart that changed since the chart was reconciled.
    let moved = BTreeMap::from([("sales".to_string(), "other".to_string())]);
    let r = bundle_record(&b.sup, "x", &d, &cx.target, &moved, &reads).unwrap();
    assert_eq!(
        r.charts[0]
            .reconciled
            .as_ref()
            .unwrap()
            .current_digest
            .as_deref(),
        Some("other")
    );
}

#[test]
fn a_bundle_the_fork_wrote_is_committed_as_its_authors_version() {
    let b = bed();
    let cx = ctx(b.dir.path(), &b.sup, &b.engine, b.state.path());
    let mut d = publish_new(&cx, dashboard("sales"), "alice").unwrap();
    d.dashboard = "dash-9".into();
    d.author = "sam".into();
    std::fs::write(b.dir.path().join(Bundle::path("dash-9")), d.to_yaml()).unwrap();
    std::fs::write(b.dir.path().join(Bundle::path("broken")), "format: 1\n").unwrap();
    let r = record_written_bundles(b.dir.path()).unwrap();
    let ok: Vec<&String> = r
        .iter()
        .filter(|(_, x)| x.is_ok())
        .map(|(k, _)| k)
        .collect();
    assert_eq!(ok, ["dash-9"]);
    assert!(r.iter().any(|(k, x)| k == "broken" && x.is_err()));
    let v = versions(b.dir.path(), "dash-9").unwrap();
    assert_eq!((v.len(), v[0].author.as_str()), (1, "sam"));
}
