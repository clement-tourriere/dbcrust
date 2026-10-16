//! The HTTP layer: the reqwest client, authentication, and reading a response
//! body incrementally with the process-wide timeout and interrupt flag
//! honoured and a row/byte budget enforced while reading (the connection is
//! dropped as soon as the budget is spent, which makes the server stop).

use dbcrust_core::database::{
    ConnectionInfo, DatabaseError, influxdb_skips_certificate_verification, influxdb_version,
    interrupt_flag, query_timeout,
};
use dbcrust_core::password_sanitizer::redact_text;
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often a running request re-checks the interrupt flag and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Connection and handshake budget (the query itself is bounded by the
/// process-wide query timeout instead).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest error body kept for the message.
const MAX_ERROR_BODY: usize = 64 * 1024;

pub(crate) const DEFAULT_PORT: u16 = 8086;
pub(crate) const DEFAULT_PORT_V3: u16 = 8181;
pub(crate) const DEFAULT_HOST: &str = "localhost";

/// Rows rendered from one reply unless the `max_rows` option says otherwise.
pub(crate) const DEFAULT_MAX_ROWS: usize = 10_000;
/// Bytes read from one reply unless the `max_bytes` option says otherwise.
pub(crate) const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Connection option: rows rendered from one reply (0 = unlimited).
pub(crate) const OPT_MAX_ROWS: &str = "max_rows";
/// Connection option: bytes read from one reply (0 = unlimited).
pub(crate) const OPT_MAX_BYTES: &str = "max_bytes";

/// How requests are authenticated. The secret never shows in `Debug`.
#[derive(Clone)]
pub(crate) enum Auth {
    None,
    /// v1 user and password (`Authorization: Basic`).
    Basic {
        user: String,
        password: String,
    },
    /// v1/v2 API token (`Authorization: Token …`).
    Token(String),
    /// v3 API token (`Authorization: Bearer …`).
    Bearer(String),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("Auth::None"),
            Self::Basic { user, .. } => write!(f, "Auth::Basic({user}, ***)"),
            Self::Token(_) => f.write_str("Auth::Token(***)"),
            Self::Bearer(_) => f.write_str("Auth::Bearer(***)"),
        }
    }
}

impl Auth {
    /// The authentication a connection asks for: the version decides what the
    /// one secret slot (`password`) means.
    pub(crate) fn from_info(info: &ConnectionInfo) -> Self {
        let user = info.username.clone().filter(|user| !user.is_empty());
        let secret = info.password.clone().filter(|secret| !secret.is_empty());
        match (influxdb_version(&info.options), user, secret) {
            (_, None, None) => Self::None,
            (3, _, Some(token)) => Self::Bearer(token),
            (2, _, Some(token)) => Self::Token(token),
            // v1 with a user: login. With only a secret: an API token, which
            // a v2 server's 1.x-compatible API accepts
            (_, Some(user), secret) => Self::Basic {
                user,
                password: secret.unwrap_or_default(),
            },
            (_, None, Some(token)) => Self::Token(token),
        }
    }

    fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        match self {
            Self::None => request,
            Self::Basic { user, password } => request.basic_auth(user, Some(password)),
            Self::Token(token) => request.header("Authorization", format!("Token {token}")),
            Self::Bearer(token) => request.header("Authorization", format!("Bearer {token}")),
        }
    }
}

/// An HTTP endpoint with its credentials.
#[derive(Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
    base: String,
    auth: Auth,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("base", &self.base)
            .field("auth", &self.auth)
            .finish()
    }
}

