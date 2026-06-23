//! DataFusion implementation for file format support (Parquet, CSV, JSON)
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, DatabaseTypeExt, MetadataProvider,
    ServerInfo,
};
use crate::db::TableDetails;
use async_trait::async_trait;
use datafusion::arrow::array::{
    BinaryArray, BinaryViewArray, FixedSizeBinaryArray, LargeBinaryArray, LargeStringArray,
    RecordBatch, StringArray, StringViewArray,
};
use datafusion::arrow::datatypes::DataType;
use datafusion::datasource::file_format::options::{
    CsvReadOptions, JsonReadOptions, ParquetReadOptions,
};
use datafusion::execution::SessionStateBuilder;
use datafusion::execution::context::SessionContext;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::SessionConfig;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;

const DEFAULT_DATAFUSION_MEMORY_LIMIT_BYTES: usize = 512 * 1024 * 1024;
const DEFAULT_DATAFUSION_BATCH_SIZE: usize = 256;
const DEFAULT_DATAFUSION_MAX_RESULT_ROWS: usize = 10_000;
const DEFAULT_DATAFUSION_MAX_CELL_CHARS: usize = 2_048;
const DEFAULT_DATAFUSION_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_DATAFUSION_MAX_QUERY_SECONDS: usize = 60;

#[derive(Debug, Clone)]
struct DataFusionSafetyLimits {
    memory_limit_bytes: usize,
    target_partitions: usize,
    batch_size: usize,
    max_result_rows: usize,
    max_cell_chars: usize,
    max_output_bytes: usize,
    max_query_seconds: usize,
}

impl DataFusionSafetyLimits {
    fn from_connection_info(connection_info: &ConnectionInfo) -> Self {
        let default_partitions = std::thread::available_parallelism()
            .map(|parallelism| parallelism.get().min(2))
            .unwrap_or(2)
            .max(1);

        Self {
            memory_limit_bytes: Self::setting(
                &connection_info.options,
                "datafusion_memory_limit_bytes",
                "DBCRUST_DATAFUSION_MEMORY_LIMIT_BYTES",
                DEFAULT_DATAFUSION_MEMORY_LIMIT_BYTES,
                1,
            ),
            target_partitions: Self::setting(
                &connection_info.options,
                "datafusion_target_partitions",
                "DBCRUST_DATAFUSION_TARGET_PARTITIONS",
                default_partitions,
                1,
            ),
            batch_size: Self::setting(
                &connection_info.options,
                "datafusion_batch_size",
                "DBCRUST_DATAFUSION_BATCH_SIZE",
                DEFAULT_DATAFUSION_BATCH_SIZE,
                1,
            ),
            max_result_rows: Self::setting(
                &connection_info.options,
                "datafusion_max_result_rows",
                "DBCRUST_DATAFUSION_MAX_RESULT_ROWS",
                DEFAULT_DATAFUSION_MAX_RESULT_ROWS,
                0,
            ),
            max_cell_chars: Self::setting(
                &connection_info.options,
                "datafusion_max_cell_chars",
                "DBCRUST_DATAFUSION_MAX_CELL_CHARS",
                DEFAULT_DATAFUSION_MAX_CELL_CHARS,
                16,
            ),
            max_output_bytes: Self::setting(
                &connection_info.options,
                "datafusion_max_output_bytes",
                "DBCRUST_DATAFUSION_MAX_OUTPUT_BYTES",
                DEFAULT_DATAFUSION_MAX_OUTPUT_BYTES,
                1024,
            ),
            max_query_seconds: Self::setting(
                &connection_info.options,
                "datafusion_max_query_seconds",
                "DBCRUST_DATAFUSION_MAX_QUERY_SECONDS",
                DEFAULT_DATAFUSION_MAX_QUERY_SECONDS,
                1,
            ),
        }
    }

