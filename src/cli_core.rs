use crate::cli::Args;
use crate::commands::{Command, CommandExecutor, CommandParser, CommandResult};
use crate::completion::{NoopCompleter, SqlCompleter};
use crate::config::Config as DbCrustConfig;
use crate::database::{ConnectionInfo, DatabaseType, DatabaseTypeExt};
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
use tracing::debug;
use url;

/// Core CLI functionality shared between Rust and Python interfaces
pub struct CliCore {
    pub config: DbCrustConfig,
    pub database: Option<Database>,
    pub connection_info: Option<ConnectionInfo>,
    pub ai_conversation: crate::ai::conversation::AiConversation,
}

#[derive(Debug)]
pub enum CliError {
    ConnectionError(String),
    CommandError(String),
    ConfigError(String),
    ArgumentError(String),
}

impl std::error::Error for CliError {}

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

impl From<Box<dyn StdError>> for CliError {
    fn from(err: Box<dyn StdError>) -> Self {
        CliError::CommandError(err.to_string())
    }
}

impl Default for CliCore {
    fn default() -> Self {
        let config = DbCrustConfig::load();

        // Initialize global vector config for formatters
        crate::vector_display::set_global_vector_config(config.vector_display.clone());

        let ai_history_len = config.ai.history_length;
        Self {
            config,
            database: None,
            connection_info: None,
            ai_conversation: crate::ai::conversation::AiConversation::new(ai_history_len),
        }
    }
}

/// SQL keywords that should never be intercepted as named query invocations.
const SQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "CREATE",
    "ALTER",
    "DROP",
    "EXPLAIN",
    "WITH",
    "SHOW",
    "SET",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "GRANT",
    "REVOKE",
    "TRUNCATE",
    "COPY",
    "VACUUM",
    "ANALYZE",
    "DO",
    "CALL",
    "EXECUTE",
    "PREPARE",
    "DEALLOCATE",
    "DECLARE",
    "FETCH",
    "CLOSE",
    "LISTEN",
    "NOTIFY",
    "UNLISTEN",
    "LOAD",
    "REINDEX",
    "CLUSTER",
    "COMMENT",
    "LOCK",
    "RESET",
    "DISCARD",
    "REFRESH",
    "IMPORT",
    "EXPORT",
    "MERGE",
    "REPLACE",
    "UPSERT",
    "DESCRIBE",
    "USE",
    "KILL",
    "PRAGMA",
    "ATTACH",
    "DETACH",
    "VALUES",
    "TABLE",
];

