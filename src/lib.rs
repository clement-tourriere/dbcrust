#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_local_definitions)]

mod cli;
// mod completion; // Removed pub mod completion;
pub mod completion;
pub mod config;
pub mod database; // New database abstraction layer
pub mod database_postgresql; // PostgreSQL implementation
pub mod database_sqlite; // SQLite implementation
pub mod database_mysql; // MySQL implementation
pub mod db;
pub mod format; // Made format module public
pub mod performance_analyzer; // Performance analysis for EXPLAIN queries
pub mod highlighter;
pub mod logging;
pub mod myconf; // MySQL configuration file support
pub mod named_queries;
pub mod pager;
pub mod password_sanitizer;
pub mod pgpass;
pub mod prompt;
pub mod script;
pub mod ssh_tunnel; // Add the SSH tunnel module
pub mod vault_client;
pub mod docker; // Docker container integration
pub mod backslash_commands; // Add backslash commands module

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
        let _connection_info = ConnectionInfo::parse_url(url)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Invalid database URL: {}", e
            )))?;
        
        // Create database connection
        let config = config::Config::load();
        let mut database = db::Database::from_url(
            url,
            Some(config.default_limit),
            Some(config.expanded_display_default),
        )
        .await
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyConnectionError, _>(format!(
            "Failed to connect to database: {}", e
        )))?;
        
        let command_trimmed = command.trim();
        
        // Handle backslash commands
        if command_trimmed.starts_with('\\') {
            match command_trimmed {
                "\\l" => {
                    // List databases
                    match database.list_databases().await {
                        Ok(databases) => Ok(crate::format::format_query_results_psql(&databases)),
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error listing databases: {}", e
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
                            "Error listing tables: {}", e
                        ))),
                    }
                }
                cmd if cmd.starts_with("\\d ") => {
                    // Describe table
                    let table_name = cmd[3..].trim();
                    if table_name.is_empty() {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                            "Table name required for \\d command".to_string()
                        ));
                    }
                    
                    match database.get_table_details(table_name).await {
                        Ok(details) => Ok(crate::format::format_table_details(&details).to_string()),
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error getting table details: {}", e
                        ))),
                    }
                }
                cmd if cmd.starts_with("\\c ") => {
                    // Connect to database
                    let db_name = cmd[3..].trim();
                    if db_name.is_empty() {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                            "Database name required for \\c command".to_string()
                        ));
                    }
                    
                    match database.connect_to_db(db_name).await {
                        Ok(_) => Ok(format!("Connected to database: {}", db_name)),
                        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Error connecting to database: {}", e
                        ))),
                    }
                }
                "\\x" => {
                    // Toggle expanded display
                    database.toggle_expanded_display();
                    let status = if database.is_expanded_display() { "on" } else { "off" };
                    Ok(format!("Expanded display is {}.", status))
                }
                "\\e" => {
                    // Toggle explain mode
                    database.toggle_explain_mode();
                    let status = if database.is_explain_mode() { "on" } else { "off" };
                    Ok(format!("Explain mode is {}.", status))
                }
                "\\a" => {
                    // Toggle autocomplete
                    let current_status = database.is_autocomplete();
                    database.set_autocomplete(!current_status);
                    let status = if database.is_autocomplete() { "on" } else { "off" };
                    Ok(format!("Autocomplete is {}.", status))
                }
                "\\cs" => {
                    // Toggle column selection
                    database.toggle_column_select_mode();
                    let status = if database.is_column_select_mode() { "on" } else { "off" };
                    Ok(format!("Column selection mode is {}.", status))
                }
                _ => {
                    Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Unknown command: {}", command_trimmed
                    )))
                }
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
                    "Error executing query: {}", e
                ))),
            }
        }
    })
}

