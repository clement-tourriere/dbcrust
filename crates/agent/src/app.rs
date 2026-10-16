//! One-shot command execution and process wiring.

use std::path::PathBuf;

use tokio::io::{self, BufReader};

use crate::cli::{Cli, Command, OutputFormat};
use crate::client::{BrokerClient, Transport, connect};
use crate::error::AgentError;
use crate::output;
use crate::socket::{Env, SocketSearch};

const DEFAULT_LABEL: &str = "dbcrust-agent";

/// How to reach the broker and how to label ourselves to it.
pub struct Session {
    pub search: SocketSearch,
    /// `--client`, when given explicitly
    pub client: Option<String>,
}

impl Session {
    pub fn new(socket: Option<String>, client: Option<String>, env: Env) -> Self {
        Self {
            search: SocketSearch::new(socket.map(PathBuf::from), env),
            client,
        }
    }

    /// `--client`, else the MCP client's own name, else `dbcrust-agent`.
    fn label<'a>(&'a self, mcp_client_name: Option<&'a str>) -> &'a str {
        self.client
            .as_deref()
            .or(mcp_client_name)
            .unwrap_or(DEFAULT_LABEL)
    }

    /// Discover the socket, connect and handshake.
    pub async fn connect(
        &self,
        mcp_client_name: Option<&str>,
    ) -> Result<BrokerClient<Transport>, AgentError> {
        let path = self
            .search
            .discover()
            .map_err(|e| AgentError::connection(e.to_string()))?;
        connect(&path, self.label(mcp_client_name)).await
    }
}

/// What a command produced: text for stdout, and the failure, if any (the
/// text may be non-empty on failure — `doctor` reports how far it got).
#[derive(Debug, Default)]
pub struct Run {
    pub stdout: String,
    pub error: Option<AgentError>,
}

impl Run {
    fn from(result: Result<String, AgentError>) -> Self {
        match result {
            Ok(stdout) => Self {
                stdout,
                error: None,
            },
            Err(error) => Self {
                stdout: String::new(),
                error: Some(error),
            },
        }
    }
}

/// Execute a one-shot command (everything except `mcp`).
pub async fn execute(command: &Command, session: &Session) -> Run {
    match command {
        Command::Doctor => doctor(session).await,
        Command::Mcp => Run::from(Err(AgentError::usage("`mcp` is not a one-shot command"))),
        other => Run::from(one_shot(other, session).await),
    }
}

async fn one_shot(command: &Command, session: &Session) -> Result<String, AgentError> {
    let mut client = session.connect(None).await?;
    let result = match command {
        Command::List(_) => output::connections_result(&client.list_connections().await?),
        Command::Tables(args) => output::tables_result(&client.list_tables(&args.alias).await?),
        Command::Describe(args) => {
            let (_, columns) = client.describe_table(&args.alias, &args.table).await?;
            output::columns_result(&columns)
        }
        Command::Query(args) => {
            client
                .query(&args.alias, &args.command, args.max_rows, args.timeout)
                .await?
        }
        Command::Explain(args) => client.explain(&args.alias, &args.command).await?,
        Command::Mcp | Command::Doctor => unreachable!("handled by execute"),
    };
    Ok(output::render_result(
        &result,
        command.format().unwrap_or_default(),
    ))
}

async fn doctor(session: &Session) -> Run {
    let mut report = String::new();
    let path = match session.search.discover() {
        Ok(path) => path,
        Err(not_found) => {
            report.push_str("socket:    not found\n");
            return Run {
                stdout: report,
                error: Some(AgentError::connection(not_found.to_string())),
            };
        }
    };
    report.push_str(&format!("socket:    {}\n", path.display()));
    let mut client = match connect(&path, session.label(None)).await {
        Ok(client) => client,
        Err(error) => {
            report.push_str(&format!("handshake: failed ({})\n", error.code.as_str()));
            return Run {
                stdout: report,
                error: Some(error),
            };
        }
    };
    report.push_str(&format!(
        "handshake: ok (protocol {})\n",
        dbcrust_agent_protocol::PROTOCOL_VERSION
    ));
    report.push_str(&format!("broker:    {}\n", client.broker_version()));
    match client.list_connections().await {
        Ok(connections) => {
            report.push_str(&format!("aliases:   {}\n", connections.len()));
            Run {
                stdout: report,
                error: None,
            }
        }
        Err(error) => {
            report.push_str(&format!("aliases:   failed ({})\n", error.code.as_str()));
            Run {
                stdout: report,
                error: Some(error),
            }
        }
    }
}

