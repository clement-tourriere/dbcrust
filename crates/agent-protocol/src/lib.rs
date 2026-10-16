//! Wire protocol between the DBCrust credential **broker** and the thin
//! `dbcrust-agent` client.
//!
//! The broker owns database credentials; AI agents never see them. They talk
//! to the broker through this protocol over a Unix domain socket and can only
//! refer to connections by owner-enrolled *alias*. There is deliberately no
//! field anywhere in the protocol that can carry a URL, a host override, a
//! credential, or a password (see the `no_credential_fields` test).
//!
//! # Transport and framing
//!
//! A byte stream (Unix domain socket). Every message is one **frame**:
//!
//! ```text
//! +-----------------------+---------------------------+
//! | length: u32, big-endian | body: UTF-8 JSON, `length` bytes |
//! +-----------------------+---------------------------+
//! ```
//!
//! The body is at most [`MAX_FRAME_LEN`] (16 MiB). A larger announced length is
//! rejected with [`FrameError::TooLarge`] *before* any body allocation.
//!
//! # Conversation
//!
//! 1. Client sends [`ClientMessage::Hello`] (`protocol` = [`PROTOCOL_VERSION`],
//!    `client` = free-form label, used for audit only).
//! 2. Broker answers [`ServerMessage::Welcome`], or
//!    [`ServerMessage::Error`] with [`ErrorCode::UnsupportedVersion`] when the
//!    protocol versions differ (then closes the connection).
//! 3. Client sends requests, each carrying a `u64` `id`; the broker answers
//!    each with exactly one response carrying the same `id`. Requests on one
//!    connection are answered in order.
//!
//! Every message is a JSON object with a `"type"` discriminator
//! (snake_case). See `PROTOCOL.md` in this crate for the full reference.
//!
//! # Features
//!
//! * `tokio` — async [`aio::read_frame`] / [`aio::write_frame`] helpers.

use std::fmt;
use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Protocol version spoken by this crate. Broker and client must match exactly.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum size, in bytes, of a frame body.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// Size of the big-endian length prefix.
pub const FRAME_HEADER_LEN: usize = 4;

/// Whether a client announcing `client` can talk to a broker speaking `broker`.
/// The protocol carries a single monotonically bumped number, so this is strict
/// equality; the broker answers [`ErrorCode::UnsupportedVersion`] otherwise.
pub fn versions_compatible(client: u32, broker: u32) -> bool {
    client == broker
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Access level the broker grants on an alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

/// Stable machine-readable error identifiers (snake_case on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnknownAlias,
    Denied,
    ReadOnlyViolation,
    DdlRefused,
    MultiStatementRefused,
    LimitExceeded,
    Timeout,
    QueryError,
    ConnectionError,
    BrokerLocked,
    UnsupportedEngine,
    ProtocolError,
    UnsupportedVersion,
    Internal,
}

impl ErrorCode {
    /// Every code, in declaration order.
    pub const ALL: [ErrorCode; 14] = [
        ErrorCode::UnknownAlias,
        ErrorCode::Denied,
        ErrorCode::ReadOnlyViolation,
        ErrorCode::DdlRefused,
        ErrorCode::MultiStatementRefused,
        ErrorCode::LimitExceeded,
        ErrorCode::Timeout,
        ErrorCode::QueryError,
        ErrorCode::ConnectionError,
        ErrorCode::BrokerLocked,
        ErrorCode::UnsupportedEngine,
        ErrorCode::ProtocolError,
        ErrorCode::UnsupportedVersion,
        ErrorCode::Internal,
    ];

