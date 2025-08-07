use crate::cli::Args;
use crate::commands::{CommandExecutor, CommandParser, CommandResult};
use crate::completion::{NoopCompleter, SqlCompleter};
use crate::config::Config as DbCrustConfig;
use crate::database::{ConnectionInfo, DatabaseTypeExt};
use crate::db::Database;
use crate::format::{format_query_results_expanded, format_query_results_psql_with_info};
use crate::history_manager::{SessionHistoryManager, SessionId};
use crate::prompt::DbPrompt;
use crate::{logging, pager};
use clap::CommandFactory;
use dirs;
use inquire;
use nu_ansi_term::{Color, Style};
use std::error::Error as StdError;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use terminal_size;
use tracing::debug;
use url;

/// Core CLI functionality shared between Rust and Python interfaces
pub struct CliCore {
    pub config: DbCrustConfig,
    pub database: Option<Database>,
    pub connection_info: Option<ConnectionInfo>,
}

#[derive(Debug)]
pub enum CliError {
    ConnectionError(String),
    CommandError(String),
    ConfigError(String),
    ArgumentError(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::ConnectionError(msg) => {
                // Check for specific Vault errors and provide user-friendly messages
                if msg.contains("Failed to list Vault databases") && msg.contains("403 Forbidden") {
                    if msg.contains("invalid token") {
                        write!(
                            f,
                            "❌ Vault authentication failed (403 Forbidden)\n\n\
                                  The Vault token appears to be invalid or expired.\n\n\
                                  To fix this issue:\n\
                                  1. Check your Vault token: $VAULT_TOKEN or ~/.vault-token file\n\
                                  2. Ensure the token is valid: vault token lookup\n\
                                  3. If expired, authenticate again: vault login\n\
                                  4. Verify you have permissions for the database mount path\n\n\
                                  For more details, set 'level = \"debug\"' in config.toml."
                        )
                    } else if msg.contains("permission denied") {
                        write!(
                            f,
                            "❌ Vault access denied (403 Forbidden)\n\n\
                                  You don't have permission to access this Vault mount path.\n\n\
                                  To fix this issue:\n\
                                  1. Check your Vault policies: vault token lookup\n\
                                  2. Verify the mount path is correct (default: 'database')\n\
                                  3. Contact your Vault administrator for access\n\n\
                                  For more details, set 'level = \"debug\"' in config.toml."
                        )
                    } else {
                        write!(f, "❌ Vault authentication failed (403 Forbidden)\n\n{msg}")
                    }
                } else if msg.contains("Vault address not set") {
                    write!(
                        f,
                        "❌ Vault configuration error\n\n\
                              The VAULT_ADDR environment variable is not set.\n\n\
                              To fix this issue:\n\
                              export VAULT_ADDR='https://your-vault-server:8200'\n\n\
                              Replace the URL with your actual Vault server address."
                    )
                } else if msg.contains("Vault token not found") {
                    write!(
                        f,
                        "❌ Vault authentication required\n\n\
                              No Vault token found.\n\n\
                              To authenticate with Vault:\n\
                              1. Set environment variable: export VAULT_TOKEN='your-token'\n\
                              2. Or save token to file: echo 'your-token' > ~/.vault-token\n\
                              3. Or authenticate: vault login"
                    )
                } else if msg.contains("Failed to get Vault credentials") && msg.contains("404") {
                    write!(
                        f,
                        "❌ Vault role or database not found\n\n\
                              The specified role or database configuration doesn't exist.\n\n\
                              Please check:\n\
                              1. The database configuration exists in Vault\n\
                              2. The role name is correct\n\
                              3. The mount path is correct (default: 'database')"
                    )
                } else {
                    // Default connection error formatting
                    write!(f, "{msg}")
                }
            }
            CliError::CommandError(msg) => write!(f, "{msg}"),
            CliError::ConfigError(msg) => write!(f, "Configuration error: {msg}"),
            CliError::ArgumentError(msg) => write!(f, "Argument error: {msg}"),
        }
    }
}

impl StdError for CliError {}

impl From<Box<dyn StdError>> for CliError {
    fn from(err: Box<dyn StdError>) -> Self {
        CliError::CommandError(err.to_string())
    }
}

impl Default for CliCore {
    fn default() -> Self {
        Self {
            config: DbCrustConfig::load(),
            database: None,
            connection_info: None,
        }
    }
}

impl CliCore {
    /// Create a new CLI core instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Main entry point for CLI execution - replaces async_main_with_args
    pub async fn run_with_args(args: Args) -> Result<i32, CliError> {
        Self::run_with_args_and_original(args, None).await
    }

    /// Main entry point with original args for shell completion generation
    pub async fn run_with_args_and_original(
        args: Args,
        original_args: Option<Vec<String>>,
    ) -> Result<i32, CliError> {
        // Initialize the logging system
        if let Err(e) = logging::init() {
            eprintln!("Warning: Failed to initialize logging: {e}");
        }
        debug!("DbCrust CLI Core started");

        let mut cli_core = Self::new();

        // Handle shell completion generation if requested
        if let Some(shell) = args.completions {
            // Pass the binary name from the original args if available
            let binary_name = original_args
                .as_ref()
                .and_then(|args| args.first())
                .map(|arg| {
                    std::path::Path::new(arg)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("dbcrust")
                        .to_string()
                })
                .unwrap_or_else(|| "dbcrust".to_string());
            cli_core.handle_shell_completion(shell, &binary_name)?;
            return Ok(0);
        }

        // Log system information
        cli_core.log_system_info(&args);

        // SSH tunnel debug output now handled by tracing system

        // Check if commands can be handled without database connection first
        if !args.command.is_empty()
            && cli_core.can_handle_commands_without_connection(&args.command)
        {
            cli_core
                .handle_command_mode_standalone(&args.command)
                .await?;
            return Ok(0);
        }

        // Handle connection and database setup if connection URL provided
        if args.connection_url.is_some() {
            cli_core.handle_database_connection(&args).await?;

            // Handle -c commands if provided (execute and exit)
            if !args.command.is_empty() {
                cli_core.handle_command_mode(&args).await?;
                return Ok(0);
            }

            // Start interactive mode with database connection
            cli_core.run_interactive_mode().await?;
        } else {
            // No connection URL provided
            if !args.command.is_empty() {
                return Err(CliError::ArgumentError(
                    "Database connection required for SQL commands. Use backslash commands like \\h for help without connection.".to_string()
                ));
            }

            // Start interactive mode without initial connection
            cli_core.run_interactive_mode_no_connection().await?;
        }

        Ok(0)
    }

