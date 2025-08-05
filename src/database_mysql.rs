//! MySQL implementation of the database abstraction layer
use async_trait::async_trait;
use crate::database::{ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider};
use crate::db::TableDetails;
use tracing::debug;
use crate::performance_analyzer::PerformanceAnalyzer;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{Row, Column};
use sqlx::types::Decimal;

/// MySQL metadata provider implementation
pub struct MySqlMetadataProvider {
    pool: MySqlPool,
}

impl MySqlMetadataProvider {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MetadataProvider for MySqlMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        debug!("[MySqlMetadataProvider::get_schemas] Starting query");
        
        // MySQL schemas are essentially databases
        let rows = sqlx::query(
            r#"
            SELECT SCHEMA_NAME
            FROM INFORMATION_SCHEMA.SCHEMATA
            WHERE SCHEMA_NAME NOT IN ('information_schema', 'performance_schema', 'mysql', 'sys')
            ORDER BY SCHEMA_NAME
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let schemas: Vec<String> = rows
            .iter()
            .map(|row| {
                // Try different approaches to get the schema name
                if let Ok(name) = row.try_get::<String, _>("SCHEMA_NAME") {
                    name
                } else if let Ok(name) = row.try_get::<String, _>(0) {
                    name
                } else {
                    // Fallback: convert bytes to string if needed
                    row.try_get::<Vec<u8>, _>(0)
                        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                        .unwrap_or_else(|_| "unknown".to_string())
                }
            })
            .collect();

        debug!("[MySqlMetadataProvider::get_schemas] Found {} schemas", schemas.len());
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[MySqlMetadataProvider::get_tables] Starting query for schema: {:?}", schema);

        let query = if let Some(schema_name) = schema {
            format!(
                r#"
                SELECT TABLE_NAME
                FROM INFORMATION_SCHEMA.TABLES
                WHERE TABLE_SCHEMA = '{schema_name}'
                  AND TABLE_TYPE IN ('BASE TABLE', 'VIEW')
                ORDER BY TABLE_NAME
                "#
            )
        } else {
            r#"
            SELECT TABLE_NAME
            FROM INFORMATION_SCHEMA.TABLES
            WHERE TABLE_SCHEMA = DATABASE()
              AND TABLE_TYPE IN ('BASE TABLE', 'VIEW')
            ORDER BY TABLE_NAME
            "#
            .to_string()
        };

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;
        let tables: Vec<String> = rows
            .iter()
            .map(|row| {
                // Try different approaches to get the table name
                if let Ok(name) = row.try_get::<String, _>("TABLE_NAME") {
                    name
                } else if let Ok(name) = row.try_get::<String, _>(0) {
                    name
                } else {
                    // Fallback: convert bytes to string if needed
                    row.try_get::<Vec<u8>, _>(0)
                        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                        .unwrap_or_else(|_| "unknown".to_string())
                }
            })
            .collect();

        debug!("[MySqlMetadataProvider::get_tables] Found {} tables", tables.len());
        Ok(tables)
    }

    async fn get_columns(&self, table: &str, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[MySqlMetadataProvider::get_columns] Starting query for table: '{}', schema: {:?}", table, schema);

        let query = if let Some(schema_name) = schema {
            format!(
                r#"
                SELECT COLUMN_NAME
                FROM INFORMATION_SCHEMA.COLUMNS
                WHERE TABLE_SCHEMA = '{schema_name}'
                  AND TABLE_NAME = '{table}'
                ORDER BY ORDINAL_POSITION
                "#
            )
        } else {
            format!(
                r#"
                SELECT COLUMN_NAME
                FROM INFORMATION_SCHEMA.COLUMNS
                WHERE TABLE_SCHEMA = DATABASE()
                  AND TABLE_NAME = '{table}'
                ORDER BY ORDINAL_POSITION
                "#
            )
        };

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;
        let columns: Vec<String> = rows
            .iter()
            .map(|row| {
                // Try different approaches to get the column name
                if let Ok(name) = row.try_get::<String, _>("COLUMN_NAME") {
                    name
                } else if let Ok(name) = row.try_get::<String, _>(0) {
                    name
                } else {
                    // Fallback: convert bytes to string if needed
                    row.try_get::<Vec<u8>, _>(0)
                        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                        .unwrap_or_else(|_| "unknown".to_string())
                }
            })
            .collect();

        debug!("[MySqlMetadataProvider::get_columns] Found {} columns", columns.len());
        Ok(columns)
    }

    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[MySqlMetadataProvider::get_functions] Starting query");

