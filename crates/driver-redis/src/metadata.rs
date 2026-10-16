//! Browsing a keyspace: bounded `SCAN` listings (never `KEYS`), key types,
//! `INFO keyspace`, and a described key (`TYPE`, `TTL`, size, sample).

use crate::conn::{command, send};
use crate::reply::render_bytes;
use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::model::{ColumnInfo, TableDetails};
use redis::Value;
use redis::aio::ConnectionManager;
use std::collections::{HashMap, HashSet};
use tokio::sync::OnceCell;

/// Keys listed by `\dt` and used for completion (`scan_limit` option).
pub(crate) const DEFAULT_SCAN_LIMIT: usize = 1000;
/// Members/fields/entries sampled by `\d key` (`sample_size` option).
pub(crate) const DEFAULT_SAMPLE_SIZE: usize = 20;
/// Keys per `SCAN` page and per `TYPE` pipeline.
const PAGE: usize = 500;
/// `SCAN` pages one listing may walk before giving up (a pattern that
/// matches nothing in a huge keyspace must not scan it forever).
const MAX_SCAN_PAGES: usize = 2000;
/// Longest value preview in a described key.
const PREVIEW_CHARS: usize = 200;

pub struct RedisMetadataProvider {
    manager: ConnectionManager,
    db: i64,
    scan_limit: usize,
    sample_size: usize,
    commands: OnceCell<Vec<String>>,
}

impl RedisMetadataProvider {
    pub(crate) fn new(
        manager: ConnectionManager,
        db: i64,
        scan_limit: usize,
        sample_size: usize,
    ) -> Self {
        Self {
            manager,
            db,
            scan_limit,
            sample_size,
            commands: OnceCell::new(),
        }
    }

    fn schema_name(&self) -> String {
        format!("db{}", self.db)
    }

    /// `Ok` when `schema` names the connected database (or is not given).
    fn check_schema(&self, schema: Option<&str>) -> Result<(), DatabaseError> {
        let Some(schema) = schema.filter(|schema| !schema.is_empty()) else {
            return Ok(());
        };
        let wanted = schema.strip_prefix("db").unwrap_or(schema);
        if wanted.parse::<i64>().ok() == Some(self.db) {
            Ok(())
        } else {
            Err(DatabaseError::MetadataError(format!(
                "'{schema}' is not the connected database (db{}); switch with \\c <number>",
                self.db
            )))
        }
    }

    async fn run(&self, parts: &[&str]) -> Result<Value, DatabaseError> {
        send(&self.manager, &command(parts)).await
    }