    /// Handle shell completion generation
    fn handle_shell_completion(
        &self,
        shell: crate::cli::Shell,
        binary_name: &str,
    ) -> Result<(), CliError> {
        use crate::shell_completion::generate_completion_with_url_schemes;
        use clap_complete::Shell as CompletionShell;

        let mut cmd = Args::command();
        let shell_type = match shell {
            crate::cli::Shell::Bash => CompletionShell::Bash,
            crate::cli::Shell::Zsh => CompletionShell::Zsh,
            crate::cli::Shell::Fish => CompletionShell::Fish,
            crate::cli::Shell::PowerShell => CompletionShell::PowerShell,
            crate::cli::Shell::Elvish => CompletionShell::Elvish,
        };

        generate_completion_with_url_schemes(shell_type, &mut cmd, binary_name, &mut io::stdout())
            .map_err(|e| CliError::CommandError(format!("Failed to generate completion: {e}")))?;
        Ok(())
    }

    /// Log system information for debugging
    fn log_system_info(&self, args: &Args) {
        debug!("Operating System: {}", std::env::consts::OS);
        debug!("Architecture: {}", std::env::consts::ARCH);
        debug!("CLI Arguments: {args:?}");

        if let Some(terminal_size) = terminal_size::terminal_size() {
            debug!("Terminal size: {}x{}", terminal_size.0.0, terminal_size.1.0);
        }

        if let Ok(user) = std::env::var("USER") {
            debug!("User: {user}");
        }

        if let Ok(pwd) = std::env::var("PWD") {
            debug!("Working directory: {pwd}");
        }
    }

    /// Check if commands can be handled without database connection
    fn can_handle_commands_without_connection(&self, commands: &[String]) -> bool {
        commands.iter().all(|cmd| {
            let trimmed = cmd.trim();
            // Only help and some informational commands can run without connection
            trimmed == "\\h"
                || trimmed == "\\help"
                || trimmed == "\\?"
                || trimmed == "\\s"
                || trimmed == "\\r"
                || trimmed.starts_with("\\config")
        })
    }

