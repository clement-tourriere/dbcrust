//! libSQL / Turso implementation of the database abstraction layer: **remote
//! only**, over Hrana-on-HTTP ([`hrana`]).
//!
//! The server is SQLite (Turso Cloud, or a self-hosted `sqld`), so a query is
//! SQLite SQL and the catalog is `sqlite_schema` + the `pragma_*` functions
//! ([`metadata`]). Local database files stay on the SQLite driver; embedded
//! replicas and sync are out of scope.
//!
//! - **Statements** are stateless by default (`[execute, close]` in one
//!   request, no baton). A `BEGIN` keeps the server-side stream open and the
//!   baton it returns is carried until `COMMIT`/`END`/`ROLLBACK`, so
//!   transactions span statements. A baton the server has expired ends the
//!   transaction with an error that says so.
//! - **The auth token** is the connection's password: it comes from
//!   `libsql://:TOKEN@host` or `?authToken=` (moved there by the URL parser),
//!   lives in the secret store, and is scrubbed from every message.
//! - **Read-only** (`--read-only`, the AI agent): after the textual guard in
//!   `dbcrust_core::safety`, the server is asked `describe` for each statement
//!   and anything it does not call read-only (`sqlite3_stmt_readonly`) is
//!   refused. A read-only *token* is the only hard limit; it cannot be proven
//!   from the client, which is why the broker does not enroll libSQL.
//! - **Cancel**: Hrana has none. Ctrl-C and the timeout drop the request; the
//!   server finishes or abandons the statement on its own.
//! - **Rows** arrive whole (`/v2/pipeline`): the row cap bounds what is
//!   shown, not what is transferred, so keep `LIMIT` on (the default).
//!
//! The server's per-statement stats (`rows_read`, `rows_written`, duration:
//! what Turso bills) are logged at debug level.

mod hrana;
mod metadata;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider, ServerInfo,
    StructuredQueryResult, connection_read_only, connection_timeout_secs,
};
use dbcrust_core::safety::READ_ONLY_VIOLATION_MARKER;
use tracing::debug;

use hrana::{
    Endpoint, HranaClient, HranaError, StmtResult, StreamRequest, StreamResponse, StreamResult,
    cell,
};
pub use metadata::LibSqlMetadataProvider;

/// A Hrana statement error as a [`DatabaseError`] (`SQLITE_CONSTRAINT: …`).
pub(crate) fn statement_error(error: &HranaError) -> DatabaseError {
    DatabaseError::QueryError(match &error.code {
        Some(code) if !code.is_empty() && !error.message.contains(code.as_str()) => {
            format!("{}: {}", code, error.message)
        }
        _ => error.message.clone(),
    })
}

/// A statement result as a table; a statement without columns and rows (DML,
/// DDL) is the empty default, like the other SQL drivers.
pub(crate) fn rows_of(result: &StmtResult) -> Result<StructuredQueryResult, DatabaseError> {
    if result.rows.is_empty() {
        return Ok(StructuredQueryResult::default());
    }
    let columns = result
        .cols
        .iter()
        .enumerate()
        .map(|(index, col)| {
            col.name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("column{}", index + 1))
        })
        .collect();
    let rows = result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| cell(value).map_err(DatabaseError::QueryError))
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StructuredQueryResult { columns, rows })
}

/// The `execute` result of a pipeline answer.
fn execute_result(results: Vec<StreamResult>) -> Result<StmtResult, DatabaseError> {
    match results.into_iter().next() {
        Some(StreamResult::Ok {
            response: StreamResponse::Execute { result },
        }) => Ok(result),
        Some(StreamResult::Error { error }) => Err(statement_error(&error)),
        _ => Err(DatabaseError::QueryError(
            "the server answered with the wrong kind of response".to_string(),
        )),
    }
}

// ----- statement classification (transactions) ----------------------------------

/// The first keyword of `sql`, upper-cased, after comments and whitespace.
fn first_keyword(sql: &str) -> String {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after
                .split_once('\n')
                .map_or("", |(_, tail)| tail)
                .trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after
                .split_once("*/")
                .map_or("", |(_, tail)| tail)
                .trim_start();
        } else {
            break;
        }
    }
    rest.split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

/// `BEGIN [DEFERRED|IMMEDIATE|EXCLUSIVE] [TRANSACTION]`
fn begins_transaction(sql: &str) -> bool {
    first_keyword(sql) == "BEGIN"
}

/// `COMMIT`, `END` or `ROLLBACK` (but not `ROLLBACK TO savepoint`).
fn ends_transaction(sql: &str) -> bool {
    match first_keyword(sql).as_str() {
        "COMMIT" | "END" => true,
        "ROLLBACK" => {
            let upper = sql.to_ascii_uppercase();
            !upper
                .split(|c: char| !c.is_ascii_alphabetic())
                .any(|word| word == "TO")
        }
        _ => false,
    }
}

