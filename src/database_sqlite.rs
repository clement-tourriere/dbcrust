//! SQLite implementation of the database abstraction layer
use crate::database::{ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider};
use crate::db::TableDetails;
use crate::performance_analyzer::PerformanceAnalyzer;
use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{Column, Row};
use tracing::debug;

/// SQLite metadata provider implementation
pub struct SqliteMetadataProvider {
    pool: SqlitePool,
}

impl SqliteMetadataProvider {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MetadataProvider for SqliteMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        debug!("[SqliteMetadataProvider::get_schemas] Starting query");

        // SQLite doesn't have traditional schemas like PostgreSQL, but it supports attached databases
        // We'll list the main database and any attached databases
        let rows = sqlx::query(
            r#"
            PRAGMA database_list
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let schemas: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

        debug!(
            "[SqliteMetadataProvider::get_schemas] Found {} schemas",
            schemas.len()
        );
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[SqliteMetadataProvider::get_tables] Starting query for schema: {:?}",
            schema
        );

        let schema_name = schema.unwrap_or("main");

        // Query sqlite_master for tables and views
        let query = format!(
            r#"
            SELECT name as table_name
            FROM {schema_name}.sqlite_master
            WHERE type IN ('table', 'view')
              AND name NOT LIKE 'sqlite_%'
            ORDER BY name
            "#
        );

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;
        let tables: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>("table_name"))
            .collect();

        debug!(
            "[SqliteMetadataProvider::get_tables] Found {} tables",
            tables.len()
        );
        Ok(tables)
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[SqliteMetadataProvider::get_columns] Starting query for table: '{}', schema: {:?}",
            table, schema
        );

        let schema_name = schema.unwrap_or("main");

        // Use PRAGMA table_info to get column information
        let query = format!("PRAGMA {schema_name}.table_info({table})");

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;

        let columns: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

        debug!(
            "[SqliteMetadataProvider::get_columns] Found {} columns",
            columns.len()
        );
        Ok(columns)
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[SqliteMetadataProvider::get_functions] Starting query");

        // SQLite has built-in functions but no user-defined function discovery like PostgreSQL
        // Return a list of common SQLite built-in functions
        let functions = vec![
            "abs".to_string(),
            "avg".to_string(),
            "coalesce".to_string(),
            "count".to_string(),
            "date".to_string(),
            "datetime".to_string(),
            "group_concat".to_string(),
            "ifnull".to_string(),
            "instr".to_string(),
            "json_extract".to_string(),
            "json_valid".to_string(),
            "length".to_string(),
            "lower".to_string(),
            "ltrim".to_string(),
            "max".to_string(),
            "min".to_string(),
            "nullif".to_string(),
            "printf".to_string(),
            "random".to_string(),
            "replace".to_string(),
            "round".to_string(),
            "rtrim".to_string(),
            "strftime".to_string(),
            "substr".to_string(),
            "sum".to_string(),
            "time".to_string(),
            "trim".to_string(),
            "typeof".to_string(),
            "upper".to_string(),
        ];

        debug!(
            "[SqliteMetadataProvider::get_functions] Found {} functions",
            functions.len()
        );
        Ok(functions)
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        debug!(
            "[SqliteMetadataProvider::get_table_details] Getting details for table: {}",
            table
        );

        let schema_name = schema.unwrap_or("main");

        // First check if the table exists
        let table_exists_query =
            format!("SELECT name FROM {schema_name}.sqlite_master WHERE type='table' AND name=?");
        let table_exists = sqlx::query(&table_exists_query)
            .bind(table)
            .fetch_optional(&self.pool)
            .await?;

        if table_exists.is_none() {
            return Err(DatabaseError::QueryError(format!(
                "Table '{table}' does not exist"
            )));
        }

        // Get basic table info using PRAGMA table_info
        let query = format!("PRAGMA {schema_name}.table_info({table})");
        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;

        let mut columns = Vec::new();
        for row in rows {
            let column = crate::db::ColumnInfo {
                name: row.get::<String, _>("name"),
                data_type: row.get::<String, _>("type"),
                nullable: row.get::<i32, _>("notnull") == 0,
                default_value: row
                    .try_get::<Option<String>, _>("dflt_value")
                    .unwrap_or(None),
                collation: String::new(), // SQLite doesn't expose collation info easily
                enum_values: None,        // SQLite doesn't have native enum support
            };
            columns.push(column);
        }

        // Get index information
        let index_query = format!("PRAGMA {schema_name}.index_list({table})");
        let index_rows = sqlx::query(&index_query).fetch_all(&self.pool).await?;

        let mut indexes = Vec::new();
        for index_row in index_rows {
            let index_name: String = index_row.get("name");
            let is_unique: bool = index_row.get("unique");

            // Get index details
            let detail_query = format!("PRAGMA {schema_name}.index_info({index_name})");
            let detail_rows = sqlx::query(&detail_query).fetch_all(&self.pool).await?;

            let mut columns_in_index = Vec::new();
            for detail_row in detail_rows {
                if let Ok(col_name) = detail_row.try_get::<String, _>("name") {
                    columns_in_index.push(col_name);
                }
            }

            let index_info = crate::db::IndexInfo {
                name: index_name.clone(),
                definition: format!("({})", columns_in_index.join(", ")),
                index_type: "btree".to_string(), // SQLite primarily uses B-tree indexes
                is_primary: index_name.contains("primary") || index_name.contains("pk"),
                is_unique,
                predicate: None, // Would need to parse CREATE INDEX statements to get predicates
                constraint_def: None,
            };
            indexes.push(index_info);
        }

        // Get foreign key information
        let fk_query = format!("PRAGMA {schema_name}.foreign_key_list({table})");
        let fk_rows = sqlx::query(&fk_query).fetch_all(&self.pool).await?;

        let mut foreign_keys = Vec::new();
        for fk_row in fk_rows {
            let from_col: String = fk_row.get("from");
            let to_table: String = fk_row.get("table");
            let to_col: String = fk_row.get("to");

            let fk_info = crate::db::ForeignKeyInfo {
                name: format!("fk_{table}_{from_col}"),
                definition: format!("FOREIGN KEY ({from_col}) REFERENCES {to_table}({to_col})"),
            };
            foreign_keys.push(fk_info);
        }

        let table_details = TableDetails {
            schema: schema_name.to_string(),
            name: table.to_string(),
            full_name: format!("{schema_name}.{table}"),
            columns,
            indexes,
            check_constraints: Vec::new(), // SQLite check constraints are harder to query
            foreign_keys,
            referenced_by: Vec::new(), // Would need complex query to find referencing tables
            nested_field_details: std::collections::HashMap::new(),
        };

        debug!("[SqliteMetadataProvider::get_table_details] Table details retrieved successfully");
        Ok(table_details)
    }

    fn supports_explain(&self) -> bool {
        true // SQLite supports EXPLAIN QUERY PLAN
    }

    fn default_schema(&self) -> Option<String> {
        Some("main".to_string())
    }
}

