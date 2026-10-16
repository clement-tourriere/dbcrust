//! DuckDB implementation of the database abstraction layer.
//!
//! An embedded engine (`duckdb` crate, compiled in with the `bundled` feature
//! plus the Parquet and JSON extensions), opened on a dedicated thread that
//! owns the connection for the whole session; see [`conn`].
//!
//! * `duckdb:///path/file.duckdb` opens (or creates) a database file,
//!   `duckdb://:memory:` (or `duckdb://`) an in-memory one;
//! * `--read-only` (and the credential broker) open the file with
//!   `access_mode=READ_ONLY`, turn `enable_external_access` off and lock the
//!   configuration: writes, `COPY … TO`, `ATTACH`, `INSTALL`/`LOAD` and file
//!   functions are refused by the engine itself, not by a textual guard;
//! * a result is streamed and cut at `max_rows` rows (100 000 unless the
//!   option says otherwise); a statement past the query timeout, or Ctrl-C, is
//!   stopped with DuckDB's `interrupt` and the session stays usable;
//! * `ducklake=<catalog>` attaches a DuckLake catalog (see [`ducklake`]).
//!
//! Metadata comes from DuckDB's catalog functions (see [`metadata`]); values
//! are rendered the way DuckDB prints them (see [`value`]).

mod conn;
mod ducklake;
mod metadata;
#[cfg(test)]
mod tests;
mod value;

use async_trait::async_trait;
use conn::Session;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider, ServerInfo,
    StructuredQueryResult, connection_read_only, quote_sql_ident,
};
use std::sync::Arc;

pub use metadata::DuckDbMetadataProvider;

/// DuckDB client: one embedded database, one pinned session.
pub struct DuckDbClient {
    connection_info: ConnectionInfo,
    session: Arc<Session>,
    current_database: std::sync::Mutex<String>,
    metadata_provider: DuckDbMetadataProvider,
}

impl DuckDbClient {
    /// Open the database named by `connection_info` (see the module docs).
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let session = Arc::new(Session::open(&connection_info).await?);
        let metadata_provider = DuckDbMetadataProvider::new(session.clone());
        let this = Self {
            connection_info,
            session,
            current_database: std::sync::Mutex::new("memory".to_string()),
            metadata_provider,
        };
        this.refresh_database().await;
        Ok(this)
    }

    /// Pick up the database name after a `USE`, an `ATTACH` or the open.
    async fn refresh_database(&self) {
        if let Ok(rows) = self
            .session
            .query_params("SELECT current_database()", &[])
            .await
        {
            if let Some(name) = rows.first().and_then(|row| row.first()).cloned().flatten() {
                self.session.set_database(&name);
                *self
                    .current_database
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = name;
            }
        }
    }

    /// Run `sql` as the user typed it and map its result to a table.
    async fn execute_checked(&self, sql: &str) -> Result<StructuredQueryResult, DatabaseError> {
        self.execute_statement(sql, true).await
    }

    /// Run `sql`. `plans_as_lines` lays a typed `EXPLAIN` out one plan line
    /// per row (what a terminal wants); the explain methods ask for the raw
    /// `(explain_key, explain_value)` row instead.
    async fn execute_statement(
        &self,
        sql: &str,
        plans_as_lines: bool,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        let statement = sql.trim();
        if statement.is_empty() {
            return Ok(StructuredQueryResult::default());
        }
        // Connections built for `--read-only` or the credential broker also
        // pass the textual guard, whatever the caller already checked: the
        // engine refuses writes by itself (`access_mode=READ_ONLY`), this
        // adds a clear message and stops the statements that merely probe
        if connection_read_only(&self.session.info().options) {
            dbcrust_core::safety::check_read_only(statement, &DatabaseType::DuckDB)
                .map_err(|violation| DatabaseError::QueryError(violation.to_string()))?;
        }
        let batch = self.session.run(statement).await?;
        if changes_current_database(statement) {
            self.refresh_database().await;
        }
        let is_explain = statement
            .split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("EXPLAIN"));
        if plans_as_lines && is_explain {
            if let Some(lines) = plan_lines(&batch.columns, &batch.rows) {
                return Ok(lines);
            }
        }
        // A command (CREATE, SET, ATTACH…) answers with an empty `Count` or
        // `Success` table: say what happened instead
        if batch.rows.is_empty() && is_command(statement) {
            return Ok(StructuredQueryResult {
                columns: vec!["Status".to_string()],
                rows: vec![vec![Some("OK".to_string())]],
            });
        }
        Ok(StructuredQueryResult {
            columns: batch.columns,
            rows: batch.rows,
        })
    }

    /// The plan of `EXPLAIN [ANALYZE] <sql>` as rows of `(key, value)`.
    async fn explain(&self, prefix: &str, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let statement = sql.trim().trim_end_matches(';');
        if statement.is_empty() {
            return Err(DatabaseError::QueryError(
                "EXPLAIN needs a statement to plan".to_string(),
            ));
        }
        if connection_read_only(&self.session.info().options) {
            dbcrust_core::safety::check_read_only(statement, &DatabaseType::DuckDB)
                .map_err(|violation| DatabaseError::QueryError(violation.to_string()))?;
        }
        let structured = self
            .execute_statement(&format!("{prefix} {statement}"), false)
            .await?;
        Ok(structured.into_display_rows("NULL"))
    }
}