    /// The snake_case wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::UnknownAlias => "unknown_alias",
            ErrorCode::Denied => "denied",
            ErrorCode::ReadOnlyViolation => "read_only_violation",
            ErrorCode::DdlRefused => "ddl_refused",
            ErrorCode::MultiStatementRefused => "multi_statement_refused",
            ErrorCode::LimitExceeded => "limit_exceeded",
            ErrorCode::Timeout => "timeout",
            ErrorCode::QueryError => "query_error",
            ErrorCode::ConnectionError => "connection_error",
            ErrorCode::BrokerLocked => "broker_locked",
            ErrorCode::UnsupportedEngine => "unsupported_engine",
            ErrorCode::ProtocolError => "protocol_error",
            ErrorCode::UnsupportedVersion => "unsupported_version",
            ErrorCode::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Messages sent by the client (agent) to the broker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Opens the conversation. `client` is a free-form label (audit only).
    Hello { protocol: u32, client: String },
    /// List the aliases the owner enrolled, with their limits.
    ListConnections { id: u64 },
    /// List tables of an alias.
    ListTables { id: u64, alias: String },
    /// Describe the columns of one table.
    DescribeTable {
        id: u64,
        alias: String,
        table: String,
    },
    /// Run one statement. `max_rows`/`timeout_secs` can only lower the
    /// limits the broker enforces for the alias.
    Query {
        id: u64,
        alias: String,
        sql: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_rows: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u32>,
    },
    /// Return the query plan of a statement (answered with `query_result`).
    Explain { id: u64, alias: String, sql: String },
}

impl ClientMessage {
    /// The request id, `None` for [`ClientMessage::Hello`].
    pub fn id(&self) -> Option<u64> {
        match self {
            ClientMessage::Hello { .. } => None,
            ClientMessage::ListConnections { id }
            | ClientMessage::ListTables { id, .. }
            | ClientMessage::DescribeTable { id, .. }
            | ClientMessage::Query { id, .. }
            | ClientMessage::Explain { id, .. } => Some(*id),
        }
    }
}

/// One enrolled connection alias.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionSummary {
    pub alias: String,
    pub engine: String,
    pub access: Access,
    /// The database session itself is read-only (enforced by the engine).
    pub native_read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub max_rows: u32,
    pub timeout_secs: u32,
}

/// One table or view of an alias.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// One column of a described table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub primary_key: bool,
}

/// Messages sent by the broker to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Answer to [`ClientMessage::Hello`].
    Welcome {
        protocol: u32,
        broker_version: String,
    },
    Connections {
        id: u64,
        connections: Vec<ConnectionSummary>,
    },
    Tables {
        id: u64,
        tables: Vec<TableSummary>,
    },
    TableDescription {
        id: u64,
        table: String,
        columns: Vec<ColumnInfo>,
    },
    /// Result set; also the answer to `explain`. Cells are strings, `None` is
    /// SQL NULL.
    QueryResult {
        id: u64,
        columns: Vec<String>,
        rows: Vec<Vec<Option<String>>>,
        row_count: u64,
        truncated: bool,
    },
    /// Failure. `id` is the failed request's id; it is `None` when the error
    /// is not tied to a request (handshake failure, undecodable frame).
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
        code: ErrorCode,
        message: String,
    },
}

