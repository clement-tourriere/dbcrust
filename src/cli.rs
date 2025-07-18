use crate::password_sanitizer::{sanitize_connection_url, sanitize_ssh_tunnel_string};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// DBCrust - A multi-database interactive client with tab completion
#[derive(Parser, Clone)]
#[command(name = "dbcrust")]
#[command(version, long_about = None)]
#[command(about = "A multi-database interactive client with tab completion (PostgreSQL, SQLite, MySQL, Docker)")]
pub struct Args {
    /// Database connection URL (e.g., postgresql://user:pass@host:port/dbname, sqlite:///path/to/file.db, mysql://user:pass@host:port/dbname, docker://user:pass@container/dbname)
    /// This is the recommended way to connect to a database server.
    /// PostgreSQL: supports sslmode parameter: disable, allow, prefer (default), require, verify-ca, verify-full
    /// SQLite: use sqlite:///absolute/path/to/database.db or sqlite://./relative/path.db
    /// MySQL: use mysql://user:pass@host:port/dbname, supports standard MySQL connection options
    /// Docker: use docker://user:pass@container_name/dbname to connect to database in Docker container
    #[arg(value_name = "CONNECTION_URL")]
    pub connection_url: Option<String>,

    /// Database connection URL (e.g., postgresql://user:pass@host:port/dbname, sqlite:///path/to/file.db, mysql://user:pass@host:port/dbname, docker://user:pass@container/dbname)
    /// If provided, this will override individual connection parameters.
    /// PostgreSQL: supports sslmode parameter: disable, allow, prefer (default), require, verify-ca, verify-full
    /// SQLite: use sqlite:///absolute/path/to/database.db or sqlite://./relative/path.db
    /// MySQL: use mysql://user:pass@host:port/dbname, supports standard MySQL connection options
    /// Docker: use docker://user:pass@container_name/dbname to connect to database in Docker container
    #[arg(long)]
    pub url: Option<String>,

    /// PostgreSQL host
    #[arg(short = 'H', long, default_value = "localhost")]
    pub host: String,

    /// PostgreSQL port
    #[arg(short, long, default_value_t = 5432)]
    pub port: u16,

    /// PostgreSQL username
    #[arg(short, long, default_value = "postgres")]
    pub user: String,

    /// PostgreSQL password
    #[arg(short = 'w', long)]
    pub password: Option<String>,

    /// PostgreSQL database name
    #[arg(short = 'd', long, default_value = "postgres")]
    pub dbname: String,

    /// Don't display the banner
    #[arg(long, default_value_t = false)]
    pub no_banner: bool,

    /// Show complete help with all commands and options
    #[arg(long, default_value_t = false)]
    pub help_all: bool,

    /// Open an SSH tunnel to access the database
    /// Format: [user[:password]@]ssh_host[:ssh_port]
    /// Example: john:pass@jumphost.example.com:2222
    #[arg(long)]
    pub ssh_tunnel: Option<String>,

    /// Enable debug mode - shows verbose SSH tunnel and connection debugging
    #[arg(long, default_value_t = false)]
    pub debug: bool,

    /// Show the location of the debug log file and exit
    #[arg(long, default_value_t = false)]
    pub show_debug_logs: bool,

    /// Enable Vault integration for connection credentials
    #[arg(long, env = "DBCRUST_VAULT", group = "connection_source")]
    pub vault: bool,

    /// Vault: Name of the database configuration in Vault (e.g., my-postgres-config)
    #[arg(long, env = "DBCRUST_VAULT_DB_NAME", requires = "vault")]
    pub vault_db_name: Option<String>,

    /// Vault: Name of the role to request credentials for
    #[arg(long, env = "DBCRUST_VAULT_ROLE_NAME", requires = "vault")]
    pub vault_role_name: Option<String>,

    /// Vault: Mount path of the database secrets engine (defaults to "database")
    #[arg(
        long,
        env = "DBCRUST_VAULT_MOUNT_PATH",
        default_value = "database",
        requires = "vault"
    )]
    pub vault_mount_path: String,

    /// Connect to a database running in a Docker container
    /// Example: --docker-container postgres-db
    #[arg(long, env = "DBCRUST_DOCKER_CONTAINER", group = "connection_source")]
    pub docker_container: Option<String>,

    /// Docker socket path (defaults to system default)
    #[arg(long, env = "DBCRUST_DOCKER_SOCKET", requires = "docker_container")]
    pub docker_socket: Option<String>,

    /// Generate shell completions for the specified shell and exit
    #[arg(long)]
    pub generate_completion: Option<Shell>,

    /// Path to write completion file to (defaults to stdout)
    #[arg(long, requires = "generate_completion")]
    pub completion_out: Option<PathBuf>,

    /// Execute the given command string and exit
    /// This option can be repeated and combined with -f option
    /// Each SQL command string is sent as a single request and executed as a single transaction
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
                "url",
                &self.url.as_ref().map(|url| sanitize_connection_url(url)),
            )
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("dbname", &self.dbname)
            .field("no_banner", &self.no_banner)
            .field("help_all", &self.help_all)
            .field(
                "ssh_tunnel",
                &self
                    .ssh_tunnel
                    .as_ref()
                    .map(|tunnel| sanitize_ssh_tunnel_string(tunnel)),
            )
            .field("debug", &self.debug)
            .field("show_debug_logs", &self.show_debug_logs)
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
