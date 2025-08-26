//! ClickHouse implementation of the database abstraction layer
use crate::complex_display::{
    ArrayDisplayAdapter, ComplexDataDisplay, ComplexDataType, ComplexDisplayConfig,
    ComplexTypeDetector, GenericComplexTypeDetector,
};
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, MetadataProvider, ServerInfo,
};
use crate::db::TableDetails;
use crate::geojson_display::GeoJsonDisplayAdapter;
use crate::json_display::JsonDisplayAdapter;
use async_trait::async_trait;
use clickhouse::{Client, Row};
use serde::Deserialize;
use tracing::debug;

/// ClickHouse metadata provider implementation
pub struct ClickHouseMetadataProvider {
    client: Client,
}

impl ClickHouseMetadataProvider {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl MetadataProvider for ClickHouseMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        debug!("[ClickHouseMetadataProvider::get_schemas] Starting query");

        // ClickHouse databases are similar to schemas
        let query = "SELECT name FROM system.databases WHERE name NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') ORDER BY name";

        #[derive(Deserialize, Row)]
        struct DatabaseName {
            name: String,
        }

        let databases = self
            .client
            .query(query)
            .fetch_all::<DatabaseName>()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get schemas: {e}")))?;

        let schemas: Vec<String> = databases.into_iter().map(|db| db.name).collect();