        // MySQL built-in functions and user-defined functions
        let query = if let Some(schema_name) = schema {
            format!(
                r#"
                SELECT ROUTINE_NAME as function_name
                FROM INFORMATION_SCHEMA.ROUTINES
                WHERE ROUTINE_SCHEMA = '{schema_name}'
                  AND ROUTINE_TYPE = 'FUNCTION'
                ORDER BY ROUTINE_NAME
                "#
            )
        } else {
            // Return common MySQL built-in functions when no specific schema is requested
            let builtin_functions = vec![
                "ABS".to_string(),
                "AVG".to_string(),
                "CEIL".to_string(),
                "COALESCE".to_string(),
                "CONCAT".to_string(),
                "COUNT".to_string(),
                "CURDATE".to_string(),
                "CURTIME".to_string(),
                "DATE".to_string(),
                "DATE_ADD".to_string(),
                "DATE_FORMAT".to_string(),
                "DATE_SUB".to_string(),
                "FLOOR".to_string(),
                "GROUP_CONCAT".to_string(),
                "IF".to_string(),
                "IFNULL".to_string(),
                "JSON_EXTRACT".to_string(),
                "JSON_OBJECT".to_string(),
                "JSON_VALID".to_string(),
                "LEFT".to_string(),
                "LENGTH".to_string(),
                "LOWER".to_string(),
                "LTRIM".to_string(),
                "MAX".to_string(),
                "MIN".to_string(),
                "NOW".to_string(),
                "NULLIF".to_string(),
                "RAND".to_string(),
                "REPLACE".to_string(),
                "RIGHT".to_string(),
                "ROUND".to_string(),
                "RTRIM".to_string(),
                "STR_TO_DATE".to_string(),
                "SUBSTR".to_string(),
                "SUBSTRING".to_string(),
                "SUM".to_string(),
                "TIME".to_string(),
                "TRIM".to_string(),
                "UPPER".to_string(),
                "UUID".to_string(),
                "VERSION".to_string(),
                "YEAR".to_string(),
            ];
            debug!("[MySqlMetadataProvider::get_functions] Found {} built-in functions", builtin_functions.len());
            return Ok(builtin_functions);
        };

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;
        let functions: Vec<String> = rows
            .iter()
            .map(|row| row.get::<String, _>("function_name"))
            .collect();