impl ServerMessage {
    /// The id of the request this message answers, if any.
    pub fn id(&self) -> Option<u64> {
        match self {
            ServerMessage::Welcome { .. } => None,
            ServerMessage::Connections { id, .. }
            | ServerMessage::Tables { id, .. }
            | ServerMessage::TableDescription { id, .. }
            | ServerMessage::QueryResult { id, .. } => Some(*id),
            ServerMessage::Error { id, .. } => *id,
        }
    }
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Framing / decoding failure.
#[derive(Debug)]
pub enum FrameError {
    /// Transport error.
    Io(io::Error),
    /// The peer closed the stream cleanly between two frames.
    Eof,
    /// The stream ended inside a header or body.
    Truncated,
    /// The announced (or outgoing) body length exceeds [`MAX_FRAME_LEN`].
    TooLarge { len: u64, max: usize },
    /// The body is not a valid message (bad JSON, unknown `type`, …).
    Json(serde_json::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "transport error: {e}"),
            FrameError::Eof => f.write_str("connection closed"),
            FrameError::Truncated => f.write_str("connection closed in the middle of a frame"),
            FrameError::TooLarge { len, max } => {
                write!(f, "frame of {len} bytes exceeds the {max}-byte limit")
            }
            FrameError::Json(e) => write!(f, "invalid message: {e}"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FrameError::Io(e) => Some(e),
            FrameError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

impl From<serde_json::Error> for FrameError {
    fn from(e: serde_json::Error) -> Self {
        FrameError::Json(e)
    }
}

fn check_len(len: u64) -> Result<usize, FrameError> {
    if len > MAX_FRAME_LEN as u64 {
        Err(FrameError::TooLarge {
            len,
            max: MAX_FRAME_LEN,
        })
    } else {
        Ok(len as usize)
    }
}

/// Serialize `msg` into a complete frame (length prefix + JSON body).
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg)?;
    let len = check_len(body.len() as u64)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + len);
    frame.extend_from_slice(&(len as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Parse a JSON frame **body** (without length prefix) into a message.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    Ok(serde_json::from_slice(body)?)
}

/// Split a complete in-memory frame into its body, validating the prefix.
pub fn frame_body(frame: &[u8]) -> Result<&[u8], FrameError> {
    let header: [u8; FRAME_HEADER_LEN] = frame
        .get(..FRAME_HEADER_LEN)
        .and_then(|h| h.try_into().ok())
        .ok_or(FrameError::Truncated)?;
    let len = check_len(u32::from_be_bytes(header) as u64)?;
    let body = &frame[FRAME_HEADER_LEN..];
    if body.len() < len {
        return Err(FrameError::Truncated);
    }
    Ok(&body[..len])
}

/// Write one frame whose body is `body` (already-serialized JSON).
pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> Result<(), FrameError> {
    let len = check_len(body.len() as u64)?;
    w.write_all(&(len as u32).to_be_bytes())?;
    w.write_all(body)?;
    w.flush()?;
    Ok(())
}

/// Read one frame body. [`FrameError::Eof`] means the peer closed cleanly
/// before the next frame started.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>, FrameError> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    let mut filled = 0;
    while filled < FRAME_HEADER_LEN {
        match r.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Err(FrameError::Eof),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = check_len(u32::from_be_bytes(header) as u64)?;
    // `take` + `read_to_end` grows the buffer as bytes arrive, so a peer
    // announcing a large frame it never sends costs nothing.
    let mut body = Vec::new();
    r.take(len as u64).read_to_end(&mut body)?;
    if body.len() < len {
        return Err(FrameError::Truncated);
    }
    Ok(body)
}

/// Serialize `msg` and write it as one frame.
pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let body = serde_json::to_vec(msg)?;
    write_frame(w, &body)
}

/// Read one frame and parse it as `T`.
pub fn read_message<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, FrameError> {
    decode(&read_frame(r)?)
}

/// Async framing over tokio streams (feature `tokio`). Same semantics as the
/// blocking helpers.
#[cfg(feature = "tokio")]
pub mod aio {
    use super::{FRAME_HEADER_LEN, FrameError, check_len, decode};
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use std::io;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    /// Write one frame whose body is `body` (already-serialized JSON).
    pub async fn write_frame<W: AsyncWrite + Unpin>(
        w: &mut W,
        body: &[u8],
    ) -> Result<(), FrameError> {
        let len = check_len(body.len() as u64)?;
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + len);
        frame.extend_from_slice(&(len as u32).to_be_bytes());
        frame.extend_from_slice(body);
        w.write_all(&frame).await?;
        w.flush().await?;
        Ok(())
    }

