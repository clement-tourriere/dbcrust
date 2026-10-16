//! SQL Server / Azure SQL implementation of the database abstraction layer.
//!
//! TDS over TCP through the vendored `tiberius` (rustls on ring; see
//! `third_party/tiberius/DBCRUST_PATCH.md`), SQL authentication only. One
//! pinned session serves the whole connection, so `SET` options, temp tables
//! and transactions persist like in `sqlcmd`.
//!
//! * a batch returns its **first** result set (later ones are read and
//!   dropped, with a log warning), capped at `max_rows` rows (100 000 unless
//!   the option says otherwise; the server is told to stop sending past it);
//! * a statement past the query timeout, or interrupted with Ctrl-C, is
//!   cancelled with a TDS attention signal; if the server does not answer,
//!   the connection is dropped and the next statement reconnects;
//! * `EXPLAIN <statement>` (and the explain mode) compile the statement under
//!   `SET SHOWPLAN_XML ON` and return the estimated plan XML; nothing runs.
//!
//! Metadata comes from the catalog views of the current database (see
//! [`metadata`]); values are formatted as SSMS shows them (see [`value`]).

mod conn;
mod metadata;
#[cfg(test)]
mod tests;
mod value;

use async_trait::async_trait;
use conn::Shared;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider, ServerInfo,
    StructuredQueryResult, connection_read_only,
};
use std::sync::Arc;
use std::time::Duration;

pub use metadata::MssqlMetadataProvider;

/// SQL Server client: one pinned, auto-reconnecting session.
pub struct MssqlClient {
    connection_info: ConnectionInfo,
    shared: Arc<Shared>,
    metadata_provider: MssqlMetadataProvider,
}

impl MssqlClient {
    /// Connect and log in (SQL authentication).
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let client = conn::open(&connection_info).await?;
        let shared = Arc::new(Shared::new(
            connection_info.clone(),
            client,
            connection_info.database.clone().unwrap_or_default(),
        ));
        let metadata_provider = MssqlMetadataProvider::new(shared.clone());
        let this = Self {
            connection_info,
            shared,
            metadata_provider,
        };
        // The database the login landed in (its default when none was asked for)
        if let Ok(rows) = this.shared.query_params("SELECT DB_NAME()", &[]).await {
            if let Some(name) = rows.first().and_then(|row| row.first()).cloned().flatten() {
                this.shared.set_database(&name);
            }
        }
        Ok(this)
    }

    /// Run `sql` and map the first result set to a table.
    async fn execute_checked(&self, sql: &str) -> Result<StructuredQueryResult, DatabaseError> {
        let statement = sql.trim();
        if statement.is_empty() {
            return Ok(StructuredQueryResult::default());
        }
        // Connections built for the credential broker refuse writes here too,
        // whatever the caller already checked
        if connection_read_only(&self.shared.info().options) {
            dbcrust_core::safety::check_read_only(statement, &DatabaseType::SqlServer)
                .map_err(|violation| DatabaseError::QueryError(violation.to_string()))?;
        }
        if let Some(inner) = explain_target(statement)? {
            return self.plan_table(inner).await;
        }

        let batch = self.shared.run_batch(statement).await?;
        if statement_changes_database(statement) {
            self.refresh_database().await;
        }
        if batch.truncated {
            tracing::warn!(
                "SQL Server result cut at {} rows (`max_rows` option; 0 = unlimited); the statement was cancelled",
                self.shared.max_rows()
            );
        }
        if batch.extra_sets > 0 {
            tracing::warn!(
                "SQL Server batch returned {} more result set(s); only the first is shown",
                batch.extra_sets
            );
        }
        Ok(match (batch.first, batch.rows_affected) {
            (Some(set), _) => StructuredQueryResult {
                columns: set.columns,
                rows: set.rows,
            },
            (None, Some(count)) => StructuredQueryResult {
                columns: vec!["Rows affected".to_string()],
                rows: vec![vec![Some(count.to_string())]],
            },
            (None, None) => StructuredQueryResult {
                columns: vec!["Status".to_string()],
                rows: vec![vec![Some("Command(s) completed successfully.".to_string())]],
            },
        })
    }

    /// Pick up the database name after a `USE`.
    async fn refresh_database(&self) {
        if let Ok(rows) = self.shared.query_params("SELECT DB_NAME()", &[]).await {
            if let Some(name) = rows.first().and_then(|row| row.first()).cloned().flatten() {
                self.shared.set_database(&name);
            }
        }
    }

    /// The estimated plan of `statement` as a one-column table.
    async fn plan_table(&self, statement: &str) -> Result<StructuredQueryResult, DatabaseError> {
        let plans = self.shared.showplan(statement).await?;
        Ok(StructuredQueryResult {
            columns: vec!["QUERY PLAN".to_string()],
            rows: plans.into_iter().map(|plan| vec![Some(plan)]).collect(),
        })
    }
}

