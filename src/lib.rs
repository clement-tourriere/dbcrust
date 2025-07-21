#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_local_definitions)]

pub mod cli;
// mod completion; // Removed pub mod completion;
pub mod backslash_commands;
pub mod completion;
pub mod config;
pub mod database; // New database abstraction layer
pub mod database_mysql; // MySQL implementation
pub mod database_postgresql; // PostgreSQL implementation
pub mod database_sqlite; // SQLite implementation
pub mod db;
pub mod docker; // Docker container integration
pub mod format; // Made format module public
pub mod highlighter;
pub mod logging;
pub mod myconf; // MySQL configuration file support
pub mod named_queries;
pub mod pager;
pub mod password_sanitizer;
pub mod performance_analyzer; // Performance analysis for EXPLAIN queries
pub mod pgpass;
pub mod prompt;
pub mod script;
pub mod ssh_tunnel; // Add the SSH tunnel module
pub mod vault_client; // Add backslash commands module

// Note: main.rs functions are not directly accessible as modules in lib.rs
// We'll create PyO3 wrappers that call the main functionality directly

#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::types::PyDict;
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
pub use ssh_tunnel::set_debug_mode;

/// A Python wrapper for the PostgreSQL database client.
#[cfg(feature = "python")]
#[pyclass]
#[derive(Clone)] // Add Clone for PyDatabase
pub struct PyDatabase {
    inner: Arc<TokioMutex<db::Database>>, // Changed to Arc<TokioMutex<db::Database>>
    rt: Arc<Runtime>,
}

/// Python wrapper for the configuration system.
#[cfg(feature = "python")]
#[pyclass]
pub struct PyConfig {
    inner: config::Config,
}

/// A Python module implemented in Rust.
#[cfg(feature = "python")]
#[pymodule]
pub fn _internal(_py: Python<'_>, m: Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDatabase>()?;
    m.add_class::<PyConfig>()?;
    m.add_function(wrap_pyfunction!(run_command, &m)?)?;
    m.add_function(wrap_pyfunction!(run_cli_loop, &m)?)?;
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
                "Failed to create Tokio runtime: {}",
                e
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
                PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
                    "Failed to connect to database: {}",
                    e
                ))
            })?;

        Ok(Self {
            inner: Arc::new(TokioMutex::new(db_instance)), // Wrap in Arc<TokioMutex<>>
            rt: rt_clone,
        })
    }

    /// Execute a SQL query.
    #[pyo3(text_signature = "($self, query)")]
    pub fn execute_query(&self, query: &str) -> PyResult<String> {
        let owned_query = query.to_string();
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.execute_query(&owned_query).await
        });

        match result {
            Ok(results) => {
                if results.is_empty() {
                    Ok("Query OK, no results.".to_string())
                } else {
                    Ok(crate::format::format_query_results(&results).to_string())
                }
            }
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                e.to_string(),
            )),
        }
    }

    /// List all databases.
    #[pyo3(text_signature = "($self)")]
    pub fn list_databases(&self) -> PyResult<String> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.list_databases().await
        });

        match result {
            Ok(databases) => Ok(crate::format::format_query_results_psql(&databases)),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                e.to_string(),
            )),
        }
    }

    /// Connect to a different database.
    #[pyo3(text_signature = "($self, dbname)")]
    pub fn connect_to_db(&self, dbname: &str) -> PyResult<()> {
        let owned_dbname = dbname.to_string();
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.connect_to_db(&owned_dbname).await
        });

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                e.to_string(),
            )),
        }
    }

    /// List tables in the current database.
    #[pyo3(text_signature = "($self)")]
    pub fn list_tables(&self) -> PyResult<String> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.list_tables().await
        });

        match result {
            Ok(tables_data) => {
                let transformed_tables: Vec<(String, String)> = tables_data
                    .into_iter()
                    .filter_map(|row| {
                        if row.len() >= 2 {
                            Some((row[1].clone(), row[0].clone()))
                        } else {
                            eprintln!("Skipping malformed table row: {:?}", row);
                            None
                        }
                    })
                    .collect();

                let db_guard = rt.block_on(async { db_arc.lock().await });

                if transformed_tables.is_empty() && !db_guard.is_column_select_mode() {
                    Ok("No tables found.".to_string())
                } else {
                    Ok(crate::format::format_tables(&transformed_tables).to_string())
                }
            }
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                e.to_string(),
            )),
        }
    }

    /// Get details for a specific table.
    #[pyo3(text_signature = "($self, table_name)")]
    pub fn get_table_details(&self, table_name: &str) -> PyResult<String> {
        let owned_table_name = table_name.to_string();
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.get_table_details(&owned_table_name).await
        });

        match result {
            Ok(details) => Ok(crate::format::format_table_details(&details).to_string()),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                e.to_string(),
            )),
        }
    }

    /// Get current database name.
    #[getter]
    pub fn current_database(&self) -> PyResult<String> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let db_name = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.get_current_db()
        });

        Ok(db_name)
    }

    #[getter]
    pub fn host(&self) -> PyResult<String> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let host = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.get_host().to_string()
        });

        Ok(host)
    }

    #[getter]
    pub fn port(&self) -> PyResult<u16> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let port = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.get_port()
        });

        Ok(port)
    }

    #[getter]
    pub fn user(&self) -> PyResult<String> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let user = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.get_username().to_string()
        });

        Ok(user)
    }

    /// Toggle expanded display mode.
    #[pyo3(text_signature = "($self)")]
    pub fn toggle_expanded_display(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let new_state = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.toggle_expanded_display()
        });

        Ok(new_state)
    }

    /// Check if expanded display mode is enabled.
    #[pyo3(text_signature = "($self)")]
    pub fn is_expanded_display(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let state = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.is_expanded_display()
        });

        Ok(state)
    }

    /// Toggle explain mode.
    #[pyo3(text_signature = "($self)")]
    pub fn toggle_explain_mode(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let new_state = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.toggle_explain_mode()
        });

        Ok(new_state)
    }

    /// Check if explain mode is enabled.
    #[pyo3(text_signature = "($self)")]
    pub fn is_explain_mode(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let state = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.is_explain_mode()
        });

        Ok(state)
    }

    /// Toggle column selection mode.
    #[pyo3(text_signature = "($self)")]
    pub fn toggle_column_select_mode(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let new_state = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.toggle_column_select_mode()
        });

        Ok(new_state)
    }

    /// Check if column selection mode is enabled.
    #[pyo3(text_signature = "($self)")]
    pub fn is_column_select_mode(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let state = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.is_column_select_mode()
        });

        Ok(state)
    }

    /// Get the last JSON explain plan.
    #[pyo3(text_signature = "($self)")]
    pub fn get_last_json_plan(&self) -> PyResult<Option<String>> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let plan = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.get_last_json_plan()
        });

        Ok(plan)
    }

    /// Set autocomplete enabled/disabled.
    #[pyo3(text_signature = "($self, enabled)")]
    pub fn set_autocomplete(&self, enabled: bool) -> PyResult<()> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.set_autocomplete(enabled);
        });

        Ok(())
    }

    /// Check if autocomplete is enabled.
    #[pyo3(text_signature = "($self)")]
    pub fn is_autocomplete(&self) -> PyResult<bool> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let state = rt.block_on(async {
            let db_guard = db_arc.lock().await;
            db_guard.is_autocomplete()
        });

        Ok(state)
    }

    /// Preload metadata for autocomplete.
    #[pyo3(text_signature = "($self)")]
    pub fn preload_metadata(&self) -> PyResult<()> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.preload_metadata().await
        });

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Failed to preload metadata: {}",
                e
            ))),
        }
    }

    /// Execute query with raw EXPLAIN output.
    #[pyo3(text_signature = "($self, query)")]
    pub fn execute_explain_query_raw(&self, query: &str) -> PyResult<Vec<Vec<String>>> {
        let owned_query = query.to_string();
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.execute_explain_query_raw(&owned_query).await
        });

        match result {
            Ok(results) => Ok(results),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Error executing raw EXPLAIN: {}",
                e
            ))),
        }
    }

    /// Execute query with formatted EXPLAIN output.
    #[pyo3(text_signature = "($self, query)")]
    pub fn execute_explain_query_formatted(&self, query: &str) -> PyResult<Vec<Vec<String>>> {
        let owned_query = query.to_string();
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        let result = rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.execute_explain_query_formatted(&owned_query).await
        });

        match result {
            Ok(results) => Ok(results),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Error executing formatted EXPLAIN: {}",
                e
            ))),
        }
    }

    /// Close the database connection.
    #[pyo3(text_signature = "($self)")]
    pub fn close(&self) -> PyResult<()> {
        let db_arc = self.inner.clone();
        let rt = self.rt.clone();

        rt.block_on(async {
            let mut db_guard = db_arc.lock().await;
            db_guard.close().await;
        });

        Ok(())
    }
}