/// SQLite database client implementation
pub struct SqliteClient {
    pool: SqlitePool,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: SqliteMetadataProvider,
}

impl SqliteClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        debug!("[SqliteClient::new] Creating SQLite client");

        // Get the database file path
        let file_path = connection_info.file_path.as_ref().ok_or_else(|| {
            DatabaseError::ConnectionError(
                "No file path provided for SQLite connection".to_string(),
            )
        })?;

        // Ensure the file path is properly formatted for SQLite URL
        let database_url = if file_path.starts_with("sqlite://") {
            file_path.clone()
        } else {
            // Resolve the path properly

            if file_path.starts_with('/') && std::path::Path::new(file_path).exists() {
                // Absolute Unix path that exists
                format!("sqlite://{file_path}")
            } else if file_path.starts_with('/') {
                // Path from URL like /test_data/test_sample.db - treat as relative
                let relative_path = file_path.trim_start_matches('/');
                let current_dir = std::env::current_dir().map_err(|e| {
                    DatabaseError::ConnectionError(format!("Could not get current directory: {e}"))
                })?;
                let full_path = current_dir.join(relative_path);
                format!("sqlite://{}", full_path.to_string_lossy())
            } else {
                // Relative path or Windows path
                let current_dir = std::env::current_dir().map_err(|e| {
                    DatabaseError::ConnectionError(format!("Could not get current directory: {e}"))
                })?;
                let full_path = current_dir.join(file_path);
                format!("sqlite://{}", full_path.to_string_lossy())
            }
        };

        debug!("[SqliteClient::new] Connecting to: {}", database_url);

        // Configure connection pool with SQLite-specific optimizations
        let pool = SqlitePoolOptions::new()
            .max_connections(5) // SQLite doesn't need as many connections as network databases
            .min_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .idle_timeout(std::time::Duration::from_secs(600)) // Keep connections alive longer
            .connect(&database_url)
            .await
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;

        // Apply SQLite-specific optimizations
        Self::apply_sqlite_optimizations(&pool).await?;

        let metadata_provider = SqliteMetadataProvider::new(pool.clone());

        // Extract database name from file path for display purposes
        let current_database = std::path::Path::new(file_path)
            .file_stem()
            .and_then(|os_str| os_str.to_str())
            .unwrap_or("main")
            .to_string();

        Ok(Self {
            pool,
            connection_info,
            current_database,
            metadata_provider,
        })
    }

    /// Apply SQLite-specific performance optimizations
    async fn apply_sqlite_optimizations(pool: &SqlitePool) -> Result<(), DatabaseError> {
        debug!("[SqliteClient] Applying SQLite optimizations");

        // Enable WAL mode for better concurrency
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(pool)
            .await?;

        // Set synchronous mode for better performance while maintaining safety
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(pool)
            .await?;

        // Enable foreign key constraints
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(pool)
            .await?;

        // Use memory for temporary storage
        sqlx::query("PRAGMA temp_store = MEMORY")
            .execute(pool)
            .await?;

        // Set memory map size for better I/O performance
        sqlx::query("PRAGMA mmap_size = 268435456") // 256MB
            .execute(pool)
            .await?;

        // Set cache size (number of pages to keep in memory)
        sqlx::query("PRAGMA cache_size = 10000") // ~40MB with 4KB pages
            .execute(pool)
            .await?;

        debug!("[SqliteClient] SQLite optimizations applied successfully");
        Ok(())
    }

    /// Format SQLite EXPLAIN QUERY PLAN output for better readability
    async fn format_explain_output(
        &self,
        raw_results: Vec<Vec<String>>,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[SqliteClient::format_explain_output] Formatting EXPLAIN QUERY PLAN output");

        if raw_results.is_empty() {
            return Ok(vec![vec!["No query plan available".to_string()]]);
        }

        let mut formatted_results = Vec::new();
        formatted_results.push(vec!["SQLite Query Plan".to_string()]);
        formatted_results.push(vec!["".to_string()]);

        // Skip header row if present
        let data_rows = if raw_results.len() > 1
            && raw_results[0].iter().any(|col| {
                col.to_lowercase().contains("id") || col.to_lowercase().contains("detail")
            }) {
            &raw_results[1..]
        } else {
            &raw_results[..]
        };

        if data_rows.is_empty() {
            formatted_results.push(vec!["No execution plan steps found".to_string()]);
            return Ok(formatted_results);
        }

        // Use performance analyzer to get metrics
        let performance_metrics = PerformanceAnalyzer::analyze_sqlite_plan(data_rows);

        // Add performance summary header
        let performance_summary =
            PerformanceAnalyzer::format_metrics_with_colors(&performance_metrics);
        for line in performance_summary {
            formatted_results.push(vec![line]);
        }

        formatted_results.push(vec!["".to_string()]);
        formatted_results.push(vec!["Detailed Plan Steps:".to_string()]);
        formatted_results.push(vec!["".to_string()]);

        // Parse and format each row with enhanced performance information
        for (step_num, row) in data_rows.iter().enumerate() {
            if row.len() >= 4 {
                let id = &row[0];
                let parent = &row[1];
                let _notused = &row[2];
                let detail = &row[3];

                // Determine indentation based on parent relationships
                let indent = self.calculate_indent(id, parent, data_rows);
                let indent_str = "  ".repeat(indent);

                // Format the step with better structure
                formatted_results.push(vec![format!(
                    "{}Step {}: {}",
                    indent_str,
                    step_num + 1,
                    detail
                )]);

                // Add analysis of the operation with performance insights
                self.analyze_operation_detail(detail, &mut formatted_results, indent + 1);
            } else if row.len() == 1 {
                // Single column output (simplified format)
                formatted_results.push(vec![format!("  {}", row[0])]);
            }
        }

        // Add legacy summary information for backward compatibility
        formatted_results.push(vec!["".to_string()]);
        formatted_results.push(vec!["Query Plan Summary:".to_string()]);
        formatted_results.push(vec![format!("  Total Steps: {}", data_rows.len())]);

        // Count different operation types
        let mut scan_count = 0;
        let mut index_count = 0;
        let mut join_count = 0;

        for row in data_rows {
            if row.len() >= 4 {
                let detail = &row[3].to_lowercase();
                if detail.contains("scan") && !detail.contains("index") {
                    scan_count += 1;
                } else if detail.contains("index") {
                    index_count += 1;
                }
                if detail.contains("join") || detail.contains("nested loop") {
                    join_count += 1;
                }
            }
        }

        if scan_count > 0 {
            formatted_results.push(vec![format!("  Table Scans: {}", scan_count)]);
        }
        if index_count > 0 {
            formatted_results.push(vec![format!("  Index Operations: {}", index_count)]);
        }
        if join_count > 0 {
            formatted_results.push(vec![format!("  Join Operations: {}", join_count)]);
        }

        Ok(formatted_results)
    }

    /// Calculate indentation level based on parent-child relationships
    fn calculate_indent(&self, _id: &str, parent: &str, all_rows: &[Vec<String>]) -> usize {
        // Simple indentation based on parent ID
        if parent == "0" || parent.is_empty() {
            0
        } else {
            // Find parent's indentation and add 1
            for row in all_rows {
                if row.len() >= 4 && row[0] == *parent {
                    return self.calculate_indent(&row[0], &row[1], all_rows) + 1;
                }
            }
            1 // Default to 1 level if parent not found
        }
    }

    /// Analyze operation detail and add explanatory information
    fn analyze_operation_detail(
        &self,
        detail: &str,
        results: &mut Vec<Vec<String>>,
        indent: usize,
    ) {
        let indent_str = "  ".repeat(indent);
        let detail_lower = detail.to_lowercase();

        // Analyze different types of operations
        if detail_lower.contains("scan table") {
            if detail_lower.contains("using index") {
                if let Some(index_start) = detail.find("using index") {
                    let index_part = &detail[index_start..];
                    results.push(vec![format!("{}→ Index-assisted table scan", indent_str)]);
                    results.push(vec![format!("{}→ {}", indent_str, index_part)]);
                }
            } else {
                results.push(vec![format!("{}→ Full table scan (no index)", indent_str)]);
                results.push(vec![format!(
                    "{}→ Performance note: Consider adding an index",
                    indent_str
                )]);
            }
        } else if detail_lower.contains("search table") {
            if detail_lower.contains("using index") {
                results.push(vec![format!("{}→ Index-based table search", indent_str)]);
                results.push(vec![format!(
                    "{}→ Efficient: Using index for lookup",
                    indent_str
                )]);
            } else {
                results.push(vec![format!("{}→ Sequential table search", indent_str)]);
            }
        } else if detail_lower.contains("using covering index") {
            results.push(vec![format!("{}→ Covering index optimization", indent_str)]);
            results.push(vec![format!(
                "{}→ Excellent: All columns found in index",
                indent_str
            )]);
        } else if detail_lower.contains("using index") {
            results.push(vec![format!("{}→ Index utilization", indent_str)]);
            results.push(vec![format!("{}→ Good: Query can use index", indent_str)]);
        } else if detail_lower.contains("using temporary b-tree") {
            results.push(vec![format!(
                "{}→ Temporary B-tree for sorting",
                indent_str
            )]);
            results.push(vec![format!(
                "{}→ Note: Creating temp structure for ORDER BY",
                indent_str
            )]);
        } else if detail_lower.contains("using integer primary key") {
            results.push(vec![format!(
                "{}→ Using rowid/integer primary key",
                indent_str
            )]);
            results.push(vec![format!(
                "{}→ Excellent: Most efficient access method",
                indent_str
            )]);
        } else if detail_lower.contains("compound subqueries") {
            results.push(vec![format!("{}→ Complex subquery processing", indent_str)]);
        } else if detail_lower.contains("co-routine") {
            results.push(vec![format!("{}→ Coroutine-based execution", indent_str)]);
            results.push(vec![format!(
                "{}→ Note: Processing subquery as coroutine",
                indent_str
            )]);
        }

        // Check for potential performance issues
        if detail_lower.contains("scan") && !detail_lower.contains("index") {
            results.push(vec![format!(
                "{}⚠ Performance Warning: Full table scan",
                indent_str
            )]);
        }
    }
}

