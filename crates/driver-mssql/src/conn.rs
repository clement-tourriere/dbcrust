//! The connection layer: map a [`ConnectionInfo`] to a tiberius [`Config`],
//! open the (single, pinned) session, and run one batch with the process-wide
//! timeout and interrupt flag honoured.
//!
//! One TDS connection serves the whole session, so `SET` options, temp
//! tables and open transactions persist across statements like they do in
//! `sqlcmd`. A statement that outlives its deadline (or Ctrl-C) is cancelled
//! with a TDS *attention* signal; if the server does not acknowledge it in
//! time, or the transport failed, the connection is dropped and the next
//! statement reconnects.

use crate::value::format_cell;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseError, SqlServerEncryption, connection_timeout_secs, interrupt_flag,
    query_timeout, sqlserver_trusts_any_certificate,
};
use dbcrust_core::password_sanitizer::redact_text;
use futures_util::TryStreamExt;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tiberius::error::Error as TdsError;
use tiberius::{AuthMethod, Client, ColumnType, Config, EncryptionLevel, QueryItem, SqlBrowser};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use tracing::{debug, warn};

pub(crate) type Conn = Client<Compat<TcpStream>>;

pub(crate) const DEFAULT_PORT: u16 = 1433;
const DEFAULT_HOST: &str = "localhost";
const DEFAULT_APPLICATION_NAME: &str = "dbcrust";
/// Connection option: seconds allowed for TCP connect + TLS + login.
const OPT_CONNECT_TIMEOUT: &str = "connect_timeout";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Connection option: rows kept from one result set (0 = unlimited).
pub(crate) const OPT_MAX_ROWS: &str = "max_rows";
pub(crate) const DEFAULT_MAX_ROWS: usize = 100_000;
/// How often a running statement re-checks the interrupt flag and deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long the server gets to acknowledge a cancel before the connection is
/// abandoned.
const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// Option value (case-insensitive key lookup).
pub(crate) fn option<'a>(info: &'a ConnectionInfo, key: &str) -> Option<&'a str> {
    info.options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

/// The host without the brackets of an IPv6 literal.
fn bare_host(info: &ConnectionInfo) -> String {
    let host = info.host.as_deref().unwrap_or(DEFAULT_HOST);
    host.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host)
        .to_string()
}

/// `host:port` or `host\instance` for messages. Never includes credentials.
pub(crate) fn describe_target(info: &ConnectionInfo) -> String {
    let host = bare_host(info);
    match option(info, "instance") {
        Some(instance) => format!("{host}\\{instance}"),
        None => format!("{host}:{}", info.port.unwrap_or(DEFAULT_PORT)),
    }
}

/// The tiberius configuration for `info`.
///
/// Encryption defaults to the whole session over TLS with the certificate
/// verified; `encrypt=false` and `trustServerCertificate=true` are explicit
/// opt-ins (the latter is logged as a warning on every connection).
pub(crate) fn build_config(info: &ConnectionInfo) -> Result<Config, DatabaseError> {
    let mut config = Config::new();
    config.host(bare_host(info));
    match option(info, "instance") {
        // The port of a named instance is learned from SQL Browser (UDP 1434)
        Some(instance) => config.instance_name(instance),
        None => config.port(info.port.unwrap_or(DEFAULT_PORT)),
    }
    if let Some(database) = info.database.as_deref().filter(|db| !db.is_empty()) {
        config.database(database);
    }
    config.application_name(
        option(info, "application_name")
            .or_else(|| option(info, "applicationName"))
            .unwrap_or(DEFAULT_APPLICATION_NAME),
    );

    let user = info
        .username
        .as_deref()
        .filter(|user| !user.is_empty())
        .ok_or_else(|| {
            DatabaseError::ConnectionError(
                "SQL Server needs a login: mssql://user:password@host/database (Windows and Azure AD sign-in are not supported yet)"
                    .to_string(),
            )
        })?;
    config.authentication(AuthMethod::sql_server(
        user,
        info.password.as_deref().unwrap_or(""),
    ));

    let encryption =
        SqlServerEncryption::parse(option(info, "encrypt")).map_err(DatabaseError::InvalidUrl)?;
    config.encryption(match encryption {
        SqlServerEncryption::LoginOnly => EncryptionLevel::Off,
        SqlServerEncryption::Required => EncryptionLevel::Required,
        SqlServerEncryption::Strict => EncryptionLevel::Strict,
    });

    let ca_cert = option(info, "ca_cert");
    if sqlserver_trusts_any_certificate(&info.options) {
        if ca_cert.is_some() {
            return Err(DatabaseError::InvalidUrl(
                "trustServerCertificate=true and ca_cert cannot be combined: pin the CA or skip verification, not both"
                    .to_string(),
            ));
        }
        warn!(
            "SQL Server certificate verification is OFF for {} (trustServerCertificate=true): \
             anyone on the network path can impersonate the server",
            describe_target(info)
        );
        config.trust_cert();
    } else if let Some(path) = ca_cert {
        config.trust_cert_ca(path);
    }
    if let Some(name) = option(info, "hostNameInCertificate") {
        config.hostname_in_certificate(name);
    }
    if option(info, "intent").is_some_and(|v| v.eq_ignore_ascii_case("readonly")) {
        // ApplicationIntent=ReadOnly only routes to a readable secondary; it
        // is not enforcement (see the read-only guard)
        config.readonly(true);
    }
    let connect_timeout = connect_timeout(info);
    config.handshake_timeout(Some(connect_timeout));
    // The statement deadline is enforced in `run_batch` against the live
    // `query_timeout()`; tiberius' own per-response stall timer is off
    config.command_timeout(None);
    Ok(config)
}

