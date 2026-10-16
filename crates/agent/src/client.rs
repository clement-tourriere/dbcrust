//! Connection to the broker: handshake plus typed request helpers.

use std::path::Path;
use std::time::Duration;

use dbcrust_agent_protocol::aio::{read_message, write_message};
use dbcrust_agent_protocol::{
    ClientMessage, ColumnInfo, ConnectionSummary, ErrorCode, FrameError, PROTOCOL_VERSION,
    ServerMessage, TableSummary,
};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::error::AgentError;

/// How long connecting and the `hello`/`welcome` exchange may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest we wait for an answer when the caller set no query timeout (the
/// broker enforces the real per-alias limits; this only stops a hung broker
/// from hanging the agent forever).
const DEFAULT_DEADLINE: Duration = Duration::from_secs(900);
/// Slack added on top of a caller-supplied query timeout for broker overhead.
const DEADLINE_SLACK: Duration = Duration::from_secs(30);

/// A tabular answer: `query`, `explain`, and (rendered) the listing commands.
/// Field order is the one-shot JSON envelope's order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub row_count: u64,
    pub truncated: bool,
}

#[cfg(unix)]
pub type Transport = tokio::net::UnixStream;
/// Never constructed: every command stops at the platform check first.
#[cfg(not(unix))]
pub type Transport = tokio::io::DuplexStream;

/// Connect to the broker socket at `path` and complete the handshake.
pub async fn connect(path: &Path, label: &str) -> Result<BrokerClient<Transport>, AgentError> {
    #[cfg(unix)]
    {
        let stream = timeout(HANDSHAKE_TIMEOUT, tokio::net::UnixStream::connect(path))
            .await
            .map_err(|_| {
                AgentError::connection(format!(
                    "timed out connecting to the broker socket {}",
                    path.display()
                ))
            })?
            .map_err(|e| {
                AgentError::connection(format!(
                    "cannot connect to the broker socket {}: {e}",
                    path.display()
                ))
            })?;
        BrokerClient::handshake(stream, label).await
    }
    #[cfg(not(unix))]
    {
        let _ = (path, label);
        Err(AgentError::unsupported_platform())
    }
}

pub struct BrokerClient<S> {
    stream: S,
    next_id: u64,
    broker_version: String,
}

fn frame_error(error: FrameError) -> AgentError {
    match error {
        FrameError::Eof | FrameError::Truncated | FrameError::Io(_) => {
            AgentError::connection(format!("lost the connection to the broker: {error}"))
        }
        FrameError::TooLarge { .. } | FrameError::Json(_) => {
            AgentError::protocol(format!("the broker sent an invalid message: {error}"))
        }
    }
}

/// Keep the audit label printable and bounded; it is informational only.
pub fn sanitize_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .filter(|c| !c.is_control())
        .take(128)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "dbcrust-agent".to_string()
    } else {
        cleaned.to_string()
    }
}