#[async_trait]
impl DatabaseClient for SqliteClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[SqliteClient::execute_query] Executing query");

        let rows = sqlx::query(sql).fetch_all(&self.pool).await?;

        if rows.is_empty() {
            return Ok(vec![]);
        }

        let mut results = Vec::new();

        // Get column names from the first row
        let first_row = &rows[0];
        let column_names: Vec<String> = (0..first_row.len())
            .map(|i| first_row.column(i).name().to_string())
            .collect();

        results.push(column_names);

        // Convert rows to strings
        for row in rows {
            let mut string_row = Vec::new();
            for i in 0..row.len() {
                let value = format_sqlite_value(&row, i)?;
                string_row.push(value);
            }
            results.push(string_row);
        }

        debug!(
            "[SqliteClient::execute_query] Query completed with {} rows",
            results.len() - 1
        );
        Ok(results)
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        debug!("[SqliteClient::test_query] Testing query for validation");
        // For SQLite, we can use EXPLAIN QUERY PLAN to validate query syntax without executing it
        let explain_sql = format!("EXPLAIN QUERY PLAN {}", sql);

        match sqlx::query(&explain_sql).fetch_all(&self.pool).await {
            Ok(_) => Ok(()),
            Err(e) => Err(DatabaseError::QueryError(format!(
                "Query validation failed: {}",
                e
            ))),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[SqliteClient::explain_query] Executing EXPLAIN QUERY PLAN for query");

        let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
        let raw_results = self.execute_query(&explain_sql).await?;

        // Format the output for better readability
        self.format_explain_output(raw_results).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[SqliteClient::explain_query_raw] Executing raw EXPLAIN QUERY PLAN for query");

        let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
        self.execute_query(&explain_sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[SqliteClient::list_databases] Listing attached databases with enhanced information"
        );

        let query = "PRAGMA database_list";
        let rows = sqlx::query(query).fetch_all(&self.pool).await?;

        let mut results = Vec::new();

        // Add header row with enhanced information
        results.push(vec![
            "Seq".to_string(),
            "Name".to_string(),
            "File".to_string(),
            "Status".to_string(),
        ]);

        // Add data rows with enhanced information
        for row in rows {
            let seq = row.get::<i32, _>(0).to_string();
            let name = row.get::<String, _>(1);
            let file = row.get::<String, _>(2);

            // Determine status based on file path and name
            let status = if name == "main" {
                "Primary"
            } else if name == "temp" {
                "Temporary"
            } else if file.is_empty() || file.is_empty() {
                "In-Memory"
            } else {
                "Attached"
            };

            // Format file path for better display
            let formatted_file = if file.is_empty() || file.is_empty() {
                if name == "temp" {
                    "(temporary)".to_string()
                } else {
                    "(in-memory)".to_string()
                }
            } else {
                // Show relative path if it's in current directory, otherwise show full path
                if file.starts_with("./") || !file.contains('/') {
                    file
                } else {
                    // Show just the filename and parent directory for long paths
                    let path = std::path::Path::new(&file);
                    if let (Some(filename), Some(parent)) = (path.file_name(), path.parent()) {
                        if let (Some(filename_str), Some(parent_str)) =
                            (filename.to_str(), parent.to_str())
                        {
                            format!(
                                ".../{}/{}",
                                parent_str.split('/').next_back().unwrap_or(""),
                                filename_str
                            )
                        } else {
                            file
                        }
                    } else {
                        file
                    }
                }
            };

            results.push(vec![seq, name, formatted_file, status.to_string()]);
        }

        // Add summary information if there are multiple databases
        if results.len() > 2 {
            // More than header + main database
            results.push(vec![
                "".to_string(),
                "".to_string(),
                "".to_string(),
                "".to_string(),
            ]);
            results.push(vec![
                "".to_string(),
                "Summary:".to_string(),
                format!("{} database(s) attached", results.len() - 3),
                "".to_string(),
            ]);
        }

        debug!(
            "[SqliteClient::list_databases] Found {} databases",
            results.len() - 1
        );
        Ok(results)
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        debug!(
            "[SqliteClient::connect_to_database] Connecting to database: {}",
            database
        );

        // For SQLite, this could mean attaching a different database file
        // For now, we'll just update the current database name for display purposes
        self.current_database = database.to_string();

        // In a full implementation, we might:
        // 1. ATTACH DATABASE 'path/to/database.db' AS database_name
        // 2. Or create a new connection to a different file

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
        debug!("[SqliteClient::close] Closing SQLite connection");
        self.pool.close().await;
        Ok(())
    }

    async fn get_server_info(&self) -> Result<crate::database::ServerInfo, DatabaseError> {
        debug!("[SqliteClient::get_server_info] Fetching server version information");

        // Query SQLite version
        let version_query = "SELECT sqlite_version()";
        let version_row = sqlx::query(version_query)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get SQLite version: {}", e))
            })?;

        let version_string: String = version_row.get(0);
        debug!(
            "[SqliteClient::get_server_info] Raw version string: {}",
            version_string
        );

        // Create ServerInfo for SQLite
        let mut server_info = crate::database::ServerInfo::sqlite(version_string);

        // Add any additional SQLite-specific information
        server_info.additional_info.insert(
            "database_file".to_string(),
            self.connection_info
                .file_path
                .clone()
                .unwrap_or_else(|| "memory".to_string()),
        );

        // Try to get additional SQLite-specific information (non-critical, don't fail if these queries fail)
        if let Ok(pragma_row) = sqlx::query("PRAGMA page_size").fetch_one(&self.pool).await {
            if let Ok(page_size) = pragma_row.try_get::<i32, _>(0) {
                server_info
                    .additional_info
                    .insert("page_size".to_string(), page_size.to_string());
            }
        }

        if let Ok(pragma_row) = sqlx::query("PRAGMA page_count").fetch_one(&self.pool).await {
            if let Ok(page_count) = pragma_row.try_get::<i32, _>(0) {
                server_info
                    .additional_info
                    .insert("page_count".to_string(), page_count.to_string());

                // Calculate database size in bytes
                if let Some(page_size_str) = server_info.additional_info.get("page_size") {
                    if let Ok(page_size) = page_size_str.parse::<i32>() {
                        let db_size_bytes = page_count * page_size;
                        let db_size_mb = (db_size_bytes as f64) / (1024.0 * 1024.0);
                        server_info
                            .additional_info
                            .insert("database_size_mb".to_string(), format!("{:.2}", db_size_mb));
                    }
                }
            }
        }

        if let Ok(pragma_row) = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await
        {
            if let Ok(journal_mode) = pragma_row.try_get::<String, _>(0) {
                server_info
                    .additional_info
                    .insert("journal_mode".to_string(), journal_mode);
            }
        }

        debug!("[SqliteClient::get_server_info] Server info retrieved successfully");
        Ok(server_info)
    }
}

