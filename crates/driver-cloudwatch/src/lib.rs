//! Amazon CloudWatch Logs for DBCrust.
//!
//! A "query" is a **Logs Insights** query over a **time window**
//! (`fields @timestamp, @message | filter @message like /error/ | sort
//! @timestamp desc | limit 50`) or one of a few typed commands (see
//! [`dbcrust_core::cloudwatch_command`]): `groups`, `streams`, `tail`, `use`,
//! `help`. Nothing writes: no PutLogEvents, no group or stream management, no
//! retention, export or subscription call is reachable, so `--read-only`
//! accepts exactly what runs and the IAM policy of the credentials stays the
//! server-side limit.
//!
//! Logs Insights bills per GB scanned, so a query is bounded before it is sent
//! (a mandatory window, at most 50 groups, a result limit, an optional scan
//! budget) and **stopped (`StopQuery`) on Ctrl-C, on timeout, on a scan budget
//! and on any failing poll**; the statistics (records matched and scanned, bytes
//! scanned) are reported after each query.
//!
//! Credentials, region, endpoint and the rustls-on-ring HTTP client come from
//! `dbcrust-aws`.
//!
//! Options (connection URL query): the AWS ones of `dbcrust-aws` (`region`,
//! `profile`, `endpoint` …) plus `groups` (comma-separated names or `prefix*`),
//! `since` (default window, `1h`), `max_window` (widest window, `7d`),
//! `max_rows` (rows per result, default 1000, at most 10000), `max_pages`
//! (requests per listing, default 20), `scan_limit` (groups `\dt` lists,
//! default 1000), `max_scan_gb` (stop a query that scanned more) and
//! `query_timeout` (seconds, 0 = the service's 60 minutes; default: the
//! configured query timeout).

mod errors;
mod metadata;
mod ops;
mod results;

use async_trait::async_trait;
use aws_sdk_cloudwatchlogs::Client;
use dbcrust_aws::{AwsSettings, Region};
use dbcrust_core::cloudwatch_command::{self, parse_duration_secs};
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider, ServerInfo,
    StructuredQueryResult, query_timeout,
};
use errors::{is_denied, map_error};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use metadata::CloudWatchMetadataProvider;

const OPT_GROUPS: &str = "groups";
const OPT_SINCE: &str = "since";
const OPT_MAX_WINDOW: &str = "max_window";
const OPT_MAX_ROWS: &str = "max_rows";
const OPT_MAX_PAGES: &str = "max_pages";
const OPT_SCAN_LIMIT: &str = "scan_limit";
const OPT_MAX_SCAN_GB: &str = "max_scan_gb";
const OPT_QUERY_TIMEOUT: &str = "query_timeout";

const DEFAULT_MAX_ROWS: usize = 1000;
/// `limit` of `StartQuery` is at most 10000.
const MAX_MAX_ROWS: usize = 10_000;
const DEFAULT_MAX_PAGES: usize = 20;
const DEFAULT_SCAN_LIMIT: usize = 1000;
const DEFAULT_WINDOW_SECS: u64 = 3600;
const DEFAULT_MAX_WINDOW_SECS: u64 = 7 * 86_400;
/// The service cancels an Insights query after an hour anyway.
const SERVER_CAP_SECS: u64 = 3600;
/// SigV4 needs a region even for endpoints that ignore it (emulators).
const FALLBACK_REGION: &str = "us-east-1";

/// A command's table, with whether a bound cut it short and what to tell the user.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedResult {
    pub result: StructuredQueryResult,
    pub truncated: bool,
    /// Things the user should see: a result cut short.
    pub warnings: Vec<String>,
    /// Facts about the call: the Insights statistics.
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
    pub(crate) default_window: u64,
    pub(crate) max_window: u64,
    /// Bytes an Insights query may scan before it is stopped.
    pub(crate) max_scan_bytes: Option<f64>,
    /// `Some(0)`: no client-side deadline (the service's own cap applies).
    pub(crate) timeout_option: Option<u64>,
}

impl Limits {
    /// Seconds a request may take: the option, else the configured query
    /// timeout, else the service's cap.
    pub(crate) fn timeout_secs(&self) -> u64 {
        match self.timeout_option {
            Some(0) => SERVER_CAP_SECS,
            Some(seconds) => seconds,
            None => query_timeout().map_or(SERVER_CAP_SECS, |t| t.as_secs()),
        }
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        let unlimited = self.timeout_option == Some(0)
            || (self.timeout_option.is_none() && query_timeout().is_none());
        Some(
            Instant::now()
                + Duration::from_secs(if unlimited {
                    SERVER_CAP_SECS
                } else {
                    self.timeout_secs()
                }),
        )
    }
}

/// What the client and its metadata provider share.
pub(crate) struct Shared {
    pub(crate) client: Client,
    selected: Mutex<Vec<String>>,
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

