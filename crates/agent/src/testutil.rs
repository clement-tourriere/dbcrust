//! In-process fake broker for tests: a real Unix socket in a tempdir that
//! speaks `dbcrust-agent-protocol`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dbcrust_agent_protocol::aio::{read_message, write_message};
use dbcrust_agent_protocol::{
    ClientMessage, ColumnInfo, ConnectionSummary, ErrorCode, PROTOCOL_VERSION, ServerMessage,
    TableSummary,
};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use crate::app::Session;
use crate::socket::{Env, SocketSearch};

/// What the fake broker answers.
#[derive(Clone, Default)]
pub struct Script {
    connections: Vec<ConnectionSummary>,
    tables: Vec<TableSummary>,
    columns: Vec<ColumnInfo>,
    truncated: bool,
    fail: Option<(ErrorCode, String)>,
    hang_up: bool,
}

impl Script {
    pub fn with_connections(mut self, connections: Vec<ConnectionSummary>) -> Self {
        self.connections = connections;
        self
    }

    pub fn with_tables(mut self, tables: Vec<TableSummary>) -> Self {
        self.tables = tables;
        self
    }

    pub fn with_columns(mut self, columns: Vec<ColumnInfo>) -> Self {
        self.columns = columns;
        self
    }

    pub fn truncated(mut self) -> Self {
        self.truncated = true;
        self
    }

    /// Answer every request with this error.
    pub fn failing_with(mut self, code: ErrorCode, message: &str) -> Self {
        self.fail = Some((code, message.to_string()));
        self
    }

    /// Close the connection right after reading `hello`.
    pub fn hang_up_after_hello(mut self) -> Self {
        self.hang_up = true;
        self
    }

    fn answer(&self, request: &ClientMessage) -> ServerMessage {
        let id = request.id().unwrap_or(0);
        if let Some((code, message)) = &self.fail {
            return ServerMessage::Error {
                id: Some(id),
                code: *code,
                message: message.clone(),
            };
        }
        match request {
            ClientMessage::Hello { .. } => unreachable!("handled before"),
            ClientMessage::ListConnections { .. } => ServerMessage::Connections {
                id,
                connections: self.connections.clone(),
            },
            ClientMessage::ListTables { .. } => ServerMessage::Tables {
                id,
                tables: self.tables.clone(),
            },
            ClientMessage::DescribeTable { table, .. } => ServerMessage::TableDescription {
                id,
                table: table.clone(),
                columns: self.columns.clone(),
            },
            ClientMessage::Query { .. } | ClientMessage::Explain { .. } => {
                ServerMessage::QueryResult {
                    id,
                    columns: vec!["id".into(), "name".into()],
                    rows: vec![
                        vec![Some("1".into()), Some("Alice".into())],
                        vec![Some("2".into()), None],
                    ],
                    row_count: 2,
                    truncated: self.truncated,
                }
            }
        }
    }
}

pub struct FakeBroker {
    _dir: tempfile::TempDir,
    path: PathBuf,
    requests: Arc<Mutex<Vec<ClientMessage>>>,
    hellos: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl FakeBroker {
    /// Must be called from inside a tokio runtime.
    pub fn start(script: Script) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let hellos = Arc::new(Mutex::new(Vec::new()));
        let task = {
            let (requests, hellos) = (requests.clone(), hellos.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let (script, requests, hellos) =
                        (script.clone(), requests.clone(), hellos.clone());
                    tokio::spawn(async move {
                        let Ok(ClientMessage::Hello { client, .. }) =
                            read_message::<_, ClientMessage>(&mut stream).await
                        else {
                            return;
                        };
                        hellos.lock().unwrap().push(client);
                        if script.hang_up {
                            return;
                        }
                        let welcome = ServerMessage::Welcome {
                            protocol: PROTOCOL_VERSION,
                            broker_version: "9.9.9-test".into(),
                        };
                        if write_message(&mut stream, &welcome).await.is_err() {
                            return;
                        }
                        while let Ok(request) = read_message::<_, ClientMessage>(&mut stream).await
                        {
                            let answer = script.answer(&request);
                            requests.lock().unwrap().push(request);
                            if write_message(&mut stream, &answer).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            })
        };
        Self {
            _dir: dir,
            path,
            requests,
            hellos,
            task,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every non-`hello` request received so far, across connections.
    pub fn requests(&self) -> Vec<ClientMessage> {
        self.requests.lock().unwrap().clone()
    }

    /// The `client` label of every `hello` received so far.
    pub fn hello_labels(&self) -> Vec<String> {
        self.hellos.lock().unwrap().clone()
    }
}

impl Drop for FakeBroker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A [`Session`] pointing at the fake broker only (never at the real
/// service/user sockets).
pub fn session_for(broker: &FakeBroker, client: Option<String>) -> Session {
    Session {
        search: SocketSearch {
            flag: Some(broker.path().to_path_buf()),
            env: Env::default(),
            service_socket: broker.path().with_file_name("no-service.sock"),
        },
        client,
    }
}
