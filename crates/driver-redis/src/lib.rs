//! Redis implementation of the database abstraction layer.
//!
//! Redis has no query language, so a DBCrust "query" for a Redis connection is
//! one command line in `redis-cli` syntax (`HGETALL user:1`,
//! `SCAN 0 MATCH user:* COUNT 100`, `SET "a b" 'c d'`). The reply is mapped to
//! a table by shape (see [`reply`]); metadata browsing (`\dt`, `\d key`) uses
//! bounded `SCAN`s and never `KEYS`.
//!
//! Replies are read whole by the `redis` crate, so the row cap (`max_rows`
//! option, 10 000 by default) bounds what is *rendered*, not what the server
//! sends: bound the request itself (`LRANGE k 0 99`, `COUNT n`) for huge
//! collections.
//!
//! Wire-compatible servers (Valkey, KeyDB, Dragonfly) work unchanged.

mod conn;
mod connect;
mod metadata;
mod reply;

use async_trait::async_trait;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider, ServerInfo,
    StructuredQueryResult, connection_read_only,
};
use dbcrust_core::redis_command::{command_name, split_command_line, subcommand_name};
use metadata::{DEFAULT_SAMPLE_SIZE, DEFAULT_SCAN_LIMIT};
use redis::Value;
use redis::aio::ConnectionManager;
use std::collections::HashMap;
use std::time::Duration;

pub use metadata::RedisMetadataProvider;

/// Rows rendered from one reply unless the `max_rows` option says otherwise.
const DEFAULT_MAX_ROWS: usize = 10_000;

/// Connection option: rows rendered from one reply.
const OPT_MAX_ROWS: &str = "max_rows";
/// Connection option: keys listed by `\dt` and used for completion.
const OPT_SCAN_LIMIT: &str = "scan_limit";
/// Connection option: members shown by `\d key`.
const OPT_SAMPLE_SIZE: &str = "sample_size";

/// A reply mapped to a table, with whether `max_rows` cut it short.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedResult {
    pub result: StructuredQueryResult,
    pub truncated: bool,
}

/// Commands this client does not run: they put the connection in a mode
/// (subscribe, monitor, transaction, other identity) that a request/response
/// client sharing one multiplexed connection cannot honour.
const UNSUPPORTED_COMMANDS: &[(&str, &str)] = &[
    (
        "SUBSCRIBE",
        "subscriptions need a dedicated connection; use redis-cli",
    ),
    (
        "PSUBSCRIBE",
        "subscriptions need a dedicated connection; use redis-cli",
    ),
    (
        "SSUBSCRIBE",
        "subscriptions need a dedicated connection; use redis-cli",
    ),
    ("UNSUBSCRIBE", "subscriptions are not supported"),
    ("PUNSUBSCRIBE", "subscriptions are not supported"),
    ("SUNSUBSCRIBE", "subscriptions are not supported"),
    ("MONITOR", "MONITOR streams replies; use redis-cli"),
    ("SYNC", "replication streams are not supported"),
    ("PSYNC", "replication streams are not supported"),
    (
        "MULTI",
        "transactions are not supported (each command runs on its own)",
    ),
    ("EXEC", "transactions are not supported"),
    ("DISCARD", "transactions are not supported"),
    ("WATCH", "transactions are not supported"),
    ("UNWATCH", "transactions are not supported"),
    (
        "SELECT",
        "switch databases with `\\c <number>` or the /N path of the URL",
    ),
    (
        "AUTH",
        "authenticate with the URL's user and password (or the secret store)",
    ),
    ("HELLO", "the protocol is fixed per connection"),
    ("RESET", "the connection state is managed by DBCrust"),
    ("QUIT", "use \\q to leave DBCrust"),
];

/// `CLIENT` subcommands that change how the connection answers.
const UNSUPPORTED_CLIENT_SUBCOMMANDS: &[&str] = &["REPLY", "TRACKING", "CACHING", "SETINFO"];

/// Redis client: one multiplexed, auto-reconnecting connection.
pub struct RedisClient {
    connection_info: ConnectionInfo,
    manager: ConnectionManager,
    db: i64,
    max_rows: usize,
    scan_limit: usize,
    sample_size: usize,
    metadata_provider: RedisMetadataProvider,
}