/// Check if user input matches a named query invocation.
/// Returns `Some((name, args))` if the first word matches a stored named query,
/// or `None` if it looks like SQL or doesn't match any named query.
fn resolve_named_query(
    input: &str,
    config: &DbCrustConfig,
    db_type: Option<&DatabaseType>,
    session_id: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let first_word = input.split_whitespace().next()?;

    // Reject SQL keywords to avoid false positives
    if SQL_KEYWORDS
        .iter()
        .any(|kw| kw.eq_ignore_ascii_case(first_word))
    {
        return None;
    }

    // Check if first word matches a named query
    if config
        .get_available_named_query(first_word, db_type, session_id)
        .is_some()
    {
        let args: Vec<String> = input
            .split_whitespace()
            .skip(1)
            .map(|s| s.to_string())
            .collect();
        return Some((first_word.to_string(), args));
    }

    None
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

        // Database clients are constructed without Config access — publish the
        // configured query timeout for them (0 disables it)
        crate::database::set_query_timeout_seconds(cli_core.config.query_timeout_seconds);

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

        // Handle self-update if requested (runs before any connection logic)
        if args.update {
            return Ok(crate::update::run_update().await);
        }

        // Handle `dbcrust config ...` — no database connection needed
        if let Some(crate::cli::CliCommand::Config { action }) = &args.subcommand {
            cli_core.handle_config_subcommand(action)?;
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
                let exit_code = cli_core.handle_command_mode(&args).await?;
                return Ok(exit_code);
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

            // No URL and nothing to execute: print help instead of opening an
            // empty REPL — connection examples live in `after_help` (cli.rs).
            Args::command()
                .print_help()
                .map_err(|e| CliError::CommandError(format!("Failed to print help: {e}")))?;
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

        if let Ok((width, height)) = crossterm::terminal::size() {
            debug!("Terminal size: {width}x{height}");
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
                    self.handle_standalone_config_command(cmd)?;
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

    /// Handle a `\config ...` string without a database connection
    /// (`dbcrust -c '\config ...'`). Command mode never opens the interactive
    /// menu — a bare `\config` falls back to the read-only summary.
    fn handle_standalone_config_command(&mut self, command_str: &str) -> Result<(), CliError> {
        let command = CommandParser::parse(command_str)
            .map_err(|e| CliError::CommandError(format!("Command parsing failed: {e}")))?;
        let result = match command {
            Command::ConfigMenu | Command::ShowConfig => {
                Ok(crate::config_editor::render_summary(&self.config))
            }
            Command::ConfigGet { key } => {
                crate::config_editor::get_value(&self.config, key.as_deref())
            }
            Command::ConfigSet { key, value } => {
                crate::config_editor::set_value(&mut self.config, &key, &value)
            }
            Command::ConfigEdit => crate::config_editor::edit_in_editor(&mut self.config),
            _ => {
                return Err(CliError::CommandError(
                    "Database connection required for this command".to_string(),
                ));
            }
        };
        match result {
            Ok(message) => {
                println!("{message}");
                Ok(())
            }
            Err(e) => Err(CliError::CommandError(e)),
        }
    }

    /// Handle the `dbcrust config [show|get|set|edit]` CLI subcommand —
    /// no database connection involved. Bare `dbcrust config` opens the
    /// interactive menu when stdin/stdout are TTYs.
    fn handle_config_subcommand(
        &mut self,
        action: &Option<crate::cli::ConfigAction>,
    ) -> Result<(), CliError> {
        use crate::cli::ConfigAction;
        let result = match action {
            None => {
                if crate::config_editor::can_run_interactive() {
                    crate::config_editor::run_menu(&mut self.config)
                } else {
                    Ok(crate::config_editor::render_summary(&self.config))
                }
            }
            Some(ConfigAction::Show) => Ok(crate::config_editor::render_summary(&self.config)),
            Some(ConfigAction::Get { key }) => {
                crate::config_editor::get_value(&self.config, key.as_deref())
            }
            Some(ConfigAction::Set { key, value }) => {
                crate::config_editor::set_value(&mut self.config, key, value)
            }
            Some(ConfigAction::Edit) => crate::config_editor::edit_in_editor(&mut self.config),
        };
        match result {
            Ok(message) => {
                println!("{message}");
                Ok(())
            }
            Err(e) => Err(CliError::CommandError(e)),
        }
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
    /// Connect to database with password management (lookup from .dbcrust, prompt on failure, save option)
    async fn connect_with_password_management(
        &mut self,
        original_url: &str,
    ) -> Result<(crate::db::Database, Option<ConnectionInfo>), CliError> {
        use crate::database::ConnectionInfo;
        use crate::dbcrust_pass::{DatabaseType, lookup_password, save_password};

        debug!(
            "🔐 Starting connection with password management for URL: {}",
            original_url
        );

        // First attempt: Try connection as-is
        match crate::db::Database::from_url(
            original_url,
            Some(self.config.default_limit),
            Some(self.config.expanded_display_default),
        )
        .await
        {
            Ok(database) => {
                debug!("✅ Initial connection successful");
                return Ok((database, None));
            }
            Err(e) => {
                let error_msg = e.to_string().to_lowercase();

                // Check if this is an authentication error
                if !Self::is_authentication_error(&error_msg) {
                    // Not an auth error, return the original error
                    eprintln!("Failed to connect to database: {e}");
                    eprintln!(
                        "Connection URL: {}",
                        crate::password_sanitizer::sanitize_connection_url(original_url)
                    );
                    return Err(CliError::ConnectionError(e.to_string()));
                }

                debug!("🔐 Authentication error detected, trying password lookup and retry");
            }
        }

        // Parse the connection info to extract parameters for password lookup
        let connection_info = ConnectionInfo::parse_url(original_url).map_err(|e| {
            CliError::ConnectionError(format!("Failed to parse connection URL: {e}"))
        })?;

        // Extract connection parameters
        let db_type_enum = match connection_info.database_type {
            crate::database::DatabaseType::PostgreSQL => DatabaseType::PostgreSQL,
            crate::database::DatabaseType::MySQL => DatabaseType::MySQL,
            crate::database::DatabaseType::MongoDB => DatabaseType::MongoDB,
            crate::database::DatabaseType::Elasticsearch => DatabaseType::Elasticsearch,
            crate::database::DatabaseType::ClickHouse => DatabaseType::ClickHouse,
            crate::database::DatabaseType::SQLite
            | crate::database::DatabaseType::Parquet
            | crate::database::DatabaseType::CSV
            | crate::database::DatabaseType::JSON
            | crate::database::DatabaseType::DuckDB => {
                // File-based databases don't use passwords, return original error
                eprintln!("Failed to connect to file-based database");
                return Err(CliError::ConnectionError(
                    "File-based connection failed".to_string(),
                ));
            }
        };

        let host = connection_info.host.as_deref().unwrap_or("localhost");
        let port = connection_info.port.unwrap_or(match db_type_enum {
            DatabaseType::PostgreSQL => 5432,
            DatabaseType::MySQL => 3306,
            DatabaseType::MongoDB => 27017,
            DatabaseType::Elasticsearch => 9200,
            DatabaseType::ClickHouse => 8123,
            DatabaseType::SQLite => 0, // Not used
        });
        let database_name = connection_info.database.as_deref().unwrap_or("");
        let username = connection_info.username.as_deref().unwrap_or("");

        // If no password was in the original URL, try looking it up
        if connection_info.password.is_none() {
            debug!("🔍 Looking up password in .dbcrust file");
            match lookup_password(db_type_enum.clone(), host, port, database_name, username) {
                Ok(Some(password)) => {
                    debug!("✅ Found password in .dbcrust file");
                    // Create new URL with password
                    let url_with_password = Self::inject_password_into_url(original_url, &password)
                        .map_err(|e| {
                            CliError::ConnectionError(format!("Failed to inject password: {e}"))
                        })?;

                    // Try connection with looked-up password
                    match crate::db::Database::from_url(
                        &url_with_password,
                        Some(self.config.default_limit),
                        Some(self.config.expanded_display_default),
                    )
                    .await
                    {
                        Ok(database) => {
                            debug!("✅ Connection successful with looked-up password");
                            return Ok((database, None));
                        }
                        Err(e) => {
                            debug!("❌ Connection failed even with looked-up password: {e}");
                            // Continue to prompt for password
                        }
                    }
                }
                Ok(None) => {
                    debug!("🔍 No password found in .dbcrust file");
                }
                Err(e) => {
                    debug!("⚠️  Error looking up password: {e}");
                }
            }
        }

        // Prompt for password interactively using inquire
        let prompted_password = inquire::Password::new("🔐 Password:")
            .without_confirmation()
            .prompt()
            .map_err(|e| CliError::ConnectionError(format!("Password input error: {e}")))?;

        // Try connection with prompted password
        let url_with_password = Self::inject_password_into_url(original_url, &prompted_password)
            .map_err(|e| CliError::ConnectionError(format!("Failed to inject password: {e}")))?;

        match crate::db::Database::from_url(
            &url_with_password,
            Some(self.config.default_limit),
            Some(self.config.expanded_display_default),
        )
        .await
        {
            Ok(database) => {
                debug!("✅ Connection successful with prompted password");

                // Automatically save the password with encryption (no confirmation prompts)
                match save_password(
                    db_type_enum,
                    host,
                    port,
                    database_name,
                    username,
                    &prompted_password,
                    true,
                ) {
                    Ok(()) => {
                        println!("✅ Password saved to .dbcrust file (encrypted)");
                    }
                    Err(e) => {
                        debug!("⚠️  Failed to save password: {e}");
                        // Don't show error to user - saving is optional, connection succeeded
                    }
                }

                Ok((database, None))
            }
            Err(e) => {
                eprintln!("❌ Connection failed with provided password: {e}");
                eprintln!(
                    "Connection URL: {}",
                    crate::password_sanitizer::sanitize_connection_url(original_url)
                );
                Err(CliError::ConnectionError(format!(
                    "Authentication failed: {e}"
                )))
            }
        }
    }

    /// Check if an error message indicates an authentication failure
    fn is_authentication_error(error_msg: &str) -> bool {
        let auth_indicators = [
            "authentication failed",
            "auth failed",
            "invalid credentials",
            "password authentication failed",
            "login failed",
            "access denied",
            "permission denied",
            "unauthorized",
            "invalid username or password",
            "authentication error",
            "wrong password",
            "invalid password",
            "login incorrect",
            "connection refused", // Some databases return this for bad auth
        ];

        auth_indicators
            .iter()
            .any(|indicator| error_msg.contains(indicator))
    }

    /// Inject password into a connection URL
    fn inject_password_into_url(
        original_url: &str,
        password: &str,
    ) -> Result<String, url::ParseError> {
        let mut parsed_url = url::Url::parse(original_url)?;
        parsed_url
            .set_password(Some(password))
            .map_err(|_| url::ParseError::EmptyHost)?;
        Ok(parsed_url.to_string())
    }

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

        // Create database connection with password management
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
            // Try connection with password management and retry logic
            self.connect_with_password_management(&full_url_str).await?
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

    /// Handle -c command mode (execute commands and exit).
    /// Returns the process exit code: non-zero when any command failed, so
    /// scripts chaining `dbcrust ... -c "..." && next-step` can rely on it.
    async fn handle_command_mode(&mut self, args: &Args) -> Result<i32, CliError> {
        let mut failed = false;
        for command in &args.command {
            let command_trimmed = command.trim();

            if command_trimmed.starts_with('\\') {
                // Handle backslash commands
                match self.execute_backslash_command(command_trimmed).await? {
                    CommandModeOutcome::Success => {}
                    CommandModeOutcome::Failed => failed = true,
                    // \q is a clean stop, not an error
                    CommandModeOutcome::Exit => break,
                }
            } else if let Some((name, args)) =
                self.resolve_named_query_for_command_mode(command_trimmed)
            {
                // Execute named query
                if self.execute_named_query_command_mode(name, args).await?
                    == CommandModeOutcome::Failed
                {
                    failed = true;
                }
            } else {
                // Execute SQL — psql-style: a single -c argument may carry
                // several semicolon-separated statements
                let database = self
                    .database
                    .as_ref()
                    .ok_or_else(|| CliError::CommandError("No database connection".to_string()))?;
                let splittable = !matches!(
                    database
                        .get_connection_info()
                        .map(|info| info.database_type.clone()),
                    Some(
                        crate::database::DatabaseType::MongoDB
                            | crate::database::DatabaseType::Elasticsearch
                    )
                );
                let statements = if splittable {
                    crate::sql_buffer::split_statements(command_trimmed)
                } else {
                    vec![command_trimmed.to_string()]
                };

                'statements: for statement in &statements {
                    let database = self.database.as_mut().ok_or_else(|| {
                        CliError::CommandError("No database connection".to_string())
                    })?;
                    match database
                        .execute_query_with_info_no_column_selection(statement)
                        .await
                    {
                        Ok(results_with_info) => {
                            if !results_with_info.data.is_empty() {
                                let is_expanded = database.is_expanded_display();
                                if is_expanded {
                                    let tables =
                                        format_query_results_expanded(&results_with_info.data);
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
                                // User-initiated abort: stop without an error
                                return Ok(if failed { 1 } else { 0 });
                            }
                            eprintln!("Error executing query: {e}");
                            failed = true;
                            // Stop the batch at the first failing statement
                            break 'statements;
                        }
                    }
                }
            }
        }

        Ok(if failed { 1 } else { 0 })
    }

    /// Check if input matches a named query in command mode (non-interactive).
    fn resolve_named_query_for_command_mode(&self, input: &str) -> Option<(String, Vec<String>)> {
        let database = self.database.as_ref()?;
        let db_type = database
            .get_connection_info()
            .map(|info| info.database_type.clone());
        let session_id = SessionId::from_database(database).map(|sid| sid.identifier);
        resolve_named_query(input, &self.config, db_type.as_ref(), session_id.as_deref())
    }

    /// Execute a named query in command mode (non-interactive).
    async fn execute_named_query_command_mode(
        &mut self,
        name: String,
        args: Vec<String>,
    ) -> Result<CommandModeOutcome, CliError> {
        let command = Command::ExecuteNamedQuery { name, args };

        let database = self
            .database
            .take()
            .ok_or_else(|| CliError::CommandError("No database connection".to_string()))?;

        let db_arc = Arc::new(Mutex::new(database));
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));

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

        let outcome = match command
            .execute(
                &db_arc,
                &mut self.config,
                &mut last_script,
                &interrupt_flag,
                &mut prompt,
            )
            .await
        {
            Ok(CommandResult::Output(output)) => {
                Self::page_or_print(&output, &self.config)?;
                CommandModeOutcome::Success
            }
            Ok(CommandResult::Error(error)) => {
                eprintln!("Named query error: {error}");
                CommandModeOutcome::Failed
            }
            Err(e) => {
                eprintln!("Error executing named query: {e}");
                CommandModeOutcome::Failed
            }
            _ => CommandModeOutcome::Success,
        };

        // Restore database reference
        let updated_db = Arc::try_unwrap(db_arc)
            .map_err(|_| CliError::CommandError("Failed to unwrap database Arc".to_string()))?
            .into_inner()
            .map_err(|_| CliError::CommandError("Failed to unwrap database Mutex".to_string()))?;

        self.database = Some(updated_db);
        Ok(outcome)
    }

    /// Execute a backslash command using the new type-safe command system
    async fn execute_backslash_command(
        &mut self,
        command_str: &str,
    ) -> Result<CommandModeOutcome, CliError> {
        // Parse string command into typed Command enum
        let command = CommandParser::parse(command_str)
            .map_err(|e| CliError::CommandError(format!("Command parsing failed: {e}")))?;

        // Command mode (-c) must never open the interactive menu: a bare
        // \config falls back to the read-only summary.
        let command = if command == Command::ConfigMenu {
            Command::ShowConfig
        } else {
            command
        };

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
        let outcome = match command
            .execute(
                &db_arc,
                &mut self.config,
                &mut last_script,
                &interrupt_flag,
                &mut prompt,
            )
            .await
        {
            // \q in -c mode is a clean stop (it used to exit 1), and the
            // database must still be restored below before returning
            Ok(CommandResult::Exit) => CommandModeOutcome::Exit,
            Ok(CommandResult::Continue) => CommandModeOutcome::Success,
            Ok(CommandResult::Output(output)) => {
                println!("{output}");
                CommandModeOutcome::Success
            }
            Ok(CommandResult::Error(error)) => {
                eprintln!("Command error: {error}");
                CommandModeOutcome::Failed
            }
            Err(e) => {
                eprintln!("Error executing command: {e}");
                CommandModeOutcome::Failed
            }
        };

        // Update database reference
        let updated_db = Arc::try_unwrap(db_arc)
            .map_err(|_| CliError::CommandError("Failed to unwrap database Arc".to_string()))?
            .into_inner()
            .map_err(|_| CliError::CommandError("Failed to unwrap database Mutex".to_string()))?;

        self.database = Some(updated_db);
        Ok(outcome)
    }

    /// Run interactive mode - core interactive logic
    /// Install a process-wide SIGINT handler that flags query cancellation
    /// instead of killing the CLI. At the prompt, reedline owns the terminal
    /// in raw mode, so Ctrl-C arrives as a key event and never reaches this
    /// handler; while a query is executing (cooked mode) it lands here and
    /// the database clients cancel server-side.
    fn install_interrupt_handler() {
        static INSTALLED: AtomicBool = AtomicBool::new(false);
        if INSTALLED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let flag = crate::database::interrupt_flag().clone();
        tokio::spawn(async move {
            loop {
                if tokio::signal::ctrl_c().await.is_err() {
                    break;
                }
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                eprintln!("\nCancel request sent…");
            }
        });
    }

    // The std-Mutex config guard is intentionally held across command
    // execution awaits (Command::execute needs &mut Config); the REPL is
    // single-task so this cannot deadlock across tasks.
    #[allow(clippy::await_holding_lock)]
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
        // The process-wide flag: the Ctrl-C handler sets it, the database
        // clients poll it to cancel the running statement server-side
        let interrupt_flag = crate::database::interrupt_flag().clone();
        interrupt_flag.store(false, std::sync::atomic::Ordering::SeqCst);
        Self::install_interrupt_handler();

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
            // Enter inserts a newline instead of submitting while a string,
            // dollar-quote, or block comment is still open
            .with_validator(Box::new(crate::sql_buffer::SqlValidator))
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

                    // If empty input but we have a pending script (from \ed or
                    // \i), execute it ONCE. The buffer is cleared afterwards:
                    // leaving it armed meant every reflexive Enter on an empty
                    // prompt silently re-ran the script — disastrous for DML.
                    if line.is_empty() {
                        if !last_script.is_empty() {
                            println!(
                                "Executing last script ({} lines)...",
                                last_script.lines().count()
                            );
                            let script = std::mem::take(&mut last_script);
                            match self
                                .execute_sql_interactive(&script, &db_arc, &interrupt_flag)
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

                    // Handle AI text-to-SQL prefix (??)
                    if let Some(nl) = line.strip_prefix("??") {
                        let nl = nl.trim();
                        if nl.is_empty() {
                            eprintln!("Usage: ?? <describe what you want in natural language>");
                            continue;
                        }
                        match self
                            .handle_ai_text_to_sql(nl, &db_arc, &config_arc, &interrupt_flag)
                            .await
                        {
                            Ok(()) => {}
                            Err(e) => {
                                eprintln!("AI error: {e}");
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

                    // Try named query auto-detection before SQL execution
                    {
                        let (db_type, session_id) = {
                            let db = db_arc.lock().unwrap();
                            let db_type = db
                                .get_connection_info()
                                .map(|info| info.database_type.clone());
                            let session_id =
                                SessionId::from_database(&db).map(|sid| sid.identifier);
                            (db_type, session_id)
                        };
                        let resolved = {
                            let config = config_arc.lock().unwrap();
                            resolve_named_query(
                                line,
                                &config,
                                db_type.as_ref(),
                                session_id.as_deref(),
                            )
                        };
                        if let Some((name, args)) = resolved {
                            let command = Command::ExecuteNamedQuery { name, args };
                            #[allow(clippy::await_holding_lock)]
                            match command
                                .execute(
                                    &db_arc,
                                    &mut config_arc.lock().unwrap(),
                                    &mut last_script,
                                    &interrupt_flag,
                                    &mut prompt,
                                )
                                .await
                            {
                                Ok(CommandResult::Output(output)) => {
                                    println!("{output}");
                                }
                                Ok(CommandResult::Error(error)) => {
                                    eprintln!("Named query error: {error}");
                                }
                                Err(e) => {
                                    eprintln!("Error executing named query: {e}");
                                }
                                _ => {}
                            }
                            continue;
                        }
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
                _ => {
                    // Handle any other signals (e.g., ExternalBreak)
                    continue;
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
    #[allow(clippy::await_holding_lock)]
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
        // Note: config lock is held across await, but this is necessary to ensure
        // mutable config access is synchronized during command execution.
        // The guard must be dropped BEFORE the match arms run: a guard created
        // in the match scrutinee lives until the end of the whole match, and
        // the AI handlers below re-lock the config — instant deadlock on
        // \ai setup / \ai model otherwise.
        #[allow(clippy::await_holding_lock)]
        let result = {
            let mut config_guard = config_arc.lock().unwrap();
            command
                .execute(
                    db_arc,
                    &mut config_guard,
                    last_script,
                    interrupt_flag,
                    prompt,
                )
                .await
        };

        match result {
            Ok(CommandResult::Exit) => Ok(true),      // Signal exit
            Ok(CommandResult::Continue) => Ok(false), // Continue interactive loop
            Ok(CommandResult::Output(output)) => {
                // Handle AI interactive commands that need special handling
                if output == "__AI_SETUP__" {
                    self.handle_ai_setup(config_arc).await;
                } else if output == "__AI_CLEAR_HISTORY__" {
                    self.ai_conversation.clear();
                    println!("AI conversation history cleared.");
                } else if let Some(arg) = output.strip_prefix("__AI_PROVIDER__") {
                    self.handle_ai_select_provider(arg, config_arc).await;
                } else if let Some(arg) = output.strip_prefix("__AI_MODEL__") {
                    self.handle_ai_select_model(arg, config_arc, db_arc).await;
                } else {
                    println!("{output}");
                }
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

    // ==================== AI Assistant Methods ====================

    /// Handle ?? text-to-SQL generation
    #[allow(clippy::await_holding_lock)]
    async fn handle_ai_text_to_sql(
        &mut self,
        natural_language: &str,
        db_arc: &Arc<Mutex<Database>>,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<(), CliError> {
        let config = config_arc.lock().unwrap().clone();

        if !config.ai.enabled {
            return Err(CliError::CommandError(
                "AI assistant is disabled. Run \\ai on or \\ai setup to configure.".to_string(),
            ));
        }

        // Fresh cancellation state (a previous Ctrl-C must not abort us)
        interrupt_flag.store(false, std::sync::atomic::Ordering::SeqCst);

        // Build schema context
        let schema_context = {
            let mut db_guard = db_arc.lock().unwrap();
            crate::ai::schema_context::build_schema_context(
                &mut db_guard,
                natural_language,
                config.ai.max_schema_tables,
            )
            .await
        };

        // Build system prompt
        let db_type = {
            let db_guard = db_arc.lock().unwrap();
            db_guard.get_database_type()
        };
        let system_prompt =
            crate::ai::prompt_templates::build_system_prompt(&db_type, &schema_context);

        // Build messages from conversation history
        let messages = self.ai_conversation.to_messages(natural_language);

        // Generate SQL. Provider/model handling is delegated to genai.
        let sql = if config.ai.streaming {
            // Streaming mode
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            let ai_config = config.ai.clone();
            let system_prompt_clone = system_prompt.clone();
            let messages_clone = messages.clone();
            let interrupt_clone = interrupt_flag.clone();

            let generate_handle = tokio::spawn(async move {
                crate::ai::generate_stream(&ai_config, &system_prompt_clone, &messages_clone, tx)
                    .await
            });

            let response =
                match crate::ai::streaming::stream_to_terminal(rx, &interrupt_clone).await {
                    Ok(response) => response,
                    Err(crate::ai::AiError::Cancelled) => {
                        // Stop the provider request too — without the abort,
                        // "cancel" would still wait out the full generation
                        generate_handle.abort();
                        eprintln!("AI generation cancelled.");
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(CliError::CommandError(format!("Streaming error: {e}")));
                    }
                };

            // Wait for generation to complete
            if let Err(e) = generate_handle
                .await
                .map_err(|e| CliError::CommandError(format!("Generation task error: {e}")))?
            {
                return Err(CliError::CommandError(format!("AI generation error: {e}")));
            }

            crate::ai::streaming::extract_sql(&response)
        } else {
            // Non-streaming mode, raced against Ctrl-C (dropping the future
            // aborts the underlying request)
            let gen_fut = crate::ai::generate(&config.ai, &system_prompt, &messages);
            tokio::pin!(gen_fut);
            let response = loop {
                tokio::select! {
                    res = &mut gen_fut => {
                        break res.map_err(|e| {
                            CliError::CommandError(format!("AI generation error: {e}"))
                        })?;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        if interrupt_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            eprintln!("AI generation cancelled.");
                            return Ok(());
                        }
                    }
                }
            };

            let sql = crate::ai::streaming::extract_sql(&response.content);

            if config.ai.show_generated_sql {
                println!("\x1b[2m{sql}\x1b[0m");
            }

            sql
        };

        if sql.is_empty() {
            return Err(CliError::CommandError(
                "AI returned empty response".to_string(),
            ));
        }

        // Add to conversation history
        self.ai_conversation.add_exchange(natural_language, &sql);

        // The user must SEE the SQL before being asked to confirm it —
        // show_generated_sql only governs display for auto-execution.
        // (Streaming mode already printed the response while it arrived.)
        let sql_already_shown = config.ai.streaming || config.ai.show_generated_sql;
        let needs_confirmation = !matches!(
            config.ai.execution_mode,
            crate::ai::config::AiExecutionMode::AutoExecute
        );
        if !sql_already_shown && needs_confirmation {
            println!("\x1b[2m{sql}\x1b[0m");
        }

        // Determine whether to execute; writes never default to Yes
        let is_read_only = crate::ai::streaming::is_select_query(&sql);
        let should_execute = match config.ai.execution_mode {
            crate::ai::config::AiExecutionMode::Confirm => {
                inquire::Confirm::new("Execute this SQL?")
                    .with_default(is_read_only)
                    .prompt()
                    .unwrap_or(false)
            }
            crate::ai::config::AiExecutionMode::AutoSelect => {
                if is_read_only {
                    true
                } else {
                    inquire::Confirm::new("Execute this write query?")
                        .with_default(false)
                        .prompt()
                        .unwrap_or(false)
                }
            }
            crate::ai::config::AiExecutionMode::AutoExecute => true,
        };

        if should_execute {
            self.execute_sql_interactive(&sql, db_arc, interrupt_flag)
                .await?;
        }

        Ok(())
    }

    /// Prompt for an API key and persist it for `adapter`. Returns false if cancelled.
    fn configure_provider_key(adapter: genai::adapter::AdapterKind) -> bool {
        use crate::ai::key_storage::{self, KeyStorageMethod, store_api_key};

        if !key_storage::requires_api_key(adapter) {
            println!(
                "{} runs locally and does not require an API key.",
                adapter.as_str()
            );
            return true;
        }

        if let Some(env_name) = key_storage::env_var_name(adapter) {
            println!(
                "\nAPI key needed for {}. You can also set the {} environment variable.",
                adapter.as_str(),
                env_name
            );
        }

        let api_key = match inquire::Password::new("Enter API key:")
            .without_confirmation()
            .prompt()
        {
            Ok(key) if !key.is_empty() => key,
            _ => {
                println!("Skipped API key configuration.");
                return false;
            }
        };

        let storage_options = vec![
            "OS Keychain",
            "Encrypted file",
            "Environment variable (show command)",
        ];
        let method =
            match inquire::Select::new("How to store the API key?", storage_options).prompt() {
                Ok("OS Keychain") => KeyStorageMethod::OsKeyring,
                Ok("Encrypted file") => KeyStorageMethod::EncryptedFile,
                Ok(_) => KeyStorageMethod::EnvVarHint,
                Err(_) => return false,
            };

        match store_api_key(adapter, &api_key, &method) {
            Ok(()) => {
                if method != KeyStorageMethod::EnvVarHint {
                    println!("API key stored successfully via {method}.");
                }
                true
            }
            Err(e) => {
                eprintln!("Failed to store API key: {e}");
                false
            }
        }
    }

    /// Interactively pick a provider from the curated wizard list. Returns None if cancelled.
    fn select_provider(prompt: &str) -> Option<genai::adapter::AdapterKind> {
        let providers = crate::ai::suggested_providers();
        let names: Vec<String> = providers.iter().map(|p| p.as_str().to_string()).collect();
        let names_ref: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        match inquire::Select::new(prompt, names_ref.clone()).prompt() {
            Ok(choice) => Some(providers[names_ref.iter().position(|&n| n == choice).unwrap_or(0)]),
            Err(_) => None,
        }
    }

    /// Handle \ai setup - interactive setup wizard
    async fn handle_ai_setup(&mut self, config_arc: &Arc<Mutex<DbCrustConfig>>) {
        // Provider selection is a UX convenience — any genai model works.
        let Some(adapter) = Self::select_provider("Select AI provider:") else {
            println!("Setup cancelled.");
            return;
        };

        Self::configure_provider_key(adapter);

        // Model name (free text — the provider is inferred from it).
        let current_model = config_arc.lock().unwrap().ai.model.clone();
        let model = match inquire::Text::new("Model:")
            .with_default(&current_model)
            .with_help_message("e.g. claude-sonnet-4-6, gpt-4o, gemini-2.5-pro, ollama::llama3.1")
            .prompt()
        {
            Ok(m) if !m.trim().is_empty() => m.trim().to_string(),
            _ => current_model,
        };

        // Optional custom endpoint (self-hosted / OpenAI-compatible gateways).
        let endpoint = match inquire::Text::new("Custom endpoint URL (optional):")
            .with_default("")
            .with_help_message("Leave empty to use the provider's default endpoint")
            .prompt()
        {
            Ok(u) if !u.trim().is_empty() => Some(u.trim().to_string()),
            _ => None,
        };

        {
            let mut config = config_arc.lock().unwrap();
            config.ai.enabled = true;
            config.ai.model = model.clone();
            config.ai.endpoint = endpoint;
            config.save_with_documentation().ok();
        }

        println!(
            "\nAI assistant configured (model: {model}). Use ?? to generate SQL from natural language."
        );
    }

    /// Handle \ai provider [name] — configure the API key for a provider.
    /// The *active* provider is inferred from the model (`\ai model`); this just
    /// stores credentials so multiple providers can be configured.
    async fn handle_ai_select_provider(
        &mut self,
        arg: &str,
        _config_arc: &Arc<Mutex<DbCrustConfig>>,
    ) {
        let adapter = if arg.is_empty() {
            match Self::select_provider("Configure API key for provider:") {
                Some(a) => a,
                None => return,
            }
        } else {
            match genai::adapter::AdapterKind::from_lower_str(&arg.to_lowercase()) {
                Some(a) => a,
                None => {
                    eprintln!(
                        "Unknown provider: {arg}. The active provider is inferred from the model — use \\ai model <name>."
                    );
                    return;
                }
            }
        };

        Self::configure_provider_key(adapter);
    }

    /// Handle \ai model [name] — set the model (provider is inferred from it).
    async fn handle_ai_select_model(
        &mut self,
        arg: &str,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
        _db_arc: &Arc<Mutex<Database>>,
    ) {
        let model = if arg.is_empty() {
            let current = config_arc.lock().unwrap().ai.model.clone();
            match inquire::Text::new("Model:")
                .with_default(&current)
                .with_help_message(
                    "Any genai-supported model — e.g. claude-sonnet-4-6, gpt-4o, ollama::llama3.1",
                )
                .prompt()
            {
                Ok(m) if !m.trim().is_empty() => m.trim().to_string(),
                _ => return,
            }
        } else {
            arg.to_string()
        };

        let mut config = config_arc.lock().unwrap();
        config.ai.model = model.clone();
        config.save_with_documentation().ok();
        let adapter = crate::ai::provider_for_model(&model);
        println!("Model set to {model} (provider: {}).", adapter.as_str());
    }

    // ==================== End AI Assistant Methods ====================

    /// Execute SQL query in interactive mode
    #[allow(clippy::await_holding_lock)]
    /// Execute a (possibly multi-statement) SQL buffer, statement by
    /// statement. Multi-statement buffers (pasted scripts, `\i` files)
    /// previously went to the driver as one string and failed in the
    /// prepared-statement path.
    async fn execute_sql_interactive(
        &mut self,
        sql: &str,
        db_arc: &Arc<Mutex<Database>>,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<(), CliError> {
        // Mongo/ES "queries" aren't SQL — never split those on semicolons
        let splittable = {
            let db_guard = db_arc.lock().unwrap();
            !matches!(
                db_guard
                    .get_connection_info()
                    .map(|info| info.database_type.clone()),
                Some(
                    crate::database::DatabaseType::MongoDB
                        | crate::database::DatabaseType::Elasticsearch
                )
            )
        };

        let statements = if splittable {
            crate::sql_buffer::split_statements(sql)
        } else {
            vec![sql.to_string()]
        };

        let total = statements.len();
        for (idx, statement) in statements.iter().enumerate() {
            // Fresh cancellation state for each statement
            interrupt_flag.store(false, std::sync::atomic::Ordering::SeqCst);

            if let Err(e) = self
                .execute_single_statement_interactive(statement, db_arc, interrupt_flag)
                .await
            {
                // Stop the batch at the first failure and say where
                if total > 1 {
                    return Err(CliError::CommandError(format!(
                        "statement {} of {}: {}",
                        idx + 1,
                        total,
                        e
                    )));
                }
                return Err(e);
            }

            // A cancelled statement also cancels the rest of the batch
            if interrupt_flag.load(std::sync::atomic::Ordering::SeqCst) && idx + 1 < total {
                eprintln!("Skipping {} remaining statement(s)", total - idx - 1);
                break;
            }
        }

        Ok(())
    }

    // Lock intentionally held across the await: the REPL is single-task and
    // query execution needs exclusive Database access for its duration
    #[allow(clippy::await_holding_lock)]
    async fn execute_single_statement_interactive(
        &mut self,
        sql: &str,
        db_arc: &Arc<Mutex<Database>>,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<(), CliError> {
        // Lock held across await for query execution with column selection
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
                let session_url = session
                    .reconstruct_connection_url()
                    .map_err(CliError::ConnectionError)?;

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
        connection
            .reconstruct_connection_url()
            .map_err(CliError::ConnectionError)
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
            use_tls: original_connection_info.use_tls,
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
            if let Ok((_, height)) = crossterm::terminal::size() {
                height as usize
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NamedQueryScope;

    fn make_test_config_with_query(
        name: &str,
        query: &str,
        scope: NamedQueryScope,
    ) -> DbCrustConfig {
        let mut config = DbCrustConfig::default();
        config
            .add_named_query_with_scope(name, query, scope)
            .unwrap();
        config
    }

    #[test]
    fn test_resolve_named_query_match_with_args() {
        let config = make_test_config_with_query(
            "check_migration",
            "SELECT * FROM migrations WHERE app = '$1'",
            NamedQueryScope::Global,
        );
        let result = resolve_named_query("check_migration auth", &config, None, None);
        assert_eq!(
            result,
            Some(("check_migration".to_string(), vec!["auth".to_string()]))
        );
    }

    #[test]
    fn test_resolve_named_query_match_multiple_args() {
        let config = make_test_config_with_query(
            "is_migration_applied",
            "SELECT applied FROM django_migrations WHERE app = '$1' AND name = '$2'",
            NamedQueryScope::Global,
        );
        let result = resolve_named_query(
            "is_migration_applied frontend_tools 0002_delete",
            &config,
            None,
            None,
        );
        assert_eq!(
            result,
            Some((
                "is_migration_applied".to_string(),
                vec!["frontend_tools".to_string(), "0002_delete".to_string()]
            ))
        );
    }

    #[test]
    fn test_resolve_named_query_match_no_args() {
        let config = make_test_config_with_query(
            "show_users",
            "SELECT * FROM users",
            NamedQueryScope::Global,
        );
        let result = resolve_named_query("show_users", &config, None, None);
        assert_eq!(result, Some(("show_users".to_string(), vec![])));
    }

    #[test]
    fn test_resolve_named_query_no_match() {
        let config = DbCrustConfig::default();
        let result = resolve_named_query("nonexistent_query arg1", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_empty_input() {
        let config = DbCrustConfig::default();
        let result = resolve_named_query("", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_whitespace_only() {
        let config = DbCrustConfig::default();
        let result = resolve_named_query("   ", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_sql_keyword_select_rejected() {
        let config = make_test_config_with_query("select", "SELECT 1", NamedQueryScope::Global);
        let result = resolve_named_query("SELECT * FROM users", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_sql_keyword_case_insensitive() {
        let config = make_test_config_with_query("select", "SELECT 1", NamedQueryScope::Global);
        let result = resolve_named_query("select * FROM users", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_sql_keywords_all_rejected() {
        let config =
            make_test_config_with_query("placeholder", "SELECT 1", NamedQueryScope::Global);
        for keyword in SQL_KEYWORDS {
            let input = format!("{keyword} something");
            let result = resolve_named_query(&input, &config, None, None);
            assert_eq!(result, None, "SQL keyword '{keyword}' should be rejected");
        }
    }

    #[test]
    fn test_resolve_named_query_scoped_global() {
        let config = make_test_config_with_query("my_query", "SELECT 1", NamedQueryScope::Global);
        // Global queries should match regardless of db_type context
        let result =
            resolve_named_query("my_query", &config, Some(&DatabaseType::PostgreSQL), None);
        assert_eq!(result, Some(("my_query".to_string(), vec![])));
    }

    #[test]
    fn test_resolve_named_query_scoped_database_type() {
        let config = make_test_config_with_query(
            "pg_query",
            "SELECT * FROM pg_tables",
            NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL),
        );

        // Should match when db_type is PostgreSQL
        let result =
            resolve_named_query("pg_query", &config, Some(&DatabaseType::PostgreSQL), None);
        assert_eq!(result, Some(("pg_query".to_string(), vec![])));

        // Should NOT match when db_type is MySQL
        let result = resolve_named_query("pg_query", &config, Some(&DatabaseType::MySQL), None);
        assert_eq!(result, None);

        // Should NOT match when no db_type context
        let result = resolve_named_query("pg_query", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_scoped_session() {
        let config = make_test_config_with_query(
            "session_query",
            "SELECT 1",
            NamedQueryScope::Session("my_session_123".to_string()),
        );

        // Should match with correct session
        let result = resolve_named_query("session_query", &config, None, Some("my_session_123"));
        assert_eq!(result, Some(("session_query".to_string(), vec![])));

        // Should NOT match with wrong session
        let result = resolve_named_query("session_query", &config, None, Some("other_session"));
        assert_eq!(result, None);
    }

    #[test]
    fn test_resolve_named_query_scope_priority() {
        let mut config = DbCrustConfig::default();
        config
            .add_named_query_with_scope("my_query", "SELECT 'global'", NamedQueryScope::Global)
            .unwrap();
        config
            .add_named_query_with_scope(
                "my_query",
                "SELECT 'postgres'",
                NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL),
            )
            .unwrap();

        // With PostgreSQL context, should resolve (scope priority handled by find_by_name)
        let result = resolve_named_query(
            "my_query arg1",
            &config,
            Some(&DatabaseType::PostgreSQL),
            None,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "my_query");
    }

    #[test]
    fn test_resolve_named_query_preserves_arg_order() {
        let config =
            make_test_config_with_query("multi_arg", "SELECT $1, $2, $3", NamedQueryScope::Global);
        let result = resolve_named_query("multi_arg first second third", &config, None, None);
        assert_eq!(
            result,
            Some((
                "multi_arg".to_string(),
                vec![
                    "first".to_string(),
                    "second".to_string(),
                    "third".to_string()
                ]
            ))
        );
    }

    #[test]
    fn test_resolve_named_query_extra_whitespace() {
        let config = make_test_config_with_query("my_query", "SELECT $1", NamedQueryScope::Global);
        let result = resolve_named_query("my_query   arg1   arg2", &config, None, None);
        assert_eq!(
            result,
            Some((
                "my_query".to_string(),
                vec!["arg1".to_string(), "arg2".to_string()]
            ))
        );
    }

    #[test]
    fn test_resolve_named_query_does_not_match_partial() {
        let config = make_test_config_with_query("check", "SELECT 1", NamedQueryScope::Global);
        // "check_migration" should NOT match "check"
        let result = resolve_named_query("check_migration arg1", &config, None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_sql_keywords_constant_is_uppercase() {
        for keyword in SQL_KEYWORDS {
            assert_eq!(
                *keyword,
                keyword.to_uppercase(),
                "SQL keyword '{keyword}' should be uppercase in the constant"
            );
        }
    }
}

/// Outcome of a single command in non-interactive (-c) mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandModeOutcome {
    /// Command succeeded — keep processing remaining commands.
    Success,
    /// Command failed — keep processing, but exit non-zero at the end.
    Failed,
    /// \q or equivalent — stop processing and exit cleanly.
    Exit,
}
