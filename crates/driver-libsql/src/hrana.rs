//! Hrana over HTTP, JSON encoding: `POST {base}/v2/pipeline`.
//!
//! The protocol is specified in the libSQL repository (`docs/HRANA_2_SPEC.md`,
//! `docs/HTTP_V2_SPEC.md`); this module is the small part of it DBCrust needs:
//!
//! - a **pipeline** is `{"baton": …, "requests": […]}`, answered by
//!   `{"baton": …, "base_url": …, "results": […]}` with one result per request;
//! - requests used here: `execute` (one statement), `describe` (the server's
//!   own `sqlite3_stmt_readonly`), `close`;
//! - a statement without a `close` in its pipeline keeps its **stream** open on
//!   the server and answers with a `baton` that names it: that is how a
//!   `BEGIN` … `COMMIT` spans several statements. Everything else is sent as
//!   `[execute, close]` with no baton, so no state is left behind;
//! - values are tagged: `null`, `integer` (a decimal *string*), `float`,
//!   `text`, `blob` (base64).
//!
//! The auth token travels only in the `Authorization` header and is scrubbed
//! from every message this module builds.

use std::time::Duration;

use dbcrust_core::database::{
    ConnectionInfo, DatabaseError, LIBSQL_PLAIN_PORT, LIBSQL_TLS_PORT, interrupt_flag,
};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// The pipeline endpoint (Hrana 2 over HTTP; Turso Cloud and `sqld` both serve it).
pub(crate) const PIPELINE_PATH: &str = "/v2/pipeline";
/// Header `sqld` reads the tenant from when it runs with namespaces.
const NAMESPACE_HEADER: &str = "x-namespace";
/// Seconds to establish the TCP/TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A request without a configured timeout still ends: this long.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// How much of an error body is shown.
const ERROR_BODY_LIMIT: usize = 300;

// ----- the endpoint ------------------------------------------------------------

/// Where and as whom to talk.
#[derive(Clone)]
pub(crate) struct Endpoint {
    /// `https://host[:port]` (no trailing slash).
    pub(crate) base_url: String,
    pub(crate) token: Option<String>,
    pub(crate) namespace: Option<String>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("namespace", &self.namespace)
            .finish()
    }
}

impl Endpoint {
    pub(crate) fn from_info(info: &ConnectionInfo) -> Result<Self, DatabaseError> {
        let host = info.host.as_deref().filter(|host| !host.is_empty());
        let host = host.ok_or_else(|| {
            DatabaseError::ConnectionError("a libSQL connection needs a host".to_string())
        })?;
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let (scheme, default_port) = if info.use_tls {
            ("https", LIBSQL_TLS_PORT)
        } else {
            ("http", LIBSQL_PLAIN_PORT)
        };
        Ok(Self {
            base_url: format!("{scheme}://{host}:{}", info.port.unwrap_or(default_port)),
            token: info.password.clone().filter(|token| !token.is_empty()),
            namespace: info.database.clone().filter(|name| !name.is_empty()),
        })
    }

    pub(crate) fn is_tls(&self) -> bool {
        self.base_url.starts_with("https://")
    }

    /// `text` with the token (and its `Bearer` form) replaced: the last line of
    /// defence for anything a server or a transport error echoes back.
    pub(crate) fn scrub(&self, text: &str) -> String {
        match &self.token {
            Some(token) => text.replace(token.as_str(), "[REDACTED]"),
            None => text.to_string(),
        }
    }
}

// ----- wire types ---------------------------------------------------------------

#[derive(Serialize)]
struct PipelineRequest<'a> {
    baton: Option<&'a str>,
    requests: &'a [StreamRequest],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamRequest {
    Execute { stmt: Stmt },
    Describe { sql: String },
    Close,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Stmt {
    pub(crate) sql: String,
    pub(crate) want_rows: bool,
}

