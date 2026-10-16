//! The gateway: the broker's only path from a protocol request to a database.
//!
//! Every request goes through the same stages:
//!
//! 1. **resolve** the alias to its secret record (missing/revoked → `unknown_alias`);
//! 2. **bind**: the connection is built *exclusively* from the record's
//!    `bound_target`, `secret` and `secret_options` — nothing from the
//!    request, `aliases.toml`, the environment, `.pgpass`, SSH tunnel
//!    patterns, Vault or Docker;
//! 3. **classify** the SQL (see `safety::classify_for_broker`), refusing
//!    multi-statement input, DDL/DCL/admin and — unless the record allows
//!    writes — DML;
//! 4. **limit**: effective limits are the record's ceiling tightened by the
//!    request (never loosened); rows are bounded by an auto-`LIMIT` of
//!    `max_rows + 1` (to detect truncation) and by fetch-side row/byte caps;
//! 5. **audit** (fail closed: if the `start` line cannot be written the
//!    request is refused);
//! 6. **execute** on a per-alias connection that is created lazily, reused,
//!    and discarded on any error or timeout, with native read-only
//!    hardening and a server-side statement timeout where the engine has one.

use super::audit::{AuditLog, AuditRecord, Decision, Phase, sql_digest};
use super::registry::{Registry, ResolveError, ResolvedAlias};
use dbcrust_agent_protocol::{
    Access, ClientMessage, ColumnInfo as WireColumn, ConnectionSummary, ErrorCode, MAX_FRAME_LEN,
    ServerMessage, TableSummary,
};
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider,
    StructuredQueryResult,
};
use dbcrust_core::safety::{BrokerStatement, RefusalKind, StatementClass, classify_for_broker};
use dbcrust_drivers::create_database_client;
use dbcrust_secrets::{AgentAccess, AgentPolicy, PolicyRequest, normalize_engine};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Statements longer than this are refused before parsing.
pub const MAX_SQL_BYTES: usize = 1024 * 1024;
/// Requests allowed to wait behind the in-flight one for the same alias.
pub const MAX_QUEUED_PER_ALIAS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// The server-side timeout fires first; the client-side deadline only has to
/// catch engines (and hangs) without one.
const DEADLINE_GRACE: Duration = Duration::from_secs(2);
const MAX_ERROR_CHARS: usize = 600;
const MAX_TABLE_NAME_CHARS: usize = 255;

/// Who is asking (recorded in the audit log, never used for authorization
/// decisions inside the gateway).
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub client_label: String,
    pub peer_uid: Option<u32>,
    pub peer_pid: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayError {
    pub code: ErrorCode,
    pub message: String,
}

impl GatewayError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn error_message(id: Option<u64>, error: &GatewayError) -> ServerMessage {
    ServerMessage::Error {
        id,
        code: error.code,
        message: error.message.clone(),
    }
}

// ---- connection construction ------------------------------------------------

/// The engine of a resolved record, as a driver type.
pub fn database_type_for(engine: &str) -> Option<DatabaseType> {
    match normalize_engine(engine).as_str() {
        "postgresql" => Some(DatabaseType::PostgreSQL),
        "mysql" => Some(DatabaseType::MySQL),
        "sqlite" => Some(DatabaseType::SQLite),
        "clickhouse" => Some(DatabaseType::ClickHouse),
        _ => None,
    }
}

