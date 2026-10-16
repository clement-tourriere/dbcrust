//! A small hand-rolled Model Context Protocol server over stdio.
//!
//! Newline-delimited JSON-RPC 2.0. stdout carries protocol messages only;
//! diagnostics go to stderr. Tool calls open a fresh broker connection each
//! time (lazy, so a broker started after the agent works, and a broker
//! restart never leaves a stale connection behind).

use std::io;

use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use crate::app::Session;
use crate::error::AgentError;
use crate::output::error_json;

/// Longest accepted input line.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// Protocol revisions we speak, newest first.
const SUPPORTED_VERSIONS: [&str; 2] = ["2025-06-18", "2025-03-26"];

const INSTRUCTIONS: &str = "\
DBCrust gives you bounded, audited access to databases whose owner enrolled them as named aliases. \
Start with `list_connections` to see the available aliases, their engine, whether they are read-only, \
and their row/time limits. Pass an alias to the other tools; you never see or supply credentials, hosts or URLs. \
Results are bounded: rows beyond the alias's limit are cut and `truncated` is true, so aggregate or filter in SQL. \
Writes, DDL and multi-statement scripts may be refused (error codes read_only_violation, ddl_refused, \
multi_statement_refused); do not try to work around a refusal. Use `list_tables` and `describe_table` before writing queries, \
`explain` to inspect plans, and `query` for one statement at a time.";

// JSON-RPC error codes
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

fn tool_definitions() -> Value {
    let alias = json!({"type": "string", "minLength": 1,
        "description": "Connection alias exactly as returned by list_connections"});
    json!([
        {
            "name": "list_connections",
            "title": "List connections",
            "description": "List the database connection aliases the owner made available, with engine, access level (read_only/read_write), whether the database session is natively read-only, and the row and time limits enforced for each alias. Call this first.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "list_tables",
            "title": "List tables",
            "description": "List the tables and views of a connection alias (schema, name, kind).",
            "inputSchema": {
                "type": "object",
                "properties": {"alias": alias},
                "required": ["alias"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "describe_table",
            "title": "Describe table",
            "description": "Describe the columns of one table: name, data type, nullability, default, and primary-key membership.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "alias": alias,
                    "table": {"type": "string", "minLength": 1,
                        "description": "Table name, optionally schema-qualified (e.g. public.users)"}
                },
                "required": ["alias", "table"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "query",
            "title": "Run a query",
            "description": "Run ONE SQL statement against a connection alias. The broker enforces read-only access, the row limit and the timeout; the result has columns, rows (all values are strings, NULL is null), row_count and truncated. Writes, DDL and multi-statement input may be refused.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "alias": alias,
                    "sql": {"type": "string", "minLength": 1,
                        "description": "A single SQL statement"},
                    "max_rows": {"type": "integer", "minimum": 1,
                        "description": "Optional lower row limit for this query; cannot exceed the alias's limit"},
                    "timeout_secs": {"type": "integer", "minimum": 1,
                        "description": "Optional lower timeout in seconds for this query; cannot exceed the alias's limit"}
                },
                "required": ["alias", "sql"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "openWorldHint": false}
        },
        {
            "name": "explain",
            "title": "Explain a query",
            "description": "Return the query plan of one SQL statement without returning its data. Same result shape as query.",
            "inputSchema": {
                "type": "object",
                "properties": {"alias": alias, "sql": {"type": "string", "minLength": 1,
                    "description": "A single SQL statement to explain"}},
                "required": ["alias", "sql"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        }
    ])
}

/// A JSON-RPC error to send back.
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_PARAMS,
            message: message.into(),
        }
    }
}

enum Line {
    Eof,
    Text(Vec<u8>),
    TooLong,
}

