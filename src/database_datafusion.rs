//! DataFusion implementation for file format support (Parquet, CSV, JSON)
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, DatabaseTypeExt, MetadataProvider,
    ServerInfo,
};
use crate::db::TableDetails;
use async_trait::async_trait;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::DataType;
use datafusion::datasource::file_format::options::{
    CsvReadOptions, NdJsonReadOptions, ParquetReadOptions,
};
use datafusion::execution::context::SessionContext;
use std::sync::Arc;
use tracing::debug;

/// DataFusion client for querying file formats (Parquet, CSV, JSON, etc.)
pub struct DataFusionClient {
    /// DataFusion session context (shared with metadata provider)
    ctx: Arc<SessionContext>,

    /// Connection information
    connection_info: ConnectionInfo,

    /// Registered tables (file paths -> table names)
    registered_tables: std::collections::HashMap<String, String>,

    /// Metadata provider
    metadata_provider: DataFusionMetadataProvider,

    /// Temporary files created for JSON conversion (kept alive during session)
    #[allow(dead_code)]
    temp_files: Vec<tempfile::NamedTempFile>,
}

impl DataFusionClient {
    /// Create a new DataFusionClient from connection info
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        debug!(
            "[DataFusionClient::new] Creating DataFusion client for {:?}",
            connection_info.database_type
        );

        // Create DataFusion session context with default config (wrapped in Arc for sharing)
        let ctx = Arc::new(SessionContext::new());

        // Create metadata provider
        let metadata_provider =
            DataFusionMetadataProvider::new(Arc::clone(&ctx), connection_info.clone());

        let mut client = Self {
            ctx,
            connection_info: connection_info.clone(),
            registered_tables: std::collections::HashMap::new(),
            metadata_provider,
            temp_files: Vec::new(),
        };

        // Register file(s) based on the file_path
        if let Some(ref file_path) = connection_info.file_path {
            client.register_file(file_path).await?;
        }