    pub(crate) fn selected(&self) -> Vec<String> {
        self.selected
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn set_selected(&self, groups: Vec<String>) {
        *self.selected.lock().unwrap_or_else(|e| e.into_inner()) = groups;
    }

    /// Prove the endpoint answers and the credentials work. Being denied is not
    /// fatal (a policy that only grants StartQuery on one group still lets
    /// queries work), but bad credentials or an unreachable endpoint are.
    async fn preflight(&self) -> Result<(), DatabaseError> {
        let secrets = self.secrets();
        let request = self.client.describe_log_groups().limit(1);
        self.guard(async {
            match request.send().await {
                Ok(_) => Ok(()),
                Err(error) if is_denied(&error) => Ok(()),
                Err(error) => Err(map_error(error, &secrets)),
            }
        })
        .await
    }
}

fn option<'a>(info: &'a ConnectionInfo, key: &str) -> Option<&'a str> {
    info.options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn size_option(info: &ConnectionInfo, key: &str, default: u64, max: u64) -> u64 {
    option(info, key)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map_or(default, |value| value.min(max))
}

fn duration_option(info: &ConnectionInfo, key: &str, default: u64) -> Result<u64, DatabaseError> {
    match option(info, key) {
        None => Ok(default),
        Some(value) => parse_duration_secs(value)
            .map_err(|message| DatabaseError::InvalidUrl(format!("{key}: {message}"))),
    }
}

fn limits_from(info: &ConnectionInfo) -> Result<Limits, DatabaseError> {
    let max_scan_bytes = match option(info, OPT_MAX_SCAN_GB) {
        None => None,
        Some(value) => match value.parse::<f64>() {
            Ok(gb) if gb.is_finite() && gb > 0.0 => Some(gb * 1024.0 * 1024.0 * 1024.0),
            _ => {
                return Err(DatabaseError::InvalidUrl(format!(
                    "max_scan_gb must be a positive number of GiB, got '{value}'"
                )));
            }
        },
    };
    let timeout_option = match option(info, OPT_QUERY_TIMEOUT) {
        None => None,
        Some(value) => Some(value.parse::<u64>().map_err(|_| {
            DatabaseError::InvalidUrl(format!(
                "query_timeout must be a number of seconds (0 = the service's 60 minutes), got '{value}'"
            ))
        })?),
    };
    let default_window = duration_option(info, OPT_SINCE, DEFAULT_WINDOW_SECS)?;
    let max_window = duration_option(info, OPT_MAX_WINDOW, DEFAULT_MAX_WINDOW_SECS)?;
    if default_window > max_window {
        return Err(DatabaseError::InvalidUrl(
            "since is wider than max_window".to_string(),
        ));
    }
    Ok(Limits {
        max_rows: size_option(
            info,
            OPT_MAX_ROWS,
            DEFAULT_MAX_ROWS as u64,
            MAX_MAX_ROWS as u64,
        ) as usize,
        max_pages: size_option(info, OPT_MAX_PAGES, DEFAULT_MAX_PAGES as u64, 1_000) as usize,
        scan_limit: size_option(info, OPT_SCAN_LIMIT, DEFAULT_SCAN_LIMIT as u64, 100_000) as usize,
        default_window,
        max_window,
        max_scan_bytes,
        timeout_option,
    })
}

/// CloudWatch Logs client: one SDK client and the log groups queries run on.
pub struct CloudWatchLogsClient {
    connection_info: ConnectionInfo,
    shared: Arc<Shared>,
    metadata_provider: CloudWatchMetadataProvider,
}

impl CloudWatchLogsClient {
    /// Build the SDK client (credentials, region, endpoint), check that the
    /// service answers and resolve the log groups the URL names.
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let settings = AwsSettings::from_connection_info(&connection_info)?;
        let limits = limits_from(&connection_info)?;
        let mut sdk = dbcrust_aws::load_sdk_config(&settings).await?;
        if sdk.region().is_none() {
            if settings.endpoint_url.is_none() {
                return Err(DatabaseError::InvalidUrl(
                    "no AWS region: use cloudwatch://eu-west-1, add ?region=eu-west-1, set \
                     AWS_REGION, or use a profile that has one"
                        .to_string(),
                ));
            }
            sdk = sdk
                .into_builder()
                .region(Region::new(FALLBACK_REGION))
                .build();
        }
        let client = Client::from_conf(aws_sdk_cloudwatchlogs::config::Builder::from(&sdk).build());

        let shared = Arc::new(Shared {
            client,
            selected: Mutex::default(),
            limits,
            secrets: settings.secrets().iter().map(|s| s.to_string()).collect(),
            region: sdk.region().map(|r| r.as_ref().to_string()),
            endpoint: settings.endpoint_url.clone(),
            credentials: settings.credential_source().to_string(),
        });
        shared.preflight().await?;