/// Build the connection for a record. This is the *only* place a broker
/// connection is described: host, port, database, user, TLS mode, password and
/// options all come from the record; the hardening keys are added last.
pub fn connection_info_for(
    resolved: &ResolvedAlias,
    read_only: bool,
    timeout_secs: u32,
) -> Result<ConnectionInfo, GatewayError> {
    let target = &resolved.record.bound_target;
    let db_type = database_type_for(&target.engine).ok_or_else(|| {
        GatewayError::new(
            ErrorCode::UnsupportedEngine,
            format!("engine {} is not supported by the broker", target.engine),
        )
    })?;

    let mut options: HashMap<String, String> = HashMap::new();
    for (key, value) in &resolved.record.secret_options {
        if !dbcrust_core::options::is_dbcrust_internal_connection_option(key) {
            options.insert(key.clone(), value.to_string());
        }
    }

    let mut use_tls = false;
    if let Some(mode) = target.tls_mode.as_deref() {
        match db_type {
            DatabaseType::PostgreSQL => {
                options.insert("sslmode".to_string(), mode.to_string());
                use_tls = matches!(mode, "require" | "verify-ca" | "verify-full");
            }
            DatabaseType::MySQL => {
                options.insert("ssl-mode".to_string(), mode.to_string());
            }
            DatabaseType::ClickHouse => {
                use_tls = matches!(mode, "tls" | "https");
            }
            _ => {}
        }
    }

    if read_only {
        options.insert(
            dbcrust_core::database::OPT_HARDEN_READ_ONLY.to_string(),
            "1".to_string(),
        );
    } else {
        // Present-but-off still marks the connection as broker-built (no
        // .pgpass, no proxy environment)
        options.insert(
            dbcrust_core::database::OPT_HARDEN_READ_ONLY.to_string(),
            "0".to_string(),
        );
    }
    options.insert(
        dbcrust_core::database::OPT_HARDEN_TIMEOUT_SECS.to_string(),
        timeout_secs.max(1).to_string(),
    );

    let password = (!resolved.record.secret.is_empty()).then(|| resolved.record.secret.to_string());
    let info = if db_type == DatabaseType::SQLite {
        ConnectionInfo {
            database_type: db_type,
            file_path: Some(target.database.clone()),
            options,
            ..Default::default()
        }
    } else {
        ConnectionInfo {
            database_type: db_type,
            host: Some(target.host.clone()),
            port: target.port,
            username: (!target.user.is_empty()).then(|| target.user.clone()),
            password,
            database: (!target.database.is_empty()).then(|| target.database.clone()),
            options,
            use_tls,
            ..Default::default()
        }
    };
    Ok(info)
}

/// Connect for a record (also used by enrollment's test-connect, so what is
/// verified is exactly what will run).
pub async fn connect_for_record(
    resolved: &ResolvedAlias,
    read_only: bool,
    timeout_secs: u32,
) -> Result<Box<dyn DatabaseClient>, GatewayError> {
    let info = connection_info_for(resolved, read_only, timeout_secs)?;
    match tokio::time::timeout(CONNECT_TIMEOUT, create_database_client(info)).await {
        Err(_) => Err(GatewayError::new(
            ErrorCode::ConnectionError,
            "timed out connecting to the database",
        )),
        // An alias enrolled by a build that had the engine, served by one
        // that does not: say so instead of a connection error
        Ok(Err(e @ DatabaseError::ConnectorNotBuilt { .. })) => Err(GatewayError::new(
            ErrorCode::UnsupportedEngine,
            clean_message(&e.to_string()),
        )),
        Ok(Err(e)) => Err(GatewayError::new(
            ErrorCode::ConnectionError,
            clean_message(&e.to_string()),
        )),
        Ok(Ok(client)) => Ok(client),
    }
}

// ---- error shaping --------------------------------------------------------

fn clean_message(raw: &str) -> String {
    let redacted = dbcrust_core::password_sanitizer::redact_text(raw);
    let flat: String = redacted
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() > MAX_ERROR_CHARS {
        let cut: String = flat.chars().take(MAX_ERROR_CHARS).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

fn map_db_error(error: &DatabaseError) -> GatewayError {
    let text = error.to_string();
    let lower = text.to_lowercase();
    let code = if lower.contains("read-only")
        || lower.contains("read only")
        || lower.contains("readonly")
        || lower.contains("query_only")
    {
        ErrorCode::ReadOnlyViolation
    } else if lower.contains("statement timeout")
        || lower.contains("canceling statement due to")
        || lower.contains("max_execution_time")
        || lower.contains("maximum statement execution time")
        || lower.contains("timeout_exceeded")
        || lower.contains("timed out")
        || lower.contains("interrupted")
    {
        ErrorCode::Timeout
    } else if matches!(error, DatabaseError::ConnectionError(_)) {
        ErrorCode::ConnectionError
    } else {
        ErrorCode::QueryError
    };
    GatewayError::new(code, clean_message(&text))
}

fn refusal_code(kind: RefusalKind) -> ErrorCode {
    match kind {
        RefusalKind::MultiStatement => ErrorCode::MultiStatementRefused,
        RefusalKind::Ddl => ErrorCode::DdlRefused,
        RefusalKind::ReadOnlyViolation => ErrorCode::ReadOnlyViolation,
        RefusalKind::Empty | RefusalKind::Malformed => ErrorCode::QueryError,
    }
}

// ---- table-name validation ---------------------------------------------------

/// A table reference the metadata providers may safely interpolate: one to
/// three dot-separated parts, each a plain identifier or a double-quoted one
/// without quotes, backslashes, semicolons or control characters.
pub fn validate_table_name(name: &str) -> Result<(), GatewayError> {
    let bad = |why: &str| {
        Err(GatewayError::new(
            ErrorCode::QueryError,
            format!("invalid table name: {why}"),
        ))
    };
    if name.is_empty() || name.chars().count() > MAX_TABLE_NAME_CHARS {
        return bad("empty or too long");
    }
    let chars: Vec<char> = name.chars().collect();
    let mut parts = 1usize;
    let mut i = 0usize;
    let mut part_len = 0usize;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                if part_len != 0 {
                    return bad("unexpected quote");
                }
                i += 1;
                let mut body = 0usize;
                loop {
                    match chars.get(i) {
                        None => return bad("unterminated quote"),
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some(c) if c.is_control() || matches!(c, '\\' | ';' | '\'' | '`') => {
                            return bad("forbidden character");
                        }
                        Some(_) => {
                            body += 1;
                            i += 1;
                        }
                    }
                }
                if body == 0 {
                    return bad("empty quoted identifier");
                }
                part_len = body;
                if i < chars.len() && chars[i] != '.' {
                    return bad("unexpected character after quote");
                }
            }
            '.' => {
                if part_len == 0 {
                    return bad("empty name part");
                }
                parts += 1;
                part_len = 0;
                i += 1;
            }
            c if c.is_alphanumeric() || c == '_' || c == '$' || c == '-' => {
                part_len += 1;
                i += 1;
            }
            _ => return bad("forbidden character"),
        }
    }
    if part_len == 0 {
        return bad("empty name part");
    }
    if parts > 3 {
        return bad("too many name parts");
    }
    Ok(())
}