/// `scheme://host:port` of a connection. Never includes credentials.
pub(crate) fn base_url(info: &ConnectionInfo) -> String {
    let host = info.host.as_deref().unwrap_or(DEFAULT_HOST);
    let port = info
        .port
        .unwrap_or(if influxdb_version(&info.options) == 3 {
            DEFAULT_PORT_V3
        } else {
            DEFAULT_PORT
        });
    let scheme = if info.use_tls { "https" } else { "http" };
    if host.contains(':') && !host.starts_with('[') {
        format!("{scheme}://[{host}]:{port}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// Parsed `max_rows` / `max_bytes` options (0 means unlimited).
pub(crate) fn limit_option(info: &ConnectionInfo, key: &str, default: usize) -> usize {
    match info
        .options
        .get(key)
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        Some(0) => usize::MAX,
        Some(limit) => limit,
        None => default,
    }
}

impl Http {
    pub(crate) fn new(info: &ConnectionInfo) -> Result<Self, DatabaseError> {
        if info.use_tls {
            // The TLS handshake takes the process-default rustls provider
            dbcrust_core::tls::install_default_crypto_provider();
        }
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("dbcrust/", env!("CARGO_PKG_VERSION")));
        if influxdb_skips_certificate_verification(&info.options) {
            tracing::warn!(
                "InfluxDB certificate verification is disabled (verify_certs=false): the server's identity is not checked"
            );
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(path) = info
            .options
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("ca_cert"))
            .map(|(_, value)| value)
        {
            let pem = std::fs::read(path).map_err(|error| {
                DatabaseError::ConnectionError(format!(
                    "Could not read the CA certificate {path}: {error}"
                ))
            })?;
            let certificate = reqwest::Certificate::from_pem(&pem).map_err(|error| {
                DatabaseError::ConnectionError(format!(
                    "{path} is not a PEM certificate: {}",
                    error.without_url()
                ))
            })?;
            builder = builder.add_root_certificate(certificate);
        }
        let client = builder.build().map_err(|error| {
            DatabaseError::ConnectionError(format!(
                "Could not build the HTTP client: {}",
                error.without_url()
            ))
        })?;
        Ok(Self {
            client,
            base: base_url(info),
            auth: Auth::from_info(info),
        })
    }

    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    pub(crate) fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.auth
            .apply(self.client.request(method, format!("{}{path}", self.base)))
    }

    /// Send `request`, giving up on Ctrl-C or when the query timeout passes.
    /// `started` is the start of the whole statement.
    pub(crate) async fn send(
        &self,
        request: RequestBuilder,
        started: Instant,
    ) -> Result<Response, DatabaseError> {
        let base = self.base.clone();
        guarded(request.send(), started)
            .await?
            .map_err(|error| transport_error(&base, error))
    }

    /// A plain GET used by connection probes: a short fixed timeout, no
    /// interrupt handling.
    pub(crate) async fn probe(&self, path: &str) -> Result<Response, DatabaseError> {
        let request = self.request(Method::GET, path).timeout(CONNECT_TIMEOUT);
        request
            .send()
            .await
            .map_err(|error| transport_error(&self.base, error))
    }
}

/// A transport failure as a connection error naming the server (never the
/// request, which holds the query text).
pub(crate) fn transport_error(base: &str, error: reqwest::Error) -> DatabaseError {
    let error = error.without_url();
    let reason = if error.is_timeout() {
        "timed out".to_string()
    } else if error.is_connect() {
        format!("could not connect ({})", describe_cause(&error))
    } else {
        describe_cause(&error)
    };
    DatabaseError::ConnectionError(format!("InfluxDB at {base}: {reason}"))
}

/// The innermost error message of a reqwest error, secrets redacted.
fn describe_cause(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source: Option<&dyn std::error::Error> = std::error::Error::source(error);
    while let Some(cause) = source {
        message = cause.to_string();
        source = cause.source();
    }
    redact_text(&message)
}

/// Await `future`, failing on Ctrl-C or when the statement exceeds the
/// configured query timeout.
pub(crate) async fn guarded<F: Future>(
    future: F,
    started: Instant,
) -> Result<F::Output, DatabaseError> {
    tokio::pin!(future);
    let interrupt = interrupt_flag();
    loop {
        tokio::select! {
            output = &mut future => return Ok(output),
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if interrupt.load(Ordering::Relaxed) {
                    return Err(DatabaseError::QueryError("query interrupted".to_string()));
                }
                if let Some(limit) = query_timeout()
                    && started.elapsed() >= limit
                {
                    return Err(DatabaseError::QueryError(format!(
                        "query timed out after {}s (the server may still be running it)",
                        limit.as_secs()
                    )));
                }
            }
        }
    }
}

