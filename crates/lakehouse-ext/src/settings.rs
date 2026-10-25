//! `[extensions.lakehouse]` in `ekos.toml` (RFC 0191 settings table).
//!
//! Endpoints only. Credentials are never written here: a secret is named by the environment
//! variable that holds it, the rule EKOS Migrate already follows (RFC 0157).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Settings {
    /// Trino coordinator, e.g. `http://127.0.0.1:58080`.
    pub trino_url: Option<String>,
    /// Polaris health endpoint (management port), e.g. `http://127.0.0.1:58182/q/health`.
    pub polaris_health_url: Option<String>,
    /// The S3 endpoint the warehouse lives on (SeaweedFS in the sandbox), e.g.
    /// `http://127.0.0.1:58333`.
    pub s3_url: Option<String>,
    /// Dagster webserver, e.g. `http://127.0.0.1:53000`.
    pub dagster_url: Option<String>,
    /// RFC 0216: `postgres` (default) or `oracle`.
    pub source_engine: Option<ekos_lakehouse_bronze::SourceEngine>,
    /// The source PostgreSQL as `host:port`. The doctor only checks it accepts connections.
    pub source: Option<String>,
    /// The source database, the role EKOS reads it as, and the environment variable that holds
    /// that role's password (never the password itself). For `oracle`, `source-database` names
    /// the project's `[[databases]]` entry instead, which holds the connection (RFC 0211): one
    /// definition for EKOS's analysis and the lakehouse.
    pub source_database: Option<String>,
    pub source_user: Option<String>,
    pub source_password_env: Option<String>,
    /// RFC 0199: a file holding the source password (owner-readable only), relative to the
    /// workspace — read when `source-password-env` is unset or not exported, as in a console job
    /// or the orchestrator's container. The installer writes it; ekos.toml holds only the path.
    pub source_password_file: Option<String>,
    /// The Trino catalog the source is read through (RFC 0192 §1 says which settings it needs).
    pub source_catalog: Option<String>,
    /// The Iceberg catalog in Trino that bronze is written to.
    pub target_catalog: Option<String>,
    /// Source schemas to land. Default: `public`.
    pub schemas: Option<Vec<String>>,
    /// Bronze namespace prefix. Default: `bronze_`.
    pub namespace_prefix: Option<String>,
    /// `"schema.table" = "column"` watermarks (RFC 0192 §3).
    pub watermarks: std::collections::BTreeMap<String, String>,
    pub full_refresh_max_rows: Option<i64>,
    /// The pipeline repository `generate` writes and `reconcile` reads the deployed plan from,
    /// relative to the project. Default: `pipeline`.
    pub pipeline_dir: Option<String>,
    /// Cron for the bronze job (default hourly) and for Iceberg maintenance (default 03:00).
    pub bronze_schedule: Option<String>,
    pub maintenance_schedule: Option<String>,
    /// Approved masks, `"schema.table.column" = "null" | "hmac"` (RFC 0193 §2).
    pub pii: std::collections::BTreeMap<String, ekos_lakehouse_silver::MaskMethod>,
    /// Tables that keep SCD2 history in silver (RFC 0193 §4).
    pub history: Vec<String>,
    /// A change class's action, overriding RFC 0190's default (`rename = "auto"`), RFC 0194.
    pub policy: ekos_lakehouse_changes::Policy,
    /// Push proposal branches to this git remote and open pull requests with `gh` (RFC 0194).
    pub git_remote: Option<String>,
    /// The local Ollama model that drafts a gold mart's prose terms (RFC 0195). Default `llama3`.
    pub gold_model: Option<String>,
    /// Default `http://127.0.0.1:11434`.
    pub ollama_url: Option<String>,
    /// A table with no successful run for this long is late (RFC 0196). Default 120.
    pub late_after_minutes: Option<i64>,
    /// The environment variable holding the alert webhook's URL (a URL with a token is a secret).
    pub alert_webhook_env: Option<String>,
    /// An OTLP/HTTP endpoint (a collector), e.g. `http://127.0.0.1:54318`.
    pub otlp_endpoint: Option<String>,
    /// Pentaho packages to translate, `name = "path"` relative to the workspace (RFC 0200).
    /// Empty: every observed directory that holds a `.ktr` or `.kjb`.
    pub pentaho_packages: std::collections::BTreeMap<String, String>,
    /// Kettle reads `''` as null; silver models apply `nullif(c, '')` to text (RFC 0202).
    /// Default true; false for a Kettle run with `KETTLE_EMPTY_STRING_DIFFERS_FROM_NULL=Y`.
    pub pentaho_empty_string_is_null: Option<bool>,
    /// EKOS RFC 0213: `generate`, `sync` and `gold draft` refuse to run until every review
    /// subject of the source model has a valid sign-off. Default false.
    pub require_signoff: Option<bool>,
    /// RFC 0225: the column the application sets on every write it makes (iDempiere `UPDATED`).
    /// A table loads incrementally on it where EKOS's write-behaviour facts say the code keeps it.
    pub application_watermark: Option<String>,
    /// Tables smaller than this stay in full even with an application watermark. Default 0.
    pub incremental_min_rows: Option<i64>,
    /// Days between delete audits of tables the code never hard-deletes. Default 7.
    pub delete_audit_days: Option<u32>,
    /// `"schema.table" = "retain" | "propagate"` for tables only purged. Default `retain`.
    pub purge_policy: std::collections::BTreeMap<String, ekos_lakehouse_bronze::PurgePolicy>,
    /// The mass-delete guard (plan D9): more than this percentage of a table's keys, or more
    /// than `mass-delete-rows` keys, vanishing in one check is not applied. Defaults 5 and 10,000.
    pub mass_delete_percent: Option<u32>,
    pub mass_delete_rows: Option<u64>,
    /// For `oracle`: the `[[databases]]` entry `source-database` names, filled in by [`load`].
    #[serde(skip)]
    pub database: Option<ekos_compiler_core::DatabaseConfig>,
}

