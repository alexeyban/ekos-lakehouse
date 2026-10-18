//! The package model (RFC 0201).

use crate::catalogue::{Category, EntryCategory};
use crate::xml::Node;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// A named parameter with its default value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub default: Option<String>,
    pub description: Option<String>,
}

/// One step of a transformation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub type_id: String,
    pub category: Category,
    pub description: Option<String>,
    pub copies: String,
    pub distribute: bool,
    /// The step element, secrets and layout removed.
    pub xml: Node,
    pub line: u32,
}

/// A hop between two steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub from: String,
    pub to: String,
    pub enabled: bool,
    /// The error-handling hop of `from` (rows that failed in it).
    pub error: bool,
}

/// One transformation (`.ktr`, or one `<transformation>` of a repository export).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transformation {
    /// Package-relative path, `/`-separated.
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    /// The repository directory (`info/directory`), `/` for a plain file.
    pub directory: String,
    pub parameters: Vec<Parameter>,
    pub connections: Vec<Connection>,
    pub steps: Vec<Step>,
    pub hops: Vec<Hop>,
}

impl Transformation {
    pub fn step(&self, name: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.name == name)
    }

    /// Steps feeding `name` over enabled, non-error hops, in hop order.
    pub fn inputs_of(&self, name: &str) -> Vec<&str> {
        self.hops
            .iter()
            .filter(|h| h.to == name && h.enabled && !h.error)
            .map(|h| h.from.as_str())
            .collect()
    }

    /// Steps `name` feeds over enabled, non-error hops, in hop order.
    pub fn outputs_of(&self, name: &str) -> Vec<&str> {
        self.hops
            .iter()
            .filter(|h| h.from == name && h.enabled && !h.error)
            .map(|h| h.to.as_str())
            .collect()
    }
}

/// One job entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEntry {
    pub name: String,
    pub type_id: String,
    pub category: EntryCategory,
    /// `nr`: the copy number; the same entry may be drawn twice.
    pub copy: u32,
    pub xml: Node,
    pub line: u32,
}

/// When a job hop is followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HopCondition {
    Unconditional,
    OnSuccess,
    OnFailure,
}

/// A hop between two job entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobHop {
    pub from: String,
    pub to: String,
    pub enabled: bool,
    pub condition: HopCondition,
}

/// One job (`.kjb`, or one `<job>` of a repository export).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    pub directory: String,
    pub parameters: Vec<Parameter>,
    pub connections: Vec<Connection>,
    pub entries: Vec<JobEntry>,
    pub hops: Vec<JobHop>,
}

/// A plugin step declared by a `plugin.xml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugin {
    /// The step type id a `.ktr` names.
    pub id: String,
    /// The implementing class (`classname` attribute).
    pub class: String,
    /// Package-relative jar paths declared by the plugin, plus the jars beside `plugin.xml`.
    pub jars: Vec<String>,
    /// The `plugin.xml` path.
    pub path: String,
}

/// Something in the package that could not be read, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Diagnostic {
    pub path: String,
    pub message: String,
}

/// Which kind of file a link runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkTarget {
    Transformation,
    Job,
}

/// One file calling another: a job entry, or a mapping/executor step.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Link {
    /// The calling file's path.
    pub from: String,
    /// The entry or step that calls.
    pub via: String,
    /// Its type id (`TRANS`, `Mapping`, `TransExecutor` …).
    pub via_type: String,
    pub target_kind: LinkTarget,
    /// The resolved package path, or `None` when nothing in the package matched.
    pub to: Option<String>,
    /// What the file named, after variable resolution.
    pub reference: String,
}

/// A whole package.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Package {
    pub transformations: Vec<Transformation>,
    pub jobs: Vec<Job>,
    pub shared_connections: Vec<Connection>,
    /// `kettle.properties`, merged in path order.
    pub properties: BTreeMap<String, String>,
    pub plugins: Vec<Plugin>,
    pub links: Vec<Link>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Package {
    pub fn transformation(&self, path: &str) -> Option<&Transformation> {
        self.transformations.iter().find(|t| t.path == path)
    }

    pub fn job(&self, path: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.path == path)
    }

    /// The connection `name` as `file_connections` defines it, else as `shared.xml` does.
    pub fn connection<'a>(
        &'a self,
        file_connections: &'a [Connection],
        name: &str,
    ) -> Option<&'a Connection> {
        file_connections
            .iter()
            .find(|c| c.name == name)
            .or_else(|| self.shared_connections.iter().find(|c| c.name == name))
    }
}
