//! Connection options and the pinned session connection.
//!
//! Everything here is shaped by what Redshift is not: no `options` startup
//! parameter (so no `-c default_transaction_read_only`/`statement_timeout`
//! there: `SET` after connecting instead), no reliance on the extended
//! protocol (every statement is a simple `Query`, so sqlx never describes a
//! statement or looks an unknown type OID up in `pg_type`), and no
//! `pg_catalog` newer than PostgreSQL 8.0.2.

use std::sync::atomic::Ordering;
use std::time::Duration;

use dbcrust_core::database::{
    ConnectionInfo, DatabaseError, DatabaseType, connection_read_only, connection_timeout_secs,
    interrupt_flag, is_hardened_connection, query_timeout, redshift_sslmode,
};
use futures_util::TryStreamExt;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPoolOptions, PgRow, PgSslMode};
use sqlx::{Either, Executor, PgPool};
use tracing::debug;

use crate::{RedshiftClient, value};

/// The database a connection without one in its URL opens (the console's default).
pub(crate) const DEFAULT_DATABASE: &str = "dev";
/// How long the pool retries a connection (leaves time for an SSH tunnel).
/// A refused connection is retried until it elapses, so tests shorten it.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 15 });

pub(crate) fn db_err(error: sqlx::Error) -> DatabaseError {
    DatabaseError::driver(DatabaseType::Redshift, error)
}

pub(crate) fn ssl_mode(info: &ConnectionInfo) -> PgSslMode {
    // `normalize_redshift_connection` records the canonical value; a
    // `ConnectionInfo` built by hand without it still means `require`
    match info
        .options
        .get("sslmode")
        .and_then(|mode| redshift_sslmode(mode))
    {
        Some("disable") => PgSslMode::Disable,
        Some("allow") => PgSslMode::Allow,
        Some("prefer") => PgSslMode::Prefer,
        Some("verify-ca") => PgSslMode::VerifyCa,
        Some("verify-full") => PgSslMode::VerifyFull,
        _ => PgSslMode::Require,
    }
}

/// sqlx connect options for `info`.
pub(crate) fn connect_options(info: &ConnectionInfo) -> PgConnectOptions {
    // Broker connections never consult ~/.pgpass: the stored record is the
    // only credential source
    let mut options = if is_hardened_connection(&info.options) {
        PgConnectOptions::new_without_pgpass()
    } else {
        PgConnectOptions::new()
    };
    if let Some(host) = &info.host {
        options = options.host(host);
    }
    options = options.port(info.port.or(info.default_port()).unwrap_or(5439));
    if let Some(username) = &info.username {
        options = options.username(username);
    }
    if let Some(password) = &info.password {
        options = options.password(password);
    }
    options = options
        .database(info.database.as_deref().unwrap_or(DEFAULT_DATABASE))
        .ssl_mode(ssl_mode(info))
        .application_name(
            info.options
                .get("application_name")
                .map_or("dbcrust", String::as_str),
        );
    if let Some(root_cert) = info.options.get("sslrootcert") {
        options = options.ssl_root_cert(root_cert);
    }
    options
}

/// The `SET` statements every new connection runs: the statement timeout (the
/// broker's ceiling, milliseconds on Redshift). Not sent as a startup option:
/// Redshift does not take `options=-c …`.
pub(crate) fn session_setup(info: &ConnectionInfo) -> Vec<String> {
    let mut statements = Vec::new();
    if let Some(secs) = connection_timeout_secs(&info.options) {
        statements.push(format!("SET statement_timeout TO {}", secs * 1000));
    }
    statements
}

/// Open the pool. `connect_with` establishes the first connection now, so a
/// wrong host/credential/TLS mode fails here with the server's message.
pub(crate) async fn open_pool(info: &ConnectionInfo) -> Result<PgPool, DatabaseError> {
    let setup = session_setup(info);
    PgPoolOptions::new()
        .max_connections(4)
        .min_connections(0)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .idle_timeout(Duration::from_secs(300))
        .test_before_acquire(false)
        .after_connect(move |conn, _meta| {
            let setup = setup.clone();
            Box::pin(async move {
                for statement in &setup {
                    conn.execute(statement.as_str()).await?;
                }
                Ok(())
            })
        })
        .connect_with(connect_options(info))
        .await
        .map_err(|e| DatabaseError::ConnectionError(e.to_string()))
}

