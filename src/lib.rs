#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_local_definitions)]

pub mod cli;
pub mod cli_core; // New unified CLI core
pub mod commands; // New type-safe enum-based command system
pub mod command_completion; // Trait-based command completion system
pub mod completion;
pub mod completion_provider; // Database-agnostic completion trait
pub mod config;
pub mod database; // New database abstraction layer
pub mod database_mysql; // MySQL implementation
pub mod database_postgresql; // PostgreSQL implementation
pub mod database_sqlite; // SQLite implementation
pub mod db;
pub mod docker; // Docker container integration
pub mod format; // Made format module public
pub mod highlighter;
pub mod history_manager; // Per-session command history management
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
pub mod ssh_tunnel; // Add the SSH tunnel module
pub mod url_scheme; // URL scheme autocompletion support
pub mod vault_client; // Add backslash commands module
pub mod vault_encryption; // Vault credential encryption utilities


// Note: main.rs functions are not directly accessible as modules in lib.rs
// We'll create PyO3 wrappers that call the main functionality directly

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

        Ok(PyDatabase {
            inner: Arc::new(TokioMutex::new(db_instance)),
            rt: rt_clone,
        })
    }

    /// Execute a query and return the results.
    pub fn execute(&self, query: &str) -> PyResult<PyObject> {
        let results = self.rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.execute_query(query).await
            })
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Query execution failed: {}",
                    e
                ))
            })?;

        Python::with_gil(|py| {
            Ok(results.into_pyobject(py)?.into_any().unbind())
        })
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
        let results = self.rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.list_databases().await
            })
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Failed to list databases: {}",
                    e
                ))
            })?;

        Python::with_gil(|py| {
            Ok(results.into_pyobject(py)?.into_any().unbind())
        })
    }

    /// List all tables.
    pub fn list_tables(&self) -> PyResult<PyObject> {
        let results = self.rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.list_tables().await
            })
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Failed to list tables: {}",
                    e
                ))
            })?;

        Python::with_gil(|py| {
            Ok(results.into_pyobject(py)?.into_any().unbind())
        })
    }

    /// Describe a table.
    pub fn describe_table(&self, table_name: &str) -> PyResult<PyObject> {
        let table_details = self.rt
            .block_on(async {
                let mut db = self.inner.lock().await;
                db.get_table_details(table_name).await
            })
            .map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Failed to describe table: {}",
                    e
                ))
            })?;

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
        self.inner.save().map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Failed to save config: {}",
                e
            ))
        })
    }
}

/// Python function to run a command using the new unified CLI system
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_command(args: Vec<String>) -> PyResult<()> {
    let rt = Runtime::new().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
            "Failed to create Tokio runtime: {}",
            e
        ))
    })?;

    rt.block_on(run_main_cli_workflow(args)).map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
            "CLI command execution failed: {}",
            e
        ))
    })
}

/// Python function to run the interactive CLI loop
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_cli_loop(connection_url: Option<String>) -> PyResult<()> {
    let rt = Runtime::new().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
            "Failed to create Tokio runtime: {}",
            e
        ))
    })?;

    rt.block_on(async {
        match connection_url {
            Some(url) => run_interactive_cli(&url).await,
            None => {
                eprintln!("Connection URL is required for interactive mode");
                Err("Connection URL required".into())
            }
        }
    })
    .map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!("Interactive CLI failed: {}", e))
    })
}

/// Unified CLI workflow using CliCore - provides 100% feature parity with Rust CLI
#[cfg(feature = "python")]
async fn run_main_cli_workflow(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
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
                print!("{}", e);
                return Ok(());
            }
            return Err(format!("Error parsing arguments: {}", e).into());
        }
    };

    // Use CliCore for all functionality - this provides 100% feature parity
    match crate::cli_core::CliCore::run_with_args_and_original(args, Some(original_args)).await {
        Ok(_exit_code) => Ok(()),
        Err(e) => Err(format!("CLI execution failed: {}", e).into()),
    }
}

/// Interactive mode for Python (replicates main.rs interactive mode)
/// Run the interactive CLI with full connection URL handling
#[cfg(feature = "python")]
pub async fn run_interactive_cli(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    use crate::cli_core::CliCore;
    use crate::cli::Args;
    
    // Create Args structure with the connection URL
    let args = Args {
        connection_url: Some(url.to_string()),
        command: Vec::new(),
        ssh_tunnel: None,
        debug: false,
        completions: None,
        no_banner: false,
        verbosity: None,
    };
    
    // Run the CLI with the constructed args
    CliCore::run_with_args(args).await.map(|_| ()).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("CLI execution failed: {}", e)
        )) as Box<dyn std::error::Error>
    })
}