fn connect_timeout(info: &ConnectionInfo) -> Duration {
    option(info, OPT_CONNECT_TIMEOUT)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs)
}

/// Open a session for `info`.
pub(crate) async fn open(info: &ConnectionInfo) -> Result<Conn, DatabaseError> {
    // The TLS handshake takes the process-default rustls provider (ring)
    dbcrust_core::tls::install_default_crypto_provider();
    let config = build_config(info)?;
    let target = describe_target(info);
    let timeout = connect_timeout(info);
    let connecting = async {
        let tcp = if option(info, "instance").is_some() {
            TcpStream::connect_named(&config).await?
        } else {
            TcpStream::connect(config.get_addr())
                .await
                .map_err(TdsError::from)?
        };
        tcp.set_nodelay(true).map_err(TdsError::from)?;
        Client::connect(config.clone(), tcp.compat_write()).await
    };
    match tokio::time::timeout(timeout, connecting).await {
        Ok(Ok(client)) => {
            debug!("[SQL Server] connected to {target}");
            Ok(client)
        }
        Ok(Err(error)) => Err(connection_error(&target, &error)),
        Err(_) => Err(DatabaseError::ConnectionError(format!(
            "Timed out after {}s connecting to SQL Server at {target}",
            timeout.as_secs()
        ))),
    }
}

/// A connect-time failure as a [`DatabaseError`], with a hint for the two
/// problems that fill the support queue: an untrusted certificate (the usual
/// self-signed development server) and an unreachable instance.
fn connection_error(target: &str, error: &TdsError) -> DatabaseError {
    let mut text = redact_text(&error.to_string());
    // tiberius suggests `Config::…` calls that mean nothing at a prompt
    if let Some(cut) = text.find(" To connect anyway") {
        text.truncate(cut);
    }
    let lower = text.to_ascii_lowercase();
    let hint = if lower.contains("unsupportedcertversion") {
        "\nSQL Server's auto-generated self-signed certificate is an old X.509 version that rustls \
         refuses even when its CA is pinned. Install a proper certificate on the server, or accept \
         this one with `?trustServerCertificate=true` on a network you trust."
    } else if lower.contains("certificate")
        || lower.contains("unknownissuer")
        || lower.contains("invalid peer")
    {
        "\nThe server certificate could not be verified. Pin its CA with `?ca_cert=/path/ca.pem` \
         (recommended), or accept any certificate with `?trustServerCertificate=true` on a network \
         you trust."
    } else if lower.contains("login failed") {
        "\nCheck the login and password; Windows and Azure AD sign-in are not supported yet."
    } else {
        ""
    };
    DatabaseError::ConnectionError(format!(
        "Could not connect to SQL Server at {target}: {text}{hint}"
    ))
}

#[cfg(test)]
pub(crate) fn connection_error_for_tests(target: &str, error: &TdsError) -> DatabaseError {
    connection_error(target, error)
}

/// A server or transport error as the right [`DatabaseError`]: a SQL error
/// (syntax, permissions, constraints…) is a query error and the session stays
/// usable; anything else means the connection is gone.
pub(crate) fn query_error(error: &TdsError) -> DatabaseError {
    let text = redact_text(&format!("{error}"));
    match error {
        TdsError::Server(token) => DatabaseError::QueryError(format!(
            "{} (SQL Server error {}, line {})",
            redact_text(token.message()),
            token.code(),
            token.line()
        )),
        _ => DatabaseError::ConnectionError(text),
    }
}

