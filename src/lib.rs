#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_local_definitions)]

pub mod ai; // AI assistant integration (text-to-SQL, multi-provider)
pub mod cli;
pub mod cli_core; // New unified CLI core
pub mod command_completion; // Trait-based command completion system
pub mod commands; // New type-safe enum-based command system
pub mod completion;
pub mod completion_provider; // Database-agnostic completion trait
pub mod complex_display; // Unified display system for complex data types
pub mod config;
pub mod config_editor; // Schema-driven \config menu, get/set, tunnel manager
pub mod database; // New database abstraction layer
pub mod database_clickhouse; // ClickHouse implementation
pub mod database_datafusion; // DataFusion implementation for file formats (Parquet, CSV, JSON)
pub mod database_elasticsearch; // Elasticsearch implementation
pub mod database_mongodb; // MongoDB implementation
pub mod database_mysql; // MySQL implementation
pub mod database_postgresql; // PostgreSQL implementation
pub mod database_sqlite; // SQLite implementation
pub mod db;
pub mod dbcrust_pass; // Universal password file (.dbcrust) support
pub mod docker; // Docker container integration
pub mod explain_tui;
pub mod format; // Made format module public
pub mod geojson_display;
pub mod highlighter;
pub mod history_manager; // Per-session command history management
pub mod json_display; // JSON display implementation
pub mod logging;
pub mod myconf; // MySQL configuration file support
pub mod named_queries;
pub mod pager;
pub mod password_encryption; // Password encryption for .dbcrust file
pub mod password_sanitizer;
pub mod performance_analyzer; // Performance analysis for EXPLAIN queries
pub mod pgpass;
pub mod prompt;
pub mod schema_tui;
pub mod script;
pub mod shell_completion; // Custom shell completion with URL schemes
pub mod sql_buffer; // Multiline validation + statement splitting for the REPL
pub mod sql_context; // SQL context analysis for better autocompletion
pub mod sql_parser; // Enhanced SQL parser for autocompletion
pub mod sql_parser_mysql; // MySQL-specific SQL parser
pub mod sql_parser_postgresql; // PostgreSQL-specific SQL parser
pub mod sql_parser_sqlite; // SQLite-specific SQL parser
pub mod sql_parser_trait; // Database-specific SQL parser trait system
pub mod ssh_tunnel; // Add the SSH tunnel module
pub mod update; // Self-update (--update): release check + channel-aware upgrade
pub mod url_scheme; // URL scheme autocompletion support
pub mod vault_client; // Add backslash commands module
pub mod vault_encryption; // Vault credential encryption utilities
pub mod vector_display; // Vector visualization for PostgreSQL extensions // GeoJSON display implementation // TUI-based query plan visualizer

// Note: main.rs functions are not directly accessible as modules in lib.rs
// We'll create PyO3 wrappers that call the main functionality directly

#[cfg(feature = "python")]
use pyo3::exceptions::PyRuntimeError;
#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::types::{PyDict, PyList};
#[cfg(feature = "python")]
use std::sync::Arc;
#[cfg(feature = "python")]
use tokio::runtime::Runtime;
#[cfg(feature = "python")]
use tokio::sync::Mutex as TokioMutex;

pub use config::Config;
pub use db::Database;
pub use logging::{debug, get_log_file_path_string};
pub use myconf::{get_mysql_config_path, lookup_mysql_password, save_mysql_config};
pub use pgpass::{get_pgpass_path, lookup_password};

#[cfg(feature = "python")]
#[pyclass]
pub struct PyDatabase {
    inner: Arc<TokioMutex<Database>>,
    rt: Arc<Runtime>,
}

#[cfg(feature = "python")]
#[pyclass]
pub struct PyConfig {
    inner: config::Config,
}

/// Enhanced database connection with context manager support
#[cfg(feature = "python")]
#[pyclass]
pub struct PyConnection {
    inner: Arc<TokioMutex<Database>>,
    rt: Arc<Runtime>,
    connection_url: String,
    database_type: Option<crate::database::DatabaseType>,
    auto_commit: bool,
    // Reserved for future use: connection timeout configuration
    #[allow(dead_code)]
    timeout: Option<f64>,
}

/// Database server information
#[cfg(feature = "python")]
#[pyclass]
pub struct PyServerInfo {
    #[pyo3(get)]
    pub version: String,
    #[pyo3(get)]
    pub database_type: String,
    #[pyo3(get)]
    pub supports_transactions: bool,
    #[pyo3(get)]
    pub supports_roles: bool,
    version_major: Option<u32>,
    version_minor: Option<u32>,
}