// ---- result shaping -------------------------------------------------------

/// Rows kept, plus whether anything was dropped and how many payload bytes
/// the kept rows hold.
struct Shaped {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    truncated: bool,
    bytes: u64,
}

/// Apply the row cap and the byte budget to a fetched result. Bytes are the
/// UTF-8 length of every cell (NULL = 0) plus column names, a payload
/// measure independent of JSON framing.
fn shape_rows(result: StructuredQueryResult, max_rows: usize, max_bytes: u64) -> Shaped {
    let StructuredQueryResult { columns, rows } = result;
    let mut bytes: u64 = columns.iter().map(|c| c.len() as u64).sum();
    let mut kept: Vec<Vec<Option<String>>> = Vec::with_capacity(rows.len().min(max_rows));
    let mut truncated = false;
    for row in rows {
        if kept.len() >= max_rows {
            truncated = true;
            break;
        }
        let row_bytes: u64 = row
            .iter()
            .map(|cell| cell.as_ref().map_or(0, |s| s.len() as u64) + 2)
            .sum();
        if bytes + row_bytes > max_bytes {
            truncated = true;
            break;
        }
        bytes += row_bytes;
        kept.push(row);
    }
    Shaped {
        columns,
        rows: kept,
        truncated,
        bytes,
    }
}

/// Build the `query_result`, dropping trailing rows until the JSON frame fits
/// the protocol's 16 MiB limit (escaping can inflate the payload).
fn query_result(id: u64, mut shaped: Shaped) -> (ServerMessage, Shaped) {
    loop {
        let message = ServerMessage::QueryResult {
            id,
            columns: shaped.columns.clone(),
            rows: shaped.rows.clone(),
            row_count: shaped.rows.len() as u64,
            truncated: shaped.truncated,
        };
        let fits = dbcrust_agent_protocol::encode(&message)
            .map(|frame| frame.len() <= MAX_FRAME_LEN + 4)
            .unwrap_or(false);
        if fits || shaped.rows.is_empty() {
            return (message, shaped);
        }
        let keep = shaped.rows.len() / 2;
        shaped.rows.truncate(keep);
        shaped.truncated = true;
    }
}

// ---- per-alias connection slots ---------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveKey {
    revision: u64,
    read_only: bool,
    timeout_secs: u32,
}

struct Live {
    client: Box<dyn DatabaseClient>,
    key: LiveKey,
}

#[derive(Default)]
struct Slot {
    state: tokio::sync::Mutex<Option<Live>>,
    waiting: AtomicUsize,
}

enum Work {
    Sql(String),
    Tables,
    Describe {
        schema: Option<String>,
        table: String,
    },
}

enum WorkOutput {
    Rows(StructuredQueryResult),
    Tables {
        names: Vec<String>,
        default_schema: Option<String>,
    },
    Details(dbcrust_core::model::TableDetails),
}

