//! `ekos-lakehouse lakehouse doctor`: does every service of the stack answer?
//!
//! Reachability and readiness only. It sends no credentials and reads no data: the Trino check
//! is `/v1/info`, MinIO and Polaris their health endpoints, Dagster `/server_info`, and the
//! source a TCP connect.

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
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub service: &'static str,
    pub target: Option<String>,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn not_configured(service: &'static str, key: &str) -> Self {
        Self {
            service,
            target: None,
            status: Status::NotConfigured,
            detail: format!("set `{key}` in [extensions.lakehouse]"),
        }
    }

    fn of(service: &'static str, target: String, outcome: Result<String, String>) -> Self {
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
    out.push(match &s.minio_url {
        None => Check::not_configured("minio", "minio-url"),
        Some(base) => {
            let url = format!("{}/minio/health/live", base.trim_end_matches('/'));
            Check::of("minio", url.clone(), status_ok(&client, &url).await)
        }
    });
    out.push(match &s.dagster_url {
        None => Check::not_configured("dagster", "dagster-url"),
        Some(base) => {
            let url = format!("{}/server_info", base.trim_end_matches('/'));
            Check::of("dagster", url.clone(), status_ok(&client, &url).await)
        }
    });
    out.push(match &s.source {
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

async fn tcp(addr: &str) -> Result<String, String> {
    match tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(_)) => Ok("accepts connections".into()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("no answer within {}s", TIMEOUT.as_secs())),
    }
}

pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        let mark = match c.status {
            Status::Ok => "ok  ",
            Status::Failed => "FAIL",
            Status::NotConfigured => "--  ",
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
    if checks.iter().all(|c| c.status == Status::NotConfigured) {
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
        let minio = serve("200 OK", "{}");
        let source = TcpListener::bind("127.0.0.1:0").unwrap();
        let s = Settings {
            trino_url: Some(trino),
            polaris_health_url: Some(format!("{polaris}/q/health")),
            minio_url: Some(minio),
            dagster_url: None,
            source: Some(source.local_addr().unwrap().to_string()),
        };
        let checks = run(&s).await;
        let by = |n: &str| checks.iter().find(|c| c.service == n).unwrap().clone();
        assert_eq!(by("trino").status, Status::Ok, "{checks:?}");
        assert_eq!(by("trino").detail, "version 476");
        assert_eq!(by("polaris").status, Status::Ok);
        assert_eq!(by("minio").status, Status::Ok);
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
            minio_url: None,
            dagster_url: None,
            source: Some(closed()),
        };
        let checks = run(&s).await;
        for svc in ["trino", "polaris", "source"] {
            let c = checks.iter().find(|c| c.service == svc).unwrap();
            assert_eq!(c.status, Status::Failed, "{svc}: {c:?}");
        }
        let err = verdict(&checks).unwrap_err().to_string();
        assert!(err.contains("trino, polaris, source"), "{err}");
    }

    #[test]
    fn checking_nothing_is_not_healthy() {
        let checks = vec![Check::not_configured("trino", "trino-url")];
        assert!(verdict(&checks).is_err());
    }
}