    /// Read one frame body; see [`super::read_frame`].
    pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>, FrameError> {
        let mut header = [0u8; FRAME_HEADER_LEN];
        let mut filled = 0;
        while filled < FRAME_HEADER_LEN {
            match r.read(&mut header[filled..]).await {
                Ok(0) if filled == 0 => return Err(FrameError::Eof),
                Ok(0) => return Err(FrameError::Truncated),
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        let len = check_len(u32::from_be_bytes(header) as u64)?;
        let mut body = Vec::new();
        r.take(len as u64).read_to_end(&mut body).await?;
        if body.len() < len {
            return Err(FrameError::Truncated);
        }
        Ok(body)
    }

    /// Serialize `msg` and write it as one frame.
    pub async fn write_message<W: AsyncWrite + Unpin, T: Serialize>(
        w: &mut W,
        msg: &T,
    ) -> Result<(), FrameError> {
        let body = serde_json::to_vec(msg)?;
        write_frame(w, &body).await
    }

    /// Read one frame and parse it as `T`.
    pub async fn read_message<R: AsyncRead + Unpin, T: DeserializeOwned>(
        r: &mut R,
    ) -> Result<T, FrameError> {
        decode(&read_frame(r).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use serde_json::{Value, json};

    fn client_samples() -> Vec<ClientMessage> {
        vec![
            ClientMessage::Hello {
                protocol: PROTOCOL_VERSION,
                client: "claude-code".into(),
            },
            ClientMessage::ListConnections { id: 1 },
            ClientMessage::ListTables {
                id: 2,
                alias: "prod".into(),
            },
            ClientMessage::DescribeTable {
                id: 3,
                alias: "prod".into(),
                table: "public.users".into(),
            },
            ClientMessage::Query {
                id: 4,
                alias: "prod".into(),
                sql: "SELECT 1".into(),
                max_rows: Some(10),
                timeout_secs: Some(5),
            },
            ClientMessage::Query {
                id: 5,
                alias: "prod".into(),
                sql: "SELECT 1".into(),
                max_rows: None,
                timeout_secs: None,
            },
            ClientMessage::Explain {
                id: 6,
                alias: "prod".into(),
                sql: "SELECT 1".into(),
            },
        ]
    }

    fn server_samples() -> Vec<ServerMessage> {
        vec![
            ServerMessage::Welcome {
                protocol: PROTOCOL_VERSION,
                broker_version: "0.38.0".into(),
            },
            ServerMessage::Connections {
                id: 1,
                connections: vec![
                    ConnectionSummary {
                        alias: "prod".into(),
                        engine: "postgresql".into(),
                        access: Access::ReadOnly,
                        native_read_only: true,
                        description: Some("Production replica".into()),
                        max_rows: 500,
                        timeout_secs: 30,
                    },
                    ConnectionSummary {
                        alias: "scratch".into(),
                        engine: "sqlite".into(),
                        access: Access::ReadWrite,
                        native_read_only: false,
                        description: None,
                        max_rows: 100,
                        timeout_secs: 10,
                    },
                ],
            },
            ServerMessage::Tables {
                id: 2,
                tables: vec![
                    TableSummary {
                        schema: Some("public".into()),
                        name: "users".into(),
                        kind: Some("table".into()),
                    },
                    TableSummary {
                        schema: None,
                        name: "events".into(),
                        kind: None,
                    },
                ],
            },
            ServerMessage::TableDescription {
                id: 3,
                table: "users".into(),
                columns: vec![
                    ColumnInfo {
                        name: "id".into(),
                        data_type: "bigint".into(),
                        nullable: Some(false),
                        default: Some("nextval('users_id_seq')".into()),
                        primary_key: true,
                    },
                    ColumnInfo {
                        name: "note".into(),
                        data_type: "text".into(),
                        nullable: None,
                        default: None,
                        primary_key: false,
                    },
                ],
            },
            ServerMessage::QueryResult {
                id: 4,
                columns: vec!["id".into(), "name".into()],
                rows: vec![
                    vec![Some("1".into()), Some("Alice".into())],
                    vec![Some("2".into()), None],
                ],
                row_count: 2,
                truncated: false,
            },
            ServerMessage::Error {
                id: Some(4),
                code: ErrorCode::ReadOnlyViolation,
                message: "writes are refused".into(),
            },
            ServerMessage::Error {
                id: None,
                code: ErrorCode::UnsupportedVersion,
                message: "protocol 2 not supported".into(),
            },
        ]
    }

    /// Compile-time reminder: adding a variant breaks these matches, which
    /// forces the sample lists above (and the credential-field scan) to grow.
    #[allow(dead_code)]
    fn exhaustive(c: &ClientMessage, s: &ServerMessage) {
        match c {
            ClientMessage::Hello { .. }
            | ClientMessage::ListConnections { .. }
            | ClientMessage::ListTables { .. }
            | ClientMessage::DescribeTable { .. }
            | ClientMessage::Query { .. }
            | ClientMessage::Explain { .. } => {}
        }
        match s {
            ServerMessage::Welcome { .. }
            | ServerMessage::Connections { .. }
            | ServerMessage::Tables { .. }
            | ServerMessage::TableDescription { .. }
            | ServerMessage::QueryResult { .. }
            | ServerMessage::Error { .. } => {}
        }
    }

    #[test]
    fn client_messages_round_trip() {
        for msg in client_samples() {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ClientMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, msg, "{json}");
        }
    }

    #[test]
    fn server_messages_round_trip() {
        for msg in server_samples() {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, msg, "{json}");
        }
    }

    #[test]
    fn wire_shape_is_stable() {
        let q = ClientMessage::Query {
            id: 7,
            alias: "a".into(),
            sql: "SELECT 1".into(),
            max_rows: Some(3),
            timeout_secs: None,
        };
        assert_eq!(
            serde_json::to_value(&q).unwrap(),
            json!({"type":"query","id":7,"alias":"a","sql":"SELECT 1","max_rows":3})
        );
        let hello: ClientMessage =
            serde_json::from_str(r#"{"type":"hello","protocol":1,"client":"x"}"#).unwrap();
        assert_eq!(
            hello,
            ClientMessage::Hello {
                protocol: 1,
                client: "x".into()
            }
        );
        let list: ClientMessage =
            serde_json::from_str(r#"{"type":"list_connections","id":9}"#).unwrap();
        assert_eq!(list.id(), Some(9));
        let err = ServerMessage::Error {
            id: Some(1),
            code: ErrorCode::DdlRefused,
            message: "m".into(),
        };
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"type":"error","id":1,"code":"ddl_refused","message":"m"})
        );
        let access = serde_json::to_value(Access::ReadWrite).unwrap();
        assert_eq!(access, json!("read_write"));
    }

    #[test]
    fn error_codes_use_snake_case_strings() {
        for code in ErrorCode::ALL {
            let v = serde_json::to_value(code).unwrap();
            assert_eq!(v, Value::String(code.as_str().to_string()));
            let back: ErrorCode = serde_json::from_value(v).unwrap();
            assert_eq!(back, code);
        }
        assert_eq!(ErrorCode::ALL.len(), 14);
    }

    #[test]
    fn frames_encode_and_decode() {
        for msg in client_samples() {
            let frame = encode(&msg).unwrap();
            let len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
            assert_eq!(len, frame.len() - 4);
            let back: ClientMessage = decode(frame_body(&frame).unwrap()).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn length_prefix_is_big_endian() {
        let mut out = Vec::new();
        write_frame(&mut out, &[b'x'; 258]).unwrap();
        assert_eq!(&out[..4], &[0, 0, 1, 2]);
    }

    #[test]
    fn sync_read_write_round_trip_back_to_back() {
        let mut wire = Vec::new();
        for msg in server_samples() {
            write_message(&mut wire, &msg).unwrap();
        }
        let mut cursor = io::Cursor::new(wire);
        for expected in server_samples() {
            let got: ServerMessage = read_message(&mut cursor).unwrap();
            assert_eq!(got, expected);
        }
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Eof)));
    }

