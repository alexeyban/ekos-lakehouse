//! `[extensions.lakehouse]` in `ekos.toml` (RFC 0191 settings table).
//!
//! Endpoints only. Credentials are never written here: a secret is named by the environment
//! variable that holds it, the rule EKOS Migrate already follows (RFC 0157).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Settings {
    /// Trino coordinator, e.g. `http://127.0.0.1:58080`.
    pub trino_url: Option<String>,
    /// Polaris health endpoint (management port), e.g. `http://127.0.0.1:58182/q/health`.
    pub polaris_health_url: Option<String>,
    /// MinIO (S3 API), e.g. `http://127.0.0.1:59000`.
    pub minio_url: Option<String>,
    /// Dagster webserver, e.g. `http://127.0.0.1:53000`.
    pub dagster_url: Option<String>,
    /// The source PostgreSQL as `host:port`. The doctor only checks it accepts connections.
    pub source: Option<String>,
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
        assert_eq!(s.minio_url, None);

        let c: ekos_compiler_core::EkosConfig =
            toml::from_str("[extensions.lakehouse]\npassword = \"x\"\n").unwrap();
        let err = load(&c).unwrap_err().to_string();
        assert!(err.contains("[extensions.lakehouse]"), "{err}");

        let none = load(&ekos_compiler_core::EkosConfig::default()).unwrap();
        assert_eq!(none, Settings::default());
    }
}