        debug!(
            "[ClickHouseMetadataProvider::get_schemas] Found {} schemas",
            schemas.len()
        );
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[ClickHouseMetadataProvider::get_tables] Starting query for schema: {:?}",
            schema
        );

        let query = if let Some(schema_name) = schema {
            format!(
                "SELECT name FROM system.tables WHERE database = '{}' ORDER BY name",
                schema_name
            )
        } else {
            "SELECT name FROM system.tables WHERE database = currentDatabase() ORDER BY name"
                .to_string()
        };

        #[derive(Deserialize, Row)]
        struct TableName {
            name: String,
        }

        let tables = self
            .client
            .query(&query)
            .fetch_all::<TableName>()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get tables: {e}")))?;

        let table_names: Vec<String> = tables.into_iter().map(|table| table.name).collect();

        debug!(
            "[ClickHouseMetadataProvider::get_tables] Found {} tables",
            table_names.len()
        );
        Ok(table_names)
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        debug!(
            "[ClickHouseMetadataProvider::get_columns] Getting columns for table: {} in schema: {:?}",
            table, schema
        );

        let query = if let Some(schema_name) = schema {
            format!(
                "SELECT name FROM system.columns WHERE database = '{}' AND table = '{}' ORDER BY position",
                schema_name, table
            )
        } else {
            format!(
                "SELECT name FROM system.columns WHERE database = currentDatabase() AND table = '{}' ORDER BY position",
                table
            )
        };

        #[derive(Deserialize, Row)]
        struct ColumnName {
            name: String,
        }

        let columns = self
            .client
            .query(&query)
            .fetch_all::<ColumnName>()
            .await
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to get columns for table {}: {e}", table))
            })?;

        let column_names: Vec<String> = columns.into_iter().map(|col| col.name).collect();

        debug!(
            "[ClickHouseMetadataProvider::get_columns] Found {} columns for table {}",
            column_names.len(),
            table
        );
        Ok(column_names)
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        debug!("[ClickHouseMetadataProvider::get_functions] Getting ClickHouse functions");

        // ClickHouse has many built-in functions
        let query = "SELECT name FROM system.functions WHERE origin = 'System' ORDER BY name";

        #[derive(Deserialize, Row)]
        struct FunctionName {
            name: String,
        }

        let functions = self
            .client
            .query(query)
            .fetch_all::<FunctionName>()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get functions: {e}")))?;

        let function_names: Vec<String> = functions.into_iter().map(|func| func.name).collect();

        debug!(
            "[ClickHouseMetadataProvider::get_functions] Found {} functions",
            function_names.len()
        );
        Ok(function_names)
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        debug!(
            "[ClickHouseMetadataProvider::get_table_details] Getting details for table: {} in schema: {:?}",
            table, schema
        );

        let database_name = schema.unwrap_or("default");
        let schema_name = schema.unwrap_or("default");

        // Get column information
        let columns_query = if schema.is_some() {
            format!(
                "SELECT name, type, default_expression, is_in_primary_key FROM system.columns WHERE database = '{}' AND table = '{}' ORDER BY position",
                database_name, table
            )
        } else {
            format!(
                "SELECT name, type, default_expression, is_in_primary_key FROM system.columns WHERE database = currentDatabase() AND table = '{}' ORDER BY position",
                table
            )
        };

        #[derive(Deserialize, Row)]
        struct ColumnDetail {
            name: String,
            #[serde(rename = "type")]
            data_type: String,
            default_expression: String,
            #[allow(dead_code)] // May be used in future for primary key detection
            is_in_primary_key: u8,
        }

        let columns = self
            .client
            .query(&columns_query)
            .fetch_all::<ColumnDetail>()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get column details: {e}")))?;

        let column_infos: Vec<crate::db::ColumnInfo> = columns
            .into_iter()
            .map(|col| crate::db::ColumnInfo {
                name: col.name,
                data_type: col.data_type,
                collation: String::new(), // ClickHouse doesn't use collations like other DBs
                nullable: true,           // ClickHouse columns are nullable by default
                default_value: if col.default_expression.is_empty() {
                    None
                } else {
                    Some(col.default_expression)
                },
            })
            .collect();

        // ClickHouse doesn't have traditional indexes like other databases
        // Instead, it has primary keys, sorting keys, and data skipping indexes
        let indexes: Vec<crate::db::IndexInfo> = Vec::new();

        // ClickHouse doesn't have traditional check constraints
        let check_constraints: Vec<crate::db::CheckConstraintInfo> = Vec::new();

        // ClickHouse doesn't have traditional foreign keys
        let foreign_keys: Vec<crate::db::ForeignKeyInfo> = Vec::new();
        let referenced_by: Vec<crate::db::ReferencedByInfo> = Vec::new();

        Ok(TableDetails {
            name: table.to_string(),
            schema: schema_name.to_string(),
            full_name: format!("{}.{}", schema_name, table),
            columns: column_infos,
            indexes,
            check_constraints,
            foreign_keys,
            referenced_by,
        })
    }

    fn supports_explain(&self) -> bool {
        true // ClickHouse supports EXPLAIN
    }

    fn default_schema(&self) -> Option<String> {
        Some("default".to_string()) // ClickHouse default database is "default"
    }
}

/// ClickHouse database client implementation
pub struct ClickHouseClient {
    client: Client,
    connection_info: ConnectionInfo,
    current_database: String,
    metadata_provider: ClickHouseMetadataProvider,
}

impl ClickHouseClient {
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        debug!("[ClickHouseClient::new] Creating ClickHouse client");

        // Build connection URL
        let host = connection_info.host.as_deref().unwrap_or("localhost");
        let port = connection_info.port.unwrap_or(8123);
        let username = connection_info.username.as_deref().unwrap_or("");
        let database = connection_info
            .database
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // ClickHouse HTTP interface doesn't use database in URL path
        // Database is specified via query parameter or USE statement
        let database_url = if let Some(password) = &connection_info.password {
            if username.is_empty() {
                format!("http://{}:{}", host, port)
            } else {
                format!("http://{}:{}@{}:{}", username, password, host, port)
            }
        } else {
            if username.is_empty() {
                format!("http://{}:{}", host, port)
            } else {
                format!("http://{}@{}:{}", username, host, port)
            }
        };

        debug!(
            "[ClickHouseClient::new] Connecting to: {}",
            crate::password_sanitizer::sanitize_connection_url(&database_url)
        );

        let client = Client::default()
            .with_url(database_url)
            .with_database(&database);

