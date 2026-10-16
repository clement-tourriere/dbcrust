//! Amazon Redshift implementation of the database abstraction layer.
//!
//! Redshift speaks the PostgreSQL wire protocol over a PostgreSQL 8.0.2-era
//! catalog, which is why it has a driver of its own on the same sqlx instead of
//! being a flavour of `dbcrust-driver-postgres`: that client leans on exactly
//! what Redshift lacks (the extended protocol with its `pg_type`/`pg_range`
//! type lookups, `options=-c …` startup parameters, `pg_catalog` columns newer
//! than 8.0.2, `EXPLAIN (FORMAT JSON)`, pgvector and extension probes).
//!
//! - **Statements** run as simple queries on one pinned connection, so
//!   transactions, `SET` and temp tables persist, and values arrive as text
//!   ([`value`]): `SUPER` is JSON text, `VARBYTE` hex, `GEOMETRY` hex EWKB.
//! - **TLS** is required by default (`sslmode=require`); `verify-full` checks
//!   the cluster certificate against the bundled web roots (Amazon Root CA 1-4
//!   among them) or `sslrootcert=`.
//! - **Metadata** comes from the `SVV_*` views ([`metadata`]), Spectrum
//!   (external) schemas included.
//! - **EXPLAIN** is the text plan, one line per row.
//! - **Read-only**: `--read-only` runs every statement inside
//!   `BEGIN READ ONLY` … `ROLLBACK`, behind the textual guard in
//!   `dbcrust_core::safety`. Redshift has no session-level read-only default,
//!   so this is a transaction-scoped extra, not a server guarantee: the
//!   user's grants are the real limit (`GRANT SELECT` only).
//! - **Cancel**: Ctrl-C and the query timeout call `pg_cancel_backend` from a
//!   second connection.
//!
//! IAM authentication and the Redshift Data API are not part of this driver
//! (user and password only); see the user guide.

mod conn;
mod metadata;
mod value;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider, ServerInfo,
    StructuredQueryResult,
};
use sqlx::{Executor, PgPool};
use tracing::debug;

use conn::{DEFAULT_DATABASE, SessionState, open_pool, wants_read_only};

pub use metadata::RedshiftMetadataProvider;

/// Redshift database client implementation
pub struct RedshiftClient {
    pub(crate) pool: PgPool,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: RedshiftMetadataProvider,
    pub(crate) session: tokio::sync::Mutex<SessionState>,
    /// Statements run inside `BEGIN READ ONLY` (see the crate docs).
    pub(crate) read_only: bool,
}

impl RedshiftClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let pool = open_pool(&connection_info).await?;
        let current_database = connection_info
            .database
            .clone()
            .unwrap_or_else(|| DEFAULT_DATABASE.to_string());
        Ok(Self {
            metadata_provider: RedshiftMetadataProvider::new(pool.clone()),
            read_only: wants_read_only(&connection_info),
            pool,
            connection_info,
            current_database,
            session: tokio::sync::Mutex::new(SessionState::new()),
        })
    }
}

/// `sql` without a leading `EXPLAIN` (explain mode may hand it over already
/// prefixed; `EXPLAIN EXPLAIN …` is a syntax error).
fn explain_target(sql: &str) -> &str {
    let trimmed = sql.trim();
    match trimmed.split_once(char::is_whitespace) {
        Some((word, rest)) if word.eq_ignore_ascii_case("explain") => rest.trim_start(),
        _ => trimmed,
    }
}

/// The `Redshift 1.0.NNNNN` part of `version()`, which otherwise reads
/// `PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by gcc …, Redshift 1.0.94871`.
fn redshift_version(version: &str) -> Option<&str> {
    let start = version.find("Redshift ")? + "Redshift ".len();
    let rest = &version[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    let found = &rest[..end];
    (!found.is_empty()).then_some(found)
}

#[async_trait]
impl DatabaseClient for RedshiftClient {
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
        // EXPLAIN validates syntax and names without running the statement
        let explain = format!("EXPLAIN {}", explain_target(sql));
        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Executor::fetch_all(&self.pool, explain.as_str()),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(DatabaseError::QueryError(format!(
                "Query validation failed: {e}"
            ))),
            Err(_) => Err(DatabaseError::QueryError(
                "Query validation timed out".to_string(),
            )),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let plan = self
            .query_structured(&format!("EXPLAIN {}", explain_target(sql)))
            .await?;
        Ok(value::plan_table(plan))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        // Redshift has no JSON plan: the raw form is the text plan as well
        self.explain_query(sql).await
    }

    async fn explain_analyze_query_raw(
        &self,
        _sql: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(DatabaseError::FeatureNotSupported {
            database_type: dbcrust_core::database::DatabaseType::Redshift,
            feature: "EXPLAIN ANALYZE (Redshift has no executed plan; use SVL_QUERY_REPORT or SYS_QUERY_DETAIL for a finished query)"
                .to_string(),
        })
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        // pg_database also lists the internal `padb_harvest` and `rdsdb`
        const SQL: &str = "\
SELECT d.datname AS \"Name\", u.usename AS \"Owner\" \
FROM pg_database d LEFT JOIN pg_user u ON u.usesysid = d.datdba \
WHERE d.datistemplate = false AND d.datname NOT IN ('padb_harvest', 'rdsdb') \
ORDER BY d.datname";
        self.execute_query(SQL).await
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        // A Redshift connection is bound to one database: open a new one
        let mut next = self.connection_info.clone();
        next.database = Some(database.to_string());
        *self = RedshiftClient::new(next).await?;
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
        Executor::fetch_all(&self.pool, "SELECT 1").await.is_ok()
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        self.pool.close().await;
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let row = Executor::fetch_one(&self.pool, "SELECT version(), current_user")
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get Redshift version: {e}"))
            })?;
        let banner = value::cell(&row, 0).unwrap_or_default();
        debug!("[RedshiftClient::get_server_info] Raw version string: {banner}");

        // `version()` leads with the PostgreSQL 8.0.2 ancestry; the number
        // that matters is the Redshift build
        let version = redshift_version(&banner).unwrap_or(banner.as_str());
        let mut info = ServerInfo::new("Redshift".to_string(), version.to_string());
        info.supports_transactions = true;
        info.supports_roles = true;
        info.parse_version_numbers();
        info.additional_info.insert(
            "current_database".to_string(),
            self.current_database.clone(),
        );
        if let Some(user) = value::cell(&row, 1) {
            info.additional_info
                .insert("current_user".to_string(), user);
        }
        info.additional_info.insert("banner".to_string(), banner);
        Ok(info)
    }
}

impl std::fmt::Debug for RedshiftClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The connection info's own Debug redacts the password
        f.debug_struct("RedshiftClient")
            .field("connection_info", &self.connection_info)
            .field("current_database", &self.current_database)
            .field("read_only", &self.read_only)
            .finish_non_exhaustive()
    }
}