impl Settings {
    pub fn silver_settings(&self) -> ekos_lakehouse_silver::SilverSettings {
        ekos_lakehouse_silver::SilverSettings {
            pii: self.pii.clone(),
            history: self.history.clone(),
        }
    }
}

impl Settings {
    pub fn plan_settings(&self) -> ekos_lakehouse_bronze::plan::PlanSettings {
        let d = ekos_lakehouse_bronze::plan::PlanSettings::default();
        ekos_lakehouse_bronze::plan::PlanSettings {
            source_catalog: self.source_catalog.clone().unwrap_or(d.source_catalog),
            target_catalog: self.target_catalog.clone().unwrap_or(d.target_catalog),
            namespace_prefix: self.namespace_prefix.clone().unwrap_or(d.namespace_prefix),
            modes: ekos_lakehouse_bronze::ModeSettings {
                watermarks: self.watermarks.clone(),
                full_refresh_max_rows: self
                    .full_refresh_max_rows
                    .unwrap_or(d.modes.full_refresh_max_rows),
                application_watermark: self.application_watermark.clone(),
                incremental_min_rows: self.incremental_min_rows.unwrap_or(0),
                delete_audit_days: self.delete_audit_days.unwrap_or(d.modes.delete_audit_days),
                purge_policy: self.purge_policy.clone(),
            },
            bronze_schedule: self.bronze_schedule.clone().unwrap_or(d.bronze_schedule),
            maintenance_schedule: self
                .maintenance_schedule
                .clone()
                .unwrap_or(d.maintenance_schedule),
            source_engine: self.engine(),
        }
    }

    pub fn engine(&self) -> ekos_lakehouse_bronze::SourceEngine {
        self.source_engine.unwrap_or_default()
    }

    /// The schemas to land. Default: `public`; for `oracle`, the database entry's owners.
    pub fn schemas(&self) -> Vec<String> {
        self.schemas
            .clone()
            .unwrap_or_else(|| match &self.database {
                Some(db) => db.owners.clone(),
                None => vec!["public".into()],
            })
    }

    /// Host and port of `source`.
    pub fn source_host_port(&self) -> anyhow::Result<(String, u16)> {
        let s = self
            .source
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("set `source` (host:port) in [extensions.lakehouse]"))?;
        let (h, p) = s
            .rsplit_once(':')
            .ok_or_else(|| anyhow::anyhow!("`source` must be host:port, got {s}"))?;
        Ok((h.to_string(), p.parse()?))
    }

    pub fn trino_connection(&self) -> anyhow::Result<ekos_lakehouse_trino::Connection> {
        Ok(ekos_lakehouse_trino::Connection {
            base_url: self
                .trino_url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("set `trino-url` in [extensions.lakehouse]"))?,
            user: "ekos".into(),
            catalog: None,
            schema: None,
        })
    }
}

/// What a relative `pipeline-dir` is resolved against. The workspace, as before — except for an
/// EKOS home project whose workspace is someone's source tree (no installer marker
/// `.lakehouse/project.json` there): its pipeline goes beside its `ekos.toml` in the home, because
/// a project run must leave the sources untouched (EKOS RFC 0178).
pub fn pipeline_base(cwd: &std::path::Path, config_path: &std::path::Path) -> std::path::PathBuf {
    let config_dir = config_path.parent().unwrap_or(cwd);
    let home_project = !same_dir(config_dir, cwd);
    if home_project && !cwd.join(".lakehouse/project.json").is_file() {
        config_dir.to_path_buf()
    } else {
        cwd.to_path_buf()
    }
}

fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.as_os_str().is_empty()
        || a == b
        || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// The project's settings; an absent table means nothing is configured yet. For an Oracle source,
/// `source-database` must name one of the project's `[[databases]]` entries (RFC 0216 §2.2).
pub fn load(config: &ekos_compiler_core::EkosConfig) -> anyhow::Result<Settings> {
    let mut s = config
        .extension_settings::<Settings>(crate::NAME)?
        .unwrap_or_default();
    if s.engine() == ekos_lakehouse_bronze::SourceEngine::Oracle {
        resolve_database(&mut s, &config.databases)?;
    }
    Ok(s)
}