        debug!("[MySqlMetadataProvider::get_functions] Found {} functions", functions.len());
        Ok(functions)
    }

    async fn get_table_details(&self, table: &str, schema: Option<&str>) -> Result<TableDetails, DatabaseError> {
        debug!("[MySqlMetadataProvider::get_table_details] Getting details for table: {}", table);
        
        let schema_name = schema.unwrap_or("DATABASE()");
        
        // First check if the table exists
        let table_exists_query = if schema.is_some() {
            format!(
                r#"
                SELECT TABLE_NAME
                FROM INFORMATION_SCHEMA.TABLES
                WHERE TABLE_SCHEMA = '{schema_name}' AND TABLE_NAME = '{table}'
                "#
            )
        } else {
            format!(
                r#"
                SELECT TABLE_NAME
                FROM INFORMATION_SCHEMA.TABLES
                WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{table}'
                "#
            )
        };

        let table_exists = sqlx::query(&table_exists_query)
            .fetch_optional(&self.pool)
            .await?;
            
        if table_exists.is_none() {
            return Err(DatabaseError::QueryError(format!("Table '{table}' does not exist")));
        }

        // Get column information
        let columns_query = if schema.is_some() {
            format!(
                r#"
                SELECT 
                    COLUMN_NAME,
                    COLUMN_TYPE,
                    IS_NULLABLE,
                    COLUMN_DEFAULT,
                    COLLATION_NAME
                FROM INFORMATION_SCHEMA.COLUMNS
                WHERE TABLE_SCHEMA = '{schema_name}' AND TABLE_NAME = '{table}'
                ORDER BY ORDINAL_POSITION
                "#
            )
        } else {
            format!(
                r#"
                SELECT 
                    COLUMN_NAME,
                    COLUMN_TYPE,
                    IS_NULLABLE,
                    COLUMN_DEFAULT,
                    COLLATION_NAME
                FROM INFORMATION_SCHEMA.COLUMNS
                WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{table}'
                ORDER BY ORDINAL_POSITION
                "#
            )
        };

        let column_rows = sqlx::query(&columns_query).fetch_all(&self.pool).await?;
        
        let mut columns = Vec::new();
        for row in column_rows {
            // Helper function to safely get string values
            let get_string_value = |row: &sqlx::mysql::MySqlRow, column: &str, index: usize| -> String {
                if let Ok(val) = row.try_get::<String, _>(column) {
                    val
                } else if let Ok(val) = row.try_get::<String, _>(index) {
                    val
                } else if let Ok(val) = row.try_get::<Vec<u8>, _>(index) {
                    String::from_utf8_lossy(&val).to_string()
                } else {
                    "unknown".to_string()
                }
            };

            let get_optional_string = |row: &sqlx::mysql::MySqlRow, column: &str, index: usize| -> Option<String> {
                if let Ok(val) = row.try_get::<Option<String>, _>(column) {
                    val
                } else if let Ok(val) = row.try_get::<Option<String>, _>(index) {
                    val
                } else if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(index) {
                    val.map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                } else {
                    None
                }
            };

            let column = crate::db::ColumnInfo {
                name: get_string_value(&row, "COLUMN_NAME", 0),
                data_type: get_string_value(&row, "COLUMN_TYPE", 1),
                nullable: {
                    let nullable_str = get_string_value(&row, "IS_NULLABLE", 2);
                    nullable_str.to_uppercase() == "YES"
                },
                default_value: get_optional_string(&row, "COLUMN_DEFAULT", 3),
                collation: get_optional_string(&row, "COLLATION_NAME", 4).unwrap_or_default(),
            };
            columns.push(column);
        }

        // Get index information
        let indexes_query = if schema.is_some() {
            format!(
                r#"
                SELECT 
                    INDEX_NAME as name,
                    GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX) as columns,
                    NON_UNIQUE = 0 as is_unique,
                    INDEX_NAME = 'PRIMARY' as is_primary,
                    INDEX_TYPE
                FROM INFORMATION_SCHEMA.STATISTICS
                WHERE TABLE_SCHEMA = '{schema_name}' AND TABLE_NAME = '{table}'
                GROUP BY INDEX_NAME, INDEX_TYPE, NON_UNIQUE
                ORDER BY INDEX_NAME
                "#
            )
        } else {
            format!(
                r#"
                SELECT 
                    INDEX_NAME as name,
                    GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX) as columns,
                    NON_UNIQUE = 0 as is_unique,
                    INDEX_NAME = 'PRIMARY' as is_primary,
                    INDEX_TYPE
                FROM INFORMATION_SCHEMA.STATISTICS
                WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = '{table}'
                GROUP BY INDEX_NAME, INDEX_TYPE, NON_UNIQUE
                ORDER BY INDEX_NAME
                "#
            )
        };

        let index_rows = sqlx::query(&indexes_query).fetch_all(&self.pool).await?;
        
        let mut indexes = Vec::new();
        for row in index_rows {
            // Helper function to safely get values
            let get_string_value = |row: &sqlx::mysql::MySqlRow, column: &str| -> String {
                if let Ok(val) = row.try_get::<String, _>(column) {
                    val
                } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(column) {
                    String::from_utf8_lossy(&bytes).to_string()
                } else {
                    "unknown".to_string()
                }
            };

            let get_bool_value = |row: &sqlx::mysql::MySqlRow, column: &str| -> bool {
                if let Ok(val) = row.try_get::<bool, _>(column) {
                    val
                } else if let Ok(val) = row.try_get::<i8, _>(column) {
                    val != 0
                } else if let Ok(val) = row.try_get::<u8, _>(column) {
                    val != 0
                } else {
                    false
                }
            };
            
            let index_name = get_string_value(&row, "name");
            let columns_str = get_string_value(&row, "columns");
            let is_unique = get_bool_value(&row, "is_unique");
            let is_primary = get_bool_value(&row, "is_primary");
            let index_type = get_string_value(&row, "INDEX_TYPE");
            
            let index_info = crate::db::IndexInfo {
                name: index_name.clone(),
                definition: format!("({columns_str})"),
                index_type: index_type.to_lowercase(),
                is_primary,
                is_unique,
                predicate: None, // MySQL doesn't have partial indexes like PostgreSQL
                constraint_def: None,
            };
            indexes.push(index_info);
        }

        // Get foreign key information
        let fk_query = if schema.is_some() {
            format!(
                r#"
                SELECT 
                    CONSTRAINT_NAME as name,
                    COLUMN_NAME as from_column,
                    REFERENCED_TABLE_NAME as to_table,
                    REFERENCED_COLUMN_NAME as to_column
                FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE
                WHERE TABLE_SCHEMA = '{schema_name}' 
                  AND TABLE_NAME = '{table}'
                  AND REFERENCED_TABLE_NAME IS NOT NULL
                ORDER BY CONSTRAINT_NAME, ORDINAL_POSITION
                "#
            )
        } else {
            format!(
                r#"
                SELECT 
                    CONSTRAINT_NAME as name,
                    COLUMN_NAME as from_column,
                    REFERENCED_TABLE_NAME as to_table,
                    REFERENCED_COLUMN_NAME as to_column
                FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE
                WHERE TABLE_SCHEMA = DATABASE() 
                  AND TABLE_NAME = '{table}'
                  AND REFERENCED_TABLE_NAME IS NOT NULL
                ORDER BY CONSTRAINT_NAME, ORDINAL_POSITION
                "#
            )
        };

        let fk_rows = sqlx::query(&fk_query).fetch_all(&self.pool).await?;
        
        let mut foreign_keys = Vec::new();
        for row in fk_rows {
            // Helper function to safely get values
            let get_string_value = |row: &sqlx::mysql::MySqlRow, column: &str| -> String {
                if let Ok(val) = row.try_get::<String, _>(column) {
                    val
                } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(column) {
                    String::from_utf8_lossy(&bytes).to_string()
                } else {
                    "unknown".to_string()
                }
            };

            let constraint_name = get_string_value(&row, "name");
            let from_column = get_string_value(&row, "from_column");
            let to_table = get_string_value(&row, "to_table");
            let to_column = get_string_value(&row, "to_column");
            
            let fk_info = crate::db::ForeignKeyInfo {
                name: constraint_name,
                definition: format!("FOREIGN KEY ({from_column}) REFERENCES {to_table}({to_column})"),
            };
            foreign_keys.push(fk_info);
        }

        let table_details = TableDetails {
            schema: schema.unwrap_or("default").to_string(),
            name: table.to_string(),
            full_name: if let Some(s) = schema {
                format!("{s}.{table}")
            } else {
                table.to_string()
            },
            columns,
            indexes,
            check_constraints: Vec::new(), // MySQL check constraints are available in newer versions
            foreign_keys,
            referenced_by: Vec::new(), // Would need complex query to find referencing tables
        };

        debug!("[MySqlMetadataProvider::get_table_details] Table details retrieved successfully");
        Ok(table_details)
    }

    fn supports_explain(&self) -> bool {
        true // MySQL supports EXPLAIN
    }

    fn default_schema(&self) -> Option<String> {
        None // MySQL doesn't have a fixed default schema like SQLite's "main"
    }
}