async fn run_work(client: &dyn DatabaseClient, work: Work) -> Result<WorkOutput, DatabaseError> {
    match work {
        Work::Sql(sql) => client
            .execute_query_structured(&sql)
            .await
            .map(WorkOutput::Rows),
        Work::Tables => {
            let provider: &dyn MetadataProvider = client.get_metadata_provider();
            let names = provider.get_tables(None).await?;
            Ok(WorkOutput::Tables {
                names,
                default_schema: provider.default_schema(),
            })
        }
        Work::Describe { schema, table } => {
            let provider = client.get_metadata_provider();
            let mut result = provider.get_table_details(&table, schema.as_deref()).await;
            // A dotted bare name parses as schema.table; retry it verbatim
            if schema.is_some() && !matches!(result, Ok(ref d) if !d.columns.is_empty()) {
                let verbatim = format!("{}.{}", schema.as_deref().unwrap_or_default(), table);
                if let Ok(details) = provider.get_table_details(&verbatim, None).await {
                    if !details.columns.is_empty() {
                        result = Ok(details);
                    }
                }
            }
            result.map(WorkOutput::Details)
        }
    }
}

// ---- audit plumbing ---------------------------------------------------------

struct Trail<'a> {
    log: &'a AuditLog,
    ctx: &'a RequestContext,
    alias: Option<String>,
    op: &'static str,
    sql: Option<(String, usize)>,
    seq: u64,
    started: Instant,
}

impl Trail<'_> {
    fn record(&self, phase: Phase, decision: Decision, code: Option<ErrorCode>) -> AuditRecord {
        AuditRecord {
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            seq: self.seq,
            phase,
            client_label: self.ctx.client_label.clone(),
            peer_uid: self.ctx.peer_uid,
            peer_pid: self.ctx.peer_pid,
            alias: self.alias.clone(),
            op: self.op.to_string(),
            decision,
            code: code.map(|c| c.as_str().to_string()),
            sql_sha256: self.sql.as_ref().map(|(hash, _)| hash.clone()),
            sql_len: self.sql.as_ref().map(|(_, len)| *len),
            rows: None,
            bytes: None,
            truncated: None,
            duration_ms: None,
        }
    }

    /// Persist the `start` line; `Err` means the request must be refused.
    fn start(&self) -> Result<(), ()> {
        self.log
            .append(&self.record(Phase::Start, Decision::Allowed, None), true)
            .map_err(|e| {
                tracing::error!("audit log write failed, refusing request: {e}");
            })
    }

    fn end(
        &self,
        decision: Decision,
        code: Option<ErrorCode>,
        rows: Option<u64>,
        bytes: Option<u64>,
        truncated: Option<bool>,
    ) {
        let mut record = self.record(Phase::End, decision, code);
        record.rows = rows;
        record.bytes = bytes;
        record.truncated = truncated;
        record.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        if let Err(e) = self.log.append(&record, false) {
            tracing::error!("audit log write failed: {e}");
        }
    }

    fn refused(&self, code: ErrorCode) {
        self.end(Decision::Refused, Some(code), None, None, None);
    }
}

// ---- the gateway -------------------------------------------------------------