    #[test]
    fn oversize_frame_is_rejected_before_reading_the_body() {
        // Only the header is present: a reader that tried to fetch the body
        // would report `Truncated`, not `TooLarge`.
        let header = ((MAX_FRAME_LEN as u32) + 1).to_be_bytes();
        let mut cursor = io::Cursor::new(header.to_vec());
        match read_frame(&mut cursor) {
            Err(FrameError::TooLarge { len, max }) => {
                assert_eq!(len, MAX_FRAME_LEN as u64 + 1);
                assert_eq!(max, MAX_FRAME_LEN);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        // The largest legal length is accepted by the length check.
        assert!(check_len(MAX_FRAME_LEN as u64).is_ok());
        assert!(matches!(
            check_len(u32::MAX as u64),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn oversize_outgoing_body_is_refused() {
        let body = vec![b' '; MAX_FRAME_LEN + 1];
        let mut out = Vec::new();
        assert!(matches!(
            write_frame(&mut out, &body),
            Err(FrameError::TooLarge { .. })
        ));
        assert!(out.is_empty(), "nothing may be written for a refused frame");
    }

    #[rstest]
    #[case::partial_header(vec![0, 0])]
    #[case::missing_body(vec![0, 0, 0, 10])]
    #[case::short_body(vec![0, 0, 0, 10, b'{', b'}'])]
    fn truncated_frames_are_errors(#[case] bytes: Vec<u8>) {
        let mut cursor = io::Cursor::new(bytes.clone());
        assert!(matches!(
            read_frame(&mut cursor),
            Err(FrameError::Truncated)
        ));
        assert!(matches!(frame_body(&bytes), Err(FrameError::Truncated)));
    }

    #[test]
    fn empty_stream_is_a_clean_eof() {
        let mut cursor = io::Cursor::new(Vec::<u8>::new());
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Eof)));
    }

    #[rstest]
    #[case::unknown_type(r#"{"type":"drop_database","id":1}"#)]
    #[case::missing_type(r#"{"id":1}"#)]
    #[case::not_json("select 1")]
    #[case::missing_field(r#"{"type":"list_tables","id":1}"#)]
    #[case::bad_id(r#"{"type":"list_connections","id":"x"}"#)]
    fn invalid_bodies_are_json_errors(#[case] body: &str) {
        assert!(matches!(
            decode::<ClientMessage>(body.as_bytes()),
            Err(FrameError::Json(_))
        ));
    }

    #[test]
    fn invalid_utf8_body_is_rejected() {
        let body = b"{\"type\":\"hello\",\"protocol\":1,\"client\":\"\xff\"}";
        assert!(matches!(
            decode::<ClientMessage>(body),
            Err(FrameError::Json(_))
        ));
    }

    #[test]
    fn version_negotiation_is_strict() {
        assert!(versions_compatible(1, 1));
        assert!(!versions_compatible(2, 1));
    }

    fn collect_keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    collect_keys(v, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|i| collect_keys(i, out)),
            _ => {}
        }
    }

