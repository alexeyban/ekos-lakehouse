//! `ekos-lakehouse lakehouse doctor`: does every service of the stack answer?
//!
//! Reachability and readiness only. It sends no credentials and reads no data: the Trino check
//! is `/v1/info`, Polaris its health endpoint, the S3 store any answer at all (stores differ in
//! health endpoints; an unauthenticated request is refused, which still proves it is up), Dagster
//! `/server_info`, and the source a TCP connect.

use crate::settings::Settings;
use serde::Serialize;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Failed,
    NotConfigured,
    /// Works, but not the way it should (a source role that can write).
    Warning,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub service: &'static str,
    pub target: Option<String>,
    pub status: Status,
    pub detail: String,
}

impl Check {
    pub fn not_configured(service: &'static str, key: &str) -> Self {
        Self {
            service,
            target: None,
            status: Status::NotConfigured,
            detail: format!("set `{key}` in [extensions.lakehouse]"),
        }
    }

    pub fn of(service: &'static str, target: String, outcome: Result<String, String>) -> Self {
        let (status, detail) = match outcome {
            Ok(d) => (Status::Ok, d),
            Err(d) => (Status::Failed, d),
        };
        Self {
            service,
            target: Some(target),
            status,
            detail,
        }
    }
}

/// Every check, in a fixed order.
pub async fn run(s: &Settings) -> Vec<Check> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .expect("a client with only a timeout set always builds");
    let mut out = Vec::new();

    out.push(match &s.trino_url {
        None => Check::not_configured("trino", "trino-url"),
        Some(base) => {
            let url = format!("{}/v1/info", base.trim_end_matches('/'));
            Check::of("trino", url.clone(), trino(&client, &url).await)
        }
    });
    out.push(match &s.polaris_health_url {
        None => Check::not_configured("polaris", "polaris-health-url"),
        Some(url) => Check::of("polaris", url.clone(), health_up(&client, url).await),
    });
    out.push(match &s.s3_url {
        None => Check::not_configured("s3", "s3-url"),
        Some(url) => Check::of("s3", url.clone(), answers(&client, url).await),
    });
    out.push(match &s.dagster_url {
        None => Check::not_configured("dagster", "dagster-url"),
        Some(base) => {
            let url = format!("{}/server_info", base.trim_end_matches('/'));
            Check::of("dagster", url.clone(), status_ok(&client, &url).await)
        }
    });
    // An Oracle source is reached at its [[databases]] entry's `host:port/service` (RFC 0216).
    let addr = match &s.database {
        Some(db) => Some(db.connect.split('/').next().unwrap_or_default().to_string()),
        None => s.source.clone(),
    };
    out.push(match &addr {
        None => Check::not_configured("source", "source"),
        Some(addr) => Check::of("source", addr.clone(), tcp(addr).await),
    });
    out
}

async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(resp)
}

async fn trino(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let info: serde_json::Value = get(client, url)
        .await?
        .json()
        .await
        .map_err(|e| format!("not Trino's /v1/info: {e}"))?;
    if info["starting"].as_bool() != Some(false) {
        return Err("Trino is still starting".into());
    }
    Ok(format!(
        "version {}",
        info["nodeVersion"]["version"].as_str().unwrap_or("unknown")
    ))
}

async fn health_up(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let body: serde_json::Value = get(client, url)
        .await?
        .json()
        .await
        .map_err(|e| format!("not a health document: {e}"))?;
    match body["status"].as_str() {
        Some("UP") => Ok("UP".into()),
        other => Err(format!("status {}", other.unwrap_or("missing"))),
    }
}

async fn status_ok(client: &reqwest::Client, url: &str) -> Result<String, String> {
    get(client, url)
        .await
        .map(|r| format!("HTTP {}", r.status()))
}

/// Up when it answers with anything but a server error: S3 refuses an anonymous request with
/// 403, and that refusal is the proof.
async fn answers(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if resp.status().is_server_error() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(format!("answers (HTTP {})", resp.status().as_u16()))
}