impl StreamRequest {
    pub(crate) fn execute(sql: &str) -> Self {
        Self::Execute {
            stmt: Stmt {
                sql: sql.to_string(),
                want_rows: true,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PipelineResponse {
    #[serde(default)]
    pub(crate) baton: Option<String>,
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    pub(crate) results: Vec<StreamResult>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamResult {
    Ok { response: StreamResponse },
    Error { error: HranaError },
}

#[derive(Debug, Deserialize)]
pub(crate) struct HranaError {
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamResponse {
    Execute {
        result: StmtResult,
    },
    Describe {
        result: DescribeResult,
    },
    Close,
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StmtResult {
    #[serde(default)]
    pub(crate) cols: Vec<Col>,
    #[serde(default)]
    pub(crate) rows: Vec<Vec<Json>>,
    #[serde(default)]
    pub(crate) affected_row_count: u64,
    #[serde(default)]
    pub(crate) rows_read: Option<u64>,
    #[serde(default)]
    pub(crate) rows_written: Option<u64>,
    #[serde(default)]
    pub(crate) query_duration_ms: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Col {
    #[serde(default)]
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DescribeResult {
    #[serde(default)]
    pub(crate) is_readonly: Option<bool>,
}

// ----- values -------------------------------------------------------------------

/// A Hrana value as the cell text the other drivers produce: `None` is SQL NULL,
/// a blob is `\x` + hex (like the SQLite driver), a float always shows its
/// fractional part.
pub(crate) fn cell(value: &Json) -> Result<Option<String>, String> {
    let kind = value
        .get("type")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("a value without a type: {value}"))?;
    let payload = value.get("value");
    match kind {
        "null" => Ok(None),
        "integer" => match payload {
            Some(Json::String(text)) => Ok(Some(text.clone())),
            Some(Json::Number(number)) => Ok(Some(number.to_string())),
            _ => Err("an integer value without its digits".to_string()),
        },
        "float" => match payload {
            Some(Json::Number(number)) => Ok(Some(format_float(number.as_f64().unwrap_or(0.0)))),
            Some(Json::String(text)) => Ok(Some(text.clone())),
            _ => Err("a float value without its number".to_string()),
        },
        "text" => match payload {
            Some(Json::String(text)) => Ok(Some(text.clone())),
            _ => Err("a text value without its string".to_string()),
        },
        "blob" => {
            let encoded = value
                .get("base64")
                .and_then(Json::as_str)
                .ok_or_else(|| "a blob value without its base64".to_string())?;
            let bytes = decode_base64(encoded)?;
            let mut hex = String::with_capacity(2 + bytes.len() * 2);
            hex.push_str("\\x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02x}"));
            }
            Ok(Some(hex))
        }
        other => Err(format!("an unknown value type '{other}'")),
    }
}

fn format_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// Base64 with or without padding (the Hrana spec leaves it to the sender).
pub(crate) fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    let engine = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    engine
        .decode(text.trim())
        .map_err(|error| format!("a blob that is not base64: {error}"))
}

// ----- the client ---------------------------------------------------------------

/// What one pipeline round trip returned.
pub(crate) struct Outcome {
    pub(crate) baton: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) results: Vec<StreamResult>,
}

pub(crate) struct HranaClient {
    http: reqwest::Client,
    pub(crate) endpoint: Endpoint,
    /// Ceiling for one request (the broker's statement timeout, when set).
    ceiling: Option<Duration>,
}

impl HranaClient {
    pub(crate) fn new(
        endpoint: Endpoint,
        ceiling: Option<Duration>,
    ) -> Result<Self, DatabaseError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("dbcrust/", env!("CARGO_PKG_VERSION")))
            // A redirect would carry the token to wherever it points; reqwest
            // drops `Authorization` across origins, but a libSQL endpoint has no
            // business redirecting a POST: fail loudly instead
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| DatabaseError::ConnectionError(error.to_string()))?;
        Ok(Self {
            http,
            endpoint,
            ceiling,
        })
    }

    fn timeout(&self) -> Duration {
        let configured = dbcrust_core::database::query_timeout();
        match (configured, self.ceiling) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// One pipeline round trip. `base_url` overrides the endpoint's for a stream
    /// the server pinned to another node; the token follows it only over the
    /// same scheme.
    pub(crate) async fn pipeline(
        &self,
        base_url: Option<&str>,
        baton: Option<&str>,
        requests: &[StreamRequest],
    ) -> Result<Outcome, DatabaseError> {
        let base = base_url.unwrap_or(&self.endpoint.base_url);
        let url = format!("{}{PIPELINE_PATH}", base.trim_end_matches('/'));
        let timeout = self.timeout();
        let mut request = self
            .http
            .post(&url)
            .timeout(timeout)
            .json(&PipelineRequest { baton, requests });
        if let Some(token) = &self.endpoint.token {
            request = request.bearer_auth(token);
        }
        if let Some(namespace) = &self.endpoint.namespace {
            request = request.header(NAMESPACE_HEADER, namespace);
        }

        // Ctrl-C drops the request: Hrana has no cancel, the server notices
        // the closed connection
        let interrupt = interrupt_flag();
        let send = request.send();
        tokio::pin!(send);
        let response = loop {
            tokio::select! {
                response = &mut send => break response,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if interrupt.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(DatabaseError::QueryError("Query cancelled".to_string()));
                    }
                }
            }
        };
        let response = response.map_err(|error| self.transport_error(error, timeout))?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let body = response.text().await.unwrap_or_default();
            return Err(self.http_error(status, retry_after.as_deref(), &body));
        }
        let body = response
            .bytes()
            .await
            .map_err(|error| self.transport_error(error, timeout))?;
        let parsed: PipelineResponse = serde_json::from_slice(&body).map_err(|error| {
            DatabaseError::QueryError(format!(
                "the server answered {url_path} with something that is not a Hrana response ({error}): is {host} a libSQL / sqld / Turso endpoint?",
                url_path = PIPELINE_PATH,
                host = self.host_label(),
            ))
        })?;