/// Implementation for PyServerInfo - Database server metadata
#[cfg(feature = "python")]
#[pymethods]
impl PyServerInfo {
    /// Get major version number
    #[getter]
    pub fn version_major(&self) -> Option<u32> {
        self.version_major
    }

    /// Get minor version number
    #[getter]
    pub fn version_minor(&self) -> Option<u32> {
        self.version_minor
    }

    /// Check if server supports a specific feature
    pub fn supports(&self, feature: &str) -> bool {
        match feature.to_lowercase().as_str() {
            "transactions" => self.supports_transactions,
            "roles" => self.supports_roles,
            "json" => matches!(self.database_type.as_str(), "PostgreSQL" | "MySQL"),
            "schemas" => matches!(self.database_type.as_str(), "PostgreSQL" | "MySQL"),
            "stored_procedures" => matches!(self.database_type.as_str(), "PostgreSQL" | "MySQL"),
            _ => false,
        }
    }

    /// Get a string representation
    pub fn __str__(&self) -> String {
        format!("{} {}", self.database_type, self.version)
    }

    /// Get detailed representation
    pub fn __repr__(&self) -> String {
        format!(
            "PyServerInfo(database_type='{}', version='{}', supports_transactions={}, supports_roles={})",
            self.database_type, self.version, self.supports_transactions, self.supports_roles
        )
    }
}

/// Structured row data
#[cfg(feature = "python")]
#[pyclass]
#[derive(Clone)]
pub struct PyRow {
    data: Vec<String>,
    column_names: Vec<String>,
}

/// Result set with multiple rows
#[cfg(feature = "python")]
#[pyclass]
pub struct PyResultSet {
    rows: Vec<PyRow>,
    column_names: Vec<String>,
    row_count: usize,
}

/// Cursor for multi-query execution
#[cfg(feature = "python")]
#[pyclass]
pub struct PyCursor {
    connection: Arc<TokioMutex<Database>>,
    rt: Arc<Runtime>,
    results: Vec<Vec<Vec<String>>>,
    current_result_index: usize,
    current_row_index: usize,
    column_names: Vec<String>,
}

// Define custom Python exceptions
#[cfg(feature = "python")]
use pyo3::create_exception;

// Create exception hierarchy
#[cfg(feature = "python")]
create_exception!(
    _internal,
    DbcrustError,
    pyo3::exceptions::PyException,
    "Base exception for all DBCrust errors"
);
#[cfg(feature = "python")]
create_exception!(
    _internal,
    DbcrustConnectionError,
    DbcrustError,
    "Database connection error"
);
#[cfg(feature = "python")]
create_exception!(
    _internal,
    DbcrustCommandError,
    DbcrustError,
    "Command execution error"
);
#[cfg(feature = "python")]
create_exception!(
    _internal,
    DbcrustConfigError,
    DbcrustError,
    "Configuration error"
);
#[cfg(feature = "python")]
create_exception!(
    _internal,
    DbcrustArgumentError,
    DbcrustError,
    "Invalid argument error"
);

/// Convert CliError to appropriate Python exception
#[cfg(feature = "python")]
fn cli_error_to_pyerr(err: crate::cli_core::CliError) -> PyErr {
    use crate::cli_core::CliError;

    match err {
        CliError::ConnectionError(msg) => DbcrustConnectionError::new_err(msg),
        CliError::CommandError(msg) => DbcrustCommandError::new_err(msg),
        CliError::ConfigError(msg) => DbcrustConfigError::new_err(msg),
        CliError::ArgumentError(msg) => DbcrustArgumentError::new_err(msg),
    }
}

/// A Python module implemented in Rust.
#[cfg(feature = "python")]
#[pymodule]
pub fn _internal(_py: Python<'_>, m: Bound<'_, PyModule>) -> PyResult<()> {
    // Legacy classes (keep for backward compatibility)
    m.add_class::<PyDatabase>()?;
    m.add_class::<PyConfig>()?;

    // New enhanced API classes
    m.add_class::<PyConnection>()?;
    m.add_class::<PyServerInfo>()?;
    m.add_class::<PyRow>()?;
    m.add_class::<PyResultSet>()?;
    m.add_class::<PyCursor>()?;

    // Functions
    m.add_function(wrap_pyfunction!(run_command, &m)?)?;
    m.add_function(wrap_pyfunction!(run_cli_loop, &m)?)?;
    m.add_function(wrap_pyfunction!(py_connect, &m)?)?;
    m.add_function(wrap_pyfunction!(ai_config_status, &m)?)?;
    m.add_function(wrap_pyfunction!(run_ai_investigation, &m)?)?;

    // Add custom exceptions to the module
    m.add("DbcrustError", _py.get_type::<DbcrustError>())?;
    m.add(
        "DbcrustConnectionError",
        _py.get_type::<DbcrustConnectionError>(),
    )?;
    m.add("DbcrustCommandError", _py.get_type::<DbcrustCommandError>())?;
    m.add("DbcrustConfigError", _py.get_type::<DbcrustConfigError>())?;
    m.add(
        "DbcrustArgumentError",
        _py.get_type::<DbcrustArgumentError>(),
    )?;

    Ok(())
}