/// Run the full CLI loop with command line arguments.
/// This function provides the same functionality as the main binary but callable from Python.
#[cfg(feature = "python")]
#[pyfunction]
pub fn run_cli_loop(args: Vec<String>) -> PyResult<i32> {
    use crate::cli::Args;
    use clap::Parser;
    
    // Parse arguments
    let parsed_args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(e) => {
            // Handle help and version display (which clap treats as "errors")
            if e.kind() == clap::error::ErrorKind::DisplayHelp 
                || e.kind() == clap::error::ErrorKind::DisplayVersion {
                print!("{}", e);
                return Ok(0);
            }
            eprintln!("Error parsing arguments: {}", e);
            return Ok(1);
        }
    };
    
    // Handle show debug logs
    if parsed_args.show_debug_logs {
        match crate::logging::get_log_file_path_string() {
            Some(log_path) => {
                println!("Debug logs are written to: {}", log_path);
                return Ok(0);
            }
            None => {
                eprintln!("Debug logging is not enabled");
                return Ok(1);
            }
        }
    }
    
    // Generate shell completions
    if let Some(shell) = parsed_args.generate_completion {
        use clap::CommandFactory;
        use clap_complete::{generate, Shell as CompletionShell};
        use std::io;
        
        let mut app = Args::command();
        let shell_type = match shell {
            crate::cli::Shell::Bash => CompletionShell::Bash,
            crate::cli::Shell::Zsh => CompletionShell::Zsh,
            crate::cli::Shell::Fish => CompletionShell::Fish,
            crate::cli::Shell::PowerShell => CompletionShell::PowerShell,
            crate::cli::Shell::Elvish => CompletionShell::Elvish,
        };
        
        if let Some(output_path) = parsed_args.completion_out {
            let mut file = std::fs::File::create(output_path).map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyIOError, _>(format!(
                    "Failed to create completion file: {}", e
                ))
            })?;
            generate(shell_type, &mut app, "dbcrust", &mut file);
        } else {
            generate(shell_type, &mut app, "dbcrust", &mut io::stdout());
        }
        return Ok(0);
    }
    
    // Handle direct commands
    if !parsed_args.command.is_empty() {
        // Determine connection URL
        let connection_url = if let Some(ref url) = parsed_args.connection_url {
            url.clone()
        } else if let Some(ref url) = parsed_args.url {
            url.clone()
        } else {
            // Build URL from individual parameters
            let password = parsed_args.password.unwrap_or_default();
            if password.is_empty() {
                format!("postgresql://{}@{}:{}/{}", 
                    parsed_args.user, 
                    parsed_args.host, 
                    parsed_args.port, 
                    parsed_args.dbname
                )
            } else {
                format!("postgresql://{}:{}@{}:{}/{}", 
                    parsed_args.user, 
                    password,
                    parsed_args.host, 
                    parsed_args.port, 
                    parsed_args.dbname
                )
            }
        };
        
        // Execute each command using the existing run_command function
        for command in parsed_args.command {
            match run_command(&connection_url, &command) {
                Ok(result) => println!("{}", result),
                Err(e) => {
                    eprintln!("Error executing command '{}': {}", command, e);
                    return Ok(1);
                }
            }
        }
        return Ok(0);
    }
    
    // For interactive mode, launch the interactive CLI
    // Create a new tokio runtime for this operation
    let rt = Runtime::new().map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
            "Failed to create Tokio runtime: {}", e
        ))
    })?;
    
    rt.block_on(async {
        match run_interactive_cli(parsed_args).await {
            Ok(exit_code) => Ok(exit_code),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Error in interactive mode: {}", e
            )))
        }
    })
}