        // Test the connection with a simple ping query
        client
            .query("SELECT 1 as test")
            .fetch_one::<u8>()
            .await
            .map_err(|e| {
                DatabaseError::ConnectionError(format!("Failed to connect to ClickHouse: {e}"))
            })?;

        let metadata_provider = ClickHouseMetadataProvider::new(client.clone());

        Ok(Self {
            client,
            connection_info,
            current_database: database,
            metadata_provider,
        })
    }

    /// Execute HTTP query via ClickHouse HTTP interface
    async fn execute_http_user_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        // Build HTTP URL
        let host = self.connection_info.host.as_deref().unwrap_or("localhost");
        let port = self.connection_info.port.unwrap_or(8123);
        let url = format!("http://{}:{}", host, port);

        debug!(
            "[ClickHouseClient::execute_http_user_query] Executing HTTP query: {}",
            sql
        );
        debug!(
            "[ClickHouseClient::execute_http_user_query] Target URL: {}",
            url
        );
        debug!(
            "[ClickHouseClient::execute_http_user_query] Username: {:?}, Has password: {}",
            self.connection_info.username,
            self.connection_info.password.is_some()
        );

        // Create HTTP client
        let client = reqwest::Client::new();

        // Try with TabSeparatedWithNames first (for SELECT queries)
        let formatted_sql = format!("{} FORMAT TabSeparatedWithNames", sql.trim_end_matches(';'));

        // Build request
        let mut request = client.post(&url);

        // Add authentication if needed
        if let Some(username) = &self.connection_info.username {
            if let Some(password) = &self.connection_info.password {
                debug!(
                    "[ClickHouseClient::execute_http_user_query] Adding basic auth with username '{}' and password",
                    username
                );
                request = request.basic_auth(username, Some(password));
            } else {
                debug!(
                    "[ClickHouseClient::execute_http_user_query] Adding basic auth with username '{}' and no password",
                    username
                );
                request = request.basic_auth(username, None::<&str>);
            }
        }

        // Add database parameter
        if let Some(database) = &self.connection_info.database {
            request = request.query(&[("database", database)]);
        }

        // Execute request
        let response = request
            .body(formatted_sql.clone())
            .send()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("HTTP request failed: {e}")))?;

        if response.status().is_success() {
            // Parse TabSeparatedWithNames response
            let text_response = response
                .text()
                .await
                .map_err(|e| DatabaseError::QueryError(format!("Failed to read response: {e}")))?;

            debug!(
                "[ClickHouseClient::execute_http_user_query] Raw response: {}",
                text_response
            );

            let mut results = Vec::new();
            let lines: Vec<&str> = text_response.lines().collect();

            if lines.is_empty() {
                return Ok(vec![vec!["(no results)".to_string()]]);
            }

            // First line is headers
            if let Some(header_line) = lines.first() {
                let headers: Vec<String> = header_line
                    .split('\t')
                    .map(|s| s.trim().to_string())
                    .collect();
                results.push(headers);
            }

            // Remaining lines are data
            for line in lines.iter().skip(1) {
                if !line.trim().is_empty() {
                    let raw_row_data: Vec<String> =
                        line.split('\t').map(|s| s.trim().to_string()).collect();

                    // Apply complex display formatting if headers are available
                    let formatted_row_data = if let Some(headers) = results.first() {
                        self.format_row_with_complex_display(&raw_row_data, headers)
                    } else {
                        raw_row_data
                    };

                    results.push(formatted_row_data);
                }
            }

            // If only headers and no data rows
            if results.len() == 1 {
                results.push(vec!["(no rows)".to_string()]);
            }

            Ok(results)
        } else {
            // Try without FORMAT (for non-SELECT queries like DDL/DML)
            debug!(
                "[ClickHouseClient::execute_http_user_query] Trying without FORMAT for DDL/DML query"
            );

            let mut request = client.post(&url);

            // Add authentication if needed
            if let Some(username) = &self.connection_info.username {
                if let Some(password) = &self.connection_info.password {
                    request = request.basic_auth(username, Some(password));
                } else {
                    request = request.basic_auth(username, None::<&str>);
                }
            }

            // Add database parameter
            if let Some(database) = &self.connection_info.database {
                request = request.query(&[("database", database)]);
            }

            // Execute without FORMAT
            let response = request
                .body(sql.to_string())
                .send()
                .await
                .map_err(|e| DatabaseError::QueryError(format!("HTTP request failed: {e}")))?;

            if response.status().is_success() {
                // DDL/DML query succeeded
                Ok(vec![
                    vec!["Status".to_string()],
                    vec!["Query executed successfully".to_string()],
                ])
            } else {
                let error_text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string());
                Err(DatabaseError::QueryError(format!(
                    "ClickHouse HTTP error: {}",
                    error_text
                )))
            }
        }
    }

    /// Execute a raw query and return results as Vec<Vec<String>>
    async fn execute_raw_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[ClickHouseClient::execute_raw_query] Executing query: {}",
            sql
        );

        // Use HTTP interface for all user queries to handle dynamic results
        self.execute_http_user_query(sql).await
    }

    /// Format a row of data with complex display adapters
    fn format_row_with_complex_display(
        &self,
        row_data: &[String],
        headers: &[String],
    ) -> Vec<String> {
        row_data
            .iter()
            .zip(headers.iter())
            .map(|(value, column_name)| self.format_value_with_complex_display(value, column_name))
            .collect()
    }

    /// Format a single value using complex display adapters if applicable
    fn format_value_with_complex_display(&self, value: &str, column_name: &str) -> String {
        // Check if this value should use complex display
        if GenericComplexTypeDetector::should_use_complex_display(column_name, value) {
            if let Some(detected_type) = GenericComplexTypeDetector::detect_type(value) {
                return self.format_complex_value(value, detected_type);
            }
        }

        // Special ClickHouse type detection
        if self.is_clickhouse_array(value) || self.is_clickhouse_tuple(value) {
            return self.format_clickhouse_complex_type(value, column_name);
        }

        value.to_string()
    }

    /// Check if value looks like a ClickHouse array: [1,2,3]
    fn is_clickhouse_array(&self, value: &str) -> bool {
        value.starts_with('[') && value.ends_with(']') && !value.contains('"')
    }

    /// Check if value looks like a ClickHouse tuple: (1,'hello',true)
    fn is_clickhouse_tuple(&self, value: &str) -> bool {
        value.starts_with('(') && value.ends_with(')') && value.contains(',')
    }

    /// Format ClickHouse-specific complex types (arrays, tuples, maps)
    fn format_clickhouse_complex_type(&self, value: &str, _column_name: &str) -> String {
        if self.is_clickhouse_array(value) {
            // Convert ClickHouse array to our ArrayDisplayAdapter
            if let Ok(elements) = self.parse_clickhouse_array(value) {
                let adapter = ArrayDisplayAdapter::new(elements);
                let config = ComplexDisplayConfig::default();
                return adapter.format(&config);
            }
        } else if self.is_clickhouse_tuple(value) {
            // For tuples, create a structured display
            if let Ok(elements) = self.parse_clickhouse_tuple(value) {
                let adapter =
                    ArrayDisplayAdapter::new(elements).with_type_hint("Tuple".to_string());
                let config = ComplexDisplayConfig::default();
                return adapter.format(&config);
            }
        }

        value.to_string()
    }

    /// Parse ClickHouse array format: [1,2,3] or ['a','b','c']
    fn parse_clickhouse_array(&self, value: &str) -> Result<Vec<String>, String> {
        let inner = value.trim_start_matches('[').trim_end_matches(']');
        if inner.trim().is_empty() {
            return Ok(vec![]);
        }

        // Simple parsing - split by comma and clean up
        let elements: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
            .collect();

        Ok(elements)
    }

    /// Parse ClickHouse tuple format: (1,'hello',true)
    fn parse_clickhouse_tuple(&self, value: &str) -> Result<Vec<String>, String> {
        let inner = value.trim_start_matches('(').trim_end_matches(')');
        if inner.trim().is_empty() {
            return Ok(vec![]);
        }

        // Simple parsing - split by comma and clean up
        let elements: Vec<String> = inner
            .split(',')
            .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
            .collect();

        Ok(elements)
    }

    /// Format a value using the appropriate complex display adapter
    fn format_complex_value(&self, value: &str, data_type: ComplexDataType) -> String {
        let config = ComplexDisplayConfig::default();

        match data_type {
            ComplexDataType::Json => {
                if let Ok(adapter) = JsonDisplayAdapter::new(value.to_string()) {
                    adapter.format(&config)
                } else {
                    value.to_string()
                }
            }
            ComplexDataType::GeoJson => {
                if let Ok(adapter) = GeoJsonDisplayAdapter::new(value.to_string()) {
                    adapter.format(&config)
                } else {
                    value.to_string()
                }
            }
            ComplexDataType::Array | ComplexDataType::Vector => {
                if let Ok(adapter) = ArrayDisplayAdapter::from_json_string(value) {
                    adapter.format(&config)
                } else {
                    value.to_string()
                }
            }
            _ => value.to_string(), // Fallback for unsupported types
        }
    }
}