/// Run a parsed command line to completion; returns the process exit status.
pub async fn run(cli: Cli, env: Env) -> u8 {
    let format: OutputFormat = cli.command.format().unwrap_or_default();

    if !cfg!(unix) {
        let error = AgentError::unsupported_platform();
        eprint!("{}", output::render_error(&error, format));
        return error.exit_code();
    }

    let session = Session::new(cli.socket, cli.client, env);

    if matches!(cli.command, Command::Mcp) {
        let stdin = BufReader::new(io::stdin());
        return match crate::mcp::serve(stdin, io::stdout(), &session).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("dbcrust-agent mcp: {e}");
                1
            }
        };
    }

    let run = execute(&cli.command, &session).await;
    print!("{}", run.stdout);
    match run.error {
        None => 0,
        Some(error) => {
            eprint!("{}", output::render_error(&error, format));
            error.exit_code()
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::cli;
    use crate::testutil::{FakeBroker, Script, session_for};
    use dbcrust_agent_protocol::{
        Access, ClientMessage, ColumnInfo, ConnectionSummary, ErrorCode, TableSummary,
    };
    use rstest::rstest;

    fn parse(args: &[&str]) -> Cli {
        cli::parse(std::iter::once("dbcrust-agent").chain(args.iter().copied())).unwrap()
    }

    async fn run_with(broker: &FakeBroker, args: &[&str]) -> Run {
        let cli = parse(args);
        let session = session_for(broker, cli.client.clone());
        execute(&cli.command, &session).await
    }

    fn summary(alias: &str) -> ConnectionSummary {
        ConnectionSummary {
            alias: alias.into(),
            engine: "postgresql".into(),
            access: Access::ReadOnly,
            native_read_only: true,
            description: Some("replica".into()),
            max_rows: 500,
            timeout_secs: 30,
        }
    }

    #[tokio::test]
    async fn list_prints_the_aliases() {
        let broker = FakeBroker::start(Script::default().with_connections(vec![summary("prod")]));
        let run = run_with(&broker, &["list", "-o", "json"]).await;
        assert!(run.error.is_none(), "{:?}", run.error);
        let v: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
        assert_eq!(v["rows"][0][0], "prod");
        assert_eq!(v["rows"][0][2], "read_only");
        assert_eq!(v["row_count"], 1);
    }

    #[tokio::test]
    async fn tables_and_describe() {
        let broker = FakeBroker::start(
            Script::default()
                .with_tables(vec![TableSummary {
                    schema: Some("public".into()),
                    name: "users".into(),
                    kind: Some("table".into()),
                }])
                .with_columns(vec![ColumnInfo {
                    name: "id".into(),
                    data_type: "bigint".into(),
                    nullable: Some(false),
                    default: None,
                    primary_key: true,
                }]),
        );
        let tables = run_with(&broker, &["tables", "prod", "-o", "csv"]).await;
        assert_eq!(tables.stdout, "schema,name,kind\npublic,users,table\n");
        let described = run_with(&broker, &["describe", "prod", "users", "-o", "jsonl"]).await;
        assert_eq!(
            described.stdout,
            "{\"name\":\"id\",\"data_type\":\"bigint\",\"nullable\":\"false\",\"default\":null,\"primary_key\":\"true\"}\n"
        );
        let seen = broker.requests();
        assert!(matches!(&seen[0], ClientMessage::ListTables { alias, .. } if alias == "prod"));
        assert!(matches!(&seen[1], ClientMessage::DescribeTable { table, .. } if table == "users"));
    }

    #[rstest]
    #[case("json")]
    #[case("jsonl")]
    #[case("csv")]
    #[case("table")]
    #[tokio::test]
    async fn query_in_every_format(#[case] format: &str) {
        let broker = FakeBroker::start(Script::default());
        let run = run_with(
            &broker,
            &[
                "query",
                "prod",
                "-c",
                "SELECT id, name FROM u",
                "-o",
                format,
                "--max-rows",
                "5",
                "--timeout",
                "9",
            ],
        )
        .await;
        assert!(run.error.is_none());
        match format {
            "json" => assert_eq!(
                run.stdout,
                "{\"columns\":[\"id\",\"name\"],\"rows\":[[\"1\",\"Alice\"],[\"2\",null]],\"row_count\":2,\"truncated\":false}\n"
            ),
            "jsonl" => assert_eq!(
                run.stdout,
                "{\"id\":\"1\",\"name\":\"Alice\"}\n{\"id\":\"2\",\"name\":null}\n"
            ),
            "csv" => assert_eq!(run.stdout, "id,name\n1,Alice\n2,\n"),
            _ => assert!(run.stdout.contains("Alice") && run.stdout.ends_with("(2 rows)\n")),
        }
        assert_eq!(
            broker.requests(),
            vec![ClientMessage::Query {
                id: 1,
                alias: "prod".into(),
                sql: "SELECT id, name FROM u".into(),
                max_rows: Some(5),
                timeout_secs: Some(9),
            }]
        );
    }

    #[tokio::test]
    async fn truncated_jsonl_ends_with_the_marker() {
        let broker = FakeBroker::start(Script::default().truncated());
        let run = run_with(&broker, &["query", "prod", "-c", "SELECT 1", "-o", "jsonl"]).await;
        assert_eq!(
            run.stdout.lines().last().unwrap(),
            r#"{"_truncated":true,"_row_count":2}"#
        );
    }

    #[tokio::test]
    async fn explain_uses_the_explain_request() {
        let broker = FakeBroker::start(Script::default());
        let run = run_with(&broker, &["explain", "prod", "-c", "SELECT 1"]).await;
        assert!(run.error.is_none());
        assert!(matches!(
            &broker.requests()[0],
            ClientMessage::Explain { sql, .. } if sql == "SELECT 1"
        ));
    }

    #[tokio::test]
    async fn client_label_is_sent_in_hello() {
        let broker = FakeBroker::start(Script::default());
        run_with(&broker, &["--client", "my-agent", "list"]).await;
        assert_eq!(broker.hello_labels(), vec!["my-agent".to_string()]);
        let broker = FakeBroker::start(Script::default());
        run_with(&broker, &["list"]).await;
        assert_eq!(broker.hello_labels(), vec!["dbcrust-agent".to_string()]);
    }

    #[rstest]
    #[case(ErrorCode::UnknownAlias, 5)]
    #[case(ErrorCode::Denied, 5)]
    #[case(ErrorCode::LimitExceeded, 5)]
    #[case(ErrorCode::ReadOnlyViolation, 4)]
    #[case(ErrorCode::DdlRefused, 4)]
    #[case(ErrorCode::MultiStatementRefused, 4)]
    #[case(ErrorCode::QueryError, 1)]
    #[case(ErrorCode::Timeout, 124)]
    #[case(ErrorCode::ConnectionError, 3)]
    #[case(ErrorCode::BrokerLocked, 3)]
    #[case(ErrorCode::ProtocolError, 2)]
    #[case(ErrorCode::UnsupportedVersion, 2)]
    #[case(ErrorCode::UnsupportedEngine, 1)]
    #[case(ErrorCode::Internal, 1)]
    #[tokio::test]
    async fn every_error_code_maps_to_its_exit_status(#[case] code: ErrorCode, #[case] exit: u8) {
        let broker = FakeBroker::start(Script::default().failing_with(code, "nope"));
        let run = run_with(
            &broker,
            &["query", "prod", "-c", "DROP TABLE x", "-o", "json"],
        )
        .await;
        let error = run.error.expect("broker error expected");
        assert_eq!(error.exit_code(), exit, "{code}");
        assert!(run.stdout.is_empty());
        let text = output::render_error(&error, OutputFormat::Json);
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["error"]["code"], code.as_str());
        assert_eq!(v["error"]["message"], "nope");
    }

    #[tokio::test]
    async fn missing_socket_is_exit_3_and_lists_paths() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            search: SocketSearch {
                flag: Some(dir.path().join("none.sock")),
                env: Env::default(),
                service_socket: dir.path().join("service.sock"),
            },
            client: None,
        };
        let run = execute(&parse(&["list"]).command, &session).await;
        let error = run.error.unwrap();
        assert_eq!(error.exit_code(), 3);
        assert!(error.message.contains("none.sock"));
        assert!(error.message.contains("dbcrust broker serve"));
    }

    #[tokio::test]
    async fn doctor_reports_socket_version_and_alias_count() {
        let broker = FakeBroker::start(Script::default().with_connections(vec![
            summary("a"),
            summary("b"),
            summary("c"),
        ]));
        let run = run_with(&broker, &["doctor"]).await;
        assert!(run.error.is_none());
        assert!(run.stdout.contains(&broker.path().display().to_string()));
        assert!(run.stdout.contains("handshake: ok (protocol 1)"));
        assert!(run.stdout.contains("broker:    9.9.9-test"));
        assert!(run.stdout.contains("aliases:   3"));
    }

    #[tokio::test]
    async fn doctor_without_a_broker_fails_with_exit_3() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            search: SocketSearch {
                flag: None,
                env: Env::default(),
                service_socket: dir.path().join("service.sock"),
            },
            client: None,
        };
        let run = execute(&Command::Doctor, &session).await;
        assert!(run.stdout.contains("socket:    not found"));
        assert_eq!(run.error.unwrap().exit_code(), 3);
    }

    #[tokio::test]
    async fn broker_dropping_the_connection_is_exit_3() {
        let broker = FakeBroker::start(Script::default().hang_up_after_hello());
        let run = run_with(&broker, &["list"]).await;
        assert_eq!(run.error.unwrap().exit_code(), 3);
    }
}
