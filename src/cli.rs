use crate::password_sanitizer::{sanitize_connection_url, sanitize_ssh_tunnel_string};
use clap::{Parser, Subcommand, ValueEnum};

/// DBCrust - a fast psql-style database workbench
#[derive(Parser, Clone)]
#[command(name = "dbcrust")]
#[command(version, long_about = None)]
#[command(
    about = "A fast psql-style database workbench for databases, files, Docker, Vault, and optional AI"
)]
#[command(arg_required_else_help = false)]
#[command(after_help = "Examples:
  dbcrust postgres://user:pass@localhost:5432/mydb
  dbcrust recent://                 # pick from recent connections
  dbcrust session://prod            # open a saved session
  dbcrust docker://my-container/mydb
  dbcrust sqlite:///path/to/file.db
  dbcrust 'parquet:///data/*.parquet'
  dbcrust config                    # interactive configuration menu (no connection)
  dbcrust config set logging.level debug
  dbcrust --update                  # update dbcrust to the latest release")]
pub struct Args {
    /// Database connection URL
    ///
    /// Examples:
    ///   PostgreSQL: postgresql://user:pass@localhost:5432/mydb
    ///   MySQL:      mysql://user:pass@localhost:3306/mydb
    ///   SQLite:     sqlite:///path/to/database.db
    ///   ClickHouse: clickhouse://user:pass@localhost:8123/mydb
    ///   Docker:     docker://container_name/mydb
    ///   Files:      parquet:///data/*.parquet | csv:///logs/*.csv | json:///events.ndjson
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

    /// Check for a newer release and update dbcrust in place
    #[arg(long)]
    pub update: bool,

    /// Utility subcommands that run without a database connection
    #[command(subcommand)]
    pub subcommand: Option<CliCommand>,
}

/// Top-level subcommands. A bare word matching a subcommand name wins over the
/// positional URL — a relative SQLite file literally named `config` must be
/// opened as `sqlite://config`.
#[derive(Subcommand, Clone, Debug)]
pub enum CliCommand {
    /// View and edit DBCrust configuration (no database connection needed)
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum ConfigAction {
    /// Print a summary of the current configuration
    Show,
    /// Print one value, or all keys when no key is given
    Get {
        /// Dotted key, e.g. logging.level
        key: Option<String>,
    },
    /// Set a configuration value
    Set {
        /// Dotted key, e.g. logging.level
        key: String,
        /// New value (quote values containing spaces)
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Open config.toml in $EDITOR and reload it on close
    Edit,
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
            .field("update", &self.update)
            .field("subcommand", &self.subcommand)
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_no_args() {
        let args = Args::try_parse_from(["dbcrust"]).unwrap();
        assert!(args.connection_url.is_none());
        assert!(args.command.is_empty());
        assert!(!args.update);
    }

    #[test]
    fn test_update_flag() {
        let args = Args::try_parse_from(["dbcrust", "--update"]).unwrap();
        assert!(args.update);
        assert!(args.connection_url.is_none());
    }

    #[test]
    fn test_connection_url() {
        let args = Args::try_parse_from(["dbcrust", "postgres://localhost/test"]).unwrap();
        assert_eq!(
            args.connection_url.as_deref(),
            Some("postgres://localhost/test")
        );
    }

    #[test]
    fn test_single_command() {
        let args = Args::try_parse_from(["dbcrust", "-c", "SELECT 1"]).unwrap();
        assert_eq!(args.command, vec!["SELECT 1"]);
    }

    #[test]
    fn test_multiple_commands() {
        let args = Args::try_parse_from(["dbcrust", "-c", "\\dt", "-c", "SELECT 1"]).unwrap();
        assert_eq!(args.command, vec!["\\dt", "SELECT 1"]);
    }

    #[test]
    fn test_ssh_tunnel() {
        let args = Args::try_parse_from([
            "dbcrust",
            "--ssh-tunnel",
            "user@host",
            "postgres://localhost/test",
        ])
        .unwrap();
        assert_eq!(args.ssh_tunnel.as_deref(), Some("user@host"));
        assert_eq!(
            args.connection_url.as_deref(),
            Some("postgres://localhost/test")
        );
    }

    #[test]
    fn test_completions() {
        let args = Args::try_parse_from(["dbcrust", "--completions", "bash"]).unwrap();
        assert_eq!(args.completions, Some(Shell::Bash));
    }

    #[test]
    fn test_config_subcommand_bare() {
        let args = Args::try_parse_from(["dbcrust", "config"]).unwrap();
        assert!(matches!(
            args.subcommand,
            Some(CliCommand::Config { action: None })
        ));
        assert!(args.connection_url.is_none());
    }

    #[test]
    fn test_config_subcommand_get() {
        let args = Args::try_parse_from(["dbcrust", "config", "get", "logging.level"]).unwrap();
        let Some(CliCommand::Config {
            action: Some(ConfigAction::Get { key }),
        }) = args.subcommand
        else {
            panic!("expected config get subcommand");
        };
        assert_eq!(key.as_deref(), Some("logging.level"));
    }

    #[test]
    fn test_config_subcommand_set() {
        let args =
            Args::try_parse_from(["dbcrust", "config", "set", "default_limit", "50"]).unwrap();
        let Some(CliCommand::Config {
            action: Some(ConfigAction::Set { key, value }),
        }) = args.subcommand
        else {
            panic!("expected config set subcommand");
        };
        assert_eq!(key, "default_limit");
        assert_eq!(value, "50");
    }

    #[test]
    fn test_config_subcommand_set_hyphen_value() {
        let args = Args::try_parse_from(["dbcrust", "config", "set", "pager_command", "less -RFX"])
            .unwrap();
        let Some(CliCommand::Config {
            action: Some(ConfigAction::Set { value, .. }),
        }) = args.subcommand
        else {
            panic!("expected config set subcommand");
        };
        assert_eq!(value, "less -RFX");
    }

    #[test]
    fn test_connection_url_still_wins_over_subcommand() {
        // A URL must not be mistaken for a subcommand.
        let args = Args::try_parse_from(["dbcrust", "postgres://localhost/test"]).unwrap();
        assert!(args.subcommand.is_none());
        assert_eq!(
            args.connection_url.as_deref(),
            Some("postgres://localhost/test")
        );
    }
}
