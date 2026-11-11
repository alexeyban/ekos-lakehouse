//! A fixture source engine for EKOS Lakehouse, built **out of tree** (RFC 0255 phase 5).
//!
//! `fixture` is a PostgreSQL-compatible database (it speaks PostgreSQL's wire protocol and SQL)
//! that is *not* the built-in `postgres` engine: it has its own name, its own type map rule (it
//! refuses `money`), its own capabilities (it reads no declared keys) and its own canonical-form
//! dialect, written here. Everything below uses only the public crates; nothing in them names it.
//!
//! The four pieces a sold engine writes:
//!
//! 1. [`FixtureSpec`] — `EngineSpec`: the type map, the watermark rule, the capabilities;
//! 2. [`FixtureCanon`] — `CanonDialect`: the canonical form for reconciliation;
//! 3. [`FixtureSource`] — `Source`: the engine, connected read-only;
//! 4. [`FixtureProvider`] — `SourceProvider`: settings, connection, dialect, conformance literals.
//!
//! `docs/writing-a-source-engine.md` walks through them.

use ekos_kir::source_capabilities::{Capabilities, Capability as C, Support};
use ekos_lakehouse_bronze::typemap::Lossiness;
use ekos_lakehouse_bronze::{EngineSpec, Mapping, SourceColumn, SourceEngine, SourceTable};
use ekos_lakehouse_ext::engines::SourceProvider;
use ekos_lakehouse_ext::settings::Settings;
use ekos_lakehouse_ext::source::Source;
use ekos_migrate_validate::conformance::Literals;
use ekos_migrate_validate::dialect::{CanonDialect, ColumnRule};
use ekos_migrate_validate::{Dialect, EngineReader, ReadError};
use std::sync::Arc;

/// The engine's name, as `source-engine = "fixture"` names it.
pub const ENGINE: SourceEngine = SourceEngine::from_static("fixture");

/// Piece 1: what the engine is without a connection.
pub struct FixtureSpec;

impl EngineSpec for FixtureSpec {
    fn engine(&self) -> SourceEngine {
        ENGINE
    }

    /// PostgreSQL's map, except `money`: its text depends on the server's locale, so it has no
    /// faithful landing here and refuses its table.
    fn map_column(&self, c: &SourceColumn) -> Mapping {
        let mut m = ekos_lakehouse_bronze::typemap::map_column(c);
        if c.pg_type == "money" {
            m.iceberg_type = String::new();
            m.lossiness = Lossiness::Refused;
            m.rule = "fixture-money".into();
            m.rationale = "money renders by the server's locale: no faithful landing".into();
            m.compare = None;
        }
        m
    }

    fn orderable(&self, source_type: &str) -> bool {
        source_type.starts_with("timestamp") || matches!(source_type, "bigint" | "integer")
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::of([
            (C::Catalog, Support::Provided),
            (C::ReadOnlyProof, Support::Provided),
            (
                C::DeclaredKeys,
                Support::absent("the fixture engine reads no constraints"),
            ),
            (C::Descriptions, Support::Provided),
            (C::ValueLists, Support::partial("enum types only")),
            (C::Routines, Support::Provided),
            (C::ColumnIdentity, Support::Provided),
            (
                C::UsageStats,
                Support::absent("the fixture engine reads no statistics"),
            ),
            (C::Profiling, Support::absent("no RFC 0252 flavor")),
            (C::Reconcile, Support::Provided),
        ])
    }
}

/// Piece 2: the canonical form (RFC 0155) in the engine's SQL — written independently of
/// EKOS's built-in PostgreSQL dialect, and trusted only after the conformance kit passes on a
/// live engine.
pub struct FixtureCanon;

/// A `'static` value: `Dialect::Registered(&CANON)` needs no registry.
pub static CANON: FixtureCanon = FixtureCanon;

impl CanonDialect for FixtureCanon {
    fn name(&self) -> &str {
        "fixture"
    }

    fn canon_body(&self, c: &str, rule: ColumnRule) -> String {
        let text = |e: String| format!("REPLACE(REPLACE({e}, '\\', '\\\\'), CHR(31), '\\x1f')");
        match rule {
            ColumnRule::Text | ColumnRule::Char | ColumnRule::Inet => {
                text(format!("CAST({c} AS TEXT)"))
            }
            ColumnRule::Bool => format!("(CASE WHEN {c} THEN 't' ELSE 'f' END)"),
            ColumnRule::Int => format!("CAST({c} AS TEXT)"),
            ColumnRule::Decimal(s) => format!(
                "TRIM(TO_CHAR({c}, 'FM9999999999999999999990{}{}'))",
                if s > 0 { "." } else { "" },
                "0".repeat(s as usize)
            ),
            ColumnRule::TimestampUtc => {
                format!("TO_CHAR({c} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US')")
            }
            ColumnRule::TimestampNaive => format!("TO_CHAR({c}, 'YYYY-MM-DD\"T\"HH24:MI:SS.US')"),
            ColumnRule::Date => format!("TO_CHAR({c}, 'YYYY-MM-DD')"),
            ColumnRule::Time => format!("TO_CHAR({c}, 'HH24:MI:SS.US')"),
            ColumnRule::Uuid => format!("LOWER(CAST({c} AS TEXT))"),
            ColumnRule::Bytes => format!("ENCODE({c}, 'hex')"),
        }
    }