fn usize_option(info: &ConnectionInfo, key: &str, default: usize) -> usize {
    info.options
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

impl RedisClient {
    /// Connect (and authenticate, and select the database of the URL path).
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let db = connect::database_index(&connection_info)?;
        let manager = conn::open(&connection_info, db).await?;
        let max_rows = usize_option(&connection_info, OPT_MAX_ROWS, DEFAULT_MAX_ROWS);
        let scan_limit = usize_option(&connection_info, OPT_SCAN_LIMIT, DEFAULT_SCAN_LIMIT);
        let sample_size = usize_option(&connection_info, OPT_SAMPLE_SIZE, DEFAULT_SAMPLE_SIZE);
        let metadata_provider =
            RedisMetadataProvider::new(manager.clone(), db, scan_limit, sample_size);
        Ok(Self {
            connection_info,
            manager,
            db,
            max_rows,
            scan_limit,
            sample_size,
            metadata_provider,
        })
    }

    fn unsupported(&self, feature: impl Into<String>) -> DatabaseError {
        DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::Redis,
            feature: feature.into(),
        }
    }

    /// Run one command line and report whether the row cap cut the reply.
    pub async fn execute_limited(&self, line: &str) -> Result<LimitedResult, DatabaseError> {
        let args = split_command_line(line)
            .map_err(|error| DatabaseError::QueryError(error.to_string()))?;
        let name = command_name(&args);

        if let Some((_, reason)) = UNSUPPORTED_COMMANDS
            .iter()
            .find(|(command, _)| *command == name)
        {
            return Err(self.unsupported(format!("{name} ({reason})")));
        }
        if name == "CLIENT" {
            if let Some(sub) = subcommand_name(&args) {
                if UNSUPPORTED_CLIENT_SUBCOMMANDS.contains(&sub.as_str()) {
                    return Err(self.unsupported(format!(
                        "CLIENT {sub} (it would change how the shared connection answers)"
                    )));
                }
            }
        }

        // Connections built for the credential broker refuse writes here too,
        // whatever the caller already checked
        if connection_read_only(&self.connection_info.options) {
            dbcrust_core::safety::check_read_only(line, &DatabaseType::Redis)
                .map_err(|violation| DatabaseError::QueryError(violation.to_string()))?;
        }

        let command = conn::command_from_args(&args);
        let value = conn::send(&self.manager, &command).await?;
        let mapped = reply::map_reply(reply::shape_for(&args), &value, self.max_rows)
            .map_err(DatabaseError::QueryError)?;
        if mapped.truncated {
            tracing::warn!(
                "Redis reply to {name} cut at {} rows (`max_rows` option); bound the request instead",
                self.max_rows
            );
        }
        Ok(LimitedResult {
            result: mapped.result,
            truncated: mapped.truncated,
        })
    }

    async fn info_section(&self, section: &str) -> Result<HashMap<String, String>, DatabaseError> {
        let reply = conn::send(&self.manager, &conn::command(&["INFO", section])).await?;
        let text = match reply {
            Value::BulkString(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Value::VerbatimString { text, .. } => text,
            other => {
                return Err(DatabaseError::QueryError(format!(
                    "unexpected INFO reply: {other:?}"
                )));
            }
        };
        Ok(parse_info(&text))
    }
}

/// `key:value` lines of an `INFO` reply (section headers dropped).
fn parse_info(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[async_trait]
impl DatabaseClient for RedisClient {
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Ok(self
            .execute_limited(sql)
            .await?
            .result
            .into_display_rows("NULL"))
    }

    async fn execute_query_structured(
        &self,
        sql: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        Ok(self.execute_limited(sql).await?.result)
    }

    /// Validates the command line; runs it only when it is a read.
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        split_command_line(sql).map_err(|error| DatabaseError::QueryError(error.to_string()))?;
        if dbcrust_core::safety::check_read_only(sql, &DatabaseType::Redis).is_ok() {
            self.execute_limited(sql).await?;
        }
        Ok(())
    }

    async fn explain_query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(self.unsupported("EXPLAIN (Redis has no query planner)"))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    /// The databases that hold keys (plus the connected one), with key counts.
    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let keyspace = self.info_section("keyspace").await?;
        let mut rows = vec![vec!["Database".to_string(), "Keys".to_string()]];
        let mut databases = self.metadata_provider.populated_database_names().await?;
        databases.sort_unstable();
        for db in databases {
            let keys = keyspace
                .get(&format!("db{db}"))
                .and_then(|stats| {
                    stats
                        .split(',')
                        .find_map(|part| part.strip_prefix("keys=").map(str::to_string))
                })
                .unwrap_or_else(|| "0".to_string());
            rows.push(vec![db.to_string(), keys]);
        }
        Ok(rows)
    }

    /// Open a new connection on database `database` (`3` or `db3`).
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        let wanted = database
            .trim()
            .strip_prefix("db")
            .unwrap_or(database.trim());
        let db: i64 = wanted.parse().ok().filter(|db| *db >= 0).ok_or_else(|| {
            DatabaseError::InvalidUrl(format!(
                "a Redis database is a number (\\c 3), got '{database}'"
            ))
        })?;
        let manager = conn::open(&self.connection_info, db).await?;
        self.metadata_provider =
            RedisMetadataProvider::new(manager.clone(), db, self.scan_limit, self.sample_size);
        self.manager = manager;
        self.db = db;
        self.connection_info.database = Some(db.to_string());
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.db.to_string()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        let ping = conn::command(&["PING"]);
        matches!(
            tokio::time::timeout(Duration::from_secs(5), conn::send(&self.manager, &ping)).await,
            Ok(Ok(_))
        )
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // The manager closes its socket when the last clone is dropped
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let server = self.info_section("server").await?;
        // Valkey answers `redis_version:7.2.4` for compatibility and names
        // itself in `server_name` / `valkey_version`
        let (server_type, version) = match server.get("valkey_version") {
            Some(version) => ("Valkey", version.clone()),
            None => (
                "Redis",
                server.get("redis_version").cloned().unwrap_or_default(),
            ),
        };
        let mut info = ServerInfo::new(server_type.to_string(), version);
        info.parse_version_numbers();
        info.supports_transactions = false;
        info.supports_roles = info.version_major.is_some_and(|major| major >= 6);
        for key in ["redis_mode", "os", "tcp_port", "uptime_in_seconds"] {
            if let Some(value) = server.get(key) {
                info.additional_info.insert(key.to_string(), value.clone());
            }
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests;
