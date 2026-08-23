use crate::password_sanitizer::{sanitize_connection_url, sanitize_ssh_tunnel_string};
use std::ffi::{OsStr, OsString};
use usage::{Subcommands, ValueEnum};

/// Connection examples shown under the root help page.
const ROOT_EXAMPLES: &str = "Examples:
  dbcrust postgres://user:pass@localhost:5432/mydb
  dbcrust recent://                 # pick from recent connections
  dbcrust session://prod            # open a saved session
  dbcrust docker://my-container/mydb
  dbcrust ./data.csv                # infer CSV from extension
  dbcrust sqlite:///path/to/file.db
  dbcrust white-dragon://localhost:7700
  dbcrust 'parquet:///data/*.parquet'
  dbcrust file://                   # pick a compatible file from the current directory
  dbcrust config                    # interactive configuration menu (no connection)
  dbcrust config set logging.level debug
  dbcrust agents                    # print the guide for AI coding agents
  dbcrust --update                  # update dbcrust to the latest release";

// The program's grammar, as `usage` sees it. (Plain comments: a doc comment
// here would become the CLI's long_about.)
//
// Private on purpose: `Args` is what the rest of the program consumes and
// it keeps every field at the top level. `usage` needs the `subcommand` field
// on the root struct (a flattened group cannot own subcommands) while the
// URL completer needs its owner to derive `usage::Args` (a root `Cli` struct
// does not implement `CommandArgs`), hence this two-level shape: the root
// holds the metadata and the subcommand, the flattened `Args` holds
// everything else, and `Args::parse_from_argv` folds the two back together.
#[derive(usage::Cli)]
#[usage(
    bin = "dbcrust",
    version,
    usage = "Usage: dbcrust [FLAGS] [URL] [SUBCOMMAND]",
    about = "A fast psql-style database workbench for databases, files, Docker, Vault, and optional AI",
    after_help = ROOT_EXAMPLES,
    unknown_flags = "error",
    completion
)]
struct Root {
    #[usage(flatten)]
    args: Args,

    /// Utility subcommands that run without a database connection
    #[usage(subcommand)]
    subcommand: Option<CliCommand>,
}

/// DBCrust command-line arguments.
///
/// Built by [`Args::parse_from_argv`] (the binaries and the Python entry
/// point) or constructed directly when embedding the CLI.
#[derive(usage::Args, Clone)]
pub struct Args {
    /// Database, search-engine, or file target URL
    ///
    /// Examples:
    ///   PostgreSQL: postgresql://user:pass@localhost:5432/mydb
    ///   MySQL:      mysql://user:pass@localhost:3306/mydb
    ///   SQLite:     sqlite:///path/to/database.db or ./database.sqlite
    ///   ClickHouse: clickhouse://user:pass@localhost:8123/mydb
    ///   White Dragon: white-dragon://localhost:7700
    ///   Docker:     docker://container_name/mydb
    ///   Files:      ./data.csv | parquet:///data/*.parquet | csv:///logs/*.csv | file://
    ///   Session:    session://saved_session_name
    ///   Recent:     recent:// (interactive selection)
    #[usage(
        value_name = "URL",
        complete = crate::shell_completion::complete_connection_url
    )]
    pub connection_url: Option<String>,

    /// Open an SSH tunnel to access the database
    ///
    /// Format: [user@]host[:port]
    #[usage(long, value_name = "TUNNEL")]
    pub ssh_tunnel: Option<String>,

    /// Generate shell completions
    #[usage(long, value_enum, value_name = "SHELL")]
    pub completions: Option<Shell>,

    /// Execute SQL command and exit
    #[usage(short = 'c', long, value_name = "SQL")]
    pub command: Vec<String>,

    /// Execute SQL from a file and exit
    ///
    /// Repeatable; runs in command-line order interleaved with -c.
    #[usage(
        short = 'f',
        long = "file",
        value_name = "PATH",
        value_hint = usage::ValueHint::FilePath
    )]
    pub file: Vec<String>,

    /// Check for a newer release and update dbcrust in place
    #[usage(long)]
    pub update: bool,

    /// Output format for query results
    #[usage(
        short = 'o',
        long = "format",
        value_enum,
        env = "DBCRUST_FORMAT",
        value_name = "FORMAT"
    )]
    pub format: Option<OutputFormat>,

    /// Reject statements that could write or cause side effects
    ///
    /// Best-effort guard; use a read-only DB role for hard guarantees.
    /// Overrides the read_only_default config; `--read-only=false` re-enables
    /// writes. `require_equals` keeps `--read-only <url>` from swallowing the
    /// URL.
    #[usage(long, default_missing = "true", require_equals, value_name = "BOOL")]
    pub read_only: Option<bool>,

    /// Query timeout in seconds for this run
    ///
    /// 0 disables; overrides the query_timeout_seconds config.
    #[usage(long, value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Row limit auto-added to top-level SELECTs for this run
    ///
    /// 0 disables; overrides the default_limit config.
    #[usage(long, value_name = "N")]
    pub max_rows: Option<usize>,

    /// Never open interactive prompts, fail with a clear error instead
    ///
    /// Implied when stdin is not a terminal.
    #[usage(long)]
    pub no_input: bool,

    /// -c/-f sources in command-line order; filled by `parse_from_argv`,
    /// empty when Args is built programmatically
    #[usage(skip)]
    pub ordered_sources: Vec<OneShotSource>,

    /// Utility subcommands that run without a database connection. Declared
    /// on [`Root`] for parsing and copied here by `parse_from_argv`.
    #[usage(skip)]
    pub subcommand: Option<CliCommand>,
}