/// Whether `error` leaves the session usable.
fn keeps_session(error: &TdsError) -> bool {
    matches!(error, TdsError::Server(_))
}

/// One result set of a batch.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ResultSet {
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<Option<String>>>,
}

/// What a batch produced: the first result set that has columns, how many
/// more there were, and whether the row cap cut the first one.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Batch {
    pub(crate) first: Option<ResultSet>,
    pub(crate) extra_sets: usize,
    pub(crate) truncated: bool,
    pub(crate) rows_affected: Option<u64>,
}

/// Why a batch stopped early.
enum Stop {
    Interrupted,
    TimedOut(Duration),
}

/// The session: the connection (absent until first use and after a failure)
/// plus what is needed to open another.
pub(crate) struct Shared {
    pub(crate) info: StdMutex<ConnectionInfo>,
    pub(crate) conn: Mutex<Option<Conn>>,
    /// The database of the session (`DB_NAME()`), kept in step with `USE`.
    pub(crate) current_database: StdMutex<String>,
}

impl Shared {
    pub(crate) fn new(info: ConnectionInfo, conn: Conn, database: String) -> Self {
        Self {
            info: StdMutex::new(info),
            conn: Mutex::new(Some(conn)),
            current_database: StdMutex::new(database),
        }
    }

    pub(crate) fn info(&self) -> ConnectionInfo {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn database(&self) -> String {
        self.current_database
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn set_database(&self, database: &str) {
        *self
            .current_database
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = database.to_string();
        self.info.lock().unwrap_or_else(|e| e.into_inner()).database = Some(database.to_string());
    }

    /// The row cap of one result set (`max_rows`, 0 = none).
    pub(crate) fn max_rows(&self) -> usize {
        let info = self.info();
        option(&info, OPT_MAX_ROWS)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_ROWS)
    }

    /// The deadline of one statement: the shorter of the CLI query timeout
    /// and the broker-style hardening timeout.
    fn deadline(&self) -> Option<Duration> {
        let hardened = connection_timeout_secs(&self.info().options).map(Duration::from_secs);
        match (query_timeout(), hardened) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Make sure `slot` holds a live connection (reconnecting after a drop).
    async fn ensure<'a>(&self, slot: &'a mut Option<Conn>) -> Result<&'a mut Conn, DatabaseError> {
        if slot.is_none() {
            let info = self.info();
            debug!("[SQL Server] reconnecting to {}", describe_target(&info));
            *slot = Some(open(&info).await?);
        }
        slot.as_mut().ok_or_else(|| {
            DatabaseError::ConnectionError("SQL Server connection unavailable".to_string())
        })
    }

    /// Run `sql` as one T-SQL batch.
    ///
    /// A plain single DML statement goes through `sp_executesql` so the row
    /// count can be reported; everything else is a batch whose first result
    /// set is returned (the rest are read and dropped, so a later statement of
    /// the batch still runs).
    pub(crate) async fn run_batch(&self, sql: &str) -> Result<Batch, DatabaseError> {
        let max_rows = self.max_rows();
        let deadline = self.deadline();
        let plain_dml = is_plain_dml(sql);
        let mut slot = self.conn.lock().await;
        let client = self.ensure(&mut slot).await?;

        let interrupt = interrupt_flag();
        let started = tokio::time::Instant::now();
        let mut stop: Option<Stop> = None;
        let outcome = {
            let work = async {
                if plain_dml {
                    execute_dml(client, sql).await
                } else {
                    read_batch(client, sql, max_rows).await
                }
            };
            tokio::pin!(work);
            loop {
                tokio::select! {
                    done = &mut work => break Some(done),
                    _ = tokio::time::sleep(POLL_INTERVAL) => {
                        if interrupt.load(Ordering::Relaxed) {
                            stop = Some(Stop::Interrupted);
                            break None;
                        }
                        if let Some(limit) = deadline {
                            if started.elapsed() >= limit {
                                stop = Some(Stop::TimedOut(limit));
                                break None;
                            }
                        }
                    }
                }
            }
        };

        match (outcome, stop) {
            (Some(Ok((batch, cancel_needed))), _) => {
                if cancel_needed {
                    // The row cap ended the read early: tell the server to stop
                    // sending, then the session is clean again
                    cancel(&mut slot).await;
                }
                Ok(batch)
            }
            (Some(Err(error)), _) => {
                if !keeps_session(&error) {
                    *slot = None;
                }
                Err(query_error(&error))
            }
            (None, Some(reason)) => {
                cancel(&mut slot).await;
                Err(DatabaseError::QueryError(match reason {
                    Stop::Interrupted => "query interrupted".to_string(),
                    Stop::TimedOut(limit) => format!(
                        "query timed out after {}s and was cancelled",
                        limit.as_secs()
                    ),
                }))
            }
            (None, None) => Err(DatabaseError::QueryError("query aborted".to_string())),
        }
    }

