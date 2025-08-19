use crate::password_sanitizer::{sanitize_connection_url, sanitize_ssh_tunnel_string};
use clap::{Parser, ValueEnum};

/// DBCrust - A blazing-fast multi-database client
#[derive(Parser, Clone)]
#[command(name = "dbcrust")]
#[command(version, long_about = None)]
#[command(about = "A blazing-fast multi-database client with intelligent autocompletion")]
#[command(arg_required_else_help = false)]
pub struct Args {
    /// Database connection URL
    ///
    /// Examples:
    ///   PostgreSQL: postgresql://user:pass@localhost:5432/mydb
    ///   MySQL:      mysql://user:pass@localhost:3306/mydb
    ///   SQLite:     sqlite:///path/to/database.db
    ///   ClickHouse: clickhouse://user:pass@localhost:8123/mydb
    ///   Docker:     docker://container_name/mydb
    ///   Session:    session://saved_session_name
    ///   Recent:     recent:// (interactive selection)
    #[arg(value_name = "URL")]
    pub connection_url: Option<String>,

    /// Open an SSH tunnel to access the database
    /// Format: [user@]host[:port]
    #[arg(long)]
    pub ssh_tunnel: Option<String>,

    /// Generate shell completions
    #[arg(long, value_enum)]
    pub completions: Option<Shell>,

    /// Execute SQL command and exit
    #[arg(short, long, action = clap::ArgAction::Append)]
    pub command: Vec<String>,
}

impl std::fmt::Debug for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Args")
            .field(
                "connection_url",
                &self
                    .connection_url
                    .as_ref()
                    .map(|url| sanitize_connection_url(url)),
            )
            .field(
                "ssh_tunnel",
                &self
                    .ssh_tunnel
                    .as_ref()
                    .map(|tunnel| sanitize_ssh_tunnel_string(tunnel)),
            )
            .field("completions", &self.completions)
            .field("command", &self.command)
            .finish()
    }
}

/// Supported shells for completion generation
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Elvish,
}