/// How long to wait for the answer to a request with this query timeout.
pub fn deadline_for(timeout_secs: Option<u32>) -> Duration {
    match timeout_secs {
        Some(secs) => Duration::from_secs(u64::from(secs)) + DEADLINE_SLACK,
        None => DEFAULT_DEADLINE,
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> BrokerClient<S> {
    /// Send `hello` and wait for `welcome`.
    pub async fn handshake(mut stream: S, label: &str) -> Result<Self, AgentError> {
        let hello = ClientMessage::Hello {
            protocol: PROTOCOL_VERSION,
            client: sanitize_label(label),
        };
        let welcome = timeout(HANDSHAKE_TIMEOUT, async {
            write_message(&mut stream, &hello).await?;
            read_message::<_, ServerMessage>(&mut stream).await
        })
        .await
        .map_err(|_| AgentError::connection("timed out waiting for the broker's welcome"))?
        .map_err(frame_error)?;

        match welcome {
            ServerMessage::Welcome {
                protocol,
                broker_version,
            } => {
                if protocol != PROTOCOL_VERSION {
                    return Err(AgentError::new(
                        ErrorCode::UnsupportedVersion,
                        format!(
                            "broker speaks protocol {protocol}, this client speaks {PROTOCOL_VERSION}; upgrade dbcrust-agent or the broker"
                        ),
                    ));
                }
                Ok(Self {
                    stream,
                    next_id: 1,
                    broker_version,
                })
            }
            ServerMessage::Error { code, message, .. } => Err(AgentError::new(code, message)),
            other => Err(AgentError::protocol(format!(
                "expected `welcome` from the broker, got {}",
                variant_name(&other)
            ))),
        }
    }

    pub fn broker_version(&self) -> &str {
        &self.broker_version
    }

    async fn request(
        &mut self,
        build: impl FnOnce(u64) -> ClientMessage,
        deadline: Duration,
    ) -> Result<ServerMessage, AgentError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = build(id);
        let response = timeout(deadline, async {
            write_message(&mut self.stream, &request).await?;
            read_message::<_, ServerMessage>(&mut self.stream).await
        })
        .await
        .map_err(|_| {
            AgentError::new(
                ErrorCode::Timeout,
                format!("no answer from the broker within {}s", deadline.as_secs()),
            )
        })?
        .map_err(frame_error)?;

        if let Some(answered) = response.id()
            && answered != id
        {
            return Err(AgentError::protocol(format!(
                "the broker answered request {answered} while {id} was pending"
            )));
        }
        match response {
            ServerMessage::Error { code, message, .. } => Err(AgentError::new(code, message)),
            other => Ok(other),
        }
    }

    pub async fn list_connections(&mut self) -> Result<Vec<ConnectionSummary>, AgentError> {
        match self
            .request(|id| ClientMessage::ListConnections { id }, DEFAULT_DEADLINE)
            .await?
        {
            ServerMessage::Connections { connections, .. } => Ok(connections),
            other => Err(unexpected("connections", &other)),
        }
    }

    pub async fn list_tables(&mut self, alias: &str) -> Result<Vec<TableSummary>, AgentError> {
        let alias = alias.to_string();
        match self
            .request(
                |id| ClientMessage::ListTables { id, alias },
                DEFAULT_DEADLINE,
            )
            .await?
        {
            ServerMessage::Tables { tables, .. } => Ok(tables),
            other => Err(unexpected("tables", &other)),
        }
    }

    pub async fn describe_table(
        &mut self,
        alias: &str,
        table: &str,
    ) -> Result<(String, Vec<ColumnInfo>), AgentError> {
        let (alias, table) = (alias.to_string(), table.to_string());
        match self
            .request(
                |id| ClientMessage::DescribeTable { id, alias, table },
                DEFAULT_DEADLINE,
            )
            .await?
        {
            ServerMessage::TableDescription { table, columns, .. } => Ok((table, columns)),
            other => Err(unexpected("table_description", &other)),
        }
    }

    pub async fn query(
        &mut self,
        alias: &str,
        sql: &str,
        max_rows: Option<u32>,
        timeout_secs: Option<u32>,
    ) -> Result<ResultSet, AgentError> {
        let (alias, sql) = (alias.to_string(), sql.to_string());
        let response = self
            .request(
                |id| ClientMessage::Query {
                    id,
                    alias,
                    sql,
                    max_rows,
                    timeout_secs,
                },
                deadline_for(timeout_secs),
            )
            .await?;
        into_result_set(response)
    }

    pub async fn explain(&mut self, alias: &str, sql: &str) -> Result<ResultSet, AgentError> {
        let (alias, sql) = (alias.to_string(), sql.to_string());
        let response = self
            .request(
                |id| ClientMessage::Explain { id, alias, sql },
                DEFAULT_DEADLINE,
            )
            .await?;
        into_result_set(response)
    }
}

fn into_result_set(message: ServerMessage) -> Result<ResultSet, AgentError> {
    match message {
        ServerMessage::QueryResult {
            columns,
            rows,
            row_count,
            truncated,
            ..
        } => Ok(ResultSet {
            columns,
            rows,
            row_count,
            truncated,
        }),
        other => Err(unexpected("query_result", &other)),
    }
}

fn variant_name(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::Welcome { .. } => "welcome",
        ServerMessage::Connections { .. } => "connections",
        ServerMessage::Tables { .. } => "tables",
        ServerMessage::TableDescription { .. } => "table_description",
        ServerMessage::QueryResult { .. } => "query_result",
        ServerMessage::Error { .. } => "error",
    }
}