#[cfg(feature = "python")]
#[pymethods]
impl PyDatabase {
    /// Create a new PostgreSQL database connection.
    #[new]
    pub fn new(host: &str, port: u16, user: &str, password: &str, dbname: &str) -> PyResult<Self> {
        let config_val = config::Config::load(); // Renamed to avoid conflict

        let rt = Runtime::new().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Failed to create Tokio runtime: {e}"
            ))
        })?;

        let rt_arc = Arc::new(rt);
        let rt_clone = rt_arc.clone();

        let db_instance = rt_arc
            .block_on(async {
                db::Database::new(
                    host,
                    port,
                    user,
                    password,
                    dbname,
                    Some(config_val.default_limit), // Use loaded config
                    Some(config_val.expanded_display_default), // Use loaded config
                    None,                           // No SSH tunnel from Python interface yet
                    None,                           // No SSL mode specified
                )
                .await
            })
            .map_err(|e| {
                DbcrustConnectionError::new_err(format!("Failed to connect to database: {e}"))
            })?;

        Ok(PyDatabase {
            inner: Arc::new(TokioMutex::new(db_instance)),
            rt: rt_clone,
        })
    }

    /// Execute a query and return the results.
    pub fn execute(&self, query: &str) -> PyResult<PyObject> {
        let results = self
            .rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.execute_query(query).await
            })
            .map_err(|e| DbcrustCommandError::new_err(format!("Query execution failed: {e}")))?;

        Python::with_gil(|py| Ok(results.into_pyobject(py)?.into_any().unbind()))
    }

    /// Get connection info as a string.
    pub fn connection_info(&self) -> PyResult<String> {
        Ok(self.rt.block_on(async {
            let db = self.inner.lock().await;
            format!(
                "Connected to {} on {}:{} as {}",
                db.get_current_db(),
                db.get_host(),
                db.get_port(),
                db.get_username()
            )
        }))
    }

    /// List all databases.
    pub fn list_databases(&self) -> PyResult<PyObject> {
        let results = self
            .rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.list_databases().await
            })
            .map_err(|e| DbcrustCommandError::new_err(format!("Failed to list databases: {e}")))?;

        Python::with_gil(|py| Ok(results.into_pyobject(py)?.into_any().unbind()))
    }

    /// List all tables.
    pub fn list_tables(&self) -> PyResult<PyObject> {
        let results = self
            .rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.list_tables().await
            })
            .map_err(|e| DbcrustCommandError::new_err(format!("Failed to list tables: {e}")))?;

        Python::with_gil(|py| Ok(results.into_pyobject(py)?.into_any().unbind()))
    }

    /// Describe a table.
    pub fn describe_table(&self, table_name: &str) -> PyResult<PyObject> {
        let table_details = self
            .rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.get_table_details(table_name).await
            })
            .map_err(|e| DbcrustCommandError::new_err(format!("Failed to describe table: {e}")))?;

        Python::with_gil(|py| {
            let dict = PyDict::new(py);
            dict.set_item("name", &table_details.name)?;
            dict.set_item("schema", &table_details.schema)?;
            dict.set_item("full_name", &table_details.full_name)?;

            // Convert columns to list of dicts
            let columns_list = PyList::empty(py);
            for col in &table_details.columns {
                let col_dict = PyDict::new(py);
                col_dict.set_item("name", &col.name)?;
                col_dict.set_item("data_type", &col.data_type)?;
                col_dict.set_item("collation", &col.collation)?;
                col_dict.set_item("nullable", col.nullable)?;
                col_dict.set_item("default_value", &col.default_value)?;
                columns_list.append(col_dict)?;
            }
            dict.set_item("columns", columns_list)?;

            Ok(dict.into())
        })
    }
}

#[cfg(feature = "python")]
impl Default for PyConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "python")]
#[pymethods]
impl PyConfig {
    #[new]
    pub fn new() -> Self {
        PyConfig {
            inner: config::Config::load(),
        }
    }