/// Run the interactive CLI with parsed arguments.
/// This function implements the full interactive CLI mode with rich features like
/// autocomplete, syntax highlighting, history, etc. that can be called from Python.
#[cfg(feature = "python")]
async fn run_interactive_cli(args: crate::cli::Args) -> Result<i32, Box<dyn std::error::Error>> {
    use crate::completion::{SqlCompleter, NoopCompleter};
    use crate::highlighter::SqlHighlighter;
    use crate::prompt::DbPrompt;
    use reedline::{
        ColumnarMenu, Completer, DefaultHinter, Emacs, EditCommand, FileBackedHistory, KeyCode, KeyModifiers,
        ReedlineEvent, ReedlineMenu, default_emacs_keybindings, Reedline, Signal, MenuBuilder
    };
    use nu_ansi_term::{Color, Style};
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::AtomicBool;
    use signal_hook::{consts::SIGINT, flag};
    
    // Initialize logging
    if let Err(e) = crate::logging::init() {
        eprintln!("Warning: Failed to initialize logging: {}", e);
    }
    
    // Load configuration
    let config = crate::config::Config::load();
    
    // Determine connection URL
    let connection_url = if let Some(ref url) = args.connection_url {
        url.clone()
    } else if let Some(ref url) = args.url {
        url.clone()
    } else {
        // Build URL from individual parameters
        let password = args.password.unwrap_or_default();
        if password.is_empty() {
            format!("postgresql://{}@{}:{}/{}", 
                args.user, 
                args.host, 
                args.port, 
                args.dbname
            )
        } else {
            format!("postgresql://{}:{}@{}:{}/{}", 
                args.user, 
                password,
                args.host, 
                args.port, 
                args.dbname
            )
        }
    };
    
    // Create database connection
    let database = crate::db::Database::from_url(
        &connection_url,
        Some(config.default_limit),
        Some(config.expanded_display_default),
    ).await?;
    
    // Print banner if not disabled
    if !args.no_banner {
        print_banner();
        println!("Connected to database: {}", database.get_current_db());
    }
    
    // Set up the rich interactive environment similar to main.rs
    let db_arc = Arc::new(Mutex::new(database));
    
    // Preload database metadata for autocomplete
    {
        let mut db_guard = db_arc.lock().unwrap();
        if let Err(e) = db_guard.preload_metadata().await {
            eprintln!("Warning: Failed to preload metadata: {}", e);
        }
    }
    
    // Set up autocompletion
    let completer: Box<dyn Completer> = if config.autocomplete_enabled {
        Box::new(SqlCompleter::new(db_arc.clone()))
    } else {
        Box::new(NoopCompleter {})
    };
    
    // Set up completion menu
    let completion_menu = Box::new(
        ColumnarMenu::default()
            .with_name("completion_menu")
            .with_text_style(Style::new().fg(Color::Green)),
    );
    
    // Set up keybindings
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
    
    // Set up syntax highlighting
    let hinter = Box::new(DefaultHinter::default().with_style(Style::new().italic().fg(Color::LightGray)));
    
    // Set up history
    let history_path = dirs::config_dir()
        .map(|dir| dir.join("dbcrust").join("history"))
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
    
    // Create the reedline editor
    let mut line_editor = Reedline::create()
        .with_completer(completer)
        .with_edit_mode(edit_mode)
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_hinter(hinter)
        .with_highlighter(Box::new(SqlHighlighter::new()))
        .with_history(history);
    
    // Set up the prompt
    let db = db_arc.lock().unwrap();
    let username = db.get_username().to_string();
    let db_name = db.get_current_db();
    drop(db);
    let mut prompt = DbPrompt::with_config(username, db_name, config.multiline_prompt_indicator.clone());
    
    // Set up signal handling
    let interrupt_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGINT, Arc::clone(&interrupt_flag))?;
    
    // Keep track of the last executed query or edited script
    let mut last_script = String::new();
    
    println!("Type \\h for help");
    
    // Main interactive loop
    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input_trimmed = input.trim();
                
                if input_trimmed.is_empty() {
                    continue;
                }
                
                // Handle special commands
                if input_trimmed.starts_with('\\') {
                    if handle_backslash_command_python(
                        input_trimmed,
                        &db_arc,
                        &mut last_script,
                        &interrupt_flag,
                        &mut prompt,
                    ).await? {
                        break; // Exit if command requests it (like \q)
                    }
                } else {
                    // Regular SQL query
                    let mut db = db_arc.lock().unwrap();
                    match db.execute_query(input_trimmed).await {
                        Ok(results) => {
                            if results.is_empty() {
                                println!("Query OK, no results.");
                            } else {
                                process_query_results_python(&mut db, results, &interrupt_flag).await?;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error executing query: {}", e);
                        }
                    }
                }
            }
            Signal::CtrlC => {
                // Handle Ctrl-C gracefully
                println!("\nType \\q to quit or continue with your query.");
                continue;
            }
            Signal::CtrlD => {
                // Handle Ctrl-D (EOF) as quit
                println!("\nGoodbye!");
                break;
            }
        }
    }
    
    // Close the database connection
    let mut db = db_arc.lock().unwrap();
    db.close().await;
    
    Ok(0)
}