    fn setting(
        options: &std::collections::HashMap<String, String>,
        option_key: &str,
        env_key: &str,
        default_value: usize,
        min_value: usize,
    ) -> usize {
        let configured = options
            .get(option_key)
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                std::env::var(env_key)
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(default_value);

        configured.max(min_value)
    }
}

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

    /// Conservative DataFusion resource and output safety limits
    safety_limits: DataFusionSafetyLimits,

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

        let safety_limits = DataFusionSafetyLimits::from_connection_info(&connection_info);
        debug!(
            "[DataFusionClient::new] Safety limits: memory={} bytes, partitions={}, batch_size={}, max_rows={}, max_cell_chars={}, max_output={} bytes, max_query_seconds={}",
            safety_limits.memory_limit_bytes,
            safety_limits.target_partitions,
            safety_limits.batch_size,
            safety_limits.max_result_rows,
            safety_limits.max_cell_chars,
            safety_limits.max_output_bytes,
            safety_limits.max_query_seconds
        );

        // Create DataFusion session context with conservative resource limits.
        // `SessionContext::new()` uses DataFusion defaults, which can use many
        // cores and unbounded memory on very large local files. DBCrust is an
        // interactive shell, so default to bounded local-machine-safe settings
        // and let power users raise them via URL options or environment vars.
        let config = SessionConfig::new()
            .with_batch_size(safety_limits.batch_size)
            .with_target_partitions(safety_limits.target_partitions);
        let runtime = RuntimeEnvBuilder::new()
            .with_memory_limit(safety_limits.memory_limit_bytes, 0.8)
            .build_arc()
            .map_err(|e| {
                DatabaseError::ConnectionError(format!(
                    "Failed to initialize DataFusion runtime limits: {e}"
                ))
            })?;
        let state = SessionStateBuilder::new()
            .with_config(config)
            .with_runtime_env(runtime)
            .with_default_features()
            .build();
        let ctx = Arc::new(SessionContext::new_with_state(state));

        // Create metadata provider
        let metadata_provider =
            DataFusionMetadataProvider::new(Arc::clone(&ctx), connection_info.clone());

        let mut client = Self {
            ctx,
            connection_info: connection_info.clone(),
            registered_tables: std::collections::HashMap::new(),
            metadata_provider,
            safety_limits,
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

        // DataFusion's object_store layer requires absolute paths to resolve
        // local files. Relative paths (e.g. "./data.csv" or "data.csv") silently
        // register an empty listing table — queries return no rows and \d shows
        // no columns — because the directory walk finds nothing at the relative
        // location. Resolve to an absolute path before handing it to DataFusion.
        // Mirrors the path resolution SqliteClient already does.
        let resolved_path = if std::path::Path::new(path).is_absolute() {
            path.to_string()
        } else {
            let cwd = std::env::current_dir().map_err(|e| {
                DatabaseError::ConnectionError(format!("Could not get current directory: {e}"))
            })?;
            // canonicalize() collapses "./", "../", and symlinks for non-glob
            // paths. For glob patterns it fails on the wildcard characters, so
            // fall back to cwd.join(path) which still yields an absolute path.
            match std::fs::canonicalize(path) {
                Ok(abs) => abs.to_string_lossy().into_owned(),
                Err(_) => cwd.join(path).to_string_lossy().into_owned(),
            }
        };
        let path: &str = &resolved_path;

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
                                JsonReadOptions::default(),
                            )
                            .await
                            .map_err(|e| {
                                DatabaseError::ConnectionError(format!(
                                    "Failed to register NDJSON file: {e}"
                                ))
                            })?;
                    } else {
                        Arc::as_ref(&self.ctx)
                            .register_json(&table_name, &register_path, JsonReadOptions::default())
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
                        .register_json(&table_name, &register_path, JsonReadOptions::default())
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
                            .register_json(&table_name, &temp_path, JsonReadOptions::default())
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

    /// Execute a DataFusion query and convert results to Vec<Vec<String>>.
    ///
    /// This intentionally streams and bounds results instead of using
    /// `DataFrame::collect()`: collecting a huge Parquet/CSV/JSON result can
    /// materialize many Arrow batches, then dbcrust would duplicate them into
    /// strings and formatted terminal output. Streaming lets us stop at safety
    /// caps and drop DataFusion execution early.
    async fn execute_datafusion_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        debug!(
            "[DataFusionClient::execute_datafusion_query] Executing query: {}",
            sql
        );

        let df = self
            .ctx
            .sql(sql)
            .await
            .map_err(|e| DatabaseError::QueryError(format!("Failed to execute query: {e}")))?;

        let stream = df.execute_stream().await.map_err(|e| {
            DatabaseError::QueryError(format!("Failed to start streaming results: {e}"))
        })?;

        Self::record_batch_stream_to_strings(stream, &self.safety_limits).await
    }

    async fn record_batch_stream_to_strings(
        mut stream: datafusion::execution::SendableRecordBatchStream,
        limits: &DataFusionSafetyLimits,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        let schema = stream.schema();
        let column_count = schema.fields().len();
        let headers: Vec<String> = schema.fields().iter().map(|f| f.name().clone()).collect();
        let mut approx_output_bytes = headers.iter().map(|h| h.len()).sum::<usize>();
        let mut results = vec![headers];
        let mut data_rows = 0usize;
        let started_at = Instant::now();
        let timeout = Duration::from_secs(limits.max_query_seconds as u64);
        let mut truncation_reason = None;

        if limits.max_result_rows == 0 {
            results.push(Self::truncation_notice_row(
                column_count,
                "result row safety cap reached (0 rows). Raise datafusion_max_result_rows if you need rows returned.".to_string(),
            ));
            return Ok(results);
        }

        loop {
            let remaining = timeout.checked_sub(started_at.elapsed()).ok_or_else(|| {
                Self::query_timeout_error(limits.max_query_seconds, started_at.elapsed())
            })?;

            let next_batch = tokio::time::timeout(remaining, stream.next())
                .await
                .map_err(|_| {
                    Self::query_timeout_error(limits.max_query_seconds, started_at.elapsed())
                })?;

            let Some(batch) = next_batch else {
                break;
            };
            let batch = batch
                .map_err(|e| DatabaseError::QueryError(format!("Failed to read results: {e}")))?;

            for row_idx in 0..batch.num_rows() {
                if data_rows >= limits.max_result_rows {
                    truncation_reason = Some(format!(
                        "result row safety cap reached ({} rows). Add a narrower SELECT/LIMIT or raise datafusion_max_result_rows if you really need more.",
                        limits.max_result_rows
                    ));
                    break;
                }

                let row = Self::record_batch_row_to_strings(&batch, row_idx, limits.max_cell_chars);
                let row_bytes = row.iter().map(|cell| cell.len()).sum::<usize>();
                if approx_output_bytes.saturating_add(row_bytes) > limits.max_output_bytes {
                    truncation_reason = Some(format!(
                        "output safety cap reached ({} bytes). Select fewer columns or raise datafusion_max_output_bytes if you really need more.",
                        limits.max_output_bytes
                    ));
                    break;
                }

                approx_output_bytes = approx_output_bytes.saturating_add(row_bytes);
                results.push(row);
                data_rows += 1;
            }

            if truncation_reason.is_some() {
                break;
            }
        }

        if let Some(reason) = truncation_reason {
            results.push(Self::truncation_notice_row(column_count, reason));
        }

        Ok(results)
    }

    fn query_timeout_error(max_query_seconds: usize, elapsed: Duration) -> DatabaseError {
        DatabaseError::QueryError(format!(
            "DataFusion query stopped after {:.1}s (safety timeout: {}s). Use a narrower predicate/projection/LIMIT or raise datafusion_max_query_seconds if this scan is intentional.",
            elapsed.as_secs_f64(),
            max_query_seconds
        ))
    }

    fn record_batch_row_to_strings(
        batch: &RecordBatch,
        row_idx: usize,
        max_cell_chars: usize,
    ) -> Vec<String> {
        let mut row = Vec::with_capacity(batch.num_columns());
        for col_idx in 0..batch.num_columns() {
            let column = batch.column(col_idx);
            row.push(Self::array_value_to_string(column, row_idx, max_cell_chars));
        }
        row
    }

    fn truncation_notice_row(column_count: usize, reason: String) -> Vec<String> {
        let mut row = vec![String::new(); column_count.max(1)];
        row[0] = format!("⚠ dbcrust truncated DataFusion results: {reason}");
        row
    }

    fn truncate_cell_value(value: &str, max_cell_chars: usize) -> String {
        match value.char_indices().nth(max_cell_chars) {
            Some((split_at, _)) => {
                format!("{}… [truncated; {} bytes]", &value[..split_at], value.len())
            }
            None => value.to_string(),
        }
    }

    fn bytes_to_display(bytes: &[u8], max_cell_chars: usize) -> String {
        let suffix_budget = 32usize;
        let max_hex_chars = max_cell_chars.saturating_sub(suffix_budget);
        let max_bytes = (max_hex_chars / 2).max(1).min(bytes.len());
        let mut rendered = String::with_capacity(max_bytes.saturating_mul(2).saturating_add(32));
        for byte in &bytes[..max_bytes] {
            rendered.push_str(&format!("{byte:02x}"));
        }
        if bytes.len() > max_bytes {
            rendered.push_str(&format!("… [truncated; {} bytes]", bytes.len()));
        }
        rendered
    }

    /// Convert an Arrow array value to a bounded String.
    fn array_value_to_string(
        array: &Arc<dyn datafusion::arrow::array::Array>,
        row_idx: usize,
        max_cell_chars: usize,
    ) -> String {
        use datafusion::arrow::util::display::{ArrayFormatter, FormatOptions};

        if array.is_null(row_idx) {
            return "NULL".to_string();
        }

        // Handle large string/binary cells without first allocating their full
        // display representation. This is essential for Parquet columns such as
        // `patch` or `file_content` where a single cell can be megabytes.
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Self::truncate_cell_value(values.value(row_idx), max_cell_chars);
        }
        if let Some(values) = array.as_any().downcast_ref::<LargeStringArray>() {
            return Self::truncate_cell_value(values.value(row_idx), max_cell_chars);
        }
        if let Some(values) = array.as_any().downcast_ref::<StringViewArray>() {
            return Self::truncate_cell_value(values.value(row_idx), max_cell_chars);
        }
        if let Some(values) = array.as_any().downcast_ref::<BinaryArray>() {
            return Self::bytes_to_display(values.value(row_idx), max_cell_chars);
        }
        if let Some(values) = array.as_any().downcast_ref::<LargeBinaryArray>() {
            return Self::bytes_to_display(values.value(row_idx), max_cell_chars);
        }
        if let Some(values) = array.as_any().downcast_ref::<BinaryViewArray>() {
            return Self::bytes_to_display(values.value(row_idx), max_cell_chars);
        }
        if let Some(values) = array.as_any().downcast_ref::<FixedSizeBinaryArray>() {
            return Self::bytes_to_display(values.value(row_idx), max_cell_chars);
        }

        // Arrow's value formatter renders the single cell for every remaining
        // type (dates, timestamps, decimals, lists, structs…). Bound its output
        // before passing it to the formatter/display layer.
        let options = FormatOptions::default().with_null("NULL");
        let rendered = match ArrayFormatter::try_new(array.as_ref(), &options) {
            Ok(formatter) => formatter
                .value(row_idx)
                .try_to_string()
                .unwrap_or_else(|e| format!("?{e}?")),
            Err(e) => format!("?{e}?"),
        };
        Self::truncate_cell_value(&rendered, max_cell_chars)
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
            format!("DataFusion {}", datafusion::DATAFUSION_VERSION),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::{Int32Array, StringViewArray};
    use datafusion::arrow::datatypes::{Field, Schema};
    use datafusion::physical_plan::stream::RecordBatchStreamAdapter;

    fn test_limits() -> DataFusionSafetyLimits {
        DataFusionSafetyLimits {
            memory_limit_bytes: DEFAULT_DATAFUSION_MEMORY_LIMIT_BYTES,
            target_partitions: 1,
            batch_size: 2,
            max_result_rows: 10,
            max_cell_chars: 32,
            max_output_bytes: 1024 * 1024,
            max_query_seconds: 5,
        }
    }

    #[test]
    fn string_view_cells_are_truncated_before_display() {
        let long_value = "x".repeat(100);
        let array: Arc<dyn datafusion::arrow::array::Array> =
            Arc::new(StringViewArray::from(vec![long_value.as_str()]));

        let rendered = DataFusionClient::array_value_to_string(&array, 0, 8);

        assert_eq!(rendered, "xxxxxxxx… [truncated; 100 bytes]");
    }

    #[tokio::test]
    async fn streaming_conversion_stops_at_result_row_cap() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int32,
            false,
        )]));
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![Arc::new(Int32Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let input = futures_util::stream::iter(vec![Ok(batch)]);
        let stream = Box::pin(RecordBatchStreamAdapter::new(Arc::clone(&schema), input))
            as datafusion::execution::SendableRecordBatchStream;
        let mut limits = test_limits();
        limits.max_result_rows = 2;

        let results = DataFusionClient::record_batch_stream_to_strings(stream, &limits)
            .await
            .unwrap();

        assert_eq!(results[0], vec!["value".to_string()]);
        assert_eq!(results[1], vec!["1".to_string()]);
        assert_eq!(results[2], vec!["2".to_string()]);
        assert!(results[3][0].contains("result row safety cap reached"));
        assert_eq!(results.len(), 4);
    }

    #[tokio::test]
    async fn streaming_conversion_stops_at_output_byte_cap() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "payload",
            DataType::Utf8View,
            false,
        )]));
        let values = StringViewArray::from(vec!["abcdef", "ghijkl"]);
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![Arc::new(values)]).unwrap();
        let input = futures_util::stream::iter(vec![Ok(batch)]);
        let stream = Box::pin(RecordBatchStreamAdapter::new(Arc::clone(&schema), input))
            as datafusion::execution::SendableRecordBatchStream;
        let mut limits = test_limits();
        limits.max_output_bytes = "payload".len() + "abcde".len();

        let results = DataFusionClient::record_batch_stream_to_strings(stream, &limits)
            .await
            .unwrap();

        assert_eq!(results[0], vec!["payload".to_string()]);
        assert!(results[1][0].contains("output safety cap reached"));
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn datafusion_client_caps_explicitly_large_limit() {
        let mut file = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        use std::io::Write;
        writeln!(file, "id,name").unwrap();
        writeln!(file, "1,Alice").unwrap();
        writeln!(file, "2,Bob").unwrap();
        writeln!(file, "3,Carol").unwrap();
        file.flush().unwrap();

        let path = file.path().to_string_lossy().to_string();
        let mut options = std::collections::HashMap::new();
        options.insert("header".to_string(), "true".to_string());
        options.insert("datafusion_max_result_rows".to_string(), "2".to_string());
        options.insert(
            "datafusion_max_output_bytes".to_string(),
            (1024 * 1024).to_string(),
        );
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::CSV,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(path.clone()),
            options,
            docker_container: None,
            use_tls: false,
        };

        let client = DataFusionClient::new(connection_info).await.unwrap();
        let table_name = DataFusionClient::extract_table_name(&path);
        let results = client
            .execute_query(&format!("SELECT * FROM {table_name} LIMIT 100"))
            .await
            .unwrap();

        assert_eq!(results[0], vec!["id".to_string(), "name".to_string()]);
        assert_eq!(results[1], vec!["1".to_string(), "Alice".to_string()]);
        assert_eq!(results[2], vec!["2".to_string(), "Bob".to_string()]);
        assert!(results[3][0].contains("result row safety cap reached"));
        assert_eq!(results.len(), 4);
    }

    /// Regression test: relative paths (e.g. `./data.csv` or `data.csv`) must
    /// be resolved to absolute before handing to DataFusion. Without this,
    /// `register_parquet`/`register_csv` silently creates an empty listing
    /// table and both `\d` and `SELECT *` return nothing.
    #[tokio::test]
    async fn datafusion_client_resolves_relative_csv_path() {
        // Create a temp directory under cwd so relative paths resolve without
        // mutating the process-wide working directory (which would race with
        // parallel tests).
        let dir = tempfile::tempdir_in(".").unwrap();
        let file_path = dir.path().join("data.csv");
        let mut file = std::fs::File::create(&file_path).unwrap();
        use std::io::Write;
        writeln!(file, "id,name").unwrap();
        writeln!(file, "1,Alice").unwrap();
        writeln!(file, "2,Bob").unwrap();
        file.flush().unwrap();

        let dir_name = dir.path().file_name().unwrap().to_str().unwrap();
        let relative_path = format!("./{dir_name}/data.csv");

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::CSV,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(relative_path),
            options: std::collections::HashMap::new(),
            docker_container: None,
            use_tls: false,
        };

        let client = DataFusionClient::new(connection_info).await.unwrap();
        let results = client
            .execute_query("SELECT * FROM data LIMIT 100")
            .await
            .unwrap();

        assert_eq!(results[0], vec!["id".to_string(), "name".to_string()]);
        assert_eq!(results[1], vec!["1".to_string(), "Alice".to_string()]);
        assert_eq!(results[2], vec!["2".to_string(), "Bob".to_string()]);
    }

    /// Regression test: `\d <table>` (get_table_details) must return the
    /// correct column list when the file was opened via a relative path.
    #[tokio::test]
    async fn datafusion_client_relative_path_table_details() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let file_path = dir.path().join("metrics.csv");
        let mut file = std::fs::File::create(&file_path).unwrap();
        use std::io::Write;
        writeln!(file, "id,value").unwrap();
        writeln!(file, "1,42").unwrap();
        file.flush().unwrap();

        let dir_name = dir.path().file_name().unwrap().to_str().unwrap();
        let relative_path = format!("./{dir_name}/metrics.csv");

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::CSV,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(relative_path),
            options: std::collections::HashMap::new(),
            docker_container: None,
            use_tls: false,
        };

        let client = DataFusionClient::new(connection_info).await.unwrap();
        let details = client
            .metadata_provider
            .get_table_details("metrics", None)
            .await
            .unwrap();

        assert_eq!(details.schema, "public");
        assert_eq!(details.columns.len(), 2);
        assert_eq!(details.columns[0].name, "id");
        assert_eq!(details.columns[1].name, "value");
    }

    /// Helper: write a tiny parquet file using DataFusion's own writer so the
    /// regression tests below don't need an external fixture or the `parquet`
    /// crate as a direct dev-dependency.
    async fn write_parquet_fixture(path: &std::path::Path) {
        use datafusion::arrow::array::{Int32Array, StringArray};
        use datafusion::arrow::datatypes::{DataType, Field, Schema};
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["alice", "bob"])),
            ],
        )
        .unwrap();

        // `tempdir_in(".")` keeps the temp fixture on the same filesystem as
        // cwd so a relative path resolves identically during the test run.
        let ctx = SessionContext::new();
        let df = ctx.read_batch(batch).unwrap();
        df.write_parquet(
            path.to_str().unwrap(),
            datafusion::dataframe::DataFrameWriteOptions::default(),
            None,
        )
        .await
        .unwrap();
    }

    /// Regression test: relative Parquet paths must resolve to absolute before
    /// registration. This mirrors `datafusion_client_resolves_relative_csv_path`
    /// but exercises `register_parquet`, which previous CSV-only coverage missed.
    #[tokio::test]
    async fn datafusion_client_resolves_relative_parquet_path() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let file_path = dir.path().join("data.parquet");
        write_parquet_fixture(&file_path).await;

        let dir_name = dir.path().file_name().unwrap().to_str().unwrap();
        let relative_path = format!("./{dir_name}/data.parquet");

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::Parquet,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(relative_path),
            options: std::collections::HashMap::new(),
            docker_container: None,
            use_tls: false,
        };

        let client = DataFusionClient::new(connection_info).await.unwrap();
        let results = client
            .execute_query("SELECT * FROM data LIMIT 100")
            .await
            .unwrap();

        assert_eq!(results[0], vec!["id".to_string(), "name".to_string()]);
        assert_eq!(results[1], vec!["1".to_string(), "alice".to_string()]);
        assert_eq!(results[2], vec!["2".to_string(), "bob".to_string()]);
    }

    /// Regression test: `\d <table>` on a Parquet file opened via a relative
    /// path must report the columns read from the parquet schema, not an empty
    /// listing-table result. Reproduces the user-reported v0.31.1 regression
    /// where `dbc export.parquet` showed zero columns.
    #[tokio::test]
    async fn datafusion_client_relative_parquet_table_details() {
        let dir = tempfile::tempdir_in(".").unwrap();
        let file_path = dir.path().join("metrics.parquet");
        write_parquet_fixture(&file_path).await;

        let dir_name = dir.path().file_name().unwrap().to_str().unwrap();
        let relative_path = format!("./{dir_name}/metrics.parquet");

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::Parquet,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(relative_path),
            options: std::collections::HashMap::new(),
            docker_container: None,
            use_tls: false,
        };

        let client = DataFusionClient::new(connection_info).await.unwrap();
        let details = client
            .metadata_provider
            .get_table_details("metrics", None)
            .await
            .unwrap();

        assert_eq!(details.schema, "public");
        assert_eq!(
            details.columns.len(),
            2,
            "parquet relative path reported zero columns"
        );
        assert_eq!(details.columns[0].name, "id");
        assert_eq!(details.columns[1].name, "name");
    }
}
