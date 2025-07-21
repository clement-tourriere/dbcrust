// Import the debug_log macro here
#[macro_use]
extern crate dbcrust;
mod cli;
// completion is now in lib.rs
mod highlighter;
mod named_queries;
mod pager;
mod password_sanitizer;
mod pgpass;
mod script;

use dbcrust::backslash_commands::BackslashCommandRegistry;
use clap::{CommandFactory, Parser};
use clap_complete;
use cli::Args;
use inquire;
use dbcrust::completion::{NoopCompleter, SqlCompleter};
use dbcrust::config::{
    self as DbCrustConfigModule,
    Config as DbCrustConfig,
};
use dbcrust::db::Database;
use dbcrust::format::{
    format_query_results_expanded, format_query_results_psql,
};
use dirs;
use highlighter::SqlHighlighter;
use nu_ansi_term::{Color, Style};
use dbcrust::logging;
use dbcrust::prompt::DbPrompt;
use reedline::{
    default_emacs_keybindings, ColumnarMenu, Completer, DefaultHinter, Emacs, EditCommand, FileBackedHistory, KeyCode,
    KeyModifiers, MenuBuilder, ReedlineEvent, ReedlineMenu,
};
use reedline::{Reedline, Signal};
use signal_hook::{consts::SIGINT, flag};
use std::error::Error as StdError;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use terminal_size;
// For `std::io::stdout().flush()`