/// Format a SQLite value to string representation
fn format_sqlite_value(row: &SqliteRow, column_index: usize) -> Result<String, DatabaseError> {
    use sqlx::TypeInfo;
    use sqlx::ValueRef;

    let column = row.column(column_index);
    let type_info = column.type_info();

    // SQLite has dynamic typing, so we need to try different approaches
    // First, try to get the raw value to check if it's NULL
    if let Ok(value_ref) = row.try_get_raw(column_index) {
        if value_ref.is_null() {
            return Ok("".to_string());
        }
    }

    // Try different types in order of likelihood
    // Try as integer first (most common for COUNT(*))
    if let Ok(val) = row.try_get::<i64, _>(column_index) {
        return Ok(val.to_string());
    }

    // Try as i32
    if let Ok(val) = row.try_get::<i32, _>(column_index) {
        return Ok(val.to_string());
    }

    // Try as f64
    if let Ok(val) = row.try_get::<f64, _>(column_index) {
        return Ok(val.to_string());
    }

    // Try as f32
    if let Ok(val) = row.try_get::<f32, _>(column_index) {
        return Ok(val.to_string());
    }

    // Try as string
    if let Ok(val) = row.try_get::<String, _>(column_index) {
        return Ok(val);
    }

    // Try as Vec<u8> for BLOB
    if let Ok(val) = row.try_get::<Vec<u8>, _>(column_index) {
        return Ok(format!("\\x{}", hex::encode(val)));
    }

    // If all else fails, try to convert via the type system
    match type_info.name() {
        "INTEGER" => {
            // Last resort for INTEGER types
            if let Ok(val) = row.try_get::<i64, _>(column_index) {
                Ok(val.to_string())
            } else {
                Err(DatabaseError::QueryError(format!(
                    "Unable to format INTEGER value at column {column_index}"
                )))
            }
        }
        "DECIMAL" | "NUMERIC" => {
            // SQLite DECIMAL/NUMERIC types - try as string first (preserves precision)
            if let Ok(val) = row.try_get::<String, _>(column_index) {
                Ok(val)
            } else if let Ok(val) = row.try_get::<f64, _>(column_index) {
                Ok(val.to_string())
            } else {
                Err(DatabaseError::QueryError(format!(
                    "Unable to format DECIMAL/NUMERIC value at column {column_index}"
                )))
            }
        }
        _ => {
            // Final fallback: return a descriptive message for unknown types
            Ok(format!(
                "[SQLite {} type - conversion not implemented]",
                type_info.name()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_sqlite_client_creation() {
        // Use the test database we created earlier
        let test_db_path = std::env::current_dir()
            .unwrap()
            .join("test_data")
            .join("test_sample.db");

        // Skip test if the test database doesn't exist
        if !test_db_path.exists() {
            println!("Test database not found at {test_db_path:?}, skipping test");
            return;
        }

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(test_db_path.to_string_lossy().to_string()),
            options: HashMap::new(),
            docker_container: None,
        };

        match SqliteClient::new(connection_info).await {
            Ok(client) => {
                assert_eq!(client.get_current_database(), "test_sample");
                assert!(client.is_connected().await);

                // Test a simple query
                let results = client
                    .execute_query("SELECT COUNT(*) FROM users")
                    .await
                    .unwrap();
                assert!(!results.is_empty());
                assert_eq!(results[0][0], "COUNT(*)"); // Header
                assert_eq!(results[1][0], "5"); // 5 users in test data
            }
            Err(e) => {
                panic!("Failed to create SQLite client: {e:?}");
            }
        }
    }

    #[tokio::test]
    async fn test_sqlite_metadata_provider() {
        // Use the test database we created earlier
        let test_db_path = std::env::current_dir()
            .unwrap()
            .join("test_data")
            .join("test_sample.db");

        // Skip test if the test database doesn't exist
        if !test_db_path.exists() {
            println!("Test database not found at {test_db_path:?}, skipping test");
            return;
        }

        let pool = SqlitePoolOptions::new()
            .connect(&format!("sqlite://{}", test_db_path.to_string_lossy()))
            .await
            .unwrap();

        let provider = SqliteMetadataProvider::new(pool);

        // Test get_schemas
        let schemas = provider.get_schemas().await.unwrap();
        assert!(schemas.contains(&"main".to_string()));

        // Test get_tables (should include our test tables)
        let tables = provider.get_tables(Some("main")).await.unwrap();
        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"posts".to_string()));
        assert!(tables.contains(&"categories".to_string()));

        // Test get_columns
        let columns = provider.get_columns("users", Some("main")).await.unwrap();
        assert!(columns.contains(&"id".to_string()));
        assert!(columns.contains(&"username".to_string()));
        assert!(columns.contains(&"email".to_string()));

        // Test get_functions
        let functions = provider.get_functions(None).await.unwrap();
        assert!(functions.contains(&"count".to_string()));
        assert!(functions.contains(&"max".to_string()));
        assert!(functions.contains(&"json_extract".to_string()));
    }

    #[tokio::test]
    async fn test_sqlite_query_execution() {
        // Use the test database we created earlier
        let test_db_path = std::env::current_dir()
            .unwrap()
            .join("test_data")
            .join("test_sample.db");

        // Skip test if the test database doesn't exist
        if !test_db_path.exists() {
            println!("Test database not found at {test_db_path:?}, skipping test");
            return;
        }

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(test_db_path.to_string_lossy().to_string()),
            options: HashMap::new(),
            docker_container: None,
        };

        let client = SqliteClient::new(connection_info).await.unwrap();

        // Test SELECT query with existing data
        let results = client
            .execute_query("SELECT username, email FROM users ORDER BY id LIMIT 2")
            .await
            .unwrap();

        assert_eq!(results.len(), 3); // Header + 2 data rows
        assert_eq!(results[0], vec!["username", "email"]); // Header
        assert_eq!(results[1][0], "alice"); // First user
        assert_eq!(results[1][1], "alice@example.com");
        assert_eq!(results[2][0], "bob"); // Second user
        assert_eq!(results[2][1], "bob@example.com");

        // Test JOIN query
        let join_results = client
            .execute_query(
                "SELECT u.username, COUNT(p.id) as post_count
             FROM users u
             LEFT JOIN posts p ON u.id = p.user_id
             GROUP BY u.id, u.username
             ORDER BY u.username",
            )
            .await
            .unwrap();

        assert!(!join_results.is_empty());
        assert_eq!(join_results[0], vec!["username", "post_count"]); // Header

        // Test EXPLAIN query
        let explain_results = client
            .explain_query("SELECT * FROM users WHERE username = 'alice'")
            .await
            .unwrap();
        assert!(!explain_results.is_empty());
        // Check if the enhanced formatting is working (should start with "SQLite Query Plan")
        assert!(
            explain_results[0]
                .iter()
                .any(|col| col.contains("SQLite Query Plan"))
        );
    }
}