/// The statement behind a leading `EXPLAIN`, if there is one. `EXPLAIN
/// ANALYZE` is refused: SHOWPLAN never executes the statement.
fn explain_target(statement: &str) -> Result<Option<&str>, DatabaseError> {
    let mut parts = statement.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if !first.eq_ignore_ascii_case("EXPLAIN") {
        return Ok(None);
    }
    let rest = parts.next().unwrap_or_default().trim_start();
    let next_word = rest.split_whitespace().next().unwrap_or_default();
    if next_word.eq_ignore_ascii_case("ANALYZE") {
        return Err(DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::SqlServer,
            feature:
                "EXPLAIN ANALYZE (SQL Server shows the estimated plan; the statement is not run)"
                    .to_string(),
        });
    }
    if rest.is_empty() {
        return Err(DatabaseError::QueryError(
            "EXPLAIN needs a statement to plan".to_string(),
        ));
    }
    Ok(Some(rest))
}

/// Whether `statement` starts with `USE` (the one statement that moves the
/// session to another database behind the driver's back).
fn statement_changes_database(statement: &str) -> bool {
    statement
        .split_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("USE"))
}

#[async_trait]
impl DatabaseClient for MssqlClient {
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

    /// Syntax check without running anything (`SET PARSEONLY`).
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        self.shared.run_statement("SET PARSEONLY ON").await?;
        let parsed = self.shared.run_statement(sql).await;
        if self
            .shared
            .run_statement("SET PARSEONLY OFF")
            .await
            .is_err()
        {
            // A session stuck in PARSEONLY would silently run nothing
            self.shared.discard().await;
        }
        parsed
            .map_err(|error| DatabaseError::QueryError(format!("Query validation failed: {error}")))
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let statement = explain_target(sql.trim())?.unwrap_or(sql.trim());
        Ok(self.plan_table(statement).await?.into_display_rows("NULL"))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        const SQL: &str = "\
SELECT name AS [Database], state_desc AS [State], collation_name AS [Collation],
       compatibility_level AS [Compatibility]
FROM sys.databases
ORDER BY name";
        let rows = self.shared.query_params(SQL, &[]).await?;
        let mut table = vec![vec![
            "Database".to_string(),
            "State".to_string(),
            "Collation".to_string(),
            "Compatibility".to_string(),
        ]];
        table.extend(rows.into_iter().map(|row| {
            row.into_iter()
                .map(|cell| cell.unwrap_or_default())
                .collect()
        }));
        Ok(table)
    }

    /// Open a new session on `database` (a `USE` on the live one would leave a
    /// reconnect on the old database).
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        let mut info = self.shared.info();
        info.database = Some(database.to_string());
        let client = conn::open(&info).await?;
        *self.shared.conn.lock().await = Some(client);
        self.shared.set_database(database);
        self.connection_info.database = Some(database.to_string());
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.shared.database()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        tokio::time::timeout(
            Duration::from_secs(5),
            self.shared.query_params("SELECT 1", &[]),
        )
        .await
        .is_ok_and(|result| result.is_ok())
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        let client = self.shared.conn.lock().await.take();
        if let Some(client) = client {
            // The server learns about it either way; a failed goodbye is moot
            let _ = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
        }
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        const SQL: &str = "\
SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)),
       CAST(SERVERPROPERTY('Edition') AS nvarchar(128)),
       CAST(SERVERPROPERTY('EngineEdition') AS nvarchar(20)),
       @@VERSION";
        let rows = self.shared.query_params(SQL, &[]).await?;
        let row = rows.first().cloned().unwrap_or_default();
        let cell = |index: usize| row.get(index).cloned().flatten().unwrap_or_default();
        let engine_edition = cell(2);
        let mut info = ServerInfo::new("SQL Server".to_string(), cell(0));
        info.parse_version_numbers();
        info.supports_transactions = true;
        info.supports_roles = true;
        info.additional_info.insert("edition".to_string(), cell(1));
        info.additional_info.insert(
            "engine_edition".to_string(),
            match engine_edition.as_str() {
                "5" => "Azure SQL Database".to_string(),
                "8" => "Azure SQL Managed Instance".to_string(),
                "9" => "Azure SQL Edge".to_string(),
                _ => engine_edition,
            },
        );
        info.additional_info.insert("banner".to_string(), cell(3));
        Ok(info)
    }
}
