//! The connection layer: open a `ConnectionManager`, send one command with the
//! process-wide timeout and interrupt flag honoured.
//!
//! A `ConnectionManager` multiplexes requests over one socket and reconnects in
//! the background after a connection error. A command that failed on a dropped
//! connection is **not** replayed (a retried `INCR` would double-count), so the
//! error reaches the caller and the next command uses the new connection.

use crate::connect::{build_connection_info, connect_timeout};
use dbcrust_core::database::{ConnectionInfo, DatabaseError, interrupt_flag, query_timeout};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{Cmd, RedisError, Value};
use std::time::{Duration, Instant};

/// How often a running command re-checks the interrupt flag and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Connect to the server `info` describes, at logical database `db`.
pub(crate) async fn open(
    info: &ConnectionInfo,
    db: i64,
) -> Result<ConnectionManager, DatabaseError> {
    if info.use_tls {
        // The TLS handshake takes the process-default rustls provider
        dbcrust_core::tls::install_default_crypto_provider();
    }
    let target = describe_target(info);
    let connection = build_connection_info(info, db)?;
    let client = redis::Client::open(connection)
        .map_err(|error| DatabaseError::InvalidUrl(error.to_string()))?;
    // The per-command deadline is enforced in `send` against the live
    // `query_timeout()`, so the manager itself waits as long as asked
    let config = ConnectionManagerConfig::new()
        .set_connection_timeout(Some(connect_timeout(info)))
        .set_response_timeout(None)
        .set_number_of_retries(1);
    ConnectionManager::new_with_config(client, config)
        .await
        .map_err(|error| {
            DatabaseError::ConnectionError(format!(
                "Could not connect to Redis at {target}: {error}"
            ))
        })
}

/// `host:port` (or the socket path) for messages. Never includes credentials.
pub(crate) fn describe_target(info: &ConnectionInfo) -> String {
    if let Some(socket) = &info.file_path {
        return socket.clone();
    }
    format!(
        "{}:{}",
        info.host.as_deref().unwrap_or(crate::connect::DEFAULT_HOST),
        info.port.unwrap_or(crate::connect::DEFAULT_PORT)
    )
}

/// A server or transport error as the right [`DatabaseError`]: a lost
/// connection is a connection error, everything else a query error.
pub(crate) fn error_from(error: RedisError) -> DatabaseError {
    if error.is_io_error() || error.is_connection_dropped() || error.is_connection_refusal() {
        DatabaseError::ConnectionError(error.to_string())
    } else {
        DatabaseError::QueryError(error.to_string())
    }
}

/// Send `cmd`, giving up on Ctrl-C or when the configured query timeout passes.
///
/// A command already on the wire cannot be cancelled (Redis has no per-request
/// cancel); abandoning it leaves its reply to be discarded by the multiplexer.
pub(crate) async fn send(manager: &ConnectionManager, cmd: &Cmd) -> Result<Value, DatabaseError> {
    let mut connection = manager.clone();
    let interrupt = interrupt_flag();
    let started = Instant::now();
    let request = cmd.query_async::<Value>(&mut connection);
    tokio::pin!(request);
    loop {
        tokio::select! {
            reply = &mut request => return reply.map_err(error_from),
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if interrupt.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(DatabaseError::QueryError("query interrupted".to_string()));
                }
                if let Some(limit) = query_timeout() {
                    if started.elapsed() >= limit {
                        return Err(DatabaseError::QueryError(format!(
                            "query timed out after {}s (the server may still be running it)",
                            limit.as_secs()
                        )));
                    }
                }
            }
        }
    }
}

/// Build a command from already split arguments (`args[0]` is the name).
pub(crate) fn command_from_args(args: &[Vec<u8>]) -> Cmd {
    let name = String::from_utf8_lossy(&args[0]).into_owned();
    let mut cmd = redis::cmd(&name);
    for arg in &args[1..] {
        cmd.arg(arg.as_slice());
    }
    cmd
}

/// Build a command from literal parts (metadata queries).
pub(crate) fn command(parts: &[&str]) -> Cmd {
    let mut cmd = redis::cmd(parts[0]);
    for part in &parts[1..] {
        cmd.arg(*part);
    }
    cmd
}
