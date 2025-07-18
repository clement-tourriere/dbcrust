/// PostgreSQL implementation of the database abstraction layer

use async_trait::async_trait;
use crate::database::{ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider};
use crate::db::TableDetails;
use crate::debug_log;
use crate::performance_analyzer::PerformanceAnalyzer;
use serde_json;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::{Row, Column};

/// PostgreSQL metadata provider implementation
pub struct PostgreSQLMetadataProvider {
    pool: PgPool,
}

impl PostgreSQLMetadataProvider {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MetadataProvider for PostgreSQLMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        debug_log!("[PostgreSQLMetadataProvider::get_schemas] Starting query");
        
        let rows = sqlx::query(
            r#"
            SELECT nspname as schema_name
            FROM pg_namespace
            WHERE nspname NOT LIKE 'pg_%' 
              AND nspname NOT IN ('information_schema', 'pg_toast')
            ORDER BY nspname
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let schemas: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>(0))
            .collect();

        debug_log!("[PostgreSQLMetadataProvider::get_schemas] Found {} schemas", schemas.len());
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug_log!("[PostgreSQLMetadataProvider::get_tables] Starting query for schema: {:?}", schema);

        let query = if let Some(schema_name) = schema {
            sqlx::query(
                r#"
                SELECT c.relname as table_name
                FROM pg_class c
                INNER JOIN pg_namespace n ON c.relnamespace = n.oid
                WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p')
                  AND n.nspname = $1
                ORDER BY c.relname
                "#,
            )
            .bind(schema_name)
        } else {
            sqlx::query(
                r#"
                SELECT c.relname as table_name
                FROM pg_class c
                INNER JOIN pg_namespace n ON c.relnamespace = n.oid
                WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p')
                  AND n.nspname NOT LIKE 'pg_%'
                  AND n.nspname NOT IN ('information_schema', 'pg_toast')
                ORDER BY n.nspname, c.relname
                "#,
            )
        };

        let rows = query.fetch_all(&self.pool).await?;
        let tables: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>(0))
            .collect();

        debug_log!("[PostgreSQLMetadataProvider::get_tables] Found {} tables", tables.len());
        Ok(tables)
    }

    async fn get_columns(&self, table: &str, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug_log!("[PostgreSQLMetadataProvider::get_columns] Starting query for table: '{}', schema: {:?}", table, schema);

        let schema_name = schema.unwrap_or("public");
        
        let rows = sqlx::query(
            r#"
            SELECT a.attname as column_name
            FROM pg_attribute a
            INNER JOIN pg_class c ON a.attrelid = c.oid
            INNER JOIN pg_namespace n ON c.relnamespace = n.oid
            WHERE n.nspname = $1 
              AND c.relname = $2
              AND a.attnum > 0
              AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(schema_name)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let columns: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>(0))
            .collect();

        debug_log!("[PostgreSQLMetadataProvider::get_columns] Found {} columns", columns.len());
        Ok(columns)
    }

    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug_log!("[PostgreSQLMetadataProvider::get_functions] Starting query for schema: {:?}", schema);

        let query = if let Some(schema_name) = schema {
            sqlx::query(
                r#"
                SELECT p.proname as routine_name
                FROM pg_proc p
                INNER JOIN pg_namespace n ON p.pronamespace = n.oid
                WHERE p.prokind = 'f'
                  AND n.nspname = $1
                ORDER BY p.proname
                "#,
            )
            .bind(schema_name)
        } else {
            sqlx::query(
                r#"
                SELECT p.proname as routine_name
                FROM pg_proc p
                INNER JOIN pg_namespace n ON p.pronamespace = n.oid
                WHERE p.prokind = 'f'
                  AND n.nspname NOT LIKE 'pg_%'
                  AND n.nspname NOT IN ('information_schema', 'pg_toast')
                ORDER BY n.nspname, p.proname
                "#,
            )
        };

        let rows = query.fetch_all(&self.pool).await?;
        let functions: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>(0))
            .collect();

        debug_log!("[PostgreSQLMetadataProvider::get_functions] Found {} functions", functions.len());
        Ok(functions)
    }

    async fn get_table_details(&self, _table: &str, _schema: Option<&str>) -> Result<TableDetails, DatabaseError> {
        // This would need to be implemented to call the existing get_table_details logic
        // For now, return an error indicating this needs implementation
        Err(DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::PostgreSQL,
            feature: "get_table_details not yet migrated".to_string(),
        })
    }

    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        Some("public".to_string())
    }
}

/// PostgreSQL database client implementation
pub struct PostgreSQLClient {
    pool: PgPool,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: PostgreSQLMetadataProvider,
}

