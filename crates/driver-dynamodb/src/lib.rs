//! Amazon DynamoDB for DBCrust.
//!
//! Two read surfaces, both bounded: PartiQL (`SELECT … FROM "Table" WHERE …`,
//! through `ExecuteStatement`) and a few typed commands (see
//! [`dbcrust_core::dynamodb_command`]): `tables`, `describe`, `scan`, `get`,
//! `query`, `help`. Nothing writes: the only PartiQL statement accepted is a
//! single `SELECT`, no command puts, updates or deletes, so `--read-only`
//! accepts exactly what runs and the IAM policy of the credentials stays the
//! server-side limit.
//!
//! DynamoDB bills by what it reads, so every request is bounded before it is
//! sent: a `Limit` that never exceeds the rows still wanted, a page budget on
//! every loop, and every call raced against Ctrl-C and the query timeout
//! (dropping the SDK future aborts the HTTP request). The capacity units a
//! command consumed are reported once it ends; a `scan` always warns.
//!
//! Credentials, region, endpoint and the rustls-on-ring HTTP client come from
//! `dbcrust-aws`.
//!
//! Options (connection URL query): the AWS ones of `dbcrust-aws` (`region`,
//! `profile`, `endpoint` …) plus `max_rows` (items per result, default 1000),
//! `max_pages` (requests per command, default 10), `scan_limit` (tables `\dt`
//! and completion list, default 1000) and `consistent_read=true`.

mod errors;
mod metadata;
mod ops;
mod value;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use dbcrust_aws::{AwsSettings, Region};
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider, ServerInfo,
    StructuredQueryResult,
};
use dbcrust_core::dynamodb_command;
use errors::{is_denied, map_error};
use ops::TableInfo;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use metadata::DynamoMetadataProvider;

const OPT_MAX_ROWS: &str = "max_rows";
const OPT_MAX_PAGES: &str = "max_pages";
const OPT_SCAN_LIMIT: &str = "scan_limit";
const OPT_CONSISTENT_READ: &str = "consistent_read";

const DEFAULT_MAX_ROWS: usize = 1000;
const MAX_MAX_ROWS: usize = 100_000;
const DEFAULT_MAX_PAGES: usize = 10;
const DEFAULT_SCAN_LIMIT: usize = 1000;
/// SigV4 needs a region even for endpoints that ignore it (DynamoDB Local).
const FALLBACK_REGION: &str = "us-east-1";

/// A command's table, with whether a bound cut it short and what to tell the user.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedResult {
    pub result: StructuredQueryResult,
    pub truncated: bool,
    /// Things the user should see: a scan's cost, a cut-short result.
    pub warnings: Vec<String>,
    /// Facts about the call: the capacity units it consumed.
    pub notices: Vec<String>,
}

impl LimitedResult {
    pub(crate) fn new(result: StructuredQueryResult) -> Self {
        Self {
            result,
            truncated: false,
            warnings: Vec::new(),
            notices: Vec::new(),
        }
    }
}

pub(crate) struct Limits {
    pub(crate) max_rows: usize,
    pub(crate) max_pages: usize,
    pub(crate) scan_limit: usize,
    pub(crate) consistent_read: bool,
}

/// What the client and its metadata provider share.
pub(crate) struct Shared {
    pub(crate) client: Client,
    table: Mutex<Option<String>>,
    infos: Mutex<HashMap<String, Arc<TableInfo>>>,
    pub(crate) limits: Limits,
    /// Secret values scrubbed from every message.
    secrets: Vec<String>,
    pub(crate) region: Option<String>,
    pub(crate) endpoint: Option<String>,
    credentials: String,
}

impl Shared {
    pub(crate) fn secrets(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }

    pub(crate) fn table(&self) -> Option<String> {
        self.table.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_table(&self, table: Option<String>) {
        *self.table.lock().unwrap_or_else(|e| e.into_inner()) = table;
    }

    pub(crate) fn cached_info(&self, table: &str) -> Option<Arc<TableInfo>> {
        self.infos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(table)
            .cloned()
    }

    pub(crate) fn cache_info(&self, table: &str, info: Arc<TableInfo>) {
        self.infos
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(table.to_string(), info);
    }

    /// Prove the endpoint answers and the credentials work. Without a table the
    /// account is probed with ListTables; with one, DescribeTable proves it
    /// exists. Being denied either is not fatal (a policy that only grants
    /// GetItem and Query on one table still lets the commands work), but an
    /// unknown table, bad credentials or an unreachable endpoint are.
    async fn preflight(&self, table: Option<&str>) -> Result<(), DatabaseError> {
        let secrets = self.secrets();
        match table {
            Some(table) => {
                let request = self.client.describe_table().table_name(table);
                match self
                    .guard(async {
                        match request.send().await {
                            Ok(_) => Ok(()),
                            Err(error) if is_denied(&error) => {
                                tracing::debug!("DescribeTable denied for {table}; continuing");
                                Ok(())
                            }
                            Err(error) => Err(map_error(error, &secrets)),
                        }
                    })
                    .await
                {
                    Ok(()) => Ok(()),
                    Err(DatabaseError::QueryError(message)) => Err(DatabaseError::ConnectionError(
                        format!("cannot open table {table}: {message}"),
                    )),
                    Err(other) => Err(other),
                }
            }
            None => {
                let request = self.client.list_tables().limit(1);
                self.guard(async {
                    match request.send().await {
                        Ok(_) => Ok(()),
                        // Listing tables is its own permission
                        Err(error) if is_denied(&error) => Ok(()),
                        Err(error) => Err(map_error(error, &secrets)),
                    }
                })
                .await
            }
        }
    }
}

fn size_option(info: &ConnectionInfo, key: &str, default: u64, max: u64) -> u64 {
    info.options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map_or(default, |value| value.min(max))
}

fn limits_from(info: &ConnectionInfo) -> Limits {
    let consistent_read = info
        .options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(OPT_CONSISTENT_READ))
        .is_some_and(|(_, value)| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        });
    Limits {
        max_rows: size_option(
            info,
            OPT_MAX_ROWS,
            DEFAULT_MAX_ROWS as u64,
            MAX_MAX_ROWS as u64,
        ) as usize,
        max_pages: size_option(info, OPT_MAX_PAGES, DEFAULT_MAX_PAGES as u64, 1_000) as usize,
        scan_limit: size_option(info, OPT_SCAN_LIMIT, DEFAULT_SCAN_LIMIT as u64, 100_000) as usize,
        consistent_read,
    }
}

