// Import the debug_log macro here
extern crate dbcrust;
mod cli;
// completion is now in lib.rs
mod highlighter;
mod named_queries;
mod pager;
mod password_sanitizer;
mod pgpass;
mod script;

use clap::Parser;
use dbcrust::cli::Args;
use dbcrust::config::Config as DbCrustConfig;
use nu_ansi_term::Color;
use std::error::Error as StdError;
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



/// Main async workflow that can be called from both main() and Python
pub async fn async_main() -> Result<(), Box<dyn StdError>> {
    let args = Args::parse();
    dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}

/// Main async workflow with pre-parsed arguments (for Python integration)
pub async fn async_main_with_args(args: Args) -> Result<(), Box<dyn StdError>> {
    dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}



#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    let args = Args::parse();
    let exit_code = dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    std::process::exit(exit_code);
}