        Ok(client)
    }

    /// Register a file or glob pattern as a table
    async fn register_file(&mut self, path: &str) -> Result<(), DatabaseError> {
        debug!(
            "[DataFusionClient::register_file] Registering file: {}",
            path
        );

        // Check if path contains glob patterns
        let is_glob = path.contains('*') || path.contains('?') || path.contains('[');

        // Determine the actual path to register and table name
        let (register_path, table_name) = if is_glob {
            // For glob patterns, use the parent directory and name table after it
            let path_obj = std::path::Path::new(path);
            let dir_path = path_obj.parent().ok_or_else(|| {
                DatabaseError::ConnectionError("Invalid glob pattern path".to_string())
            })?;

            let table_name = dir_path
                .file_name()
                .and_then(|s| s.to_str())
                .map(Self::sanitize_table_name)
                .unwrap_or_else(|| "data".to_string());

            (dir_path.to_str().unwrap().to_string(), table_name)
        } else {
            // For single files, use the filename as table name
            let table_name = Self::extract_table_name(path);
            (path.to_string(), table_name)
        };

        debug!(
            "[DataFusionClient::register_file] Table name: {}, Path: {}",
            table_name, register_path
        );

        // Register based on database type using DataFusion's register_* methods
        // Methods on SessionContext take &self, so we can call them through the Arc
        match self.connection_info.database_type {
            DatabaseType::Parquet => {
                // Configure options to preserve column names from Parquet metadata
                let options = ParquetReadOptions {
                    skip_metadata: Some(false), // Preserve metadata including column names
                    ..Default::default()
                };

                Arc::as_ref(&self.ctx)
                    .register_parquet(&table_name, &register_path, options)
                    .await
                    .map_err(|e| {
                        DatabaseError::ConnectionError(format!(
                            "Failed to register Parquet file: {e}"
                        ))
                    })?;

                // Normalize column names to lowercase for case-insensitive access
                self.normalize_parquet_column_names(&table_name).await?;
            }
            DatabaseType::CSV => {
                // Build CSV options from query parameters
                let mut options = CsvReadOptions::default();

                // Check for has_header option
                if let Some(header) = self.connection_info.options.get("header") {
                    options.has_header = header.to_lowercase() == "true";
                }

                // Check for delimiter option
                if let Some(delimiter) = self.connection_info.options.get("delimiter") {
                    if let Some(c) = delimiter.chars().next() {
                        options.delimiter = c as u8;
                    }
                }

                Arc::as_ref(&self.ctx)
                    .register_csv(&table_name, &register_path, options)
                    .await
                    .map_err(|e| {
                        DatabaseError::ConnectionError(format!("Failed to register CSV file: {e}"))
                    })?;
            }
            DatabaseType::JSON => {
                // DataFusion's register_json only supports NDJSON (newline-delimited JSON)
                // Check file extension to determine if it's already NDJSON format
                let is_ndjson = register_path.ends_with(".jsonl")
                    || register_path.ends_with(".ndjson")
                    || register_path.ends_with(".json-lines");

                if is_ndjson {
                    // File is already NDJSON format - register directly without conversion
                    debug!("[DataFusionClient::register_file] Detected NDJSON file by extension");

                    // DataFusion requires .json extension, so create a symlink if needed
                    if !register_path.ends_with(".json") {
                        // Create a temporary .json symlink
                        let temp_dir = std::env::temp_dir();
                        let temp_path = temp_dir.join(format!("{table_name}.json"));

                        // Remove existing symlink if it exists
                        let _ = std::fs::remove_file(&temp_path);

                        // Create symlink (works on Unix-like systems)
                        #[cfg(unix)]
                        {
                            std::os::unix::fs::symlink(&register_path, &temp_path).map_err(
                                |e| {
                                    DatabaseError::ConnectionError(format!(
                                        "Failed to create symlink: {e}"
                                    ))
                                },
                            )?;
                        }

                        // On Windows or if symlink fails, copy the file
                        #[cfg(not(unix))]
                        {
                            std::fs::copy(&register_path, &temp_path).map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to copy file: {}",
                                    e
                                ))
                            })?;
                        }

                        Arc::as_ref(&self.ctx)
                            .register_json(
                                &table_name,
                                temp_path.to_str().unwrap(),
                                NdJsonReadOptions::default(),
                            )
                            .await
                            .map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to register NDJSON file: {e}"
                                ))
                            })?;
                    } else {
                        Arc::as_ref(&self.ctx)
                            .register_json(
                                &table_name,
                                &register_path,
                                NdJsonReadOptions::default(),
                            )
                            .await
                            .map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to register NDJSON file: {e}"
                                ))
                            })?;
                    }
                } else {
                    // Regular JSON file - try NDJSON first (fast path), then convert if needed
                    let result = Arc::as_ref(&self.ctx)
                        .register_json(&table_name, &register_path, NdJsonReadOptions::default())
                        .await;

                    // If that fails, try converting regular JSON to NDJSON
                    if result.is_err() {
                        debug!(
                            "[DataFusionClient::register_file] NDJSON registration failed, trying JSON conversion"
                        );

                        // Read the JSON file and convert to NDJSON
                        let json_content =
                            std::fs::read_to_string(&register_path).map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to read JSON file: {e}"
                                ))
                            })?;

                        let ndjson_content =
                            Self::convert_json_to_ndjson(&json_content).map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to convert JSON to NDJSON: {e}"
                                ))
                            })?;

                        // Write to temporary NDJSON file with .json extension
                        let temp_file = tempfile::Builder::new()
                            .suffix(".json")
                            .tempfile()
                            .map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to create temp file: {e}"
                                ))
                            })?;

                        std::fs::write(temp_file.path(), &ndjson_content).map_err(|e| {
                            DatabaseError::ConnectionError(format!(
                                "Failed to write temp NDJSON file: {e}"
                            ))
                        })?;

                        // Register the temporary NDJSON file
                        let temp_path = temp_file.path().to_str().unwrap().to_string();
                        Arc::as_ref(&self.ctx)
                            .register_json(&table_name, &temp_path, NdJsonReadOptions::default())
                            .await
                            .map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to register converted JSON file: {e}"
                                ))
                            })?;

                        // Keep the temp file alive by storing it
                        self.temp_files.push(temp_file);
                        debug!(
                            "[DataFusionClient::register_file] Successfully registered JSON file via NDJSON conversion"
                        );
                    } else {
                        debug!(
                            "[DataFusionClient::register_file] Successfully registered NDJSON file directly"
                        );
                    }
                }
            }
            _ => {
                return Err(DatabaseError::FeatureNotSupported {
                    database_type: self.connection_info.database_type.clone(),
                    feature: "file registration".to_string(),
                });
            }
        }

        self.registered_tables
            .insert(register_path.clone(), table_name.clone());
        debug!(
            "[DataFusionClient::register_file] Successfully registered table: {}",
            table_name
        );

        Ok(())
    }

    /// Normalize Parquet column names to lowercase for case-insensitive SQL access
    /// This allows unquoted identifiers to work: SELECT _col_20 instead of "_COL_20"
    async fn normalize_parquet_column_names(&self, table_name: &str) -> Result<(), DatabaseError> {
        // Get the registered table from the catalog
        let catalog = self
            .ctx
            .catalog("datafusion")
            .ok_or_else(|| DatabaseError::MetadataError("Catalog not found".to_string()))?;

        let schema_provider = catalog
            .schema("public")
            .ok_or_else(|| DatabaseError::MetadataError("Schema not found".to_string()))?;

        let table = schema_provider.table(table_name).await?.ok_or_else(|| {
            DatabaseError::MetadataError(format!("Table '{table_name}' not found"))
        })?;

        let table_schema = table.schema();

        // Check if any column names contain uppercase letters
        let needs_normalization = table_schema
            .fields()
            .iter()
            .any(|f| f.name().chars().any(|c| c.is_uppercase()));

        if !needs_normalization {
            debug!(
                "[DataFusionClient] Table '{}' column names already lowercase, skipping normalization",
                table_name
            );
            return Ok(());
        }

        debug!(
            "[DataFusionClient] Normalizing column names to lowercase for table '{}'",
            table_name
        );

        // Create a DataFrame from the table with lowercase column aliases
        let df = self
            .ctx
            .table(table_name)
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to access table: {e}")))?;

        // Build SELECT expressions with lowercase aliases
        use datafusion::prelude::*;
        let select_exprs: Vec<Expr> = table_schema
            .fields()
            .iter()
            .map(|field| {
                let original_name = field.name();
                let lowercase_name = original_name.to_lowercase();

                if original_name != &lowercase_name {
                    // Use quoted identifier to preserve exact case for uppercase columns
                    // This prevents DataFusion from lowercasing the column name
                    let quoted_col = format!("\"{original_name}\"");
                    col(quoted_col).alias(&lowercase_name)
                } else {
                    // No change needed - already lowercase
                    col(original_name)
                }
            })
            .collect();

        // Apply the projection to create normalized DataFrame
        let normalized_df = df
            .select(select_exprs)
            .map_err(|e| DatabaseError::QueryError(format!("Failed to normalize columns: {e}")))?;

        // Deregister the original table
        self.ctx.deregister_table(table_name).map_err(|e| {
            DatabaseError::QueryError(format!("Failed to deregister original table: {e}"))
        })?;

        // Register the normalized DataFrame with the same table name
        self.ctx
            .register_table(table_name, normalized_df.into_view())
            .map_err(|e| {
                DatabaseError::QueryError(format!("Failed to register normalized table: {e}"))
            })?;

        debug!(
            "[DataFusionClient] Successfully normalized column names for table '{}'",
            table_name
        );

        Ok(())
    }

    /// Extract table name from file path
    /// Converts filename to a valid SQL identifier by replacing invalid characters
    fn extract_table_name(path: &str) -> String {
        let path = std::path::Path::new(path);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("data");

        Self::sanitize_table_name(stem)
    }

    /// Sanitize a string to create a valid SQL table identifier
    fn sanitize_table_name(name: &str) -> String {
        // Replace invalid characters with underscores
        // SQL identifiers can't contain dots, dashes, spaces, etc.
        let table_name = name.replace(['.', '-', ' ', '/', '\\'], "_");

        // If name starts with a digit, prefix with underscore
        if table_name.chars().next().is_some_and(|c| c.is_numeric()) {
            format!("_{table_name}")
        } else {
            table_name
        }
    }

    /// Convert regular JSON (object or array) to NDJSON format
    /// Handles:
    /// - Single JSON objects: {"a": 1, "b": 2} -> {"a": 1, "b": 2}\n
    /// - JSON arrays: [{"a": 1}, {"a": 2}] -> {"a": 1}\n{"a": 2}\n
    /// - Nested structures: Extracts array elements or flattens nested objects
    fn convert_json_to_ndjson(json_str: &str) -> Result<String, String> {
        use serde_json::Value;

        let json_value: Value =
            serde_json::from_str(json_str).map_err(|e| format!("Failed to parse JSON: {e}"))?;

        let mut ndjson_lines = Vec::new();

        match &json_value {
            // If it's an array, each element becomes a line
            Value::Array(arr) => {
                for item in arr {
                    let line = serde_json::to_string(item)
                        .map_err(|e| format!("Failed to serialize array item: {e}"))?;
                    ndjson_lines.push(line);
                }
            }
            // If it's an object, check if it has nested data
            Value::Object(obj) => {
                // Try to find an array field (common in API responses)
                let mut found_array = false;
                for (_key, value) in obj {
                    if let Value::Array(arr) = value {
                        // Check if array contains objects (not primitives)
                        let has_objects = arr.iter().any(|item| matches!(item, Value::Object(_)));
                        if has_objects {
                            // Use the first array with objects found
                            for item in arr {
                                if matches!(item, Value::Object(_)) {
                                    let line = serde_json::to_string(item).map_err(|e| {
                                        format!("Failed to serialize nested array item: {e}")
                                    })?;
                                    ndjson_lines.push(line);
                                }
                            }
                            found_array = true;
                            break;
                        }
                    }
                }

                // If no suitable array found, use entire object as single record
                if !found_array {
                    // Preserve the complete object structure including all fields
                    // This allows access to both top-level fields and nested structures
                    // Example: request_id, lease_id, data.chroot_namespace, etc.
                    let line = serde_json::to_string(&json_value)
                        .map_err(|e| format!("Failed to serialize object: {e}"))?;
                    ndjson_lines.push(line);
                }
            }
            // For primitive values, wrap in an object
            _ => {
                let wrapped = serde_json::json!({"value": json_value});
                let line = serde_json::to_string(&wrapped)
                    .map_err(|e| format!("Failed to serialize primitive value: {e}"))?;
                ndjson_lines.push(line);
            }
        }

        if ndjson_lines.is_empty() {
            return Err("No data to convert".to_string());
        }

        Ok(ndjson_lines.join("\n") + "\n")
    }

    /// Execute a DataFusion query and convert results to Vec<Vec<String>>
    async fn execute_datafusion_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[DataFusionClient::execute_datafusion_query] Executing query: {}",
            sql
        );

        // Execute query
        let df = self
            .ctx
            .sql(sql)
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to execute query: {e}")))?;

        // Collect results
        let batches = df
            .collect()
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to collect results: {e}")))?;

        // Convert to Vec<Vec<String>>
        Self::record_batches_to_strings(&batches)
    }

    /// Convert Arrow RecordBatches to Vec<Vec<String>>
    fn record_batches_to_strings(
        batches: &[RecordBatch],
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        if batches.is_empty() {
            return Ok(vec![]);
        }

        let schema = batches[0].schema();
        let mut results = Vec::new();

        // Add header row
        let headers: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        results.push(headers);

        // Add data rows
        for batch in batches {
            for row_idx in 0..batch.num_rows() {
                let mut row = Vec::new();
                for col_idx in 0..batch.num_columns() {
                    let column = batch.column(col_idx);
                    let value = Self::array_value_to_string(column, row_idx);
                    row.push(value);
                }
                results.push(row);
            }
        }

        Ok(results)
    }

    /// Convert an Arrow array value to String
    fn array_value_to_string(
        array: &Arc<dyn datafusion::arrow::array::Array>,
        row_idx: usize,
    ) -> String {
        use datafusion::arrow::array::*;

        if array.is_null(row_idx) {
            return "NULL".to_string();
        }

        // Handle different data types
        match array.data_type() {
            DataType::Int8 => {
                let arr = array.as_any().downcast_ref::<Int8Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Int16 => {
                let arr = array.as_any().downcast_ref::<Int16Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Int32 => {
                let arr = array.as_any().downcast_ref::<Int32Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Int64 => {
                let arr = array.as_any().downcast_ref::<Int64Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::UInt8 => {
                let arr = array.as_any().downcast_ref::<UInt8Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::UInt16 => {
                let arr = array.as_any().downcast_ref::<UInt16Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::UInt32 => {
                let arr = array.as_any().downcast_ref::<UInt32Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::UInt64 => {
                let arr = array.as_any().downcast_ref::<UInt64Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Float32 => {
                let arr = array.as_any().downcast_ref::<Float32Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Float64 => {
                let arr = array.as_any().downcast_ref::<Float64Array>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Boolean => {
                let arr = array.as_any().downcast_ref::<BooleanArray>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Utf8 => {
                let arr = array.as_any().downcast_ref::<StringArray>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Utf8View => {
                let arr = array.as_any().downcast_ref::<StringViewArray>().unwrap();
                arr.value(row_idx).to_string()
            }
            DataType::Date32 | DataType::Date64 | DataType::Timestamp(_, _) => {
                // Format dates/timestamps
                format!("{array:?}")
            }
            _ => {
                // Fallback for other types
                format!("{array:?}")
            }
        }
    }
}

/// DataFusion metadata provider implementation
pub struct DataFusionMetadataProvider {
    ctx: Arc<SessionContext>,
    #[allow(dead_code)] // May be used for future features
    connection_info: ConnectionInfo,
}

impl DataFusionMetadataProvider {
    pub fn new(ctx: Arc<SessionContext>, connection_info: ConnectionInfo) -> Self {
        Self {
            ctx,
            connection_info,
        }
    }

    /// Helper function to simplify Arrow DataType for readable display
    /// Returns (summary, optional_field_details)
    fn simplify_datatype_for_display(data_type: &DataType) -> (String, Option<Vec<String>>) {
        match data_type {
            DataType::Struct(fields) if !fields.is_empty() => {
                let summary = format!("Struct<{} fields>", fields.len());

                // Extract field details for separate display
                let max_fields = 10;
                let mut details = Vec::new();
                for field in fields.iter().take(max_fields) {
                    let field_type = Self::simplify_datatype_for_display_simple(field.data_type());
                    details.push(format!("  - {}: {}", field.name(), field_type));
                }

                if fields.len() > max_fields {
                    details.push(format!(
                        "  ... and {} more fields",
                        fields.len() - max_fields
                    ));
                }

                (summary, Some(details))
            }
            _ => (Self::simplify_datatype_for_display_simple(data_type), None),
        }
    }

    /// Simplified type display without nested expansion (for inline display)
    fn simplify_datatype_for_display_simple(data_type: &DataType) -> String {
        match data_type {
            DataType::Struct(fields) => {
                format!("Struct<{} fields>", fields.len())
            }
            DataType::List(field) => {
                format!(
                    "List<{}>",
                    Self::simplify_datatype_for_display_simple(field.data_type())
                )
            }
            DataType::LargeList(field) => {
                format!(
                    "LargeList<{}>",
                    Self::simplify_datatype_for_display_simple(field.data_type())
                )
            }
            DataType::Map(entries, _) => {
                format!(
                    "Map<{}>",
                    Self::simplify_datatype_for_display_simple(entries.data_type())
                )
            }
            // Simple types - use debug format
            _ => format!("{data_type:?}"),
        }
    }

    /// Extract all field paths including nested struct fields
    /// Returns paths like ["data", "data.chroot_namespace", "data.exact_paths"]
    fn extract_all_field_paths(schema: &Arc<datafusion::arrow::datatypes::Schema>) -> Vec<String> {
        let mut paths = Vec::new();
        for field in schema.fields() {
            Self::extract_nested_field_paths(field, "", &mut paths);
        }
        paths
    }

    /// Recursively extract nested field paths from an Arrow Field
    fn extract_nested_field_paths(
        field: &datafusion::arrow::datatypes::Field,
        prefix: &str,
        paths: &mut Vec<String>,
    ) {
        let field_path = if prefix.is_empty() {
            field.name().clone()
        } else {
            format!("{}.{}", prefix, field.name())
        };

        paths.push(field_path.clone());

        // Recurse into struct fields
        if let DataType::Struct(nested_fields) = field.data_type() {
            for nested_field in nested_fields {
                Self::extract_nested_field_paths(nested_field, &field_path, paths);
            }
        }
    }
}

#[async_trait]
impl MetadataProvider for DataFusionMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        // DataFusion has a single default schema
        Ok(vec!["datafusion".to_string()])
    }

    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // Get registered tables from catalog
        let catalog = self
            .ctx
            .catalog("datafusion")
            .ok_or_else(|| DatabaseError::MetadataError("Default catalog not found".to_string()))?;

        let schema = catalog
            .schema("public")
            .ok_or_else(|| DatabaseError::MetadataError("Default schema not found".to_string()))?;

        let table_names: Vec<String> = schema.table_names();
        Ok(table_names)
    }

    async fn get_columns(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        // Get table schema from catalog
        let catalog = self
            .ctx
            .catalog("datafusion")
            .ok_or_else(|| DatabaseError::MetadataError("Default catalog not found".to_string()))?;

        let schema = catalog
            .schema("public")
            .ok_or_else(|| DatabaseError::MetadataError("Default schema not found".to_string()))?;

        let table_provider = schema
            .table(table)
            .await?
            .ok_or_else(|| DatabaseError::MetadataError(format!("Table '{table}' not found")))?;

        let table_schema = table_provider.schema();

        // Return all field paths including nested struct fields for autocomplete
        Ok(Self::extract_all_field_paths(&table_schema))
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // DataFusion has built-in functions
        // For now, return empty list - can be enhanced later
        Ok(vec![])
    }

    async fn get_table_details(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        // Get basic table information
        let catalog = self
            .ctx
            .catalog("datafusion")
            .ok_or_else(|| DatabaseError::MetadataError("Default catalog not found".to_string()))?;

        let schema = catalog
            .schema("public")
            .ok_or_else(|| DatabaseError::MetadataError("Default schema not found".to_string()))?;

        let table_provider = schema
            .table(table)
            .await?
            .ok_or_else(|| DatabaseError::MetadataError(format!("Table '{table}' not found")))?;

        let table_schema = table_provider.schema();

        // Convert Arrow schema to TableDetails and collect nested field details
        let mut nested_field_details = std::collections::HashMap::new();
        let columns: Vec<crate::db::ColumnInfo> = table_schema
            .fields()
            .iter()
            .map(|f| {
                let (type_summary, details) = Self::simplify_datatype_for_display(f.data_type());

                // Store nested field details if present
                if let Some(details) = details {
                    nested_field_details.insert(f.name().clone(), details);
                }

                crate::db::ColumnInfo {
                    name: f.name().clone(),
                    data_type: type_summary,
                    collation: String::new(),
                    nullable: f.is_nullable(),
                    default_value: None,
                    enum_values: None,
                }
            })
            .collect();

        Ok(TableDetails {
            name: table.to_string(),
            schema: "public".to_string(),
            full_name: format!("public.{table}"),
            columns,
            nested_field_details,
            indexes: vec![],
            check_constraints: vec![],
            foreign_keys: vec![],
            referenced_by: vec![],
        })
    }

    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        Some("public".to_string())
    }
}

#[async_trait]
impl DatabaseClient for DataFusionClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.execute_datafusion_query(sql).await
    }

    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        // For DataFusion, just try to parse the query
        self.ctx
            .sql(sql)
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Query validation failed: {e}")))?;
        Ok(())
    }

    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        let explain_sql = format!("EXPLAIN {sql}");
        self.execute_datafusion_query(&explain_sql).await
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        // DataFusion doesn't have JSON EXPLAIN, so return same as explain_query
        self.explain_query(sql).await
    }

    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        // DataFusion doesn't have multiple databases concept
        Ok(vec![
            vec!["Name".to_string()],
            vec!["datafusion".to_string()],
        ])
    }

    async fn connect_to_database(&mut self, _database: &str) -> Result<(), DatabaseError> {
        // DataFusion doesn't support switching databases
        Err(DatabaseError::FeatureNotSupported {
            database_type: self.connection_info.database_type.clone(),
            feature: "database switching".to_string(),
        })
    }

    fn get_current_database(&self) -> String {
        // Return the file name or "datafusion" as database name
        self.connection_info
            .file_path
            .as_ref()
            .map(|p| Self::extract_table_name(p))
            .unwrap_or_else(|| "datafusion".to_string())
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        // For file-based databases, check if files are still accessible
        true
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // DataFusion doesn't need explicit cleanup
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        Ok(ServerInfo::new(
            self.connection_info
                .database_type
                .display_name()
                .to_string(),
            "DataFusion 50.0.0".to_string(),
        ))
    }
}
