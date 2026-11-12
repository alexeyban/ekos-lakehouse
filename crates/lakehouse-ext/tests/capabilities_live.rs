//! RFC 0255 phase 4, live: the PostgreSQL provider runs the public conformance kit on the
//! sandbox source through its own read-only session, and the checked capabilities keep
//! `reconcile` provided. Needs the sandbox (`deploy/docker-compose.lakehouse.yml`), Pagila and
//! `EKOS_LAKEHOUSE_LIVE=1`. Reads only.

use ekos_kir::source_capabilities::Capability;
use ekos_lakehouse_ext::settings::Settings;

fn live() -> bool {
    std::env::var("EKOS_LAKEHOUSE_LIVE").is_ok_and(|v| v == "1")
}

/// The sandbox's source PostgreSQL as `host:port`, or `EKOS_LH_SOURCE`.
fn source_addr() -> String {
    std::env::var("EKOS_LH_SOURCE").unwrap_or_else(|_| "127.0.0.1:55433".into())
}

#[test]
fn postgres_passes_the_kit_and_keeps_reconcile() {
    if !live() {
        eprintln!("skipped: set EKOS_LAKEHOUSE_LIVE=1 with the sandbox up");
        return;
    }
    let s = Settings {
        source: Some(source_addr()),
        source_database: Some("pagila".into()),
        source_user: Some("ekos".into()),
        source_password_env: Some("EKOS_LH_PG_PASSWORD".into()),
        ..Default::default()
    };
    let provider = s.engines.get(&s.engine()).unwrap().clone();
    let src = ekos_lakehouse_ext::plan_cmd::connect_source(&s, "lakehouse-test").unwrap();
    // The sandbox's role may write (the doctor warns then); the proof itself must answer.
    src.writes_refused().unwrap();
    let report = ekos_lakehouse_ext::capabilities::conformance(provider.as_ref(), src.as_ref())
        .expect("postgres brings literals");
    eprintln!(
        "RFC 0255 conformance postgres via PgSource: {} cases, {} hashes, buckets {}, passed {}, id {}",
        report.cases_checked,
        report.hashes_checked,
        report.buckets_checked,
        report.passed(),
        report.id
    );
    assert!(report.passed(), "{report:#?}");
    let sc = ekos_lakehouse_ext::capabilities::record(provider.as_ref(), &s, &[], Some(&report));
    assert!(sc.capabilities.get(Capability::Reconcile).is_provided());
    assert_eq!(sc.conformance_run.as_deref(), Some(report.id.as_str()));
    eprintln!("{}", ekos_lakehouse_ext::capabilities::render(&sc));
}