impl PostgreSQLClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        // Build PostgreSQL connection options
        let mut connect_options = sqlx::postgres::PgConnectOptions::new();
        
        if let Some(ref host) = connection_info.host {
            connect_options = connect_options.host(host);
        }
        
        if let Some(port) = connection_info.port {
            connect_options = connect_options.port(port);
        } else if let Some(default_port) = connection_info.default_port() {
            connect_options = connect_options.port(default_port);
        }
        
        if let Some(ref username) = connection_info.username {
            connect_options = connect_options.username(username);
        }
        
        if let Some(ref password) = connection_info.password {
            connect_options = connect_options.password(password);
        }
        
        let database_name = connection_info.database.clone().unwrap_or_else(|| "postgres".to_string());
        connect_options = connect_options.database(&database_name);

        // Handle SSL mode from options
        if let Some(sslmode) = connection_info.options.get("sslmode") {
            let ssl_mode = match sslmode.as_str() {
                "disable" => sqlx::postgres::PgSslMode::Disable,
                "allow" => sqlx::postgres::PgSslMode::Allow,
                "prefer" => sqlx::postgres::PgSslMode::Prefer,
                "require" => sqlx::postgres::PgSslMode::Require,
                "verify-ca" => sqlx::postgres::PgSslMode::VerifyCa,
                "verify-full" => sqlx::postgres::PgSslMode::VerifyFull,
                _ => sqlx::postgres::PgSslMode::Prefer, // Default
            };
            connect_options = connect_options.ssl_mode(ssl_mode);
        }

        // Configure connection pool
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .min_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .idle_timeout(std::time::Duration::from_secs(300))
            .connect_with(connect_options)
            .await
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;

        let metadata_provider = PostgreSQLMetadataProvider::new(pool.clone());

        Ok(Self {
            pool,
            connection_info,
            current_database: database_name,
            metadata_provider,
        })
    }

    /// Format PostgreSQL EXPLAIN JSON output for better readability
    async fn format_explain_output(&self, raw_results: Vec<Vec<String>>) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug_log!("[PostgreSQLClient::format_explain_output] Formatting PostgreSQL EXPLAIN output");
        
        if raw_results.is_empty() {
            return Ok(vec![vec!["No query plan available".to_string()]]);
        }
        
        let mut formatted_results = Vec::new();
        formatted_results.push(vec!["PostgreSQL Query Plan".to_string()]);
        formatted_results.push(vec!["".to_string()]);
        
        // Process each row (usually just one row for JSON format)
        // Skip the first row which contains column headers
        for (i, row) in raw_results.iter().enumerate() {
            if i == 0 {
                // Skip header row
                continue;
            }
            
            let json_str = &row[0];
            debug_log!("[PostgreSQLClient::format_explain_output] Attempting to parse JSON: {}", json_str);
            
            // Parse JSON
            match serde_json::from_str::<serde_json::Value>(json_str) {
                Ok(json) => {
                    // Use performance analyzer to get metrics
                    let performance_metrics = PerformanceAnalyzer::analyze_postgresql_plan(&json);
                    
                    // Add performance summary header
                    let performance_summary = PerformanceAnalyzer::format_metrics_with_colors(&performance_metrics);
                    for line in performance_summary {
                        formatted_results.push(vec![line]);
                    }
                    
                    formatted_results.push(vec!["".to_string()]);
                    formatted_results.push(vec!["💡 Use \\copy to copy the raw JSON plan to clipboard".to_string()]);
                },
                Err(e) => {
                    debug_log!("[PostgreSQLClient::format_explain_output] JSON parse error: {}", e);
                    formatted_results.push(vec![format!("JSON Parse Error: {}", e)]);
                    formatted_results.push(vec![json_str.clone()]);
                }
            }
        }
        
        if formatted_results.len() <= 2 {
            formatted_results.push(vec!["No query plan information available".to_string()]);
        }
        
        Ok(formatted_results)
    }
}