    /// Get the default query limit.
    pub fn get_default_limit(&self) -> usize {
        self.inner.default_limit
    }

    /// Set the default query limit.
    pub fn set_default_limit(&mut self, limit: usize) {
        self.inner.default_limit = limit;
    }

    /// Get expanded display default setting.
    pub fn get_expanded_display_default(&self) -> bool {
        self.inner.expanded_display_default
    }

    /// Set expanded display default setting.
    pub fn set_expanded_display_default(&mut self, expanded: bool) {
        self.inner.expanded_display_default = expanded;
    }

    /// Save the configuration (documented format, comments preserved).
    pub fn save(&self) -> PyResult<()> {
        self.inner
            .save_with_documentation()
            .map_err(|e| DbcrustConfigError::new_err(format!("Failed to save config: {e}")))
    }
}

/// Implementation for PyConnection - Enhanced database connection API
#[cfg(feature = "python")]
#[pymethods]
impl PyConnection {
    /// Create a new database connection from URL
    #[new]
    pub fn new(
        connection_url: &str,
        timeout: Option<f64>,
        auto_commit: Option<bool>,
    ) -> PyResult<Self> {
        let rt = Runtime::new()
            .map_err(|e| DbcrustError::new_err(format!("Failed to create Tokio runtime: {e}")))?;

        let rt_arc = Arc::new(rt);
        let rt_clone = rt_arc.clone();

        let db_instance = rt_arc
            .block_on(async {
                Database::from_url(
                    connection_url,
                    None, // Use default limit
                    None, // Use default expanded display
                )
                .await
            })
            .map_err(|e| {
                DbcrustConnectionError::new_err(format!("Failed to connect to database: {e}"))
            })?;

        // Detect database type from URL
        let database_type = if connection_url.starts_with("postgres://")
            || connection_url.starts_with("postgresql://")
        {
            Some(crate::database::DatabaseType::PostgreSQL)
        } else if connection_url.starts_with("mysql://") {
            Some(crate::database::DatabaseType::MySQL)
        } else if connection_url.starts_with("sqlite://") {
            Some(crate::database::DatabaseType::SQLite)
        } else {
            None
        };

        Ok(PyConnection {
            inner: Arc::new(TokioMutex::new(db_instance)),
            rt: rt_clone,
            connection_url: connection_url.to_string(),
            database_type,
            auto_commit: auto_commit.unwrap_or(true),
            timeout,
        })
    }

    /// Context manager entry - return a reference that Python can use
    pub fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Context manager exit
    pub fn __exit__(
        &mut self,
        _exc_type: Option<PyObject>,
        _exc_value: Option<PyObject>,
        _traceback: Option<PyObject>,
    ) -> PyResult<bool> {
        // Connection cleanup if needed
        // Return false to let exceptions propagate
        Ok(false)
    }

    /// Create a cursor for executing queries
    pub fn cursor(&self) -> PyResult<PyCursor> {
        Ok(PyCursor {
            connection: self.inner.clone(),
            rt: self.rt.clone(),
            results: Vec::new(),
            current_result_index: 0,
            current_row_index: 0,
            column_names: Vec::new(),
        })
    }

    /// Get database server information
    pub fn get_server_info(&self) -> PyResult<PyServerInfo> {
        // Get real server info from the database
        let inner = self.inner.clone();
        let rt = self.rt.clone();

        let server_info = rt
            .block_on(async move {
                let db = inner.lock().await;
                db.get_database_client()
                    .ok_or("No database client available")?
                    .get_server_info()
                    .await
                    .map_err(|e| format!("Failed to get server info: {}", e))
            })
            .map_err(|e: String| PyRuntimeError::new_err(e))?;

        Ok(PyServerInfo {
            version: server_info.server_version,
            database_type: server_info.server_type,
            supports_transactions: server_info.supports_transactions,
            supports_roles: server_info.supports_roles,
            version_major: server_info.version_major.map(|v| v as u32),
            version_minor: server_info.version_minor.map(|v| v as u32),
        })
    }