        // The groups the URL names: `?groups=a,b` (names, or `prefix*`) and the
        // path (a group, or a prefix selecting several)
        let mut groups: Vec<String> = Vec::new();
        if let Some(list) = option(&connection_info, OPT_GROUPS) {
            let specs: Vec<String> = list
                .split(',')
                .map(str::trim)
                .filter(|spec| !spec.is_empty())
                .map(str::to_string)
                .collect();
            groups = shared.resolve_groups(&specs).await.map_err(connect_error)?;
        }
        if let Some(prefix) = connection_info.database.as_deref() {
            let named = match shared.resolve_url_prefix(prefix).await {
                Ok(named) => named,
                // not allowed to list groups: take the path as the group itself
                Err(DatabaseError::QueryError(message)) if message.contains("AccessDenied") => {
                    vec![prefix.to_string()]
                }
                Err(error) => return Err(connect_error(error)),
            };
            for name in named {
                if !groups.contains(&name) {
                    groups.push(name);
                }
            }
        }
        if groups.len() > ops::MAX_QUERY_GROUPS {
            return Err(DatabaseError::InvalidUrl(format!(
                "{} log groups selected: one query may name at most {}",
                groups.len(),
                ops::MAX_QUERY_GROUPS
            )));
        }
        shared.set_selected(groups);

        let metadata_provider = CloudWatchMetadataProvider::new(shared.clone());
        Ok(Self {
            connection_info,
            shared,
            metadata_provider,
        })
    }

    fn unsupported(&self, feature: impl Into<String>) -> DatabaseError {
        DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::CloudWatchLogs,
            feature: feature.into(),
        }
    }

    /// Run one line (a command or a Logs Insights query); the result says
    /// whether a bound cut it short.
    pub async fn execute_limited(&self, line: &str) -> Result<LimitedResult, DatabaseError> {
        let input = cloudwatch_command::parse(line)
            .map_err(|error| DatabaseError::QueryError(error.to_string()))?;
        let out = self.shared.execute(input).await?;
        for warning in &out.warnings {
            tracing::warn!("CloudWatch Logs: {warning}");
        }
        for notice in &out.notices {
            tracing::info!("CloudWatch Logs: {notice}");
        }
        Ok(out)
    }
}

/// A group the URL names that cannot be resolved fails the connection.
fn connect_error(error: DatabaseError) -> DatabaseError {
    match error {
        DatabaseError::QueryError(message) => DatabaseError::ConnectionError(message),
        other => other,
    }
}

#[async_trait]
impl DatabaseClient for CloudWatchLogsClient {
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

    /// Validates the line; nothing is requested (a query would start billable work).
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        cloudwatch_command::parse(sql)
            .map(|_| ())
            .map_err(|error| DatabaseError::QueryError(error.to_string()))
    }

    async fn explain_query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(self.unsupported("EXPLAIN (Logs Insights has no query plan; the statistics after a query tell what it scanned)"))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    /// The account's log groups (`\l`).
    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let (groups, _) = self
            .shared
            .describe_groups(None, self.shared.limits.scan_limit)
            .await?;
        let mut rows = vec![vec!["Log group".to_string()]];
        rows.extend(
            groups
                .into_iter()
                .filter_map(|group| group.log_group_name)
                .map(|name| vec![name]),
        );
        Ok(rows)
    }

    /// Select other log groups (`\c /aws/lambda/orders`, `\c /aws/lambda/*`).
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        let spec = database.trim();
        if spec.is_empty()
            || spec
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        {
            return Err(DatabaseError::InvalidUrl(format!(
                "'{database}' is not a log group name (\\c /aws/lambda/orders, or a prefix ending in *)"
            )));
        }
        let groups = self
            .shared
            .resolve_groups(&[spec.to_string()])
            .await
            .map_err(connect_error)?;
        self.connection_info.database = Some(spec.trim_end_matches('*').to_string());
        self.shared.set_selected(groups);
        Ok(())
    }

    fn get_current_database(&self) -> String {
        let groups = self.shared.selected();
        match groups.as_slice() {
            [] => String::new(),
            [only] => only.clone(),
            [first, rest @ ..] => format!("{first} (+{})", rest.len()),
        }
    }

    fn get_connection_info(&self) -> &ConnectionInfo {
        &self.connection_info
    }

    fn get_metadata_provider(&self) -> &dyn MetadataProvider {
        &self.metadata_provider
    }

    async fn is_connected(&self) -> bool {
        matches!(
            tokio::time::timeout(Duration::from_secs(5), self.shared.preflight()).await,
            Ok(Ok(()))
        )
    }

    async fn close(&mut self) -> Result<(), DatabaseError> {
        // Nothing is held open between requests
        Ok(())
    }

    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError> {
        let kind = match &self.shared.endpoint {
            Some(endpoint) => format!("CloudWatch Logs-compatible service at {endpoint}"),
            None => "Amazon CloudWatch Logs".to_string(),
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
        info.additional_info
            .insert("log_groups".to_string(), self.shared.selected().join(", "));
        info.additional_info.insert(
            "default_window".to_string(),
            ops::human_duration(self.shared.limits.default_window),
        );
        info.additional_info.insert(
            "max_window".to_string(),
            ops::human_duration(self.shared.limits.max_window),
        );
        Ok(info)
    }
}

#[cfg(test)]
mod tests;
