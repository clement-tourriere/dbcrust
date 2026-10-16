//! Command-line grammar, declared with the same `usage` derive the main
//! `dbcrust` binary uses.

use std::ffi::{OsStr, OsString};

use usage::{Subcommands, ValueEnum};

const ROOT_EXAMPLES: &str = "Examples:
  dbcrust-agent list
  dbcrust-agent tables prod
  dbcrust-agent describe prod users
  dbcrust-agent query prod -c 'SELECT * FROM users' -o json --max-rows 50
  dbcrust-agent explain prod -c 'SELECT * FROM users WHERE id = 1'
  dbcrust-agent mcp                  # MCP server on stdio
  dbcrust-agent doctor               # check the broker connection

The broker (`dbcrust broker serve`, started by the database owner) holds the
credentials; this client only ever names owner-enrolled aliases.";

// Plain comments on purpose: a doc comment on the root would become the
// CLI's long_about.
#[derive(usage::Cli)]
#[usage(
    bin = "dbcrust-agent",
    version,
    usage = "Usage: dbcrust-agent [FLAGS] <COMMAND>",
    about = "Credential-free DBCrust client for AI agents: queries owner-enrolled aliases through the DBCrust broker",
    after_help = ROOT_EXAMPLES,
    unknown_flags = "error"
)]
struct Root {
    /// Path of the broker's Unix socket (used alone: no fallback)
    ///
    /// Default: $DBCRUST_BROKER_SOCKET alone when set, else the first existing
    /// of /var/run/dbcrust/broker.sock and the per-user socket.
    #[usage(long, global, value_name = "PATH")]
    socket: Option<String>,

    /// Label recorded in the broker's audit log (default: dbcrust-agent, or
    /// the MCP client's own name under `mcp`)
    #[usage(long, global, value_name = "LABEL")]
    client: Option<String>,

    #[usage(subcommand)]
    command: Option<Command>,
}

/// Output formats of the one-shot commands.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Plain aligned text (default)
    #[default]
    Table,
    /// RFC 4180 CSV with a header row
    Csv,
    /// One `{"columns":[…],"rows":[[…]],"row_count":n,"truncated":bool}` line
    Json,
    /// One JSON object per row; trailing `{"_truncated":true,"_row_count":n}` when truncated
    Jsonl,
}

/// Flags shared by commands that print a result.
#[derive(usage::Args, Clone, Debug)]
pub struct Printing {
    /// Output format
    #[usage(short = 'o', long = "format", value_enum, value_name = "FORMAT")]
    pub format: Option<OutputFormat>,
}

#[derive(usage::Args, Clone, Debug)]
pub struct ListArgs {
    #[usage(flatten)]
    pub printing: Printing,
}

#[derive(usage::Args, Clone, Debug)]
pub struct TablesArgs {
    /// Connection alias (see `dbcrust-agent list`)
    #[usage(value_name = "ALIAS")]
    pub alias: String,
    #[usage(flatten)]
    pub printing: Printing,
}

#[derive(usage::Args, Clone, Debug)]
pub struct DescribeArgs {
    /// Connection alias (see `dbcrust-agent list`)
    #[usage(value_name = "ALIAS")]
    pub alias: String,
    /// Table name, optionally schema-qualified
    #[usage(value_name = "TABLE")]
    pub table: String,
    #[usage(flatten)]
    pub printing: Printing,
}

#[derive(usage::Args, Clone, Debug)]
pub struct QueryArgs {
    /// Connection alias (see `dbcrust-agent list`)
    #[usage(value_name = "ALIAS")]
    pub alias: String,
    /// SQL statement to run (one statement)
    #[usage(short = 'c', long, value_name = "SQL")]
    pub command: String,
    /// Lower the row limit for this query (the broker's limit still applies)
    #[usage(long, value_name = "N")]
    pub max_rows: Option<u32>,
    /// Lower the timeout for this query, in seconds
    #[usage(long, value_name = "SECS")]
    pub timeout: Option<u32>,
    #[usage(flatten)]
    pub printing: Printing,
}

#[derive(usage::Args, Clone, Debug)]
pub struct ExplainArgs {
    /// Connection alias (see `dbcrust-agent list`)
    #[usage(value_name = "ALIAS")]
    pub alias: String,
    /// SQL statement to explain
    #[usage(short = 'c', long, value_name = "SQL")]
    pub command: String,
    #[usage(flatten)]
    pub printing: Printing,
}

#[derive(Subcommands, Clone, Debug)]
pub enum Command {
    /// List the connection aliases the owner enrolled, with their limits
    List(ListArgs),
    /// List the tables of an alias
    Tables(TablesArgs),
    /// Describe the columns of a table
    Describe(DescribeArgs),
    /// Run one SQL statement (limits and read-only rules are enforced by the broker)
    Query(QueryArgs),
    /// Show the query plan of a statement
    Explain(ExplainArgs),
    /// Serve the Model Context Protocol on stdio (for Claude Code, Codex, Cursor)
    Mcp,
    /// Check that a broker is reachable and speaks our protocol
    Doctor,
}

