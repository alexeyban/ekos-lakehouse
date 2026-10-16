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
    /// The source PostgreSQL as `host:port`. The doctor only checks it accepts connections.
    pub source: Option<String>,
    /// The source database, the role EKOS reads it as, and the environment variable that holds
    /// that role's password (never the password itself).
    pub source_database: Option<String>,
    pub source_user: Option<String>,
    pub source_password_env: Option<String>,
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
            },
        }
    }

    pub fn schemas(&self) -> Vec<String> {
        self.schemas
            .clone()
            .unwrap_or_else(|| vec!["public".into()])
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

/// The project's settings; an absent table means nothing is configured yet.
pub fn load(config: &ekos_compiler_core::EkosConfig) -> anyhow::Result<Settings> {
    Ok(config
        .extension_settings::<Settings>(crate::NAME)?
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

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
