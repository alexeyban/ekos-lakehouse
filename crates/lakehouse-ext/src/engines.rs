//! RFC 0255 §3.3: connecting a source engine. A [`SourceProvider`] is one engine — its pure
//! [`EngineSpec`], the `[extensions.lakehouse]` keys it reads, its read-only connection and its
//! extra doctor checks — and [`Engines`] is the set a build has, passed explicitly (RFC 0149:
//! dependency injection, never global state).
//!
//! PostgreSQL and Oracle are the two built-in providers. A private binary adds its own:
//!
//! ```ignore
//! ekos::app::main_with(Extensions::new(vec![
//!     Arc::new(LakehouseExtension::new().with_engine(Arc::new(sap::Provider))),
//! ]))
//! ```

use crate::doctor::Check;
use crate::settings::Settings;
use crate::source::Source;
use ekos_compiler_core::DatabaseConfig;
use ekos_lakehouse_bronze::{EngineSpec, SourceEngine};
use std::sync::Arc;

/// One source engine, as the Lakehouse connects it (RFC 0255 §3.3).
pub trait SourceProvider: Send + Sync {
    /// What the engine is without a connection: type map, watermark rule, capabilities.
    fn spec(&self) -> Arc<dyn EngineSpec>;

    /// Which `[extensions.lakehouse]` keys this engine reads and which it refuses (RFC 0216
    /// §2.2); may fill in what the settings name (Oracle: the `[[databases]]` entry). A refusal
    /// names the key and the engine. Default: every key accepted.
    fn check_settings(
        &self,
        _settings: &mut Settings,
        _databases: &[DatabaseConfig],
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// The engine, connected read-only.
    fn connect(&self, settings: &Settings, purpose: &str) -> anyhow::Result<Box<dyn Source>>;

    /// The canonical-form dialect the source side of a reconciliation is rendered in (RFC 0255
    /// §3.4): a built-in one, or `Dialect::Registered` with the engine's own `CanonDialect`. It
    /// must be the dialect of the connected source's `EngineReader`.
    fn dialect(&self) -> ekos_migrate_validate::Dialect;

    /// `host:port` the doctor's reachability check connects to. Default: `source`.
    fn address(&self, settings: &Settings) -> Option<String> {
        settings.source.clone()
    }

    /// The role the source is read as, for the doctor's report. Default: `source-user`.
    fn role(&self, settings: &Settings) -> String {
        settings.source_user.clone().unwrap_or_default()
    }

    /// Trino connector properties, besides `unsupported-type-handling=CONVERT_TO_VARCHAR`, that
    /// expose columns the connector hides for this engine (`lakehouse doctor`). Default: none.
    fn connector_hint(&self) -> &'static str {
        ""
    }

    /// Extra `lakehouse doctor` checks for this engine (RFC 0196 §4). Default: none.
    fn doctor(&self, _source: &dyn Source, _settings: &Settings) -> Vec<Check> {
        vec![]
    }

    /// A name for the provider in startup errors. Default: its Rust type.
    fn provider_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// The source engines of a build: a cheap clone over a shared list.
#[derive(Clone)]
pub struct Engines(Arc<[Arc<dyn SourceProvider>]>);

impl Engines {
    /// PostgreSQL and Oracle.
    pub fn builtin() -> Self {
        Self(Arc::from(vec![
            Arc::new(Postgres) as Arc<dyn SourceProvider>,
            Arc::new(Oracle) as Arc<dyn SourceProvider>,
        ]))
    }

    /// No engine at all, for building a set from scratch.
    pub fn empty() -> Self {
        Self(Arc::from(Vec::new()))
    }

    /// These engines and `provider`; refused when an engine of that name is already here, naming
    /// both providers (as EKOS refuses two extensions with one command, RFC 0191).
    pub fn try_with(self, provider: Arc<dyn SourceProvider>) -> anyhow::Result<Self> {
        let name = provider.spec().engine();
        if let Some(have) = self.0.iter().find(|p| p.spec().engine() == name) {
            anyhow::bail!(
                "two source engines are named `{name}`: {} and {}",
                have.provider_name(),
                provider.provider_name()
            );
        }
        let mut all: Vec<Arc<dyn SourceProvider>> = self.0.iter().cloned().collect();
        all.push(provider);
        Ok(Self(Arc::from(all)))
    }

    /// The engines' names, in registration order.
    pub fn names(&self) -> Vec<SourceEngine> {
        self.0.iter().map(|p| p.spec().engine()).collect()
    }

    pub fn providers(&self) -> impl Iterator<Item = &Arc<dyn SourceProvider>> {
        self.0.iter()
    }

    /// The provider of `engine`, or the refusal naming it (RFC 0255 §3.1).
    pub fn get(&self, engine: &SourceEngine) -> anyhow::Result<&Arc<dyn SourceProvider>> {
        self.0
            .iter()
            .find(|p| p.spec().engine() == *engine)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    ekos_lakehouse_bronze::engine::not_in_this_build(engine, &self.names())
                )
            })
    }

    pub fn spec(&self, engine: &SourceEngine) -> anyhow::Result<Arc<dyn EngineSpec>> {
        Ok(self.get(engine)?.spec())
    }
}

impl Default for Engines {
    fn default() -> Self {
        Self::builtin()
    }
}

impl std::fmt::Debug for Engines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

/// Two sets are equal when they name the same engines in the same order.
impl PartialEq for Engines {
    fn eq(&self, other: &Self) -> bool {
        self.names() == other.names()
    }
}

impl Eq for Engines {}

/// PostgreSQL, through EKOS's read-only session (RFC 0157).
pub struct Postgres;

