//! InfluxDB implementation of the database abstraction layer, over HTTP.
//!
//! The URL scheme (or `?version=`) picks the API generation, and with it the
//! query language of a "query":
//!
//! | Scheme | Server | Language | Endpoint |
//! |---|---|---|---|
//! | `influxdb://` | v1 | InfluxQL | `GET /query` |
//! | `influxdb2://` | v2 | Flux | `POST /api/v2/query` (annotated CSV) |
//! | `influxdb3://` | v3 | SQL | `POST /api/v3/query_sql` (CSV) |
//!
//! `?language=influxql` sends InfluxQL to a v2 (1.x-compatible API, needs a
//! DBRP mapping) or v3 (`query_influxql`) server. A trailing `s`
//! (`influxdbs://`) or `+https` is TLS. The API token of v2/v3 rides in the
//! URL's password slot (or `?token=`) like a password, and is sent as an
//! `Authorization` header only.
//!
//! Replies are read while they arrive and the read stops at `max_rows` rows
//! (10 000) or `max_bytes` bytes (64 MiB), dropping the connection so the
//! server stops producing. InfluxQL reads go as `GET` and anything else as
//! `POST`; InfluxDB 1.x does not enforce that split (a write sent as `GET`
//! still runs, with only a deprecation warning), so `--read-only` rests on the
//! statement classifier and, for a hard limit, on a read-only user or token.

mod conn;
mod csv;
mod exec;
mod flux;
mod influxql;
mod metadata;
mod table;
#[cfg(test)]
mod tests;

use async_trait::async_trait;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, InfluxLanguage, MetadataProvider,
    ServerInfo, StructuredQueryResult, connection_read_only,
};
use exec::Engine;
use std::sync::Arc;

pub use metadata::InfluxMetadataProvider;
pub use table::Table;

/// InfluxDB client: an HTTP endpoint with its credentials; nothing is held
/// open between statements.
pub struct InfluxClient {
    connection_info: ConnectionInfo,
    engine: Arc<Engine>,
    metadata_provider: InfluxMetadataProvider,
}

impl InfluxClient {
    /// Reach the server and prove the credentials work.
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let engine = Arc::new(Engine::new(&connection_info)?);
        engine.connect_check().await?;
        let metadata_provider = InfluxMetadataProvider::new(engine.clone());
        Ok(Self {
            connection_info,
            engine,
            metadata_provider,
        })
    }

    fn unsupported(&self, feature: impl Into<String>) -> DatabaseError {
        DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::InfluxDB,
            feature: feature.into(),
        }
    }

    /// Run one statement and report whether the row or byte budget cut the
    /// reply (the server is told to stop by dropping the connection).
    pub async fn execute_limited(&self, query: &str) -> Result<Table, DatabaseError> {
        let statement = query.trim().trim_end_matches(';').trim();
        if statement.is_empty() {
            return Ok(Table::default());
        }
        // Connections built for the credential broker refuse writes here too,
        // whatever the caller already checked
        if connection_read_only(&self.connection_info.options) {
            dbcrust_core::safety::check_influxdb_read_only(statement, self.engine.language)
                .map_err(|violation| DatabaseError::QueryError(violation.to_string()))?;
        }
        let table = self.engine.run_user(statement).await?;
        if table.truncated {
            tracing::warn!(
                "InfluxDB reply cut at {} rows / {} bytes (`max_rows` / `max_bytes` options; 0 = unlimited); bound the query instead",
                self.engine.max_rows,
                self.engine.max_bytes
            );
        }
        for notice in &table.notices {
            tracing::warn!("InfluxDB: {notice}");
        }
        Ok(table)
    }
}

#[async_trait]
impl DatabaseClient for InfluxClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Ok(self
            .execute_limited(sql)
            .await?
            .into_result()
            .into_display_rows("NULL"))
    }

    async fn execute_query_structured(
        &self,
        sql: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        Ok(self.execute_limited(sql).await?.into_result())
    }

    /// Validates the statement without running it: v3 asks the planner
    /// (`EXPLAIN`), InfluxQL and Flux have nothing cheaper than running.
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        let statement = sql.trim().trim_end_matches(';').trim();
        if statement.is_empty() {
            return Err(DatabaseError::QueryError(
                "the statement is empty".to_string(),
            ));
        }
        if self.engine.version == 3 && self.engine.language == InfluxLanguage::Sql {
            self.engine
                .run_user(&format!("EXPLAIN {statement}"))
                .await?;
        }
        Ok(())
    }

    /// v3 only: `EXPLAIN` returns the logical and physical plans as text.
    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        if self.engine.version != 3 || self.engine.language != InfluxLanguage::Sql {
            return Err(
                self.unsupported("EXPLAIN (only InfluxDB v3 SQL has a query planner to ask)")
            );
        }
        let statement = sql.trim().trim_end_matches(';').trim();
        Ok(self
            .execute_limited(&format!("EXPLAIN {statement}"))
            .await?
            .into_result()
            .into_display_rows("NULL"))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    /// The databases (v1, v3) or buckets (v2) the credentials can see.
    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        Ok(self
            .metadata_provider
            .databases_table()
            .await?
            .into_result()
            .into_display_rows("NULL"))
    }

    /// Switch to another database (v1, v3) or bucket (v2).
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        let wanted = database.trim();
        if wanted.is_empty() {
            return Err(DatabaseError::InvalidUrl(
                "the database name is empty".to_string(),
            ));
        }
        // Check the name when the credentials may list databases; a token that
        // cannot (a bucket-scoped v2 token) is taken at its word
        if let Ok(known) = self.metadata_provider.get_schemas().await
            && !known.is_empty()
            && !known.iter().any(|name| name == wanted)
        {
            return Err(DatabaseError::ConnectionError(format!(
                "InfluxDB has no database or bucket named '{wanted}'"
            )));
        }
        self.engine.set_database(wanted);
        self.connection_info.database = Some(wanted.to_string());
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.engine.database().unwrap_or_default()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        self.engine.ping().await
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // Requests are independent: nothing stays open
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let version = self.engine.server_version();
        let mut info = ServerInfo::new("InfluxDB".to_string(), version);
        info.parse_version_numbers();
        info.supports_transactions = false;
        info.supports_roles = false;
        info.additional_info
            .insert("api".to_string(), format!("v{}", self.engine.version));
        info.additional_info.insert(
            "language".to_string(),
            self.engine.language.as_str().to_string(),
        );
        if let Some(org) = &self.engine.org {
            info.additional_info.insert("org".to_string(), org.clone());
        }
        Ok(info)
    }
}