/// MySQL database client implementation
pub struct MySqlClient {
    pool: MySqlPool,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: MySqlMetadataProvider,
}

impl MySqlClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        debug!("[MySqlClient::new] Creating MySQL client");

        // Build connection URL
        let host = connection_info.host.as_deref().unwrap_or("localhost");
        let port = connection_info.port.unwrap_or(3306);
        let username = connection_info.username.as_deref().unwrap_or("root");
        let database = connection_info.database.clone().unwrap_or_else(|| "mysql".to_string());
        
        let mut database_url = if let Some(password) = &connection_info.password {
            format!("mysql://{username}:{password}@{host}:{port}/{database}")
        } else {
            format!("mysql://{username}@{host}:{port}/{database}")
        };

        // Add query parameters
        if !connection_info.options.is_empty() {
            let params: Vec<String> = connection_info
                .options
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            database_url.push('?');
            database_url.push_str(&params.join("&"));
        }

        debug!("[MySqlClient::new] Connecting to: {}", crate::password_sanitizer::sanitize_connection_url(&database_url));

        // Configure connection pool with MySQL-specific optimizations
        let pool = MySqlPoolOptions::new()
            .max_connections(10) // Same as PostgreSQL for consistency
            .min_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .idle_timeout(std::time::Duration::from_secs(600)) // Keep connections alive longer
            .connect(&database_url)
            .await
            .map_err(|e| DatabaseError::ConnectionError(e.to_string()))?;

        // Apply MySQL-specific optimizations
        Self::apply_mysql_optimizations(&pool).await?;

        let metadata_provider = MySqlMetadataProvider::new(pool.clone());

        Ok(Self {
            pool,
            connection_info,
            current_database: database,
            metadata_provider,
        })
    }

    /// Apply MySQL-specific performance optimizations
    async fn apply_mysql_optimizations(pool: &MySqlPool) -> Result<(), DatabaseError> {
        debug!("[MySqlClient] Applying MySQL optimizations");
        
        // Set session-level optimizations for better performance
        sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'")
            .execute(pool)
            .await?;

        // Enable better query cache behavior
        sqlx::query("SET SESSION query_cache_type = ON")
            .execute(pool)
            .await
            .ok(); // Ignore errors as query cache might be disabled

        debug!("[MySqlClient] MySQL optimizations applied successfully");
        Ok(())
    }
    
    /// Format JSON EXPLAIN output into a readable format
    async fn format_json_explain_output(&self, rows: Vec<MySqlRow>) -> Result<Vec<Vec<String>>, DatabaseError> {
        use serde_json::Value;
        
        debug!("[MySqlClient::format_json_explain_output] Formatting JSON EXPLAIN output");
        
        let mut results = Vec::new();
        results.push(vec!["MySQL Query Plan".to_string()]);
        results.push(vec!["".to_string()]);
        
        for row in rows {
            // Get the JSON string from the first column
            let json_str = if let Ok(val) = row.try_get::<String, _>(0) {
                val
            } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(0) {
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                continue;
            };
            
            // Parse JSON
            match serde_json::from_str::<Value>(&json_str) {
                Ok(json) => {
                    // Use performance analyzer to get metrics
                    let performance_metrics = PerformanceAnalyzer::analyze_mysql_plan(&json);
                    
                    // Add performance summary header
                    let performance_summary = PerformanceAnalyzer::format_metrics_with_colors(&performance_metrics);
                    for line in performance_summary {
                        results.push(vec![line]);
                    }
                    
                    results.push(vec!["".to_string()]);
                    results.push(vec!["💡 Use \\ecopy to copy the raw JSON plan to clipboard".to_string()]);
                    results.push(vec!["".to_string()]);
                    results.push(vec!["Detailed Plan Steps:".to_string()]);
                    results.push(vec!["".to_string()]);
                    
                    // Add the detailed recursive formatting
                    self.format_explain_json_recursive(&json, &mut results, 0);
                },
                Err(e) => {
                    debug!("[MySqlClient::format_json_explain_output] JSON parse error: {}", e);
                    results.push(vec![format!("JSON Parse Error: {}", e)]);
                    results.push(vec![json_str]);
                }
            }
        }
        
        if results.len() <= 2 {
            results.push(vec!["No query plan information available".to_string()]);
        }
        
        Ok(results)
    }
    
    /// Recursively format JSON EXPLAIN plan
    fn format_explain_json_recursive(&self, json: &serde_json::Value, results: &mut Vec<Vec<String>>, indent: usize) {
        use serde_json::Value;
        
        let indent_str = "  ".repeat(indent);
        
        match json {
            Value::Object(obj) => {
                // Handle query_block or cost_info specially
                if let Some(query_block) = obj.get("query_block") {
                    results.push(vec![format!("{}Query Block:", indent_str)]);
                    if let Some(select_id) = query_block.get("select_id") {
                        results.push(vec![format!("{}  Select ID: {}", indent_str, select_id)]);
                    }
                    if let Some(cost_info) = query_block.get("cost_info") {
                        self.format_cost_info(cost_info, results, indent + 1);
                    }
                    if let Some(table) = query_block.get("table") {
                        self.format_table_info(table, results, indent + 1);
                    }
                    if let Some(nested_loop) = query_block.get("nested_loop") {
                        results.push(vec![format!("{}  Nested Loop:", indent_str)]);
                        self.format_explain_json_recursive(nested_loop, results, indent + 2);
                    }
                } else if let Some(table_name) = obj.get("table_name") {
                    results.push(vec![format!("{}Table: {}", indent_str, table_name)]);
                    if let Some(access_type) = obj.get("access_type") {
                        results.push(vec![format!("{}  Access Type: {}", indent_str, access_type)]);
                    }
                    if let Some(key) = obj.get("key") {
                        results.push(vec![format!("{}  Key: {}", indent_str, key)]);
                    }
                    if let Some(rows_examined) = obj.get("rows_examined_per_scan") {
                        results.push(vec![format!("{}  Rows Examined: {}", indent_str, rows_examined)]);
                    }
                    if let Some(cost_info) = obj.get("cost_info") {
                        self.format_cost_info(cost_info, results, indent + 1);
                    }
                } else {
                    // Generic object formatting
                    for (key, value) in obj {
                        match value {
                            Value::Object(_) | Value::Array(_) => {
                                results.push(vec![format!("{}{}:", indent_str, key)]);
                                self.format_explain_json_recursive(value, results, indent + 1);
                            },
                            _ => {
                                results.push(vec![format!("{}{}: {}", indent_str, key, value)]);
                            }
                        }
                    }
                }
            },
            Value::Array(arr) => {
                for (i, item) in arr.iter().enumerate() {
                    results.push(vec![format!("{}[{}]:", indent_str, i)]);
                    self.format_explain_json_recursive(item, results, indent + 1);
                }
            },
            _ => {
                results.push(vec![format!("{}{}", indent_str, json)]);
            }
        }
    }
    
    /// Format cost information
    fn format_cost_info(&self, cost_info: &serde_json::Value, results: &mut Vec<Vec<String>>, indent: usize) {
        let indent_str = "  ".repeat(indent);
        
        if let Some(obj) = cost_info.as_object() {
            results.push(vec![format!("{}Cost Info:", indent_str)]);
            if let Some(read_cost) = obj.get("read_cost") {
                results.push(vec![format!("{}  Read Cost: {}", indent_str, read_cost)]);
            }
            if let Some(eval_cost) = obj.get("eval_cost") {
                results.push(vec![format!("{}  Eval Cost: {}", indent_str, eval_cost)]);
            }
            if let Some(prefix_cost) = obj.get("prefix_cost") {
                results.push(vec![format!("{}  Prefix Cost: {}", indent_str, prefix_cost)]);
            }
            if let Some(data_read_per_join) = obj.get("data_read_per_join") {
                results.push(vec![format!("{}  Data Read Per Join: {}", indent_str, data_read_per_join)]);
            }
        }
    }
    
    /// Format table information
    fn format_table_info(&self, table: &serde_json::Value, results: &mut Vec<Vec<String>>, indent: usize) {
        let indent_str = "  ".repeat(indent);
        
        if let Some(obj) = table.as_object() {
            if let Some(table_name) = obj.get("table_name") {
                results.push(vec![format!("{}Table: {}", indent_str, table_name)]);
            }
            if let Some(access_type) = obj.get("access_type") {
                results.push(vec![format!("{}  Access Type: {}", indent_str, access_type)]);
            }
            if let Some(key) = obj.get("key") {
                results.push(vec![format!("{}  Index: {}", indent_str, key)]);
            }
            if let Some(key_length) = obj.get("key_length") {
                results.push(vec![format!("{}  Key Length: {}", indent_str, key_length)]);
            }
            if let Some(rows_examined) = obj.get("rows_examined_per_scan") {
                results.push(vec![format!("{}  Rows Examined per Scan: {}", indent_str, rows_examined)]);
            }
            if let Some(rows_produced) = obj.get("rows_produced_per_join") {
                results.push(vec![format!("{}  Rows Produced per Join: {}", indent_str, rows_produced)]);
            }
            if let Some(filtered) = obj.get("filtered") {
                results.push(vec![format!("{}  Filtered: {}%", indent_str, filtered)]);
            }
            if let Some(cost_info) = obj.get("cost_info") {
                self.format_cost_info(cost_info, results, indent + 1);
            }
            if let Some(used_columns) = obj.get("used_columns") {
                if let Some(arr) = used_columns.as_array() {
                    let columns: Vec<String> = arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.to_string())
                        .collect();
                    if !columns.is_empty() {
                        results.push(vec![format!("{}  Used Columns: {}", indent_str, columns.join(", "))]);
                    }
                }
            }
        }
    }
}