/// Handle backslash commands in Python interactive mode, returns true if should exit
#[cfg(feature = "python")]
async fn handle_backslash_command_python(
    input: &str,
    db_arc: &Arc<std::sync::Mutex<crate::db::Database>>,
    _last_script: &mut String,
    _interrupt_flag: &Arc<std::sync::atomic::AtomicBool>,
    prompt: &mut crate::prompt::DbPrompt,
) -> Result<bool, Box<dyn std::error::Error>> {
    use crate::format::{format_query_results_psql};
    
    match input {
        "\\q" => return Ok(true), // Signal to exit
        "\\h" => print_help_python(),
        "\\e" => {
            let mut db = db_arc.lock().unwrap();
            let mode = db.toggle_explain_mode();
            println!("EXPLAIN mode is {}", if mode { "on" } else { "off" });
        }
        cmd if cmd.starts_with("\\er ") => {
            // Execute query with raw EXPLAIN output
            let query = cmd[4..].trim();
            if query.is_empty() {
                println!("Error: Please provide a query after \\er");
                return Ok(false);
            }

            let mut db = db_arc.lock().unwrap();
            match db.execute_explain_query_raw(query).await {
                Ok(results) => {
                    println!("Raw EXPLAIN Output:");
                    for row in &results {
                        for cell in row {
                            println!("{}", cell);
                        }
                    }
                }
                Err(e) => eprintln!("Error executing raw EXPLAIN: {}", e),
            }
        }
        cmd if cmd.starts_with("\\ef ") => {
            // Execute query with formatted EXPLAIN output only
            let query = cmd[4..].trim();
            if query.is_empty() {
                println!("Error: Please provide a query after \\ef");
                return Ok(false);
            }

            let mut db = db_arc.lock().unwrap();
            match db.execute_explain_query_formatted(query).await {
                Ok(formatted_output) => {
                    println!("Formatted EXPLAIN Output:");
                    println!("{}", crate::format::format_query_results(&formatted_output));
                }
                Err(e) => eprintln!("Error executing formatted EXPLAIN: {}", e),
            }
        }
        "\\x" => {
            let mut db = db_arc.lock().unwrap();
            db.toggle_expanded_display();
            let status = if db.is_expanded_display() { "on" } else { "off" };
            println!("Expanded display is {}.", status);
        }
        "\\a" => {
            let mut db = db_arc.lock().unwrap();
            let current_status = db.is_autocomplete();
            db.set_autocomplete(!current_status);
            let status = if db.is_autocomplete() { "on" } else { "off" };
            println!("Autocomplete is {}.", status);
        }
        "\\cs" => {
            let mut db = db_arc.lock().unwrap();
            db.toggle_column_select_mode();
            let status = if db.is_column_select_mode() { "on" } else { "off" };
            println!("Column selection mode is {}.", status);
        }
        "\\l" => {
            let mut db = db_arc.lock().unwrap();
            match db.list_databases().await {
                Ok(databases) => {
                    let formatted = format_query_results_psql(&databases);
                    println!("{}", formatted);
                }
                Err(e) => eprintln!("Error listing databases: {}", e),
            }
        }
        "\\dt" => {
            let mut db = db_arc.lock().unwrap();
            match db.list_tables().await {
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
                        println!("No tables found.");
                    } else {
                        println!("{}", crate::format::format_tables(&transformed_tables));
                    }
                }
                Err(e) => eprintln!("Error listing tables: {}", e),
            }
        }
        cmd if cmd.starts_with("\\d ") => {
            let table_name = cmd[3..].trim();
            if table_name.is_empty() {
                println!("Error: Please provide a table name after \\d");
                return Ok(false);
            }
            
            let mut db = db_arc.lock().unwrap();
            match db.get_table_details(table_name).await {
                Ok(details) => {
                    println!("{}", crate::format::format_table_details(&details));
                }
                Err(e) => eprintln!("Error getting table details: {}", e),
            }
        }
        cmd if cmd.starts_with("\\c ") => {
            let db_name = cmd[3..].trim();
            if db_name.is_empty() {
                println!("Error: Please provide a database name after \\c");
                return Ok(false);
            }
            
            let mut db = db_arc.lock().unwrap();
            match db.connect_to_db(db_name).await {
                Ok(_) => {
                    println!("Connected to database: {}", db_name);
                    // Update the prompt to reflect the new database
                    let username = db.get_username().to_string();
                    let new_db_name = db.get_current_db();
                    *prompt = crate::prompt::DbPrompt::with_config(username, new_db_name, "".to_string());
                }
                Err(e) => eprintln!("Error connecting to database: {}", e),
            }
        }
        _ => {
            println!("Unknown command: {}", input);
            println!("Type \\h for help");
        }
    }
    
    Ok(false)
}