    /// Run a metadata query with `@P1…` parameters; returns the first result
    /// set as text cells. Not subject to the user's timeout and Ctrl-C (it is
    /// bounded by a fixed deadline instead).
    pub(crate) async fn query_params(
        &self,
        sql: &str,
        params: &[&str],
    ) -> Result<Vec<Vec<Option<String>>>, DatabaseError> {
        const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
        let mut slot = self.conn.lock().await;
        let client = self.ensure(&mut slot).await?;
        let owned: Vec<String> = params.iter().map(|p| (*p).to_string()).collect();
        let refs: Vec<&dyn tiberius::ToSql> =
            owned.iter().map(|p| p as &dyn tiberius::ToSql).collect();
        let fetched = tokio::time::timeout(METADATA_TIMEOUT, async {
            let stream = client.query(sql, &refs).await?;
            let rows = stream.into_first_result().await?;
            Ok::<_, TdsError>(rows)
        })
        .await;
        match fetched {
            Ok(Ok(rows)) => Ok(rows
                .iter()
                .map(|row| {
                    row.cells()
                        .map(|(column, data)| format_cell(column.column_type(), data))
                        .collect()
                })
                .collect()),
            Ok(Err(error)) => {
                if !keeps_session(&error) {
                    *slot = None;
                }
                Err(DatabaseError::MetadataError(match &error {
                    TdsError::Server(token) => redact_text(token.message()),
                    other => redact_text(&other.to_string()),
                }))
            }
            Err(_) => {
                *slot = None;
                Err(DatabaseError::MetadataError(
                    "SQL Server metadata query timed out".to_string(),
                ))
            }
        }
    }

    /// Run a parameterless statement whose rows are not needed (session
    /// toggles such as `SET SHOWPLAN_XML ON`).
    pub(crate) async fn run_statement(&self, sql: &str) -> Result<(), DatabaseError> {
        let mut slot = self.conn.lock().await;
        let client = self.ensure(&mut slot).await?;
        let sent = tokio::time::timeout(CANCEL_GRACE * 2, async {
            client
                .simple_query(sql)
                .await?
                .into_results()
                .await
                .map(|_| ())
        })
        .await;
        match sent {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                if !keeps_session(&error) {
                    *slot = None;
                }
                Err(query_error(&error))
            }
            Err(_) => {
                *slot = None;
                Err(DatabaseError::QueryError(
                    "SQL Server did not answer a session statement".to_string(),
                ))
            }
        }
    }

    /// Drop the connection (after a failed clean-up, say); the next
    /// statement reconnects.
    pub(crate) async fn discard(&self) {
        *self.conn.lock().await = None;
    }

    /// Run `plan_sql` under `SET SHOWPLAN_XML ON` and return the plan XML
    /// documents. The statement is compiled, never executed; the setting is
    /// always switched off again, and the connection dropped if that fails.
    pub(crate) async fn showplan(&self, plan_sql: &str) -> Result<Vec<String>, DatabaseError> {
        let mut slot = self.conn.lock().await;
        let client = self.ensure(&mut slot).await?;
        let planned = tokio::time::timeout(Duration::from_secs(60), async {
            client
                .simple_query("SET SHOWPLAN_XML ON")
                .await?
                .into_results()
                .await?;
            let plan = fetch_plan(client, plan_sql).await;
            let off = async {
                client
                    .simple_query("SET SHOWPLAN_XML OFF")
                    .await?
                    .into_results()
                    .await
                    .map(|_| ())
            }
            .await;
            Ok::<_, TdsError>((plan, off))
        })
        .await;
        match planned {
            Ok(Ok((plan, off))) => {
                if off.is_err() {
                    // A session stuck in SHOWPLAN mode would answer every
                    // statement with a plan: start over
                    *slot = None;
                }
                plan.map_err(|error| query_error(&error))
            }
            Ok(Err(error)) => {
                if !keeps_session(&error) {
                    *slot = None;
                }
                Err(query_error(&error))
            }
            Err(_) => {
                *slot = None;
                Err(DatabaseError::QueryError(
                    "SQL Server did not return the plan in time".to_string(),
                ))
            }
        }
    }
}

