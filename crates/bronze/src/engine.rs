//! RFC 0255 §3.1–3.2: the source engine as an open identifier, and [`EngineSpec`] — everything
//! bronze decides from the engine, without a connection.
//!
//! PostgreSQL and Oracle are two registered specs ([`builtin`]); an engine built out of tree is
//! a third implementation of the same trait, handed to [`crate::plan::plan_for`] and the mode
//! functions by whoever registered it (the Lakehouse extension's `Engines`). Nothing here holds
//! a registry: specs are passed, never looked up in global state.

use crate::model::SourceColumn;
use crate::typemap::Mapping;
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;

/// A source engine's stable name: `postgres`, `oracle`, or a registered out-of-tree engine.
///
/// Serialized as the bare name, as the closed enum before it was (`"oracle"`), so a lock file
/// written before RFC 0255 reads and writes unchanged. A name follows the `[[databases]]` name
/// rule: `[a-z0-9][a-z0-9_-]{0,62}`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SourceEngine(Cow<'static, str>);

impl SourceEngine {
    pub const POSTGRES: Self = Self(Cow::Borrowed("postgres"));
    pub const ORACLE: Self = Self(Cow::Borrowed("oracle"));

    /// An engine by name, checked against the name rule.
    pub fn new(name: impl Into<String>) -> Result<Self, String> {
        let name = name.into();
        if !valid_name(&name) {
            return Err(format!(
                "{name:?} is not an engine name: lower-case letters, digits, `_` and `-`, at most \
                 63 characters, starting with a letter or digit"
            ));
        }
        Ok(match name.as_str() {
            "postgres" => Self::POSTGRES,
            "oracle" => Self::ORACLE,
            _ => Self(Cow::Owned(name)),
        })
    }