impl Command {
    /// The `-o` format of this command, `None` when it has none.
    pub fn format(&self) -> Option<OutputFormat> {
        let printing = match self {
            Command::List(a) => &a.printing,
            Command::Tables(a) => &a.printing,
            Command::Describe(a) => &a.printing,
            Command::Query(a) => &a.printing,
            Command::Explain(a) => &a.printing,
            Command::Mcp | Command::Doctor => return None,
        };
        printing.format
    }
}

/// A successfully parsed command line.
#[derive(Debug, Clone)]
pub struct Cli {
    pub socket: Option<String>,
    pub client: Option<String>,
    pub command: Command,
}

/// Why no [`Cli`] was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseExit {
    /// `--help` / `--version` / bare invocation: stdout, exit 0
    Output(String),
    /// Malformed command line: stderr, exit 2
    Usage(String),
}

/// Parse a full argv (program name first).
pub fn parse<I, T>(argv: I) -> Result<Cli, ParseExit>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let owned: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let refs: Vec<&OsStr> = owned.iter().map(OsString::as_os_str).collect();
    match Root::parse_from_argv(&refs) {
        Ok(Root {
            socket,
            client,
            command: Some(command),
        }) => Ok(Cli {
            socket,
            client,
            command,
        }),
        Ok(_) => Err(ParseExit::Output(
            Root::render_help(Root::command(), false).unwrap_or_default(),
        )),
        Err(usage::Error::Help { cmd, long }) => Err(ParseExit::Output(
            Root::render_help(cmd, long).unwrap_or_default(),
        )),
        Err(usage::Error::Version { .. }) => Err(ParseExit::Output(format!(
            "dbcrust-agent {}\n",
            env!("CARGO_PKG_VERSION")
        ))),
        Err(error) => Err(ParseExit::Usage(Root::render_failure(&refs, &error))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(args: &[&str]) -> Cli {
        let argv = std::iter::once("dbcrust-agent").chain(args.iter().copied());
        parse(argv).unwrap_or_else(|e| panic!("{args:?}: {e:?}"))
    }

    fn fails(args: &[&str]) -> ParseExit {
        let argv = std::iter::once("dbcrust-agent").chain(args.iter().copied());
        parse(argv).unwrap_err()
    }

    #[test]
    fn query_with_all_flags() {
        let cli = ok(&[
            "--socket",
            "/tmp/b.sock",
            "--client",
            "me",
            "query",
            "prod",
            "-c",
            "SELECT 1",
            "-o",
            "jsonl",
            "--max-rows",
            "5",
            "--timeout",
            "7",
        ]);
        assert_eq!(cli.socket.as_deref(), Some("/tmp/b.sock"));
        assert_eq!(cli.client.as_deref(), Some("me"));
        let Command::Query(q) = cli.command else {
            panic!("query expected")
        };
        assert_eq!(q.alias, "prod");
        assert_eq!(q.command, "SELECT 1");
        assert_eq!(q.max_rows, Some(5));
        assert_eq!(q.timeout, Some(7));
        assert_eq!(q.printing.format, Some(OutputFormat::Jsonl));
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = ok(&["list", "--socket", "/tmp/x.sock"]);
        assert_eq!(cli.socket.as_deref(), Some("/tmp/x.sock"));
        assert!(matches!(cli.command, Command::List(_)));
    }

    #[test]
    fn simple_commands() {
        assert!(matches!(ok(&["mcp"]).command, Command::Mcp));
        assert!(matches!(ok(&["doctor"]).command, Command::Doctor));
        let Command::Describe(d) = ok(&["describe", "prod", "public.users"]).command else {
            panic!()
        };
        assert_eq!(
            (d.alias.as_str(), d.table.as_str()),
            ("prod", "public.users")
        );
        let Command::Tables(t) = ok(&["tables", "prod", "-o", "csv"]).command else {
            panic!()
        };
        assert_eq!(t.printing.format, Some(OutputFormat::Csv));
        let Command::Explain(e) = ok(&["explain", "prod", "-c", "SELECT 1"]).command else {
            panic!()
        };
        assert_eq!(e.command, "SELECT 1");
    }

    #[test]
    fn usage_errors() {
        assert!(matches!(fails(&["query", "prod"]), ParseExit::Usage(_)));
        assert!(matches!(fails(&["query"]), ParseExit::Usage(_)));
        assert!(matches!(fails(&["bogus"]), ParseExit::Usage(_)));
        assert!(matches!(fails(&["list", "--nope"]), ParseExit::Usage(_)));
        assert!(matches!(
            fails(&["query", "p", "-c", "x", "-o", "xml"]),
            ParseExit::Usage(_)
        ));
    }

    #[test]
    fn help_and_version_and_bare_invocation() {
        let ParseExit::Output(help) = fails(&["--help"]) else {
            panic!()
        };
        assert!(help.contains("dbcrust-agent"));
        let ParseExit::Output(version) = fails(&["--version"]) else {
            panic!()
        };
        assert!(version.starts_with("dbcrust-agent "));
        assert!(matches!(fails(&[]), ParseExit::Output(_)));
    }

    #[test]
    fn no_environment_driven_flags() {
        // The client may only read four env vars (see socket.rs); the grammar
        // must not smuggle in more through `env = "…"`.
        let spec = Root::to_kdl();
        assert!(!spec.contains("env="), "{spec}");
    }
}