/// Read one `\n`-terminated line, never buffering more than `max` bytes: an
/// over-long line is consumed (and discarded) up to its newline.
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R, max: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut overflowed = false;
    let mut saw_any = false;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(match (saw_any, overflowed) {
                (false, _) => Line::Eof,
                (true, true) => Line::TooLong,
                (true, false) => Line::Text(line),
            });
        }
        saw_any = true;
        let (take, found_newline) = match chunk.iter().position(|b| *b == b'\n') {
            Some(pos) => (pos, true),
            None => (chunk.len(), false),
        };
        if !overflowed {
            if line.len() + take > max {
                overflowed = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(&chunk[..take]);
            }
        }
        let consumed = take + usize::from(found_newline);
        reader.consume(consumed);
        if found_newline {
            return Ok(if overflowed {
                Line::TooLong
            } else {
                Line::Text(line)
            });
        }
    }
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> io::Result<()> {
    let mut text = serde_json::to_string(message).unwrap_or_default();
    text.push('\n');
    writer.write_all(text.as_bytes()).await?;
    writer.flush().await
}

fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Serve MCP on `reader`/`writer` until the reader reaches EOF.
pub async fn serve<R, W>(mut reader: R, mut writer: W, session: &Session) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut client_name: Option<String> = None;
    loop {
        let bytes = match read_line(&mut reader, MAX_LINE_BYTES).await? {
            Line::Eof => return Ok(()),
            Line::TooLong => {
                eprintln!("dbcrust-agent mcp: input line exceeds {MAX_LINE_BYTES} bytes, ignored");
                let message = error_response(
                    &Value::Null,
                    INVALID_REQUEST,
                    "request too large (line exceeds 4 MiB)",
                );
                send(&mut writer, &message).await?;
                continue;
            }
            Line::Text(bytes) => bytes,
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Some(reply) = handle_line(&bytes, session, &mut client_name).await {
            send(&mut writer, &reply).await?;
        }
    }
}

/// Handle one input line; `None` means "no reply" (notifications).
async fn handle_line(
    bytes: &[u8],
    session: &Session,
    client_name: &mut Option<String>,
) -> Option<Value> {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(e) => {
            return Some(error_response(
                &Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            ));
        }
    };
    let Value::Object(object) = value else {
        return Some(error_response(
            &Value::Null,
            INVALID_REQUEST,
            "expected a JSON-RPC request object (batches are not supported)",
        ));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response to something we never sent, or garbage
        if object.contains_key("result") || object.contains_key("error") {
            return None;
        }
        return Some(error_response(
            id.as_ref().unwrap_or(&Value::Null),
            INVALID_REQUEST,
            "missing `method`",
        ));
    };
    let params = object.get("params");

    // No `id`: a notification, never answered
    let id = id?;

    let outcome = match method {
        "initialize" => Ok(initialize(params, client_name)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => call_tool(params, session, client_name.as_deref()).await,
        other => Err(RpcError {
            code: METHOD_NOT_FOUND,
            message: format!("method not found: {other}"),
        }),
    };
    Some(match outcome {
        Ok(result) => response(&id, result),
        Err(e) => error_response(&id, e.code, &e.message),
    })
}

fn initialize(params: Option<&Value>, client_name: &mut Option<String>) -> Value {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    let version = match requested {
        Some(v) if SUPPORTED_VERSIONS.contains(&v) => v,
        _ => SUPPORTED_VERSIONS[0],
    };
    if let Some(name) = params
        .and_then(|p| p.get("clientInfo"))
        .and_then(|c| c.get("name"))
        .and_then(Value::as_str)
        .filter(|n| !n.trim().is_empty())
    {
        *client_name = Some(name.to_string());
    }
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "dbcrust-agent", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

/// Parsed, validated arguments of a tool call.
enum ToolCall {
    ListConnections,
    ListTables {
        alias: String,
    },
    DescribeTable {
        alias: String,
        table: String,
    },
    Query {
        alias: String,
        sql: String,
        max_rows: Option<u32>,
        timeout_secs: Option<u32>,
    },
    Explain {
        alias: String,
        sql: String,
    },
}

fn string_arg(args: &Map<String, Value>, key: &str) -> Result<String, RpcError> {
    match args.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(RpcError::invalid_params(format!(
            "argument `{key}` must not be empty"
        ))),
        Some(_) => Err(RpcError::invalid_params(format!(
            "argument `{key}` must be a string"
        ))),
        None => Err(RpcError::invalid_params(format!(
            "missing required argument `{key}`"
        ))),
    }
}

fn limit_arg(args: &Map<String, Value>, key: &str) -> Result<Option<u32>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| {
                RpcError::invalid_params(format!("argument `{key}` must be a positive integer"))
            }),
    }
}

