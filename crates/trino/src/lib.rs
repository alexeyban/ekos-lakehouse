//! A Trino client over its REST protocol, and the read-only [`TrinoReader`] the RFC 0156 tiers
//! run through (RFC 0190).
//!
//! The protocol: `POST /v1/statement` with the SQL, then follow `nextUri` until there is none,
//! collecting `columns` and `data` on the way; an `error` in any response ends the query. A 502,
//! 503 or 504 means "try the same URI again" (Trino's client protocol).
//!
//! Blocking, because [`EngineReader`] is synchronous. A caller inside a tokio runtime runs it on
//! its own thread, as EKOS Migrate does for PostgreSQL.

use ekos_migrate_target_clickhouse::classify::{StatementClass, classify_with};
use ekos_migrate_validate::{Dialect, EngineReader, ReadError};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum TrinoError {
    #[error("Trino at {url}: {message}")]
    Http { url: String, message: String },
    #[error("Trino query failed: {name}: {message}")]
    Query { name: String, message: String },
    #[error("refused before sending: {0}")]
    Refused(String),
}

/// Where and as whom queries run.
#[derive(Debug, Clone)]
pub struct Connection {
    /// e.g. `http://127.0.0.1:58080`
    pub base_url: String,
    /// `X-Trino-User`. Not a credential: the sandbox runs without authentication.
    pub user: String,
    pub catalog: Option<String>,
    pub schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    next_uri: Option<String>,
    #[serde(default)]
    columns: Option<Vec<Column>>,
    #[serde(default)]
    data: Option<Vec<Vec<Value>>>,
    error: Option<PageError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageError {
    message: String,
    #[serde(default)]
    error_name: String,
}

pub struct Client {
    conn: Connection,
    http: reqwest::blocking::Client,
}

const RETRIES: u32 = 20;

impl Client {
    pub fn new(conn: Connection) -> Result<Self, TrinoError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| TrinoError::Http {
                url: conn.base_url.clone(),
                message: e.to_string(),
            })?;
        Ok(Self { conn, http })
    }

    /// Run one statement to completion.
    pub fn execute(&self, sql: &str) -> Result<QueryResult, TrinoError> {
        let url = format!("{}/v1/statement", self.conn.base_url.trim_end_matches('/'));
        let mut req = self
            .http
            .post(&url)
            .header("X-Trino-User", &self.conn.user)
            .body(sql.to_string());
        if let Some(c) = &self.conn.catalog {
            req = req.header("X-Trino-Catalog", c);
        }
        if let Some(s) = &self.conn.schema {
            req = req.header("X-Trino-Schema", s);
        }
        let mut page = self.send(req, &url)?;
        let mut out = QueryResult::default();
        loop {
            if let Some(e) = page.error {
                return Err(TrinoError::Query {
                    name: e.error_name,
                    message: e.message,
                });
            }
            if let Some(cols) = page.columns.take()
                && out.columns.is_empty()
            {
                out.columns = cols;
            }
            if let Some(data) = page.data.take() {
                out.rows.extend(data);
            }
            let Some(next) = page.next_uri.take() else {
                return Ok(out);
            };
            page = self.send(self.http.get(&next), &next)?;
        }
    }

    fn send(&self, req: reqwest::blocking::RequestBuilder, url: &str) -> Result<Page, TrinoError> {
        let http = |message: String| TrinoError::Http {
            url: url.to_string(),
            message,
        };
        let mut attempt = 0;
        loop {
            let resp = req
                .try_clone()
                .expect("a request with a string body can be cloned")
                .send()
                .map_err(|e| http(e.to_string()))?;
            let status = resp.status().as_u16();
            if matches!(status, 502..=504) && attempt < RETRIES {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50 * u64::from(attempt)));
                continue;
            }
            if !resp.status().is_success() {
                let body = resp.text().unwrap_or_default();
                return Err(http(format!("HTTP {status}: {}", body.trim())));
            }
            return resp
                .json()
                .map_err(|e| http(format!("not a Trino page: {e}")));
        }
    }
}

/// The RFC 0156 read path into Trino. Read-only by construction: every statement is classified
/// with Trino's grammar first, and anything that is not a plain read is refused before it is
/// sent — the validator must never be able to change what it validates.
pub struct TrinoReader {
    client: Client,
    label: String,
}

impl TrinoReader {
    pub fn new(conn: Connection) -> Result<Self, TrinoError> {
        let label = format!("trino:{}", conn.base_url);
        Ok(Self {
            client: Client::new(conn)?,
            label,
        })
    }

    /// Rows as strings, the way the other readers return them: SQL NULL as an empty cell (as the
    /// PostgreSQL reader's simple protocol does), strings as they are, numbers and booleans in
    /// their JSON text. Trino sends decimals as JSON strings, so their scale is kept.
    pub fn read(&self, sql: &str) -> Result<Vec<Vec<String>>, TrinoError> {
        check_read_only(sql)?;
        let result = self.client.execute(sql)?;
        Ok(result
            .rows
            .into_iter()
            .map(|r| r.into_iter().map(cell).collect())
            .collect())
    }
}