        // The server may pin the stream to another node; only follow it on the
        // same scheme, never from https down to http
        let base_url = parsed.base_url.filter(|next| {
            let same_scheme = next.starts_with("https://") == self.endpoint.is_tls();
            if !same_scheme {
                tracing::debug!("[libsql] ignoring a base_url that changes the scheme");
            }
            same_scheme && !next.is_empty()
        });
        Ok(Outcome {
            baton: parsed.baton,
            base_url,
            results: parsed.results,
        })
    }

    fn host_label(&self) -> String {
        self.endpoint
            .base_url
            .split_once("://")
            .map_or(self.endpoint.base_url.as_str(), |(_, rest)| rest)
            .to_string()
    }

    fn transport_error(&self, error: reqwest::Error, timeout: Duration) -> DatabaseError {
        let host = self.host_label();
        let message = if error.is_timeout() {
            return DatabaseError::QueryError(format!(
                "Query timed out after {} seconds (query_timeout_seconds)",
                timeout.as_secs()
            ));
        } else if error.is_connect() {
            format!("could not connect to {host}: {}", root_cause(&error))
        } else if error.is_redirect() {
            format!(
                "{host} redirected the request: a libSQL endpoint should answer {PIPELINE_PATH} directly"
            )
        } else {
            format!("request to {host} failed: {}", root_cause(&error))
        };
        let message = self.endpoint.scrub(&message);
        if error.is_connect() {
            DatabaseError::ConnectionError(message)
        } else {
            DatabaseError::QueryError(message)
        }
    }

    fn http_error(
        &self,
        status: reqwest::StatusCode,
        retry_after: Option<&str>,
        body: &str,
    ) -> DatabaseError {
        let detail = server_message(body);
        let detail = self.endpoint.scrub(&detail);
        let host = self.host_label();
        match status.as_u16() {
            401 | 403 => DatabaseError::ConnectionError(
                format!(
                    "authentication failed: {host} rejected the auth token (HTTP {})",
                    status.as_u16()
                ) + &if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                },
            ),
            404 => DatabaseError::ConnectionError(format!(
                "{host} does not serve {PIPELINE_PATH} (HTTP 404){}: is it a libSQL / sqld / Turso endpoint{}?",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                },
                if self.endpoint.namespace.is_some() {
                    ", and does the namespace exist"
                } else {
                    ""
                }
            )),
            429 => DatabaseError::QueryError(format!(
                "{host} is rate limiting this client (HTTP 429){}",
                retry_after
                    .map(|after| format!(", retry after {after}"))
                    .unwrap_or_default()
            )),
            code @ 500..=599 => DatabaseError::QueryError(format!(
                "{host} failed (HTTP {code}){}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )),
            code => DatabaseError::QueryError(format!(
                "{host} answered HTTP {code}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            )),
        }
    }
}

/// The innermost cause of a transport error (`reqwest` wraps hyper wraps io).
fn root_cause(error: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = error;
    while let Some(next) = source.source() {
        source = next;
    }
    source.to_string()
}

/// The message of an error body: `{"error": "…"}` / `{"message": "…"}` or the
/// text itself, cut short.
pub(crate) fn server_message(body: &str) -> String {
    let text = serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|json| {
            ["error", "message"]
                .iter()
                .find_map(|key| match json.get(key) {
                    Some(Json::String(text)) => Some(text.clone()),
                    Some(other) if !other.is_null() => Some(other.to_string()),
                    _ => None,
                })
        })
        .unwrap_or_else(|| body.trim().to_string());
    if text.chars().count() > ERROR_BODY_LIMIT {
        let cut: String = text.chars().take(ERROR_BODY_LIMIT).collect();
        format!("{cut}…")
    } else {
        text
    }
}