fn reject_unknown(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), RpcError> {
    match args.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(RpcError::invalid_params(format!(
            "unknown argument `{key}`"
        ))),
        None => Ok(()),
    }
}

fn parse_tool_call(params: Option<&Value>) -> Result<ToolCall, RpcError> {
    let params = params
        .and_then(Value::as_object)
        .ok_or_else(|| RpcError::invalid_params("`params` must be an object with a tool `name`"))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool `name`"))?;
    let empty = Map::new();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(map)) => map,
        Some(_) => return Err(RpcError::invalid_params("`arguments` must be an object")),
    };
    match name {
        "list_connections" => {
            reject_unknown(args, &[])?;
            Ok(ToolCall::ListConnections)
        }
        "list_tables" => {
            reject_unknown(args, &["alias"])?;
            Ok(ToolCall::ListTables {
                alias: string_arg(args, "alias")?,
            })
        }
        "describe_table" => {
            reject_unknown(args, &["alias", "table"])?;
            Ok(ToolCall::DescribeTable {
                alias: string_arg(args, "alias")?,
                table: string_arg(args, "table")?,
            })
        }
        "query" => {
            reject_unknown(args, &["alias", "sql", "max_rows", "timeout_secs"])?;
            Ok(ToolCall::Query {
                alias: string_arg(args, "alias")?,
                sql: string_arg(args, "sql")?,
                max_rows: limit_arg(args, "max_rows")?,
                timeout_secs: limit_arg(args, "timeout_secs")?,
            })
        }
        "explain" => {
            reject_unknown(args, &["alias", "sql"])?;
            Ok(ToolCall::Explain {
                alias: string_arg(args, "alias")?,
                sql: string_arg(args, "sql")?,
            })
        }
        other => Err(RpcError::invalid_params(format!("unknown tool: {other}"))),
    }
}

fn structured<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

async fn run_tool(
    call: ToolCall,
    session: &Session,
    client_name: Option<&str>,
) -> Result<Value, AgentError> {
    let mut client = session.connect(client_name).await?;
    Ok(match call {
        ToolCall::ListConnections => {
            json!({"connections": structured(&client.list_connections().await?)})
        }
        ToolCall::ListTables { alias } => {
            json!({"tables": structured(&client.list_tables(&alias).await?)})
        }
        ToolCall::DescribeTable { alias, table } => {
            let (table, columns) = client.describe_table(&alias, &table).await?;
            json!({"table": table, "columns": structured(&columns)})
        }
        ToolCall::Query {
            alias,
            sql,
            max_rows,
            timeout_secs,
        } => structured(&client.query(&alias, &sql, max_rows, timeout_secs).await?),
        ToolCall::Explain { alias, sql } => structured(&client.explain(&alias, &sql).await?),
    })
}