/// `sql` without a leading `EXPLAIN [QUERY PLAN]`.
fn explain_target(sql: &str) -> &str {
    let mut rest = sql.trim();
    for word in ["EXPLAIN", "QUERY", "PLAN"] {
        match rest.split_once(char::is_whitespace) {
            Some((first, tail)) if first.eq_ignore_ascii_case(word) => rest = tail.trim_start(),
            _ => break,
        }
    }
    rest
}

/// The `a` (access) claim of a JWT auth token, for display only: the client
/// cannot know the server checks it.
pub fn token_access(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("a")?.as_str().map(str::to_string)
}

// ----- the client ----------------------------------------------------------------

/// A server-side stream a transaction is running in.
struct OpenStream {
    baton: String,
    base_url: Option<String>,
}

/// libSQL database client implementation
pub struct LibSqlClient {
    hrana: Arc<HranaClient>,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: LibSqlMetadataProvider,
    stream: tokio::sync::Mutex<Option<OpenStream>>,
    read_only: bool,
}

impl LibSqlClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let endpoint = Endpoint::from_info(&connection_info)?;
        let ceiling = connection_timeout_secs(&connection_info.options).map(Duration::from_secs);
        let hrana = Arc::new(HranaClient::new(endpoint, ceiling)?);
        let current_database = connection_info
            .database
            .clone()
            .unwrap_or_else(|| "main".to_string());
        let client = Self {
            metadata_provider: LibSqlMetadataProvider::new(hrana.clone()),
            read_only: connection_read_only(&connection_info.options),
            hrana,
            connection_info,
            current_database,
            stream: tokio::sync::Mutex::new(None),
        };
        // Connect for real: a wrong host, token or namespace fails here, with
        // the server's reason, instead of on the first query
        client.probe().await.map_err(|error| match error {
            DatabaseError::QueryError(message) => DatabaseError::ConnectionError(message),
            other => other,
        })?;
        Ok(client)
    }

    /// `SELECT 1` in a stream of its own.
    async fn probe(&self) -> Result<(), DatabaseError> {
        let outcome = self
            .hrana
            .pipeline(
                None,
                None,
                &[StreamRequest::execute("SELECT 1"), StreamRequest::Close],
            )
            .await?;
        execute_result(outcome.results).map(|_| ())
    }

    /// Ask the server whether `sql` is read-only (`sqlite3_stmt_readonly`).
    async fn ensure_read_only(&self, sql: &str) -> Result<(), DatabaseError> {
        let outcome = self
            .hrana
            .pipeline(
                None,
                None,
                &[
                    StreamRequest::Describe {
                        sql: sql.to_string(),
                    },
                    StreamRequest::Close,
                ],
            )
            .await?;
        match outcome.results.into_iter().next() {
            Some(StreamResult::Ok {
                response: StreamResponse::Describe { result },
            }) => match result.is_readonly {
                Some(true) => Ok(()),
                Some(false) => Err(DatabaseError::QueryError(format!(
                    "{READ_ONLY_VIOLATION_MARKER} the server says this statement is not read-only"
                ))),
                None => Err(DatabaseError::QueryError(format!(
                    "{READ_ONLY_VIOLATION_MARKER} the server did not say whether this statement is read-only"
                ))),
            },
            Some(StreamResult::Error { error }) => Err(statement_error(&error)),
            _ => Err(DatabaseError::QueryError(
                "the server answered with the wrong kind of response".to_string(),
            )),
        }
    }

    /// Run one statement; inside a transaction it continues the open stream.
    async fn run(&self, sql: &str) -> Result<StmtResult, DatabaseError> {
        if self.read_only {
            self.ensure_read_only(sql).await?;
        }
        let begins = begins_transaction(sql);
        let ends = ends_transaction(sql);

        let mut stream = self.stream.lock().await;
        let keep_open = stream.is_some() || begins;
        let mut requests = vec![StreamRequest::execute(sql)];
        if !keep_open || ends {
            requests.push(StreamRequest::Close);
        }
        let (baton, base_url) = match stream.as_ref() {
            Some(open) => (Some(open.baton.clone()), open.base_url.clone()),
            None => (None, None),
        };

        let outcome = match self
            .hrana
            .pipeline(base_url.as_deref(), baton.as_deref(), &requests)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                if baton.is_some() {
                    // The stream is gone (expired, server restarted): so is the transaction
                    *stream = None;
                    return Err(DatabaseError::QueryError(format!(
                        "{error}; the open transaction was lost (the server closed its stream), start again with BEGIN"
                    )));
                }
                return Err(error);
            }
        };

        let failed = matches!(outcome.results.first(), Some(StreamResult::Error { .. }));
        *stream = if keep_open && !ends && !(begins && failed) {
            outcome.baton.clone().map(|baton| OpenStream {
                baton,
                base_url: outcome.base_url.clone().or(base_url.clone()),
            })
        } else {
            None
        };
        if begins && failed {
            // A BEGIN that failed leaves nothing to keep: close what it opened
            if let Some(baton) = outcome.baton.as_deref() {
                let _ = self
                    .hrana
                    .pipeline(
                        outcome.base_url.as_deref(),
                        Some(baton),
                        &[StreamRequest::Close],
                    )
                    .await;
            }
        }
        drop(stream);

        let result = execute_result(outcome.results)?;
        debug!(
            "[LibSqlClient] {} row(s), rows_read={:?} rows_written={:?} affected={} in {:?} ms",
            result.rows.len(),
            result.rows_read,
            result.rows_written,
            result.affected_row_count,
            result.query_duration_ms
        );
        Ok(result)
    }

    async fn query_structured(&self, sql: &str) -> Result<StructuredQueryResult, DatabaseError> {
        rows_of(&self.run(sql).await?)
    }
}