/// Refuses anything but a single plain read, judged by the RFC 0160 classifier with Trino's
/// grammar. Unparseable text is refused too.
pub fn check_read_only(sql: &str) -> Result<(), TrinoError> {
    let classified = classify_with(sql, &sqlparser::dialect::GenericDialect {})
        .map_err(|e| TrinoError::Refused(e.to_string()))?;
    match classified.as_slice() {
        [one] if one.class == StatementClass::Read => Ok(()),
        [one] => Err(TrinoError::Refused(format!(
            "a {:?} statement on the read path",
            one.class
        ))),
        many => Err(TrinoError::Refused(format!(
            "{} statements; the read path takes one",
            many.len()
        ))),
    }
}

fn cell(v: Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s,
        other => other.to_string(),
    }
}

impl EngineReader for TrinoReader {
    fn dialect(&self) -> Dialect {
        Dialect::Trino
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, ReadError> {
        self.read(sql).map_err(|e| ReadError::Query {
            engine: self.label.clone(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A fake coordinator: answers the n-th request with the n-th body (status, JSON), and
    /// records each request line and its `X-Trino-User` header.
    fn coordinator(pages: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let base2 = base.clone();
        std::thread::spawn(move || {
            let mut pages = pages.into_iter();
            for stream in listener.incoming().flatten() {
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut len = 0usize;
                let mut user = String::new();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if lower.starts_with("x-trino-user:") {
                        user = h["x-trino-user:".len()..].trim().to_string();
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(format!(
                    "{} user={user} body={}",
                    first.trim(),
                    String::from_utf8_lossy(&body)
                ));
                let (status, json) = pages.next().unwrap_or((500, "{}".into()));
                let json = json.replace("{BASE}", &base2);
                let mut s = stream;
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        (base, seen)
    }

    fn conn(base: &str) -> Connection {
        Connection {
            base_url: base.to_string(),
            user: "ekos".into(),
            catalog: Some("iceberg".into()),
            schema: None,
        }
    }

    #[test]
    fn follows_next_uri_collects_pages_and_retries_a_503() {
        let (base, seen) = coordinator(vec![
            (200, r#"{"id":"q1","nextUri":"{BASE}/v1/statement/q1/1"}"#.into()),
            (503, "".into()),
            (
                200,
                r#"{"id":"q1","nextUri":"{BASE}/v1/statement/q1/2","columns":[{"name":"a","type":"bigint"},{"name":"b","type":"decimal(12,2)"},{"name":"c","type":"varchar"}],"data":[[1,"1.50",null]]}"#.into(),
            ),
            (200, r#"{"id":"q1","data":[[2,"0.05","x"]]}"#.into()),
        ]);
        let r = TrinoReader::new(conn(&base)).unwrap();
        let rows = r.query("SELECT a, b, c FROM t").unwrap();
        assert_eq!(
            rows,
            vec![
                vec!["1".to_string(), "1.50".into(), "".into()],
                vec!["2".into(), "0.05".into(), "x".into()],
            ]
        );
        let log = seen.lock().unwrap();
        assert!(log[0].starts_with("POST /v1/statement"), "{log:?}");
        assert!(log[0].contains("user=ekos"), "{log:?}");
        assert!(log[0].contains("body=SELECT a, b, c FROM t"), "{log:?}");
        assert_eq!(log.len(), 4, "the 503 was retried: {log:?}");
        assert_eq!(r.dialect(), Dialect::Trino);
    }

    #[test]
    fn a_query_error_is_reported_with_its_name() {
        let (base, _) = coordinator(vec![(
            200,
            r#"{"id":"q2","error":{"message":"line 1:8: Column 'x' cannot be resolved","errorName":"COLUMN_NOT_FOUND"}}"#
                .into(),
        )]);
        let err = TrinoReader::new(conn(&base))
            .unwrap()
            .query("SELECT x FROM t")
            .unwrap_err()
            .to_string();
        assert!(err.contains("COLUMN_NOT_FOUND"), "{err}");
    }

    #[test]
    fn the_read_path_refuses_writes_before_sending_them() {
        // Nothing listens here: a refused statement must never reach the network.
        let r = TrinoReader::new(conn("http://127.0.0.1:1")).unwrap();
        for sql in [
            "INSERT INTO iceberg.bronze.t SELECT * FROM postgresql.public.t",
            "DELETE FROM iceberg.bronze.t",
            "DROP TABLE iceberg.bronze.t",
            "ALTER TABLE iceberg.bronze.t EXECUTE optimize",
            "SELECT 1; SELECT 2",
        ] {
            let err = r.read(sql).unwrap_err();
            assert!(matches!(err, TrinoError::Refused(_)), "{sql}: {err}");
        }
        assert!(check_read_only("SELECT count(*) FROM iceberg.bronze.t").is_ok());
    }
}