/// DynamoDB client: one SDK client and a current table.
pub struct DynamoClient {
    connection_info: ConnectionInfo,
    shared: Arc<Shared>,
    metadata_provider: DynamoMetadataProvider,
}

impl DynamoClient {
    /// Build the SDK client (credentials, region, endpoint) and check that the
    /// table — or, without one, the account — answers.
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let settings = AwsSettings::from_connection_info(&connection_info)?;
        let mut sdk = dbcrust_aws::load_sdk_config(&settings).await?;
        if sdk.region().is_none() {
            if settings.endpoint_url.is_none() {
                return Err(DatabaseError::InvalidUrl(
                    "no AWS region: use dynamodb://eu-west-1, add ?region=eu-west-1, set \
                     AWS_REGION, or use a profile that has one"
                        .to_string(),
                ));
            }
            sdk = sdk
                .into_builder()
                .region(Region::new(FALLBACK_REGION))
                .build();
        }
        let client = Client::from_conf(aws_sdk_dynamodb::config::Builder::from(&sdk).build());

        let table = connection_info.database.clone();
        let shared = Arc::new(Shared {
            client,
            table: Mutex::new(table.clone()),
            infos: Mutex::default(),
            limits: limits_from(&connection_info),
            secrets: settings.secrets().iter().map(|s| s.to_string()).collect(),
            region: sdk.region().map(|r| r.as_ref().to_string()),
            endpoint: settings.endpoint_url.clone(),
            credentials: settings.credential_source().to_string(),
        });
        shared.preflight(table.as_deref()).await?;

        let metadata_provider = DynamoMetadataProvider::new(shared.clone());
        Ok(Self {
            connection_info,
            shared,
            metadata_provider,
        })
    }

    fn unsupported(&self, feature: impl Into<String>) -> DatabaseError {
        DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::DynamoDB,
            feature: feature.into(),
        }
    }

    /// Run one line (a command or a PartiQL `SELECT`); the result says whether
    /// a bound cut it short.
    pub async fn execute_limited(&self, line: &str) -> Result<LimitedResult, DatabaseError> {
        let input = dynamodb_command::parse(line)
            .map_err(|error| DatabaseError::QueryError(error.to_string()))?;
        let out = self.shared.execute(input).await?;
        for warning in &out.warnings {
            tracing::warn!("DynamoDB: {warning}");
        }
        for notice in &out.notices {
            tracing::info!("DynamoDB: {notice}");
        }
        Ok(out)
    }
}

#[async_trait]
impl DatabaseClient for DynamoClient {
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

    /// Validates the line; nothing is requested.
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        dynamodb_command::parse(sql)
            .map(|_| ())
            .map_err(|error| DatabaseError::QueryError(error.to_string()))
    }

    async fn explain_query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(self.unsupported(
            "EXPLAIN (PartiQL has no query plan; `describe` shows the keys and indexes)",
        ))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    /// The account's tables (`\l`).
    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let (names, _) = self
            .shared
            .list_tables(self.shared.limits.scan_limit)
            .await?;
        let mut rows = vec![vec!["Table".to_string()]];
        rows.extend(names.into_iter().map(|name| vec![name]));
        Ok(rows)
    }

    /// Switch to another table (`\c TABLE`), checking that it answers.
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        let table = database.trim().trim_end_matches('/');
        if table.is_empty()
            || table
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(DatabaseError::InvalidUrl(format!(
                "'{database}' is not a table name (\\c TABLE)"
            )));
        }
        self.shared.preflight(Some(table)).await?;
        self.shared.set_table(Some(table.to_string()));
        self.connection_info.database = Some(table.to_string());
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.shared.table().unwrap_or_default()
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                self.shared.preflight(self.shared.table().as_deref())
            )
            .await,
            Ok(Ok(()))
        )
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // Nothing is held open between requests
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let kind = match &self.shared.endpoint {
            Some(endpoint) => format!("DynamoDB-compatible service at {endpoint}"),
            None => "Amazon DynamoDB".to_string(),
        };
        // The "version" of the banner line is the region the requests are signed for
        let mut info = ServerInfo::new(kind, self.shared.region.clone().unwrap_or_default());
        info.supports_transactions = false;
        info.supports_roles = false;
        if let Some(region) = &self.shared.region {
            info.additional_info
                .insert("region".to_string(), region.clone());
        }
        info.additional_info
            .insert("credentials".to_string(), self.shared.credentials.clone());
        if let Some(table) = self.shared.table() {
            info.additional_info.insert("table".to_string(), table);
        }
        info.additional_info.insert(
            "max_rows".to_string(),
            self.shared.limits.max_rows.to_string(),
        );
        Ok(info)
    }
}

#[cfg(test)]
mod tests;