/// `EXPLAIN QUERY PLAN` rows (`id`, `parent`, `notused`, `detail`) as an
/// indented tree, one line per step.
fn plan_lines(rows: &[Vec<Option<String>>]) -> Vec<Vec<String>> {
    let parent_of = |id: &str| {
        rows.iter()
            .find(|row| row.first().and_then(|cell| cell.as_deref()) == Some(id))
            .and_then(|row| row.get(1).cloned().flatten())
    };
    let mut lines = vec![vec!["libSQL Query Plan".to_string()], vec![String::new()]];
    for row in rows {
        let detail = row.get(3).cloned().flatten().unwrap_or_default();
        let mut depth = 0;
        let mut parent = row.get(1).cloned().flatten();
        while let Some(id) = parent.filter(|id| id != "0" && !id.is_empty()) {
            depth += 1;
            if depth > 32 {
                break;
            }
            parent = parent_of(&id);
        }
        lines.push(vec![format!("{}{detail}", "  ".repeat(depth))]);
    }
    lines
}

#[async_trait]
impl DatabaseClient for LibSqlClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let structured = self.query_structured(sql).await?;
        if structured.columns.is_empty() && structured.rows.is_empty() {
            // Legacy shape for empty results: no header row at all
            return Ok(vec![]);
        }
        // Legacy NULL sentinel: SQL NULL has always rendered as ""
        Ok(structured.into_display_rows(""))
    }

    async fn execute_query_structured(
        &self,
        sql: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        self.query_structured(sql).await
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        // The server prepares the statement without running it
        let outcome = self
            .hrana
            .pipeline(
                None,
                None,
                &[
                    StreamRequest::Describe {
                        sql: explain_target(sql).to_string(),
                    },
                    StreamRequest::Close,
                ],
            )
            .await
            .map_err(|error| {
                DatabaseError::QueryError(format!("Query validation failed: {error}"))
            })?;
        match outcome.results.into_iter().next() {
            Some(StreamResult::Error { error }) => Err(DatabaseError::QueryError(format!(
                "Query validation failed: {}",
                statement_error(&error)
            ))),
            _ => Ok(()),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let plan = self
            .query_structured(&format!("EXPLAIN QUERY PLAN {}", explain_target(sql)))
            .await?;
        Ok(plan_lines(&plan.rows))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let plan = self
            .query_structured(&format!("EXPLAIN QUERY PLAN {}", explain_target(sql)))
            .await?;
        Ok(plan.into_display_rows(""))
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let result = self.query_structured("PRAGMA database_list").await?;
        Ok(result.into_display_rows(""))
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        // A namespace is part of the connection (the `x-namespace` header)
        let mut next = self.connection_info.clone();
        next.database = Some(database.to_string());
        *self = LibSqlClient::new(next).await?;
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.current_database.clone()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        self.probe().await.is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // Release the server-side stream of a transaction left open
        if let Some(open) = self.stream.get_mut().take() {
            let _ = self
                .hrana
                .pipeline(
                    open.base_url.as_deref(),
                    Some(&open.baton),
                    &[StreamRequest::Close],
                )
                .await;
        }
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let version = self
            .query_structured("SELECT sqlite_version()")
            .await
            .map_err(|error| {
                DatabaseError::QueryError(format!("Failed to get the SQLite version: {error}"))
            })?;
        let version = version
            .rows
            .first()
            .and_then(|row| row.first().cloned().flatten())
            .unwrap_or_default();
        let mut info = ServerInfo::new("libSQL".to_string(), version);
        info.supports_transactions = true;
        info.supports_roles = false;
        info.parse_version_numbers();
        let endpoint = &self.hrana.endpoint;
        info.additional_info
            .insert("endpoint".to_string(), endpoint.base_url.clone());
        if let Some(namespace) = &endpoint.namespace {
            info.additional_info
                .insert("namespace".to_string(), namespace.clone());
        }
        if let Some(access) = endpoint.token.as_deref().and_then(token_access) {
            // What the token says about itself; the server enforces it, DBCrust cannot check
            info.additional_info
                .insert("token_access_claim".to_string(), access);
        }
        Ok(info)
    }
}

impl std::fmt::Debug for LibSqlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibSqlClient")
            .field("endpoint", &self.hrana.endpoint)
            .field("read_only", &self.read_only)
            .finish_non_exhaustive()
    }
}