async fn tcp(addr: &str) -> Result<String, String> {
    match tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(_)) => Ok("accepts connections".into()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {}s", TIMEOUT.as_secs())),
    }
}

/// Checks that need the source and the engine together (RFC 0196 §4): failures that would
/// otherwise happen later and quietly. Blocking: the caller runs it off the async runtime.
pub fn deep(s: &Settings, redaction: &ekos_common::redaction::RedactionConfig) -> Vec<Check> {
    let mut out = vec![];
    if s.source_database.is_none() || s.trino_url.is_none() {
        out.push(Check::not_configured(
            "columns",
            "source-database and trino-url",
        ));
        return out;
    }
    let pg = crate::plan_cmd::connect_source(s, "lakehouse-doctor");
    let trino = s
        .trino_connection()
        .and_then(|c| ekos_lakehouse_trino::TrinoReader::new(c).map_err(Into::into));
    match (&pg, &trino) {
        (Ok(pg), Ok(trino)) => {
            let ps = s.plan_settings();
            let target = format!(
                "{} via catalog {}",
                s.source_database.as_deref().unwrap_or("?"),
                ps.source_catalog
            );
            match pg.read_tables(trino, &ps.source_catalog, &s.schemas(), redaction, 0) {
                Ok(tables) => {
                    let hidden: Vec<String> = tables
                        .iter()
                        .flat_map(|t| {
                            t.columns
                                .iter()
                                .filter(|c| c.trino_type.is_none())
                                .map(move |c| format!("{}.{}.{}", t.schema, t.name, c.name))
                        })
                        .collect();
                    let n: usize = tables.iter().map(|t| t.columns.len()).sum();
                    out.push(Check::of(
                        "columns",
                        target,
                        if hidden.is_empty() {
                            Ok(format!("the connector exposes all {n} columns of {} tables", tables.len()))
                        } else {
                            Err(format!(
                                "the connector hides {} of {n} columns ({}…): set {}unsupported-type-handling=CONVERT_TO_VARCHAR \
                                 on catalog {}",
                                hidden.len(),
                                hidden.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
                                if pg.engine() == ekos_lakehouse_bronze::SourceEngine::Postgres {
                                    "postgresql.array-mapping=AS_ARRAY and "
                                } else {
                                    ""
                                },
                                ps.source_catalog
                            ))
                        },
                    ));
                }
                Err(e) => out.push(Check::of("columns", target, Err(e.to_string()))),
            }
            let user = match &s.database {
                Some(db) => db.user.clone(),
                None => s.source_user.clone().unwrap_or_default(),
            };
            out.push(match pg.writes_refused() {
                Ok(true) => Check::of("role", user.clone(), Ok("the source refuses this role's writes".into())),
                Ok(_) => Check {
                    service: "role",
                    target: (!user.is_empty()).then(|| user.clone()),
                    status: Status::Warning,
                    detail: "this role can write to the source: EKOS only reads, but a read-only role is the guarantee".into(),
                },
                Err(e) => Check::of("role", user.clone(), Err(e.to_string())),
            });
            let catalog = s.plan_settings().target_catalog;
            out.push(Check::of(
                "iceberg",
                catalog.clone(),
                ekos_lakehouse_trino::Client::new(s.trino_connection().expect("checked"))
                    .and_then(|c| {
                        c.execute(&format!(
                            "SHOW SCHEMAS FROM \"{}\"",
                            catalog.replace('"', "\"\"")
                        ))
                    })
                    .map(|r| format!("the catalog answers ({} namespaces)", r.rows.len()))
                    .map_err(|e| e.to_string()),
            ));
        }
        (Err(e), _) => out.push(Check::of("columns", "source".into(), Err(e.to_string()))),
        (_, Err(e)) => out.push(Check::of("columns", "trino".into(), Err(e.to_string()))),
    }
    if let Some(model) = &s.gold_model {
        let url = format!(
            "{}/api/tags",
            s.ollama_url.as_deref().unwrap_or("http://127.0.0.1:11434")
        );
        let found = reqwest::blocking::get(&url)
            .and_then(|r| r.json::<serde_json::Value>())
            .map_err(|e| e.to_string())
            .and_then(|v| {
                let names: Vec<String> = v["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m["name"].as_str().map(str::to_string))
                    .collect();
                if names
                    .iter()
                    .any(|n| n == model || n.split(':').next() == Some(model.as_str()))
                {
                    Ok(format!("{model} is available"))
                } else {
                    Err(format!(
                        "{model} is not pulled (have: {})",
                        names.join(", ")
                    ))
                }
            });
        out.push(Check::of("gold-model", url, found));
    }
    out
}

pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        let mark = match c.status {
            Status::Ok => "ok  ",
            Status::Failed => "FAIL",
            Status::NotConfigured => "--  ",
            Status::Warning => "WARN",
        };
        out.push_str(&format!(
            "{mark}  {:<8} {}  {}\n",
            c.service,
            c.target.as_deref().unwrap_or("-"),
            c.detail
        ));
    }
    out
}