#[allow(dead_code)]
fn print_help(config: &DbCrustConfig) {
    print_banner(config);
    println!();
    println!("Available commands:");
    println!("  Multi-line input:");
    println!("  Alt+Enter    - Insert newline (continue multi-line query) [Primary]");
    println!("  Shift+Enter  - Insert newline (macOS: may not work in all terminals)");
    println!("  Ctrl+Enter   - Insert newline (macOS: may not work in all terminals)");
    println!("  Enter        - Execute query/command");
    println!();
    println!("  \\q          - Quit the client");
    println!("  \\l          - List all databases (enhanced for MySQL/SQLite)");
    println!("  \\dt         - List tables in current database");
    println!("  \\d          - List all tables in current database");
    println!("  \\d <table>  - Show detailed information about a table");
    println!("  \\c <dbname> - Connect to a different database");
    println!("  \\h          - Show this help message");
    println!("  \\x          - Toggle expanded display (vertical format)");
    println!("  \\setx       - Set current expanded display mode as default");
    println!("  \\e          - Toggle EXPLAIN mode (auto-explain queries)");
    println!("  \\sete       - Set current EXPLAIN mode as default");
    println!("  Advanced EXPLAIN Commands:");
    println!("  \\er <query> - Execute query with raw EXPLAIN output (no formatting)");
    println!("  \\ef <query> - Execute query with formatted EXPLAIN output only");
    println!("  \\ex <query> <file> - Execute EXPLAIN and export to file");
    println!("  \\a          - Toggle autocomplete on/off");
    println!("  \\seta       - Set current autocomplete setting as default");
    println!("  \\pager      - Toggle pager for long output");
    println!("  \\setpager   - Set current pager setting as default");
    println!("  \\banner     - Toggle banner display on/off");
    println!("  \\setbanner  - Set current banner display setting as default");
    println!("  \\copy       - Copy last EXPLAIN JSON plan to clipboard");
    println!("  \\config     - Show current configuration");
    println!("  \\save       - Save current connection as default configuration");
    println!("  \\setmulti <indicator> - Set multiline prompt indicator (empty for none)");
    println!("  \\pgpass     - Show PostgreSQL password file information (.pgpass)");
    println!("  \\myconf     - Show/manage MySQL configuration file (.my.cnf)");
    println!("  \\pragma     - Show/manage SQLite pragma settings");
    println!("");
    println!("  Database-specific commands:");
    println!("  \\du         - List users (MySQL/PostgreSQL)");
    println!("  \\di         - List indexes (SQLite)");
    println!("  \\dp         - List pragmas (SQLite)");
    println!("  \\docker     - List available Docker database containers");
    println!("  Script Handling:");
    println!("  \\ed         - Enter multiline edit mode");
    println!("  \\w <file>   - Save last query or edited script to file");
    println!("  \\i <file>   - Execute SQL commands from file");
    println!("  Column Selection:");
    println!("  \\cs         - Toggle column selection mode");
    println!("  \\setcs      - Set current column selection mode as default");
    println!("  \\clrcs      - Clear all saved column views");
    println!("  \\resetview  - Reset column view for the most recent query");
    println!("  \\csthreshold <n> - Set column selection auto-enable threshold");
    println!("  Named Queries:");
    println!("  \\n          - List all named queries");
    println!("  \\n <n>   - Execute a named query");
    println!("  \\n <n> <args>... - Execute a named query with arguments");
    println!("  \\ns <n> <query> - Save a named query");
    println!("  \\nd <n>  - Delete a named query");
    println!("  Sessions (Multi-Database Support):");
    println!("  \\s          - List all saved sessions (PostgreSQL, MySQL, SQLite)");
    println!("  \\s <n>   - Connect to a saved session");
    println!("  \\ss <n>  - Save current connection as a named session");
    println!("  \\sd <n>  - Delete a saved session");
    println!("  SSH Tunnels:");
    println!("  --ssh-tunnel option can be used to establish an SSH tunnel");
    println!("  Format: [user[:password]@]ssh_host[:ssh_port]");
    println!("  Example: --ssh-tunnel john:pass@jumphost.example.com:2222");
    println!("  Note: SSH tunnels are currently only supported for PostgreSQL connections");
    println!("  <SQL>       - Execute SQL query");
    println!("");
    println!("Connection:");
    println!("  dbcrust [CONNECTION_URL] - Connect using a database connection URL");
    println!("  Supported databases: PostgreSQL, MySQL, SQLite");
    println!("");
    println!(
        "  PostgreSQL Format: postgresql://[user[:password]@][host][:port][/dbname][?param1=value1&...]"
    );
    println!("  Example: dbcrust postgresql://user:pass@localhost/mydb");
    println!("  Example: dbcrust postgresql://user:pass@db.com/mydb?sslmode=require");
    println!("  Example: dbcrust user:pass@localhost/mydb");
    println!("");
    println!(
        "  MySQL Format: mysql://[user[:password]@][host][:port][/dbname][?param1=value1&...]"
    );
    println!("  Example: dbcrust mysql://user:pass@localhost:3306/mydb");
    println!("  Example: dbcrust mysql://user:pass@db.com/mydb?charset=utf8mb4");
    println!("");
    println!("  SQLite Format: sqlite:///path/to/database.db");
    println!("  Example: dbcrust sqlite:///home/user/mydb.db");
    println!("  Example: dbcrust sqlite://./relative/path/mydb.db");
    println!("");
    println!("  Supported sslmode values:");
    println!("    disable    - only try a non-SSL connection");
    println!(
        "    allow      - first try a non-SSL connection; if that fails, try an SSL connection"
    );
    println!(
        "    prefer     - first try an SSL connection; if that fails, try a non-SSL connection (default)"
    );
    println!("    require    - only try an SSL connection");
    println!(
        "    verify-ca  - only try an SSL connection, and verify that the server certificate is issued by a trusted CA"
    );
    println!(
        "    verify-full - only try an SSL connection, verify the server certificate is issued by a trusted CA and matches the hostname"
    );
    println!("");
    println!("Non-interactive mode:");
    println!("  -c, --command <COMMAND> - Execute the given command string and exit");
    println!("  This option can be repeated to execute multiple commands");
    println!("  Example: dbcrust -c \"SELECT * FROM users;\" -c \"SELECT COUNT(*) FROM orders;\"");
    println!("");
    println!("Vault Connection (PostgreSQL only):");
    println!("  dbcrust vault://<role_name>@<mount_path>/<vault_db_name>");
    println!("  All components are optional:");
    println!("  - If role_name is not specified, you will be prompted to select one");
    println!("  - If mount_path is not specified, default is 'database'");
    println!("  - If vault_db_name is not specified, you will be prompted to select one");
    println!("  Example: dbcrust vault://my-role@database/postgres-prod");
    println!("  Example: dbcrust vault:///postgres-prod (uses default mount path)");
    println!("  Example: dbcrust vault://my-role@ (prompts for database name)");
    println!("");
    println!("Shell Completions:");
    println!("  --generate-completion <SHELL> - Generate shell completions");
    println!("  Supported shells: bash, zsh, fish, powershell, elvish");
    println!("  Example: dbcrust --generate-completion bash > dbcrust.bash");
    println!("");
    println!("Notes:");
    println!(
        "  SELECT queries have a default limit of {} rows (configurable in config.toml)",
        DbCrustConfig::default().default_limit
    );
    println!("  Use \\setx to set your expanded display preference as default");
    println!("  Use \\sete to set your EXPLAIN mode preference as default");
    println!(
        "  Named queries support positional parameters ($1, $2), raw aggregation ($*) and string aggregation ($@)"
    );
    println!("  Passwords are stored in database-specific credential files, not in saved sessions:");
    println!("    - PostgreSQL: ~/.pgpass file");
    println!("    - MySQL: ~/.my.cnf file"); 
    println!("    - SQLite: no password needed (file-based)");
    println!("  SSH tunnels can be configured in config.toml with regex patterns");
    println!("  Debug logging can be enabled in config.toml with debug_logging_enabled = true");
    println!("  Debug logs are written to ~/.config/dbcrust/debug.log");
}