#[cfg(feature = "python")]
#[pymethods]
impl PyConfig {
    /// Create a new configuration object.
    #[new]
    pub fn new() -> Self {
        PyConfig {
            inner: config::Config::default(),
        }
    }

    /// Load configuration from file.
    #[staticmethod]
    pub fn load() -> Self {
        PyConfig {
            inner: config::Config::load(),
        }
    }

    /// Save configuration to file.
    pub fn save(&self) -> PyResult<()> {
        match self.inner.save() {
            Ok(_) => Ok(()),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string())),
        }
    }

    /// Get configuration as a dictionary.
    pub fn as_dict<'py>(&self, py: Python<'py>) -> PyResult<Py<PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("host", &self.inner.host)?;
        dict.set_item("port", self.inner.port)?;
        dict.set_item("user", &self.inner.user)?;
        dict.set_item("dbname", &self.inner.dbname)?;
        dict.set_item("save_password", self.inner.save_password)?;
        dict.set_item("default_limit", self.inner.default_limit)?;
        dict.set_item(
            "expanded_display_default",
            self.inner.expanded_display_default,
        )?;
        if let Some(ref password) = self.inner.password {
            dict.set_item("password", password)?;
        }
        Ok(dict.into())
    }

    /// Update configuration from a dictionary.
    pub fn update_from_dict(&mut self, dict: Bound<'_, PyDict>) -> PyResult<()> {
        if let Some(host) = dict.get_item("host")? {
            if !host.is_none() {
                self.inner.host = host.extract()?;
            }
        }
        if let Some(port) = dict.get_item("port")? {
            if !port.is_none() {
                self.inner.port = port.extract()?;
            }
        }
        if let Some(user) = dict.get_item("user")? {
            if !user.is_none() {
                self.inner.user = user.extract()?;
            }
        }
        if let Some(dbname) = dict.get_item("dbname")? {
            if !dbname.is_none() {
                self.inner.dbname = dbname.extract()?;
            }
        }
        if let Some(save_password) = dict.get_item("save_password")? {
            if !save_password.is_none() {
                self.inner.save_password = save_password.extract()?;
            }
        }
        if let Some(default_limit) = dict.get_item("default_limit")? {
            if !default_limit.is_none() {
                self.inner.default_limit = default_limit.extract()?;
            }
        }
        if let Some(expanded_display_default) = dict.get_item("expanded_display_default")? {
            if !expanded_display_default.is_none() {
                self.inner.expanded_display_default = expanded_display_default.extract()?;
            }
        }
        if let Some(password) = dict.get_item("password")? {
            if !password.is_none() {
                self.inner.password = Some(password.extract()?);
            } else {
                self.inner.password = None;
            }
        }
        Ok(())
    }
}