/// The plan documents `sql` produces while `SHOWPLAN_XML` is on.
async fn fetch_plan(client: &mut Conn, sql: &str) -> Result<Vec<String>, TdsError> {
    let rows = client.simple_query(sql).await?.into_first_result().await?;
    Ok(rows
        .iter()
        .flat_map(|row| row.cells())
        .filter_map(|(column, data)| format_cell(column.column_type(), data))
        .collect())
}

/// Send a TDS attention to the statement in flight; drop the connection when
/// the server does not acknowledge it in time.
async fn cancel(slot: &mut Option<Conn>) {
    let Some(client) = slot.as_mut() else { return };
    match tokio::time::timeout(CANCEL_GRACE, client.cancel_query()).await {
        Ok(Ok(())) => debug!("[SQL Server] statement cancelled"),
        Ok(Err(error)) => {
            warn!("[SQL Server] cancel failed ({error}); reconnecting on the next statement");
            *slot = None;
        }
        Err(_) => {
            warn!("[SQL Server] cancel not acknowledged; reconnecting on the next statement");
            *slot = None;
        }
    }
}

/// Read a batch: keep the first result set that has columns (up to
/// `max_rows`), count the others. Returns the batch and whether the caller
/// must cancel the server-side statement (the cap ended the read early).
async fn read_batch(
    client: &mut Conn,
    sql: &str,
    max_rows: usize,
) -> Result<(Batch, bool), TdsError> {
    let mut stream = client.simple_query(sql).await?;
    let mut batch = Batch::default();
    let mut types: Vec<ColumnType> = Vec::new();
    // Index of the result set being read, and the one we keep
    let (mut current, mut kept): (Option<usize>, Option<usize>) = (None, None);
    while let Some(item) = stream.try_next().await? {
        match item {
            QueryItem::Metadata(meta) => {
                current = Some(meta.result_index());
                if kept.is_none() && !meta.columns().is_empty() {
                    kept = current;
                    types = meta.columns().iter().map(|c| c.column_type()).collect();
                    batch.first = Some(ResultSet {
                        columns: meta
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect(),
                        rows: Vec::new(),
                    });
                } else {
                    batch.extra_sets += 1;
                }
            }
            QueryItem::Row(row) => {
                if current != kept {
                    continue;
                }
                let Some(first) = batch.first.as_mut() else {
                    continue;
                };
                if max_rows > 0 && first.rows.len() >= max_rows {
                    batch.truncated = true;
                    drop(stream);
                    return Ok((batch, true));
                }
                first.rows.push(
                    row.cells()
                        .zip(types.iter())
                        .map(|((_, data), column_type)| format_cell(*column_type, data))
                        .collect(),
                );
            }
        }
    }
    Ok((batch, false))
}

/// Run a plain DML statement and report how many rows it changed.
async fn execute_dml(client: &mut Conn, sql: &str) -> Result<(Batch, bool), TdsError> {
    let result = client.execute(sql, &[]).await?;
    Ok((
        Batch {
            rows_affected: Some(result.total()),
            ..Batch::default()
        },
        false,
    ))
}

/// Whether `sql` is one INSERT/UPDATE/DELETE/MERGE statement with no result
/// set of its own (no `OUTPUT`, no trailing `SELECT`, no second statement),
/// judged on the text with strings, comments and quoted names blanked out.
pub(crate) fn is_plain_dml(sql: &str) -> bool {
    let blanked = blank_literals(sql);
    let mut words = blanked
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty());
    let Some(first) = words.next() else {
        return false;
    };
    if !["INSERT", "UPDATE", "DELETE", "MERGE"]
        .iter()
        .any(|verb| first.eq_ignore_ascii_case(verb))
    {
        return false;
    }
    let has_output = blanked
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|w| w.eq_ignore_ascii_case("OUTPUT") || w.eq_ignore_ascii_case("SELECT"));
    let trimmed = blanked.trim_end().trim_end_matches(';');
    !has_output && !trimmed.contains(';')
}

/// `sql` with string literals, `[bracketed]`/`"quoted"` names and comments
/// replaced by spaces (same length), so keyword sniffing cannot be fooled.
pub(crate) fn blank_literals(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let (close, doubled) = match c {
            '\'' => ('\'', true),
            '"' => ('"', true),
            '[' => (']', true),
            _ => ('\0', false),
        };
        if doubled {
            out.push(' ');
            i += 1;
            while i < chars.len() {
                if chars[i] == close {
                    if chars.get(i + 1) == Some(&close) {
                        out.push_str("  ");
                        i += 2;
                        continue;
                    }
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let mut level = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    level += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    level -= 1;
                    out.push_str("  ");
                    i += 2;
                    if level == 0 {
                        break;
                    }
                } else {
                    out.push(' ');
                    i += 1;
                }
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}