    /// The protocol must never be able to carry credentials or a way to
    /// redirect the broker: scan every key of every serialized sample.
    #[test]
    fn no_credential_fields() {
        const FORBIDDEN: [&str; 9] = [
            "password",
            "passwd",
            "pwd",
            "url",
            "uri",
            "host",
            "secret",
            "token",
            "credential",
        ];
        let mut keys = Vec::new();
        for m in client_samples() {
            collect_keys(&serde_json::to_value(m).unwrap(), &mut keys);
        }
        for m in server_samples() {
            collect_keys(&serde_json::to_value(m).unwrap(), &mut keys);
        }
        assert!(keys.len() > 20, "the scan must actually see fields");
        for key in &keys {
            let lower = key.to_lowercase();
            for bad in FORBIDDEN {
                assert!(
                    !lower.contains(bad),
                    "protocol field `{key}` looks credential-bearing (`{bad}`)"
                );
            }
        }
    }

    #[cfg(feature = "tokio")]
    mod async_io {
        use super::*;

        #[tokio::test]
        async fn async_round_trip_over_duplex() {
            let (mut a, mut b) = tokio::io::duplex(64 * 1024);
            let msgs = server_samples();
            let sent = msgs.clone();
            let writer = tokio::spawn(async move {
                for m in &sent {
                    aio::write_message(&mut a, m).await.unwrap();
                }
            });
            for expected in msgs {
                let got: ServerMessage = aio::read_message(&mut b).await.unwrap();
                assert_eq!(got, expected);
            }
            writer.await.unwrap();
        }

        #[tokio::test]
        async fn async_oversize_and_truncated() {
            let header = ((MAX_FRAME_LEN as u32) + 1).to_be_bytes();
            let mut r: &[u8] = &header;
            assert!(matches!(
                aio::read_frame(&mut r).await,
                Err(FrameError::TooLarge { .. })
            ));
            let mut r: &[u8] = &[0, 0, 0, 5, b'{'];
            assert!(matches!(
                aio::read_frame(&mut r).await,
                Err(FrameError::Truncated)
            ));
            let mut r: &[u8] = &[];
            assert!(matches!(
                aio::read_frame(&mut r).await,
                Err(FrameError::Eof)
            ));
            let mut sink = Vec::new();
            let big = vec![0u8; MAX_FRAME_LEN + 1];
            assert!(matches!(
                aio::write_frame(&mut sink, &big).await,
                Err(FrameError::TooLarge { .. })
            ));
            assert!(sink.is_empty());
        }
    }
}