    /// Execute a single query immediately (convenience method)
    pub fn execute_immediate(&self, query: &str) -> PyResult<PyResultSet> {
        let results = self
            .rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.execute_query(query).await
            })
            .map_err(|e| DbcrustCommandError::new_err(format!("Query execution failed: {e}")))?;

        // Extract column names and data rows
        if results.is_empty() {
            return Ok(PyResultSet {
                row_count: 0,
                column_names: vec![],
                rows: vec![],
            });
        }

        // First row contains column names
        let column_names = results[0].clone();

        // Convert data rows to PyRow objects (skip header row)
        let rows: Vec<PyRow> = if results.len() > 1 {
            results[1..]
                .iter()
                .map(|row_data| PyRow {
                    data: row_data.clone(),
                    column_names: column_names.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };

        Ok(PyResultSet {
            row_count: rows.len(),
            column_names,
            rows,
        })
    }

    /// Get connection URL (read-only)
    #[getter]
    pub fn connection_url(&self) -> &str {
        &self.connection_url
    }

    /// Get database type
    #[getter]
    pub fn database_type(&self) -> String {
        match &self.database_type {
            Some(db_type) => format!("{:?}", db_type),
            None => "Unknown".to_string(),
        }
    }

    /// Get auto-commit setting
    #[getter]
    pub fn auto_commit(&self) -> bool {
        self.auto_commit
    }

    /// Set auto-commit setting
    #[setter]
    pub fn set_auto_commit(&mut self, auto_commit: bool) {
        self.auto_commit = auto_commit;
    }
}

/// Implementation for PyCursor - Multi-query execution and result navigation
#[cfg(feature = "python")]
#[pymethods]
impl PyCursor {
    /// Execute a single SQL statement
    pub fn execute(&mut self, query: &str) -> PyResult<usize> {
        let results = self
            .rt
            .block_on(async {
                let mut db = self.connection.lock().await;
                db.execute_query(query).await
            })
            .map_err(|e| DbcrustCommandError::new_err(format!("Query execution failed: {e}")))?;

        // Extract column names and data rows
        if !results.is_empty() {
            // First row contains column names
            self.column_names = results[0].clone();
            // Remaining rows contain actual data
            let data_rows = if results.len() > 1 {
                results[1..].to_vec()
            } else {
                Vec::new()
            };
            self.results = vec![data_rows];
        } else {
            self.column_names = vec![];
            self.results = vec![Vec::new()];
        }

        self.current_result_index = 0;
        self.current_row_index = 0;

        Ok(self.results[0].len())
    }

    /// Execute multiple SQL statements separated by semicolons
    pub fn executescript(&mut self, script: &str) -> PyResult<usize> {
        // Split on semicolon and execute each statement
        let statements: Vec<&str> = script
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        let mut all_results = Vec::new();
        let mut total_rows = 0;
        let mut first_column_names = Vec::new();

        for (i, statement) in statements.iter().enumerate() {
            let results = self
                .rt
                .block_on(async {
                    let mut db = self.connection.lock().await;
                    db.execute_query(statement).await
                })
                .map_err(|e| {
                    DbcrustCommandError::new_err(format!("Script execution failed: {e}"))
                })?;

            // Process results to separate headers from data
            if !results.is_empty() {
                // For the first statement, save column names
                if i == 0 {
                    first_column_names = results[0].clone();
                }
                // Extract data rows (skip header row)
                let data_rows = if results.len() > 1 {
                    results[1..].to_vec()
                } else {
                    Vec::new()
                };
                total_rows += data_rows.len();
                all_results.push(data_rows);
            } else {
                all_results.push(Vec::new());
            }
        }

        // Store all results for navigation
        self.results = all_results;
        self.current_result_index = 0;
        self.current_row_index = 0;
        self.column_names = first_column_names;

        Ok(total_rows)
    }

    /// Fetch the next row from current result set
    pub fn fetchone(&mut self) -> PyResult<Option<PyRow>> {
        if self.results.is_empty() || self.current_result_index >= self.results.len() {
            return Ok(None);
        }

        let current_results = &self.results[self.current_result_index];
        if self.current_row_index >= current_results.len() {
            return Ok(None);
        }

        let row_data = current_results[self.current_row_index].clone();
        self.current_row_index += 1;

        Ok(Some(PyRow {
            data: row_data,
            column_names: self.column_names.clone(),
        }))
    }

    /// Fetch multiple rows from current result set
    pub fn fetchmany(&mut self, size: Option<usize>) -> PyResult<Vec<PyRow>> {
        let fetch_size = size.unwrap_or(1);
        let mut rows = Vec::new();

        for _ in 0..fetch_size {
            if let Some(row) = self.fetchone()? {
                rows.push(row);
            } else {
                break;
            }
        }

        Ok(rows)
    }

    /// Fetch all remaining rows from current result set
    pub fn fetchall(&mut self) -> PyResult<Vec<PyRow>> {
        if self.results.is_empty() || self.current_result_index >= self.results.len() {
            return Ok(Vec::new());
        }

        let current_results = &self.results[self.current_result_index];
        let rows: Vec<PyRow> = current_results
            .iter()
            .skip(self.current_row_index)
            .map(|data| PyRow {
                data: data.clone(),
                column_names: self.column_names.clone(),
            })
            .collect();

        self.current_row_index = current_results.len();
        Ok(rows)
    }