/// Process query results with proper formatting and paging in Python mode
#[cfg(feature = "python")]
async fn process_query_results_python(
    database: &mut crate::db::Database,
    results: Vec<Vec<String>>,
    interrupt_flag: &Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::format::{format_query_results, format_query_results_expanded};
    use std::sync::atomic::Ordering;
    
    if interrupt_flag.load(Ordering::SeqCst) {
        println!("Query interrupted by user.");
        interrupt_flag.store(false, Ordering::SeqCst);
        return Ok(());
    }
    
    // For Python mode, we'll use simple println instead of pager for now
    // This could be enhanced later to detect terminal capabilities
    if database.is_expanded_display() {
        let tables = format_query_results_expanded(&results);
        for table in tables {
            println!("{}", table);
        }
    } else {
        let formatted_output = format_query_results(&results);
        println!("{}", formatted_output);
    }
    
    Ok(())
}

/// Print help information for Python interactive mode
#[cfg(feature = "python")]
fn print_help_python() {
    println!("Available commands:");
    println!("  Multi-line input:");
    println!("  Alt+Enter       - Insert newline (continue multi-line query) [Primary]");
    println!("  Shift+Enter     - Insert newline (may not work in all terminals)");
    println!("  Ctrl+Enter      - Insert newline (may not work in all terminals)");
    println!("  Enter           - Execute query/command");
    println!();
    println!("  \\q              - Quit");
    println!("  \\h              - Show this help");
    println!("  \\l              - List databases");
    println!("  \\dt             - List tables");
    println!("  \\d <table>      - Describe table");
    println!("  \\c <database>   - Connect to database");
    println!("  \\x              - Toggle expanded display");
    println!("  \\e              - Toggle explain mode");
    println!("  \\a              - Toggle autocomplete");
    println!("  \\cs             - Toggle column selection mode");
    println!("  \\er <query>     - Execute query with raw EXPLAIN output");
    println!("  \\ef <query>     - Execute query with formatted EXPLAIN output");
    println!();
    println!("For more information, visit: https://github.com/ctourriere/dbcrust");
}

/// Print the DBCRUST ASCII art banner
#[cfg(feature = "python")]
fn print_banner() {
    let banner = r#"
  _____    ____    _____  _____   _    _   _____  _______
 |  __ \  |  _ \  / ____|/ ____| | |  | | / ____||__   __|
 | |  | | | |_) || |    | |  __  | |  | || (___     | |   
 | |  | | |  _ < | |    | | |_ | | |  | | \___ \    | |   
 | |__| | | |_) || |____| |__| | | |__| | ____) |   | |   
 |_____/  |____/  \_____\_____/  \______||_____/    |_|   
                Multi-Database Interactive Client
"#;
    
    println!("{}", banner);
}


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