async fn call_tool(
    params: Option<&Value>,
    session: &Session,
    client_name: Option<&str>,
) -> Result<Value, RpcError> {
    let call = parse_tool_call(params)?;
    Ok(match run_tool(call, session, client_name).await {
        Ok(structured) => {
            let text = serde_json::to_string(&structured).unwrap_or_default();
            json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": structured,
                "isError": false,
            })
        }
        Err(error) => {
            let text = error_json(error.code.as_str(), &error.message);
            let structured: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": structured,
                "isError": true,
            })
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testutil::{FakeBroker, Script, session_for};
    use dbcrust_agent_protocol::{Access, ClientMessage, ConnectionSummary, ErrorCode};
    use tokio::io::{AsyncReadExt, BufReader, duplex};

    /// Feed `input` to the server and return every output line as JSON.
    async fn converse(session: &Session, input: &[u8]) -> Vec<Value> {
        let (mut client_write, server_read) = duplex(8 * 1024 * 1024);
        let (server_write, mut client_read) = duplex(8 * 1024 * 1024);
        client_write.write_all(input).await.unwrap();
        drop(client_write);
        serve(BufReader::new(server_read), server_write, session)
            .await
            .unwrap();
        let mut out = String::new();
        client_read.read_to_string(&mut out).await.unwrap();
        out.lines()
            .map(|l| serde_json::from_str(l).expect("stdout carries only JSON"))
            .collect()
    }

    fn lines(values: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for v in values {
            out.extend_from_slice(serde_json::to_string(v).unwrap().as_bytes());
            out.push(b'\n');
        }
        out
    }

    fn init(version: &str) -> Value {
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "1.0"}}})
    }

    #[tokio::test]
    async fn full_conversation() {
        let broker =
            FakeBroker::start(Script::default().with_connections(vec![ConnectionSummary {
                alias: "prod".into(),
                engine: "postgresql".into(),
                access: Access::ReadOnly,
                native_read_only: true,
                description: None,
                max_rows: 100,
                timeout_secs: 10,
            }]));
        let session = session_for(&broker, None);
        let mut input = lines(&[
            init("2025-03-26"),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"query","arguments":{"alias":"prod","sql":"SELECT id, name FROM u","max_rows":5}}}),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                "name":"list_connections","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":6,"method":"no/such/method"}),
        ]);
        input.extend_from_slice(b"{not json\n");
        let out = converse(&session, &input).await;

        // initialize: negotiates down to the client's version
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "dbcrust-agent");
        assert_eq!(
            out[0]["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
        assert!(
            out[0]["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("owner")
        );
        // no reply to the notification: next output is the ping answer
        assert_eq!(out[1]["id"], 2);
        assert_eq!(out[1]["result"], json!({}));
        // tools/list
        let names: Vec<&str> = out[2]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_connections",
                "list_tables",
                "describe_table",
                "query",
                "explain"
            ]
        );
        for tool in out[2]["result"]["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().unwrap().len() > 20);
        }
        // tools/call query
        let result = &out[3]["result"];
        assert_eq!(out[3]["id"], 4);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["row_count"], 2);
        assert_eq!(result["structuredContent"]["rows"][1][1], Value::Null);
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
        assert_eq!(result["content"][0]["type"], "text");
        // list_connections
        assert_eq!(
            out[4]["result"]["structuredContent"]["connections"][0]["alias"],
            "prod"
        );
        // unknown method
        assert_eq!(out[5]["id"], 6);
        assert_eq!(out[5]["error"]["code"], -32601);
        // malformed JSON
        assert_eq!(out[6]["error"]["code"], -32700);
        assert_eq!(out[6]["id"], Value::Null);
        assert_eq!(out.len(), 7);

        // the broker saw the MCP client's name and the bounded query
        assert_eq!(broker.hello_labels(), vec!["test-client", "test-client"]);
        assert_eq!(
            broker.requests()[0],
            ClientMessage::Query {
                id: 1,
                alias: "prod".into(),
                sql: "SELECT id, name FROM u".into(),
                max_rows: Some(5),
                timeout_secs: None,
            }
        );
    }

    #[tokio::test]
    async fn explicit_client_flag_beats_client_info() {
        let broker = FakeBroker::start(Script::default());
        let session = session_for(&broker, Some("forced".into()));
        let input = lines(&[
            init("2025-06-18"),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"list_connections"}}),
        ]);
        converse(&session, &input).await;
        assert_eq!(broker.hello_labels(), vec!["forced"]);
    }

    #[tokio::test]
    async fn unsupported_protocol_version_gets_our_newest() {
        let broker = FakeBroker::start(Script::default());
        let out = converse(&session_for(&broker, None), &lines(&[init("1999-01-01")])).await;
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-06-18");
        let out = converse(&session_for(&broker, None), &lines(&[init("2025-06-18")])).await;
        assert_eq!(out[0]["result"]["protocolVersion"], "2025-06-18");
    }

    #[tokio::test]
    async fn broker_errors_become_tool_errors() {
        let broker = FakeBroker::start(
            Script::default().failing_with(ErrorCode::ReadOnlyViolation, "writes are refused"),
        );
        let input = lines(&[
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
            "name":"query","arguments":{"alias":"prod","sql":"DELETE FROM u"}}}),
        ]);
        let out = converse(&session_for(&broker, None), &input).await;
        let result = &out[0]["result"];
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["structuredContent"]["error"]["code"],
            "read_only_violation"
        );
        assert_eq!(
            result["structuredContent"]["error"]["message"],
            "writes are refused"
        );
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["error"]["code"], "read_only_violation");
        assert!(
            out[0].get("error").is_none(),
            "tool failures are not JSON-RPC errors"
        );
    }

    #[tokio::test]
    async fn unreachable_broker_is_a_tool_error_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            search: crate::socket::SocketSearch {
                flag: Some(dir.path().join("none.sock")),
                env: crate::socket::Env::default(),
                service_socket: dir.path().join("service.sock"),
            },
            client: None,
        };
        let input = lines(&[
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"list_connections"}}),
        ]);
        let out = converse(&session, &input).await;
        assert_eq!(out[0]["result"]["isError"], true);
        assert_eq!(
            out[0]["result"]["structuredContent"]["error"]["code"],
            "connection_error"
        );
    }

    #[tokio::test]
    async fn invalid_params_are_32602() {
        let broker = FakeBroker::start(Script::default());
        let cases = [
            json!({"name":"query","arguments":{"alias":"prod"}}),
            json!({"name":"query","arguments":{"alias":"prod","sql":5}}),
            json!({"name":"query","arguments":{"alias":"prod","sql":"x","max_rows":-1}}),
            json!({"name":"query","arguments":{"alias":"prod","sql":"x","max_rows":"5"}}),
            json!({"name":"query","arguments":{"alias":"prod","sql":"x","url":"postgres://h"}}),
            json!({"name":"list_tables","arguments":{"alias":""}}),
            json!({"name":"nope","arguments":{}}),
            json!({"arguments":{}}),
            json!({"name":"list_tables","arguments":[1]}),
        ];
        let requests: Vec<Value> = cases
            .iter()
            .enumerate()
            .map(
                |(i, params)| json!({"jsonrpc":"2.0","id":i,"method":"tools/call","params":params}),
            )
            .collect();
        let out = converse(&session_for(&broker, None), &lines(&requests)).await;
        assert_eq!(out.len(), cases.len());
        for (i, reply) in out.iter().enumerate() {
            assert_eq!(reply["error"]["code"], -32602, "case {i}: {reply}");
        }
        assert!(
            broker.requests().is_empty(),
            "invalid calls never reach the broker"
        );
    }

    #[tokio::test]
    async fn protocol_edge_cases() {
        let broker = FakeBroker::start(Script::default());
        let mut input = Vec::new();
        input.extend_from_slice(b"\n   \n"); // blank lines are ignored
        input.extend_from_slice(b"[1,2]\n"); // batch → invalid request
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":9}\n"); // no method
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n"); // a response: ignored
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\"}\n");
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"unknown/notification\"}\n");
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":\"abc\",\"method\":\"ping\"}\r\n");
        let out = converse(&session_for(&broker, None), &input).await;
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(out[0]["error"]["code"], -32600);
        assert_eq!(out[1]["error"]["code"], -32600);
        assert_eq!(out[1]["id"], 9);
        assert_eq!(out[2]["id"], "abc");
        assert_eq!(out[2]["result"], json!({}));
    }

    #[tokio::test]
    async fn oversized_lines_are_rejected_and_the_stream_recovers() {
        let broker = FakeBroker::start(Script::default());
        let mut input = Vec::new();
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"");
        input.extend(std::iter::repeat_n(b'a', MAX_LINE_BYTES + 10));
        input.extend_from_slice(b"\"}\n");
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
        let out = converse(&session_for(&broker, None), &input).await;
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["error"]["code"], -32600);
        assert!(
            out[0]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("too large")
        );
        assert_eq!(out[1]["id"], 2);
    }

    #[tokio::test]
    async fn line_without_trailing_newline_is_still_handled() {
        let broker = FakeBroker::start(Script::default());
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}";
        let out = converse(&session_for(&broker, None), input).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], 1);
    }

    #[test]
    fn tool_schemas_expose_no_connection_overrides() {
        let tools = tool_definitions();
        for tool in tools.as_array().unwrap() {
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            for key in props.keys() {
                for bad in ["url", "host", "password", "secret", "token", "dsn"] {
                    assert!(!key.contains(bad), "{} exposes `{key}`", tool["name"]);
                }
            }
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
    }
}