pub struct Gateway {
    registry: Arc<Registry>,
    audit: Arc<AuditLog>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl Gateway {
    pub fn new(registry: Arc<Registry>, audit: Arc<AuditLog>) -> Self {
        Self {
            registry,
            audit,
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    fn slot(&self, alias: &str) -> Arc<Slot> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots
            .entry(alias.to_string())
            .or_insert_with(|| Arc::new(Slot::default()))
            .clone()
    }

    /// Forget an alias's slot: its connection closes once no request holds it.
    fn evict(&self, alias: &str) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots.remove(alias);
    }

    /// Drop every live connection (shutdown, tests).
    pub async fn close_all(&self) {
        let slots: Vec<Arc<Slot>> = {
            let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            slots.values().cloned().collect()
        };
        for slot in slots {
            *slot.state.lock().await = None;
        }
    }

    fn trail<'a>(
        &'a self,
        ctx: &'a RequestContext,
        alias: Option<&str>,
        op: &'static str,
        sql: Option<&str>,
    ) -> Trail<'a> {
        Trail {
            log: &self.audit,
            ctx,
            alias: alias.map(|a| a.chars().take(64).collect()),
            op,
            sql: sql.map(|s| (sql_digest(s), s.len())),
            seq: self.audit.next_seq(),
            started: Instant::now(),
        }
    }

    pub async fn handle(&self, ctx: &RequestContext, request: ClientMessage) -> ServerMessage {
        match request {
            ClientMessage::Hello { .. } => ServerMessage::Error {
                id: None,
                code: ErrorCode::ProtocolError,
                message: "hello may only be sent once, as the first message".to_string(),
            },
            ClientMessage::ListConnections { id } => self.list_connections(ctx, id).await,
            ClientMessage::ListTables { id, alias } => {
                self.metadata(ctx, id, alias, None, "list_tables").await
            }
            ClientMessage::DescribeTable { id, alias, table } => {
                self.metadata(ctx, id, alias, Some(table), "describe_table")
                    .await
            }
            ClientMessage::Query {
                id,
                alias,
                sql,
                max_rows,
                timeout_secs,
            } => {
                let request = PolicyRequest {
                    access: None,
                    max_rows,
                    timeout_secs,
                    max_bytes: None,
                };
                self.statement(ctx, id, alias, sql, request, false).await
            }
            ClientMessage::Explain { id, alias, sql } => {
                self.statement(ctx, id, alias, sql, PolicyRequest::default(), true)
                    .await
            }
        }
    }

    async fn resolve(&self, alias: &str) -> Result<Arc<ResolvedAlias>, GatewayError> {
        let registry = self.registry.clone();
        let name = alias.to_string();
        match tokio::task::spawn_blocking(move || registry.resolve(&name)).await {
            Ok(Ok(resolved)) => Ok(resolved),
            Ok(Err(ResolveError::Unknown)) => {
                // Revoked (or never enrolled): do not keep its connection
                // open in the background
                self.evict(alias);
                let shown: String = alias.chars().take(64).collect();
                Err(GatewayError::new(
                    ErrorCode::UnknownAlias,
                    format!("no connection named {shown:?} is available to this broker"),
                ))
            }
            Ok(Err(ResolveError::Denied(why))) => {
                self.evict(alias);
                Err(GatewayError::new(ErrorCode::Denied, why))
            }
            Ok(Err(ResolveError::Locked(_))) => Err(GatewayError::new(
                ErrorCode::BrokerLocked,
                "the broker's secret store is locked; the owner must unlock it",
            )),
            Ok(Err(ResolveError::Unavailable(why))) => {
                tracing::error!("alias resolution failed: {why}");
                Err(GatewayError::new(
                    ErrorCode::Internal,
                    "the broker could not read its connection records",
                ))
            }
            Err(_) => Err(GatewayError::new(ErrorCode::Internal, "internal error")),
        }
    }

    async fn list_connections(&self, ctx: &RequestContext, id: u64) -> ServerMessage {
        let trail = self.trail(ctx, None, "list_connections", None);
        if trail.start().is_err() {
            return audit_unavailable(id);
        }
        let registry = self.registry.clone();
        let listed = tokio::task::spawn_blocking(move || registry.list()).await;
        match listed {
            Ok(Ok(aliases)) => {
                let connections: Vec<ConnectionSummary> = aliases
                    .iter()
                    .map(|a| ConnectionSummary {
                        alias: a.alias.clone(),
                        engine: normalize_engine(&a.record.bound_target.engine),
                        access: match a.policy.access {
                            AgentAccess::ReadOnly => Access::ReadOnly,
                            AgentAccess::ReadWrite => Access::ReadWrite,
                        },
                        native_read_only: a.policy.access == AgentAccess::ReadOnly,
                        description: a.entry.description.clone(),
                        max_rows: a.policy.max_rows,
                        timeout_secs: a.policy.timeout_secs,
                    })
                    .collect();
                trail.end(
                    Decision::Allowed,
                    None,
                    Some(connections.len() as u64),
                    None,
                    None,
                );
                ServerMessage::Connections { id, connections }
            }
            Ok(Err(ResolveError::Locked(_))) => {
                trail.refused(ErrorCode::BrokerLocked);
                error_message(
                    Some(id),
                    &GatewayError::new(
                        ErrorCode::BrokerLocked,
                        "the broker's secret store is locked; the owner must unlock it",
                    ),
                )
            }
            _ => {
                trail.end(Decision::Error, Some(ErrorCode::Internal), None, None, None);
                error_message(
                    Some(id),
                    &GatewayError::new(ErrorCode::Internal, "internal error"),
                )
            }
        }
    }

    async fn metadata(
        &self,
        ctx: &RequestContext,
        id: u64,
        alias: String,
        table: Option<String>,
        op: &'static str,
    ) -> ServerMessage {
        let trail = self.trail(ctx, Some(&alias), op, None);
        let resolved = match self.resolve(&alias).await {
            Ok(resolved) => resolved,
            Err(e) => {
                trail.refused(e.code);
                return error_message(Some(id), &e);
            }
        };
        let Some(db_type) = database_type_for(&resolved.record.bound_target.engine) else {
            trail.refused(ErrorCode::UnsupportedEngine);
            return error_message(
                Some(id),
                &GatewayError::new(ErrorCode::UnsupportedEngine, "unsupported engine"),
            );
        };
        let work = match &table {
            None => Work::Tables,
            Some(name) => {
                if let Err(e) = validate_table_name(name) {
                    trail.refused(e.code);
                    return error_message(Some(id), &e);
                }
                let (schema, table) = dbcrust_core::model::metadata_table_parts(name, &db_type);
                Work::Describe { schema, table }
            }
        };
        if trail.start().is_err() {
            return audit_unavailable(id);
        }
        let policy = resolved.policy;
        match self.execute(&resolved, &policy, work).await {
            Ok(WorkOutput::Tables {
                names,
                default_schema,
            }) => {
                let tables: Vec<TableSummary> = names
                    .iter()
                    .map(|name| {
                        let (schema, table) =
                            dbcrust_core::model::metadata_table_parts(name, &db_type);
                        TableSummary {
                            schema: schema.or_else(|| default_schema.clone()),
                            name: table,
                            kind: None,
                        }
                    })
                    .collect();
                trail.end(
                    Decision::Allowed,
                    None,
                    Some(tables.len() as u64),
                    None,
                    None,
                );
                ServerMessage::Tables { id, tables }
            }
            Ok(WorkOutput::Details(details)) => {
                let primary = primary_key_columns(&details);
                let columns: Vec<WireColumn> = details
                    .columns
                    .iter()
                    .map(|c| WireColumn {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                        nullable: Some(c.nullable),
                        default: c.default_value.clone(),
                        primary_key: primary.contains(&c.name.to_lowercase()),
                    })
                    .collect();
                trail.end(
                    Decision::Allowed,
                    None,
                    Some(columns.len() as u64),
                    None,
                    None,
                );
                ServerMessage::TableDescription {
                    id,
                    table: table.unwrap_or_default(),
                    columns,
                }
            }
            Ok(WorkOutput::Rows(_)) => {
                trail.end(Decision::Error, Some(ErrorCode::Internal), None, None, None);
                error_message(
                    Some(id),
                    &GatewayError::new(ErrorCode::Internal, "internal error"),
                )
            }
            Err(e) => {
                trail.end(Decision::Error, Some(e.code), None, None, None);
                error_message(Some(id), &e)
            }
        }
    }

    async fn statement(
        &self,
        ctx: &RequestContext,
        id: u64,
        alias: String,
        sql: String,
        request: PolicyRequest,
        explain: bool,
    ) -> ServerMessage {
        let op = if explain { "explain" } else { "query" };
        let trail = self.trail(ctx, Some(&alias), op, Some(&sql));

        macro_rules! refuse {
            ($error:expr) => {{
                let error: GatewayError = $error;
                trail.refused(error.code);
                return error_message(Some(id), &error);
            }};
        }

        if sql.len() > MAX_SQL_BYTES {
            refuse!(GatewayError::new(
                ErrorCode::LimitExceeded,
                format!("statements are limited to {MAX_SQL_BYTES} bytes"),
            ));
        }
        let resolved = match self.resolve(&alias).await {
            Ok(resolved) => resolved,
            Err(e) => refuse!(e),
        };
        let Some(db_type) = database_type_for(&resolved.record.bound_target.engine) else {
            refuse!(GatewayError::new(
                ErrorCode::UnsupportedEngine,
                "unsupported engine",
            ));
        };

        // Effective limits: the record's ceiling, tightened by the request
        let policy = resolved.policy.tighten_with(request);

        let statement = match classify_for_broker(&sql, &db_type) {
            Ok(statement) => statement,
            Err(refusal) => refuse!(GatewayError::new(
                refusal_code(refusal.kind),
                refusal.reason,
            )),
        };
        if statement.class == StatementClass::Write && policy.access == AgentAccess::ReadOnly {
            refuse!(GatewayError::new(
                ErrorCode::ReadOnlyViolation,
                "this connection is read-only for agents; the statement would change data",
            ));
        }

        // What actually runs
        let exec_sql: String = if explain {
            if statement.verb == "EXPLAIN" {
                refuse!(GatewayError::new(
                    ErrorCode::QueryError,
                    "pass the statement itself; the explain operation adds EXPLAIN",
                ));
            }
            let planned = match db_type {
                DatabaseType::SQLite => format!("EXPLAIN QUERY PLAN {}", statement.sql),
                _ => format!("EXPLAIN {}", statement.sql),
            };
            // Belt and braces: what we are about to run must itself classify
            // as a plain read (EXPLAIN without ANALYZE never executes)
            match classify_for_broker(&planned, &db_type) {
                Ok(check) if check.class == StatementClass::Read => planned,
                _ => refuse!(GatewayError::new(
                    ErrorCode::DdlRefused,
                    "this statement cannot be explained",
                )),
            }
        } else {
            execution_sql(&statement, &policy)
        };

        if trail.start().is_err() {
            return audit_unavailable(id);
        }

        match self.execute(&resolved, &policy, Work::Sql(exec_sql)).await {
            Ok(WorkOutput::Rows(result)) => {
                let shaped = shape_rows(result, policy.max_rows as usize, policy.max_bytes);
                let (message, shaped) = query_result(id, shaped);
                trail.end(
                    Decision::Allowed,
                    None,
                    Some(shaped.rows.len() as u64),
                    Some(shaped.bytes),
                    Some(shaped.truncated),
                );
                message
            }
            Ok(_) => {
                trail.end(Decision::Error, Some(ErrorCode::Internal), None, None, None);
                error_message(
                    Some(id),
                    &GatewayError::new(ErrorCode::Internal, "internal error"),
                )
            }
            Err(e) => {
                trail.end(Decision::Error, Some(e.code), None, None, None);
                error_message(Some(id), &e)
            }
        }
    }

    /// Run `work` on the alias's connection: serialized per alias, bounded
    /// queue, lazily connected, discarded on error or timeout.
    async fn execute(
        &self,
        resolved: &Arc<ResolvedAlias>,
        policy: &AgentPolicy,
        work: Work,
    ) -> Result<WorkOutput, GatewayError> {
        let slot = self.slot(&resolved.alias);
        let budget = Duration::from_secs(u64::from(policy.timeout_secs));
        if slot.waiting.fetch_add(1, Ordering::SeqCst) >= MAX_QUEUED_PER_ALIAS {
            slot.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(GatewayError::new(
                ErrorCode::LimitExceeded,
                "too many requests are already waiting for this connection",
            ));
        }
        let lock = tokio::time::timeout(budget + DEADLINE_GRACE, slot.state.lock()).await;
        slot.waiting.fetch_sub(1, Ordering::SeqCst);
        let mut guard = lock.map_err(|_| {
            GatewayError::new(
                ErrorCode::Timeout,
                "timed out waiting for earlier requests on this connection",
            )
        })?;

        let key = LiveKey {
            revision: resolved.record.revision,
            read_only: resolved.policy.access == AgentAccess::ReadOnly,
            timeout_secs: policy.timeout_secs,
        };
        if guard.as_ref().is_none_or(|live| live.key != key) {
            *guard = None;
            let client = connect_for_record(resolved, key.read_only, key.timeout_secs).await?;
            *guard = Some(Live { client, key });
        }

        let outcome = {
            let live = guard
                .as_ref()
                .ok_or_else(|| GatewayError::new(ErrorCode::Internal, "connection unavailable"))?;
            tokio::time::timeout(budget + DEADLINE_GRACE, run_work(&*live.client, work)).await
        };
        match outcome {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => {
                // Unknown connection state after any error: reconnect lazily
                *guard = None;
                Err(map_db_error(&e))
            }
            Err(_) => {
                *guard = None;
                Err(GatewayError::new(
                    ErrorCode::Timeout,
                    format!("the statement exceeded the {}s limit", policy.timeout_secs),
                ))
            }
        }
    }
}

