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
mod prompt;
mod script;

use clap::{CommandFactory, Parser};
use cli::Args;
use dbcrust::completion::{NoopCompleter, SqlCompleter};
use dbcrust::config::{
    self as DbCrustConfigModule,
    Config as DbCrustConfig, // Removed ConnectionConfig, SavedSession
};
use dbcrust::db::Database as DbCrustDatabase;
// Removed DbCrustDbModule and DbCrustTableDetails
use dbcrust::db::Database;
// use dbcrust::database::{ConnectionInfo, create_database_client}; // Will be used for future PostgreSQL migration
use dbcrust::format::{
    format_query_results_expanded, format_query_results_psql, format_table_details,
};
use dirs;
use highlighter::SqlHighlighter;
use inquire::Select;
use nu_ansi_term::{Color, Style};
// Import ssh_tunnel module from lib.rs
use dbcrust::logging;
use dbcrust::{get_mysql_config_path, lookup_mysql_password};
use prettytable::{Cell, Row, Table};
use prompt::DbPrompt;
use reedline::{
    default_emacs_keybindings, ColumnarMenu, Completer, DefaultHinter, Emacs, EditCommand, FileBackedHistory, KeyCode,
    KeyModifiers, MenuBuilder, ReedlineEvent, ReedlineMenu,
};
use reedline::{Reedline, Signal};
use rpassword::prompt_password;
use script::{edit_multiline_script, load_script_from_file, save_script_to_file};
use signal_hook::{consts::SIGINT, flag};
use sqlx::postgres::PgSslMode;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::io::{self, Write};
use arboard::Clipboard;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use terminal_size;
use url::Url;
// For `std::io::stdout().flush()`

/// Database-aware password lookup
/// Determines the database type and uses the appropriate configuration file
fn lookup_database_password(
    database_url: Option<&str>,
    host: &str,
    port: u16,
    database: &str,
    user: &str,
) -> Option<String> {
    // Determine database type from URL or default to PostgreSQL
    let database_type = if let Some(url) = database_url {
        if url.starts_with("mysql://") {
            "mysql"
        } else if url.starts_with("sqlite://") {
            "sqlite"
        } else {
            "postgresql"
        }
    } else {
        // Default to PostgreSQL for backward compatibility
        "postgresql"
    };

    debug_log!(
        "[lookup_database_password] Looking up password for {} database",
        database_type
    );

    match database_type {
        "mysql" => {
            debug_log!("[lookup_database_password] Using MySQL configuration file lookup");
            lookup_mysql_password(host, port, database, user)
        }
        "sqlite" => {
            // SQLite doesn't typically use passwords (file-based)
            debug_log!("[lookup_database_password] SQLite doesn't use password authentication");
            None
        }
        _ => {
            // Default to PostgreSQL pgpass lookup
            debug_log!("[lookup_database_password] Using PostgreSQL .pgpass file lookup");
            pgpass::lookup_password(host, port, database, user)
        }
    }
}

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
    println!("  Passwords are always stored in the .pgpass file, not in saved sessions");
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