    /// An engine name known at compile time (an out-of-tree provider's own). Panics on a name
    /// that breaks the rule, which is a programming error in that provider, caught by its tests.
    pub const fn from_static(name: &'static str) -> Self {
        assert!(valid_name_const(name.as_bytes()), "not an engine name");
        Self(Cow::Borrowed(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The default: a lock file written for PostgreSQL does not name its engine, so it reads back
    /// byte-identical to one written before RFC 0216.
    pub fn is_postgres(&self) -> bool {
        self.0 == "postgres"
    }
}

impl Default for SourceEngine {
    fn default() -> Self {
        Self::POSTGRES
    }
}

impl std::fmt::Display for SourceEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SourceEngine {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Self::new(name).map_err(serde::de::Error::custom)
    }
}

fn valid_name(name: &str) -> bool {
    valid_name_const(name.as_bytes())
}

const fn valid_name_const(b: &[u8]) -> bool {
    if b.is_empty() || b.len() > 63 {
        return false;
    }
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let ok =
            c.is_ascii_lowercase() || c.is_ascii_digit() || (i > 0 && (c == b'_' || c == b'-'));
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

/// What an engine is without a connection (RFC 0255 §3.2). Pure: no database, no I/O.
///
/// [`crate::typemap::Lossiness`] and [`crate::typemap::CompareRule`] stay closed: an engine maps
/// its types into them and cannot add a member. A type with no faithful landing is
/// [`crate::typemap::Lossiness::Refused`], naming the column.
pub trait EngineSpec: Send + Sync {
    fn engine(&self) -> SourceEngine;
    /// The engine's type → Iceberg type, its lossiness and its compare rule (RFC 0192, 0216 §2.4).
    fn map_column(&self, column: &SourceColumn) -> Mapping;
    /// Whether a column of this source type can order a watermark (`mode.rs`).
    fn orderable(&self, source_type: &str) -> bool;
}

/// PostgreSQL (RFC 0192).
#[derive(Debug, Clone, Copy, Default)]
pub struct PostgresSpec;

impl EngineSpec for PostgresSpec {
    fn engine(&self) -> SourceEngine {
        SourceEngine::POSTGRES
    }
    fn map_column(&self, column: &SourceColumn) -> Mapping {
        crate::typemap::map_column(column)
    }
    fn orderable(&self, source_type: &str) -> bool {
        source_type.starts_with("timestamp") || matches!(source_type, "date" | "integer" | "bigint")
    }
}

/// Oracle (RFC 0216).
#[derive(Debug, Clone, Copy, Default)]
pub struct OracleSpec;

impl EngineSpec for OracleSpec {
    fn engine(&self) -> SourceEngine {
        SourceEngine::ORACLE
    }
    fn map_column(&self, column: &SourceColumn) -> Mapping {
        crate::typemap_oracle::map_column(column)
    }
    /// A DATE carries a time; an integer NUMBER orders like one.
    fn orderable(&self, source_type: &str) -> bool {
        source_type == "DATE"
            || source_type.starts_with("TIMESTAMP")
            || source_type == "INTEGER"
            || (source_type.starts_with("NUMBER(") && source_type.ends_with(",0)"))
    }
}

static POSTGRES: PostgresSpec = PostgresSpec;
static ORACLE: OracleSpec = OracleSpec;

/// The engines every build has, in registration order.
pub fn builtins() -> [&'static dyn EngineSpec; 2] {
    [&POSTGRES, &ORACLE]
}

/// A built-in engine's spec, or `None` for an engine this crate does not define.
pub fn builtin(engine: &SourceEngine) -> Option<&'static dyn EngineSpec> {
    builtins().into_iter().find(|s| s.engine() == *engine)
}

/// The refusal for a lock file or setting naming an engine the running build lacks (RFC 0255
/// §3.1): a pipeline planned by a build with a sold engine, opened by the public build.
pub fn not_in_this_build(engine: &SourceEngine, have: &[SourceEngine]) -> String {
    let have: Vec<&str> = have.iter().map(SourceEngine::as_str).collect();
    format!(
        "source engine `{engine}` is not in this build (it has: {}); the pipeline was planned by \
         a build that includes it",
        have.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_as_the_enum_did() {
        for (e, text) in [
            (SourceEngine::POSTGRES, "\"postgres\""),
            (SourceEngine::ORACLE, "\"oracle\""),
        ] {
            assert_eq!(serde_json::to_string(&e).unwrap(), text);
            assert_eq!(serde_json::from_str::<SourceEngine>(text).unwrap(), e);
        }
        let sap: SourceEngine = serde_json::from_str("\"sap-hana\"").unwrap();
        assert_eq!(sap.as_str(), "sap-hana");
        assert!(!sap.is_postgres());
        assert!(builtin(&sap).is_none());
    }

    #[test]
    fn a_bad_name_is_refused() {
        for bad in ["", "Oracle", "-x", "a b", &"x".repeat(64)] {
            assert!(SourceEngine::new(bad).is_err(), "{bad:?}");
            assert!(serde_json::from_str::<SourceEngine>(&format!("{bad:?}")).is_err());
        }
        assert!(SourceEngine::new("db2_z-os").is_ok());
        assert_eq!(
            SourceEngine::from_static("fixture"),
            SourceEngine::new("fixture").unwrap()
        );
    }

    #[test]
    fn the_builtins_are_postgres_and_oracle() {
        let names: Vec<SourceEngine> = builtins().iter().map(|s| s.engine()).collect();
        assert_eq!(names, [SourceEngine::POSTGRES, SourceEngine::ORACLE]);
        assert!(
            builtin(&SourceEngine::ORACLE)
                .unwrap()
                .orderable("NUMBER(10,0)")
        );
        assert!(!builtin(&SourceEngine::ORACLE).unwrap().orderable("NUMBER"));
        assert!(
            builtin(&SourceEngine::POSTGRES)
                .unwrap()
                .orderable("bigint")
        );
        let e = not_in_this_build(&SourceEngine::new("sap").unwrap(), &names);
        assert!(
            e.starts_with("source engine `sap` is not in this build (it has: postgres, oracle)")
        );
    }
}
