//! EKOS RFC 0258 phase 3 end to end against a local Superset stub (no real Superset is called):
//! sync from Superset's REST API → the ledger → health; a planted failed load turns the dashboard
//! `failing` with the table named, in the header and the certification; recovery returns it to
//! `trusted`; the alert fires once; a restricted reader sees no dashboard.

use chrono::{Duration, Utc};
use ekos_kir::{KirId, KirObject, ObjectKind};
use ekos_lakehouse_ext::dashboards::{self, CERTIFIER, DashboardsSettings, Superset};
use ekos_lakehouse_ext::{LakehouseExtension, facts, marts, requirements};
use ekos_lakehouse_health::dashboard::DashState;
use ekos_ledger::KnowledgeStore;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

// ─────────────────────────────── the stub ───────────────────────────────

#[derive(Default)]
struct Stub {
    position_json: Option<String>,
    certified_by: String,
    certification_details: String,
    puts: Vec<Value>,
}

fn route(state: &Mutex<Stub>, method: &str, path: &str, body: &str) -> (u16, Value, bool) {
    let path = path.split('?').next().unwrap_or(path);
    let mut st = state.lock().unwrap();
    match (method, path) {
        ("GET", "/api/v1/dashboard/") => (
            200,
            json!({"count": 2, "result": [
                {"id": 7, "dashboard_title": "Sales", "url": "/superset/dashboard/7/"},
                {"id": 8, "dashboard_title": "Hand-made", "url": "/superset/dashboard/8/"},
            ]}),
            false,
        ),
        ("GET", "/api/v1/dashboard/7/datasets") => (
            200,
            json!({"result": [
                {"id": 10, "schema": "public", "table_name": "ai_abcdef_1",
                 "sql": "select month, sum(total) as total from iceberg.gold.sales group by 1"},
                {"id": 11, "schema": "public", "table_name": "ai_abcdef_2",
                 "sql": "select count(*) from iceberg.silver_public.customers"},
            ]}),
            false,
        ),
        ("GET", "/api/v1/dashboard/8/datasets") => (
            200,
            json!({"result": [{"id": 12, "schema": "public", "table_name": "orders", "sql": null}]}),
            false,
        ),
        ("GET", "/api/v1/dashboard/7/charts") => (
            200,
            json!({"result": [
                {"id": 1, "slice_name": "Sales by month", "form_data": {"datasource": "10__table"}},
                {"id": 2, "slice_name": "Customers", "form_data": {"datasource": "11__table"}},
            ]}),
            false,
        ),
        ("GET", "/api/v1/dashboard/7") => (
            200,
            json!({"result": {
                "id": 7,
                "position_json": st.position_json,
                "certified_by": st.certified_by,
                "certification_details": st.certification_details,
            }}),
            false,
        ),
        ("GET", "/api/v1/security/csrf_token/") => (200, json!({"result": "csrf-1"}), true),
        ("PUT", "/api/v1/dashboard/7") => {
            let v: Value = serde_json::from_str(body).unwrap();
            if let Some(p) = v["position_json"].as_str() {
                st.position_json = Some(p.to_string());
            }
            if let Some(c) = v["certified_by"].as_str() {
                st.certified_by = c.to_string();
            }
            if let Some(c) = v["certification_details"].as_str() {
                st.certification_details = c.to_string();
            }
            st.puts.push(v);
            (200, json!({"id": 7}), false)
        }
        _ => (404, json!({"message": "not found"}), false),
    }
}

