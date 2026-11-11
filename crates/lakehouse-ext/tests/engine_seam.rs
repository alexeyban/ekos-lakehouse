//! RFC 0255 phase 2: engines are registered, not matched. An engine this build lacks is refused
//! by name wherever it is named (settings, a deployed lock); a registered one is accepted with no
//! change to this crate; two providers of one engine stop startup.

use ekos_lakehouse_bronze::{EngineSpec, Mapping, SourceColumn, SourceEngine};
use ekos_lakehouse_ext::LakehouseExtension;
use ekos_lakehouse_ext::engines::{Engines, SourceProvider};
use ekos_lakehouse_ext::settings::{Settings, load_with};
use ekos_lakehouse_ext::source::Source;
use std::sync::Arc;

struct FixtureSpec;

impl EngineSpec for FixtureSpec {
    fn engine(&self) -> SourceEngine {
        SourceEngine::from_static("fixture")
    }
    fn map_column(&self, c: &SourceColumn) -> Mapping {
        ekos_lakehouse_bronze::typemap::map_column(c)
    }
    fn orderable(&self, t: &str) -> bool {
        t == "bigint"
    }
}

struct Fixture;

impl SourceProvider for Fixture {
    fn spec(&self) -> Arc<dyn EngineSpec> {
        Arc::new(FixtureSpec)
    }
    fn check_settings(
        &self,
        s: &mut Settings,
        _: &[ekos_compiler_core::DatabaseConfig],
    ) -> anyhow::Result<()> {
        if s.source_user.is_some() {
            anyhow::bail!("source-user is not read by the fixture engine");
        }
        Ok(())
    }
    fn connect(&self, _: &Settings, _: &str) -> anyhow::Result<Box<dyn Source>> {
        anyhow::bail!("the fixture engine has no database")
    }
    fn dialect(&self) -> ekos_migrate_validate::Dialect {
        ekos_migrate_validate::Dialect::Postgres
    }
}

fn config(extra: &str) -> ekos_compiler_core::EkosConfig {
    toml::from_str(&format!(
        "[extensions.lakehouse]\nsource-engine = \"fixture\"\n{extra}"
    ))
    .unwrap()
}

#[test]
fn settings_naming_an_engine_the_build_lacks_are_refused_by_name() {
    let e = load_with(&config(""), &Engines::builtin())
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("source engine `fixture` is not in this build (it has: postgres, oracle)"),
        "{e}"
    );
}

#[test]
fn a_registered_engine_reads_and_checks_its_own_settings() {
    let ext = LakehouseExtension::new().with_engine(Arc::new(Fixture));
    let s = load_with(&config(""), ext.engines()).unwrap();
    assert_eq!(s.engine().as_str(), "fixture");
    // The provider's refusal names the key and the engine.
    let e = load_with(&config("source-user = \"x\"\n"), ext.engines())
        .unwrap_err()
        .to_string();
    assert_eq!(e, "source-user is not read by the fixture engine");
    // Connecting goes to the provider, not to a built-in match.
    let e = match ekos_lakehouse_ext::plan_cmd::connect_source(&s, "test") {
        Err(e) => e.to_string(),
        Ok(_) => panic!("the fixture has no database"),
    };
    assert_eq!(e, "the fixture engine has no database");
}

#[test]
fn a_lock_naming_an_engine_the_build_lacks_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let lock = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rfc0255/idempiere.lock.json"),
    )
    .unwrap()
    .replace(
        "\"source-engine\": \"oracle\"",
        "\"source-engine\": \"sap\"",
    );
    std::fs::write(dir.path().join("lakehouse.lock.json"), lock).unwrap();
    let s = Settings::default();
    let e = ekos_lakehouse_ext::plan_cmd::read_lock_for(dir.path(), &s)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("lakehouse.lock.json: source engine `sap` is not in this build"),
        "{e}"
    );
    // A lock naming an invalid engine name does not parse at all.
    let bad = std::fs::read_to_string(dir.path().join("lakehouse.lock.json"))
        .unwrap()
        .replace("\"sap\"", "\"SAP HANA\"");
    std::fs::write(dir.path().join("lakehouse.lock.json"), bad).unwrap();
    assert!(ekos_lakehouse_ext::plan_cmd::read_lock(dir.path()).is_err());
}

#[test]
fn two_providers_of_one_engine_stop_startup() {
    let e = LakehouseExtension::new()
        .with_engine(Arc::new(Fixture))
        .try_with_engine(Arc::new(Fixture))
        .err()
        .expect("refused")
        .to_string();
    assert!(
        e.starts_with("two source engines are named `fixture`"),
        "{e}"
    );
    assert_eq!(e.matches("Fixture").count(), 2, "names both providers: {e}");
    let panicked = std::panic::catch_unwind(|| {
        LakehouseExtension::new().with_engine(Arc::new(ekos_lakehouse_ext::engines::Postgres))
    });
    assert!(panicked.is_err());
}

/// The default extension still offers exactly the built-in engines.
#[test]
fn the_default_extension_has_the_builtin_engines() {
    assert_eq!(
        LakehouseExtension::new().engines().names(),
        [SourceEngine::POSTGRES, SourceEngine::ORACLE]
    );
}