fn resolve_database(
    s: &mut Settings,
    databases: &[ekos_compiler_core::DatabaseConfig],
) -> anyhow::Result<()> {
    let postgres_only = [
        ("source", s.source.is_some()),
        ("source-user", s.source_user.is_some()),
        ("source-password-env", s.source_password_env.is_some()),
        ("source-password-file", s.source_password_file.is_some()),
    ];
    let set: Vec<&str> = postgres_only
        .iter()
        .filter(|(_, on)| *on)
        .map(|(k, _)| *k)
        .collect();
    if !set.is_empty() {
        anyhow::bail!(
            "{} {} PostgreSQL's: an Oracle source's connection is its [[databases]] entry",
            set.join(", "),
            if set.len() == 1 { "is" } else { "are" }
        );
    }
    let Some(name) = s.source_database.clone() else {
        anyhow::bail!("set `source-database` to the name of a [[databases]] entry");
    };
    let db = databases.iter().find(|d| d.name == name).ok_or_else(|| {
        anyhow::anyhow!(
            "source-database = {name:?} names no [[databases]] entry (have: {})",
            databases
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    s.database = Some(db.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORACLE: &str = r#"
[[databases]]
name = "erp"
kind = "oracle"
connect = "db:1521/XEPDB1"
user = "ekos_ro"
password-env = "ERP_PW"
owners = ["ADEMPIERE"]
"#;

    /// A home project's pipeline never lands in its sources (EKOS RFC 0178).
    #[test]
    fn a_home_projects_pipeline_is_kept_out_of_its_sources() {
        let sources = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("ekos.toml");
        assert_eq!(pipeline_base(sources.path(), &config), home.path());
        // A project in its own directory, as before.
        let local = sources.path().join("ekos.toml");
        assert_eq!(pipeline_base(sources.path(), &local), sources.path());
        // The installer's projects: the workspace is the pipeline's own directory.
        std::fs::create_dir_all(sources.path().join(".lakehouse")).unwrap();
        std::fs::write(sources.path().join(".lakehouse/project.json"), "{}").unwrap();
        assert_eq!(pipeline_base(sources.path(), &config), sources.path());
    }

    /// RFC 0216 §2.2: an Oracle source's connection is its [[databases]] entry.
    #[test]
    fn an_oracle_source_is_a_databases_entry() {
        let c: ekos_compiler_core::EkosConfig = toml::from_str(&format!(
            "{ORACLE}[extensions.lakehouse]\nsource-engine = \"oracle\"\nsource-database = \"erp\"\n"
        ))
        .unwrap();
        let s = load(&c).unwrap();
        assert_eq!(s.database.as_ref().unwrap().connect, "db:1521/XEPDB1");
        assert_eq!(s.schemas(), ["ADEMPIERE"]);
        assert_eq!(
            s.plan_settings().source_engine,
            ekos_lakehouse_bronze::SourceEngine::Oracle
        );

        let unknown: ekos_compiler_core::EkosConfig = toml::from_str(&format!(
            "{ORACLE}[extensions.lakehouse]\nsource-engine = \"oracle\"\nsource-database = \"crm\"\n"
        ))
        .unwrap();
        let e = load(&unknown).unwrap_err().to_string();
        assert!(
            e.contains("names no [[databases]] entry (have: erp)"),
            "{e}"
        );

        let mixed: ekos_compiler_core::EkosConfig = toml::from_str(&format!(
            "{ORACLE}[extensions.lakehouse]\nsource-engine = \"oracle\"\nsource-database = \"erp\"\n\
             source = \"db:5432\"\nsource-user = \"x\"\n"
        ))
        .unwrap();
        let e = load(&mixed).unwrap_err().to_string();
        assert!(e.starts_with("source, source-user are PostgreSQL's"), "{e}");
    }

    #[test]
    fn reads_its_table_and_refuses_unknown_keys() {
        let c: ekos_compiler_core::EkosConfig = toml::from_str(
            "[extensions.lakehouse]\ntrino-url = \"http://t:8080\"\nsource = \"db:5432\"\n",
        )
        .unwrap();
        let s = load(&c).unwrap();
        assert_eq!(s.trino_url.as_deref(), Some("http://t:8080"));
        assert_eq!(s.source.as_deref(), Some("db:5432"));
        assert_eq!(s.s3_url, None);

        let c: ekos_compiler_core::EkosConfig =
            toml::from_str("[extensions.lakehouse]\npassword = \"x\"\n").unwrap();
        let err = load(&c).unwrap_err().to_string();
        assert!(err.contains("[extensions.lakehouse]"), "{err}");

        let none = load(&ekos_compiler_core::EkosConfig::default()).unwrap();
        assert_eq!(none, Settings::default());
    }
}