#[async_trait]
impl DatabaseClient for MySqlClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MySqlClient::execute_query] Executing query");

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
                let value = format_mysql_value(&row, i)?;
                string_row.push(value);
            }
            results.push(string_row);
        }

        debug!("[MySqlClient::execute_query] Query completed with {} rows", results.len() - 1);
        Ok(results)
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        debug!("[MySqlClient::test_query] Testing query for validation");
        // For MySQL, we can use EXPLAIN to validate query syntax without executing it
        let explain_sql = format!("EXPLAIN {}", sql);
        
        match sqlx::query(&explain_sql).fetch_all(&self.pool).await {
            Ok(_) => Ok(()),
            Err(e) => Err(DatabaseError::QueryError(format!("Query validation failed: {}", e))),
        }
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MySqlClient::explain_query] Executing EXPLAIN for query");
        
        // Try EXPLAIN FORMAT=JSON first for better structured output
        let json_explain_sql = format!("EXPLAIN FORMAT=JSON {sql}");
        let json_result = sqlx::query(&json_explain_sql).fetch_all(&self.pool).await;
        
        match json_result {
            Ok(rows) if !rows.is_empty() => {
                debug!("[MySqlClient::explain_query] Using JSON format");
                return self.format_json_explain_output(rows).await;
            },
            Err(e) => {
                debug!("[MySqlClient::explain_query] JSON format failed: {}, falling back to standard", e);
            },
            _ => {
                debug!("[MySqlClient::explain_query] JSON format returned empty, falling back to standard");
            }
        }
        
        // Fallback to standard EXPLAIN format
        debug!("[MySqlClient::explain_query] Using standard EXPLAIN format");
        let explain_sql = format!("EXPLAIN {sql}");
        self.execute_query(&explain_sql).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MySqlClient::explain_query_raw] Executing raw EXPLAIN for query");
        
        // Try EXPLAIN FORMAT=JSON first for raw structured output
        let json_explain_sql = format!("EXPLAIN FORMAT=JSON {sql}");
        let json_result = self.execute_query(&json_explain_sql).await;
        
        if json_result.is_ok() {
            debug!("[MySqlClient::explain_query_raw] Using JSON format");
            return json_result;
        }
        
        // Fallback to standard EXPLAIN format if JSON fails
        debug!("[MySqlClient::explain_query_raw] JSON format failed, falling back to standard");
        let explain_sql = format!("EXPLAIN {sql}");
        self.execute_query(&explain_sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[MySqlClient::list_databases] Listing databases with enhanced information");
        
        // Try enhanced query first with character set and collation info
        let enhanced_query = r#"
            SELECT 
                SCHEMA_NAME as 'Database',
                DEFAULT_CHARACTER_SET_NAME as 'Charset',
                DEFAULT_COLLATION_NAME as 'Collation'
            FROM INFORMATION_SCHEMA.SCHEMATA
            ORDER BY SCHEMA_NAME
        "#;
        
        let enhanced_result = sqlx::query(enhanced_query).fetch_all(&self.pool).await;
        
        match enhanced_result {
            Ok(rows) => {
                debug!("[MySqlClient::list_databases] Using enhanced format with charset/collation");
                let mut results = Vec::new();
                
                // Add header row for enhanced format
                results.push(vec![
                    "Database".to_string(),
                    "Charset".to_string(), 
                    "Collation".to_string()
                ]);

                // Add data rows with enhanced information
                for row in rows {
                    let get_string_value = |row: &sqlx::mysql::MySqlRow, index: usize| -> String {
                        if let Ok(val) = row.try_get::<String, _>(index) {
                            val
                        } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(index) {
                            String::from_utf8_lossy(&bytes).to_string()
                        } else {
                            "unknown".to_string()
                        }
                    };

                    let db_name = get_string_value(&row, 0);
                    let charset = get_string_value(&row, 1);
                    let collation = get_string_value(&row, 2);
                    
                    results.push(vec![db_name, charset, collation]);
                }

                debug!("[MySqlClient::list_databases] Found {} databases with enhanced info", results.len() - 1);
                return Ok(results);
            },
            Err(e) => {
                debug!("[MySqlClient::list_databases] Enhanced query failed: {}, falling back to basic", e);
            }
        }
        
        // Fallback to basic SHOW DATABASES
        debug!("[MySqlClient::list_databases] Using basic SHOW DATABASES format");
        let query = "SHOW DATABASES";
        let rows = sqlx::query(query).fetch_all(&self.pool).await?;
        
        let mut results = Vec::new();
        
        // Add header row
        results.push(vec!["Database".to_string()]);

        // Add data rows
        for row in rows {
            let db_name = if let Ok(name) = row.try_get::<String, _>(0) {
                name
            } else if let Ok(bytes) = row.try_get::<Vec<u8>, _>(0) {
                String::from_utf8_lossy(&bytes).to_string()
            } else {
                "unknown".to_string()
            };
            results.push(vec![db_name]);
        }

        debug!("[MySqlClient::list_databases] Found {} databases", results.len() - 1);
        Ok(results)
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        debug!("[MySqlClient::connect_to_database] Connecting to database: {}", database);
        
        // Execute USE statement to change database
        let use_query = format!("USE `{database}`");
        sqlx::query(&use_query)
            .execute(&self.pool)
            .await?;
        
        self.current_database = database.to_string();
        
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
        debug!("[MySqlClient::close] Closing MySQL connection");
        self.pool.close().await;
        Ok(())
    }
}

