//! PostgreSQL implementation of the database abstraction layer
use async_trait::async_trait;
use crate::database::{ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider};
use crate::db::TableDetails;
use tracing::debug;
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

    /// Get detailed column information including data types, nullability, and defaults
    async fn get_detailed_columns(&self, table: &str, schema: Option<&str>) -> Result<Vec<crate::db::ColumnInfo>, DatabaseError> {
        let schema_name = schema.unwrap_or("public");
        
        let rows = sqlx::query(
            r#"
            SELECT 
                a.attname as column_name,
                format_type(a.atttypid, a.atttypmod) as data_type,
                COALESCE(c.collname, '') as collation,
                NOT a.attnotnull as nullable,
                pg_get_expr(d.adbin, d.adrelid) as default_value
            FROM pg_attribute a
            INNER JOIN pg_class t ON a.attrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            LEFT JOIN pg_attrdef d ON a.attrelid = d.adrelid AND a.attnum = d.adnum
            LEFT JOIN pg_collation c ON a.attcollation = c.oid AND a.attcollation <> 0
            WHERE n.nspname = $1 
              AND t.relname = $2
              AND a.attnum > 0
              AND NOT a.attisdropped
            ORDER BY a.attnum
            "#,
        )
        .bind(schema_name)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let columns: Vec<crate::db::ColumnInfo> = rows
            .iter()
            .map(|row| {
                crate::db::ColumnInfo {
                    name: row.get::<String, _>("column_name"),
                    data_type: row.get::<String, _>("data_type"),
                    collation: row.get::<String, _>("collation"),
                    nullable: row.get::<bool, _>("nullable"),
                    default_value: row.get::<Option<String>, _>("default_value"),
                }
            })
            .collect();

        Ok(columns)
    }

    /// Get index information for a table
    async fn get_table_indexes(&self, table: &str, schema: &str) -> Result<Vec<crate::db::IndexInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                i.relname as index_name,
                CASE 
                    WHEN ix.indisunique AND ix.indisprimary THEN 'PRIMARY KEY'
                    WHEN ix.indisunique THEN 'UNIQUE'
                    ELSE 'INDEX'
                END as index_type,
                ix.indisprimary as is_primary,
                ix.indisunique as is_unique,
                pg_get_expr(ix.indpred, ix.indrelid) as predicate,
                pg_get_indexdef(ix.indexrelid) as definition
            FROM pg_index ix
            INNER JOIN pg_class i ON i.oid = ix.indexrelid
            INNER JOIN pg_class t ON t.oid = ix.indrelid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            WHERE n.nspname = $1 AND t.relname = $2
            ORDER BY ix.indisprimary DESC, ix.indisunique DESC, i.relname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let indexes: Vec<crate::db::IndexInfo> = rows
            .iter()
            .map(|row| {
                crate::db::IndexInfo {
                    name: row.get::<String, _>("index_name"),
                    index_type: row.get::<String, _>("index_type"),
                    is_primary: row.get::<bool, _>("is_primary"),
                    is_unique: row.get::<bool, _>("is_unique"),
                    predicate: row.get::<Option<String>, _>("predicate"),
                    definition: row.get::<String, _>("definition"),
                    constraint_def: None,
                }
            })
            .collect();

        Ok(indexes)
    }

    /// Get foreign key constraints for a table
    async fn get_table_foreign_keys(&self, table: &str, schema: &str) -> Result<Vec<crate::db::ForeignKeyInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                c.conname as constraint_name,
                pg_get_constraintdef(c.oid) as definition
            FROM pg_constraint c
            INNER JOIN pg_class t ON c.conrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            WHERE n.nspname = $1 
              AND t.relname = $2 
              AND c.contype = 'f'
            ORDER BY c.conname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let foreign_keys: Vec<crate::db::ForeignKeyInfo> = rows
            .iter()
            .map(|row| {
                crate::db::ForeignKeyInfo {
                    name: row.get::<String, _>("constraint_name"),
                    definition: row.get::<String, _>("definition"),
                }
            })
            .collect();

        Ok(foreign_keys)
    }

    /// Get check constraints for a table
    async fn get_table_check_constraints(&self, table: &str, schema: &str) -> Result<Vec<crate::db::CheckConstraintInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                c.conname as constraint_name,
                pg_get_constraintdef(c.oid) as definition
            FROM pg_constraint c
            INNER JOIN pg_class t ON c.conrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            WHERE n.nspname = $1 
              AND t.relname = $2 
              AND c.contype = 'c'
            ORDER BY c.conname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let check_constraints: Vec<crate::db::CheckConstraintInfo> = rows
            .iter()
            .map(|row| {
                crate::db::CheckConstraintInfo {
                    name: row.get::<String, _>("constraint_name"),
                    definition: row.get::<String, _>("definition"),
                }
            })
            .collect();

        Ok(check_constraints)
    }

    /// Get tables that reference this table (reverse foreign keys)
    async fn get_table_referenced_by(&self, table: &str, schema: &str) -> Result<Vec<crate::db::ReferencedByInfo>, DatabaseError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                n.nspname as referencing_schema,
                t.relname as referencing_table,
                c.conname as constraint_name,
                pg_get_constraintdef(c.oid) as definition
            FROM pg_constraint c
            INNER JOIN pg_class t ON c.conrelid = t.oid
            INNER JOIN pg_namespace n ON t.relnamespace = n.oid
            INNER JOIN pg_class ref_t ON c.confrelid = ref_t.oid
            INNER JOIN pg_namespace ref_n ON ref_t.relnamespace = ref_n.oid
            WHERE ref_n.nspname = $1 
              AND ref_t.relname = $2 
              AND c.contype = 'f'
            ORDER BY n.nspname, t.relname, c.conname
            "#,
        )
        .bind(schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await?;

        let referenced_by: Vec<crate::db::ReferencedByInfo> = rows
            .iter()
            .map(|row| {
                crate::db::ReferencedByInfo {
                    schema: row.get::<String, _>("referencing_schema"),
                    table: row.get::<String, _>("referencing_table"),
                    constraint_name: row.get::<String, _>("constraint_name"),
                    definition: row.get::<String, _>("definition"),
                }
            })
            .collect();

        Ok(referenced_by)
    }
}

