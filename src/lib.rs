#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_local_definitions)]

pub mod cli;
pub mod cli_core; // New unified CLI core
pub mod command_completion; // Trait-based command completion system
pub mod commands; // New type-safe enum-based command system
pub mod completion;
pub mod completion_provider; // Database-agnostic completion trait
pub mod complex_display; // Unified display system for complex data types
pub mod config;
pub mod database; // New database abstraction layer
pub mod database_clickhouse; // ClickHouse implementation
pub mod database_elasticsearch; // Elasticsearch implementation
pub mod database_mongodb; // MongoDB implementation
pub mod database_mysql; // MySQL implementation
pub mod database_postgresql; // PostgreSQL implementation
pub mod database_sqlite; // SQLite implementation
pub mod db;
pub mod docker; // Docker container integration
pub mod format; // Made format module public
pub mod geojson_display;
pub mod highlighter;
pub mod history_manager; // Per-session command history management
pub mod json_display; // JSON display implementation
pub mod logging;
pub mod myconf; // MySQL configuration file support
pub mod named_queries;
pub mod pager;
pub mod password_sanitizer;
pub mod performance_analyzer; // Performance analysis for EXPLAIN queries
pub mod pgpass;
pub mod prompt;
pub mod script;
pub mod shell_completion; // Custom shell completion with URL schemes
pub mod sql_context; // SQL context analysis for better autocompletion
pub mod sql_parser; // Enhanced SQL parser for autocompletion
pub mod sql_parser_mysql; // MySQL-specific SQL parser
pub mod sql_parser_postgresql; // PostgreSQL-specific SQL parser
pub mod sql_parser_sqlite; // SQLite-specific SQL parser
pub mod sql_parser_trait; // Database-specific SQL parser trait system
pub mod ssh_tunnel; // Add the SSH tunnel module
pub mod url_scheme; // URL scheme autocompletion support
pub mod vault_client; // Add backslash commands module
pub mod vault_encryption; // Vault credential encryption utilities
pub mod vector_display; // Vector visualization for PostgreSQL extensions // GeoJSON display implementation

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

    /// Save the configuration.
    pub fn save(&self) -> PyResult<()> {
        self.inner
            .save()
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
        let mut rows = Vec::new();

        for i in self.current_row_index..current_results.len() {
            rows.push(PyRow {
                data: current_results[i].clone(),
                column_names: self.column_names.clone(),
            });
        }

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
    };

    // Run the CLI with the constructed args
    CliCore::run_with_args(args).await.map(|_| ()).map_err(|e| {
        Box::new(std::io::Error::other(format!("CLI execution failed: {e}")))
            as Box<dyn std::error::Error>
    })
}