impl SourceProvider for Postgres {
    fn spec(&self) -> Arc<dyn EngineSpec> {
        Arc::new(ekos_lakehouse_bronze::PostgresSpec)
    }

    fn connect(&self, s: &Settings, purpose: &str) -> anyhow::Result<Box<dyn Source>> {
        Ok(Box::new(crate::source::Postgres(connect_postgres(
            s, purpose,
        )?)))
    }

    fn dialect(&self) -> ekos_migrate_validate::Dialect {
        ekos_migrate_validate::Dialect::Postgres
    }

    fn connector_hint(&self) -> &'static str {
        "postgresql.array-mapping=AS_ARRAY and "
    }

    fn provider_name(&self) -> &'static str {
        "built-in postgres"
    }
}

/// The PostgreSQL source, through EKOS's read-only session (RFC 0157).
fn connect_postgres(s: &Settings, purpose: &str) -> anyhow::Result<ekos_pg_live::PgSource> {
    let (host, port) = s.source_host_port()?;
    let database = s
        .source_database
        .clone()
        .ok_or_else(|| anyhow::anyhow!("set `source-database` in [extensions.lakehouse]"))?;
    let conn =
        ekos_migrate::connection::ConnectionRef::parse(&format!("postgres://{host}/{database}"))?
            .with_secret_env(s.source_password_env.clone())
            .with_secret_file(
                s.source_password_file
                    .as_ref()
                    .map(std::path::PathBuf::from),
            );
    Ok(ekos_pg_live::PgSource::connect(
        &conn,
        &host,
        port,
        s.source_user.as_deref().unwrap_or("ekos"),
        purpose,
        &ekos_pg_live::SessionPolicy::default(),
    )?)
}

/// Oracle (RFC 0216): the connection is the project's `[[databases]]` entry.
pub struct Oracle;

impl SourceProvider for Oracle {
    fn spec(&self) -> Arc<dyn EngineSpec> {
        Arc::new(ekos_lakehouse_bronze::OracleSpec)
    }

    fn check_settings(
        &self,
        settings: &mut Settings,
        databases: &[DatabaseConfig],
    ) -> anyhow::Result<()> {
        crate::settings::resolve_database(settings, databases)
    }

    fn connect(&self, s: &Settings, purpose: &str) -> anyhow::Result<Box<dyn Source>> {
        let db = s.database.as_ref().ok_or_else(|| {
            anyhow::anyhow!("an Oracle source needs `source-database` naming a [[databases]] entry")
        })?;
        Ok(Box::new(crate::source_oracle::Oracle::connect(
            db, purpose,
        )?))
    }

    fn dialect(&self) -> ekos_migrate_validate::Dialect {
        ekos_migrate_validate::Dialect::Oracle
    }

    /// The entry's `host:port/service`, up to the service.
    fn address(&self, s: &Settings) -> Option<String> {
        s.database
            .as_ref()
            .map(|db| db.connect.split('/').next().unwrap_or_default().to_string())
    }

    fn role(&self, s: &Settings) -> String {
        s.database
            .as_ref()
            .map(|db| db.user.clone())
            .unwrap_or_default()
    }

    fn provider_name(&self) -> &'static str {
        "built-in oracle"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(&'static str);

    impl SourceProvider for Fixture {
        fn spec(&self) -> Arc<dyn EngineSpec> {
            struct Spec(&'static str);
            impl EngineSpec for Spec {
                fn engine(&self) -> SourceEngine {
                    SourceEngine::from_static(self.0)
                }
                fn map_column(
                    &self,
                    c: &ekos_lakehouse_bronze::SourceColumn,
                ) -> ekos_lakehouse_bronze::Mapping {
                    ekos_lakehouse_bronze::typemap::map_column(c)
                }
                fn orderable(&self, _: &str) -> bool {
                    false
                }
            }
            Arc::new(Spec(self.0))
        }
        fn connect(&self, _: &Settings, _: &str) -> anyhow::Result<Box<dyn Source>> {
            anyhow::bail!("fixture")
        }
        fn dialect(&self) -> ekos_migrate_validate::Dialect {
            ekos_migrate_validate::Dialect::Postgres
        }
    }

    #[test]
    fn the_builtins_are_postgres_then_oracle() {
        assert_eq!(
            Engines::builtin().names(),
            [SourceEngine::POSTGRES, SourceEngine::ORACLE]
        );
    }

    /// RFC 0255 §5: two providers with one engine name stop startup, naming both.
    #[test]
    fn two_providers_of_one_engine_are_refused_naming_both() {
        let e = Engines::builtin()
            .try_with(Arc::new(Fixture("oracle")))
            .unwrap_err()
            .to_string();
        assert!(
            e.starts_with("two source engines are named `oracle`"),
            "{e}"
        );
        assert!(e.contains("built-in oracle"), "{e}");
        assert!(e.contains("Fixture"), "{e}");
        let more = Engines::builtin()
            .try_with(Arc::new(Fixture("fixture")))
            .unwrap();
        assert_eq!(more.names().len(), 3);
        assert!(more.get(&SourceEngine::from_static("fixture")).is_ok());
    }

    /// RFC 0255 §3.1: an engine the build lacks is refused by its name.
    #[test]
    fn an_engine_the_build_lacks_is_refused_by_name() {
        let e = match Engines::builtin().get(&SourceEngine::new("sap").unwrap()) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("sap is not built in"),
        };
        assert!(
            e.starts_with("source engine `sap` is not in this build (it has: postgres, oracle)"),
            "{e}"
        );
    }
}