    /// Move to the next result set (for multi-statement queries)
    pub fn nextset(&mut self) -> PyResult<bool> {
        if self.current_result_index + 1 < self.results.len() {
            self.current_result_index += 1;
            self.current_row_index = 0;
            // TODO: Update column names for new result set
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get the number of rows in current result set
    #[getter]
    pub fn rowcount(&self) -> usize {
        if self.results.is_empty() || self.current_result_index >= self.results.len() {
            0
        } else {
            self.results[self.current_result_index].len()
        }
    }

    /// Get column metadata for current result set
    #[getter]
    pub fn description(&self) -> Vec<String> {
        self.column_names.clone()
    }

    /// Close the cursor (cleanup)
    pub fn close(&mut self) -> PyResult<()> {
        self.results.clear();
        self.current_result_index = 0;
        self.current_row_index = 0;
        self.column_names.clear();
        Ok(())
    }
}

/// Implementation for PyRow - Individual row data
#[cfg(feature = "python")]
#[pymethods]
impl PyRow {
    /// Get data by column index
    pub fn __getitem__(&self, index: usize) -> PyResult<&str> {
        self.data.get(index).map(|s| s.as_str()).ok_or_else(|| {
            pyo3::exceptions::PyIndexError::new_err(format!("Index {} out of range", index))
        })
    }

    /// Get data by column name (if column names are available)
    pub fn get(&self, column_name: &str) -> PyResult<Option<&str>> {
        if let Some(pos) = self
            .column_names
            .iter()
            .position(|name| name == column_name)
        {
            Ok(self.data.get(pos).map(|s| s.as_str()))
        } else {
            Ok(None)
        }
    }

    /// Get all data as a list
    pub fn values(&self) -> Vec<&str> {
        self.data.iter().map(|s| s.as_str()).collect()
    }

    /// Get column names
    pub fn columns(&self) -> Vec<&str> {
        self.column_names.iter().map(|s| s.as_str()).collect()
    }

    /// Number of columns
    pub fn __len__(&self) -> usize {
        self.data.len()
    }
}

/// Implementation for PyResultSet - Collection of rows
#[cfg(feature = "python")]
#[pymethods]
impl PyResultSet {
    /// Get number of rows
    #[getter]
    pub fn rowcount(&self) -> usize {
        self.row_count
    }

    /// Get column names
    #[getter]
    pub fn columns(&self) -> Vec<&str> {
        self.column_names.iter().map(|s| s.as_str()).collect()
    }

    /// Get all rows
    pub fn rows(&self) -> Vec<PyRow> {
        self.rows.clone()
    }

    /// Get row by index
    pub fn __getitem__(&self, index: usize) -> PyResult<PyRow> {
        self.rows.get(index).cloned().ok_or_else(|| {
            pyo3::exceptions::PyIndexError::new_err(format!("Index {} out of range", index))
        })
    }

    /// Number of rows
    pub fn __len__(&self) -> usize {
        self.rows.len()
    }
}

/// Connect function - creates a PyConnection from URL with optional parameters
#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(signature = (connection_url, timeout=None, auto_commit=None))]
pub fn py_connect(
    connection_url: &str,
    timeout: Option<f64>,
    auto_commit: Option<bool>,
) -> PyResult<PyConnection> {
    PyConnection::new(connection_url, timeout, auto_commit)
}

/// Return non-secret AI configuration diagnostics for Python/Django callers.
#[cfg(feature = "python")]
#[pyfunction]
pub fn ai_config_status(py: Python<'_>) -> PyResult<Py<PyAny>> {
    let config_dir = crate::config::Config::get_config_directory().map_err(|e| {
        DbcrustConfigError::new_err(format!("Failed to resolve config directory: {e}"))
    })?;
    let config_path = config_dir.join("config.toml");
    let mut config = crate::config::Config::load();
    apply_python_ai_autodiscovery(&mut config);

    let dict = PyDict::new(py);
    dict.set_item("config_dir", config_dir.display().to_string())?;
    dict.set_item("config_file", config_path.display().to_string())?;
    dict.set_item("config_file_exists", config_path.exists())?;
    dict.set_item(
        "dbcrust_config_dir_env",
        std::env::var("DBCRUST_CONFIG_DIR").unwrap_or_default(),
    )?;
    dict.set_item(
        "codex_auth_file",
        crate::ai::chatgpt_auth::codex_auth_path()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    )?;
    dict.set_item("enabled", config.ai.enabled)?;
    dict.set_item("provider", &config.ai.provider)?;
    dict.set_item("model", &config.ai.model)?;
    dict.set_item("auth_method", config.ai.auth_method.to_string())?;
    dict.set_item("streaming", config.ai.streaming)?;
    dict.set_item("execution_mode", config.ai.execution_mode.to_string())?;
    Ok(dict.into_any().unbind())
}

#[cfg(feature = "python")]
fn apply_python_ai_autodiscovery(config: &mut crate::config::Config) {
    if config.ai.enabled || crate::ai::chatgpt_auth::codex_auth_path().is_none() {
        return;
    }

    // A Django dashboard/management-command click is already an explicit AI
    // action. If the user has a Codex/ChatGPT login mounted in the container,
    // use it directly instead of requiring a dbcrust config volume.
    config.ai.enabled = true;
    config.ai.provider = "openai".to_string();
    config.ai.auth_method = crate::ai::config::AiAuthMethod::ChatgptSubscription;
    config.ai.model = "gpt-5-codex".to_string();
}

/// Run an AI investigation against a database, optionally seeded with extra
/// context (e.g. Django models + ORM code). Returns the final analysis text.
///
/// With `agentic=true` (default) this runs the same tool-using investigation loop
/// as the REPL's `???`: the model calls read-only tools, observes results, and
/// iterates until it produces a structured analysis. With `agentic=false` it does
/// a single-shot text-to-SQL generation with the extra context prepended. Reuses
/// the on-disk AI config when present; for Django/Python calls, a mounted Codex
/// login at `~/.codex/auth.json` is enough to use ChatGPT subscription auth.
///
/// The GIL is **released** for the whole (multi-second) investigation, so a
/// caller running this in a background thread (e.g. the Django dashboard) does
/// not freeze other Python threads. Progress is silent by default (programmatic
/// callers get a clean return value); set `progress_path` to tail the agent's
/// narration from a file, or `stdout_progress=True` to print it (the management
/// command does this).
#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(signature = (connection_url, question, django_context, agentic=true, max_iterations=None, progress_path=None, stdout_progress=false))]
pub fn run_ai_investigation(
    py: Python<'_>,
    connection_url: String,
    question: String,
    django_context: String,
    agentic: bool,
    max_iterations: Option<usize>,
    progress_path: Option<String>,
    stdout_progress: bool,
) -> PyResult<String> {
    // Release the GIL: the investigation is all Rust (genai HTTP + DB) and takes
    // many seconds; holding the GIL would block every other Python thread,
    // including the dashboard's polling requests. Errors are plain Strings here
    // (no PyErr without the GIL) and mapped back to a Python exception after.
    let result: Result<String, String> = py.detach(move || {
        let rt = Runtime::new().map_err(|e| format!("Failed to create Tokio runtime: {e}"))?;

        rt.block_on(async move {
            let mut config = crate::config::Config::load();
            apply_python_ai_autodiscovery(&mut config);
            if !config.ai.enabled {
                let config_path = crate::config::Config::get_config_file_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| "<unknown config path>".to_string());
                return Err(format!(
                    "AI assistant is disabled in {config_path}. If `dbcrust` shows AI enabled, the Django/Python process is using a different config directory (often a different HOME, Docker container, or service user). Set DBCRUST_CONFIG_DIR to the CLI config directory, or mount ~/.codex/auth.json into the Django user's home to use your ChatGPT subscription without a dbcrust config volume."
                ));
            }

            let extra = if django_context.trim().is_empty() {
                None
            } else {
                Some(django_context.as_str())
            };

            // Gui frontend mode: headless/background use — no interactive terminal
            // UI (column selection) and no stdout status banners that would leak
            // into Django logs.
            let mut database = crate::db::Database::from_url_with_mode(
                &connection_url,
                None,
                None,
                crate::db::FrontendMode::Gui,
            )
            .await
            .map_err(|e| format!("Failed to connect: {e}"))?;
            let db_type = database.get_database_type();

            let progress: Box<dyn crate::ai::agent::ProgressSink> = match &progress_path {
                Some(path) => Box::new(crate::ai::agent::FileProgress::new(path.clone())),
                None if stdout_progress => Box::new(crate::ai::agent::StdoutProgress),
                // Silent by default so programmatic callers don't emit traces.
                None => Box::new(crate::ai::agent::NoOpProgress),
            };

            if agentic {
                let seed = crate::ai::schema_context::build_agent_seed_context(&mut database).await;
                let system_prompt = crate::ai::prompt_templates::build_agentic_system_prompt(
                    &db_type, &seed, extra,
                );

                let db_arc = std::sync::Arc::new(std::sync::Mutex::new(database));
                let interrupt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let max_iters = max_iterations.unwrap_or(config.ai.agentic_max_iterations);
                let executor = crate::ai::agent::DbToolExecutor::new(
                    db_arc.clone(),
                    interrupt.clone(),
                    config.ai.agentic_max_rows_per_tool,
                );

                // No session here — the question is the lone user message.
                let messages = vec![(crate::ai::MessageRole::User, question.clone())];
                crate::ai::agent::run_agent(
                    &config.ai,
                    &system_prompt,
                    &messages,
                    max_iters,
                    &executor,
                    progress.as_ref(),
                    &interrupt,
                )
                .await
                .map_err(|e| format!("AI investigation failed: {e}"))
            } else {
                // Single-shot: build the full schema context and prepend the extra context.
                let (schema_ctx, _cacheable) = crate::ai::schema_context::build_schema_context(
                    &mut database,
                    &question,
                    config.ai.max_schema_tables,
                )
                .await;
                let combined = match extra {
                    Some(ctx) => format!("{ctx}\n\n{schema_ctx}"),
                    None => schema_ctx,
                };
                let system_prompt =
                    crate::ai::prompt_templates::build_system_prompt(&db_type, &combined);
                let messages = vec![(crate::ai::MessageRole::User, question.to_string())];
                let resp = crate::ai::generate(&config.ai, &system_prompt, &messages)
                    .await
                    .map_err(|e| format!("AI generation failed: {e}"))?;
                Ok(crate::ai::streaming::extract_sql(&resp.content))
            }
        })
    });

    result.map_err(DbcrustError::new_err)
}

/// Python function to run a command using the new unified CLI system
/// Returns the exit code (0 for success, non-zero for controlled exits)
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_command(args: Vec<String>) -> PyResult<i32> {
    let rt = Runtime::new()
        .map_err(|e| DbcrustError::new_err(format!("Failed to create Tokio runtime: {e}")))?;

    rt.block_on(run_main_cli_workflow(args))
}

/// Python function to run the interactive CLI loop
/// Returns the exit code (0 for success, non-zero for controlled exits)
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_cli_loop(connection_url: Option<String>) -> PyResult<i32> {
    let rt = Runtime::new()
        .map_err(|e| DbcrustError::new_err(format!("Failed to create Tokio runtime: {e}")))?;

    rt.block_on(async {
        match connection_url {
            Some(url) => run_interactive_cli(&url).await.map(|_| 0),
            None => Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Connection URL is required for interactive mode",
            )) as Box<dyn std::error::Error>),
        }
    })
    .map_err(|e| DbcrustArgumentError::new_err(format!("Interactive CLI failed: {e}")))
}