/// A one-connection-at-a-time HTTP/1.1 server; checks a PUT carries the CSRF token and cookie.
fn serve(state: Arc<Mutex<Stub>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let mut headers = vec![];
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                headers.push(h.trim().to_ascii_lowercase());
            }
            let len = headers
                .iter()
                .find_map(|h| h.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let authorized = headers.iter().any(|h| h == "authorization: bearer t0ken");
            let (mut code, mut v, set_cookie) = if authorized {
                route(&state, &method, &path, &String::from_utf8_lossy(&body))
            } else {
                (401, json!({"msg": "no token"}), false)
            };
            if method == "PUT"
                && !(headers.iter().any(|h| h == "x-csrftoken: csrf-1")
                    && headers.iter().any(|h| h == "cookie: session=s1"))
            {
                (code, v) = (400, json!({"message": "CSRF"}));
            }
            let text = v.to_string();
            let cookie = if set_cookie {
                "Set-Cookie: session=s1; HttpOnly; Path=/\r\n"
            } else {
                ""
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n{cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    format!("http://{addr}")
}

// ─────────────────────────────── ledger facts ───────────────────────────────

fn run(store: &dyn KnowledgeStore, table: &str, layer: &str, id: &str, ok: bool, ago_s: i64) {
    let mut o = KirObject::new(
        format!("{table} {layer} run {id}"),
        ObjectKind::Custom(facts::RUN_KIND.into()),
    );
    o.id = facts::run_id(table, layer, id);
    o.created_at = Utc::now() - Duration::seconds(ago_s);
    for (k, v) in [
        ("table", json!(table)),
        ("layer", json!(layer)),
        ("run_id", json!(id)),
        ("status", json!(if ok { "ok" } else { "failed" })),
        (
            "message",
            json!((!ok).then_some("TrinoUserError: Table 'public.orders' does not exist")),
        ),
    ] {
        o.properties.insert(k.into(), v);
    }
    store.append_object(&o).unwrap();
}

fn pipeline(store: &dyn KnowledgeStore) {
    run(store, "public.orders", "bronze", "b1", true, 600);
    run(store, "public.customers", "bronze", "b2", true, 1800);
    run(store, "public.orders", "silver", "s1", true, 500);
    run(store, "public.customers", "silver", "s1", true, 500);
    run(store, "gold.sales", "gold", "g1", true, 400);
    facts::record_requirement_approval(
        store,
        &requirements::Approval {
            mart: "sales".into(),
            by: "ann".into(),
            at: "2026-11-11T05:00:00Z".into(),
            note: None,
            digest: "d".into(),
            needs: vec!["public.orders".into(), "public.customers".into()],
        },
    )
    .unwrap();
    facts::record_mart_reconciliation(
        store,
        &marts::Reconciliation {
            mart: "sales".into(),
            at: "2026-11-11T05:30:00Z".into(),
            run_id: Some("g1".into()),
            measures: vec![marts::Totals {
                measure: "total".into(),
                mart_total: "379.90".into(),
                silver_total: "379.90".into(),
                equal: true,
            }],
        },
    )
    .unwrap();
}

fn ledger(dir: &std::path::Path) -> Box<dyn KnowledgeStore> {
    Box::new(ekos_ledger::Ledger::open(&dir.join("ledger.db")).unwrap())
}

#[test]
fn a_failed_load_turns_the_dashboard_failing_in_superset_and_recovery_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let store = ledger(dir.path());
    pipeline(store.as_ref());

    let state = Arc::new(Mutex::new(Stub {
        position_json: Some(
            json!({"ROOT_ID": {"type": "ROOT", "id": "ROOT_ID", "children": ["GRID_ID"]},
                   "GRID_ID": {"type": "GRID", "id": "GRID_ID", "children": ["ROW-0"], "parents": ["ROOT_ID"]},
                   "ROW-0": {"type": "ROW", "id": "ROW-0", "children": []}})
            .to_string(),
        ),
        ..Default::default()
    }));
    let base = serve(state.clone());
    let s = DashboardsSettings {
        superset_url: Some(base.clone()),
        superset_header: Some(true),
        console_url: Some("http://console/ext/lakehouse".into()),
        ..Default::default()
    };
    let sup = Superset::with_token(&base, "t0ken").unwrap();

    // Sync: only the dashboard the fork published (8's dataset is a plain table).
    let read = dashboards::read_superset(&s, &sup).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].charts[0].reads, ["iceberg.gold.sales"]);
    let (n, removed) = dashboards::sync(store.as_ref(), &read, 120, "t1").unwrap();
    assert_eq!((n, removed), (1, 0));

    let h = dashboards::health(store.as_ref(), 120).unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].state, DashState::Trusted, "{:#?}", h[0].evidence);
    let customers_loaded = Utc::now().timestamp() - 1800;
    assert!(
        (h[0].data_as_of.unwrap() - customers_loaded).abs() <= 2,
        "data as of = the earlier of the two sources"
    );

    // A dry run writes nothing; a real one certifies and adds the header.
    let said = dashboards::publish(&s, &h, &sup, true).unwrap();
    assert!(said[0].contains("would write"), "{said:?}");
    assert!(state.lock().unwrap().puts.is_empty());
    dashboards::publish(&s, &h, &sup, false).unwrap();
    {
        let st = state.lock().unwrap();
        assert_eq!(st.puts.len(), 1);
        assert_eq!(st.certified_by, CERTIFIER);
        assert!(
            st.position_json
                .as_deref()
                .unwrap()
                .contains("Data pipeline: trusted")
        );
    }
    let alerts = dir.path().join("alerts");
    assert!(dashboards::alert(&h, &alerts, None).unwrap().is_empty());

    // The planted failure: orders' last bronze load failed.
    run(store.as_ref(), "public.orders", "bronze", "b3", false, 60);
    let h = dashboards::health(store.as_ref(), 120).unwrap();
    assert_eq!(h[0].state, DashState::Failing);
    assert!(
        h[0].evidence[0].contains("bronze public.orders is failing"),
        "{:?}",
        h[0].evidence
    );
    let sent = dashboards::alert(&h, &alerts, None).unwrap();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].contains("\"Sales\" (7) is failing, was trusted"));
    assert!(
        dashboards::alert(&h, &alerts, None).unwrap().is_empty(),
        "once per transition"
    );
    dashboards::publish(&s, &h, &sup, false).unwrap();
    {
        let st = state.lock().unwrap();
        assert_eq!(st.certified_by, "", "EKOS's certification is removed");
        let p = st.position_json.as_deref().unwrap();
        assert!(p.contains("Data pipeline: failing"), "{p}");
        assert!(p.contains("public.orders"), "the table is named: {p}");
    }

    // Recovery.
    run(store.as_ref(), "public.orders", "bronze", "b4", true, 30);
    let h = dashboards::health(store.as_ref(), 120).unwrap();
    assert_eq!(h[0].state, DashState::Trusted, "{:#?}", h[0].evidence);
    dashboards::publish(&s, &h, &sup, false).unwrap();
    assert_eq!(state.lock().unwrap().certified_by, CERTIFIER);
    let said = dashboards::publish(&s, &h, &sup, false).unwrap();
    assert!(said[0].contains("unchanged"), "{said:?}");
}