async fn async_main() -> Result<(), Box<dyn StdError>> {
    // Initialize the logging system
    if let Err(e) = logging::init() {
        eprintln!("Warning: Failed to initialize logging: {}", e);
    }
    debug_log!("DbCrust started");

    let mut config = DbCrustConfig::load(); // Load config first for defaults and other settings
    let mut args = Args::parse();

    // Handle the help-all flag if provided
    if args.help_all {
        print_help(&config);
        return Ok(());
    }

    // Handle shell completion generation if requested
    if let Some(shell) = args.generate_completion {
        let mut cmd = Args::command();

        if let Some(path) = args.completion_out {
            // Write to a file
            let mut file = std::fs::File::create(&path)
                .map_err(|e| format!("Failed to create completion file: {}", e))?;

            match shell {
                cli::Shell::Bash => {
                    clap_complete::generate(
                        clap_complete::shells::Bash,
                        &mut cmd,
                        "dbcrust",
                        &mut file,
                    );
                }
                cli::Shell::Zsh => {
                    clap_complete::generate(
                        clap_complete::shells::Zsh,
                        &mut cmd,
                        "dbcrust",
                        &mut file,
                    );
                }
                cli::Shell::Fish => {
                    clap_complete::generate(
                        clap_complete::shells::Fish,
                        &mut cmd,
                        "dbcrust",
                        &mut file,
                    );
                }
                cli::Shell::PowerShell => {
                    clap_complete::generate(
                        clap_complete::shells::PowerShell,
                        &mut cmd,
                        "dbcrust",
                        &mut file,
                    );
                }
                cli::Shell::Elvish => {
                    clap_complete::generate(
                        clap_complete::shells::Elvish,
                        &mut cmd,
                        "dbcrust",
                        &mut file,
                    );
                }
            }

            println!("Shell completion for {:?} written to {:?}", shell, path);
        } else {
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
        }
        return Ok(());
    }

    // Check if we should just show debug log location and exit
    if args.show_debug_logs {
        match dbcrust::logging::get_log_file_path_string() {
            Some(path) => println!("Debug log file is located at: {}", path),
            None => println!("Debug log file path could not be determined."),
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

    let db_host_final: String;
    let db_port_final: u16;
    let db_user_final: String;
    let mut db_name_final: String;
    let db_password_final: String;
    let mut ssl_mode_final: Option<PgSslMode> = None;

    // First check if we have a vault:// URL
    let vault_params = args
        .connection_url
        .as_ref()
        .and_then(|url| parse_vault_url(url))
        .or_else(|| args.url.as_ref().and_then(|url| parse_vault_url(url)));

    if let Some((role_name, mount_path, db_name)) = vault_params {
        // Set the vault flag and parameters based on the URL
        args.vault = true;
        args.vault_mount_path = mount_path;
        if let Some(role) = role_name {
            args.vault_role_name = Some(role);
        }
        if let Some(name) = db_name {
            args.vault_db_name = Some(name);
        }
    }

    if args.vault {
        // --- Vault Connection Path ---
        if args.command.is_empty() {
            println!("Vault connection enabled. Attempting to fetch credentials...");
        }

        let actual_db_name = match args.vault_db_name {
            Some(ref name) => name.clone(),
            None => {
                if args.command.is_empty() {
                    println!(
                        "Vault database name (--vault-db-name) not specified. Listing available configurations from Vault mount '{}':",
                        args.vault_mount_path
                    );
                }

                match dbcrust::vault_client::list_vault_databases(&args.vault_mount_path).await {
                    Ok(databases) => {
                        if databases.is_empty() {
                            eprintln!(
                                "No database configurations found or accessible under Vault path '{}/config'.",
                                args.vault_mount_path
                            );
                            return Err("No database configurations found in Vault.".into());
                        } else {
                            // Filter databases to only include those with at least one available role
                            let databases_with_roles =
                                dbcrust::vault_client::filter_databases_with_available_roles(
                                    &args.vault_mount_path,
                                    databases,
                                )
                                .await
                                .map_err(|e| {
                                    eprintln!("Error filtering database configurations: {}", e);
                                    e
                                })?;

                            if databases_with_roles.is_empty() {
                                eprintln!(
                                    "No database configurations with accessible roles found under Vault path '{}'.",
                                    args.vault_mount_path
                                );
                                return Err("No database configurations with accessible roles found in Vault.".into());
                            }

                            if args.command.is_empty() {
                                println!("Please select a database configuration:");
                                let selected_db = match Select::new(
                                    "Database configurations",
                                    databases_with_roles.clone(),
                                )
                                .with_help_message("Type to filter, enter to select")
                                .with_page_size(10)
                                .prompt()
                                {
                                    Ok(selection) => selection,
                                    Err(err) => {
                                        // Handle cancellation or other errors
                                        eprintln!("Database selection cancelled: {}", err);
                                        return Err("Database selection cancelled.".into());
                                    }
                                };
                                println!("Selected database: {}", selected_db);
                                selected_db
                            } else {
                                // For non-interactive mode, require explicit database name
                                return Err(
                                    "--vault-db-name is required when using -c option with Vault"
                                        .into(),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error listing Vault database configurations: {}", e);
                        return Err(format!(
                            "Failed to list database configurations from Vault: {}",
                            e
                        )
                        .into());
                    }
                }
            }
        };

        let db_config_data = dbcrust::vault_client::get_vault_database_config(
            &args.vault_mount_path,
            &actual_db_name,
        )
        .await
        .map_err(|e| {
            eprintln!(
                "Failed to get Vault database configuration for '{}' at mount '{}': {}",
                actual_db_name, args.vault_mount_path, e
            );
            e
        })?;

        let connection_url_template = db_config_data
            .connection_details
            .connection_url
            .ok_or_else(|| {
                dbcrust::vault_client::VaultError::MissingConnectionUrl(actual_db_name.clone())
            })?;

        let actual_role_name = match args.vault_role_name {
            Some(ref role) => role.clone(),
            None => {
                // Instead of using db_config_data.allowed_roles, use our new function to get roles
                // that the current user has access to
                let available_roles = dbcrust::vault_client::get_available_roles_for_user(
                    &args.vault_mount_path,
                    &actual_db_name,
                )
                .await
                .map_err(|e| {
                    eprintln!(
                        "Failed to get available roles for database '{}' at mount '{}': {}",
                        actual_db_name, args.vault_mount_path, e
                    );
                    e
                })?;

                if !available_roles.is_empty() {
                    if args.command.is_empty() {
                        // Always display interactive selection for roles
                        println!("Please select a role:");
                    }
                    let selected_role = if args.command.is_empty() {
                        match Select::new("Available roles", available_roles.clone())
                            .with_help_message("Type to filter, enter to select")
                            .with_page_size(10)
                            .prompt()
                        {
                            Ok(selection) => selection,
                            Err(err) => {
                                // Handle cancellation or other errors
                                eprintln!("Role selection cancelled: {}", err);
                                return Err("Role selection cancelled.".into());
                            }
                        }
                    } else {
                        // For non-interactive mode, require explicit role name
                        return Err(
                            "--vault-role-name is required when using -c option with Vault".into(),
                        );
                    };

                    selected_role.to_string()
                } else {
                    eprintln!(
                        "No available roles found for database '{}' at mount '{}'. This could be because the database has no roles configured, or because you don't have permission to use any of the roles.",
                        actual_db_name, args.vault_mount_path
                    );
                    return Err(format!("No available roles for '{}'.", actual_db_name).into());
                }
            }
        };

        let dynamic_creds = dbcrust::vault_client::get_dynamic_credentials(
            &args.vault_mount_path,
            &actual_db_name,
            &actual_role_name,
        )
        .await
        .map_err(|e| {
            eprintln!(
                "Failed to get dynamic credentials for database '{}' with role '{}' at mount '{}': {}",
                actual_db_name, actual_role_name, args.vault_mount_path, e
            );
            e
        })?;

        if args.command.is_empty() {
            println!(
                "Successfully obtained dynamic credentials for database '{}' with role '{}'",
                actual_db_name, actual_role_name
            );
        }

        // Parse the connection_url with dynamic credentials
        let postgres_url = dbcrust::vault_client::construct_postgres_url(
            &connection_url_template,
            &dynamic_creds.username,
            &dynamic_creds.password,
        )
        .map_err(|e| {
            eprintln!("Failed to construct PostgreSQL URL from template: {}", e);
            e
        })?;

        // Parse the full URL to extract components
        let url_obj = Url::parse(&postgres_url).map_err(|e| {
            eprintln!("Failed to parse constructed URL: {}", e);
            e
        })?;

        // Extract components from the URL
        db_host_final = url_obj.host_str().unwrap_or("localhost").to_string();
        db_port_final = url_obj.port().unwrap_or(5432);
        db_user_final = url_obj.username().to_string();
        db_password_final = url_obj.password().unwrap_or("").to_string();
        db_name_final = url_obj.path().trim_start_matches('/').to_string();

        if db_name_final.is_empty() {
            db_name_final = "postgres".to_string(); // Default database name if not in URL
        }

        if args.command.is_empty() {
            println!(
                "Connecting to PostgreSQL at {}:{}/{} as user {}",
                db_host_final, db_port_final, db_name_final, db_user_final
            );
        }
    } else {
        // --- Standard Connection Path (URL, CLI args, or loaded config) ---
        // First check the positional connection_url, then the --url flag, then fall back to individual parameters
        let args_clone = args.clone();
        let url_str = args.connection_url.as_ref().or(args.url.as_ref()).cloned();

        if let Some(url_str) = url_str {
            let full_url_str = if !url_str.contains("://") {
                format!("postgresql://{}", url_str)
            } else {
                url_str
            };

            // Check if this is a non-PostgreSQL URL and handle it with the new abstraction layer
            if full_url_str.starts_with("sqlite://") || full_url_str.starts_with("mysql://") || full_url_str.starts_with("docker://") {
                // Use the new Database::from_url method for non-PostgreSQL databases
                debug_log!("Detected non-PostgreSQL URL, using new database abstraction layer");
                let mut database = match Database::from_url(
                    &full_url_str,
                    Some(config.default_limit.clone()),
                    Some(config.expanded_display_default.clone()),
                )
                .await
                {
                    Ok(db) => {
                        if full_url_str.starts_with("sqlite://") {
                            println!("✓ Successfully connected to SQLite database");
                        } else if full_url_str.starts_with("mysql://") {
                            println!("✓ Successfully connected to MySQL database");
                        } else if full_url_str.starts_with("docker://") {
                            println!("✓ Successfully connected to Docker database");
                        }
                        db
                    }
                    Err(e) => {
                        let db_type = if full_url_str.starts_with("sqlite://") {
                            "SQLite"
                        } else if full_url_str.starts_with("mysql://") {
                            "MySQL"
                        } else {
                            "Docker"
                        };
                        eprintln!("✗ Failed to connect to {} database", db_type);
                        eprintln!(
                            "Connection URL: {}",
                            crate::password_sanitizer::sanitize_connection_url(&full_url_str)
                        );

                        let error_msg = e.to_string();
                        if full_url_str.starts_with("sqlite://") {
                            if error_msg.contains("does not exist")
                                || error_msg.contains("No such file")
                            {
                                eprintln!("\n🔍 SQLite File Issue:");
                                eprintln!("Database file does not exist or is not accessible");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Check if the file path is correct");
                                eprintln!("• Verify you have read/write permissions");
                                eprintln!("• Create the directory if it doesn't exist");
                                eprintln!(
                                    "• Use absolute path: sqlite:///full/path/to/database.db"
                                );
                            } else if error_msg.contains("permission")
                                || error_msg.contains("Permission denied")
                            {
                                eprintln!("\n🔍 SQLite Permission Issue:");
                                eprintln!("Permission denied accessing database file");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Check file permissions: ls -la path/to/database.db");
                                eprintln!("• Ensure directory is writable");
                                eprintln!("• Run with appropriate user permissions");
                            }
                        } else if full_url_str.starts_with("mysql://") {
                            // MySQL
                            if error_msg.contains("DNS resolution failed")
                                || error_msg.contains("nodename nor servname")
                            {
                                eprintln!("\n🔍 MySQL DNS Resolution Issue:");
                                eprintln!("The MySQL hostname could not be resolved");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Check if the hostname is spelled correctly");
                                eprintln!("• Try using an IP address instead");
                                eprintln!("• Verify your network connection");
                            } else if error_msg.contains("Connection refused")
                                || error_msg.contains("Connection timeout")
                            {
                                eprintln!("\n🔍 MySQL Connection Issue:");
                                eprintln!("Could not connect to MySQL server");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Verify MySQL server is running");
                                eprintln!("• Check if port 3306 is correct");
                                eprintln!("• Ensure firewall allows connections");
                                eprintln!("• Try: mysql -h hostname -u username -p");
                            } else if error_msg.contains("Access denied")
                                || error_msg.contains("authentication")
                            {
                                eprintln!("\n🔍 MySQL Authentication Issue:");
                                eprintln!("Username or password is incorrect");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Verify username exists in MySQL");
                                eprintln!("• Check password (use .my.cnf file for security)");
                                eprintln!("• Ensure user has proper privileges");
                            }
                        } else {
                            // Docker
                            if error_msg.contains("Container") && error_msg.contains("not found") {
                                eprintln!("\n🔍 Docker Container Issue:");
                                eprintln!("Container not found or not running");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Check if container exists: docker ps -a");
                                eprintln!("• Start container: docker start container_name");
                                eprintln!("• Verify container name is correct");
                                eprintln!("• List database containers: docker ps --filter 'label=database'");
                            } else if error_msg.contains("Docker connection failed") {
                                eprintln!("\n🔍 Docker Connection Issue:");
                                eprintln!("Cannot connect to Docker daemon");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Check if Docker daemon is running");
                                eprintln!("• Verify Docker socket permissions");
                                eprintln!("• Try: docker info");
                            } else if error_msg.contains("No exposed ports") {
                                eprintln!("\n🔍 Docker Port Issue:");
                                eprintln!("Database port not exposed from container");
                                eprintln!("Troubleshooting steps:");
                                eprintln!("• Check port mapping: docker port container_name");
                                eprintln!("• Restart container with -p flag: docker run -p 5432:5432");
                                eprintln!("• Verify database is listening on correct port");
                            }
                        }

                        eprintln!("\nDetailed error: {}", e);
                        return Err(e);
                    }
                };

                debug_log!("Successfully connected to database");

                // Handle -c commands if provided (execute and exit)
                if !args.command.is_empty() {
                    for command in &args.command {
                        let command_trimmed = command.trim();

                        if command_trimmed.is_empty() {
                            continue;
                        }

                        // Check if this is a backslash command
                        if command_trimmed.starts_with('\\') {
                            // Handle backslash commands (create minimal context for compatibility)
                            let db_arc = Arc::new(Mutex::new(database));
                            let mut dummy_last_script = String::new();
                            let dummy_interrupt_flag = Arc::new(AtomicBool::new(false));
                            let mut dummy_prompt =
                                DbPrompt::new("user".to_string(), "db".to_string());

                            match handle_backslash_command(
                                command_trimmed,
                                &db_arc,
                                &mut dummy_last_script,
                                &dummy_interrupt_flag,
                                &mut dummy_prompt,
                            )
                            .await
                            {
                                Ok(should_exit) => {
                                    if should_exit {
                                        return Ok(());
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Error executing backslash command: {}", e);
                                }
                            }

                            // Get the database back from the Arc for next iteration
                            database = Arc::try_unwrap(db_arc)
                                .map_err(|_| "Failed to unwrap Arc")?
                                .into_inner()
                                .map_err(|_| "Failed to unlock Mutex")?;
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
                                        let expanded_tables =
                                            format_query_results_expanded(&results);
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

                // Start interactive mode for non-PostgreSQL database
                return run_interactive_mode(database, config, args_clone).await;
            }

            match Url::parse(&full_url_str) {
                Ok(parsed_url) => {
                    db_host_final = parsed_url
                        .host_str()
                        .unwrap_or(&config.connection.host)
                        .to_string();

                    // Only use saved connection details as fallbacks if the host matches
                    // Otherwise use standard PostgreSQL defaults
                    let host_matches_saved = if let Some(host_from_url) = parsed_url.host_str() {
                        host_from_url == config.connection.host
                    } else {
                        true // If no host in URL, we're using config host anyway
                    };

                    db_port_final = parsed_url.port().unwrap_or(if host_matches_saved {
                        config.connection.port
                    } else {
                        5432
                    });

                    db_user_final = if parsed_url.username().is_empty() {
                        if host_matches_saved {
                            config.connection.user.clone()
                        } else {
                            "postgres".to_string()
                        }
                    } else {
                        parsed_url.username().to_string()
                    };

                    let path_dbname = parsed_url.path().trim_start_matches('/').to_string();
                    db_name_final = if path_dbname.is_empty() {
                        if host_matches_saved {
                            config.connection.dbname.clone()
                        } else {
                            "postgres".to_string()
                        }
                    } else {
                        path_dbname
                    };

                    // Parse query parameters for sslmode
                    if let Some(query_pairs) = parsed_url.query() {
                        let query_params: HashMap<String, String> =
                            url::form_urlencoded::parse(query_pairs.as_bytes())
                                .into_owned()
                                .collect();

                        if let Some(sslmode_str) = query_params.get("sslmode") {
                            ssl_mode_final = match sslmode_str.as_str() {
                                "disable" => Some(PgSslMode::Disable),
                                "allow" => Some(PgSslMode::Allow),
                                "prefer" => Some(PgSslMode::Prefer),
                                "require" => Some(PgSslMode::Require),
                                "verify-ca" => Some(PgSslMode::VerifyCa),
                                "verify-full" => Some(PgSslMode::VerifyFull),
                                _ => Some(PgSslMode::Prefer), // Default for unknown values
                            };
                        }
                    }

                    if let Some(pass_from_url) = parsed_url.password() {
                        db_password_final = pass_from_url.to_string();
                    } else {
                        // No password in URL, resolve through other means
                        db_password_final = match args.password {
                            Some(pass) => pass, // CLI --password
                            None => lookup_database_password(
                                Some(&full_url_str),
                                &db_host_final,
                                db_port_final,
                                &db_name_final,
                                &db_user_final,
                            )
                            .unwrap_or_else(|| {
                                print!(
                                    "Password for {}@{}:{}/{}: ",
                                    db_user_final, db_host_final, db_port_final, db_name_final
                                );
                                std::io::stdout().flush().expect("Flush failed");
                                prompt_password("").expect("Failed to read password")
                            }),
                        };
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Error parsing connection URL: {}. Falling back to individual CLI args/config.",
                        e
                    );
                    db_host_final = args.host.clone(); // CLI --host or default from config
                    db_port_final = args.port; // CLI --port or default from config
                    db_user_final = args.user.clone(); // CLI --user or default from config
                    db_name_final = args.dbname.clone(); // CLI --dbname or default from config
                    db_password_final = match args.password {
                        // CLI --password
                        Some(pass) => pass,
                        None => lookup_database_password(
                            args.connection_url.as_deref().or(args.url.as_deref()),
                            &db_host_final,
                            db_port_final,
                            &db_name_final,
                            &db_user_final,
                        )
                        .unwrap_or_else(|| {
                            print!(
                                "Password for {}@{}:{}/{}: ",
                                db_user_final, db_host_final, db_port_final, db_name_final
                            );
                            std::io::stdout().flush().expect("Flush failed");
                            prompt_password("").expect("Failed to read password")
                        }),
                    };
                }
            }
        } else {
            // No Vault, No URL: Use individual CLI args (which have defaults from clap, falling back to loaded config values if not specified on CLI)
            db_host_final = args.host.clone();
            db_port_final = args.port;
            db_user_final = args.user.clone();
            db_name_final = args.dbname.clone();

            db_password_final = match args.password {
                Some(pass) => pass, // CLI --password
                None => match config.connection.password {
                    // Password from loaded config (if any, e.g. from a previous --url in config)
                    Some(ref conf_pass) if !conf_pass.is_empty() => conf_pass.clone(),
                    _ => lookup_database_password(
                        args.connection_url.as_deref().or(args.url.as_deref()),
                        &db_host_final,
                        db_port_final,
                        &db_name_final,
                        &db_user_final,
                    )
                    .unwrap_or_else(|| {
                        print!(
                            "Password for {}@{}:{}/{}: ",
                            db_user_final, db_host_final, db_port_final, db_name_final
                        );
                        std::io::stdout().flush().expect("Flush failed");
                        prompt_password("").expect("Failed to read password")
                    }),
                },
            };
        }
    }

    // After db_host_final etc. are set, update the main config an_instance for consistency elsewhere.
    // Using update_connection_params to avoid accidentally overwriting user config sections
    config.update_connection_params(
        db_host_final.clone(),
        db_port_final,
        db_user_final.clone(),
        db_name_final.clone(),
    );
    config.connection.password = None; // Always clear password from main config object after resolution

    if !args.no_banner && args.command.is_empty() && config.show_banner_default {
        print_banner(&config);
    }

    // SSH tunnel configuration uses the db_host_final resolved above
    let ssh_tunnel_config = if let Some(tunnel_str) = args.ssh_tunnel {
        config.parse_ssh_tunnel_string(&tunnel_str)
    } else {
        config.get_ssh_tunnel_for_host(&db_host_final)
    };

    let mut db = match DbCrustDatabase::new(
        &db_host_final,
        db_port_final,
        &db_user_final,
        &db_password_final,
        &db_name_final,
        Some(config.default_limit),
        Some(config.expanded_display_default),
        ssh_tunnel_config,
        ssl_mode_final,
    )
    .await
    {
        Ok(database) => {
            println!("✓ Successfully connected to PostgreSQL database");
            database
        }
        Err(e) => {
            eprintln!("✗ Failed to connect to PostgreSQL database");
            eprintln!(
                "Connection details: {}@{}:{}/{}",
                db_user_final, db_host_final, db_port_final, db_name_final
            );

            // Enhanced error message based on error type
            let error_msg = e.to_string();
            if error_msg.contains("DNS resolution failed")
                || error_msg.contains("nodename nor servname")
            {
                eprintln!("\n🔍 DNS Resolution Issue:");
                eprintln!("The hostname '{}' could not be resolved.", db_host_final);
                eprintln!("Troubleshooting steps:");
                eprintln!("• Check if the hostname is spelled correctly");
                eprintln!(
                    "• Try using an IP address instead: dbcrust postgres://user:pass@192.168.1.100/db"
                );
                eprintln!("• Verify your network connection");
                eprintln!("• Check if your DNS server is working");
            } else if error_msg.contains("Connection timeout")
                || error_msg.contains("Connection refused")
            {
                eprintln!("\n🔍 Connection Issue:");
                eprintln!("Could not connect to {}:{}", db_host_final, db_port_final);
                eprintln!("Troubleshooting steps:");
                eprintln!("• Verify PostgreSQL server is running");
                eprintln!(
                    "• Check if port {} is correct (default is 5432)",
                    db_port_final
                );
                eprintln!("• Ensure firewall allows connections");
                eprintln!("• Try: telnet {} {}", db_host_final, db_port_final);
            } else if error_msg.contains("password authentication failed")
                || error_msg.contains("authentication")
            {
                eprintln!("\n🔍 Authentication Issue:");
                eprintln!("Username or password is incorrect");
                eprintln!("Troubleshooting steps:");
                eprintln!("• Verify username '{}' exists", db_user_final);
                eprintln!("• Check password (use .pgpass file for security)");
                eprintln!("• Ensure user has login privileges");
                eprintln!(
                    "• Try: psql -h {} -p {} -U {} -d {}",
                    db_host_final, db_port_final, db_user_final, db_name_final
                );
            } else if error_msg.contains("database") && error_msg.contains("does not exist") {
                eprintln!("\n🔍 Database Issue:");
                eprintln!("Database '{}' does not exist", db_name_final);
                eprintln!("Troubleshooting steps:");
                eprintln!("• Verify database name is correct");
                eprintln!("• List available databases: \\l");
                eprintln!("• Connect to 'postgres' database first");
            } else if error_msg.contains("SSL") || error_msg.contains("tls") {
                eprintln!("\n🔍 SSL/TLS Issue:");
                eprintln!("SSL connection problem");
                eprintln!("Troubleshooting steps:");
                eprintln!(
                    "• Try with sslmode=disable: postgres://user:pass@host/db?sslmode=disable"
                );
                eprintln!("• Check server SSL configuration");
                eprintln!("• Verify certificates if using verify-ca or verify-full");
            }

            eprintln!("\nDetailed error: {}", e);
            return Err(e);
        }
    };

    // Handle -c commands if provided (execute and exit)
    if !args.command.is_empty() {
        for command in &args.command {
            let command_trimmed = command.trim();

            if command_trimmed.is_empty() {
                continue;
            }

            // Execute the command
            match db.execute_query(command_trimmed).await {
                Ok(results) => {
                    if results.is_empty() {
                        // No output for commands that don't return results (like INSERT, UPDATE, etc.)
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
                Err(err) => {
                    eprintln!("Error: {}", err);
                    // Continue with next command rather than exit, to match psql behavior
                }
            }
        }

        // Close the database connection and exit
        db.close().await;
        return Ok(());
    }

    let db_arc = Arc::new(Mutex::new(db));

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

    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input_trimmed = input.trim();

                if input_trimmed.is_empty() {
                    continue;
                }

                // Handle special commands
                if input_trimmed.starts_with('\\') {
                    match input_trimmed {
                        "\\q" => break,
                        "\\h" => print_help(&config),
                        "\\ed" => {
                            // Enter multiline edit mode
                            println!("Entering multiline edit mode...");

                            // Show current content if any
                            if !last_script.is_empty() {
                                println!("Editing existing script ({}  bytes):", last_script.len());
                                // Only show a preview if it's not too long
                                if last_script.lines().count() <= 5 {
                                    for line in last_script.lines() {
                                        println!("  {}", line);
                                    }
                                } else {
                                    // Show first few lines
                                    for line in last_script.lines().take(3) {
                                        println!("  {}", line);
                                    }
                                    println!(
                                        "  ... ({} more lines) ...",
                                        last_script.lines().count() - 3
                                    );
                                }
                            }

                            match edit_multiline_script(&last_script) {
                                Ok(script) => {
                                    if script.is_empty() {
                                        println!("No changes made (empty script).");
                                        continue;
                                    }

                                    // Save the edited script
                                    last_script = script.clone();

                                    println!(
                                        "Multiline script ready ({} bytes, {} lines). Execute? (y/n) [default: y]",
                                        script.len(),
                                        script.lines().count()
                                    );
                                    let mut confirm = String::new();
                                    std::io::stdin().read_line(&mut confirm)?;
                                    let confirm = confirm.trim().to_lowercase();

                                    if confirm.is_empty() || confirm == "y" || confirm == "yes" {
                                        println!("Executing script...");
                                        let mut db = db_arc.lock().unwrap();
                                        match db.execute_query(&script).await {
                                            Ok(results) => {
                                                if results.is_empty() {
                                                    println!("Query OK, no results.");
                                                } else {
                                                    // Check if we should auto-enable column selection based on column count
                                                    let column_count = results[0].len();
                                                    let auto_enable = db
                                                        .should_auto_enable_column_selection(
                                                            column_count,
                                                        );

                                                    // Process results with column selection if enabled or auto-enabled
                                                    let processed_results = if db
                                                        .is_column_select_mode()
                                                        || auto_enable
                                                    {
                                                        // Reset the interrupt flag before column selection
                                                        interrupt_flag
                                                            .store(false, Ordering::SeqCst);

                                                        // If auto-enabled, show a more informative message
                                                        if auto_enable
                                                            && !db.is_column_select_mode()
                                                        {
                                                            println!(
                                                                "Auto-enabling column selection mode due to high column count ({} columns exceeds threshold of {})",
                                                                column_count,
                                                                db.get_column_selection_threshold()
                                                            );
                                                            println!(
                                                                "This threshold can be configured with \\csthreshold command"
                                                            );
                                                        } else {
                                                            println!(
                                                                "Entering column selection mode..."
                                                            );
                                                        }

                                                        // Interactive column selection
                                                        match db.interactive_column_selection(
                                                            &results,
                                                            &interrupt_flag,
                                                        ) {
                                                            Ok(filtered) => {
                                                                if !filtered.is_empty()
                                                                    && !results.is_empty()
                                                                {
                                                                    println!(
                                                                        "Column selection: filtered data has {} rows, {} columns (original: {} rows, {} columns)",
                                                                        filtered.len(),
                                                                        filtered[0].len(),
                                                                        results.len(),
                                                                        results[0].len()
                                                                    );
                                                                }
                                                                filtered
                                                            }
                                                            Err(e) => {
                                                                eprintln!(
                                                                    "Error during column selection: {}",
                                                                    e
                                                                );
                                                                results
                                                            }
                                                        }
                                                    } else {
                                                        results
                                                    };

                                                    // Format and display the results
                                                    if db.is_expanded_display() {
                                                        let expanded_tables =
                                                            format_query_results_expanded(
                                                                &processed_results,
                                                            );
                                                        let mut output_buffer = String::new();
                                                        for table in expanded_tables {
                                                            output_buffer
                                                                .push_str(&table.to_string());
                                                            output_buffer.push_str("\n");
                                                        }
                                                        if db.is_explain_mode() {
                                                            handle_explain_output(&output_buffer, &config);
                                                        } else {
                                                            handle_output(&output_buffer, &config);
                                                        }
                                                    } else {
                                                        // Use psql-style formatting
                                                        let output = format_query_results_psql(
                                                            &processed_results,
                                                        );
                                                        if db.is_explain_mode() {
                                                            handle_explain_output(&output, &config);
                                                        } else {
                                                            handle_output(&output, &config);
                                                        }
                                                    }
                                                }
                                            }
                                            Err(err) => {
                                                eprintln!("Error: {}", err);
                                            }
                                        }
                                    } else {
                                        println!("Script execution cancelled");
                                        // Still save the script for potential saving later
                                        last_script = script;
                                    }
                                }
                                Err(e) => eprintln!("Error in multiline edit mode: {}", e),
                            }
                        }
                        cmd if cmd.starts_with("\\w ") => {
                            // Save the last executed query or edited script to a file
                            let filename = cmd[3..].trim();
                            if filename.is_empty() {
                                println!("Error: missing filename");
                                continue;
                            }

                            if last_script.is_empty() {
                                println!("Error: no query/script to save");
                                continue;
                            }

                            match save_script_to_file(&last_script, filename) {
                                Ok(_) => println!("Saved script to file: {}", filename),
                                Err(e) => eprintln!("Error saving script: {}", e),
                            }
                        }
                        cmd if cmd.starts_with("\\i ") => {
                            // Load and execute SQL commands from a file
                            let filename = cmd[3..].trim();
                            if filename.is_empty() {
                                println!("Error: missing filename");
                                continue;
                            }

                            match load_script_from_file(filename) {
                                Ok(script) => {
                                    println!("Executing script from file: {}", filename);
                                    let mut db = db_arc.lock().unwrap();
                                    match db.execute_query(&script).await {
                                        Ok(results) => {
                                            // Store the script for potential saving
                                            last_script = script;

                                            if results.is_empty() {
                                                println!("Query OK, no results.");
                                            } else {
                                                // Process results (existing column selection logic)
                                                // ... existing code ...

                                                // Check if we should auto-enable column selection based on column count
                                                let column_count = results[0].len();
                                                let auto_enable = db
                                                    .should_auto_enable_column_selection(
                                                        column_count,
                                                    );

                                                // Process results with column selection if enabled or auto-enabled
                                                let processed_results = if db
                                                    .is_column_select_mode()
                                                    || auto_enable
                                                {
                                                    // Reset the interrupt flag before column selection
                                                    interrupt_flag.store(false, Ordering::SeqCst);

                                                    // If auto-enabled, show a more informative message
                                                    if auto_enable && !db.is_column_select_mode() {
                                                        println!(
                                                            "Auto-enabling column selection mode due to high column count ({} columns exceeds threshold of {})",
                                                            column_count,
                                                            db.get_column_selection_threshold()
                                                        );
                                                        println!(
                                                            "This threshold can be configured with \\csthreshold command"
                                                        );
                                                    } else {
                                                        println!(
                                                            "Entering column selection mode..."
                                                        );
                                                    }

                                                    // Interactive column selection
                                                    match db.interactive_column_selection(
                                                        &results,
                                                        &interrupt_flag,
                                                    ) {
                                                        Ok(filtered) => {
                                                            if !filtered.is_empty()
                                                                && !results.is_empty()
                                                            {
                                                                println!(
                                                                    "Column selection: filtered data has {} rows, {} columns (original: {} rows, {} columns)",
                                                                    filtered.len(),
                                                                    filtered[0].len(),
                                                                    results.len(),
                                                                    results[0].len()
                                                                );
                                                            }
                                                            filtered
                                                        }
                                                        Err(e) => {
                                                            eprintln!(
                                                                "Error during column selection: {}",
                                                                e
                                                            );
                                                            results
                                                        }
                                                    }
                                                } else {
                                                    results
                                                };

                                                // Format and display the results
                                                if db.is_expanded_display() {
                                                    let expanded_tables =
                                                        format_query_results_expanded(
                                                            &processed_results,
                                                        );
                                                    let mut output_buffer = String::new();
                                                    for table in expanded_tables {
                                                        output_buffer.push_str(&table.to_string());
                                                        output_buffer.push_str("\n");
                                                    }
                                                    if db.is_explain_mode() {
                                                        handle_explain_output(&output_buffer, &config);
                                                    } else {
                                                        handle_output(&output_buffer, &config);
                                                    }
                                                } else {
                                                    // Use psql-style formatting
                                                    let output = format_query_results_psql(
                                                        &processed_results,
                                                    );
                                                    if db.is_explain_mode() {
                                                        handle_explain_output(&output, &config);
                                                    } else {
                                                        handle_output(&output, &config);
                                                    }
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            eprintln!("Error: {}", err);
                                        }
                                    }
                                }
                                Err(e) => eprintln!("Error loading script from file: {}", e),
                            }
                        }
                        "\\l" => {
                            let mut db = db_arc.lock().unwrap();
                            match db.list_databases().await {
                                Ok(databases) => {
                                    print!("{}", format_query_results_psql(&databases));
                                }
                                Err(e) => eprintln!("Error listing databases: {}", e),
                            }
                        }
                        "\\dt" => {
                            let mut db = db_arc.lock().unwrap();
                            match db.list_tables().await {
                                Ok(tables) => {
                                    print!("{}", format_query_results_psql(&tables));
                                }
                                Err(e) => eprintln!("Error listing tables: {}", e),
                            }
                        }
                        "\\d" => {
                            let mut db = db_arc.lock().unwrap();
                            match db.list_tables().await {
                                Ok(tables) => {
                                    print!("{}", format_query_results_psql(&tables));
                                }
                                Err(e) => eprintln!("Error listing tables: {}", e),
                            }
                        }
                        "\\x" => {
                            let mut db = db_arc.lock().unwrap();
                            let mode = db.toggle_expanded_display();
                            println!("Expanded display is {}", if mode { "on" } else { "off" });
                        }
                        "\\e" => {
                            let mut db = db_arc.lock().unwrap();
                            let mode = db.toggle_explain_mode();
                            println!("EXPLAIN mode is {}", if mode { "on" } else { "off" });
                        }
                        "\\banner" => {
                            let mut db = db_arc.lock().unwrap();
                            let mode = db.toggle_banner_enabled();
                            println!("Banner display is {}", if mode { "on" } else { "off" });
                        }
                        cmd if cmd.starts_with("\\er ") => {
                            // Execute query with raw EXPLAIN output
                            let query = cmd[4..].trim();
                            if query.is_empty() {
                                println!("Error: Please provide a query after \\er");
                                continue;
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
                                    println!("💡 Use \\copy to copy the raw JSON plan to clipboard");
                                }
                                Err(e) => eprintln!("Error executing raw EXPLAIN: {}", e),
                            }
                        }
                        cmd if cmd.starts_with("\\ef ") => {
                            // Execute query with formatted EXPLAIN output only
                            let query = cmd[4..].trim();
                            if query.is_empty() {
                                println!("Error: Please provide a query after \\ef");
                                continue;
                            }

                            let mut db = db_arc.lock().unwrap();
                            match db.execute_explain_query_formatted(query).await {
                                Ok(results) => {
                                    handle_explain_output(&format_query_results_psql(&results), &config);
                                }
                                Err(e) => eprintln!("Error executing formatted EXPLAIN: {}", e),
                            }
                        }
                        cmd if cmd.starts_with("\\ex ") => {
                            // Execute EXPLAIN and export to file
                            let parts: Vec<&str> = cmd[4..].splitn(2, ' ').collect();
                            if parts.len() < 2 {
                                println!("Error: Please provide a query and filename after \\ex");
                                println!("Usage: \\ex <query> <filename>");
                                continue;
                            }

                            let query = parts[0].trim();
                            let filename = parts[1].trim();

                            if query.is_empty() || filename.is_empty() {
                                println!("Error: Both query and filename must be provided");
                                continue;
                            }

                            let mut db = db_arc.lock().unwrap();
                            match db.execute_explain_query_raw(query).await {
                                Ok(results) => {
                                    let formatted_output = format_query_results_psql(&results);
                                    match std::fs::write(filename, formatted_output) {
                                        Ok(_) => {
                                            println!("EXPLAIN output exported to: {}", filename)
                                        }
                                        Err(e) => {
                                            eprintln!("Error writing to file '{}': {}", filename, e)
                                        }
                                    }
                                }
                                Err(e) => eprintln!("Error executing EXPLAIN for export: {}", e),
                            }
                        }
                        "\\cs" => {
                            let mut db = db_arc.lock().unwrap();
                            let mode = db.toggle_column_select_mode();
                            println!(
                                "Column selection mode is {}",
                                if mode { "on" } else { "off" }
                            );
                        }
                        "\\setx" => {
                            // Get the current expanded display setting
                            let current_expanded = db_arc.lock().unwrap().is_expanded_display();

                            // Update config with the current setting
                            config.expanded_display_default = current_expanded;

                            // Save config
                            match config.save() {
                                Ok(_) => println!(
                                    "Default expanded display set to {}",
                                    if current_expanded { "ON" } else { "OFF" }
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\a" => {
                            let mut db = db_arc.lock().unwrap();
                            let enabled = !db.is_autocomplete();
                            db.set_autocomplete(enabled);
                            println!("Autocomplete is {}", if enabled { "on" } else { "off" });

                            // Update the line editor with new completer
                            if enabled {
                                let new_completer: Box<dyn Completer> =
                                    Box::new(SqlCompleter::new(db_arc.clone()));
                                line_editor = line_editor.with_completer(new_completer);
                                println!("Preparing autocompletion - this may take a moment...");
                            } else {
                                let new_completer: Box<dyn Completer> = Box::new(NoopCompleter {});
                                line_editor = line_editor.with_completer(new_completer);
                            }
                        }
                        "\\seta" => {
                            // Save current autocomplete setting as default
                            let db = db_arc.lock().unwrap();
                            config.autocomplete_enabled = db.is_autocomplete();

                            match config.save() {
                                Ok(_) => println!(
                                    "Default autocomplete set to: {}",
                                    if db.is_autocomplete() { "on" } else { "off" }
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\sete" => {
                            // Save current explain mode as default
                            let db = db_arc.lock().unwrap();
                            config.explain_mode_default = db.is_explain_mode();

                            match config.save() {
                                Ok(_) => println!(
                                    "Default EXPLAIN mode set to: {}",
                                    if db.is_explain_mode() { "on" } else { "off" }
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\setcs" => {
                            // Get the current column selection mode setting
                            let current_mode = db_arc.lock().unwrap().is_column_select_mode();

                            // Update config with the current setting
                            config.column_selection_mode_default = current_mode;

                            // Save config
                            match config.save() {
                                Ok(_) => println!(
                                    "Default column selection mode set to {}",
                                    if current_mode { "ON" } else { "OFF" }
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\setbanner" => {
                            // Get the current banner setting
                            let current_banner = db_arc.lock().unwrap().is_banner_enabled();

                            // Update config with the current setting
                            config.show_banner_default = current_banner;

                            // Save config
                            match config.save() {
                                Ok(_) => println!(
                                    "Default banner display set to: {}",
                                    if current_banner { "on" } else { "off" }
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\clrcs" => {
                            let mut db = db_arc.lock().unwrap();
                            db.clear_column_views();
                            println!("Cleared all saved column views");
                        }
                        "\\resetview" => {
                            let mut db = db_arc.lock().unwrap();
                            db.reset_column_view();
                            println!("Reset column view for the most recent query");
                        }
                        "\\csthreshold" => {
                            // Get the current column selection threshold setting
                            let current_threshold = config.column_selection_threshold;

                            // Prompt for new threshold
                            print!(
                                "Enter new column selection auto-enable threshold (current: {}): ",
                                current_threshold
                            );
                            std::io::stdout().flush().expect("Flush failed");
                            let mut new_threshold = String::new();
                            std::io::stdin().read_line(&mut new_threshold)?;
                            let new_threshold = new_threshold.trim().parse::<usize>()?;

                            // Update config with the new setting
                            config.column_selection_threshold = new_threshold;

                            // Save config
                            match config.save() {
                                Ok(_) => println!(
                                    "Default column selection auto-enable threshold set to {}",
                                    new_threshold
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\pager" => {
                            config.pager_enabled = !config.pager_enabled;
                            if config.pager_enabled {
                                println!("Pager is now enabled.");
                            } else {
                                println!("Pager is now disabled.");
                            }
                        }
                        "\\setpager" => {
                            // The `pager_enabled` field in `config` is already up-to-date due to \pager command
                            // So we just need to save the config.
                            match config.save() {
                                Ok(_) => println!(
                                    "Default pager setting saved: {}",
                                    if config.pager_enabled {
                                        "enabled"
                                    } else {
                                        "disabled"
                                    }
                                ),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\config" => {
                            let config_str = format!("{:#?}", config); // Use debug formatting for config
                            let mut output = config_str.clone();

                            // Add debug log file information
                            output.push_str("\nDebug Logging Information:\n");
                            output.push_str(&format!(
                                "  Debug logging enabled: {}\n",
                                config.debug_logging_enabled
                            ));

                            // Show debug log file location if available
                            match dbcrust::get_log_file_path_string() {
                                Some(path) => {
                                    output.push_str(&format!("  Debug log file: {}\n", path))
                                }
                                None => output
                                    .push_str("  Debug log file path could not be determined.\n"),
                            }

                            if config.debug_logging_enabled {
                                output
                                    .push_str("  Debug logs are being written to the log file.\n");
                                output.push_str("  To disable debug logging, set debug_logging_enabled = false in config.toml\n");
                            } else {
                                output.push_str("  Debug logging is disabled. To enable, set debug_logging_enabled = true in config.toml\n");
                            }

                            handle_output(&output, &config); // config output can also be paged
                        }
                        "\\save" => {
                            // Get the current database name
                            let current_db = db_arc.lock().unwrap().get_current_db();
                            let current_host = config.connection.host.clone();
                            let current_port = config.connection.port;
                            let current_user = config.connection.user.clone();

                            // Ask for confirmation before saving to default config
                            println!(
                                "Save current connection parameters as default configuration?"
                            );
                            println!("This will update your config.toml file with:");
                            println!("  host   = \"{}\"", current_host);
                            println!("  port   = {}", current_port);
                            println!("  user   = \"{}\"", current_user);
                            println!("  dbname = \"{}\"", current_db);
                            println!("Confirm (y/n) [default: n]:");

                            let mut confirm = String::new();
                            std::io::stdin().read_line(&mut confirm)?;
                            let confirm = confirm.trim().to_lowercase();

                            if confirm != "y" && confirm != "yes" {
                                println!("Operation cancelled.");
                                continue;
                            }

                            // Update config with current database
                            if current_db != config.connection.dbname {
                                config.connection.dbname = current_db;
                            }

                            // Ask if password should be saved
                            println!("Save password to .pgpass file? (y/n) [default: n]");
                            let mut save_pass = String::new();
                            std::io::stdin().read_line(&mut save_pass)?;
                            let save_pass = save_pass.trim().to_lowercase();

                            if save_pass == "y" || save_pass == "yes" {
                                // Save password to .pgpass file
                                match pgpass::save_password(
                                    &config.connection.host,
                                    config.connection.port,
                                    &config.connection.dbname,
                                    &config.connection.user,
                                    &db_password_final,
                                ) {
                                    Ok(_) => println!("Password saved to .pgpass file"),
                                    Err(e) => {
                                        eprintln!("Error saving password to .pgpass file: {}", e)
                                    }
                                }
                            }

                            // Never store password in config file
                            config.connection.save_password = false;
                            config.connection.password = None;

                            // Save config
                            match config.save() {
                                Ok(_) => println!("Configuration saved successfully"),
                                Err(e) => eprintln!("Error saving configuration: {}", e),
                            }
                        }
                        "\\pgpass" => {
                            println!("Information about database password files:");

                            // PostgreSQL .pgpass file information
                            println!("\n📁 PostgreSQL (.pgpass file):");
                            println!(
                                "  .pgpass file is used to store PostgreSQL database passwords."
                            );
                            println!("  It is a text file with the following format:");
                            println!("    hostname:port:database:username:password");

                            match pgpass::get_pgpass_path() {
                                Some(path) => {
                                    println!("  Your .pgpass file location: {}", path.display());
                                    if path.exists() {
                                        println!("  Status: ✅ File exists");
                                    } else {
                                        println!("  Status: ❌ File does not exist");
                                    }
                                }
                                None => println!("  Could not determine .pgpass file location"),
                            }

                            println!("  Each field can contain * as a wildcard.");
                            println!(
                                "  On Unix systems, the file should have permissions 0600 (readable/writable only by owner)."
                            );
                            println!("  You can set permissions with: chmod 0600 ~/.pgpass");
                            println!(
                                "  You can also set the PGPASSFILE environment variable to specify a different location."
                            );

                            // MySQL .my.cnf file information
                            println!("\n🐬 MySQL (.my.cnf file):");
                            println!(
                                "  .my.cnf file is used to store MySQL connection options and passwords."
                            );
                            println!("  It is an INI-style configuration file with sections:");
                            println!("    [client]");
                            println!("    host = hostname");
                            println!("    port = 3306");
                            println!("    user = username");
                            println!("    password = password");
                            println!("    database = database_name");

                            match get_mysql_config_path() {
                                Some(path) => {
                                    println!(
                                        "  Your MySQL config file location: {}",
                                        path.display()
                                    );
                                    if path.exists() {
                                        println!("  Status: ✅ File exists");
                                    } else {
                                        println!("  Status: ❌ File does not exist");
                                    }
                                }
                                None => {
                                    println!("  No MySQL configuration file found.");
                                    println!(
                                        "  Searched locations: ~/.my.cnf, /etc/mysql/my.cnf, /etc/my.cnf"
                                    );
                                }
                            }

                            println!(
                                "  MySQL also supports SSL options: ssl-ca, ssl-cert, ssl-key"
                            );
                            println!(
                                "  You can set the MYSQL_CONFIG environment variable to specify a different location."
                            );

                            // SQLite information
                            println!("\n🗃️ SQLite:");
                            println!(
                                "  SQLite databases are file-based and typically don't require passwords."
                            );
                            println!("  Connection is based on file path and permissions:");
                            println!("    sqlite:///absolute/path/to/database.db");
                            println!("    sqlite://./relative/path/to/database.db");

                            // General information
                            println!("\n🔧 General Usage:");
                            println!(
                                "  When connecting, dbcrust will automatically check the appropriate"
                            );
                            println!("  configuration file based on your database URL:");
                            println!("  • postgresql:// URLs use .pgpass file");
                            println!("  • mysql:// URLs use .my.cnf file");
                            println!("  • sqlite:// URLs use file system permissions");
                            println!(
                                "  You can provide an empty password to use automatic authentication."
                            );
                        }
                        "\\myconf" => {
                            println!("MySQL Configuration File (.my.cnf) Management:");
                            println!();

                            // Show current MySQL configuration file status
                            match get_mysql_config_path() {
                                Some(path) => {
                                    println!("📁 Configuration File Location: {}", path.display());
                                    if path.exists() {
                                        println!("   Status: ✅ File exists");

                                        // Try to read and display current configuration
                                        match std::fs::read_to_string(&path) {
                                            Ok(contents) => {
                                                println!();
                                                println!("📄 Current Configuration:");
                                                println!("{}", "─".repeat(50));
                                                for (line_num, line) in contents.lines().enumerate()
                                                {
                                                    let trimmed = line.trim();
                                                    if !trimmed.is_empty()
                                                        && !trimmed.starts_with('#')
                                                    {
                                                        println!("{:3}: {}", line_num + 1, line);
                                                    }
                                                }
                                                println!("{}", "─".repeat(50));
                                            }
                                            Err(e) => {
                                                println!(
                                                    "   ⚠️  Could not read file contents: {}",
                                                    e
                                                );
                                            }
                                        }
                                    } else {
                                        println!("   Status: ❌ File does not exist");
                                        println!();
                                        println!("💡 To create a .my.cnf file, you can:");
                                        println!("   1. Use the example below");
                                        println!("   2. Run 'mysql_config_editor set' command");
                                        println!("   3. Manually create the file");
                                    }
                                }
                                None => {
                                    println!("❌ No MySQL configuration file found");
                                    println!("   Searched locations:");
                                    println!("   • ~/.my.cnf");
                                    println!("   • /etc/mysql/my.cnf");
                                    println!("   • /etc/my.cnf");
                                    println!();
                                    println!("💡 You can create a configuration file at ~/.my.cnf");
                                }
                            }

                            println!();
                            println!("📝 Example .my.cnf configuration:");
                            println!("{}", "─".repeat(40));
                            println!("[client]");
                            println!("host = localhost");
                            println!("port = 3306");
                            println!("user = your_username");
                            println!("password = your_password");
                            println!("database = your_default_database");
                            println!();
                            println!("# Optional SSL settings");
                            println!("ssl-ca = /path/to/ca.pem");
                            println!("ssl-cert = /path/to/client-cert.pem");
                            println!("ssl-key = /path/to/client-key.pem");
                            println!("{}", "─".repeat(40));

                            println!();
                            println!("🔐 Security Notes:");
                            println!(
                                "   • File should have permissions 600 (readable only by owner)"
                            );
                            println!("   • Command: chmod 600 ~/.my.cnf");
                            println!("   • Keep passwords secure and avoid version control");

                            println!();
                            println!("🔧 Advanced Options:");
                            println!(
                                "   • Use MYSQL_CONFIG environment variable for custom location"
                            );
                            println!("   • Use mysql_config_editor for encrypted password storage");
                            println!("   • Multiple configuration files are supported");

                            // Check current database connection type
                            let db = db_arc.lock().unwrap();
                            if let Some(ref database_client) = db.get_database_client() {
                                let conn_info = database_client.get_connection_info();
                                if conn_info.database_type == dbcrust::database::DatabaseType::MySQL
                                {
                                    println!();
                                    println!("🔗 Current Connection (MySQL):");
                                    println!(
                                        "   Host: {}",
                                        conn_info.host.as_deref().unwrap_or("localhost")
                                    );
                                    println!("   Port: {}", conn_info.port.unwrap_or(3306));
                                    println!(
                                        "   User: {}",
                                        conn_info.username.as_deref().unwrap_or("unknown")
                                    );
                                    println!(
                                        "   Database: {}",
                                        conn_info.database.as_deref().unwrap_or("none")
                                    );

                                    // Check if current connection details match any .my.cnf entry
                                    if let Some(_found_password) = lookup_mysql_password(
                                        conn_info.host.as_deref().unwrap_or("localhost"),
                                        conn_info.port.unwrap_or(3306),
                                        conn_info.database.as_deref().unwrap_or(""),
                                        conn_info.username.as_deref().unwrap_or(""),
                                    ) {
                                        println!("   ✅ Password found in .my.cnf");
                                    } else {
                                        println!("   ❌ No matching password entry in .my.cnf");
                                    }
                                }
                            }
                        }
                        "\\pragma" => {
                            println!("SQLite Pragma Settings Management:");
                            println!();

                            // Check if we're currently connected to SQLite
                            let db = db_arc.lock().unwrap();
                            if let Some(ref database_client) = db.get_database_client() {
                                let conn_info = database_client.get_connection_info();
                                if conn_info.database_type
                                    == dbcrust::database::DatabaseType::SQLite
                                {
                                    println!("🔗 Current SQLite Connection:");
                                    println!(
                                        "   Database: {}",
                                        conn_info.file_path.as_deref().unwrap_or("unknown")
                                    );
                                    println!();

                                    // Show current pragma settings
                                    println!("📊 Current Pragma Settings:");
                                    println!("{}", "─".repeat(50));

                                    // Get some important pragma settings
                                    let pragmas_to_check = vec![
                                        ("journal_mode", "Database journaling mode"),
                                        ("synchronous", "Synchronous mode for writes"),
                                        ("foreign_keys", "Foreign key constraint checking"),
                                        ("temp_store", "Temporary storage location"),
                                        ("cache_size", "Page cache size"),
                                        ("mmap_size", "Memory-mapped I/O size"),
                                        ("auto_vacuum", "Automatic vacuum mode"),
                                        ("page_size", "Database page size"),
                                        ("encoding", "Database text encoding"),
                                        ("user_version", "User-defined version number"),
                                    ];

                                    // This would need to be implemented as async, but for now show the template
                                    drop(db); // Release the lock

                                    for (pragma, description) in pragmas_to_check {
                                        println!("   {}: {}", pragma, description);
                                        // TODO: Add actual pragma value query when implementing async support
                                        println!(
                                            "      Current value: <requires async implementation>"
                                        );
                                    }
                                    println!("{}", "─".repeat(50));
                                } else {
                                    println!("❌ Not connected to SQLite database");
                                    println!(
                                        "   Current connection: {:?}",
                                        conn_info.database_type
                                    );
                                }
                            } else {
                                println!("❌ No active database connection");
                            }

                            println!();
                            println!("📝 Common SQLite Pragma Commands:");
                            println!("{}", "─".repeat(40));
                            println!("Performance Optimization:");
                            println!("  PRAGMA journal_mode = WAL;         -- Enable WAL mode");
                            println!("  PRAGMA synchronous = NORMAL;       -- Balanced sync mode");
                            println!("  PRAGMA cache_size = 10000;         -- Set cache size");
                            println!(
                                "  PRAGMA mmap_size = 268435456;       -- Enable memory mapping"
                            );
                            println!(
                                "  PRAGMA temp_store = MEMORY;         -- Use memory for temp"
                            );
                            println!();
                            println!("Data Integrity:");
                            println!(
                                "  PRAGMA foreign_keys = ON;          -- Enable FK constraints"
                            );
                            println!(
                                "  PRAGMA integrity_check;            -- Check database integrity"
                            );
                            println!(
                                "  PRAGMA quick_check;                -- Quick integrity check"
                            );
                            println!();
                            println!("Database Maintenance:");
                            println!("  PRAGMA auto_vacuum = INCREMENTAL;  -- Enable auto vacuum");
                            println!("  PRAGMA optimize;                   -- Optimize database");
                            println!("  PRAGMA vacuum;                     -- Reclaim space");
                            println!("{}", "─".repeat(40));

                            println!();
                            println!("🔧 Usage Examples:");
                            println!(
                                "   Query current setting: SELECT * FROM pragma_journal_mode;"
                            );
                            println!("   Set new value: PRAGMA journal_mode = WAL;");
                            println!("   Reset to default: PRAGMA journal_mode = DELETE;");

                            println!();
                            println!("⚠️  Important Notes:");
                            println!("   • Some pragmas require database restart to take effect");
                            println!(
                                "   • WAL mode improves concurrency but requires more disk space"
                            );
                            println!("   • Always backup before changing critical settings");
                            println!("   • Some pragmas are read-only and cannot be changed");

                            println!();
                            println!("📚 Reference:");
                            println!("   For complete pragma documentation, visit:");
                            println!("   https://www.sqlite.org/pragma.html");
                        }
                        "\\n" => {
                            // List all named queries
                            let queries = config.list_named_queries();
                            if queries.is_empty() {
                                println!("No named queries found.");
                            } else {
                                let mut table = Table::new();
                                table
                                    .add_row(Row::new(vec![Cell::new("Name"), Cell::new("Query")]));

                                for (name, query) in queries {
                                    table.add_row(Row::new(vec![
                                        Cell::new(&name),
                                        Cell::new(&query),
                                    ]));
                                }

                                table.printstd();
                            }
                        }
                        cmd if cmd.starts_with("\\n ") => {
                            // Execute a named query
                            let parts: Vec<&str> = cmd[3..].trim().split_whitespace().collect();
                            if parts.is_empty() {
                                println!("Error: missing query name");
                                continue;
                            }

                            let name = parts[0];
                            let args = &parts[1..];

                            match config.get_named_query(name) {
                                Some(query) => {
                                    // Process the query with parameter substitution
                                    let processed_query = named_queries::process_query(query, args);
                                    println!("Executing named query: {}", name);

                                    // Execute the processed query
                                    let mut db = db_arc.lock().unwrap();
                                    match db.execute_query(&processed_query).await {
                                        Ok(results) => {
                                            if results.is_empty() {
                                                println!("Query OK, no results.");
                                            } else {
                                                // Check if we should auto-enable column selection based on column count
                                                let column_count = results[0].len();
                                                let auto_enable = db
                                                    .should_auto_enable_column_selection(
                                                        column_count,
                                                    );

                                                // Process results with column selection if enabled or auto-enabled
                                                let processed_results = if db
                                                    .is_column_select_mode()
                                                    || auto_enable
                                                {
                                                    // Reset the interrupt flag before column selection
                                                    interrupt_flag.store(false, Ordering::SeqCst);

                                                    // If auto-enabled, show a more informative message
                                                    if auto_enable && !db.is_column_select_mode() {
                                                        println!(
                                                            "Auto-enabling column selection mode due to high column count ({} columns exceeds threshold of {})",
                                                            column_count,
                                                            db.get_column_selection_threshold()
                                                        );
                                                        println!(
                                                            "This threshold can be configured with \\csthreshold command"
                                                        );
                                                    } else {
                                                        println!(
                                                            "Entering column selection mode..."
                                                        );
                                                    }

                                                    // Interactive column selection
                                                    match db.interactive_column_selection(
                                                        &results,
                                                        &interrupt_flag,
                                                    ) {
                                                        Ok(filtered) => {
                                                            if !filtered.is_empty()
                                                                && !results.is_empty()
                                                            {
                                                                println!(
                                                                    "Column selection: filtered data has {} rows, {} columns (original: {} rows, {} columns)",
                                                                    filtered.len(),
                                                                    filtered[0].len(),
                                                                    results.len(),
                                                                    results[0].len()
                                                                );
                                                            }
                                                            filtered
                                                        }
                                                        Err(e) => {
                                                            eprintln!(
                                                                "Error during column selection: {}",
                                                                e
                                                            );
                                                            results
                                                        }
                                                    }
                                                } else {
                                                    results
                                                };

                                                if db.is_expanded_display() {
                                                    let expanded_tables =
                                                        format_query_results_expanded(
                                                            &processed_results,
                                                        );
                                                    let mut output_buffer = String::new();
                                                    for table in expanded_tables {
                                                        output_buffer.push_str(&table.to_string());
                                                        output_buffer.push_str("\n");
                                                    }
                                                    if db.is_explain_mode() {
                                                        handle_explain_output(&output_buffer, &config);
                                                    } else {
                                                        handle_output(&output_buffer, &config);
                                                    }
                                                } else {
                                                    // Use psql-style formatting
                                                    let output = format_query_results_psql(
                                                        &processed_results,
                                                    );
                                                    if db.is_explain_mode() {
                                                        handle_explain_output(&output, &config);
                                                    } else {
                                                        handle_output(&output, &config);
                                                    }
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            eprintln!("Error: {}", err);
                                        }
                                    }
                                }
                                None => println!("No named query found with name: {}", name),
                            }
                        }
                        cmd if cmd.starts_with("\\ns ") => {
                            // Save a named query
                            let cmd_str = cmd[4..].trim();

                            // Find the first whitespace after the name
                            if let Some(space_pos) = cmd_str.find(char::is_whitespace) {
                                let name = &cmd_str[..space_pos];
                                let query = &cmd_str[space_pos..].trim();

                                if name.is_empty() || query.is_empty() {
                                    println!("Error: name and query must not be empty");
                                    continue;
                                }

                                // Save the query without blocking validation
                                match config.add_named_query(name, query) {
                                    Ok(_) => {
                                        println!("Saved query: {}", name);

                                        // Create a new completer with refreshed config
                                        // Only update if autocomplete is enabled
                                        if db_arc.lock().unwrap().is_autocomplete() {
                                            let new_completer: Box<dyn Completer> =
                                                Box::new(SqlCompleter::new(db_arc.clone()));
                                            line_editor = line_editor.with_completer(new_completer);
                                        }

                                        // Optional validation (non-blocking)
                                        let mut db = db_arc.lock().unwrap();
                                        if let Err(e) = db.validate_query(query).await {
                                            println!(
                                                "Warning: Query may have syntax issues: {}",
                                                e
                                            );
                                            println!(
                                                "Query was saved anyway. Check syntax before executing."
                                            );
                                        }
                                    }
                                    Err(e) => eprintln!("Error saving query: {}", e),
                                }
                            } else {
                                println!("Error: missing query. Format is \\ns <n> <query>");
                            }
                        }
                        cmd if cmd.starts_with("\\nd ") => {
                            // Delete a named query
                            let name = cmd[4..].trim();

                            if name.is_empty() {
                                println!("Error: missing query name");
                                continue;
                            }

                            match config.delete_named_query(name) {
                                Ok(true) => {
                                    println!("{}: Deleted", name);

                                    // Create a new completer with refreshed config
                                    // Only update if autocomplete is enabled
                                    if db_arc.lock().unwrap().is_autocomplete() {
                                        let new_completer: Box<dyn Completer> =
                                            Box::new(SqlCompleter::new(db_arc.clone()));
                                        line_editor = line_editor.with_completer(new_completer);
                                    }
                                }
                                Ok(false) => println!("No query named '{}'", name),
                                Err(e) => eprintln!("Error deleting query: {}", e),
                            }
                        }
                        cmd if cmd.starts_with("\\c ") => {
                            let dbname = cmd[3..].trim();
                            let mut db = db_arc.lock().unwrap();
                            match db.connect_to_db(dbname).await {
                                Ok(_) => {
                                    println!("Connected to database {}", dbname);

                                    // Update prompt with new database name
                                    let username = db.get_username().to_string();
                                    let db_name = db.get_current_db();
                                    drop(db); // Release the lock before recreating prompt

                                    // Create new prompt with updated info
                                    let new_prompt = DbPrompt::with_config(username, db_name, config.multiline_prompt_indicator.clone());
                                    prompt = new_prompt;
                                }
                                Err(_) => eprintln!("Failed to connect to database {}", dbname),
                            }
                        }
                        cmd if cmd.starts_with("\\d ") => {
                            let table_name = cmd[3..].trim();
                            let mut db = db_arc.lock().unwrap();
                            match db.get_table_details(table_name).await {
                                Ok(details) => {
                                    let output_content = format_table_details(&details);
                                    handle_output(&output_content, &config);
                                }
                                Err(e) => eprintln!("Error: {}", e),
                            }
                        }
                        "\\s" => {
                            // List all saved sessions
                            let sessions = config.list_sessions();
                            if sessions.is_empty() {
                                println!("No saved sessions found.");
                            } else {
                                let mut table = Table::new();
                                table.add_row(Row::new(vec![
                                    Cell::new("Name"),
                                    Cell::new("Type"),
                                    Cell::new("Host"),
                                    Cell::new("Port"),
                                    Cell::new("User"),
                                    Cell::new("Database"),
                                ]));

                                for (name, session) in sessions {
                                    let host_display = if session.database_type
                                        == dbcrust::database::DatabaseType::SQLite
                                    {
                                        session.file_path.as_deref().unwrap_or("(memory)")
                                    } else {
                                        &session.host
                                    };

                                    table.add_row(Row::new(vec![
                                        Cell::new(&name),
                                        Cell::new(&session.database_type.to_string()),
                                        Cell::new(host_display),
                                        Cell::new(&session.port.to_string()),
                                        Cell::new(&session.user),
                                        Cell::new(&session.dbname),
                                    ]));
                                }

                                table.printstd();
                            }
                        }
                        cmd if cmd.starts_with("\\s ") => {
                            // Connect to a saved session
                            let session_name = cmd[3..].trim();

                            match config.get_session(session_name) {
                                Some(session) => {
                                    // Build connection URL based on database type
                                    let connection_url = match session.database_type {
                                        dbcrust::database::DatabaseType::PostgreSQL => {
                                            format!(
                                                "postgresql://{}@{}:{}/{}",
                                                session.user,
                                                session.host,
                                                session.port,
                                                session.dbname
                                            )
                                        }
                                        dbcrust::database::DatabaseType::MySQL => {
                                            format!(
                                                "mysql://{}@{}:{}/{}",
                                                session.user,
                                                session.host,
                                                session.port,
                                                session.dbname
                                            )
                                        }
                                        dbcrust::database::DatabaseType::SQLite => {
                                            if let Some(ref file_path) = session.file_path {
                                                format!("sqlite:///{}", file_path)
                                            } else {
                                                "sqlite:///:memory:".to_string()
                                            }
                                        }
                                    };

                                    // Create new database connection using the abstraction layer
                                    match DbCrustDatabase::from_url(
                                        &connection_url,
                                        Some(config.default_limit),
                                        Some(config.expanded_display_default),
                                    )
                                    .await
                                    {
                                        Ok(db) => {
                                            let mut current_db = db_arc.lock().unwrap();
                                            *current_db = db;
                                            println!(
                                                "Connected to {} session '{}'",
                                                session.database_type, session_name
                                            );

                                            // Update prompt with new database and username
                                            let username = current_db.get_username().to_string();
                                            let db_name = current_db.get_current_db();
                                            drop(current_db); // Release the lock

                                            // Create new prompt with updated info
                                            let new_prompt = DbPrompt::with_config(username, db_name, config.multiline_prompt_indicator.clone());
                                            prompt = new_prompt;

                                            // Refresh completer if autocomplete is enabled
                                            let current_db = db_arc.lock().unwrap();
                                            if current_db.is_autocomplete() {
                                                let new_completer: Box<dyn Completer> =
                                                    Box::new(SqlCompleter::new(db_arc.clone()));
                                                line_editor =
                                                    line_editor.with_completer(new_completer);
                                            }
                                        }
                                        Err(e) => eprintln!(
                                            "Error connecting to {} session '{}': {}",
                                            session.database_type, session_name, e
                                        ),
                                    }
                                }
                                None => println!("No session found with name: {}", session_name),
                            }
                        }
                        cmd if cmd.starts_with("\\ss ") => {
                            // Save current connection as a named session
                            let session_name = cmd[4..].trim();

                            if session_name.is_empty() {
                                println!("Error: session name must not be empty");
                                continue;
                            }

                            // Get connection information from the current database
                            let db = db_arc.lock().unwrap();
                            let (database_type, file_path, options) =
                                if let Some(database_client) = db.get_database_client() {
                                    let connection_info = database_client.get_connection_info();
                                    (
                                        connection_info.database_type.clone(),
                                        connection_info.file_path.clone(),
                                        connection_info.options.clone(),
                                    )
                                } else {
                                    // Fallback to PostgreSQL for legacy connections
                                    (
                                        dbcrust::database::DatabaseType::PostgreSQL,
                                        None,
                                        std::collections::HashMap::new(),
                                    )
                                };
                            drop(db);

                            // Create a temporary clone of config for session saving
                            let mut temp_config = config.clone();

                            match temp_config.save_session_with_db_type(
                                session_name,
                                database_type.clone(),
                                file_path.clone(),
                                options,
                            ) {
                                Ok(_) => {
                                    config.saved_sessions = temp_config.saved_sessions;
                                    match config.save() {
                                        Ok(_) => {
                                            let connection_display = match database_type {
                                                dbcrust::database::DatabaseType::SQLite => {
                                                    format!(
                                                        "SQLite: {}",
                                                        file_path.as_deref().unwrap_or("(memory)")
                                                    )
                                                }
                                                _ => {
                                                    format!(
                                                        "{}:{}/{}",
                                                        config.connection.host,
                                                        config.connection.port,
                                                        config.connection.dbname
                                                    )
                                                }
                                            };
                                            println!(
                                                "{} session '{}' saved successfully (connection: {})",
                                                database_type, session_name, connection_display
                                            );
                                        }
                                        Err(e) => eprintln!(
                                            "Error saving current configuration with new session: {}",
                                            e
                                        ),
                                    }

                                    // Only offer to save password for PostgreSQL and MySQL
                                    if database_type != dbcrust::database::DatabaseType::SQLite {
                                        println!(
                                            "Save password to database configuration file for this session's connection details? (y/n) [default: n]"
                                        );
                                        let mut save_pass_input = String::new();
                                        std::io::stdin().read_line(&mut save_pass_input)?;
                                        let save_pass_input = save_pass_input.trim().to_lowercase();

                                        if save_pass_input == "y" || save_pass_input == "yes" {
                                            match database_type {
                                                dbcrust::database::DatabaseType::PostgreSQL => {
                                                    match pgpass::save_password(
                                                        &config.connection.host,
                                                        config.connection.port,
                                                        &config.connection.dbname,
                                                        &config.connection.user,
                                                        &db_password_final, // Use the resolved password for the current connection
                                                    ) {
                                                        Ok(_) => println!(
                                                            "Password saved to .pgpass file for session parameters"
                                                        ),
                                                        Err(e) => {
                                                            eprintln!(
                                                                "Error saving password to .pgpass: {}",
                                                                e
                                                            )
                                                        }
                                                    }
                                                }
                                                dbcrust::database::DatabaseType::MySQL => {
                                                    println!(
                                                        "MySQL password management via .my.cnf is not yet implemented for session saving."
                                                    );
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                Err(e) => eprintln!(
                                    "Error staging session '{}' for saving: {}",
                                    session_name, e
                                ),
                            }
                        }
                        cmd if cmd.starts_with("\\sd ") => {
                            // Delete a saved session
                            let session_name = cmd[4..].trim();

                            if session_name.is_empty() {
                                println!("Error: session name must not be empty");
                                continue;
                            }

                            match config.delete_session(session_name) {
                                Ok(true) => println!("{}: Deleted", session_name),
                                Ok(false) => println!("No session named '{}'", session_name),
                                Err(e) => eprintln!("Error deleting session: {}", e),
                            }
                        }
                        cmd if cmd.starts_with("\\csthreshold ") => {
                            let threshold_str = cmd[13..].trim();

                            match threshold_str.parse::<usize>() {
                                Ok(threshold) => {
                                    // Update config with the new setting
                                    config.column_selection_threshold = threshold;

                                    // Update the Database instance too
                                    let mut db = db_arc.lock().unwrap();
                                    db.set_column_selection_threshold(threshold);
                                    drop(db);

                                    // Save config
                                    match config.save() {
                                        Ok(_) => println!(
                                            "Column selection auto-enable threshold set to {} columns",
                                            threshold
                                        ),
                                        Err(e) => eprintln!("Error saving configuration: {}", e),
                                    }
                                }
                                Err(_) => {
                                    println!(
                                        "Error: invalid threshold value. Please enter a positive number."
                                    );
                                }
                            }
                        }
                        "\\copy" => {
                            let db = db_arc.lock().unwrap();
                            match db.get_last_json_plan() {
                                Some(json_plan) => {
                                    match Clipboard::new() {
                                        Ok(mut clipboard) => {
                                            match clipboard.set_text(&json_plan) {
                                                Ok(_) => println!("✓ JSON plan copied to clipboard ({} characters)", json_plan.len()),
                                                Err(e) => eprintln!("Error copying to clipboard: {}", e),
                                            }
                                        }
                                        Err(e) => eprintln!("Error accessing clipboard: {}", e),
                                    }
                                }
                                None => {
                                    println!("No JSON plan available to copy. Run an EXPLAIN query first.");
                                }
                            }
                        }
                        "\\docker" => {
                            // List Docker database containers
                            match dbcrust::docker::DockerClient::new() {
                                Ok(docker_client) => {
                                    match docker_client.list_database_containers().await {
                                        Ok(containers) => {
                                            if containers.is_empty() {
                                                println!("No database containers found.");
                                                println!("Make sure you have database containers running.");
                                                println!("Example: docker run -d --name postgres-db -p 5432:5432 -e POSTGRES_PASSWORD=password postgres");
                                            } else {
                                                println!("\n🐳 Database Containers:");
                                                println!("{:<20} {:<15} {:<12} {:<15} {:<10}", "NAME", "DATABASE", "STATUS", "IMAGE", "PORTS");
                                                println!("{}", "-".repeat(80));
                                                
                                                for container in containers {
                                                    let db_type = container.database_type
                                                        .as_ref()
                                                        .map(|dt| format!("{}", dt))
                                                        .unwrap_or("Unknown".to_string());
                                                    
                                                    let status_icon = if container.status.contains("running") || container.status.contains("Up") {
                                                        "🟢"
                                                    } else {
                                                        "🔴"
                                                    };
                                                    
                                                    let ports = if let Some(host_port) = container.host_port {
                                                        if let Some(container_port) = container.container_port {
                                                            format!("{}:{}", host_port, container_port)
                                                        } else {
                                                            format!("{}", host_port)
                                                        }
                                                    } else {
                                                        "none".to_string()
                                                    };
                                                    
                                                    let status_with_icon = format!("{} {}", status_icon, 
                                                        if container.status.len() > 10 { 
                                                            &container.status[..10] 
                                                        } else { 
                                                            &container.status 
                                                        });
                                                    
                                                    let image_short = if container.image.len() > 12 {
                                                        format!("{}...", &container.image[..9])
                                                    } else {
                                                        container.image.clone()
                                                    };
                                                    
                                                    println!("{:<20} {:<15} {:<12} {:<15} {:<10}", 
                                                        container.name, db_type, status_with_icon, image_short, ports);
                                                    
                                                    // Show connection example for running containers
                                                    if container.status.contains("running") || container.status.contains("Up") {
                                                        if let Some(_) = container.host_port {
                                                            println!("  └─ Connect: docker://{}", container.name);
                                                        } else {
                                                            // Check if OrbStack domain is available
                                                            if let Some(database_type) = &container.database_type {
                                                                if let Ok(Some((orbstack_host, _port))) = docker_client.get_orbstack_domain(&container, database_type) {
                                                                    println!("  └─ Connect: docker://{} (OrbStack: {})", container.name, orbstack_host);
                                                                } else {
                                                                    println!("  └─ Connect: docker://{} (no exposed port)", container.name);
                                                                }
                                                            } else {
                                                                println!("  └─ Connect: (no exposed port)");
                                                            }
                                                        }
                                                    }
                                                }
                                                
                                                println!("\nUsage:");
                                                println!("  dbcrust docker://container_name");
                                                println!("  dbcrust docker://user:pass@container_name/database");
                                                println!("  dbcrust --docker-container container_name");
                                                println!("\nNote: Containers without exposed ports can use OrbStack domains automatically");
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("Error listing Docker containers: {}", e);
                                            eprintln!("Make sure Docker is running and accessible.");
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Error connecting to Docker: {}", e);
                                    eprintln!("Make sure Docker daemon is running.");
                                }
                            }
                        }
                        _ => println!("Unknown command. Type \\h for help."),
                    }
                    continue;
                }

                // Execute SQL query
                let mut db = db_arc.lock().unwrap();
                match db.execute_query(input_trimmed).await {
                    Ok(results) => {
                        // Store the query for potential saving
                        last_script = input_trimmed.to_string();

                        if results.is_empty() {
                            println!("Query OK, no results.");
                        } else {
                            // Check if we should auto-enable column selection based on column count
                            let column_count = results[0].len();
                            let auto_enable = db.should_auto_enable_column_selection(column_count);

                            // Process results with column selection if enabled or auto-enabled
                            let processed_results = if db.is_column_select_mode() || auto_enable {
                                // Reset the interrupt flag before column selection
                                interrupt_flag.store(false, Ordering::SeqCst);

                                // If auto-enabled, show a more informative message
                                if auto_enable && !db.is_column_select_mode() {
                                    println!(
                                        "Auto-enabling column selection mode due to high column count ({} columns exceeds threshold of {})",
                                        column_count,
                                        db.get_column_selection_threshold()
                                    );
                                    println!(
                                        "This threshold can be configured with \\csthreshold command"
                                    );
                                } else {
                                    println!("Entering column selection mode...");
                                }

                                // Interactive column selection
                                match db.interactive_column_selection(&results, &interrupt_flag) {
                                    Ok(filtered) => {
                                        if !filtered.is_empty() && !results.is_empty() {
                                            println!(
                                                "Column selection: filtered data has {} rows, {} columns (original: {} rows, {} columns)",
                                                filtered.len(),
                                                filtered[0].len(),
                                                results.len(),
                                                results[0].len()
                                            );
                                        }
                                        filtered
                                    }
                                    Err(e) => {
                                        eprintln!("Error during column selection: {}", e);
                                        results
                                    }
                                }
                            } else {
                                results
                            };

                            // Format and display the results
                            if db.is_expanded_display() {
                                let expanded_tables =
                                    format_query_results_expanded(&processed_results);
                                let mut output_buffer = String::new();
                                for table in expanded_tables {
                                    output_buffer.push_str(&table.to_string());
                                    output_buffer.push_str("\n");
                                }
                                if db.is_explain_mode() {
                                    handle_explain_output(&output_buffer, &config);
                                } else {
                                    handle_output(&output_buffer, &config);
                                }
                            } else {
                                // Use psql-style formatting
                                let output = format_query_results_psql(&processed_results);
                                if db.is_explain_mode() {
                                    handle_explain_output(&output, &config);
                                } else {
                                    handle_output(&output, &config);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("Error: {}", err);
                    }
                }
            }
            Signal::CtrlC => {
                // Check if we're in the column selection mode
                if interrupt_flag.load(Ordering::SeqCst) {
                    // Just reset the flag - the read operation in column selection will fail
                    // which will be handled gracefully
                    interrupt_flag.store(false, Ordering::SeqCst);
                    println!("\nColumn selection cancelled, showing all columns");
                } else {
                    // Normal Ctrl-C behavior
                    println!(); // Just print a newline to clear the current line
                }
                continue;
            }
            Signal::CtrlD => {
                println!("\nExiting.");
                break;
            }
        }
    }

    // Explicitly close the database connection and tunnel
    match std::sync::Arc::try_unwrap(db_arc) {
        Ok(mutex) => {
            match mutex.into_inner() {
                Ok(mut db_instance) => {
                    // Successfully got ownership of the Database instance
                    debug_log!("Shutting down database connection and SSH tunnel...");
                    db_instance.close().await;
                    debug_log!("Database connection and SSH tunnel shut down.");
                }
                Err(poison_err) => {
                    eprintln!(
                        "Mutex was poisoned. Attempting to close resources anyway: {}",
                        poison_err
                    );
                    let mut db_guard = poison_err.into_inner();
                    db_guard.close().await;
                }
            }
        }
        Err(arc_still_shared) => {
            // This typically means db_arc was cloned and the clones are still alive.
            // This could be a logic error elsewhere if exclusive ownership was expected here.
            match arc_still_shared.lock() {
                Ok(mut db_guard) => {
                    db_guard.close().await;
                }
                Err(poison_err) => {
                    eprintln!(
                        "Failed to lock Arc-shared database for closing (poisoned): {}",
                        poison_err
                    );
                    // At this point, can't do much more if the lock is poisoned and it's shared.
                }
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


/// Run the interactive mode for any database type
async fn run_interactive_mode(
    database: Database,
    config: DbCrustConfig,
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
    println!("Type \\h for help");

    let interrupt_flag = Arc::new(AtomicBool::new(false));
    // Register a signal handler for SIGINT (Ctrl-C)
    flag::register(SIGINT, Arc::clone(&interrupt_flag))?;

    // Keep track of the last executed query or edited script
    let mut last_script = String::new();

    loop {
        match line_editor.read_line(&prompt)? {
            Signal::Success(input) => {
                let input_trimmed = input.trim();

                if input_trimmed.is_empty() {
                    continue;
                }

                // Handle special commands
                if input_trimmed.starts_with('\\') {
                    if handle_backslash_command(
                        input_trimmed,
                        &db_arc,
                        &mut last_script,
                        &interrupt_flag,
                        &mut prompt,
                    )
                    .await?
                    {
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
                                process_query_results(&mut db, results, &interrupt_flag).await?;
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

    Ok(())
}

/// Handle backslash commands, returns true if should exit
async fn handle_backslash_command(
    input: &str,
    db_arc: &Arc<Mutex<Database>>,
    last_script: &mut String,
    interrupt_flag: &Arc<AtomicBool>,
    prompt: &mut DbPrompt,
) -> Result<bool, Box<dyn StdError>> {
    match input {
        "\\q" => return Ok(true), // Signal to exit
        "\\h" => print_help(&DbCrustConfig::load()),
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
                    println!("💡 Use \\copy to copy the raw JSON plan to clipboard");
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
                Ok(results) => {
                    let config = DbCrustConfig::load();
                    let formatted_output = format_query_results_psql(&results);
                    handle_explain_output(&formatted_output, &config);
                }
                Err(e) => eprintln!("Error executing formatted EXPLAIN: {}", e),
            }
        }
        cmd if cmd.starts_with("\\ex ") => {
            // Execute EXPLAIN and export to file
            let parts: Vec<&str> = cmd[4..].splitn(2, ' ').collect();
            if parts.len() < 2 {
                println!("Error: Please provide a query and filename after \\ex");
                println!("Usage: \\ex <query> <filename>");
                return Ok(false);
            }

            let query = parts[0].trim();
            let filename = parts[1].trim();

            if query.is_empty() || filename.is_empty() {
                println!("Error: Both query and filename must be provided");
                return Ok(false);
            }

            let mut db = db_arc.lock().unwrap();
            match db.execute_explain_query_raw(query).await {
                Ok(results) => {
                    let formatted_output = format_query_results_psql(&results);
                    match std::fs::write(filename, formatted_output) {
                        Ok(_) => println!("EXPLAIN output exported to: {}", filename),
                        Err(e) => eprintln!("Error writing to file '{}': {}", filename, e),
                    }
                }
                Err(e) => eprintln!("Error executing EXPLAIN for export: {}", e),
            }
        }
        "\\ed" => {
            // Handle multiline editor (abbreviated for space)
            println!("Entering multiline edit mode...");
            if !last_script.is_empty() {
                println!("Editing existing script ({} bytes):", last_script.len());
            }

            match edit_multiline_script(last_script) {
                Ok(script) => {
                    if !script.is_empty() {
                        *last_script = script.clone();
                        println!("Execute script? (y/n) [default: y]");
                        let mut confirm = String::new();
                        std::io::stdin().read_line(&mut confirm)?;
                        let confirm = confirm.trim().to_lowercase();

                        if confirm.is_empty() || confirm == "y" || confirm == "yes" {
                            let mut db = db_arc.lock().unwrap();
                            match db.execute_query(&script).await {
                                Ok(results) => {
                                    if !results.is_empty() {
                                        process_query_results(&mut db, results, interrupt_flag)
                                            .await?;
                                    } else {
                                        println!("Query OK, no results.");
                                    }
                                }
                                Err(e) => eprintln!("Error executing script: {}", e),
                            }
                        }
                    }
                }
                Err(e) => eprintln!("Error in editor: {}", e),
            }
        }
        cmd if cmd.starts_with("\\dt") => {
            let mut db = db_arc.lock().unwrap();
            match db.list_tables().await {
                Ok(results) => {
                    let output = format_query_results_psql(&results);
                    print!("{}", output);
                }
                Err(e) => eprintln!("Error listing tables: {}", e),
            }
        }
        cmd if cmd.starts_with("\\l") => {
            let mut db = db_arc.lock().unwrap();
            match db.list_databases().await {
                Ok(results) => {
                    let output = format_query_results_psql(&results);
                    print!("{}", output);
                }
                Err(e) => eprintln!("Error listing databases: {}", e),
            }
        }
        cmd if cmd.starts_with("\\du") => {
            let mut db = db_arc.lock().unwrap();
            match db.list_users().await {
                Ok(results) => {
                    let output = format_query_results_psql(&results);
                    print!("{}", output);
                }
                Err(e) => eprintln!("Error listing users: {}", e),
            }
        }
        cmd if cmd.starts_with("\\di") => {
            let mut db = db_arc.lock().unwrap();
            match db.list_indexes().await {
                Ok(results) => {
                    let output = format_query_results_psql(&results);
                    print!("{}", output);
                }
                Err(e) => eprintln!("Error listing indexes: {}", e),
            }
        }
        cmd if cmd.starts_with("\\dp") => {
            let mut db = db_arc.lock().unwrap();
            match db.list_pragmas().await {
                Ok(results) => {
                    let output = format_query_results_psql(&results);
                    print!("{}", output);
                }
                Err(e) => eprintln!("Error listing pragmas: {}", e),
            }
        }
        cmd if cmd.starts_with("\\d ") => {
            let table_name = cmd.strip_prefix("\\d ").unwrap().trim();
            let mut db = db_arc.lock().unwrap();
            match db.get_table_details(table_name).await {
                Ok(details) => {
                    let output = format_table_details(&details);
                    print!("{}", output);
                }
                Err(e) => eprintln!("Error getting table details: {}", e),
            }
        }
        cmd if cmd.starts_with("\\c ") => {
            let db_name = cmd.strip_prefix("\\c ").unwrap().trim();
            let mut db = db_arc.lock().unwrap();
            match db.connect_to_db(db_name).await {
                Ok(_) => {
                    let new_db_name = db.get_current_db();
                    prompt.update_database(&new_db_name);
                    println!("Connected to database: {}", new_db_name);
                }
                Err(e) => eprintln!("Error connecting to database: {}", e),
            }
        }
        "\\pgpass" => {
            println!("Information about database password files:");

            // PostgreSQL .pgpass file information
            println!("\n📁 PostgreSQL (.pgpass file):");
            println!("  .pgpass file is used to store PostgreSQL database passwords.");
            println!("  It is a text file with the following format:");
            println!("    hostname:port:database:username:password");

            match pgpass::get_pgpass_path() {
                Some(path) => {
                    println!("  Your .pgpass file location: {}", path.display());
                    if path.exists() {
                        println!("  Status: ✅ File exists");
                    } else {
                        println!("  Status: ❌ File does not exist");
                    }
                }
                None => println!("  Could not determine .pgpass file location"),
            }

            println!("  Each field can contain * as a wildcard.");
            println!(
                "  On Unix systems, the file should have permissions 0600 (readable/writable only by owner)."
            );
            println!("  You can set permissions with: chmod 0600 ~/.pgpass");
            println!(
                "  You can also set the PGPASSFILE environment variable to specify a different location."
            );

            // MySQL .my.cnf file information
            println!("\n🐬 MySQL (.my.cnf file):");
            println!("  .my.cnf file is used to store MySQL connection options and passwords.");
            println!("  It is an INI-style configuration file with sections:");
            println!("    [client]");
            println!("    host = hostname");
            println!("    port = 3306");
            println!("    user = username");
            println!("    password = password");
            println!("    database = database_name");

            match get_mysql_config_path() {
                Some(path) => {
                    println!("  Your MySQL config file location: {}", path.display());
                    if path.exists() {
                        println!("  Status: ✅ File exists");
                    } else {
                        println!("  Status: ❌ File does not exist");
                    }
                }
                None => {
                    println!("  No MySQL configuration file found.");
                    println!("  Searched locations: ~/.my.cnf, /etc/mysql/my.cnf, /etc/my.cnf");
                }
            }

            println!("  MySQL also supports SSL options: ssl-ca, ssl-cert, ssl-key");
            println!(
                "  You can set the MYSQL_CONFIG environment variable to specify a different location."
            );

            // SQLite information
            println!("\n🗃️ SQLite:");
            println!("  SQLite databases are file-based and typically don't require passwords.");
            println!("  Connection is based on file path and permissions:");
            println!("    sqlite:///absolute/path/to/database.db");
            println!("    sqlite://./relative/path/to/database.db");

            // General information
            println!("\n🔧 General Usage:");
            println!("  When connecting, dbcrust will automatically check the appropriate");
            println!("  configuration file based on your database URL:");
            println!("  • postgresql:// URLs use .pgpass file");
            println!("  • mysql:// URLs use .my.cnf file");
            println!("  • sqlite:// URLs use file system permissions");
            println!("  You can provide an empty password to use automatic authentication.");
        }
        "\\myconf" => {
            println!("MySQL Configuration File (.my.cnf) Management:");
            println!();

            // Show current MySQL configuration file status
            match get_mysql_config_path() {
                Some(path) => {
                    println!("📁 Configuration File Location: {}", path.display());
                    if path.exists() {
                        println!("   Status: ✅ File exists");

                        // Try to read and display current configuration
                        match std::fs::read_to_string(&path) {
                            Ok(contents) => {
                                println!();
                                println!("📄 Current Configuration:");
                                println!("{}", "─".repeat(50));
                                for (line_num, line) in contents.lines().enumerate() {
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                                        println!("{:3}: {}", line_num + 1, line);
                                    }
                                }
                                println!("{}", "─".repeat(50));
                            }
                            Err(e) => {
                                println!("   ⚠️  Could not read file contents: {}", e);
                            }
                        }
                    } else {
                        println!("   Status: ❌ File does not exist");
                        println!();
                        println!("💡 To create a .my.cnf file, you can:");
                        println!("   1. Use the example below");
                        println!("   2. Run 'mysql_config_editor set' command");
                        println!("   3. Manually create the file");
                    }
                }
                None => {
                    println!("❌ No MySQL configuration file found");
                    println!("   Searched locations:");
                    println!("   • ~/.my.cnf");
                    println!("   • /etc/mysql/my.cnf");
                    println!("   • /etc/my.cnf");
                    println!();
                    println!("💡 You can create a configuration file at ~/.my.cnf");
                }
            }

            println!();
            println!("📝 Example .my.cnf configuration:");
            println!("{}", "─".repeat(40));
            println!("[client]");
            println!("host = localhost");
            println!("port = 3306");
            println!("user = your_username");
            println!("password = your_password");
            println!("database = your_default_database");
            println!();
            println!("# Optional SSL settings");
            println!("ssl-ca = /path/to/ca.pem");
            println!("ssl-cert = /path/to/client-cert.pem");
            println!("ssl-key = /path/to/client-key.pem");
            println!("{}", "─".repeat(40));

            println!();
            println!("🔐 Security Notes:");
            println!("   • File should have permissions 600 (readable only by owner)");
            println!("   • Command: chmod 600 ~/.my.cnf");
            println!("   • Keep passwords secure and avoid version control");

            println!();
            println!("🔧 Advanced Options:");
            println!("   • Use MYSQL_CONFIG environment variable for custom location");
            println!("   • Use mysql_config_editor for encrypted password storage");
            println!("   • Multiple configuration files are supported");

            // Check current database connection type
            if let Some(ref database_client) = db_arc.lock().unwrap().get_database_client() {
                let conn_info = database_client.get_connection_info();
                if conn_info.database_type == dbcrust::database::DatabaseType::MySQL {
                    println!();
                    println!("🔗 Current Connection (MySQL):");
                    println!(
                        "   Host: {}",
                        conn_info.host.as_deref().unwrap_or("localhost")
                    );
                    println!("   Port: {}", conn_info.port.unwrap_or(3306));
                    println!(
                        "   User: {}",
                        conn_info.username.as_deref().unwrap_or("unknown")
                    );
                    println!(
                        "   Database: {}",
                        conn_info.database.as_deref().unwrap_or("none")
                    );

                    // Check if current connection details match any .my.cnf entry
                    if let Some(_found_password) = lookup_mysql_password(
                        conn_info.host.as_deref().unwrap_or("localhost"),
                        conn_info.port.unwrap_or(3306),
                        conn_info.database.as_deref().unwrap_or(""),
                        conn_info.username.as_deref().unwrap_or(""),
                    ) {
                        println!("   ✅ Password found in .my.cnf");
                    } else {
                        println!("   ❌ No matching password entry in .my.cnf");
                    }
                }
            }
        }
        "\\pragma" => {
            println!("SQLite Pragma Settings Management:");
            println!();

            // Check if we're currently connected to SQLite
            if let Some(ref database_client) = db_arc.lock().unwrap().get_database_client() {
                let conn_info = database_client.get_connection_info();
                if conn_info.database_type == dbcrust::database::DatabaseType::SQLite {
                    println!("🔗 Current SQLite Connection:");
                    println!(
                        "   Database: {}",
                        conn_info.file_path.as_deref().unwrap_or("unknown")
                    );
                    println!();

                    // Show current pragma settings
                    println!("📊 Current Pragma Settings:");
                    println!("{}", "─".repeat(50));

                    // Get some important pragma settings
                    let pragmas_to_check = vec![
                        ("journal_mode", "Database journaling mode"),
                        ("synchronous", "Synchronous mode for writes"),
                        ("foreign_keys", "Foreign key constraint checking"),
                        ("temp_store", "Temporary storage location"),
                        ("cache_size", "Page cache size"),
                        ("mmap_size", "Memory-mapped I/O size"),
                        ("auto_vacuum", "Automatic vacuum mode"),
                        ("page_size", "Database page size"),
                        ("encoding", "Database text encoding"),
                        ("user_version", "User-defined version number"),
                    ];

                    for (pragma, description) in pragmas_to_check {
                        println!("   {}: {}", pragma, description);
                        // TODO: Add actual pragma value query when implementing async support
                        println!("      Current value: <requires async implementation>");
                    }
                    println!("{}", "─".repeat(50));
                } else {
                    println!("❌ Not connected to SQLite database");
                    println!("   Current connection: {:?}", conn_info.database_type);
                }
            } else {
                println!("❌ No active database connection");
            }

            println!();
            println!("📝 Common SQLite Pragma Commands:");
            println!("{}", "─".repeat(40));
            println!("Performance Optimization:");
            println!("  PRAGMA journal_mode = WAL;         -- Enable WAL mode");
            println!("  PRAGMA synchronous = NORMAL;       -- Balanced sync mode");
            println!("  PRAGMA cache_size = 10000;         -- Set cache size");
            println!("  PRAGMA mmap_size = 268435456;       -- Enable memory mapping");
            println!("  PRAGMA temp_store = MEMORY;         -- Use memory for temp");
            println!();
            println!("Data Integrity:");
            println!("  PRAGMA foreign_keys = ON;          -- Enable FK constraints");
            println!("  PRAGMA integrity_check;            -- Check database integrity");
            println!("  PRAGMA quick_check;                -- Quick integrity check");
            println!();
            println!("Database Maintenance:");
            println!("  PRAGMA auto_vacuum = INCREMENTAL;  -- Enable auto vacuum");
            println!("  PRAGMA optimize;                   -- Optimize database");
            println!("  PRAGMA vacuum;                     -- Reclaim space");
            println!("{}", "─".repeat(40));

            println!();
            println!("🔧 Usage Examples:");
            println!("   Query current setting: SELECT * FROM pragma_journal_mode;");
            println!("   Set new value: PRAGMA journal_mode = WAL;");
            println!("   Reset to default: PRAGMA journal_mode = DELETE;");

            println!();
            println!("⚠️  Important Notes:");
            println!("   • Some pragmas require database restart to take effect");
            println!("   • WAL mode improves concurrency but requires more disk space");
            println!("   • Always backup before changing critical settings");
            println!("   • Some pragmas are read-only and cannot be changed");

            println!();
            println!("📚 Reference:");
            println!("   For complete pragma documentation, visit:");
            println!("   https://www.sqlite.org/pragma.html");
        }
        "\\cs" => {
            let mut db = db_arc.lock().unwrap();
            let mode = db.toggle_column_select_mode();
            println!(
                "Column selection mode is {}",
                if mode { "on" } else { "off" }
            );
        }
        "\\setcs" => {
            // Get the current column selection mode setting
            let current_mode = db_arc.lock().unwrap().is_column_select_mode();

            let mut config = DbCrustConfig::load();
            config.column_selection_mode_default = current_mode;

            match config.save() {
                Ok(_) => {
                    println!(
                        "Default column selection mode set to {}",
                        if current_mode { "on" } else { "off" }
                    );
                }
                Err(e) => eprintln!("Error saving configuration: {}", e),
            }
        }
        cmd if cmd.starts_with("\\csthreshold ") => {
            let threshold_str = cmd[13..].trim(); // Skip "\\csthreshold "
            match threshold_str.parse::<usize>() {
                Ok(threshold) => {
                    let mut config = DbCrustConfig::load();
                    config.column_selection_threshold = threshold;

                    match config.save() {
                        Ok(_) => {
                            let mut db = db_arc.lock().unwrap();
                            db.set_column_selection_threshold(threshold);
                            println!(
                                "Column selection auto-enable threshold set to {}",
                                threshold
                            );
                        }
                        Err(e) => eprintln!("Error saving configuration: {}", e),
                    }
                }
                Err(_) => {
                    eprintln!("Error: threshold must be a valid number");
                }
            }
        }
        "\\csthreshold" => {
            // Get the current column selection threshold setting
            let current_threshold = DbCrustConfig::load().column_selection_threshold;

            print!(
                "Enter new column selection auto-enable threshold (current: {}): ",
                current_threshold
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let new_threshold = input.trim().parse::<usize>().unwrap_or(current_threshold);

            let mut config = DbCrustConfig::load();
            config.column_selection_threshold = new_threshold;

            match config.save() {
                Ok(_) => {
                    let mut db = db_arc.lock().unwrap();
                    db.set_column_selection_threshold(new_threshold);
                    println!(
                        "Default column selection auto-enable threshold set to {}",
                        new_threshold
                    );
                }
                Err(e) => eprintln!("Error saving configuration: {}", e),
            }
        }
        "\\docker" => {
            // List Docker database containers
            match dbcrust::docker::DockerClient::new() {
                Ok(docker_client) => {
                    match docker_client.list_database_containers().await {
                        Ok(containers) => {
                            if containers.is_empty() {
                                println!("No database containers found.");
                                println!("Make sure you have database containers running.");
                                println!("Example: docker run -d --name postgres-db -p 5432:5432 -e POSTGRES_PASSWORD=password postgres");
                            } else {
                                println!("\n🐳 Database Containers:");
                                println!("{:<20} {:<15} {:<12} {:<15} {:<10}", "NAME", "DATABASE", "STATUS", "IMAGE", "PORTS");
                                println!("{}", "-".repeat(80));
                                
                                for container in containers {
                                    let db_type = container.database_type
                                        .as_ref()
                                        .map(|dt| format!("{}", dt))
                                        .unwrap_or("Unknown".to_string());
                                    
                                    let status_with_icon = if container.status.contains("running") || container.status.contains("Up") {
                                        format!("🟢 {}", container.status)
                                    } else {
                                        format!("🔴 {}", container.status)
                                    };
                                    
                                    let image_short = if container.image.len() > 15 {
                                        format!("{}...", &container.image[..12])
                                    } else {
                                        container.image.clone()
                                    };
                                    
                                    let ports = if let Some(port) = container.host_port {
                                        format!(":{}", port)
                                    } else {
                                        "none".to_string()
                                    };
                                    
                                    println!("{:<20} {:<15} {:<12} {:<15} {:<10}", 
                                        container.name, db_type, status_with_icon, image_short, ports);
                                    
                                    // Show connection example for running containers
                                    if container.status.contains("running") || container.status.contains("Up") {
                                        if let Some(_) = container.host_port {
                                            println!("  └─ Connect: docker://{}", container.name);
                                        } else {
                                            // Check if OrbStack domain is available
                                            if let Some(database_type) = &container.database_type {
                                                if let Ok(Some((orbstack_host, _port))) = docker_client.get_orbstack_domain(&container, database_type) {
                                                    println!("  └─ Connect: docker://{} (OrbStack: {})", container.name, orbstack_host);
                                                } else {
                                                    println!("  └─ Connect: docker://{} (no exposed port)", container.name);
                                                }
                                            } else {
                                                println!("  └─ Connect: (no exposed port)");
                                            }
                                        }
                                    }
                                }
                                
                                println!("\nUsage:");
                                println!("  dbcrust docker://container_name");
                                println!("  dbcrust docker://user:pass@container_name/database");
                                println!("  dbcrust --docker-container container_name");
                                println!("\nNote: Containers without exposed ports can use OrbStack domains automatically");
                            }
                        }
                        Err(e) => {
                            eprintln!("Error listing Docker containers: {}", e);
                            eprintln!("Make sure Docker is running and accessible.");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error connecting to Docker: {}", e);
                    eprintln!("Make sure Docker daemon is running.");
                }
            }
        }
        cmd if cmd.starts_with("\\setmulti") => {
            // Set multiline prompt indicator
            let indicator = if cmd.len() > 9 {
                cmd[10..].to_string() // Skip "\\setmulti "
            } else {
                String::new() // Empty if no argument provided
            };
            
            // Update the current prompt
            let username = db_arc.lock().unwrap().get_username().to_string();
            let db_name = db_arc.lock().unwrap().get_current_db();
            *prompt = DbPrompt::with_config(username, db_name, indicator.clone());
            
            // Save to config
            let mut config = DbCrustConfig::load();
            config.multiline_prompt_indicator = indicator.clone();
            match config.save() {
                Ok(_) => {
                    if indicator.is_empty() {
                        println!("Multiline prompt indicator removed (set to empty)");
                    } else {
                        println!("Multiline prompt indicator set to: '{}'", indicator);
                    }
                }
                Err(e) => eprintln!("Error saving configuration: {}", e),
            }
        }
        "\\copy" => {
            let db = db_arc.lock().unwrap();
            match db.get_last_json_plan() {
                Some(json_plan) => {
                    match Clipboard::new() {
                        Ok(mut clipboard) => {
                            match clipboard.set_text(&json_plan) {
                                Ok(_) => println!("✓ JSON plan copied to clipboard ({} characters)", json_plan.len()),
                                Err(e) => eprintln!("Error copying to clipboard: {}", e),
                            }
                        }
                        Err(e) => eprintln!("Error accessing clipboard: {}", e),
                    }
                }
                None => {
                    println!("No JSON plan available to copy. Run an EXPLAIN query first.");
                }
            }
        }
        _ => {
            println!("Unknown command: {}", input);
            println!("Type \\h for help");
        }
    }
    Ok(false)
}

/// Process query results with column selection and formatting
async fn process_query_results(
    db: &mut Database,
    results: Vec<Vec<String>>,
    interrupt_flag: &Arc<AtomicBool>,
) -> Result<(), Box<dyn StdError>> {
    let column_count = results[0].len();
    let auto_enable = db.should_auto_enable_column_selection(column_count);

    let processed_results = if db.is_column_select_mode() || auto_enable {
        interrupt_flag.store(false, Ordering::SeqCst);

        if auto_enable && !db.is_column_select_mode() {
            println!(
                "Auto-enabling column selection mode due to high column count ({} columns exceeds threshold of {})",
                column_count,
                db.get_column_selection_threshold()
            );
        }

        match db.interactive_column_selection(&results, interrupt_flag) {
            Ok(filtered) => {
                if !filtered.is_empty() {
                    filtered
                } else {
                    println!("Column selection cancelled or no columns selected.");
                    return Ok(());
                }
            }
            Err(e) => {
                eprintln!("Error in column selection: {}", e);
                results
            }
        }
    } else {
        results
    };

    // Format and display results
    if db.is_expanded_display() {
        let expanded_tables = format_query_results_expanded(&processed_results);
        for table in expanded_tables {
            println!("{}", table);
        }
    } else {
        let output = format_query_results_psql(&processed_results);
        print!("{}", output);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::path::PathBuf;

    // Test the parse_vault_url function
    #[test]
    fn test_parse_vault_url() {
        // Test with a complete URL with all components
        let url = "vault://admin-role@secrets/postgres-prod";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, Some("admin-role".to_string()));
        assert_eq!(mount, "secrets".to_string());
        assert_eq!(db, Some("postgres-prod".to_string()));

        // Test with no role name
        let url = "vault://@secrets/postgres-prod";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, None);
        assert_eq!(mount, "secrets".to_string());
        assert_eq!(db, Some("postgres-prod".to_string()));

        // Test with default mount path
        let url = "vault://admin-role@/postgres-prod";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, Some("admin-role".to_string()));
        assert_eq!(mount, "database".to_string());
        assert_eq!(db, Some("postgres-prod".to_string()));

        // Test with no database name
        let url = "vault://admin-role@secrets/";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, Some("admin-role".to_string()));
        assert_eq!(mount, "secrets".to_string());
        assert_eq!(db, None);

        // Test with minimal URL (only the protocol)
        let url = "vault:///";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, None);
        assert_eq!(mount, "database".to_string());
        assert_eq!(db, None);

        // Test with just specifying the database
        let url = "vault:///postgres-prod";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, None);
        assert_eq!(mount, "database".to_string());
        assert_eq!(db, Some("postgres-prod".to_string()));

        // Test with only specifying the mount path (no role, no db)
        let url = "vault://secrets";
        let result = parse_vault_url(url);
        assert!(result.is_some());
        let (role, mount, db) = result.unwrap();
        assert_eq!(role, None);
        assert_eq!(mount, "secrets".to_string());
        assert_eq!(db, None);

        // Test with an invalid URL that doesn't start with vault://
        let url = "postgresql://user:pass@localhost:5432/mydb";
        let result = parse_vault_url(url);
        assert!(result.is_none());
    }

    // Helper function to simulate retrieving a password from different sources
    fn get_test_password(
        cmd_password: Option<String>,
        config_password: Option<String>,
        pgpass_lookup_result: Option<String>,
    ) -> String {
        match cmd_password {
            Some(pass) => pass,
            None => match config_password {
                Some(pass) => pass,
                None => match pgpass_lookup_result {
                    Some(pass) => pass,
                    None => "prompted_password".to_string(), // Simulate prompted password
                },
            },
        }
    }

    #[rstest]
    #[case(Some("cmd_password".to_string()), None, None, "cmd_password")]
    #[case(None, Some("config_password".to_string()), None, "config_password")]
    #[case(None, None, Some("pgpass_password".to_string()), "pgpass_password")]
    #[case(None, None, None, "prompted_password")]
    fn test_password_resolution(
        #[case] cmd_password: Option<String>,
        #[case] config_password: Option<String>,
        #[case] pgpass_lookup_result: Option<String>,
        #[case] expected: &str,
    ) {
        let result = get_test_password(cmd_password, config_password, pgpass_lookup_result);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_command_line_args_always_override_config() {
        let cmd_host = "cmd_host".to_string();
        let cmd_port = 5434;
        let cmd_user = "cmd_user".to_string();
        let cmd_dbname = "cmd_dbname".to_string();

        let args = Args {
            host: cmd_host.clone(),
            port: cmd_port,
            user: cmd_user.clone(),
            dbname: cmd_dbname.clone(),
            url: None,
            connection_url: None,
            password: None,
            no_banner: false,
            help_all: false,
            ssh_tunnel: None,
            debug: false,
            show_debug_logs: false,
            vault: false,
            vault_db_name: None,
            vault_role_name: None,
            vault_mount_path: "database".to_string(),
            docker_container: None,
            docker_socket: None,
            generate_completion: None,
            completion_out: None,
            command: vec![],
        };

        assert_eq!(args.host, cmd_host);
        assert_eq!(args.port, cmd_port);
        assert_eq!(args.user, cmd_user);
        assert_eq!(args.dbname, cmd_dbname);
    }

    #[test]
    fn test_positional_connection_url() {
        // Test with positional connection URL
        let url = "postgresql://testuser:testpass@testhost:5555/testdb".to_string();

        let args = Args {
            host: "default_host".to_string(),
            port: 5432,
            user: "default_user".to_string(),
            dbname: "default_dbname".to_string(),
            url: None,
            connection_url: Some(url),
            password: None,
            no_banner: false,
            help_all: false,
            ssh_tunnel: None,
            debug: false,
            show_debug_logs: false,
            vault: false,
            vault_db_name: None,
            vault_role_name: None,
            vault_mount_path: "database".to_string(),
            docker_container: None,
            docker_socket: None,
            generate_completion: None,
            completion_out: None,
            command: vec![],
        };

        // Make assertions to validate that connection_url is used
        assert!(args.connection_url.is_some());
        let url_str = args.connection_url.unwrap();

        // Parse the URL
        let parsed_url = Url::parse(&url_str).unwrap();

        // Verify each component
        assert_eq!(parsed_url.host_str().unwrap(), "testhost");
        assert_eq!(parsed_url.port().unwrap(), 5555);
        assert_eq!(parsed_url.username(), "testuser");
        assert_eq!(parsed_url.password().unwrap(), "testpass");
        assert_eq!(parsed_url.path(), "/testdb");
    }

    #[test]
    fn test_sslmode_url_parsing() {
        // Helper function to parse sslmode from URL query parameters
        fn parse_sslmode_from_url(url_str: &str) -> Option<PgSslMode> {
            if let Ok(parsed_url) = Url::parse(url_str) {
                if let Some(query_pairs) = parsed_url.query() {
                    let query_params: HashMap<String, String> =
                        url::form_urlencoded::parse(query_pairs.as_bytes())
                            .into_owned()
                            .collect();

                    if let Some(sslmode_str) = query_params.get("sslmode") {
                        return match sslmode_str.as_str() {
                            "disable" => Some(PgSslMode::Disable),
                            "allow" => Some(PgSslMode::Allow),
                            "prefer" => Some(PgSslMode::Prefer),
                            "require" => Some(PgSslMode::Require),
                            "verify-ca" => Some(PgSslMode::VerifyCa),
                            "verify-full" => Some(PgSslMode::VerifyFull),
                            _ => {
                                eprintln!(
                                    "Warning: Unknown sslmode value '{}'. Using default (prefer).",
                                    sslmode_str
                                );
                                Some(PgSslMode::Prefer)
                            }
                        };
                    }
                }
            }
            None
        }

        // Test URL without sslmode parameter
        let url_no_ssl = "postgresql://user:pass@localhost/mydb";
        let ssl_mode = parse_sslmode_from_url(url_no_ssl);
        assert!(ssl_mode.is_none());

        // Test URL with sslmode=require
        let url_require = "postgresql://user:pass@localhost/mydb?sslmode=require";
        let ssl_mode = parse_sslmode_from_url(url_require);
        assert!(ssl_mode.is_some());
        assert!(matches!(ssl_mode.unwrap(), PgSslMode::Require));

        // Test URL with sslmode=disable
        let url_disable = "postgresql://user:pass@localhost/mydb?sslmode=disable";
        let ssl_mode = parse_sslmode_from_url(url_disable);
        assert!(ssl_mode.is_some());
        assert!(matches!(ssl_mode.unwrap(), PgSslMode::Disable));

        // Test URL with sslmode=verify-ca
        let url_verify_ca = "postgresql://user:pass@localhost/mydb?sslmode=verify-ca";
        let ssl_mode = parse_sslmode_from_url(url_verify_ca);
        assert!(ssl_mode.is_some());
        assert!(matches!(ssl_mode.unwrap(), PgSslMode::VerifyCa));

        // Test URL with sslmode=verify-full
        let url_verify_full = "postgresql://user:pass@localhost/mydb?sslmode=verify-full";
        let ssl_mode = parse_sslmode_from_url(url_verify_full);
        assert!(ssl_mode.is_some());
        assert!(matches!(ssl_mode.unwrap(), PgSslMode::VerifyFull));

        // Test URL with multiple query parameters
        let url_multiple =
            "postgresql://user:pass@localhost/mydb?sslmode=require&connect_timeout=10";
        let ssl_mode = parse_sslmode_from_url(url_multiple);
        assert!(ssl_mode.is_some());
        assert!(matches!(ssl_mode.unwrap(), PgSslMode::Require));

        // Test URL with invalid sslmode value (should default to prefer)
        let url_invalid = "postgresql://user:pass@localhost/mydb?sslmode=invalid";
        let ssl_mode = parse_sslmode_from_url(url_invalid);
        assert!(ssl_mode.is_some());
        assert!(matches!(ssl_mode.unwrap(), PgSslMode::Prefer));
    }

    #[test]
    fn test_shell_completion_args() {
        let args = Args {
            host: "default_host".to_string(),
            port: 5432,
            user: "default_user".to_string(),
            dbname: "default_dbname".to_string(),
            url: None,
            connection_url: None,
            password: None,
            no_banner: false,
            help_all: false,
            ssh_tunnel: None,
            debug: false,
            show_debug_logs: false,
            vault: false,
            vault_db_name: None,
            vault_role_name: None,
            vault_mount_path: "database".to_string(),
            docker_container: None,
            docker_socket: None,
            generate_completion: Some(cli::Shell::Bash),
            completion_out: Some(PathBuf::from("/tmp/dbcrust.bash")),
            command: vec![],
        };

        assert_eq!(args.generate_completion, Some(cli::Shell::Bash));
        assert_eq!(
            args.completion_out,
            Some(PathBuf::from("/tmp/dbcrust.bash"))
        );
    }

    #[test]
    fn test_command_option() {
        let args = Args {
            host: "localhost".to_string(),
            port: 5432,
            user: "postgres".to_string(),
            dbname: "testdb".to_string(),
            url: None,
            connection_url: None,
            password: None,
            no_banner: false,
            help_all: false,
            ssh_tunnel: None,
            debug: false,
            show_debug_logs: false,
            vault: false,
            vault_db_name: None,
            vault_role_name: None,
            vault_mount_path: "database".to_string(),
            docker_container: None,
            docker_socket: None,
            generate_completion: None,
            completion_out: None,
            command: vec!["SELECT 1;".to_string(), "SELECT 2;".to_string()],
        };

        assert_eq!(args.command.len(), 2);
        assert_eq!(args.command[0], "SELECT 1;");
        assert_eq!(args.command[1], "SELECT 2;");
    }
}
