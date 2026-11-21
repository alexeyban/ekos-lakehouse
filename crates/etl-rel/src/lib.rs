//! The engine-neutral relational IR of an ETL job (RFC 0266, extracted from RFC 0202's
//! `kettle-rel`): one typed node per step, each with its output schema and how faithfully it was
//! carried over, the database connection a table is read through, and the printer from the IR
//! to Trino SQL (`render`, from RFC 0204's `pentaho-dbt`). Pure.
//!
//! How a tool's steps become nodes stays with the tool: Kettle's in `kettle-rel`, Talend's in
//! RFC 0267.

pub mod model;
pub mod render;

pub use model::*;

use serde::{Deserialize, Serialize};

/// A database connection as a file or `shared.xml` defines it. Never holds a password: the XML
/// tree it is read from has none (see [`crate::xml::is_secret_element`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    /// Kettle's database type, e.g. `POSTGRESQL`, `MSSQLNATIVE`, `MYSQL`.
    pub db_type: String,
    /// `Native`, `JNDI`, `ODBC` …
    pub access: String,
    pub host: Option<String>,
    pub database: Option<String>,
    pub port: Option<String>,
    pub username: Option<String>,
    /// The `PREFERRED_SCHEMA_NAME` attribute, when set.
    pub schema: Option<String>,
}

/// Column types of the tables a transformation reads and writes. The translator's catalog is
/// the lakehouse lock file (silver models); tests use a fixed one.
pub trait Catalog {
    /// The columns of `table`, or `None` when it is not known.
    fn columns(&self, table: &TableRef) -> Option<Vec<Field>>;
}

/// A catalog that knows no table.
pub struct NoCatalog;

impl Catalog for NoCatalog {
    fn columns(&self, _table: &TableRef) -> Option<Vec<Field>> {
        None
    }
}