    /// Up to `limit` distinct key names, found with `SCAN` pages.
    pub(crate) async fn scan_keys(
        &self,
        pattern: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, DatabaseError> {
        let mut keys = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = "0".to_string();
        let page = PAGE.to_string();
        for _ in 0..MAX_SCAN_PAGES {
            let mut parts = vec!["SCAN", cursor.as_str(), "COUNT", page.as_str()];
            if let Some(pattern) = pattern {
                parts.extend(["MATCH", pattern]);
            }
            let reply = self.run(&parts).await?;
            let Value::Array(reply) = reply else {
                return Err(DatabaseError::MetadataError(
                    "unexpected SCAN reply".to_string(),
                ));
            };
            let [next, Value::Array(page_keys)] = reply.as_slice() else {
                return Err(DatabaseError::MetadataError(
                    "unexpected SCAN reply".to_string(),
                ));
            };
            for key in page_keys {
                if let Value::BulkString(bytes) = key {
                    let key = render_bytes(bytes);
                    if seen.insert(key.clone()) {
                        keys.push(key);
                        if keys.len() >= limit {
                            return Ok(keys);
                        }
                    }
                }
            }
            cursor = match next {
                Value::BulkString(bytes) => render_bytes(bytes),
                Value::SimpleString(text) => text.clone(),
                Value::Int(number) => number.to_string(),
                _ => break,
            };
            if cursor == "0" {
                break;
            }
        }
        Ok(keys)
    }

    /// `TYPE` of every key, pipelined (best effort: an ACL that denies `TYPE`
    /// yields no kinds, not an error).
    pub(crate) async fn key_types(&self, keys: &[String]) -> HashMap<String, String> {
        let mut kinds = HashMap::new();
        for chunk in keys.chunks(PAGE) {
            let mut pipeline = redis::pipe();
            for key in chunk {
                pipeline.cmd("TYPE").arg(key.as_str());
            }
            let mut connection = self.manager.clone();
            match pipeline.query_async::<Vec<String>>(&mut connection).await {
                Ok(types) => {
                    kinds.extend(chunk.iter().cloned().zip(types));
                }
                Err(_) => return HashMap::new(),
            }
        }
        kinds
    }

    /// `INFO keyspace`: the databases that hold keys, as `db<N>`.
    async fn populated_databases(&self) -> Result<Vec<i64>, DatabaseError> {
        let reply = self.run(&["INFO", "keyspace"]).await?;
        let text = match reply {
            Value::BulkString(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Value::VerbatimString { text, .. } => text,
            other => {
                return Err(DatabaseError::MetadataError(format!(
                    "unexpected INFO reply: {other:?}"
                )));
            }
        };
        Ok(keyspace_databases(&text))
    }

    pub(crate) async fn populated_database_names(&self) -> Result<Vec<i64>, DatabaseError> {
        let mut databases = self.populated_databases().await?;
        if !databases.contains(&self.db) {
            databases.push(self.db);
            databases.sort_unstable();
        }
        Ok(databases)
    }

    async fn text(&self, parts: &[&str]) -> Option<String> {
        match self.run(parts).await.ok()? {
            Value::BulkString(bytes) => Some(render_bytes(&bytes)),
            Value::SimpleString(text) => Some(text),
            Value::Int(number) => Some(number.to_string()),
            Value::Double(number) => Some(number.to_string()),
            _ => None,
        }
    }

    /// `TYPE` of an existing key.
    async fn key_type(&self, key: &str) -> Result<String, DatabaseError> {
        match self.text(&["TYPE", key]).await {
            Some(kind) if kind != "none" => Ok(kind),
            _ => Err(DatabaseError::MetadataError(format!(
                "key '{key}' does not exist"
            ))),
        }
    }

    async fn integer(&self, parts: &[&str]) -> Option<i64> {
        self.text(parts).await?.parse().ok()
    }
}

/// `db0:keys=3,expires=0,avg_ttl=0` lines → database numbers.
pub(crate) fn keyspace_databases(info: &str) -> Vec<i64> {
    let mut databases: Vec<i64> = info
        .lines()
        .filter_map(|line| {
            let (name, _) = line.trim().split_once(':')?;
            name.strip_prefix("db")?.parse().ok()
        })
        .collect();
    databases.sort_unstable();
    databases.dedup();
    databases
}

fn preview(text: &str) -> String {
    let mut out: String = text.chars().take(PREVIEW_CHARS).collect();
    if text.chars().count() > PREVIEW_CHARS {
        out.push('…');
    }
    out
}

fn member(name: impl Into<String>, data_type: &str, value: Option<String>) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        data_type: data_type.to_string(),
        collation: String::new(),
        nullable: true,
        default_value: value,
        enum_values: None,
    }
}

fn bulk_text(value: &Value) -> String {
    match value {
        Value::BulkString(bytes) => render_bytes(bytes),
        Value::SimpleString(text) => text.clone(),
        Value::Int(number) => number.to_string(),
        Value::Double(number) => number.to_string(),
        Value::Nil => "NULL".to_string(),
        other => crate::reply::to_json(other).to_string(),
    }
}