/// A one-shot execution source: an inline `-c` command or a `-f` SQL file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OneShotSource {
    Command(String),
    File(String),
}

/// Why [`Args::parse_from_argv`] produced no [`Args`]: the process should
/// print something and exit instead of running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseExit {
    /// `--help`, `--version`, a shell-completion reply, or the usage spec:
    /// goes to stdout, exit status 0
    Output(String),
    /// A malformed command line: the rendered diagnostic for stderr, exit
    /// status 2 (the same status clap used, so scripts keep working)
    Usage(String),
}

impl ParseExit {
    /// Process exit status this outcome maps to.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Output(_) => 0,
            Self::Usage(_) => 2,
        }
    }

    /// Print the text where it belongs and exit with [`Self::exit_code`].
    pub fn exit(self) -> ! {
        match &self {
            Self::Output(text) => print!("{text}"),
            Self::Usage(text) => eprint!("{text}"),
        }
        std::process::exit(self.exit_code())
    }
}

impl std::fmt::Display for ParseExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Output(text) | Self::Usage(text) => f.write_str(text.trim_end()),
        }
    }
}

impl std::error::Error for ParseExit {}

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

    /// Parse a full argv (program name first).
    ///
    /// Answers the hidden `__complete_word__` and `__usage_spec__` requests
    /// the generated completion scripts and the `usage` CLI send, renders
    /// `--help`/`--version`, and preserves the relative order of `-c` and
    /// `-f` occurrences (the parser binds each flag separately; a second pass
    /// over the event stream restores the interleaving).
    pub fn parse_from_argv<I, T>(argv: I) -> Result<Self, ParseExit>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let owned: Vec<OsString> = argv.into_iter().map(Into::into).collect();
        let refs: Vec<&OsStr> = owned.iter().map(OsString::as_os_str).collect();
        let words = refs.get(1..).unwrap_or(&[]);

        if let Some(reply) = Root::completion_request(owned.get(1..).unwrap_or(&[])) {
            return Err(ParseExit::Output(reply));
        }
        if let Some(spec) = Root::spec_request(words) {
            return Err(ParseExit::Output(spec));
        }

        let root = match Root::parse_from_argv(&refs) {
            Ok(root) => root,
            Err(usage::Error::Help { cmd, long }) => {
                return Err(ParseExit::Output(
                    Root::render_help(cmd, long).unwrap_or_default(),
                ));
            }
            Err(usage::Error::Version { .. }) => {
                return Err(ParseExit::Output(Self::version_line()));
            }
            Err(error) => return Err(ParseExit::Usage(Root::render_failure(&refs, &error))),
        };

        let mut args = root.args;
        args.subcommand = root.subcommand;
        args.ordered_sources = ordered_sources(words, &args);
        Ok(args)
    }

    /// The root help page (`dbcrust -h`).
    pub fn help_text() -> String {
        Root::render_help(Root::command(), false).unwrap_or_default()
    }

    /// What `--version` prints.
    pub fn version_line() -> String {
        format!("dbcrust {}\n", env!("CARGO_PKG_VERSION"))
    }

    /// A completion script for `binary_name` (the generated script calls
    /// `binary_name __complete_word__ …` back at completion time, so the
    /// `dbc` binary registers — and answers — under its own name).
    pub fn completion_script(shell: usage::complete::Shell, binary_name: &str) -> String {
        if binary_name == "dbcrust" {
            Root::completion_script(shell)
        } else {
            usage::script::script_for(binary_name, binary_name, shell)
        }
    }

    /// The CLI grammar as a usage spec (KDL), for `usage g markdown|manpage`.
    pub fn usage_spec() -> String {
        Root::to_kdl()
    }
}