    fn hash_hex_expr(&self, text: &str) -> String {
        format!("ENCODE(SHA256(CONVERT_TO({text}, 'UTF8')), 'hex')")
    }

    fn prefix60_expr(&self, hash: &str) -> String {
        format!("(('x' || SUBSTR({hash}, 1, 15))::BIT(60))::BIGINT")
    }
}

/// The dialect as the validator passes it.
pub fn dialect() -> Dialect {
    Dialect::Registered(&CANON)
}

/// The engine's read path: PostgreSQL's read-only session, reporting the fixture's dialect.
pub struct FixtureReader(pub ekos_pg_live::PgSource);

impl EngineReader for FixtureReader {
    fn dialect(&self) -> Dialect {
        dialect()
    }
    fn label(&self) -> &str {
        "fixture"
    }
    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, ReadError> {
        EngineReader::query(&self.0, sql)
    }
}

/// Piece 3: the engine, connected read-only.
pub struct FixtureSource {
    catalog: ekos_lakehouse_ext::source::Postgres,
    reader: FixtureReader,
}

impl Source for FixtureSource {
    fn engine(&self) -> SourceEngine {
        ENGINE
    }

    /// The catalog is PostgreSQL's; the engine reads no keys (its `declared_keys` is absent),
    /// so they are not handed on.
    fn read_tables(
        &self,
        trino: &ekos_lakehouse_trino::TrinoReader,
        trino_catalog: &str,
        schemas: &[String],
        redaction: &ekos_common::redaction::RedactionConfig,
        max_measured_rows: i64,
    ) -> anyhow::Result<Vec<SourceTable>> {
        let mut tables = self.catalog.read_tables(
            trino,
            trino_catalog,
            schemas,
            redaction,
            max_measured_rows,
        )?;
        for t in &mut tables {
            t.foreign_keys.clear();
        }
        Ok(tables)
    }

    fn reader(&self) -> &dyn EngineReader {
        &self.reader
    }

    fn writes_refused(&self) -> anyhow::Result<bool> {
        self.catalog.writes_refused()
    }
}

/// Piece 4: the provider the Lakehouse is given (`LakehouseExtension::new().with_engine(…)`).
pub struct FixtureProvider;

impl SourceProvider for FixtureProvider {
    fn spec(&self) -> Arc<dyn EngineSpec> {
        Arc::new(FixtureSpec)
    }

    /// The fixture reads `source`, `source-database`, `source-user`, `source-password-env`;
    /// it refuses `source-password-file`, naming the key and the engine.
    fn check_settings(
        &self,
        s: &mut Settings,
        _: &[ekos_compiler_core::DatabaseConfig],
    ) -> anyhow::Result<()> {
        if s.source_password_file.is_some() {
            anyhow::bail!("source-password-file is not read by the fixture engine");
        }
        if s.source_database.is_none() {
            anyhow::bail!("the fixture engine needs `source-database`");
        }
        Ok(())
    }

    fn connect(&self, s: &Settings, purpose: &str) -> anyhow::Result<Box<dyn Source>> {
        let (host, port) = s.source_host_port()?;
        let database = s.source_database.clone().unwrap_or_default();
        let conn = ekos_migrate::connection::ConnectionRef::parse(&format!(
            "postgres://{host}/{database}"
        ))?
        .with_secret_env(s.source_password_env.clone());
        let session = |p: &str| {
            ekos_pg_live::PgSource::connect(
                &conn,
                &host,
                port,
                s.source_user.as_deref().unwrap_or("ekos"),
                p,
                &ekos_pg_live::SessionPolicy::default(),
            )
        };
        Ok(Box::new(FixtureSource {
            catalog: ekos_lakehouse_ext::source::Postgres(session(purpose)?),
            reader: FixtureReader(session(purpose)?),
        }))
    }

    fn dialect(&self) -> Dialect {
        dialect()
    }

    fn conformance_literals(&self) -> Option<Box<dyn Literals>> {
        Some(Box::new(FixtureLiterals))
    }

    fn provider_name(&self) -> &'static str {
        "fixture engine (out of tree)"
    }
}

/// The literals the conformance kit renders the fixture's dialect over. The engine speaks
/// PostgreSQL's SQL, so its typed literals are PostgreSQL's; the dialect under test is ours.
pub struct FixtureLiterals;

impl Literals for FixtureLiterals {
    fn dialect(&self) -> Dialect {
        dialect()
    }
    fn literal(&self, case: &str) -> Option<String> {
        postgres_literals().literal(case)
    }
    fn unmapped(&self) -> Vec<(&'static str, &'static str)> {
        postgres_literals().unmapped()
    }
    fn bucket_rows(&self) -> (String, &'static str, &'static str) {
        postgres_literals().bucket_rows()
    }
}

fn postgres_literals() -> Box<dyn Literals> {
    ekos_migrate_validate::conformance::builtin(Dialect::Postgres).expect("built in")
}

/// The Lakehouse extension with the fixture engine registered beside the built-in ones.
pub fn lakehouse() -> ekos_lakehouse_ext::LakehouseExtension {
    ekos_lakehouse_ext::LakehouseExtension::new().with_engine(Arc::new(FixtureProvider))
}
