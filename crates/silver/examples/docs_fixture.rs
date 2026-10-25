//! Regenerates the lakehouse-docs test fixture: a deployed plan (`lakehouse.lock.json`) and the
//! silver dbt project, built by the real bronze and silver generators from a `SourceTable` JSON.
//!
//!     python tools/lakehouse-docs/tests/fixtures/make_sources.py
//!     cargo run -p ekos-lakehouse-silver --example docs_fixture -- \
//!         tools/lakehouse-docs/tests/fixtures/sources.json tools/lakehouse-docs/tests/fixtures/pipeline
//!
//! The gold mart, proposals and evidence files in the fixture are written by hand.
use ekos_lakehouse_bronze::{SourceTable, plan, plan::PlanSettings};
use ekos_lakehouse_silver::{MaskMethod, SilverSettings, files};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src: Vec<SourceTable> =
        serde_json::from_str(&std::fs::read_to_string(&a[1]).unwrap()).unwrap();
    let out = std::path::Path::new(&a[2]);
    let mut s = PlanSettings {
        source_catalog: "pagila".into(),
        ..Default::default()
    };
    for t in [
        "film",
        "customer",
        "rental",
        "address",
        "city",
        "language",
        "inventory",
    ] {
        s.modes
            .watermarks
            .insert(format!("public.{t}"), "last_update".into());
    }
    let p = plan(&src, &s);
    std::fs::create_dir_all(out).unwrap();
    std::fs::write(
        out.join("lakehouse.lock.json"),
        serde_json::to_string_pretty(&p).unwrap(),
    )
    .unwrap();
    let mut ss = SilverSettings::default();
    ss.pii
        .insert("public.customer.email".into(), MaskMethod::Hmac);
    ss.pii
        .insert("public.address.phone".into(), MaskMethod::Null);
    ss.history.push("public.customer".into());
    let (f, r) = files(&p, &ss);
    for (k, v) in f {
        let pth = out.join(k);
        std::fs::create_dir_all(pth.parent().unwrap()).unwrap();
        std::fs::write(pth, v).unwrap();
    }
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
}