/// Pairs out of a flat RESP2 list or a RESP3 map.
fn flat_pairs(value: &Value) -> Vec<(String, String)> {
    match value {
        Value::Map(pairs) => pairs
            .iter()
            .map(|(key, value)| (bulk_text(key), bulk_text(value)))
            .collect(),
        Value::Array(items) => items
            .chunks(2)
            .map(|chunk| {
                (
                    bulk_text(&chunk[0]),
                    chunk.get(1).map_or_else(String::new, bulk_text),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[async_trait]
impl MetadataProvider for RedisMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        Ok(self
            .populated_database_names()
            .await?
            .into_iter()
            .map(|db| format!("db{db}"))
            .collect())
    }

    /// The keys of the connected database (bounded by `scan_limit`), found
    /// with `SCAN`. Key names double as completion candidates.
    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        self.check_schema(schema)?;
        let keys = self.scan_keys(None, self.scan_limit).await?;
        if keys.len() >= self.scan_limit {
            tracing::debug!(
                "Redis key listing stopped at {} keys (raise `scan_limit` to see more)",
                self.scan_limit
            );
        }
        Ok(keys)
    }

    /// `TYPE` of each listed key (`\dt` shows it in the Type column).
    async fn get_table_kinds(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, String>, DatabaseError> {
        Ok(self.key_types(names).await)
    }

    /// Field names of a hash, element positions of a list, … (sampled), or
    /// `value` for a string.
    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        self.check_schema(schema)?;
        let kind = self.key_type(table).await?;
        Ok(self
            .sample(&kind, table)
            .await?
            .into_iter()
            .map(|column| column.name)
            .collect())
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let commands = self
            .commands
            .get_or_init(|| async {
                let from_server = match self.run(&["COMMAND", "LIST"]).await {
                    Ok(Value::Array(items)) => items
                        .iter()
                        .map(bulk_text)
                        .map(|name| name.to_ascii_uppercase())
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                if from_server.is_empty() {
                    dbcrust_core::redis_command::COMMON_COMMANDS
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect()
                } else {
                    let mut names = from_server;
                    names.sort_unstable();
                    names.dedup();
                    names
                }
            })
            .await;
        Ok(commands.clone())
    }

    /// One key: its `TYPE`, `TTL`, memory, size, and a sample of members.
    ///
    /// The result is shaped like a table so every frontend can show it:
    /// the leading `type`/`ttl`/`memory`/`length` rows are the properties
    /// (their value is in the Type column), the rest are sampled members
    /// (value in the Default column).
    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        self.check_schema(schema)?;
        let kind = self.key_type(table).await?;

        let mut columns = vec![member("type", &kind, None)];
        let ttl = match self.integer(&["PTTL", table]).await {
            Some(-1) => "no expiry".to_string(),
            Some(millis) if millis >= 0 => format_ttl(millis),
            _ => "unknown".to_string(),
        };
        columns.push(member("ttl", &ttl, None));
        if let Some(bytes) = self.integer(&["MEMORY", "USAGE", table]).await {
            columns.push(member("memory", &format!("{bytes} bytes"), None));
        }
        let (length_command, unit) = match kind.as_str() {
            "string" => (Some("STRLEN"), "bytes"),
            "list" => (Some("LLEN"), "elements"),
            "hash" => (Some("HLEN"), "fields"),
            "set" => (Some("SCARD"), "members"),
            "zset" => (Some("ZCARD"), "members"),
            "stream" => (Some("XLEN"), "entries"),
            _ => (None, ""),
        };
        if let Some(command_name) = length_command {
            if let Some(length) = self.integer(&[command_name, table]).await {
                columns.push(member("length", &format!("{length} {unit}"), None));
            }
        }
        if let Some(encoding) = self.text(&["OBJECT", "ENCODING", table]).await {
            columns.push(member("encoding", &encoding, None));
        }

        columns.extend(self.sample(&kind, table).await?);

        Ok(TableDetails {
            name: table.to_string(),
            schema: self.schema_name(),
            full_name: format!("{}.{table}", self.schema_name()),
            columns,
            indexes: Vec::new(),
            check_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
            nested_field_details: HashMap::new(),
        })
    }

    fn supports_explain(&self) -> bool {
        false
    }

    fn default_schema(&self) -> Option<String> {
        Some(self.schema_name())
    }
}

fn format_ttl(millis: i64) -> String {
    let seconds = millis / 1000;
    if millis % 1000 == 0 || seconds >= 10 {
        format!("{seconds} seconds")
    } else {
        format!("{millis} ms")
    }
}

impl RedisMetadataProvider {
    /// A bounded sample of the key's members as column rows.
    async fn sample(&self, kind: &str, key: &str) -> Result<Vec<ColumnInfo>, DatabaseError> {
        let count = self.sample_size.max(1);
        let last = (count - 1).to_string();
        let count_text = count.to_string();
        let mut rows = Vec::new();
        match kind {
            "string" => {
                let value = self
                    .text(&["GETRANGE", key, "0", &(PREVIEW_CHARS * 4).to_string()])
                    .await;
                rows.push(member("value", "string", value.map(|v| preview(&v))));
            }
            "hash" => {
                let reply = self.run(&["HSCAN", key, "0", "COUNT", &count_text]).await?;
                if let Value::Array(parts) = reply {
                    if let Some(Value::Array(flat)) = parts.get(1) {
                        for (field, value) in flat_pairs(&Value::Array(flat.clone()))
                            .into_iter()
                            .take(count)
                        {
                            rows.push(member(field, "string", Some(preview(&value))));
                        }
                    }
                }
            }
            "list" => {
                if let Value::Array(items) = self.run(&["LRANGE", key, "0", &last]).await? {
                    for (index, item) in items.iter().enumerate() {
                        rows.push(member(
                            index.to_string(),
                            "string",
                            Some(preview(&bulk_text(item))),
                        ));
                    }
                }
            }
            "set" => {
                let reply = self.run(&["SSCAN", key, "0", "COUNT", &count_text]).await?;
                if let Value::Array(parts) = reply {
                    if let Some(Value::Array(members)) = parts.get(1) {
                        for item in members.iter().take(count) {
                            rows.push(member(preview(&bulk_text(item)), "member", None));
                        }
                    }
                }
            }
            "zset" => {
                let reply = self.run(&["ZRANGE", key, "0", &last, "WITHSCORES"]).await?;
                for (name, score) in flat_pairs_or_nested(&reply) {
                    rows.push(member(preview(&name), "member (score)", Some(score)));
                }
            }
            "stream" => {
                let reply = self
                    .run(&["XRANGE", key, "-", "+", "COUNT", &count_text])
                    .await?;
                if let Value::Array(entries) = reply {
                    for entry in entries {
                        if let Value::Array(parts) = entry {
                            let id = parts.first().map(bulk_text).unwrap_or_default();
                            let fields = parts.get(1).map(flat_pairs).unwrap_or_default();
                            let rendered = fields
                                .iter()
                                .map(|(field, value)| format!("{field}={value}"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            rows.push(member(id, "entry", Some(preview(&rendered))));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(rows)
    }
}

/// `[member, score, …]` (RESP2) or `[[member, score], …]` (RESP3).
fn flat_pairs_or_nested(value: &Value) -> Vec<(String, String)> {
    match value {
        Value::Array(items)
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| matches!(item, Value::Array(inner) if inner.len() == 2)) =>
        {
            items
                .iter()
                .filter_map(|item| match item {
                    Value::Array(inner) => Some((bulk_text(&inner[0]), bulk_text(&inner[1]))),
                    _ => None,
                })
                .collect()
        }
        other => flat_pairs(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyspace_lines_name_the_populated_databases() {
        let info =
            "# Keyspace\r\ndb0:keys=3,expires=1,avg_ttl=0\r\ndb7:keys=1,expires=0,avg_ttl=0\r\n";
        assert_eq!(keyspace_databases(info), vec![0, 7]);
        assert!(keyspace_databases("# Keyspace\r\n").is_empty());
    }

    #[test]
    fn ttl_is_human_readable() {
        assert_eq!(format_ttl(3_599_000), "3599 seconds");
        assert_eq!(format_ttl(1500), "1500 ms");
    }

    #[test]
    fn long_values_are_previewed() {
        let long = "x".repeat(500);
        let text = preview(&long);
        assert_eq!(text.chars().count(), PREVIEW_CHARS + 1);
        assert!(text.ends_with('…'));
        assert_eq!(preview("short"), "short");
    }

    #[test]
    fn scored_members_flat_and_nested() {
        let flat = Value::Array(vec![
            Value::BulkString(b"a".to_vec()),
            Value::BulkString(b"1".to_vec()),
        ]);
        assert_eq!(flat_pairs_or_nested(&flat), vec![("a".into(), "1".into())]);
        let nested = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"a".to_vec()),
            Value::Double(1.5),
        ])]);
        assert_eq!(
            flat_pairs_or_nested(&nested),
            vec![("a".into(), "1.5".into())]
        );
    }
}