#[async_trait]
impl MetadataProvider for PostgreSQLMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        debug!("[PostgreSQLMetadataProvider::get_schemas] Starting query");
        
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

        debug!("[PostgreSQLMetadataProvider::get_schemas] Found {} schemas", schemas.len());
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[PostgreSQLMetadataProvider::get_tables] Starting query for schema: {:?}", schema);

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

        debug!("[PostgreSQLMetadataProvider::get_tables] Found {} tables", tables.len());
        Ok(tables)
    }

    async fn get_columns(&self, table: &str, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[PostgreSQLMetadataProvider::get_columns] Starting query for table: '{}', schema: {:?}", table, schema);

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

        debug!("[PostgreSQLMetadataProvider::get_columns] Found {} columns", columns.len());
        Ok(columns)
    }

    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[PostgreSQLMetadataProvider::get_functions] Starting query for schema: {:?}", schema);

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

        debug!("[PostgreSQLMetadataProvider::get_functions] Found {} functions", functions.len());
        Ok(functions)
    }

    async fn get_table_details(&self, table: &str, schema: Option<&str>) -> Result<TableDetails, DatabaseError> {
        debug!("[PostgreSQLMetadataProvider::get_table_details] Starting query for table: '{}', schema: {:?}", table, schema);

        let schema_name = schema.unwrap_or("public");

        // Get basic table information and columns
        let columns = self.get_detailed_columns(table, Some(schema_name)).await?;
        
        // Get indexes
        let indexes = self.get_table_indexes(table, schema_name).await?;
        
        // Get foreign keys
        let foreign_keys = self.get_table_foreign_keys(table, schema_name).await?;
        
        // Get check constraints
        let check_constraints = self.get_table_check_constraints(table, schema_name).await?;
        
        // Get referenced by information
        let referenced_by = self.get_table_referenced_by(table, schema_name).await?;

        let table_details = TableDetails {
            name: table.to_string(),
            schema: schema_name.to_string(),
            full_name: format!("{}.{}", schema_name, table),
            columns,
            indexes,
            check_constraints,
            foreign_keys,
            referenced_by,
        };

        debug!("[PostgreSQLMetadataProvider::get_table_details] Successfully fetched details for table: '{}'", table);
        Ok(table_details)
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

        // Configure connection pool - don't connect yet for SSH tunnel scenarios
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .min_connections(0)  // Don't pre-connect - wait for SSH tunnel
            .acquire_timeout(std::time::Duration::from_secs(15)) // Allow time for SSH tunnel establishment
            .idle_timeout(std::time::Duration::from_secs(300))
            .test_before_acquire(false)  // Skip connection tests
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
        debug!("[PostgreSQLClient::format_explain_output] Formatting PostgreSQL EXPLAIN output");
        
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
            debug!("[PostgreSQLClient::format_explain_output] Attempting to parse JSON: {}", json_str);
            
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
                    formatted_results.push(vec!["💡 Use \\ecopy to copy the raw JSON plan to clipboard".to_string()]);
                },
                Err(e) => {
                    debug!("[PostgreSQLClient::format_explain_output] JSON parse error: {}", e);
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
        debug!("[PostgreSQLClient::execute_query] Executing query");

        // Add timeout to prevent hanging queries
        let timeout_duration = std::time::Duration::from_secs(30); // 30 seconds timeout
        let rows = match tokio::time::timeout(
            timeout_duration,
            sqlx::query(sql).fetch_all(&self.pool)
        ).await {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => return Err(DatabaseError::QueryError(e.to_string())),
            Err(_) => return Err(DatabaseError::QueryError("Query timed out after 30 seconds".to_string())),
        };

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

        debug!("[PostgreSQLClient::execute_query] Query completed with {} rows", results.len() - 1);
        Ok(results)
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        debug!("[PostgreSQLClient::test_query] Testing query for validation");
        // For PostgreSQL, we can use EXPLAIN to validate query syntax without executing it
        let explain_sql = format!("EXPLAIN {}", sql);
        let timeout_duration = std::time::Duration::from_secs(10); // Shorter timeout for tests
        
        match tokio::time::timeout(
            timeout_duration,
            sqlx::query(&explain_sql).fetch_all(&self.pool)
        ).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(DatabaseError::QueryError(format!("Query validation failed: {}", e))),
            Err(_) => Err(DatabaseError::QueryError("Query validation timed out".to_string())),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {sql}");
        let raw_results = self.execute_query(&explain_sql).await?;
        self.format_explain_output(raw_results).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN (FORMAT JSON) {sql}");
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
        (sqlx::query("SELECT 1").fetch_one(&self.pool).await).is_ok()
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
    
    // Handle NULL values first - try the most generic nullable type
    if let Ok(value) = row.try_get::<Option<String>, _>(column_index) {
        if value.is_none() {
            return Ok("".to_string());
        }
    }

    // Match on PostgreSQL type names and convert appropriately
    match type_name {
        // String types
        "TEXT" | "VARCHAR" | "CHAR" | "BPCHAR" | "NAME" | "CITEXT" => {
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Integer types
        "INT2" | "SMALLINT" => {
            row.try_get::<i16, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "INT4" | "INTEGER" | "SERIAL" => {
            row.try_get::<i32, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "INT8" | "BIGINT" | "BIGSERIAL" => {
            row.try_get::<i64, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "OID" => {
            row.try_get::<i32, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Floating point types
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
        "NUMERIC" | "DECIMAL" => {
            row.try_get::<sqlx::types::Decimal, _>(column_index)
                .map(|v| v.to_string())
                .or_else(|_| {
                    // Fallback for numeric values that can't be represented as Decimal
                    row.try_get::<String, _>(column_index)
                })
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Boolean type
        "BOOL" | "BOOLEAN" => {
            row.try_get::<bool, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Date and time types
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
        "TIME" => {
            row.try_get::<chrono::NaiveTime, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "TIMETZ" => {
            // PostgreSQL TIMETZ - for now treat as string since chrono doesn't have a direct equivalent
            row.try_get::<String, _>(column_index)
                .or_else(|_| {
                    // If string doesn't work, try as time and convert
                    row.try_get::<chrono::NaiveTime, _>(column_index)
                        .map(|v| v.to_string())
                })
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "INTERVAL" => {
            // PostgreSQL intervals - SQLx doesn't have built-in support, try as string
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // JSON types
        "JSON" | "JSONB" => {
            row.try_get::<serde_json::Value, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // UUID type
        "UUID" => {
            row.try_get::<sqlx::types::Uuid, _>(column_index)
                .map(|v| v.to_string())
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Binary data types
        "BYTEA" => {
            row.try_get::<Vec<u8>, _>(column_index)
                .map(|v| format!("\\x{}", hex::encode(v)))
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Network address types
        "INET" | "CIDR" => {
            row.try_get::<std::net::IpAddr, _>(column_index)
                .map(|v| v.to_string())
                .or_else(|_| {
                    // Fallback to string if IP parsing fails
                    row.try_get::<String, _>(column_index)
                })
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        "MACADDR" | "MACADDR8" => {
            // Try MAC address type if available, otherwise fallback to string
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Array types - handle common array types
        t if t.ends_with("[]") => {
            // For arrays, try to get as JSON first, then fallback to string
            match row.try_get::<serde_json::Value, _>(column_index) {
                Ok(json_val) => Ok(json_val.to_string()),
                Err(_) => {
                    // Fallback to string representation
                    row.try_get::<String, _>(column_index)
                        .map_err(|e| DatabaseError::QueryError(e.to_string()))
                }
            }
        }
        
        // Geometric types - these are complex, try as string
        "POINT" | "LINE" | "LSEG" | "BOX" | "PATH" | "POLYGON" | "CIRCLE" => {
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Range types
        "INT4RANGE" | "INT8RANGE" | "NUMRANGE" | "TSRANGE" | "TSTZRANGE" | "DATERANGE" => {
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // XML type
        "XML" => {
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Bit string types
        "BIT" | "VARBIT" => {
            // Bit strings as string representation
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Money type
        "MONEY" => {
            // Money as string representation
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(e.to_string()))
        }
        
        // Custom/composite types and unknown types - try as string
        _ => {
            row.try_get::<String, _>(column_index)
                .map_err(|e| DatabaseError::QueryError(format!("Unable to format PostgreSQL type '{}': {}", type_name, e)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;
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
                panic!("Unexpected error: {e:?}");
            }
        }
    }
}