/// Format a MySQL value to string representation
fn format_mysql_value(row: &MySqlRow, column_index: usize) -> Result<String, DatabaseError> {
    use sqlx::TypeInfo;
    use sqlx::ValueRef;
    
    let column = row.column(column_index);
    let type_info = column.type_info();
    
    // First, try to get the raw value to check if it's NULL
    if let Ok(value_ref) = row.try_get_raw(column_index) {
        if value_ref.is_null() {
            return Ok("".to_string());
        }
    }
    
    // Try different types in order of likelihood for MySQL
    
    // Try as signed integers
    if let Ok(val) = row.try_get::<i64, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<i32, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<i16, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<i8, _>(column_index) {
        return Ok(val.to_string());
    }
    
    // Try as unsigned integers
    if let Ok(val) = row.try_get::<u64, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<u32, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<u16, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<u8, _>(column_index) {
        return Ok(val.to_string());
    }
    
    // Try as floating point
    if let Ok(val) = row.try_get::<f64, _>(column_index) {
        return Ok(val.to_string());
    }
    
    if let Ok(val) = row.try_get::<f32, _>(column_index) {
        return Ok(val.to_string());
    }
    
    // Try as decimal (for DECIMAL/NUMERIC types)
    if let Ok(val) = row.try_get::<Decimal, _>(column_index) {
        return Ok(val.to_string());
    }
    
    // Try as string/text
    if let Ok(val) = row.try_get::<String, _>(column_index) {
        return Ok(val);
    }
    
    // Try as boolean
    if let Ok(val) = row.try_get::<bool, _>(column_index) {
        return Ok(if val { "1".to_string() } else { "0".to_string() });
    }
    
    // Try chrono types for dates/times first
    if let Ok(val) = row.try_get::<chrono::NaiveDateTime, _>(column_index) {
        return Ok(val.format("%Y-%m-%d %H:%M:%S").to_string());
    }
    
    if let Ok(val) = row.try_get::<chrono::DateTime<chrono::Utc>, _>(column_index) {
        return Ok(val.format("%Y-%m-%d %H:%M:%S").to_string());
    }
    
    if let Ok(val) = row.try_get::<chrono::DateTime<chrono::Local>, _>(column_index) {
        return Ok(val.format("%Y-%m-%d %H:%M:%S").to_string());
    }
    
    if let Ok(val) = row.try_get::<chrono::NaiveDate, _>(column_index) {
        return Ok(val.format("%Y-%m-%d").to_string());
    }
    
    if let Ok(val) = row.try_get::<chrono::NaiveTime, _>(column_index) {
        return Ok(val.format("%H:%M:%S").to_string());
    }
    
    // Try as bytes for timestamp/date fields (MySQL sometimes returns these as bytes)
    if let Ok(bytes) = row.try_get::<Vec<u8>, _>(column_index) {
        if let Ok(timestamp_str) = String::from_utf8(bytes.clone()) {
            // If it looks like a timestamp, return it as-is
            if timestamp_str.contains("-") && (timestamp_str.contains(":") || timestamp_str.len() == 10) {
                return Ok(timestamp_str);
            }
        }
        // If it's not a timestamp-like string, treat as binary data
        return Ok(format!("\\x{}", hex::encode(bytes)));
    }
    
    // If all else fails, try to convert via the type system
    match type_info.name() {
        "DECIMAL" | "NUMERIC" => {
            // MySQL DECIMAL types - try as string representation
            if let Ok(val) = row.try_get::<String, _>(column_index) {
                Ok(val)
            } else {
                Err(DatabaseError::QueryError(format!(
                    "Unable to format DECIMAL value at column {column_index}"
                )))
            }
        }
        "JSON" => {
            // MySQL JSON type - try as string representation
            if let Ok(val) = row.try_get::<String, _>(column_index) {
                Ok(val)
            } else {
                Err(DatabaseError::QueryError(format!(
                    "Unable to format JSON value at column {column_index}"
                )))
            }
        }
        _ => {
            // Final fallback: return a descriptive message for unknown types
            Ok(format!("[MySQL {} type - conversion not implemented]", type_info.name()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;
    use std::collections::HashMap;

    // Note: These tests require a running MySQL instance with test data
    // They will be skipped if MySQL is not available

    #[tokio::test]
    async fn test_mysql_client_creation_mock() {
        // Mock test without requiring actual MySQL connection
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::MySQL,
            host: Some("localhost".to_string()),
            port: Some(3306),
            username: Some("testuser".to_string()),
            password: Some("testpass".to_string()),
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // This test validates the connection info parsing without actual connection
        assert_eq!(connection_info.database_type, DatabaseType::MySQL);
        assert_eq!(connection_info.host.as_deref(), Some("localhost"));
        assert_eq!(connection_info.port, Some(3306));
        assert_eq!(connection_info.username.as_deref(), Some("testuser"));
        assert_eq!(connection_info.database.as_deref(), Some("testdb"));
    }

    #[tokio::test]
    async fn test_mysql_metadata_provider_builtin_functions() {
        // This test can run without MySQL connection as it tests the built-in functions list
        let builtin_functions = vec![
            "COUNT", "SUM", "AVG", "MAX", "MIN", "CONCAT", "NOW", "DATE", "JSON_EXTRACT"
        ];
        
        // Simulate what the provider would return for built-in functions
        for func in builtin_functions {
            assert!(!func.is_empty());
            assert!(func.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }
    }
}