fn print_banner(config: &DbCrustConfig) {
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

#[allow(dead_code)]
fn handle_output(content: &str, config: &DbCrustConfig) {
    if content.is_empty() {
        return;
    }

    let lines = content.lines().count();
    let term_height_result = terminal_size::terminal_size();
    let term_height = term_height_result.map(|(_w, h)| h.0 as usize).unwrap_or(25);

    let use_pager = if config.pager_enabled {
        if config.pager_threshold_lines == 0 {
            lines > term_height
        } else {
            lines > config.pager_threshold_lines
        }
    } else {
        false
    };

    if use_pager {
        if let Err(_e) = pager::page_output(content, &config.pager_command) {
            // Errors (including fallback to print!) are handled within page_output
        }
    } else {
        print!("{}", content);
    }
}

#[allow(dead_code)]
fn handle_explain_output(content: &str, config: &DbCrustConfig) {
    if content.is_empty() {
        return;
    }

    let lines = content.lines().count();
    
    // For EXPLAIN output, only enable paging if there are more than 50 lines
    let use_pager = config.pager_enabled && lines > 50;

    if use_pager {
        if let Err(_e) = pager::page_output(content, &config.pager_command) {
            // Errors (including fallback to print!) are handled within page_output
        }
    } else {
        print!("{}", content);
    }
}

fn log_system_info(args: &Args) {
    debug_log!("System Info:");
    debug_log!("  OS: {}", std::env::consts::OS);
    debug_log!("  Arch: {}", std::env::consts::ARCH);
    debug_log!("  Command line args: {:?}", args);
    if let Some(terminal_size) = terminal_size::terminal_size() {
        debug_log!(
            "  Terminal size: {}x{}",
            terminal_size.0.0,
            terminal_size.1.0
        );
    }
}

// Parse a vault:// URL and extract vault parameters
// Format: vault://<role_name>@<mount_path:database>/<vault_db_name>
// All components are optional:
// - If role_name is not specified, user will be prompted to select one
// - If mount_path is not specified, defaults to "database"
// - If vault_db_name is not specified, user will be prompted to select one
fn parse_vault_url(url_str: &str) -> Option<(Option<String>, String, Option<String>)> {
    if !url_str.starts_with("vault://") {
        return None;
    }

    // Remove the protocol prefix
    let url_without_prefix = &url_str["vault://".len()..];

    // Extract role_name and mount_path from the user/host part
    let (user_host_part, db_part) = match url_without_prefix.find('/') {
        Some(idx) => (
            &url_without_prefix[..idx],
            Some(&url_without_prefix[idx + 1..]),
        ),
        None => (url_without_prefix, None),
    };

    // Parse the role_name@mount_path part
    let (role_name, mount_path) = match user_host_part.find('@') {
        Some(idx) => {
            let role = user_host_part[..idx].to_string();
            let role_opt = if role.is_empty() { None } else { Some(role) };
            let mount = user_host_part[idx + 1..].to_string();
            (
                role_opt,
                if mount.is_empty() {
                    "database".to_string()
                } else {
                    mount
                },
            )
        }
        None => {
            // No @ symbol means no role_name specified, use entire string as mount_path
            let mount = user_host_part.to_string();
            (
                None,
                if mount.is_empty() {
                    "database".to_string()
                } else {
                    mount
                },
            )
        }
    };

    // Extract vault_db_name from the path part
    let vault_db_name = db_part.map(|s| s.to_string()).filter(|s| !s.is_empty());

    Some((role_name, mount_path, vault_db_name))
}

/// Main async workflow that can be called from both main() and Python
pub async fn async_main() -> Result<(), Box<dyn StdError>> {
    let args = Args::parse();
    async_main_with_args(args).await
}

/// Main async workflow with pre-parsed arguments (for Python integration)
pub async fn async_main_with_args(args: Args) -> Result<(), Box<dyn StdError>> {
    // Initialize the logging system
    if let Err(e) = logging::init() {
        eprintln!("Warning: Failed to initialize logging: {}", e);
    }
    debug_log!("DbCrust started");

    let mut config = DbCrustConfig::load(); // Load config first for defaults and other settings

    // Handle shell completion generation if requested
    if let Some(shell) = args.completions {
        let mut cmd = Args::command();

        // Write to stdout
        match shell {
            cli::Shell::Bash => {
                clap_complete::generate(
                    clap_complete::shells::Bash,
                    &mut cmd,
                    "dbcrust",
                    &mut io::stdout(),
                );
            }
            cli::Shell::Zsh => {
                clap_complete::generate(
                    clap_complete::shells::Zsh,
                    &mut cmd,
                    "dbcrust",
                    &mut io::stdout(),
                );
            }
            cli::Shell::Fish => {
                clap_complete::generate(
                    clap_complete::shells::Fish,
                    &mut cmd,
                    "dbcrust",
                    &mut io::stdout(),
                );
            }
            cli::Shell::PowerShell => {
                clap_complete::generate(
                    clap_complete::shells::PowerShell,
                    &mut cmd,
                    "dbcrust",
                    &mut io::stdout(),
                );
            }
            cli::Shell::Elvish => {
                clap_complete::generate(
                    clap_complete::shells::Elvish,
                    &mut cmd,
                    "dbcrust",
                    &mut io::stdout(),
                );
            }
        }
        return Ok(());
    }


    // Log system information
    log_system_info(&args);

    // Set SSH tunnel debug mode if --debug flag is provided
    dbcrust::ssh_tunnel::set_debug_mode(args.debug);

    // Also enable debug logging if --debug flag is provided, overriding config
    if args.debug {
        // Create a temporary config just for this run with debug enabled
        let mut temp_config = config.clone();
        temp_config.debug_logging_enabled = true;

        // Save the config with debug enabled only if debug logging is not already enabled in the config
        if !config.debug_logging_enabled {
            debug_log!("Debug logging enabled via command line flag");
        }

        // Use the modified config for this session
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

    // Show banner if not disabled
    if !args.no_banner && args.command.is_empty() && config.show_banner_default {
        print_banner(&config);
    }

    // Handle session URLs
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
                    dbcrust::database::DatabaseType::PostgreSQL => "PostgreSQL",
                    dbcrust::database::DatabaseType::MySQL => "MySQL",
                    dbcrust::database::DatabaseType::SQLite => "SQLite",
                };
                let option = if session.database_type == dbcrust::database::DatabaseType::SQLite {
                    if let Some(ref file_path) = session.file_path {
                        format!("{} - {} ({})", name, file_path, db_type)
                    } else {
                        format!("{} - SQLite (no path)", name)
                    }
                } else {
                    format!("{} - {}@{}:{}/{} ({})", 
                        name, session.user, session.host, session.port, session.dbname, db_type)
                };
                options.push(option);
            }
            
            // Use inquire for interactive selection
            let selected_option = inquire::Select::new("Select a saved session:", options)
                .prompt()
                .map_err(|e| format!("Selection cancelled: {}", e))?;
            
            // Find the session name from the selected option
            sessions.iter()
                .find(|(name, session)| {
                    let db_type = match session.database_type {
                        dbcrust::database::DatabaseType::PostgreSQL => "PostgreSQL",
                        dbcrust::database::DatabaseType::MySQL => "MySQL",
                        dbcrust::database::DatabaseType::SQLite => "SQLite",
                    };
                    let option = if session.database_type == dbcrust::database::DatabaseType::SQLite {
                        if let Some(ref file_path) = session.file_path {
                            format!("{} - {} ({})", name, file_path, db_type)
                        } else {
                            format!("{} - SQLite (no path)", name)
                        }
                    } else {
                        format!("{} - {}@{}:{}/{} ({})", 
                            name, session.user, session.host, session.port, session.dbname, db_type)
                    };
                    option == selected_option
                })
                .map(|(name, _)| name.clone())
                .ok_or("Invalid selection")?
        } else {
            session_name.to_string()
        };
        
        println!("🔗 Connecting to saved session '{}'...", final_session_name);
        
        // Get the saved session from config
        match config.get_session(&final_session_name) {
            Some(session) => {
                // Reconstruct connection URL from saved session
                let session_url = match session.database_type {
                    dbcrust::database::DatabaseType::SQLite => {
                        if let Some(ref file_path) = session.file_path {
                            format!("sqlite://{}", file_path)
                        } else {
                            return Err("SQLite session missing file path".into());
                        }
                    }
                    dbcrust::database::DatabaseType::MySQL => {
                        // Try to get password from .my.cnf file
                        if let Some(password) = dbcrust::myconf::lookup_mysql_password(
                            &session.host, session.port, &session.dbname, &session.user
                        ) {
                            format!("mysql://{}:{}@{}:{}/{}", 
                                session.user, password, session.host, session.port, session.dbname)
                        } else {
                            // No password found in .my.cnf, construct URL without password
                            // The database connection will prompt for password
                            format!("mysql://{}@{}:{}/{}", 
                                session.user, session.host, session.port, session.dbname)
                        }
                    }
                    dbcrust::database::DatabaseType::PostgreSQL => {
                        // Check if this is a Docker session
                        if session.host.starts_with("DOCKER:") {
                            // Extract container name and create docker:// URL
                            let container_name = session.host.strip_prefix("DOCKER:").unwrap_or(&session.host);
                            println!("🐳 Re-resolving Docker container for saved session: {}", container_name);
                            format!("docker://{}", container_name)
                        } else {
                            // Regular PostgreSQL connection - try to get password from .pgpass file
                            if let Some(password) = dbcrust::pgpass::lookup_password(
                                &session.host, session.port, &session.dbname, &session.user
                            ) {
                                format!("postgresql://{}:{}@{}:{}/{}", 
                                    session.user, password, session.host, session.port, session.dbname)
                            } else {
                                // No password found in .pgpass, construct URL without password
                                // The database connection will prompt for password
                                format!("postgresql://{}@{}:{}/{}", 
                                    session.user, session.host, session.port, session.dbname)
                            }
                        }
                    }
                };
                
                // Create database from session
                let database = match Database::from_url(
                    &session_url,
                    Some(config.default_limit.clone()),
                    Some(config.expanded_display_default.clone()),
                ).await {
                    Ok(db) => db,
                    Err(e) => {
                        eprintln!("Failed to connect to session '{}': {}", final_session_name, e);
                        return Err(e);
                    }
                };
                
                println!("✓ Successfully connected to session '{}'", final_session_name);
                
                // Track this connection in history
                let sanitized_url = password_sanitizer::sanitize_connection_url(&session_url);
                if let Err(e) = config.add_recent_connection_auto_display(
                    sanitized_url,
                    session.database_type.clone(),
                    true
                ) {
                    debug_log!("Failed to add connection to history: {}", e);
                }
                
                // Handle commands and start interactive mode
                return handle_database_connection(database, config, args).await;
            }
            None => {
                eprintln!("Session '{}' not found. Use \\s to list available sessions.", final_session_name);
                return Err("Session not found".into());
            }
        }
    }

    // Handle recent:// URLs for interactive recent connection selection
    if full_url_str.starts_with("recent://") {
        let recent_connections = config.get_recent_connections();
        
        if recent_connections.is_empty() {
            eprintln!("No recent connections found. Connect to a database first to build connection history.");
            return Err("No recent connections available".into());
        }
        
        // Create options for inquire selection
        let mut options = Vec::new();
        for conn in recent_connections.iter().take(20) {
            let status = if conn.success { "✅" } else { "❌" };
            let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
            let db_type = match conn.database_type {
                dbcrust::database::DatabaseType::PostgreSQL => "PostgreSQL",
                dbcrust::database::DatabaseType::MySQL => "MySQL",
                dbcrust::database::DatabaseType::SQLite => "SQLite",
            };
            let option = format!("{} {} - {} ({})", 
                status,
                conn.display_name,
                timestamp,
                db_type
            );
            options.push(option);
        }
        
        // Use inquire for interactive selection
        let selected_option = inquire::Select::new("Select a recent connection:", options)
            .prompt()
            .map_err(|e| format!("Selection cancelled: {}", e))?;
        
        // Find the index of the selected option
        let selected_index = recent_connections.iter().take(20).enumerate()
            .find(|(_i, conn)| {
                let status = if conn.success { "✅" } else { "❌" };
                let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
                let db_type = match conn.database_type {
                    dbcrust::database::DatabaseType::PostgreSQL => "PostgreSQL",
                    dbcrust::database::DatabaseType::MySQL => "MySQL",
                    dbcrust::database::DatabaseType::SQLite => "SQLite",
                };
                let option = format!("{} {} - {} ({})", 
                    status,
                    conn.display_name,
                    timestamp,
                    db_type
                );
                option == selected_option
            })
            .map(|(i, _)| i)
            .ok_or("Invalid selection")?;
        
        let selected_connection = &recent_connections[selected_index];
        println!("🔗 Connecting to: {}", selected_connection.display_name);
        
        // Use the stored connection URL to reconnect
        // For Docker connections, we need to re-resolve them since the container IP might have changed
        if selected_connection.connection_url.contains(" # Docker: ") {
            // Extract the container name from the Docker comment
            if let Some(docker_pos) = selected_connection.connection_url.find(" # Docker: ") {
                let container_name = &selected_connection.connection_url[docker_pos + 11..]; // Skip " # Docker: "
                full_url_str = format!("docker://{}", container_name);
                println!("🐳 Re-resolving Docker container: {}", container_name);
            } else {
                full_url_str = selected_connection.connection_url.clone();
            }
        } else {
            full_url_str = selected_connection.connection_url.clone();
        }
    }

    // Handle vault URLs
    if full_url_str.starts_with("vault://") || full_url_str.starts_with("vaultdb://") {
        // Parse vault URL and get dynamic credentials
        let vault_params = parse_vault_url(&full_url_str)
            .ok_or_else(|| format!("Invalid vault URL format: {}", full_url_str))?;

        // Get vault credentials and construct connection URL
        let (role_name, mount_path, db_name) = vault_params;
        
        // Handle interactive prompting for missing components
        let db_name = match db_name {
            Some(name) => name,
            None => {
                // List available databases and prompt user to select
                match dbcrust::vault_client::list_vault_databases(&mount_path).await {
                    Ok(databases) => {
                        if databases.is_empty() {
                            eprintln!("No databases available at mount path '{}'", mount_path);
                            std::process::exit(1);
                        }
                        
                        // Filter databases to only show those with available roles
                        let accessible_databases = dbcrust::vault_client::filter_databases_with_available_roles(&mount_path, databases).await
                            .map_err(|e| {
                                eprintln!("Error filtering databases: {}", e);
                                e
                            })?;
                        
                        if accessible_databases.is_empty() {
                            eprintln!("No accessible databases found at mount path '{}'", mount_path);
                            std::process::exit(1);
                        }
                        
                        // Prompt user to select database
                        match inquire::Select::new("Select a database:", accessible_databases.clone()).prompt() {
                            Ok(selected) => selected,
                            Err(e) => {
                                eprintln!("Error selecting database: {}", e);
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to list databases: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        };
        
        let role_name = match role_name {
            Some(name) => name,
            None => {
                // List available roles for the database and prompt user to select
                match dbcrust::vault_client::get_available_roles_for_user(&mount_path, &db_name).await {
                    Ok(roles) => {
                        if roles.is_empty() {
                            eprintln!("No roles available for database '{}'", db_name);
                            std::process::exit(1);
                        }
                        
                        // Prompt user to select role
                        match inquire::Select::new("Select a role:", roles.clone()).prompt() {
                            Ok(selected) => selected,
                            Err(e) => {
                                eprintln!("Error selecting role: {}", e);
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to get available roles: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        };

        println!("🔐 Requesting temporary database credentials from Vault...");
        println!("   This may take 5-10 seconds while Vault creates a new database user.");
        let start_time = std::time::Instant::now();
        let dynamic_creds = dbcrust::vault_client::get_dynamic_credentials(
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
        // Note: This might have already been fetched during role selection
        debug_log!("Getting database configuration...");
        let config_start = std::time::Instant::now();
        let db_config = dbcrust::vault_client::get_vault_database_config(&mount_path, &db_name)
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

        let postgres_url = dbcrust::vault_client::construct_postgres_url(
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
            dbcrust::database::DatabaseType::PostgreSQL, // Vault typically provides PostgreSQL
            true
        ) {
            debug_log!("Failed to add vault connection to history: {}", e);
        }

        // Handle commands and start interactive mode
        return handle_database_connection(database, config, args).await;
    }

    // Handle Docker URLs specially to get connection info for tracking
    let (database, docker_connection_info) = if full_url_str.starts_with("docker://") {
        Database::from_docker_url_with_tracking(
            &full_url_str,
            Some(config.default_limit.clone()),
            Some(config.expanded_display_default.clone()),
        )
        .await
        .map_err(|e| {
            eprintln!("Failed to connect to database: {}", e);
            eprintln!("Connection URL: {}", crate::password_sanitizer::sanitize_connection_url(&full_url_str));
            e
        })?
    } else {
        let database = Database::from_url(
            &full_url_str,
            Some(config.default_limit.clone()),
            Some(config.expanded_display_default.clone()),
        )
        .await
        .map_err(|e| {
            eprintln!("Failed to connect to database: {}", e);
            eprintln!("Connection URL: {}", crate::password_sanitizer::sanitize_connection_url(&full_url_str));
            e
        })?;
        (database, None)
    };

    // Handle SSH tunnel if specified
    if let Some(ref tunnel_str) = args.ssh_tunnel {
        let _ssh_tunnel_config = config.parse_ssh_tunnel_string(tunnel_str);
        // SSH tunnel configuration would be handled by the Database::from_url method
        // or we could add it as a separate configuration step
    }

    println!("✓ Successfully connected to database");

    // Track connection in history
    let (database_type, connection_url_for_history) = if let Some(resolved_info) = docker_connection_info {
        // For Docker connections, use the resolved connection info
        let resolved_url = resolved_info.to_url();
        let sanitized_url = password_sanitizer::sanitize_connection_url(&resolved_url);
        (resolved_info.database_type, sanitized_url)
    } else {
        // For non-Docker connections, use the original URL
        let database_type = if full_url_str.starts_with("postgresql://") {
            dbcrust::database::DatabaseType::PostgreSQL
        } else if full_url_str.starts_with("mysql://") {
            dbcrust::database::DatabaseType::MySQL
        } else if full_url_str.starts_with("sqlite://") {
            dbcrust::database::DatabaseType::SQLite
        } else {
            // Default to PostgreSQL for URLs without scheme
            dbcrust::database::DatabaseType::PostgreSQL
        };
        
        let sanitized_url = password_sanitizer::sanitize_connection_url(&full_url_str);
        (database_type, sanitized_url)
    };
    
    if let Err(e) = config.add_recent_connection_auto_display(
        connection_url_for_history,
        database_type,
        true
    ) {
        debug_log!("Failed to add connection to history: {}", e);
    }

    // Handle commands and start interactive mode
    handle_database_connection(database, config, args).await
}

/// Handle database connection after successful connection
pub async fn handle_database_connection(
    mut database: Database,
    mut config: DbCrustConfig,
    args: Args,
) -> Result<(), Box<dyn StdError>> {
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
                    (db_guard.get_username().to_string(), db_guard.get_current_db())
                };
                let mut prompt = DbPrompt::with_config(
                    username,
                    db_name,
                    config.multiline_prompt_indicator.clone(),
                );
                
                match command_registry.execute(
                    command_trimmed,
                    &db_arc,
                    &mut config,
                    &mut last_script,
                    &interrupt_flag,
                    &mut prompt,
                ).await {
                    Ok(should_exit) => {
                        if should_exit {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        eprintln!("Error executing command: {}", e);
                        std::process::exit(1);
                    }
                }
                
                // Update the database reference
                database = Arc::try_unwrap(db_arc).map_err(|_| "Failed to unwrap Arc")?.into_inner().map_err(|_| "Failed to unwrap Mutex")?;
                continue;
            }

            // Execute the command
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
                    std::process::exit(1);
                }
            }
        }
        return Ok(());
    }

    // Start interactive mode
    run_interactive_mode(database, config, args).await
}

/// Run the interactive mode for any database type
pub async fn run_interactive_mode(
    database: Database,
    mut config: DbCrustConfig,
    args: Args,
) -> Result<(), Box<dyn StdError>> {
    // Print banner if not disabled and config allows it
    if !args.no_banner && config.show_banner_default {
        print_banner(&config);
        println!("Connected to database: {}", database.get_current_db());
    }

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

    let history_path = DbCrustConfigModule::get_config_dir()
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
    let mut prompt = DbPrompt::with_config(username, db_name, config.multiline_prompt_indicator.clone());

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
                    match command_registry.execute(
                        input_trimmed,
                        &db_arc,
                        &mut config,
                        &mut last_script,
                        &interrupt_flag,
                        &mut prompt,
                    ).await {
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

fn main() -> Result<(), Box<dyn StdError>> {
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(async_main());
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