    /// Handle standalone command mode (commands that don't require database connection)
    async fn handle_command_mode_standalone(
        &mut self,
        commands: &[String],
    ) -> Result<(), CliError> {
        for command in commands {
            let command_trimmed = command.trim();

            match command_trimmed {
                "\\h" | "\\help" | "\\?" => {
                    println!("{}", Self::get_categorized_help());
                }
                "\\s" => {
                    // List saved sessions
                    let sessions = self.config.list_sessions();
                    if sessions.is_empty() {
                        println!("No saved sessions found. Use \\ss <name> to save a session.");
                    } else {
                        println!("Saved sessions:");
                        for (name, session) in sessions {
                            let db_type = session.database_type.display_name();
                            if session.database_type.is_file_based() {
                                if let Some(ref file_path) = session.file_path {
                                    println!("  {name} - {file_path} ({db_type})");
                                } else {
                                    println!("  {name} - SQLite (no path)");
                                }
                            } else {
                                println!(
                                    "  {} - {}@{}:{}/{} ({})",
                                    name,
                                    session.user,
                                    session.host,
                                    session.port,
                                    session.dbname,
                                    db_type
                                );
                            }
                        }
                    }
                }
                "\\r" => {
                    // List recent connections
                    let recent = self.config.get_recent_connections();
                    if recent.is_empty() {
                        println!("No recent connections found.");
                    } else {
                        println!("Recent connections:");
                        for (i, conn) in recent.iter().take(10).enumerate() {
                            let status = if conn.success { "✅" } else { "❌" };
                            let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
                            println!(
                                "  {} {} {} - {}",
                                i + 1,
                                status,
                                conn.display_name,
                                timestamp
                            );
                        }
                    }
                }
                cmd if cmd.starts_with("\\config") => {
                    println!("Current configuration:");
                    println!("  Default limit: {}", self.config.default_limit);
                    println!(
                        "  Expanded display: {}",
                        self.config.expanded_display_default
                    );
                    println!(
                        "  Autocomplete enabled: {}",
                        self.config.autocomplete_enabled
                    );
                    println!("  Pager enabled: {}", self.config.pager_enabled);
                    println!("  Logging level: {}", self.config.logging.level);
                }
                _ => {
                    eprintln!("Command '{command_trimmed}' requires a database connection");
                    return Err(CliError::CommandError(
                        "Database connection required for this command".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Interactive mode without initial database connection
    async fn run_interactive_mode_no_connection(&mut self) -> Result<(), CliError> {
        // Show banner if config allows it
        if self.config.show_banner {
            Self::print_banner(&self.config);
        }

        println!("Welcome to DBCrust! No database connected yet.");
        println!();
        println!("To connect to a database, provide a connection URL:");
        println!("  dbc postgres://user@host:5432/database");
        println!("  dbc session://saved_session_name");
        println!(
            "  dbc recent://                        # Interactive recent connection selection"
        );
        println!();
        println!("Examples:");
        println!("  dbc postgres://user@localhost/mydb");
        println!("  dbc sqlite:///path/to/file.db");
        println!("  dbc docker://postgres-container");
        println!();
        println!("For more help: dbc --help");
        Ok(())
    }

    /// Print the banner (moved from main.rs)
    fn print_banner(config: &DbCrustConfig) {
        use nu_ansi_term::Color;

        let banner = r#"
██████╗ ██████╗  ██████╗██████╗ ██╗   ██╗███████╗████████╗
██╔══██╗██╔══██╗██╔════╝██╔══██╗██║   ██║██╔════╝╚══██╔══╝
██║  ██║██████╔╝██║     ██████╔╝██║   ██║███████╗   ██║
██║  ██║██╔══██╗██║     ██╔══██╗██║   ██║╚════██║   ██║
██████╔╝██████╔╝╚██████╗██║  ██║╚██████╔╝███████║   ██║
╚═════╝ ╚═════╝  ╚═════╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝   ╚═╝
        "#;

        println!("{}", Color::Cyan.bold().paint(banner));
        println!(
            "SELECT queries use a default limit of {} rows. Use \\config to change defaults.",
            config.default_limit
        );
    }

    /// Handle database connection setup - core connection logic
    pub async fn handle_database_connection(&mut self, args: &Args) -> Result<(), CliError> {
        let connection_url = args.connection_url.clone().ok_or_else(|| {
            CliError::ArgumentError("No database connection specified".to_string())
        })?;

        // Normalize URL if it doesn't have a scheme
        let mut full_url_str = if !connection_url.contains("://") {
            format!("postgres://{connection_url}")
        } else {
            connection_url
        };

        // Handle different URL schemes
        full_url_str = self.handle_special_url_schemes(full_url_str).await?;

        // Handle vault URLs
        if full_url_str.starts_with("vault://") {
            let (database, connection_info) = self.handle_vault_connection(&full_url_str).await?;

            // Track vault connection in history with vault metadata
            // Reconstruct the complete vault URL from metadata (like saved sessions do)
            let complete_vault_url = if let Some(ref conn_info) = connection_info {
                if let (Some(vault_mount), Some(vault_database), Some(vault_role)) = (
                    conn_info.options.get("vault_mount"),
                    conn_info.options.get("vault_database"),
                    conn_info.options.get("vault_role"),
                ) {
                    if vault_role.is_empty() {
                        format!("vault://{vault_mount}/{vault_database}")
                    } else {
                        format!("vault://{vault_role}@{vault_mount}/{vault_database}")
                    }
                } else {
                    full_url_str.to_string()
                }
            } else {
                full_url_str.to_string()
            };

            let options = if let Some(ref conn_info) = connection_info {
                conn_info.options.clone()
            } else {
                std::collections::HashMap::new()
            };

            if let Err(e) = self.config.add_recent_connection_with_options(
                complete_vault_url,
                crate::database::DatabaseType::PostgreSQL, // Vault connections are typically PostgreSQL
                true,
                options,
            ) {
                debug!("Failed to add vault connection to history: {}", e);
            }

            self.database = Some(database);
            self.connection_info = connection_info;
            return Ok(());
        }

        // Create database connection
        let (database, connection_info) = if full_url_str.starts_with("docker://") {
            crate::db::Database::from_docker_url_with_tracking(
                &full_url_str,
                Some(self.config.default_limit),
                Some(self.config.expanded_display_default),
            )
            .await
            .map_err(|e| {
                eprintln!("Failed to connect to database: {e}");
                eprintln!(
                    "Connection URL: {}",
                    crate::password_sanitizer::sanitize_connection_url(&full_url_str)
                );
                CliError::ConnectionError(e.to_string())
            })?
        } else {
            let database = crate::db::Database::from_url(
                &full_url_str,
                Some(self.config.default_limit),
                Some(self.config.expanded_display_default),
            )
            .await
            .map_err(|e| {
                eprintln!("Failed to connect to database: {e}");
                eprintln!(
                    "Connection URL: {}",
                    crate::password_sanitizer::sanitize_connection_url(&full_url_str)
                );
                CliError::ConnectionError(e.to_string())
            })?;
            (database, None)
        };

        // Track connection in history
        let (database_type, connection_url_for_history) = if let Some(ref resolved_info) =
            connection_info
        {
            let resolved_url = resolved_info.to_url();
            let sanitized_url = crate::password_sanitizer::sanitize_connection_url(&resolved_url);
            (resolved_info.database_type.clone(), sanitized_url)
        } else {
            // Extract scheme from URL and use from_scheme method
            let database_type = if let Some(scheme_end) = full_url_str.find("://") {
                let scheme = &full_url_str[..scheme_end];
                crate::database::DatabaseType::from_scheme(scheme)
                    .unwrap_or(crate::database::DatabaseType::PostgreSQL)
            } else {
                crate::database::DatabaseType::PostgreSQL
            };

            let sanitized_url = crate::password_sanitizer::sanitize_connection_url(&full_url_str);
            (database_type, sanitized_url)
        };

        if let Err(e) = self.config.add_recent_connection_auto_display(
            connection_url_for_history,
            database_type,
            true,
        ) {
            debug!("Failed to add connection to history: {}", e);
        }

        self.database = Some(database);
        self.connection_info = connection_info;

        // Show success message
        println!("✓ Successfully connected to database");
        Ok(())
    }

    /// Handle -c command mode (execute commands and exit)
    async fn handle_command_mode(&mut self, args: &Args) -> Result<(), CliError> {
        for command in &args.command {
            let command_trimmed = command.trim();

            if command_trimmed.starts_with('\\') {
                // Handle backslash commands
                self.execute_backslash_command(command_trimmed).await?;
            } else {
                // Execute SQL command
                let database = self
                    .database
                    .as_mut()
                    .ok_or_else(|| CliError::CommandError("No database connection".to_string()))?;

                match database
                    .execute_query_with_info_no_column_selection(command_trimmed)
                    .await
                {
                    Ok(results_with_info) => {
                        if !results_with_info.data.is_empty() {
                            let is_expanded = database.is_expanded_display();
                            if is_expanded {
                                let tables = format_query_results_expanded(&results_with_info.data);
                                let mut combined_output = String::new();
                                for table in tables {
                                    combined_output.push_str(&format!("{table}\n"));
                                }
                                Self::page_or_print(&combined_output, &self.config)?;
                            } else {
                                let formatted_output = format_query_results_psql_with_info(
                                    &results_with_info.data,
                                    results_with_info.column_info.as_ref(),
                                );
                                Self::page_or_print(&formatted_output, &self.config)?;
                            }
                        }
                    }
                    Err(e) => {
                        // Check if this is a column selection abort
                        if e.to_string().contains("Column selection aborted") {
                            // Just return to REPL without error message
                            return Ok(());
                        }
                        eprintln!("Error executing query: {e}");
                    }
                }
            }
        }

        Ok(())
    }

    /// Execute a backslash command using the new type-safe command system
    async fn execute_backslash_command(&mut self, command_str: &str) -> Result<(), CliError> {
        // Parse string command into typed Command enum
        let command = CommandParser::parse(command_str)
            .map_err(|e| CliError::CommandError(format!("Command parsing failed: {e}")))?;

        let database = self
            .database
            .take()
            .ok_or_else(|| CliError::CommandError("No database connection".to_string()))?;

        let db_arc = Arc::new(Mutex::new(database));
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));

        // Create prompt
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
            self.config.multiline_prompt_indicator.clone(),
        );

        // Execute the typed command using the CommandExecutor trait
        match command
            .execute(
                &db_arc,
                &mut self.config,
                &mut last_script,
                &interrupt_flag,
                &mut prompt,
            )
            .await
        {
            Ok(CommandResult::Exit) => {
                return Err(CliError::CommandError("Exit requested".to_string()));
            }
            Ok(CommandResult::Continue) => {
                // Command executed successfully, continue
            }
            Ok(CommandResult::Output(output)) => {
                println!("{output}");
            }
            Ok(CommandResult::Error(error)) => {
                eprintln!("Command error: {error}");
            }
            Err(e) => {
                eprintln!("Error executing command: {e}");
            }
        }

        // Update database reference
        let updated_db = Arc::try_unwrap(db_arc)
            .map_err(|_| CliError::CommandError("Failed to unwrap database Arc".to_string()))?
            .into_inner()
            .map_err(|_| CliError::CommandError("Failed to unwrap database Mutex".to_string()))?;

        self.database = Some(updated_db);
        Ok(())
    }

    /// Run interactive mode - core interactive logic
    pub async fn run_interactive_mode(&mut self) -> Result<(), CliError> {
        use crate::highlighter::SqlHighlighter;
        use reedline::{Reedline, Signal};

        let database = self
            .database
            .take()
            .ok_or_else(|| CliError::CommandError("No database connection".to_string()))?;

        // Show banner if config allows it
        if self.config.show_banner {
            Self::print_banner(&self.config);
        }

        let db_arc = Arc::new(Mutex::new(database));
        let config_arc = Arc::new(Mutex::new(self.config.clone()));
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));

        // Create prompt
        let (username, db_name) = {
            let db_guard = db_arc.lock().unwrap();
            (
                db_guard.get_username().to_string(),
                db_guard.get_current_db(),
            )
        };

        let mut prompt = DbPrompt::with_config(
            username.clone(),
            db_name.clone(),
            self.config.multiline_prompt_indicator.clone(),
        );

        // Create shared state for full line buffer access
        let full_line_buffer = Arc::new(Mutex::new(None::<String>));

        // Create highlighter for SQL syntax
        let highlighter = SqlHighlighter::new(full_line_buffer.clone());

        // Set up reedline components exactly as in the working version
        use reedline::{
            ColumnarMenu, DefaultHinter, Emacs, FileBackedHistory, KeyCode, KeyModifiers,
            MenuBuilder, ReedlineEvent, ReedlineMenu, default_emacs_keybindings,
        };

        // Set up completion menu
        let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));