fn audit_unavailable(id: u64) -> ServerMessage {
    error_message(
        Some(id),
        &GatewayError::new(
            ErrorCode::Internal,
            "the audit log is unavailable, so the request was refused",
        ),
    )
}

/// The SQL to run for a classified statement: SELECTs get the auto-LIMIT of
/// `max_rows + 1`; everything else runs as classified.
fn execution_sql(statement: &BrokerStatement, policy: &AgentPolicy) -> String {
    if statement.is_select {
        statement.with_row_limit(policy.max_rows.saturating_add(1))
    } else {
        statement.sql.clone()
    }
}

/// Lower-cased names of the columns in the table's primary key, parsed from
/// the index definition (`PRIMARY KEY, btree (id, tenant_id)` / `(id)`).
fn primary_key_columns(details: &dbcrust_core::model::TableDetails) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for index in details.indexes.iter().filter(|i| i.is_primary) {
        let definition = &index.definition;
        if let (Some(open), Some(close)) = (definition.rfind('('), definition.rfind(')')) {
            if open < close {
                for part in definition[open + 1..close].split(',') {
                    let name = part
                        .trim()
                        .trim_matches(|c| matches!(c, '"' | '`' | '[' | ']'))
                        .to_lowercase();
                    if !name.is_empty() {
                        names.insert(name);
                    }
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("users", true)]
    #[case("public.users", true)]
    #[case("db.public.users", true)]
    #[case("\"My Table\"", true)]
    #[case("public.\"Odd.Name\"", true)]
    #[case("order-items", true)]
    #[case("", false)]
    #[case("a.b.c.d", false)]
    #[case("users; DROP TABLE users", false)]
    #[case("users'--", false)]
    #[case("users\"", false)]
    #[case("\"a\"b", false)]
    #[case("\"unterminated", false)]
    #[case("a..b", false)]
    #[case(".a", false)]
    #[case("a.", false)]
    #[case("a`b", false)]
    #[case("a\\b", false)]
    #[case("a\nb", false)]
    #[case("\"a;b\"", false)]
    fn table_name_validation(#[case] name: &str, #[case] ok: bool) {
        assert_eq!(validate_table_name(name).is_ok(), ok, "{name:?}");
    }

    fn result(rows: usize, cell: &str) -> StructuredQueryResult {
        StructuredQueryResult {
            columns: vec!["a".to_string()],
            rows: (0..rows).map(|_| vec![Some(cell.to_string())]).collect(),
        }
    }

    #[test]
    fn row_cap_truncates_and_flags() {
        let shaped = shape_rows(result(10, "x"), 3, u64::MAX);
        assert_eq!(shaped.rows.len(), 3);
        assert!(shaped.truncated);
        let exact = shape_rows(result(3, "x"), 3, u64::MAX);
        assert_eq!(exact.rows.len(), 3);
        assert!(!exact.truncated);
    }

    #[test]
    fn byte_budget_truncates_and_flags() {
        // each row: 100 bytes + 2 overhead; column name 1 byte
        let cell = "y".repeat(100);
        let shaped = shape_rows(result(10, &cell), 1000, 350);
        assert_eq!(shaped.rows.len(), 3);
        assert!(shaped.truncated);
        assert!(shaped.bytes <= 350);
        // a single row over budget yields zero rows but still flags truncation
        let none = shape_rows(result(2, &cell), 1000, 50);
        assert!(none.rows.is_empty());
        assert!(none.truncated);
    }

    #[test]
    fn oversize_frames_are_trimmed_to_fit() {
        let cell = "z".repeat(1024 * 1024);
        let shaped = Shaped {
            columns: vec!["a".to_string()],
            rows: (0..40).map(|_| vec![Some(cell.clone())]).collect(),
            truncated: false,
            bytes: 0,
        };
        let (message, shaped) = query_result(1, shaped);
        let frame = dbcrust_agent_protocol::encode(&message).unwrap();
        assert!(frame.len() <= MAX_FRAME_LEN + 4);
        assert!(shaped.truncated);
        assert!(!shaped.rows.is_empty());
    }

    #[rstest]
    #[case(
        "cannot execute INSERT in a read-only transaction",
        ErrorCode::ReadOnlyViolation
    )]
    #[case("attempt to write a readonly database", ErrorCode::ReadOnlyViolation)]
    #[case("canceling statement due to statement timeout", ErrorCode::Timeout)]
    #[case(
        "Query execution was interrupted, maximum statement execution time exceeded",
        ErrorCode::Timeout
    )]
    #[case("interrupted", ErrorCode::Timeout)]
    #[case("syntax error at or near \"FROM\"", ErrorCode::QueryError)]
    fn database_errors_map_to_protocol_codes(#[case] message: &str, #[case] code: ErrorCode) {
        let error = DatabaseError::QueryError(message.to_string());
        assert_eq!(map_db_error(&error).code, code);
    }

    #[test]
    fn error_messages_are_redacted_and_bounded() {
        let error = DatabaseError::ConnectionError(
            "failed postgres://alice:hunter2-canary@db.internal/app".to_string(),
        );
        let mapped = map_db_error(&error);
        assert_eq!(mapped.code, ErrorCode::ConnectionError);
        assert!(
            !mapped.message.contains("hunter2-canary"),
            "{}",
            mapped.message
        );
        let long = DatabaseError::QueryError("x".repeat(5000));
        assert!(map_db_error(&long).message.chars().count() <= MAX_ERROR_CHARS + 1);
    }
}