/// Walk the parser's event stream to recover the argv order of `-c`/`-f`
/// occurrences, then zip it with the already-bound values.
fn ordered_sources(words: &[&OsStr], args: &Args) -> Vec<OneShotSource> {
    let mut kinds = Vec::new();
    let mut parser = usage::Parser::new(Root::command(), words);
    while let Some(Ok(event)) = parser.next_event() {
        if let usage::Event::Flag {
            flag,
            value: Some(_),
            ..
        } = event
        {
            match flag.name {
                "command" => kinds.push(SourceKind::Command),
                "file" => kinds.push(SourceKind::File),
                _ => {}
            }
        }
    }
    let mut commands = args.command.iter().cloned();
    let mut files = args.file.iter().cloned();
    kinds
        .into_iter()
        .filter_map(|kind| match kind {
            SourceKind::Command => commands.next().map(OneShotSource::Command),
            SourceKind::File => files.next().map(OneShotSource::File),
        })
        .collect()
}

enum SourceKind {
    Command,
    File,
}

/// Top-level subcommands. A bare word matching a subcommand name wins over the
/// positional URL — a relative SQLite file literally named `config` must be
/// opened as `sqlite://config`.
#[derive(Subcommands, Clone, Debug)]
pub enum CliCommand {
    /// View and edit DBCrust configuration (no database connection needed)
    #[usage(after_help = "Examples:
  dbcrust config                    # interactive menu
  dbcrust config show
  dbcrust config get logging.level
  dbcrust config set logging.level debug
  dbcrust config edit")]
    Config {
        #[usage(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Print the guide for AI coding agents
    ///
    /// URL schemes, one-shot flags, output formats, exit codes, safety rails.
    #[usage(after_help = "")]
    Agents,
}

#[derive(Subcommands, Clone, Debug)]
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
        ///
        /// `double_dash = "automatic"`: once the key is bound, flag parsing
        /// ends so a value like `-RFX` is taken literally.
        #[usage(double_dash = "automatic")]
        key: String,
        /// New value (quote values containing spaces)
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
            .field("read_only", &self.read_only)
            .field("timeout", &self.timeout)
            .field("max_rows", &self.max_rows)
            .field("no_input", &self.no_input)
            .field("subcommand", &self.subcommand)
            .finish()
    }
}

/// Supported shells for completion generation (`--completions`).
#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    #[usage(aliases("powershell", "pwsh"))]
    PowerShell,
    #[usage(alias = "nu")]
    Nushell,
}

impl From<Shell> for usage::complete::Shell {
    fn from(shell: Shell) -> Self {
        match shell {
            Shell::Bash => Self::Bash,
            Shell::Zsh => Self::Zsh,
            Shell::Fish => Self::Fish,
            Shell::PowerShell => Self::PowerShell,
            Shell::Nushell => Self::Nu,
        }
    }
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

    #[test]
    fn test_no_args() {
        let args = Args::parse_from_argv(["dbcrust"]).unwrap();
        assert!(args.connection_url.is_none());
        assert!(args.command.is_empty());
        assert!(!args.update);
    }

    #[test]
    fn test_update_flag() {
        let args = Args::parse_from_argv(["dbcrust", "--update"]).unwrap();
        assert!(args.update);
        assert!(args.connection_url.is_none());
    }

    #[test]
    fn test_connection_url() {
        let args = Args::parse_from_argv(["dbcrust", "postgres://localhost/test"]).unwrap();
        assert_eq!(
            args.connection_url.as_deref(),
            Some("postgres://localhost/test")
        );
    }