/// State of the pinned per-session connection.
///
/// Interactive statements all run on ONE connection so that transactions,
/// `SET` and temp tables behave like psql. Metadata queries use the pool and
/// never disturb the session.
pub(crate) struct SessionState {
    conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    backend_pid: Option<i32>,
    /// False while a statement is mid-flight: if a new statement starts and
    /// this is still false, the previous future was dropped mid-protocol and
    /// the connection must be discarded.
    clean: bool,
}

impl SessionState {
    pub(crate) fn new() -> Self {
        Self {
            conn: None,
            backend_pid: None,
            clean: true,
        }
    }

    fn discard(&mut self) {
        self.conn = None;
        self.backend_pid = None;
    }
}

enum CancelReason {
    Interrupted,
    TimedOut(Duration),
}

/// Rows of the first result set that returned any.
pub(crate) struct Fetched {
    pub(crate) rows: Vec<PgRow>,
}

/// Run `sql` as a simple query and keep the first non-empty result set (a
/// `a; b` string is accepted; the rest is read and dropped so the connection
/// stays in sync).
async fn run_statement(conn: &mut PgConnection, sql: &str) -> Result<Vec<PgRow>, sqlx::Error> {
    let mut stream = Executor::fetch_many(&mut *conn, sql);
    let mut first: Option<Vec<PgRow>> = None;
    let mut current: Vec<PgRow> = Vec::new();
    while let Some(item) = stream.try_next().await? {
        match item {
            Either::Right(row) => current.push(row),
            Either::Left(_finished) => {
                let rows = std::mem::take(&mut current);
                if first.is_none() && !rows.is_empty() {
                    first = Some(rows);
                }
            }
        }
    }
    Ok(first.unwrap_or_default())
}

/// `run_statement` inside `BEGIN READ ONLY` … `ROLLBACK`: the transaction mode
/// Redshift documents for `BEGIN`. It is defence in depth behind the textual
/// guard, not a claim of server enforcement (there is no session-level
/// read-only default to set, and it cannot be verified without a cluster).
async fn run_statement_read_only(
    conn: &mut PgConnection,
    sql: &str,
) -> Result<Vec<PgRow>, sqlx::Error> {
    Executor::execute(&mut *conn, "BEGIN READ ONLY")
        .await
        .map_err(|e| {
            sqlx::Error::Protocol(format!("could not start a READ ONLY transaction: {e}"))
        })?;
    let result = run_statement(conn, sql).await;
    // The transaction is always closed: after an error it is aborted, and
    // either way nothing was meant to be kept
    let rollback = Executor::execute(&mut *conn, "ROLLBACK").await;
    match (result, rollback) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(rows), Ok(_)) => Ok(rows),
    }
}