/// An `EXPLAIN` answer (`explain_key`, `explain_value` with one plan) as one
/// plan line per row; `None` for any other result.
fn plan_lines(columns: &[String], rows: &[Vec<Option<String>>]) -> Option<StructuredQueryResult> {
    let is_plan = columns == ["explain_key", "explain_value"] && rows.len() == 1;
    let plan = rows.first()?.get(1)?.as_deref().filter(|_| is_plan)?;
    Some(StructuredQueryResult {
        columns: vec!["QUERY PLAN".to_string()],
        rows: plan
            .lines()
            .map(|line| vec![Some(line.to_string())])
            .collect(),
    })
}

/// Whether `statement` is a command: its verb produces no table of its own.
fn is_command(statement: &str) -> bool {
    const COMMANDS: [&str; 20] = [
        "CREATE",
        "DROP",
        "ALTER",
        "SET",
        "RESET",
        "USE",
        "ATTACH",
        "DETACH",
        "INSTALL",
        "LOAD",
        "CHECKPOINT",
        "VACUUM",
        "ANALYZE",
        "TRUNCATE",
        "COMMENT",
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "EXPORT",
        "IMPORT",
    ];
    statement.split_whitespace().next().is_some_and(|word| {
        COMMANDS
            .iter()
            .any(|command| word.eq_ignore_ascii_case(command))
    })
}

/// Whether `statement` starts with a verb that moves the session to another
/// database behind the driver's back.
fn changes_current_database(statement: &str) -> bool {
    statement.split_whitespace().next().is_some_and(|word| {
        ["USE", "ATTACH", "DETACH"]
            .iter()
            .any(|verb| word.eq_ignore_ascii_case(verb))
    })
}

#[async_trait]
impl DatabaseClient for DuckDbClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let structured = self.execute_checked(sql).await?;
        if structured.columns.is_empty() && structured.rows.is_empty() {
            return Ok(vec![]);
        }
        Ok(structured.into_display_rows("NULL"))
    }

    async fn execute_query_structured(
        &self,
        sql: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        self.execute_checked(sql).await
    }

    /// Syntax and name check without running anything (`EXPLAIN` binds the
    /// statement but does not execute it).
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        let statement = sql.trim().trim_end_matches(';');
        self.session
            .run(&format!("EXPLAIN {statement}"))
            .await
            .map(|_| ())
            .map_err(|error| DatabaseError::QueryError(format!("Query validation failed: {error}")))
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let raw = self.explain("EXPLAIN", sql).await?;
        // One output line per row, so a plan wraps nowhere
        let mut rows = vec![vec!["QUERY PLAN".to_string()]];
        for row in raw.into_iter().skip(1) {
            if let Some(plan) = row.get(1) {
                rows.extend(plan.lines().map(|line| vec![line.to_string()]));
            }
        }
        Ok(rows)
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain("EXPLAIN", sql).await
    }

    async fn explain_analyze_query_raw(
        &self,
        sql: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain("EXPLAIN ANALYZE", sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let rows = self
            .session
            .query_params(
                "SELECT database_name, COALESCE(path, '(in-memory)'), type, \
                        CASE WHEN internal THEN 'System' ELSE 'User' END \
                 FROM duckdb_databases() ORDER BY internal, database_name",
                &[],
            )
            .await?;
        let current = self.get_current_database();
        let mut table = vec![vec![
            "Name".to_string(),
            "Path".to_string(),
            "Type".to_string(),
            "Kind".to_string(),
            "Current".to_string(),
        ]];
        for row in rows {
            let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
            let name = cell(0);
            let is_current = if name == current { "*" } else { "" };
            table.push(vec![
                name,
                cell(1),
                cell(2),
                cell(3),
                is_current.to_string(),
            ]);
        }
        Ok(table)
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        // `USE` takes `catalog` or `catalog.schema`; quote what the user typed
        let target = database
            .split('.')
            .map(quote_sql_ident)
            .collect::<Vec<_>>()
            .join(".");
        self.session.run(&format!("USE {target}")).await?;
        self.refresh_database().await;
        self.connection_info.database = Some(self.get_current_database());
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.current_database
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        self.session.is_open() && self.session.query_params("SELECT 1", &[]).await.is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // Closing checkpoints the database: wait for it off the async threads
        let session = self.session.clone();
        let _ = tokio::task::spawn_blocking(move || session.close()).await;
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let rows = self
            .session
            .query_params("SELECT version()", &[])
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get DuckDB version: {e}")))?;
        let version = rows
            .first()
            .and_then(|row| row.first())
            .cloned()
            .flatten()
            .unwrap_or_default();
        let mut info = ServerInfo::new(
            "DuckDB".to_string(),
            version.trim_start_matches('v').to_string(),
        );
        info.supports_transactions = true;
        info.parse_version_numbers();
        let spec = self.session.spec();
        info.additional_info.insert(
            "database_file".to_string(),
            spec.path.clone().unwrap_or_else(|| "memory".to_string()),
        );
        info.additional_info.insert(
            "access_mode".to_string(),
            if spec.read_only_file {
                "read_only"
            } else {
                "read_write"
            }
            .to_string(),
        );
        if spec.hardened {
            info.additional_info
                .insert("external_access".to_string(), "disabled".to_string());
        }
        for (setting, label) in [("threads", "threads"), ("memory_limit", "memory_limit")] {
            if let Ok(rows) = self
                .session
                .query_params(&format!("SELECT current_setting('{setting}')"), &[])
                .await
            {
                if let Some(value) = rows.first().and_then(|row| row.first()).cloned().flatten() {
                    info.additional_info.insert(label.to_string(), value);
                }
            }
        }
        Ok(info)
    }
}