    #[test]
    fn test_single_command() {
        let args = Args::parse_from_argv(["dbcrust", "-c", "SELECT 1"]).unwrap();
        assert_eq!(args.command, vec!["SELECT 1"]);
    }

    #[test]
    fn test_multiple_commands() {
        let args = Args::parse_from_argv(["dbcrust", "-c", "\\dt", "-c", "SELECT 1"]).unwrap();
        assert_eq!(args.command, vec!["\\dt", "SELECT 1"]);
    }

    #[test]
    fn test_ssh_tunnel() {
        let args = Args::parse_from_argv([
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
        let args = Args::parse_from_argv(["dbcrust", "--completions", "bash"]).unwrap();
        assert_eq!(args.completions, Some(Shell::Bash));
    }

    #[test]
    fn test_agents_subcommand() {
        let args = Args::parse_from_argv(["dbcrust", "agents"]).unwrap();
        assert!(matches!(args.subcommand, Some(CliCommand::Agents)));
        assert!(args.connection_url.is_none());
    }

    #[test]
    fn test_config_subcommand_bare() {
        let args = Args::parse_from_argv(["dbcrust", "config"]).unwrap();
        assert!(matches!(
            args.subcommand,
            Some(CliCommand::Config { action: None })
        ));
        assert!(args.connection_url.is_none());
    }

    #[test]
    fn test_config_subcommand_get() {
        let args = Args::parse_from_argv(["dbcrust", "config", "get", "logging.level"]).unwrap();
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
            Args::parse_from_argv(["dbcrust", "config", "set", "default_limit", "50"]).unwrap();
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
        let args =
            Args::parse_from_argv(["dbcrust", "config", "set", "pager_command", "less -RFX"])
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
        // Programmatically built Args have no ordered_sources — the fallback
        // puts files before commands
        let mut args =
            Args::parse_from_argv(["dbcrust", "-c", "SELECT 1", "-f", "a.sql", "url"]).unwrap();
        args.ordered_sources.clear();
        assert_eq!(
            args.one_shot_sources(),
            vec![
                OneShotSource::File("a.sql".to_string()),
                OneShotSource::Command("SELECT 1".to_string()),
            ]
        );
    }

    #[test]
    fn test_read_only_flag_forms() {
        // Bare flag must NOT swallow the URL
        let args = Args::parse_from_argv(["dbcrust", "--read-only", "postgres://h/db"]).unwrap();
        assert_eq!(args.read_only, Some(true));
        assert_eq!(args.connection_url.as_deref(), Some("postgres://h/db"));

        // Explicit override of a config default
        let args = Args::parse_from_argv(["dbcrust", "--read-only=false", "url"]).unwrap();
        assert_eq!(args.read_only, Some(false));

        // Absent → defer to config
        let args = Args::parse_from_argv(["dbcrust", "url"]).unwrap();
        assert_eq!(args.read_only, None);

        // Space-separated value is rejected (require_equals)
        assert!(Args::parse_from_argv(["dbcrust", "--read-only", "false", "url"]).is_err());
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
        let args = Args::parse_from_argv(["dbcrust", "-o", "json", "sqlite://test.db"]).unwrap();
        assert_eq!(args.format, Some(OutputFormat::Json));
        assert_eq!(args.connection_url.as_deref(), Some("sqlite://test.db"));

        let args = Args::parse_from_argv(["dbcrust", "--format", "csv"]).unwrap();
        assert_eq!(args.format, Some(OutputFormat::Csv));

        let args = Args::parse_from_argv(["dbcrust"]).unwrap();
        assert_eq!(args.format, None);

        assert!(Args::parse_from_argv(["dbcrust", "-o", "yaml"]).is_err());
    }

    #[test]
    fn test_help_and_version_are_output_exits() {
        let Err(exit) = Args::parse_from_argv(["dbcrust", "--help"]) else {
            panic!("--help must not parse into Args");
        };
        assert_eq!(exit.exit_code(), 0);
        let ParseExit::Output(text) = exit else {
            panic!("--help is stdout output");
        };
        assert!(text.contains("Usage: dbcrust"));
        assert!(text.contains("--read-only"));
        assert!(
            text.contains("dbcrust session://prod"),
            "after_help examples"
        );

        let Err(ParseExit::Output(short)) = Args::parse_from_argv(["dbcrust", "-h"]) else {
            panic!("-h is stdout output");
        };
        assert!(short.len() < text.len(), "-h is the short page");
        assert_eq!(Args::help_text(), short);

        for flag in ["--version", "-V"] {
            let Err(exit) = Args::parse_from_argv(["dbcrust", flag]) else {
                panic!("{flag} must not parse into Args");
            };
            assert_eq!(exit, ParseExit::Output(Args::version_line()));
        }
        assert_eq!(
            Args::version_line(),
            format!("dbcrust {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn test_subcommand_help_carries_its_own_examples() {
        let Err(ParseExit::Output(text)) = Args::parse_from_argv(["dbcrust", "config", "-h"])
        else {
            panic!("config -h is stdout output");
        };
        assert!(text.contains("dbcrust config set logging.level debug"));
        assert!(!text.contains("dbcrust session://prod"));
    }

    #[test]
    fn test_usage_errors_exit_2() {
        let Err(exit) = Args::parse_from_argv(["dbcrust", "--wat"]) else {
            panic!("unknown flag must be rejected");
        };
        assert_eq!(exit.exit_code(), 2);
        let ParseExit::Usage(text) = exit else {
            panic!("unknown flag is a usage error");
        };
        assert!(text.contains("--wat"), "{text}");
        assert!(text.contains("--help"), "{text}");

        assert!(Args::parse_from_argv(["dbcrust", "config", "set", "only_key"]).is_err());
        assert!(Args::parse_from_argv(["dbcrust", "config", "bogus"]).is_err());
    }

    #[test]
    fn test_hidden_completion_and_spec_requests() {
        let Err(ParseExit::Output(reply)) = Args::parse_from_argv([
            "dbcrust",
            "__complete_word__",
            "--shell",
            "zsh",
            "--line",
            "dbcrust --for",
        ]) else {
            panic!("completion request is stdout output");
        };
        assert!(reply.contains("--format"), "{reply}");

        let Err(ParseExit::Output(spec)) = Args::parse_from_argv(["dbcrust", "__usage_spec__"])
        else {
            panic!("spec request is stdout output");
        };
        assert!(spec.starts_with("name dbcrust"), "{spec}");
        assert!(spec.contains("cmd config"), "{spec}");
        assert_eq!(spec, Args::usage_spec());
    }

    #[test]
    fn test_completion_script_targets_binary_name() {
        let dbcrust = Args::completion_script(usage::complete::Shell::Bash, "dbcrust");
        assert!(dbcrust.contains("'dbcrust' __complete_word__"));
        let dbc = Args::completion_script(usage::complete::Shell::Bash, "dbc");
        assert!(dbc.contains("'dbc' __complete_word__"));
        assert!(!dbc.contains("'dbcrust' __complete_word__"));
    }

    #[test]
    fn test_shell_value_enum_aliases() {
        let args = Args::parse_from_argv(["dbcrust", "--completions", "nu"]).unwrap();
        assert_eq!(args.completions, Some(Shell::Nushell));
        let args = Args::parse_from_argv(["dbcrust", "--completions", "pwsh"]).unwrap();
        assert_eq!(args.completions, Some(Shell::PowerShell));
        let args = Args::parse_from_argv(["dbcrust", "--completions", "power-shell"]).unwrap();
        assert_eq!(args.completions, Some(Shell::PowerShell));
        assert!(Args::parse_from_argv(["dbcrust", "--completions", "elvish"]).is_err());
    }

    #[test]
    fn test_config_set_bare_hyphen_value() {
        let args =
            Args::parse_from_argv(["dbcrust", "config", "set", "pager_command", "-RFX"]).unwrap();
        let Some(CliCommand::Config {
            action: Some(ConfigAction::Set { value, .. }),
        }) = args.subcommand
        else {
            panic!("expected config set subcommand");
        };
        assert_eq!(value, "-RFX");
    }

    #[test]
    fn test_connection_url_still_wins_over_subcommand() {
        // A URL must not be mistaken for a subcommand.
        let args = Args::parse_from_argv(["dbcrust", "postgres://localhost/test"]).unwrap();
        assert!(args.subcommand.is_none());
        assert_eq!(
            args.connection_url.as_deref(),
            Some("postgres://localhost/test")
        );
    }
}