impl RedshiftClient {
    /// Run `sql` on the pinned session connection, cancelling it server-side
    /// (`pg_cancel_backend`) when Ctrl-C is pressed or the configured query
    /// timeout elapses. Server-side cancellation lets the in-flight future
    /// complete normally (SQLSTATE 57014) instead of being dropped mid-
    /// protocol, so the session connection stays usable.
    pub(crate) async fn fetch_session(&self, sql: &str) -> Result<Fetched, DatabaseError> {
        let mut session = self.session.lock().await;

        // A connection whose previous statement never completed has an
        // unknown protocol state
        if !session.clean {
            debug!("[RedshiftClient] Discarding dirty session connection");
            session.discard();
        }

        if session.conn.is_none() {
            let mut conn = self
                .pool
                .acquire()
                .await
                .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;
            let pid = pinned_backend_pid(&mut conn).await?;
            debug!("[RedshiftClient] Pinned session connection (backend pid {pid})");
            session.conn = Some(conn);
            session.backend_pid = Some(pid);
        }

        let backend_pid = session.backend_pid.unwrap_or_default();
        // The connection is only put back once the statement completes with a
        // synchronized protocol state; clean=false marks the in-flight window
        session.clean = false;
        let mut conn = session.conn.take().ok_or_else(|| {
            DatabaseError::ConnectionError("session connection unavailable".to_string())
        })?;

        let interrupt = interrupt_flag();
        let timeout = query_timeout();
        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
        let read_only = self.read_only;

        let mut cancel_reason: Option<CancelReason> = None;
        let mut grace_deadline: Option<tokio::time::Instant> = None;
        let mut cancel_expired = false;

        let result = {
            let query_fut = async {
                if read_only {
                    run_statement_read_only(&mut conn, sql).await
                } else {
                    run_statement(&mut conn, sql).await
                }
            };
            tokio::pin!(query_fut);

            loop {
                tokio::select! {
                    res = &mut query_fut => break res,
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        let now = tokio::time::Instant::now();
                        if let Some(grace) = grace_deadline {
                            if now >= grace {
                                // The cancel request was ignored; abandon the
                                // statement (conn is dropped below instead of
                                // being put back)
                                cancel_expired = true;
                                break Err(sqlx::Error::WorkerCrashed);
                            }
                            continue;
                        }
                        let interrupted = interrupt.load(Ordering::Relaxed);
                        let timed_out = deadline.is_some_and(|d| now >= d);
                        if interrupted || timed_out {
                            cancel_reason = Some(if interrupted {
                                CancelReason::Interrupted
                            } else {
                                CancelReason::TimedOut(timeout.unwrap_or_default())
                            });
                            // Best-effort server-side cancel from a separate
                            // pool connection; the pinned query then completes
                            // with SQLSTATE 57014. Bounded: an exhausted pool
                            // must not delay the grace period
                            let _ = tokio::time::timeout(
                                Duration::from_secs(3),
                                Executor::execute(
                                    &self.pool,
                                    format!("SELECT pg_cancel_backend({backend_pid})").as_str(),
                                ),
                            )
                            .await;
                            grace_deadline = Some(now + Duration::from_secs(5));
                        }
                    }
                }
            }
        };

        if cancel_expired {
            // conn is dropped here in unknown protocol state; the next
            // statement pins a fresh one
            return Err(DatabaseError::QueryError(
                "Query did not respond to cancellation; the session connection was discarded"
                    .to_string(),
            ));
        }

        match result {
            Ok(rows) => {
                session.clean = true;
                session.conn = Some(conn);
                Ok(Fetched { rows })
            }
            Err(error) => {
                // A server-reported error leaves the protocol synchronized so
                // the connection can be reused; io/protocol-level failures
                // mean it must be discarded
                if matches!(error, sqlx::Error::Database(_)) {
                    session.clean = true;
                    session.conn = Some(conn);
                } else {
                    session.discard();
                }
                let was_cancelled = matches!(
                    &error,
                    sqlx::Error::Database(db) if db.code().as_deref() == Some("57014")
                );
                match (was_cancelled, cancel_reason) {
                    (true, Some(CancelReason::Interrupted)) => {
                        Err(DatabaseError::QueryError("Query cancelled".to_string()))
                    }
                    (true, Some(CancelReason::TimedOut(t))) => {
                        Err(DatabaseError::QueryError(format!(
                            "Query timed out after {} seconds (query_timeout_seconds)",
                            t.as_secs()
                        )))
                    }
                    _ => Err(DatabaseError::QueryError(error.to_string())),
                }
            }
        }
    }

    /// Run `sql` on the session and map it to a table (NULL kept distinct).
    pub(crate) async fn query_structured(
        &self,
        sql: &str,
    ) -> Result<dbcrust_core::database::StructuredQueryResult, DatabaseError> {
        let fetched = self.fetch_session(sql).await?;
        Ok(value::rows_to_result(&fetched.rows))
    }
}

/// `pg_backend_pid()` of a freshly acquired connection (simple protocol).
async fn pinned_backend_pid(conn: &mut PgConnection) -> Result<i32, DatabaseError> {
    let rows = Executor::fetch_all(&mut *conn, "SELECT pg_backend_pid()")
        .await
        .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;
    rows.first()
        .and_then(|row| value::cell(row, 0))
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| {
            DatabaseError::ConnectionError("the server did not report a backend pid".to_string())
        })
}

/// Whether the connection was asked to be read-only (`--read-only`, the
/// broker's hardening marker).
pub(crate) fn wants_read_only(info: &ConnectionInfo) -> bool {
    connection_read_only(&info.options)
}