/// Whether to keep feeding the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    Stop,
}

/// Consumer of a response body, fed in arrival order.
pub(crate) trait Sink: Send {
    fn feed(&mut self, bytes: &[u8]) -> Result<Flow, DatabaseError>;
    /// The body ended on its own (not cut by [`Flow::Stop`]).
    fn finish(&mut self) -> Result<(), DatabaseError>;
}

/// What stopped a [`stream_body`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamEnd {
    /// The sink asked to stop (its row budget is spent).
    pub(crate) sink_stopped: bool,
    /// The byte budget ran out before the body ended.
    pub(crate) byte_limit_hit: bool,
}

/// Feed the body of a successful `response` to `sink`, stopping when the sink
/// says so or `max_bytes` have been read. The response is dropped on return,
/// which closes the connection and makes the server stop producing.
pub(crate) async fn stream_body(
    mut response: Response,
    sink: &mut dyn Sink,
    max_bytes: usize,
    started: Instant,
) -> Result<StreamEnd, DatabaseError> {
    let base = response.url().origin().ascii_serialization();
    let mut total = 0usize;
    loop {
        let chunk = guarded(response.chunk(), started)
            .await?
            .map_err(|error| transport_error(&base, error))?;
        let Some(chunk) = chunk else {
            sink.finish()?;
            return Ok(StreamEnd {
                sink_stopped: false,
                byte_limit_hit: false,
            });
        };
        let allowed = max_bytes.saturating_sub(total).min(chunk.len());
        total += allowed;
        if sink.feed(&chunk[..allowed])? == Flow::Stop {
            return Ok(StreamEnd {
                sink_stopped: true,
                byte_limit_hit: false,
            });
        }
        if allowed < chunk.len() {
            return Ok(StreamEnd {
                sink_stopped: false,
                byte_limit_hit: true,
            });
        }
    }
}

/// The body of a failed response as a short, redacted message.
pub(crate) async fn error_message(mut response: Response, started: Instant) -> String {
    let mut body: Vec<u8> = Vec::new();
    while body.len() < MAX_ERROR_BODY {
        match guarded(response.chunk(), started).await {
            Ok(Ok(Some(chunk))) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    message_from_body(&body)
}

/// `message` / `error` / `msg` of a JSON error body, else the text itself.
pub(crate) fn message_from_body(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let message = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            ["message", "error", "msg"]
                .iter()
                .find_map(|key| value.get(key).and_then(|v| v.as_str()).map(str::to_string))
        })
        .unwrap_or_else(|| text.trim().to_string());
    let mut message = redact_text(message.trim());
    if message.len() > 600 {
        let mut cut = 600;
        while !message.is_char_boundary(cut) {
            cut -= 1;
        }
        message.truncate(cut);
        message.push('…');
    }
    message
}

/// A failed HTTP status as an error: authentication and permission problems
/// say so; `connecting` makes them connection errors.
pub(crate) fn status_error(status: StatusCode, message: &str, connecting: bool) -> DatabaseError {
    let detail = if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    };
    match status {
        StatusCode::UNAUTHORIZED => DatabaseError::ConnectionError(format!(
            "InfluxDB authentication failed (HTTP 401){detail}"
        )),
        StatusCode::FORBIDDEN if connecting => {
            DatabaseError::ConnectionError(format!("InfluxDB permission denied (HTTP 403){detail}"))
        }
        StatusCode::FORBIDDEN => {
            DatabaseError::QueryError(format!("InfluxDB permission denied (HTTP 403){detail}"))
        }
        _ if connecting => DatabaseError::ConnectionError(format!(
            "InfluxDB answered HTTP {}{detail}",
            status.as_u16()
        )),
        _ => {
            DatabaseError::QueryError(format!("InfluxDB error (HTTP {}){detail}", status.as_u16()))
        }
    }
}