#[async_trait]
impl DatabaseClient for ClickHouseClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.execute_raw_query(sql).await
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        debug!("[ClickHouseClient::test_query] Testing query: {}", sql);

        // For ClickHouse, we can test by trying to explain the query
        let explain_query = format!("EXPLAIN {}", sql);
        self.client
            .query(&explain_query)
            .fetch_one::<String>()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Query validation failed: {e}")))?;

        Ok(())
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[ClickHouseClient::explain_query] Explaining query: {}",
            sql
        );

        let explain_sql = format!("EXPLAIN PLAN {}", sql);
        self.execute_raw_query(&explain_sql).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[ClickHouseClient::explain_query_raw] Raw explain for query: {}",
            sql
        );

        // ClickHouse EXPLAIN with more details
        let explain_sql = format!("EXPLAIN SYNTAX {}", sql);
        self.execute_raw_query(&explain_sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!("[ClickHouseClient::list_databases] Listing databases");

        let query = "SELECT name FROM system.databases ORDER BY name";
        self.execute_raw_query(query).await
    }

    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        debug!(
            "[ClickHouseClient::connect_to_database] Switching to database: {}",
            database
        );

        // ClickHouse doesn't have a USE statement like MySQL
        // Instead, we need to create a new client with the new database
        let mut new_connection_info = self.connection_info.clone();
        new_connection_info.database = Some(database.to_string());

        let new_client = Self::new(new_connection_info).await?;

        // Replace our client with the new one
        self.client = new_client.client;
        self.connection_info = new_client.connection_info;
        self.current_database = database.to_string();
        self.metadata_provider = new_client.metadata_provider;

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
        // Test connection with a simple query
        match self.client.query("SELECT 1").fetch_one::<u8>().await {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        debug!("[ClickHouseClient::close] Closing ClickHouse connection");
        // ClickHouse client doesn't need explicit closing
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        debug!("[ClickHouseClient::get_server_info] Getting server info");

        // Get ClickHouse version
        #[derive(Deserialize, Row)]
        struct VersionInfo {
            version: String,
        }

        let version_result = self
            .client
            .query("SELECT version() as version")
            .fetch_one::<VersionInfo>()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to get version: {e}")))?;

        let mut server_info = ServerInfo::new("ClickHouse".to_string(), version_result.version);
        server_info.parse_version_numbers();
        server_info.supports_transactions = false; // ClickHouse doesn't support traditional transactions
        server_info.supports_roles = true; // ClickHouse supports role-based access control

        Ok(server_info)
    }
}