        // Set up keybindings (this enables Tab completion!)
        let mut keybindings = default_emacs_keybindings();
        keybindings.add_binding(
            KeyModifiers::NONE,
            KeyCode::Tab,
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::Menu("completion_menu".to_string()),
                ReedlineEvent::MenuNext,
            ]),
        );

        // Add Shift+Tab for navigating up through suggestions
        keybindings.add_binding(
            KeyModifiers::SHIFT,
            KeyCode::BackTab,
            ReedlineEvent::MenuPrevious,
        );

        let edit_mode = Box::new(Emacs::new(keybindings));

        // Set up hinter
        let hinter = Box::new(
            DefaultHinter::default().with_style(Style::new().italic().fg(Color::LightGray)),
        );

        // Set up session-based history
        let history = match SessionHistoryManager::new(&self.config) {
            Ok(mut history_manager) => {
                // Try to generate session ID from database connection
                let session_id = {
                    let db_guard = db_arc.lock().unwrap();
                    SessionId::from_database(&db_guard)
                };

                if let Some(session_id) = session_id {
                    debug!(
                        "Using session-based history for: {}",
                        session_id.display_name
                    );
                    history_manager.get_session_history(&session_id)
                } else {
                    debug!("No session info available, using default history");
                    history_manager.get_default_history()
                }
            }
            Err(e) => {
                debug!(
                    "Failed to create session history manager: {}, using fallback history",
                    e
                );
                // Fallback to default history creation
                let history_path = crate::config::Config::get_config_dir()
                    .map(|dir| dir.join("history"))
                    .unwrap_or_else(|_| {
                        dirs::home_dir()
                            .expect("Could not determine home directory")
                            .join(".dbcrust_history")
                    });
                Box::new(
                    FileBackedHistory::with_file(50, history_path)
                        .unwrap_or_else(|_| FileBackedHistory::default()),
                )
            }
        };

        // Create completer and editor with full configuration
        let completer = if self.config.autocomplete_enabled {
            Box::new(SqlCompleter::new_with_line_buffer(
                db_arc.clone(),
                config_arc.clone(),
                full_line_buffer.clone(),
            )) as Box<dyn reedline::Completer>
        } else {
            Box::new(NoopCompleter {}) as Box<dyn reedline::Completer>
        };

        let mut line_editor = Reedline::create()
            .use_bracketed_paste(true) // Enable bracketed paste for multi-line pasted content
            .with_completer(completer)
            .with_edit_mode(edit_mode)
            .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
            .with_hinter(hinter)
            .with_highlighter(Box::new(highlighter))
            .with_history(history);

        println!("Connected! Type \\h for help or \\q to quit.");

        // Main interactive loop
        loop {
            let sig = line_editor
                .read_line(&prompt)
                .map_err(|e| CliError::CommandError(format!("Read line error: {e}")))?;

            match sig {
                Signal::Success(buffer) => {
                    let line = buffer.trim();

                    // If empty input but we have a last_script, execute it
                    if line.is_empty() {
                        if !last_script.is_empty() {
                            println!(
                                "Executing last script ({} lines)...",
                                last_script.lines().count()
                            );
                            match self
                                .execute_sql_interactive(&last_script, &db_arc, &interrupt_flag)
                                .await
                            {
                                Ok(_) => {}
                                Err(e) => {
                                    eprintln!("SQL error: {e}");
                                }
                            }
                        }
                        continue;
                    }

                    // Handle backslash commands
                    if line.starts_with('\\') {
                        match self
                            .execute_backslash_command_interactive(
                                line,
                                &db_arc,
                                &config_arc,
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
                                eprintln!("Command error: {e}");
                            }
                        }
                        continue;
                    }

                    // Handle SQL queries (reedline handles multiline with Alt+Enter automatically)
                    match self
                        .execute_sql_interactive(line, &db_arc, &interrupt_flag)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("SQL error: {e}");
                        }
                    }
                }
                Signal::CtrlC => {
                    // Handle interrupt - just continue to next prompt
                    println!("^C");
                    continue;
                }
                Signal::CtrlD => {
                    // Exit on Ctrl+D
                    println!("Goodbye!");
                    break;
                }
            }
        }

        // Update database reference
        match Arc::try_unwrap(db_arc) {
            Ok(mutex) => match mutex.into_inner() {
                Ok(updated_db) => {
                    self.database = Some(updated_db);
                }
                Err(_) => {
                    debug!("Failed to unwrap database mutex");
                }
            },
            Err(_) => {
                debug!("Failed to unwrap database Arc");
            }
        }

        // Update config reference to persist changes from command execution
        match Arc::try_unwrap(config_arc) {
            Ok(mutex) => match mutex.into_inner() {
                Ok(updated_config) => {
                    self.config = updated_config;
                }
                Err(_) => {
                    debug!("Failed to unwrap config mutex");
                }
            },
            Err(_) => {
                debug!("Failed to unwrap config Arc");
            }
        }

        Ok(())
    }

    /// Execute backslash command in interactive mode - returns whether to exit
    async fn execute_backslash_command_interactive(
        &mut self,
        command_str: &str,
        db_arc: &Arc<Mutex<Database>>,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
        last_script: &mut String,
        interrupt_flag: &Arc<AtomicBool>,
        prompt: &mut DbPrompt,
    ) -> Result<bool, CliError> {
        // Parse string command into typed Command enum
        let command = CommandParser::parse(command_str)
            .map_err(|e| CliError::CommandError(format!("Command parsing failed: {e}")))?;

        // Execute the typed command using the CommandExecutor trait
        match command
            .execute(
                db_arc,
                &mut config_arc.lock().unwrap(),
                last_script,
                interrupt_flag,
                prompt,
            )
            .await
        {
            Ok(CommandResult::Exit) => Ok(true),      // Signal exit
            Ok(CommandResult::Continue) => Ok(false), // Continue interactive loop
            Ok(CommandResult::Output(output)) => {
                println!("{output}");
                Ok(false)
            }
            Ok(CommandResult::Error(error)) => {
                eprintln!("Command error: {error}");
                Ok(false)
            }
            Err(e) => {
                eprintln!("Error executing command: {e}");
                Ok(false)
            }
        }
    }

    /// Execute SQL query in interactive mode
    async fn execute_sql_interactive(
        &mut self,
        sql: &str,
        db_arc: &Arc<Mutex<Database>>,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<(), CliError> {
        let results_with_info = {
            let mut db_guard = db_arc.lock().unwrap();
            match db_guard
                .execute_query_with_interrupt_and_info(sql, interrupt_flag)
                .await
            {
                Ok(results_with_info) => results_with_info,
                Err(e) => {
                    // Check if this is a column selection abort
                    if e.to_string().contains("Column selection aborted") {
                        // Return Ok to go back to REPL without error
                        return Ok(());
                    }
                    return Err(CliError::CommandError(e.to_string()));
                }
            }
        };

        if !results_with_info.data.is_empty() {
            let is_expanded = {
                let db_guard = db_arc.lock().unwrap();
                db_guard.is_expanded_display()
            };

            if is_expanded {
                let tables = format_query_results_expanded(&results_with_info.data);
                let mut combined_output = String::new();
                for table in tables {
                    combined_output.push_str(&format!("{table}\n"));
                }
                Self::page_or_print(&combined_output, &self.config)?;
            } else {
                let formatted_output = format_query_results_psql_with_info(
                    &results_with_info.data,
                    results_with_info.column_info.as_ref(),
                );
                Self::page_or_print(&formatted_output, &self.config)?;
            }
        }

        Ok(())
    }

    /// Handle special URL schemes like session:// and recent://
    async fn handle_special_url_schemes(&mut self, mut url: String) -> Result<String, CliError> {
        // Handle session URLs
        if url.starts_with("session://") {
            url = self.handle_session_url(&url).await?;
        }

        // Handle recent URLs
        if url.starts_with("recent://") {
            url = self.handle_recent_url().await?;
        }

        Ok(url)
    }

    /// Handle session:// URLs
    async fn handle_session_url(&mut self, url: &str) -> Result<String, CliError> {
        let session_name = url.strip_prefix("session://").unwrap_or("");

        let final_session_name = if session_name.is_empty() {
            // Interactive session selection
            let sessions = self.config.list_sessions();

            if sessions.is_empty() {
                return Err(CliError::ConnectionError(
                    "No saved sessions found. Use \\ss <name> to save a session first.".to_string(),
                ));
            }

            // Create options for inquire selection
            let mut options = Vec::new();
            for (name, session) in sessions.iter() {
                let db_type = session.database_type.display_name();
                let option = if session.database_type.is_file_based() {
                    if let Some(ref file_path) = session.file_path {
                        format!("{name} - {file_path} ({db_type})")
                    } else {
                        format!("{name} - SQLite (no path)")
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
                .map_err(|e| CliError::ConnectionError(format!("Selection cancelled: {e}")))?;

            // Find the session name from the selected option
            sessions
                .iter()
                .find(|(name, session)| {
                    let db_type = session.database_type.display_name();
                    let option = if session.database_type.is_file_based() {
                        if let Some(ref file_path) = session.file_path {
                            format!("{name} - {file_path} ({db_type})")
                        } else {
                            format!("{name} - SQLite (no path)")
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
                .ok_or_else(|| CliError::ConnectionError("Invalid selection".to_string()))?
        } else {
            session_name.to_string()
        };

        println!("🔗 Connecting to saved session '{final_session_name}'...");

        // Get the saved session from config and reconstruct URL
        match self.config.get_session(&final_session_name) {
            Some(session) => {
                let session_url = if session.database_type.is_file_based() {
                    if session.host.starts_with("DOCKER:") {
                        let container_name = session
                            .host
                            .strip_prefix("DOCKER:")
                            .unwrap_or(&session.host);
                        println!(
                            "🐳 Re-resolving Docker container for saved session: {container_name}"
                        );
                        format!("docker://{container_name}")
                    } else if let Some(ref file_path) = session.file_path {
                        format!("sqlite://{file_path}")
                    } else {
                        return Err(CliError::ConnectionError(
                            "SQLite session missing file path".to_string(),
                        ));
                    }
                } else {
                    // Handle network-based database types (PostgreSQL, MySQL)
                    if session.host.starts_with("DOCKER:") {
                        let container_name = session
                            .host
                            .strip_prefix("DOCKER:")
                            .unwrap_or(&session.host);
                        println!(
                            "🐳 Re-resolving Docker container for saved session: {container_name}"
                        );
                        format!("docker://{container_name}")
                    } else if let (Some(vault_mount), Some(vault_database), Some(vault_role)) = (
                        session.options.get("vault_mount"),
                        session.options.get("vault_database"),
                        session.options.get("vault_role"),
                    ) {
                        // This is a Vault session, reconstruct vault:// URL for fresh credentials
                        println!(
                            "🔐 Re-obtaining Vault credentials for saved session: {vault_database}"
                        );
                        if vault_role.is_empty() {
                            format!("vault://{vault_mount}/{vault_database}")
                        } else {
                            format!("vault://{vault_role}@{vault_mount}/{vault_database}")
                        }
                    } else {
                        // Use trait method to get URL scheme and handle password lookup
                        let url_scheme = session.database_type.url_scheme();

                        // Try to get password from appropriate credential store
                        let password = match session.database_type {
                            crate::database::DatabaseType::PostgreSQL => {
                                crate::pgpass::lookup_password(
                                    &session.host,
                                    session.port,
                                    &session.dbname,
                                    &session.user,
                                )
                            }
                            crate::database::DatabaseType::MySQL => {
                                crate::myconf::lookup_mysql_password(
                                    &session.host,
                                    session.port,
                                    &session.dbname,
                                    &session.user,
                                )
                            }
                            _ => None,
                        };

                        if let Some(password) = password {
                            format!(
                                "{}://{}:{}@{}:{}/{}",
                                url_scheme,
                                session.user,
                                password,
                                session.host,
                                session.port,
                                session.dbname
                            )
                        } else {
                            format!(
                                "{}://{}@{}:{}/{}",
                                url_scheme,
                                session.user,
                                session.host,
                                session.port,
                                session.dbname
                            )
                        }
                    }
                };

                println!("✓ Successfully retrieved session '{final_session_name}'");

                // Track this connection in history
                let sanitized_url =
                    crate::password_sanitizer::sanitize_connection_url(&session_url);
                if let Err(e) = self.config.add_recent_connection_auto_display(
                    sanitized_url,
                    session.database_type.clone(),
                    true,
                ) {
                    debug!("Failed to add connection to history: {}", e);
                }

                Ok(session_url)
            }
            None => Err(CliError::ConnectionError(format!(
                "Session '{final_session_name}' not found. Use \\s to list available sessions."
            ))),
        }
    }

    /// Handle recent:// URLs
    async fn handle_recent_url(&mut self) -> Result<String, CliError> {
        let recent_connections = self.config.get_recent_connections();

        if recent_connections.is_empty() {
            return Err(CliError::ConnectionError(
                "No recent connections found. Connect to a database first to build connection history.".to_string()
            ));
        }

        // Create options for inquire selection
        let mut options = Vec::new();
        for conn in recent_connections.iter().take(20) {
            let status = if conn.success { "✅" } else { "❌" };
            let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
            let db_type = conn.database_type.display_name();
            let option = format!(
                "{} {} - {} ({})",
                status, conn.display_name, timestamp, db_type
            );
            options.push(option);
        }

        // Use inquire for interactive selection
        let selected_option = inquire::Select::new("Select a recent connection:", options)
            .prompt()
            .map_err(|e| CliError::ConnectionError(format!("Selection cancelled: {e}")))?;

        // Find the connection URL for the selected option
        let selected_connection = recent_connections
            .iter()
            .take(20)
            .find(|conn| {
                let status = if conn.success { "✅" } else { "❌" };
                let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
                let db_type = conn.database_type.display_name();
                let option = format!(
                    "{} {} - {} ({})",
                    status, conn.display_name, timestamp, db_type
                );
                option == selected_option
            })
            .ok_or_else(|| CliError::ConnectionError("Invalid selection".to_string()))?;

        println!(
            "🔗 Connecting to recent connection: {}",
            selected_connection.display_name
        );

        // Reconstruct the connection URL with credentials (similar to session handling)
        let reconstructed_url =
            self.reconstruct_recent_connection_with_credentials(selected_connection)?;
        Ok(reconstructed_url)
    }

    /// Reconstruct a recent connection URL with credentials from credential stores
    fn reconstruct_recent_connection_with_credentials(
        &self,
        connection: &crate::config::RecentConnection,
    ) -> Result<String, CliError> {
        // Parse the original connection URL to extract components
        let original_url = &connection.connection_url;

        // Handle special cases first
        if original_url.starts_with("docker://") {
            // Docker connections are handled specially - just return as-is
            return Ok(original_url.clone());
        }

        if original_url.starts_with("vault://") {
            // Check if we have vault metadata stored (like saved sessions do)
            if let (Some(vault_mount), Some(vault_database), Some(vault_role)) = (
                connection.options.get("vault_mount"),
                connection.options.get("vault_database"),
                connection.options.get("vault_role"),
            ) {
                // This is a Vault connection with stored metadata, reconstruct vault:// URL for fresh credentials
                println!(
                    "🔐 Re-obtaining Vault credentials for recent connection: {vault_database}"
                );
                if vault_role.is_empty() {
                    return Ok(format!("vault://{vault_mount}/{vault_database}"));
                } else {
                    return Ok(format!(
                        "vault://{vault_role}@{vault_mount}/{vault_database}"
                    ));
                }
            } else {
                // Fallback: use original vault URL
                println!("🔐 Re-obtaining Vault credentials for recent connection");
                return Ok(original_url.clone());
            }
        }

        // Check if this was originally a Docker connection (based on display_name)
        if connection.display_name.contains("Docker:") {
            // Extract container name from display_name like "postgres@localhost:5432/postgres (Docker: ward-postgres-1)"
            if let Some(docker_part) = connection.display_name.split("Docker:").nth(1) {
                let container_name = docker_part.trim().trim_end_matches(')');
                println!(
                    "🐳 Re-resolving Docker container for recent connection: {container_name}"
                );
                return Ok(format!("docker://{container_name}"));
            }
        }

        if original_url.starts_with("sqlite://") {
            // SQLite doesn't need credentials
            return Ok(original_url.clone());
        }

        // Parse the URL to extract connection components
        let parsed_url = url::Url::parse(original_url).map_err(|e| {
            CliError::ConnectionError(format!(
                "Failed to parse recent connection URL '{original_url}': {e}"
            ))
        })?;

        let scheme = parsed_url.scheme();
        let host = parsed_url.host_str().unwrap_or("localhost");
        let port = parsed_url.port().unwrap_or(match scheme {
            "postgresql" => 5432,
            "mysql" => 3306,
            _ => return Ok(original_url.clone()), // Unknown scheme, return as-is
        });
        let username = parsed_url.username();
        let database = parsed_url.path().trim_start_matches('/');

        // Look up password from appropriate credential store using trait-based approach
        let reconstructed_url = if connection.database_type.is_file_based() {
            // SQLite was already handled above
            original_url.clone()
        } else {
            let url_scheme = connection.database_type.url_scheme();

            // Try to get password from appropriate credential store
            let password = match connection.database_type {
                crate::database::DatabaseType::PostgreSQL => {
                    crate::pgpass::lookup_password(host, port, database, username)
                }
                crate::database::DatabaseType::MySQL => {
                    crate::myconf::lookup_mysql_password(host, port, database, username)
                }
                _ => None,
            };

            if let Some(password) = password {
                format!("{url_scheme}://{username}:{password}@{host}:{port}/{database}")
            } else {
                // No password found, return URL without password (will prompt)
                format!("{url_scheme}://{username}@{host}:{port}/{database}")
            }
        };

        Ok(reconstructed_url)
    }

    /// Handle vault:// URLs
    async fn handle_vault_connection(
        &mut self,
        url: &str,
    ) -> Result<(Database, Option<ConnectionInfo>), CliError> {
        let (role, mount_path, database_name) = crate::vault_client::parse_vault_url(url)
            .ok_or_else(|| CliError::ConnectionError(format!("Invalid vault URL format: {url}")))?;

        println!("🔐 Connecting to Vault...");

        // Handle optional parameters - if None, prompt user to select
        let db_name = match database_name {
            Some(name) => name.clone(),
            None => {
                // List all available databases and filter to only show accessible ones
                let all_databases = crate::vault_client::list_vault_databases(&mount_path)
                    .await
                    .map_err(|e| {
                        CliError::ConnectionError(format!("Failed to list Vault databases: {e}"))
                    })?;

                let databases = crate::vault_client::filter_databases_with_available_roles(
                    &mount_path,
                    all_databases,
                )
                .await
                .map_err(|e| {
                    CliError::ConnectionError(format!("Failed to filter accessible databases: {e}"))
                })?;

                if databases.is_empty() {
                    return Err(CliError::ConnectionError(
                        "No accessible databases found in Vault mount".to_string(),
                    ));
                }

                inquire::Select::new("Select a database:", databases)
                    .prompt()
                    .map_err(|e| {
                        CliError::ConnectionError(format!("Database selection cancelled: {e}"))
                    })?
            }
        };

        let role_name = match role {
            Some(name) => name.clone(),
            None => {
                // List available roles for the selected database and prompt user to select
                let roles =
                    crate::vault_client::get_available_roles_for_user(&mount_path, &db_name)
                        .await
                        .map_err(|e| {
                            CliError::ConnectionError(format!("Failed to list Vault roles: {e}"))
                        })?;

                if roles.is_empty() {
                    return Err(CliError::ConnectionError(format!(
                        "No roles available for database '{db_name}'"
                    )));
                }

                inquire::Select::new(&format!("Select role for database '{db_name}':"), roles)
                    .prompt()
                    .map_err(|e| {
                        CliError::ConnectionError(format!("Role selection cancelled: {e}"))
                    })?
            }
        };

        // Get dynamic credentials from Vault (with caching)
        let (credentials, _lease_info) = crate::vault_client::get_dynamic_credentials_with_caching(
            &mount_path,
            &db_name,
            &role_name,
            &mut self.config,
        )
        .await
        .map_err(|e| CliError::ConnectionError(format!("Failed to get Vault credentials: {e}")))?;

        println!("✅ Successfully obtained dynamic credentials from Vault");
        println!("🔗 Connecting to PostgreSQL with temporary credentials...");

        // Get the database configuration from Vault to build the connection URL
        let db_config = crate::vault_client::get_vault_database_config(&mount_path, &db_name)
            .await
            .map_err(|e| {
                CliError::ConnectionError(format!("Failed to get database config from Vault: {e}"))
            })?;

        // Extract the connection URL template from the config
        let connection_url_template = db_config
            .connection_details
            .connection_url
            .as_ref()
            .ok_or_else(|| {
                CliError::ConnectionError(
                    "No connection URL found in Vault database config".to_string(),
                )
            })?;

        // Construct the PostgreSQL URL using the dynamic credentials
        let postgres_url = crate::vault_client::construct_postgres_url(
            connection_url_template,
            &credentials.username,
            &credentials.password,
        )
        .map_err(|e| {
            CliError::ConnectionError(format!("Failed to construct connection URL: {e}"))
        })?;

        // Create database connection using the dynamic credentials
        let mut database = Database::from_url(
            &postgres_url,
            Some(self.config.default_limit),
            Some(self.config.expanded_display_default),
        )
        .await
        .map_err(|e| {
            CliError::ConnectionError(format!("Failed to connect with Vault credentials: {e}"))
        })?;

        // Create connection info for the Vault connection
        // Parse the original connection URL template to get the real host/port (not tunneled)
        let original_connection_info =
            crate::database::ConnectionInfo::parse_url(connection_url_template).map_err(|e| {
                CliError::ConnectionError(format!(
                    "Failed to parse Vault connection URL template: {e}"
                ))
            })?;

        // Create connection info with original host/port and Vault metadata
        let mut options = std::collections::HashMap::new();
        options.insert("vault_mount".to_string(), mount_path.clone());
        options.insert("vault_database".to_string(), db_name.clone());
        options.insert("vault_role".to_string(), role_name.clone());

        let connection_info = crate::database::ConnectionInfo {
            database_type: crate::database::DatabaseType::PostgreSQL,
            host: original_connection_info.host.clone(), // Use original host, not tunnel host
            port: original_connection_info.port,         // Use original port, not tunnel port
            username: Some(credentials.username.clone()),
            password: Some(credentials.password),
            database: original_connection_info.database.clone(), // Use original database name
            file_path: None,
            options,
            docker_container: None,
        };

        // Set the connection info in the database so it's accessible via get_connection_info()
        database.set_connection_info_override(connection_info.clone());

        println!("✅ Successfully connected to PostgreSQL via Vault");
        println!("👤 Connected as temporary user: {}", credentials.username);

        Ok((database, Some(connection_info)))
    }

    /// Get command completions for autocomplete
    pub fn get_command_completions(prefix: &str) -> Vec<String> {
        use crate::commands::CommandParser;
        CommandParser::get_command_names()
            .into_iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|s| s.to_string())
            .collect()
    }

    /// Get help text for a specific command
    pub fn get_command_help(command_str: &str) -> Option<String> {
        use crate::commands::CommandParser;
        if let Ok(command) = CommandParser::parse(command_str) {
            let description = command.description();
            let usage = command.usage();
            Some(format!("{usage} - {description}"))
        } else {
            None
        }
    }

    /// Get categorized help text for all commands
    pub fn get_categorized_help() -> String {
        use crate::commands::CommandParser;
        let mut help = String::new();
        help.push_str("Available Commands:\n\n");

        for (category, commands) in CommandParser::get_commands_by_category() {
            help.push_str(&format!("{category:?}:\n"));
            for (cmd, desc) in commands {
                help.push_str(&format!("  {cmd:<12} - {desc}\n"));
            }
            help.push('\n');
        }

        help
    }

    /// Determine if output should be paged based on line count and configuration
    fn should_use_pager(output: &str, config: &DbCrustConfig) -> bool {
        // If pager is disabled, never page
        if !config.pager_enabled {
            return false;
        }

        // Count lines in output
        let line_count = output.lines().count();

        // Get the threshold
        let threshold = if config.pager_threshold_lines == 0 {
            // Use terminal height if available, otherwise default to 25
            if let Some(terminal_size) = terminal_size::terminal_size() {
                terminal_size.1.0 as usize
            } else {
                25 // Fallback default
            }
        } else {
            config.pager_threshold_lines
        };

        line_count > threshold
    }

    /// Route output to pager or direct print based on configuration and content size
    fn page_or_print(output: &str, config: &DbCrustConfig) -> Result<(), CliError> {
        if Self::should_use_pager(output, config) {
            // Try to use pager
            match pager::page_output(output, &config.pager_command) {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Pager failed, fall back to direct output
                    debug!("Pager failed, falling back to direct output: {}", e);
                    print!("{output}");
                    Ok(())
                }
            }
        } else {
            // Direct output
            print!("{output}");
            Ok(())
        }
    }
}