fn unexpected(wanted: &str, got: &ServerMessage) -> AgentError {
    AgentError::protocol(format!(
        "expected `{wanted}` from the broker, got `{}`",
        variant_name(got)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("claude-code", "claude-code")]
    #[case("  padded  ", "padded")]
    #[case("", "dbcrust-agent")]
    #[case("bad\u{0}\n\tlabel", "badlabel")]
    fn labels_are_sanitized(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(sanitize_label(raw), expected);
    }

    #[test]
    fn long_labels_are_truncated() {
        assert_eq!(sanitize_label(&"x".repeat(1000)).chars().count(), 128);
    }

    #[test]
    fn deadlines() {
        assert_eq!(deadline_for(Some(10)), Duration::from_secs(40));
        assert_eq!(deadline_for(None), DEFAULT_DEADLINE);
    }

    #[tokio::test]
    async fn handshake_rejects_other_protocol_versions() {
        let (client_end, mut broker_end) = tokio::io::duplex(4096);
        let broker = tokio::spawn(async move {
            let _hello: ClientMessage = read_message(&mut broker_end).await.unwrap();
            write_message(
                &mut broker_end,
                &ServerMessage::Welcome {
                    protocol: PROTOCOL_VERSION + 1,
                    broker_version: "9.9.9".into(),
                },
            )
            .await
            .unwrap();
        });
        let error = BrokerClient::handshake(client_end, "t")
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.code,
            crate::error::Code::Remote(ErrorCode::UnsupportedVersion)
        );
        broker.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_surfaces_broker_error() {
        let (client_end, mut broker_end) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _hello: ClientMessage = read_message(&mut broker_end).await.unwrap();
            write_message(
                &mut broker_end,
                &ServerMessage::Error {
                    id: None,
                    code: ErrorCode::BrokerLocked,
                    message: "locked".into(),
                },
            )
            .await
            .unwrap();
        });
        let error = BrokerClient::handshake(client_end, "t")
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.code,
            crate::error::Code::Remote(ErrorCode::BrokerLocked)
        );
        assert_eq!(error.exit_code(), 3);
    }

    #[tokio::test]
    async fn closed_connection_is_a_connection_error() {
        let (client_end, broker_end) = tokio::io::duplex(4096);
        drop(broker_end);
        let error = BrokerClient::handshake(client_end, "t")
            .await
            .err()
            .unwrap();
        assert_eq!(error.exit_code(), 3);
    }

    #[tokio::test]
    async fn mismatched_response_id_is_a_protocol_error() {
        let (client_end, mut broker_end) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _hello: ClientMessage = read_message(&mut broker_end).await.unwrap();
            write_message(
                &mut broker_end,
                &ServerMessage::Welcome {
                    protocol: PROTOCOL_VERSION,
                    broker_version: "1".into(),
                },
            )
            .await
            .unwrap();
            let _req: ClientMessage = read_message(&mut broker_end).await.unwrap();
            write_message(
                &mut broker_end,
                &ServerMessage::Connections {
                    id: 999,
                    connections: vec![],
                },
            )
            .await
            .unwrap();
        });
        let mut client = BrokerClient::handshake(client_end, "t").await.unwrap();
        let error = client.list_connections().await.unwrap_err();
        assert_eq!(
            error.code,
            crate::error::Code::Remote(ErrorCode::ProtocolError)
        );
    }
}