/// Execute a single command against a database URL and return the result.
/// This function supports both SQL queries and backslash commands.
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_command(url: &str, command: &str) -> PyResult<String> {
    use crate::database::ConnectionInfo;

    // Create a new tokio runtime for this operation
    let rt = Runtime::new().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
            "Failed to create Tokio runtime: {}",
            e
        ))
    })?;

    rt.block_on(async {
        // Parse the database URL
        let _connection_info = ConnectionInfo::parse_url(url).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Invalid database URL: {}", e))
        })?;

        // Create database connection
        let config = config::Config::load();
        let mut database = db::Database::from_url(
            url,
            Some(config.default_limit),
            Some(config.expanded_display_default),
        )
        .await
        .map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
                "Failed to connect to database: {}",
                e
            ))
        })?;

        let command_trimmed = command.trim();

        // Handle backslash commands
        if command_trimmed.starts_with('\\') {
            match command_trimmed {
                "\\l" => {
                    // List databases
                    match database.list_databases().await {
                        Ok(databases) => Ok(crate::format::format_query_results_psql(&databases)),
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error listing databases: {}",
                            e
                        ))),
                    }
                }
                "\\dt" => {
                    // List tables
                    match database.list_tables().await {
                        Ok(tables_data) => {
                            let transformed_tables: Vec<(String, String)> = tables_data
                                .into_iter()
                                .filter_map(|row| {
                                    if row.len() >= 2 {
                                        Some((row[1].clone(), row[0].clone()))
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            if transformed_tables.is_empty() {
                                Ok("No tables found.".to_string())
                            } else {
                                Ok(crate::format::format_tables(&transformed_tables).to_string())
                            }
                        }
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error listing tables: {}",
                            e
                        ))),
                    }
                }
                cmd if cmd.starts_with("\\d ") => {
                    // Describe table
                    let table_name = cmd[3..].trim();
                    if table_name.is_empty() {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                            "Table name required for \\d command".to_string(),
                        ));
                    }

                    match database.get_table_details(table_name).await {
                        Ok(details) => {
                            Ok(crate::format::format_table_details(&details).to_string())
                        }
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error getting table details: {}",
                            e
                        ))),
                    }
                }
                cmd if cmd.starts_with("\\c ") => {
                    // Connect to database
                    let db_name = cmd[3..].trim();
                    if db_name.is_empty() {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                            "Database name required for \\c command".to_string(),
                        ));
                    }

                    match database.connect_to_db(db_name).await {
                        Ok(_) => Ok(format!("Connected to database: {}", db_name)),
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error connecting to database: {}",
                            e
                        ))),
                    }
                }
                "\\x" => {
                    // Toggle expanded display
                    database.toggle_expanded_display();
                    let status = if database.is_expanded_display() {
                        "on"
                    } else {
                        "off"
                    };
                    Ok(format!("Expanded display is {}.", status))
                }
                "\\e" => {
                    // Toggle explain mode
                    database.toggle_explain_mode();
                    let status = if database.is_explain_mode() {
                        "on"
                    } else {
                        "off"
                    };
                    Ok(format!("Explain mode is {}.", status))
                }
                "\\a" => {
                    // Toggle autocomplete
                    let current_status = database.is_autocomplete();
                    database.set_autocomplete(!current_status);
                    let status = if database.is_autocomplete() {
                        "on"
                    } else {
                        "off"
                    };
                    Ok(format!("Autocomplete is {}.", status))
                }
                "\\cs" => {
                    // Toggle column selection
                    database.toggle_column_select_mode();
                    let status = if database.is_column_select_mode() {
                        "on"
                    } else {
                        "off"
                    };
                    Ok(format!("Column selection mode is {}.", status))
                }
                _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Unknown command: {}",
                    command_trimmed
                ))),
            }
        } else {
            // Handle SQL queries
            match database.execute_query(command_trimmed).await {
                Ok(results) => {
                    if results.is_empty() {
                        Ok("Query OK, no results.".to_string())
                    } else {
                        Ok(crate::format::format_query_results(&results).to_string())
                    }
                }
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Error executing query: {}",
                    e
                ))),
            }
        }
    })
}

/// Run the full CLI loop with command line arguments.
/// This function provides the same functionality as the main binary but callable from Python.
/// It replicates the exact logic from main.rs to ensure perfect feature parity.
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_cli_loop(args: Vec<String>) -> PyResult<i32> {
    // Create a new tokio runtime for this operation
    let rt = Runtime::new().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
            "Failed to create Tokio runtime: {}",
            e
        ))
    })?;

    rt.block_on(async {
        match run_main_cli_workflow(args).await {
            Ok(_) => Ok(0),
            Err(e) => {
                eprintln!("Error: {}", e);
                Ok(1)
            }
        }
    })
}

