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
  dbcrust ./data.csv                # infer CSV from extension
  dbcrust sqlite:///path/to/file.db
  dbcrust 'parquet:///data/*.parquet'
  dbcrust file://                   # pick a compatible file from the current directory
  dbcrust config                    # interactive configuration menu (no connection)
  dbcrust config set logging.level debug
  dbcrust --update                  # update dbcrust to the latest release")]
pub struct Args {
    /// Database connection URL
    ///
    /// Examples:
    ///   PostgreSQL: postgresql://user:pass@localhost:5432/mydb
    ///   MySQL:      mysql://user:pass@localhost:3306/mydb
    ///   SQLite:     sqlite:///path/to/database.db or ./database.sqlite
    ///   ClickHouse: clickhouse://user:pass@localhost:8123/mydb
    ///   Docker:     docker://container_name/mydb
    ///   Files:      ./data.csv | parquet:///data/*.parquet | csv:///logs/*.csv | file://
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

    /// Execute SQL from a file and exit (repeatable; runs in command-line
    /// order interleaved with -c)
    #[arg(short = 'f', long = "file", value_name = "PATH", action = clap::ArgAction::Append)]
    pub file: Vec<String>,

    /// Check for a newer release and update dbcrust in place
    #[arg(long)]
    pub update: bool,

    /// Output format for query results
    #[arg(
        short = 'o',
        long = "format",
        value_enum,
        env = "DBCRUST_FORMAT",
        value_name = "FORMAT"
    )]
    pub format: Option<OutputFormat>,

    /// Query timeout in seconds for this run (0 disables; overrides the
    /// query_timeout_seconds config)
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Row limit auto-added to top-level SELECTs for this run (0 disables;
    /// overrides the default_limit config)
    #[arg(long, value_name = "N")]
    pub max_rows: Option<usize>,

    /// Never open interactive prompts; fail with a clear error instead
    /// (implied when stdin is not a terminal)
    #[arg(long)]
    pub no_input: bool,

    /// -c/-f sources in command-line order; filled by `parse_from_argv`,
    /// empty when Args is built programmatically
    #[arg(skip)]
    pub ordered_sources: Vec<OneShotSource>,

    /// Utility subcommands that run without a database connection
    #[command(subcommand)]
    pub subcommand: Option<CliCommand>,
}

/// A one-shot execution source: an inline `-c` command or a `-f` SQL file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OneShotSource {
    Command(String),
    File(String),
}

impl Args {
    /// The `-c`/`-f` sources to execute. In command-line order when parsed
    /// via [`Args::parse_from_argv`]; programmatically built Args fall back
    /// to files-then-commands.
    pub fn one_shot_sources(&self) -> Vec<OneShotSource> {
        if !self.ordered_sources.is_empty() {
            return self.ordered_sources.clone();
        }
        self.file
            .iter()
            .cloned()
            .map(OneShotSource::File)
            .chain(self.command.iter().cloned().map(OneShotSource::Command))
            .collect()
    }

    /// Parse argv preserving the relative order of `-c` and `-f` occurrences
    /// (clap collects each flag separately; argv indices restore the
    /// interleaving).
    pub fn parse_from_argv<I, T>(argv: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        use clap::{CommandFactory, FromArgMatches};
        let matches = Self::command().try_get_matches_from(argv)?;
        let mut args = Self::from_arg_matches(&matches)?;
        let mut sources: Vec<(usize, OneShotSource)> = Vec::new();
        if let Some(indices) = matches.indices_of("command") {
            sources.extend(indices.zip(args.command.iter().cloned().map(OneShotSource::Command)));
        }
        if let Some(indices) = matches.indices_of("file") {
            sources.extend(indices.zip(args.file.iter().cloned().map(OneShotSource::File)));
        }
        sources.sort_by_key(|(index, _)| *index);
        args.ordered_sources = sources.into_iter().map(|(_, source)| source).collect();
        Ok(args)
    }
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
            .field("file", &self.file)
            .field("update", &self.update)
            .field("format", &self.format)
            .field("timeout", &self.timeout)
            .field("max_rows", &self.max_rows)
            .field("no_input", &self.no_input)
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

/// Output format for query results (`-o/--format`, env `DBCRUST_FORMAT`).
///
/// Defined here rather than in `format.rs` because cli.rs is also compiled
/// standalone into the binaries (`mod cli;` in main.rs) and must not depend
/// on lib-only modules.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// psql-style aligned table (default)
    #[default]
    Table,
    /// Vertical one-record-per-block layout (same as `\x`)
    Expanded,
    /// RFC 4180 CSV with a header row
    Csv,
    /// One `{"columns":[…],"rows":[[…]],"row_count":n,"truncated":bool}` envelope per statement
    Json,
    /// One JSON object per data row, keys in column order
    Jsonl,
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
    fn test_file_flag_and_interleaved_sources() {
        let args = Args::parse_from_argv([
            "dbcrust",
            "sqlite://t.db",
            "-f",
            "a.sql",
            "-c",
            "SELECT 1",
            "-f",
            "b.sql",
        ])
        .unwrap();
        assert_eq!(args.file, vec!["a.sql", "b.sql"]);
        assert_eq!(args.command, vec!["SELECT 1"]);
        assert_eq!(
            args.one_shot_sources(),
            vec![
                OneShotSource::File("a.sql".to_string()),
                OneShotSource::Command("SELECT 1".to_string()),
                OneShotSource::File("b.sql".to_string()),
            ]
        );
    }

    #[test]
    fn test_one_shot_sources_fallback_for_programmatic_args() {
        // try_parse_from does not fill ordered_sources — the fallback puts
        // files before commands
        let args =
            Args::try_parse_from(["dbcrust", "-c", "SELECT 1", "-f", "a.sql", "url"]).unwrap();
        assert!(args.ordered_sources.is_empty());
        assert_eq!(
            args.one_shot_sources(),
            vec![
                OneShotSource::File("a.sql".to_string()),
                OneShotSource::Command("SELECT 1".to_string()),
            ]
        );
    }

    #[test]
    fn test_timeout_max_rows_no_input_flags() {
        let args = Args::parse_from_argv([
            "dbcrust",
            "--timeout",
            "10",
            "--max-rows",
            "0",
            "--no-input",
            "postgres://localhost/db",
        ])
        .unwrap();
        assert_eq!(args.timeout, Some(10));
        assert_eq!(args.max_rows, Some(0));
        assert!(args.no_input);
        assert_eq!(
            args.connection_url.as_deref(),
            Some("postgres://localhost/db")
        );

        let args = Args::parse_from_argv(["dbcrust"]).unwrap();
        assert_eq!(args.timeout, None);
        assert_eq!(args.max_rows, None);
        assert!(!args.no_input);
        assert!(args.one_shot_sources().is_empty());
    }

    #[test]
    fn test_format_flag() {
        let args = Args::try_parse_from(["dbcrust", "-o", "json", "sqlite://test.db"]).unwrap();
        assert_eq!(args.format, Some(OutputFormat::Json));
        assert_eq!(args.connection_url.as_deref(), Some("sqlite://test.db"));

        let args = Args::try_parse_from(["dbcrust", "--format", "csv"]).unwrap();
        assert_eq!(args.format, Some(OutputFormat::Csv));

        let args = Args::try_parse_from(["dbcrust"]).unwrap();
        assert_eq!(args.format, None);

        assert!(Args::try_parse_from(["dbcrust", "-o", "yaml"]).is_err());
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