#[async_trait]
impl DatabaseClient for PostgreSQLClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug_log!("[PostgreSQLClient::execute_query] Executing query");

        let rows = sqlx::query(sql)
            .fetch_all(&self.pool)
            .await?;

        if rows.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        
        // Get column names from the first row
        let first_row = &rows[0];
        let column_names: Vec<String> = (0..first_row.len())
            .map(|i| {
                first_row.column(i).name().to_string()
            })
            .collect();
        
        results.push(column_names);

        // Convert rows to strings
        for row in rows {
            let mut string_row = Vec::new();
            for i in 0..row.len() {
                let value = format_postgresql_value(&row, i)?;
                string_row.push(value);
            }
            results.push(string_row);
        }

        debug_log!("[PostgreSQLClient::execute_query] Query completed with {} rows", results.len() - 1);
        Ok(results)
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {}", sql);
        let raw_results = self.execute_query(&explain_sql).await?;
        self.format_explain_output(raw_results).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {}", sql);
        self.execute_query(&explain_sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let query = r#"
            SELECT 
                d.datname AS "Name",
                pg_get_userbyid(d.datdba) AS "Owner",
                pg_encoding_to_char(d.encoding) AS "Encoding",
                CASE WHEN d.datcollate = d.datctype THEN d.datcollate ELSE d.datcollate || '/' || d.datctype END AS "Collate",
                pg_size_pretty(pg_database_size(d.datname)) AS "Size"
            FROM 
                pg_database d
            WHERE 
                d.datistemplate = false
            ORDER BY 
                d.datname
        "#;

        self.execute_query(query).await
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        // Create new connection info with updated database
        let mut new_connection_info = self.connection_info.clone();
        new_connection_info.database = Some(database.to_string());

        // Create new client with the updated connection
        let new_client = PostgreSQLClient::new(new_connection_info).await?;
        
        // Replace current connection
        *self = new_client;
        
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
        // Try a simple query to check if connection is still alive
        match sqlx::query("SELECT 1").fetch_one(&self.pool).await {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        self.pool.close().await;
        Ok(())
    }
}

/// Format a PostgreSQL value to string representation
fn format_postgresql_value(row: &PgRow, column_index: usize) -> Result<String, DatabaseError> {
    use sqlx::TypeInfo;
    
    let column = row.column(column_index);
    let type_name = column.type_info().name();
    
    // Handle NULL values
    if let Ok(value) = row.try_get::<Option<String>, _>(column_index) {
        if value.is_none() {
            return Ok("".to_string());
        }
    }

    // Match on PostgreSQL type names and convert appropriately
    let result = match type_name {
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" => {
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "INT4" | "INTEGER" => {
            row.try_get::<i32, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "INT8" | "BIGINT" => {
            row.try_get::<i64, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "FLOAT4" | "REAL" => {
            row.try_get::<f32, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "FLOAT8" | "DOUBLE PRECISION" => {
            row.try_get::<f64, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "BOOL" | "BOOLEAN" => {
            row.try_get::<bool, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "TIMESTAMPTZ" => {
            row.try_get::<chrono::DateTime<chrono::Utc>, _>(column_index)
                .map(|v| v.to_rfc3339())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "TIMESTAMP" => {
            row.try_get::<chrono::NaiveDateTime, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "DATE" => {
            row.try_get::<chrono::NaiveDate, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "JSON" | "JSONB" => {
            row.try_get::<serde_json::Value, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "UUID" => {
            row.try_get::<sqlx::types::Uuid, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        _ => {
            // For unknown types, try to get as string
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_format_explain_output() {
        // Create a mock PostgreSQLClient for testing
        // Note: This test doesn't require a real database connection
        let raw_results = vec![
            vec!["QUERY PLAN".to_string()],  // Header row
            vec![r#"[{"Plan": {"Node Type": "Seq Scan", "Relation Name": "test_table", "Alias": "test_table", "Startup Cost": 0.00, "Total Cost": 10.00, "Plan Rows": 100, "Plan Width": 32}}]"#.to_string()],
        ];
        
        // We can't easily test the full format_explain_output without a real client,
        // but we can test the JSON parsing logic
        let json_str = &raw_results[1][0];
        let json_result = serde_json::from_str::<serde_json::Value>(json_str);
        
        assert!(json_result.is_ok(), "Should successfully parse EXPLAIN JSON output");
        
        // Test that trying to parse the header row would fail
        let header_str = &raw_results[0][0];
        let header_result = serde_json::from_str::<serde_json::Value>(header_str);
        
        assert!(header_result.is_err(), "Should fail to parse header row as JSON");
    }

    #[tokio::test]
    async fn test_postgresql_client_creation() {
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("postgres".to_string()),
            password: Some("test".to_string()),
            database: Some("postgres".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // This test will fail if no PostgreSQL server is running, which is expected
        // In a real test environment, we'd use a test database or mock
        match PostgreSQLClient::new(connection_info).await {
            Ok(_) => {
                // Connection successful - this would happen in integration tests
            }
            Err(DatabaseError::ConnectionError(_)) => {
                // Expected when no test database is available
            }
            Err(e) => {
                panic!("Unexpected error: {:?}", e);
            }
        }
    }
}