/// Unified CLI workflow using CliCore - provides 100% feature parity with Rust CLI
#[cfg(feature = "python")]
async fn run_main_cli_workflow(args: Vec<String>) -> PyResult<i32> {
    use crate::cli::Args;
    use clap::Parser;

    // Store the original args for shell completion generation
    let original_args = args.clone();

    // Parse arguments
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => {
            // Handle help and version display (which clap treats as "errors")
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                print!("{e}");
                return Ok(0);
            }
            return Err(DbcrustArgumentError::new_err(format!(
                "Error parsing arguments: {e}"
            )));
        }
    };

    // Use CliCore for all functionality - this provides 100% feature parity
    match crate::cli_core::CliCore::run_with_args_and_original(args, Some(original_args)).await {
        Ok(exit_code) => Ok(exit_code),
        Err(e) => Err(cli_error_to_pyerr(e)),
    }
}

/// Interactive mode for Python (replicates main.rs interactive mode)
/// Run the interactive CLI with full connection URL handling
#[cfg(feature = "python")]
pub async fn run_interactive_cli(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use crate::cli::Args;
    use crate::cli_core::CliCore;

    // Create Args structure with the connection URL
    let args = Args {
        connection_url: Some(url.to_string()),
        command: Vec::new(),
        ssh_tunnel: None,
        completions: None,
        update: false,
        subcommand: None,
    };

    // Run the CLI with the constructed args
    CliCore::run_with_args(args).await.map(|_| ()).map_err(|e| {
        Box::new(std::io::Error::other(format!("CLI execution failed: {e}")))
            as Box<dyn std::error::Error>
    })
}