/// Replicate the exact main.rs workflow for Python integration
/// This ensures 100% feature parity with the Rust CLI
#[cfg(feature = "python")]
async fn run_main_cli_workflow(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    use crate::backslash_commands::BackslashCommandRegistry;
    use crate::cli::Args;
    use crate::config::Config as DbCrustConfig;
    use crate::db::Database;
    use crate::format::{format_query_results_expanded, format_query_results_psql};
    use crate::password_sanitizer;
    use crate::prompt::DbPrompt;
    use clap::Parser;
    use inquire;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    // Initialize the logging system
    if let Err(e) = crate::logging::init() {
        eprintln!("Warning: Failed to initialize logging: {}", e);
    }
    debug_log!("DbCrust started from Python");

    let mut config = DbCrustConfig::load();

    // Parse arguments
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => {
            // Handle help and version display (which clap treats as "errors")
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                print!("{}", e);
                return Ok(());
            }
            return Err(format!("Error parsing arguments: {}", e).into());
        }
    };

    // Handle shell completion generation if requested
    if let Some(shell) = args.completions {
        use clap::CommandFactory;
        use clap_complete::{generate, Shell as CompletionShell};
        use std::io;

        let mut cmd = Args::command();
        let shell_type = match shell {
            crate::cli::Shell::Bash => CompletionShell::Bash,
            crate::cli::Shell::Zsh => CompletionShell::Zsh,
            crate::cli::Shell::Fish => CompletionShell::Fish,
            crate::cli::Shell::PowerShell => CompletionShell::PowerShell,
            crate::cli::Shell::Elvish => CompletionShell::Elvish,
        };

        generate(shell_type, &mut cmd, "dbcrust", &mut io::stdout());
        return Ok(());
    }

    // Set SSH tunnel debug mode if --debug flag is provided
    crate::ssh_tunnel::set_debug_mode(args.debug);

    // Also enable debug logging if --debug flag is provided, overriding config
    if args.debug {
        let mut temp_config = config.clone();
        temp_config.debug_logging_enabled = true;

        if !config.debug_logging_enabled {
            debug_log!("Debug logging enabled via command line flag");
        }
        config = temp_config;
    }

    // Require connection URL to be provided
    let connection_url = match args.connection_url.clone() {
        Some(url) => url,
        None => {
            eprintln!("Connection URL is required. Use --help for usage information.");
            return Err("Connection URL is required".into());
        }
    };

    // Normalize URL if it doesn't have a scheme
    let mut full_url_str = if !connection_url.contains("://") {
        format!("postgresql://{}", connection_url)
    } else {
        connection_url
    };

    // Handle session URLs (exactly like main.rs)
    if full_url_str.starts_with("session://") {
        let session_name = full_url_str.strip_prefix("session://").unwrap_or("");

        let final_session_name = if session_name.is_empty() {
            // Interactive session selection
            let sessions = config.list_sessions();

            if sessions.is_empty() {
                eprintln!("No saved sessions found. Use \\ss <name> to save a session first.");
                return Err("No saved sessions available".into());
            }

            // Create options for inquire selection
            let mut options = Vec::new();
            for (name, session) in sessions.iter() {
                let db_type = match session.database_type {
                    crate::database::DatabaseType::PostgreSQL => "PostgreSQL",
                    crate::database::DatabaseType::MySQL => "MySQL",
                    crate::database::DatabaseType::SQLite => "SQLite",
                };
                let option = if session.database_type == crate::database::DatabaseType::SQLite {
                    if let Some(ref file_path) = session.file_path {
                        format!("{} - {} ({})", name, file_path, db_type)
                    } else {
                        format!("{} - SQLite (no path)", name)
                    }
                } else {
                    format!(
                        "{} - {}@{}:{}/{} ({})",
                        name, session.user, session.host, session.port, session.dbname, db_type
                    )
                };
                options.push(option);
            }

            // Use inquire for interactive selection
            let selected_option = inquire::Select::new("Select a saved session:", options)
                .prompt()
                .map_err(|e| format!("Selection cancelled: {}", e))?;

            // Find the session name from the selected option
            sessions
                .iter()
                .find(|(name, session)| {
                    let db_type = match session.database_type {
                        crate::database::DatabaseType::PostgreSQL => "PostgreSQL",
                        crate::database::DatabaseType::MySQL => "MySQL",
                        crate::database::DatabaseType::SQLite => "SQLite",
                    };
                    let option = if session.database_type == crate::database::DatabaseType::SQLite {
                        if let Some(ref file_path) = session.file_path {
                            format!("{} - {} ({})", name, file_path, db_type)
                        } else {
                            format!("{} - SQLite (no path)", name)
                        }
                    } else {
                        format!(
                            "{} - {}@{}:{}/{} ({})",
                            name, session.user, session.host, session.port, session.dbname, db_type
                        )
                    };
                    option == selected_option
                })
                .map(|(name, _)| name.clone())
                .ok_or("Invalid selection")?
        } else {
            session_name.to_string()
        };

        println!("🔗 Connecting to saved session '{}'...", final_session_name);

        // Get the saved session from config and reconstruct URL (same logic as main.rs)
        match config.get_session(&final_session_name) {
            Some(session) => {
                let session_url = match session.database_type {
                    crate::database::DatabaseType::SQLite => {
                        if let Some(ref file_path) = session.file_path {
                            format!("sqlite://{}", file_path)
                        } else {
                            return Err("SQLite session missing file path".into());
                        }
                    }
                    crate::database::DatabaseType::MySQL => {
                        if let Some(password) = crate::myconf::lookup_mysql_password(
                            &session.host,
                            session.port,
                            &session.dbname,
                            &session.user,
                        ) {
                            format!(
                                "mysql://{}:{}@{}:{}/{}",
                                session.user, password, session.host, session.port, session.dbname
                            )
                        } else {
                            format!(
                                "mysql://{}@{}:{}/{}",
                                session.user, session.host, session.port, session.dbname
                            )
                        }
                    }
                    crate::database::DatabaseType::PostgreSQL => {
                        if session.host.starts_with("DOCKER:") {
                            let container_name = session
                                .host
                                .strip_prefix("DOCKER:")
                                .unwrap_or(&session.host);
                            println!(
                                "🐳 Re-resolving Docker container for saved session: {}",
                                container_name
                            );
                            format!("docker://{}", container_name)
                        } else {
                            if let Some(password) = crate::pgpass::lookup_password(
                                &session.host,
                                session.port,
                                &session.dbname,
                                &session.user,
                            ) {
                                format!(
                                    "postgresql://{}:{}@{}:{}/{}",
                                    session.user,
                                    password,
                                    session.host,
                                    session.port,
                                    session.dbname
                                )
                            } else {
                                format!(
                                    "postgresql://{}@{}:{}/{}",
                                    session.user, session.host, session.port, session.dbname
                                )
                            }
                        }
                    }
                };

                full_url_str = session_url;
                println!("✓ Successfully retrieved session '{}'", final_session_name);

                // Track this connection in history
                let sanitized_url = password_sanitizer::sanitize_connection_url(&full_url_str);
                if let Err(e) = config.add_recent_connection_auto_display(
                    sanitized_url,
                    session.database_type.clone(),
                    true,
                ) {
                    debug_log!("Failed to add connection to history: {}", e);
                }
            }
            None => {
                eprintln!(
                    "Session '{}' not found. Use \\s to list available sessions.",
                    final_session_name
                );
                return Err("Session not found".into());
            }
        }
    }

    // Handle recent:// URLs for interactive recent connection selection
    if full_url_str.starts_with("recent://") {
        let recent_connections = config.get_recent_connections();

        if recent_connections.is_empty() {
            eprintln!(
                "No recent connections found. Connect to a database first to build connection history."
            );
            return Err("No recent connections available".into());
        }

        // Create options for inquire selection
        let mut options = Vec::new();
        for conn in recent_connections.iter().take(20) {
            let status = if conn.success { "✅" } else { "❌" };
            let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
            let db_type = match conn.database_type {
                crate::database::DatabaseType::PostgreSQL => "PostgreSQL",
                crate::database::DatabaseType::MySQL => "MySQL",
                crate::database::DatabaseType::SQLite => "SQLite",
            };
            let option = format!(
                "{} {} - {} ({})",
                status, conn.display_name, timestamp, db_type
            );
            options.push(option);
        }

        // Use inquire for interactive selection
        let selected_option = inquire::Select::new("Select a recent connection:", options)
            .prompt()
            .map_err(|e| format!("Selection cancelled: {}", e))?;

        // Find the index of the selected option and get connection URL
        let selected_index = recent_connections
            .iter()
            .take(20)
            .enumerate()
            .find(|(_i, conn)| {
                let status = if conn.success { "✅" } else { "❌" };
                let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
                let db_type = match conn.database_type {
                    crate::database::DatabaseType::PostgreSQL => "PostgreSQL",
                    crate::database::DatabaseType::MySQL => "MySQL",
                    crate::database::DatabaseType::SQLite => "SQLite",
                };
                let option = format!(
                    "{} {} - {} ({})",
                    status, conn.display_name, timestamp, db_type
                );
                option == selected_option
            })
            .map(|(i, _)| i)
            .ok_or("Invalid selection")?;

        let selected_connection = &recent_connections[selected_index];
        println!("🔗 Connecting to: {}", selected_connection.display_name);

        // Handle Docker connections that need re-resolution
        if selected_connection.connection_url.contains(" # Docker: ") {
            if let Some(docker_pos) = selected_connection.connection_url.find(" # Docker: ") {
                let container_name = &selected_connection.connection_url[docker_pos + 11..];
                full_url_str = format!("docker://{}", container_name);
                println!("🐳 Re-resolving Docker container: {}", container_name);
            } else {
                full_url_str = selected_connection.connection_url.clone();
            }
        } else {
            full_url_str = selected_connection.connection_url.clone();
        }
    }

    // Handle vault URLs (exactly like main.rs)
    if full_url_str.starts_with("vault://") || full_url_str.starts_with("vaultdb://") {
        // Parse vault URL and get dynamic credentials
        let vault_params = crate::vault_client::parse_vault_url(&full_url_str)
            .ok_or_else(|| format!("Invalid vault URL format: {}", full_url_str))?;

        // Get vault credentials and construct connection URL
        let (role_name, mount_path, db_name) = vault_params;
        
        // Handle interactive prompting for missing components
        let db_name = match db_name {
            Some(name) => name,
            None => {
                // List available databases and prompt user to select
                match crate::vault_client::list_vault_databases(&mount_path).await {
                    Ok(databases) => {
                        if databases.is_empty() {
                            eprintln!("No databases available at mount path '{}'", mount_path);
                            return Err("No databases available".into());
                        }
                        
                        // Filter databases to only show those with available roles
                        let accessible_databases = crate::vault_client::filter_databases_with_available_roles(&mount_path, databases).await
                            .map_err(|e| {
                                eprintln!("Error filtering databases: {}", e);
                                e
                            })?;
                        
                        if accessible_databases.is_empty() {
                            eprintln!("No accessible databases found at mount path '{}'", mount_path);
                            return Err("No accessible databases found".into());
                        }
                        
                        // Prompt user to select database
                        match inquire::Select::new("Select a database:", accessible_databases.clone()).prompt() {
                            Ok(selected) => selected,
                            Err(e) => {
                                eprintln!("Error selecting database: {}", e);
                                return Err(format!("Database selection failed: {}", e).into());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to list databases: {}", e);
                        return Err(format!("Failed to list databases: {}", e).into());
                    }
                }
            }
        };
        
        let role_name = match role_name {
            Some(name) => name,
            None => {
                // List available roles for the database and prompt user to select
                match crate::vault_client::get_available_roles_for_user(&mount_path, &db_name).await {
                    Ok(roles) => {
                        if roles.is_empty() {
                            eprintln!("No roles available for database '{}'", db_name);
                            return Err("No roles available".into());
                        }
                        
                        // Prompt user to select role
                        match inquire::Select::new("Select a role:", roles.clone()).prompt() {
                            Ok(selected) => selected,
                            Err(e) => {
                                eprintln!("Error selecting role: {}", e);
                                return Err(format!("Role selection failed: {}", e).into());
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to get available roles: {}", e);
                        return Err(format!("Failed to get available roles: {}", e).into());
                    }
                }
            }
        };

        println!("🔐 Requesting temporary database credentials from Vault...");
        println!("   This may take 5-10 seconds while Vault creates a new database user.");
        let start_time = std::time::Instant::now();
        let dynamic_creds = crate::vault_client::get_dynamic_credentials(
            &mount_path,
            &db_name,
            &role_name,
        )
        .await
        .map_err(|e| {
            eprintln!("Failed to get vault credentials: {}", e);
            e
        })?;
        let creds_elapsed = start_time.elapsed();
        println!("✓ Credentials obtained in {:.1}s", creds_elapsed.as_secs_f32());
        debug_log!("Got dynamic credentials in {:?}", creds_elapsed);

        // Get vault database config to construct final URL
        debug_log!("Getting database configuration...");
        let config_start = std::time::Instant::now();
        let db_config = crate::vault_client::get_vault_database_config(&mount_path, &db_name)
            .await
            .map_err(|e| {
                eprintln!("Failed to get vault database config: {}", e);
                e
            })?;
        debug_log!("Got database config in {:?}", config_start.elapsed());

        let connection_url_template = db_config
            .connection_details
            .connection_url
            .ok_or_else(|| "Missing connection URL in vault config")?;

        let postgres_url = crate::vault_client::construct_postgres_url(
            &connection_url_template,
            &dynamic_creds.username,
            &dynamic_creds.password,
        )
        .map_err(|e| {
            eprintln!("Failed to construct PostgreSQL URL: {}", e);
            e
        })?;

        // Use the constructed URL for connection
        println!("🔌 Connecting to database...");
        let connect_start = std::time::Instant::now();
        let database = Database::from_url(
            &postgres_url,
            Some(config.default_limit.clone()),
            Some(config.expanded_display_default.clone()),
        )
        .await
        .map_err(|e| {
            eprintln!("Failed to connect to vault database: {}", e);
            e
        })?;

        let connect_elapsed = connect_start.elapsed();
        println!("✓ Database connection established in {:.1}s", connect_elapsed.as_secs_f32());

        // Track connection in history
        let sanitized_vault_url = password_sanitizer::sanitize_connection_url(&full_url_str);
        if let Err(e) = config.add_recent_connection_auto_display(
            sanitized_vault_url,
            crate::database::DatabaseType::PostgreSQL, // Vault typically provides PostgreSQL
            true
        ) {
            debug_log!("Failed to add vault connection to history: {}", e);
        }

        // Handle commands and start interactive mode (delegate to existing logic)
        if !args.command.is_empty() {
            // Handle -c commands with vault connection
            let mut database = database;
            for command in &args.command {
                let command_trimmed = command.trim();

                // Check if this is a backslash command
                if command_trimmed.starts_with('\\') {
                    // Initialize backslash command registry for -c commands
                    let command_registry = BackslashCommandRegistry::new();
                    let db_arc = Arc::new(Mutex::new(database));
                    let mut last_script = String::new();
                    let interrupt_flag = Arc::new(AtomicBool::new(false));

                    // Create prompt with single database lock to avoid deadlock
                    let (username, db_name) = {
                        let db_guard = db_arc.lock().unwrap();
                        (
                            db_guard.get_username().to_string(),
                            db_guard.get_current_db(),
                        )
                    };
                    let mut prompt = DbPrompt::with_config(
                        username,
                        db_name,
                        config.multiline_prompt_indicator.clone(),
                    );

                    match command_registry
                        .execute(
                            command_trimmed,
                            &db_arc,
                            &mut config,
                            &mut last_script,
                            &interrupt_flag,
                            &mut prompt,
                        )
                        .await
                    {
                        Ok(should_exit) => {
                            if should_exit {
                                return Ok(());
                            }
                        }
                        Err(e) => {
                            eprintln!("Error executing command: {}", e);
                            return Err(e);
                        }
                    }

                    // Update the database reference
                    database = Arc::try_unwrap(db_arc)
                        .map_err(|_| "Failed to unwrap Arc")?
                        .into_inner()
                        .map_err(|_| "Failed to unwrap Mutex")?;
                } else {
                    // Execute the SQL command
                    match database.execute_query(command_trimmed).await {
                        Ok(results) => {
                            if results.is_empty() {
                                // No output for commands that don't return results
                            } else {
                                // Format and display the results
                                if database.is_expanded_display() {
                                    let tables = format_query_results_expanded(&results);
                                    for table in tables {
                                        println!("{}", table);
                                    }
                                } else {
                                    let formatted_output = format_query_results_psql(&results);
                                    println!("{}", formatted_output);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error executing query: {}", e);
                        }
                    }
                }
            }
            return Ok(());
        } else {
            // Start interactive mode with vault connection
            return run_python_interactive_mode(database, config, args).await;
        }
    }

    // Create database connection
    let (mut database, docker_connection_info) = if full_url_str.starts_with("docker://") {
        Database::from_docker_url_with_tracking(
            &full_url_str,
            Some(config.default_limit),
            Some(config.expanded_display_default),
        )
        .await
        .map_err(|e| {
            eprintln!("Failed to connect to database: {}", e);
            eprintln!(
                "Connection URL: {}",
                password_sanitizer::sanitize_connection_url(&full_url_str)
            );
            e
        })?
    } else {
        let database = Database::from_url(
            &full_url_str,
            Some(config.default_limit),
            Some(config.expanded_display_default),
        )
        .await
        .map_err(|e| {
            eprintln!("Failed to connect to database: {}", e);
            eprintln!(
                "Connection URL: {}",
                password_sanitizer::sanitize_connection_url(&full_url_str)
            );
            e
        })?;
        (database, None)
    };

    println!("✓ Successfully connected to database");

    // Track connection in history
    let (database_type, connection_url_for_history) =
        if let Some(resolved_info) = docker_connection_info {
            let resolved_url = resolved_info.to_url();
            let sanitized_url = password_sanitizer::sanitize_connection_url(&resolved_url);
            (resolved_info.database_type, sanitized_url)
        } else {
            let database_type = if full_url_str.starts_with("postgresql://") {
                crate::database::DatabaseType::PostgreSQL
            } else if full_url_str.starts_with("mysql://") {
                crate::database::DatabaseType::MySQL
            } else if full_url_str.starts_with("sqlite://") {
                crate::database::DatabaseType::SQLite
            } else {
                crate::database::DatabaseType::PostgreSQL
            };

            let sanitized_url = password_sanitizer::sanitize_connection_url(&full_url_str);
            (database_type, sanitized_url)
        };

    if let Err(e) =
        config.add_recent_connection_auto_display(connection_url_for_history, database_type, true)
    {
        debug_log!("Failed to add connection to history: {}", e);
    }

    // Handle -c commands if provided (execute and exit)
    if !args.command.is_empty() {
        for command in &args.command {
            let command_trimmed = command.trim();

            if command_trimmed.is_empty() {
                continue;
            }

            // Check if this is a backslash command
            if command_trimmed.starts_with('\\') {
                // Initialize backslash command registry for -c commands
                let command_registry = BackslashCommandRegistry::new();
                let db_arc = Arc::new(Mutex::new(database));
                let mut last_script = String::new();
                let interrupt_flag = Arc::new(AtomicBool::new(false));

                // Create prompt with single database lock to avoid deadlock
                let (username, db_name) = {
                    let db_guard = db_arc.lock().unwrap();
                    (
                        db_guard.get_username().to_string(),
                        db_guard.get_current_db(),
                    )
                };
                let mut prompt = DbPrompt::with_config(
                    username,
                    db_name,
                    config.multiline_prompt_indicator.clone(),
                );

                match command_registry
                    .execute(
                        command_trimmed,
                        &db_arc,
                        &mut config,
                        &mut last_script,
                        &interrupt_flag,
                        &mut prompt,
                    )
                    .await
                {
                    Ok(should_exit) => {
                        if should_exit {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        eprintln!("Error executing command: {}", e);
                        return Err(e);
                    }
                }

                // Update the database reference
                database = Arc::try_unwrap(db_arc)
                    .map_err(|_| "Failed to unwrap Arc")?
                    .into_inner()
                    .map_err(|_| "Failed to unwrap Mutex")?;
                continue;
            }

            // Execute the SQL command
            match database.execute_query(command_trimmed).await {
                Ok(results) => {
                    if results.is_empty() {
                        // No output for commands that don't return results
                    } else {
                        // Format and display the results
                        if database.is_expanded_display() {
                            let expanded_tables = format_query_results_expanded(&results);
                            for table in expanded_tables {
                                println!("{}", table);
                            }
                        } else {
                            // Use psql-style formatting
                            let output = format_query_results_psql(&results);
                            print!("{}", output);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error executing command: {}", e);
                    return Err(e);
                }
            }
        }
        return Ok(());
    }

    // Start interactive mode (exactly like main.rs)
    run_python_interactive_mode(database, config, args).await
}

/// Interactive mode for Python (replicates main.rs interactive mode)
#[cfg(feature = "python")]
async fn run_python_interactive_mode(
    database: Database,
    mut config: crate::config::Config,
    args: crate::cli::Args,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::backslash_commands::BackslashCommandRegistry;
    use crate::completion::{NoopCompleter, SqlCompleter};
    use crate::format::{format_query_results_expanded, format_query_results_psql};
    use crate::highlighter::SqlHighlighter;
    use crate::prompt::DbPrompt;
    use nu_ansi_term::{Color, Style};
    use reedline::{
        default_emacs_keybindings, ColumnarMenu, Completer, DefaultHinter, EditCommand, Emacs, FileBackedHistory,
        KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu,
        Signal,
    };
    use signal_hook::{consts::SIGINT, flag};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    let db_arc = Arc::new(Mutex::new(database));

    // Preload database metadata in parallel to warm up caches for interactive mode
    if config.debug_logging_enabled {
        debug_log!("Preloading database metadata for interactive mode...");
    }
    {
        let mut db_guard = db_arc.lock().unwrap();
        if let Err(e) = db_guard.preload_metadata().await {
            if config.debug_logging_enabled {
                debug_log!("Error preloading metadata: {}", e);
            }
        }
    }

    let completer: Box<dyn Completer> = if config.autocomplete_enabled {
        Box::new(SqlCompleter::new(db_arc.clone()))
    } else {
        Box::new(NoopCompleter {})
    };

    let completion_menu = Box::new(
        ColumnarMenu::default()
            .with_name("completion_menu")
            .with_text_style(Style::new().fg(Color::Green)),
    );

    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );

    // Add multi-line support: Shift+Enter, Ctrl+Enter, and Alt+Enter for newlines
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    keybindings.add_binding(
        KeyModifiers::ALT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );

    let edit_mode = Box::new(Emacs::new(keybindings));

    let hinter =
        Box::new(DefaultHinter::default().with_style(Style::new().italic().fg(Color::LightGray)));

    let history_path = crate::config::get_config_dir()
        .map(|dir| dir.join("history"))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not determine home directory")
                .join(".dbcrust_history")
        });

    let history = Box::new(match FileBackedHistory::with_file(1000, history_path) {
        Ok(history) => history,
        Err(e) => {
            eprintln!("Warning: Could not create history file: {}", e);
            FileBackedHistory::default()
        }
    });

    let mut line_editor = Reedline::create()
        .with_completer(completer)
        .with_edit_mode(edit_mode)
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_hinter(hinter)
        .with_highlighter(Box::new(SqlHighlighter::new()))
        .with_history(history);

    let db = db_arc.lock().unwrap();
    let username = db.get_username().to_string();
    let db_name = db.get_current_db();
    drop(db);
    let mut prompt =
        DbPrompt::with_config(username, db_name, config.multiline_prompt_indicator.clone());

    // Only show help message in interactive mode
    if args.command.is_empty() {
        println!("Type \\h for help");
    }

    let interrupt_flag = Arc::new(AtomicBool::new(false));
    // Register a signal handler for SIGINT (Ctrl-C)
    flag::register(SIGINT, Arc::clone(&interrupt_flag))?;

    // Keep track of the last executed query or edited script
    let mut last_script = String::new();

    // Initialize backslash command registry
    let command_registry = BackslashCommandRegistry::new();

    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input_trimmed = input.trim();

                if input_trimmed.is_empty() {
                    continue;
                }

                // Handle special commands
                if input_trimmed.starts_with('\\') {
                    match command_registry
                        .execute(
                            input_trimmed,
                            &db_arc,
                            &mut config,
                            &mut last_script,
                            &interrupt_flag,
                            &mut prompt,
                        )
                        .await
                    {
                        Ok(should_exit) => {
                            if should_exit {
                                break;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error executing command: {}", e);
                        }
                    }
                } else {
                    // Regular SQL query
                    last_script = input_trimmed.to_string();

                    let mut db = db_arc.lock().unwrap();
                    match db.execute_query(input_trimmed).await {
                        Ok(results) => {
                            if results.is_empty() {
                                println!("Query OK, no results.");
                            } else {
                                // Format and display the results
                                if db.is_expanded_display() {
                                    let expanded_tables = format_query_results_expanded(&results);
                                    for table in expanded_tables {
                                        println!("{}", table);
                                    }
                                } else {
                                    // Use psql-style formatting
                                    let output = format_query_results_psql(&results);
                                    print!("{}", output);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error: {}", e);
                        }
                    }
                }
            }
            Signal::CtrlC => {
                println!("^C");
                continue;
            }
            Signal::CtrlD => {
                println!("\nGoodbye!");
                break;
            }
        }
    }

    Ok(())
}

/// Run the interactive CLI with full connection URL handling (session://, vault://, docker://, recent://)
/// This function implements the same connection logic as main.rs

#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        let _ = $crate::logging::debug(&format!($($arg)*));
    };
}

#[macro_export]
macro_rules! debug_time {
    ($operation:expr, $time:expr) => {
        let _ = $crate::logging::debug(&format!("{} took {:?}", $operation, $time));
    };
}