#[test]
fn a_restricted_reader_sees_no_dashboard() {
    use ekos::extension::EkosExtension;
    let dir = tempfile::tempdir().unwrap();
    let store = ledger(dir.path());
    pipeline(store.as_ref());
    let record = ekos_lakehouse_health::dashboard::DashboardRecord {
        dashboard_id: 7,
        title: "Sales".into(),
        url: "/superset/dashboard/7/".into(),
        charts: vec![ekos_lakehouse_health::dashboard::ChartRecord {
            chart_id: 1,
            name: "Sales by month".into(),
            dataset: "public.ai_abcdef_1".into(),
            reads: vec!["gold.sales".into()],
            unreadable: None,
        }],
        chain_known: vec![],
        removed: false,
    };
    dashboards::sync(store.as_ref(), &[record], 120, "t1").unwrap();
    let ext = LakehouseExtension::new();
    assert!(ext.mcp_tool_is_scoped("ekos_dashboard_health"));
    let call = |s: &dyn KnowledgeStore| {
        ext.call_mcp_tool("ekos_dashboard_health", &json!({}), s)
            .unwrap()
            .unwrap()["dashboards"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(call(store.as_ref()), 1);

    let scoped = |rules: Value| {
        let scope =
            ekos_ledger::scope::ReadScope::from_value(&json!({"default": "deny", "rules": rules}))
                .unwrap();
        ekos_ledger::scope::ScopedStore::with_scopes(ledger(dir.path()), &[scope]).unwrap()
    };
    // A source-restricted reader: lakehouse facts carry no source.
    assert_eq!(
        call(&scoped(json!([{"effect": "allow", "source": "erp"}]))),
        0
    );
    // A reader allowed the dashboard's own fact but not the facts on its chain.
    let id: KirId = dashboards::record_id(7);
    let only_the_record = scoped(json!([{"effect": "allow", "object": id}]));
    assert_eq!(
        dashboards::read(&only_the_record).unwrap().0.len(),
        1,
        "the record itself is visible"
    );
    assert_eq!(call(&only_the_record), 0, "its chain is not");
}