/// Failure when any configured service failed, or when nothing is configured at all (a doctor
/// that checked nothing must not read as healthy).
pub fn verdict(checks: &[Check]) -> anyhow::Result<()> {
    let failed: Vec<&str> = checks
        .iter()
        .filter(|c| c.status == Status::Failed)
        .map(|c| c.service)
        .collect();
    if !failed.is_empty() {
        anyhow::bail!("not reachable: {}", failed.join(", "));
    }
    if checks
        .iter()
        .all(|c| matches!(c.status, Status::NotConfigured | Status::Warning))
    {
        anyhow::bail!("nothing to check: [extensions.lakehouse] names no service");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A server answering every request with `body` (JSON) and `status`.
    fn serve(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    /// A port nothing listens on.
    fn closed() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        drop(l);
        a.to_string()
    }

    #[tokio::test]
    async fn healthy_services_pass() {
        let trino = serve(
            "200 OK",
            r#"{"nodeVersion":{"version":"476"},"starting":false}"#,
        );
        let polaris = serve("200 OK", r#"{"status":"UP","checks":[]}"#);
        let s3 = serve("403 Forbidden", "<Error/>");
        let source = TcpListener::bind("127.0.0.1:0").unwrap();
        let s = Settings {
            trino_url: Some(trino),
            polaris_health_url: Some(format!("{polaris}/q/health")),
            s3_url: Some(s3),
            dagster_url: None,
            source: Some(source.local_addr().unwrap().to_string()),
            ..Default::default()
        };
        let checks = run(&s).await;
        let by = |n: &str| checks.iter().find(|c| c.service == n).unwrap().clone();
        assert_eq!(by("trino").status, Status::Ok, "{checks:?}");
        assert_eq!(by("trino").detail, "version 476");
        assert_eq!(by("polaris").status, Status::Ok);
        assert_eq!(
            by("s3").status,
            Status::Ok,
            "a refusal still proves the store is up"
        );
        assert_eq!(by("dagster").status, Status::NotConfigured);
        assert_eq!(by("source").status, Status::Ok);
        verdict(&checks).unwrap();
        assert!(render(&checks).contains("--    dagster"));
    }

    #[tokio::test]
    async fn a_starting_trino_a_down_polaris_and_a_closed_port_fail() {
        let s = Settings {
            trino_url: Some(serve("200 OK", r#"{"starting":true}"#)),
            polaris_health_url: Some(serve("503 Service Unavailable", r#"{"status":"DOWN"}"#)),
            s3_url: Some(serve("500 Internal Server Error", "")),
            dagster_url: None,
            source: Some(closed()),
            ..Default::default()
        };
        let checks = run(&s).await;
        for svc in ["trino", "polaris", "s3", "source"] {
            let c = checks.iter().find(|c| c.service == svc).unwrap();
            assert_eq!(c.status, Status::Failed, "{svc}: {c:?}");
        }
        let err = verdict(&checks).unwrap_err().to_string();
        assert!(err.contains("trino, polaris, s3, source"), "{err}");
    }

    #[test]
    fn checking_nothing_is_not_healthy() {
        let checks = vec![Check::not_configured("trino", "trino-url")];
        assert!(verdict(&checks).is_err());
    }
}
