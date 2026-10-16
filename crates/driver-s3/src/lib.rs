//! Amazon S3 (and R2, MinIO and other S3-compatible stores) for DBCrust.
//!
//! An object store has no query language, so a "query" here is one of a few
//! typed commands (see [`dbcrust_core::s3_command`]): `ls`, `head`, `get`,
//! `buckets`, `cd`, `pwd`, `help`. They are read-only by construction: no
//! command writes, deletes, copies or presigns, so `--read-only` accepts all of
//! them and the IAM policy of the credentials stays the server-side limit.
//!
//! Costs are real money, so every request is bounded before it is sent: a
//! `Range` (and a hard byte cap) on every GetObject, `max-keys` and a page
//! budget on every listing, and every call is raced against Ctrl-C and the query
//! timeout (dropping the SDK future aborts the HTTP request).
//!
//! Credentials, region, endpoint and the rustls-on-ring HTTP client come from
//! `dbcrust-aws`.
//!
//! Options (connection URL query): the AWS ones of `dbcrust-aws` (`region`,
//! `profile`, `endpoint` …) plus `max_rows` (rows per result, default 1000),
//! `max_pages` (listing requests per command, default 100), `scan_limit`
//! (entries `\dt` and completion list, default 1000), `get_bytes` (default
//! bytes `get` shows, default 64 KiB) and `max_get_bytes` (hard cap per `get`,
//! default 1 MiB, at most 64 MiB).

mod errors;
mod format;
mod metadata;
mod ops;

use async_trait::async_trait;
use aws_sdk_s3::Client;
use dbcrust_aws::{AwsSettings, Region};
use dbcrust_core::database::{
    ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType, MetadataProvider, ServerInfo,
    StructuredQueryResult,
};
use dbcrust_core::s3_command;
use errors::{is_denied, map_error};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use metadata::S3MetadataProvider;

const OPT_MAX_ROWS: &str = "max_rows";
const OPT_MAX_PAGES: &str = "max_pages";
const OPT_SCAN_LIMIT: &str = "scan_limit";
const OPT_GET_BYTES: &str = "get_bytes";
const OPT_MAX_GET_BYTES: &str = "max_get_bytes";

const DEFAULT_MAX_ROWS: usize = 1000;
const MAX_MAX_ROWS: usize = 1_000_000;
const DEFAULT_MAX_PAGES: usize = 100;
const DEFAULT_SCAN_LIMIT: usize = 1000;
const DEFAULT_GET_BYTES: u64 = 64 * 1024;
const DEFAULT_MAX_GET_BYTES: u64 = 1024 * 1024;
const MAX_MAX_GET_BYTES: u64 = 64 * 1024 * 1024;
/// SigV4 needs a region even for endpoints that ignore it (MinIO, LocalStack).
const FALLBACK_REGION: &str = "us-east-1";

/// A command's table, with whether a bound cut it short and what to tell the user.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedResult {
    pub result: StructuredQueryResult,
    pub truncated: bool,
    pub notices: Vec<String>,
}

impl LimitedResult {
    pub(crate) fn new(result: StructuredQueryResult) -> Self {
        Self {
            result,
            truncated: false,
            notices: Vec::new(),
        }
    }
}

/// The bucket and prefix commands act on; `cd` and `\c` change them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Location {
    pub(crate) bucket: Option<String>,
    pub(crate) prefix: String,
}

pub(crate) struct Limits {
    pub(crate) max_rows: usize,
    pub(crate) max_pages: usize,
    pub(crate) scan_limit: usize,
    pub(crate) default_get_bytes: u64,
    pub(crate) max_get_bytes: u64,
}

/// What the client and its metadata provider share.
pub(crate) struct Shared {
    pub(crate) client: Client,
    location: Mutex<Location>,
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

    fn location(&self) -> Location {
        self.location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(crate) fn bucket(&self) -> Option<String> {
        self.location().bucket
    }

    pub(crate) fn prefix(&self) -> String {
        self.location().prefix
    }

    pub(crate) fn set_prefix(&self, prefix: String) {
        self.location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prefix = prefix;
    }

    fn set_location(&self, bucket: Option<String>, prefix: String) {
        *self.location.lock().unwrap_or_else(|e| e.into_inner()) = Location { bucket, prefix };
    }

    /// `s3://bucket/prefix` (the bucket may be absent).
    pub(crate) fn location_url(&self) -> String {
        let location = self.location();
        format!(
            "s3://{}{}",
            location.bucket.as_deref().unwrap_or(""),
            if location.prefix.is_empty() {
                String::new()
            } else {
                format!("/{}", location.prefix)
            }
        )
    }

    /// Prove the endpoint answers and the credentials work. A bucket is
    /// checked with HeadBucket; being denied it is not fatal (a policy that
    /// only grants GetObject on a prefix still lets `get` work), but an unknown
    /// bucket, bad credentials or a wrong region are.
    async fn preflight(&self, bucket: Option<&str>) -> Result<(), DatabaseError> {
        let secrets = self.secrets();
        match bucket {
            Some(bucket) => {
                let request = self.client.head_bucket().bucket(bucket);
                match self
                    .guard(async {
                        match request.send().await {
                            Ok(_) => Ok(()),
                            Err(error) if is_denied(&error) => {
                                tracing::debug!("HeadBucket denied for {bucket}; continuing");
                                Ok(())
                            }
                            Err(error) => Err(map_error(error, &secrets)),
                        }
                    })
                    .await
                {
                    Ok(()) => Ok(()),
                    Err(DatabaseError::QueryError(message)) => Err(DatabaseError::ConnectionError(
                        format!("cannot open s3://{bucket}: {message}"),
                    )),
                    Err(other) => Err(other),
                }
            }
            None => {
                let request = self.client.list_buckets().max_buckets(1);
                self.guard(async {
                    match request.send().await {
                        Ok(_) => Ok(()),
                        // Listing buckets is its own permission; keys that can
                        // only read objects connect fine without it
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
    let max_get_bytes = size_option(
        info,
        OPT_MAX_GET_BYTES,
        DEFAULT_MAX_GET_BYTES,
        MAX_MAX_GET_BYTES,
    );
    Limits {
        max_rows: size_option(
            info,
            OPT_MAX_ROWS,
            DEFAULT_MAX_ROWS as u64,
            MAX_MAX_ROWS as u64,
        ) as usize,
        max_pages: size_option(info, OPT_MAX_PAGES, DEFAULT_MAX_PAGES as u64, 10_000) as usize,
        scan_limit: size_option(info, OPT_SCAN_LIMIT, DEFAULT_SCAN_LIMIT as u64, 100_000) as usize,
        default_get_bytes: size_option(info, OPT_GET_BYTES, DEFAULT_GET_BYTES, max_get_bytes)
            .min(max_get_bytes),
        max_get_bytes,
    }
}

/// S3 client: one SDK client, a current bucket and prefix.
pub struct S3Client {
    connection_info: ConnectionInfo,
    shared: Arc<Shared>,
    metadata_provider: S3MetadataProvider,
}

impl S3Client {
    /// Build the SDK client (credentials, region, endpoint) and check that the
    /// bucket — or, without one, the account — answers.
    pub async fn new(connection_info: ConnectionInfo) -> Result<Self, DatabaseError> {
        let settings = AwsSettings::from_connection_info(&connection_info)?;
        let mut sdk = dbcrust_aws::load_sdk_config(&settings).await?;
        if sdk.region().is_none() {
            if settings.endpoint_url.is_none() {
                return Err(DatabaseError::InvalidUrl(
                    "no AWS region: add ?region=eu-west-1 to the URL, set AWS_REGION, or use a \
                     profile that has one"
                        .to_string(),
                ));
            }
            sdk = sdk
                .into_builder()
                .region(Region::new(FALLBACK_REGION))
                .build();
        }
        let mut config =
            aws_sdk_s3::config::Builder::from(&sdk).force_path_style(settings.force_path_style);
        if settings.anonymous {
            config = config.allow_no_auth();
        }
        let client = Client::from_conf(config.build());

        let bucket = connection_info.database.clone();
        let shared = Arc::new(Shared {
            client,
            location: Mutex::new(Location {
                bucket: bucket.clone(),
                prefix: connection_info.file_path.clone().unwrap_or_default(),
            }),
            limits: limits_from(&connection_info),
            secrets: settings.secrets().iter().map(|s| s.to_string()).collect(),
            region: sdk.region().map(|r| r.as_ref().to_string()),
            endpoint: settings.endpoint_url.clone(),
            credentials: settings.credential_source().to_string(),
        });
        shared.preflight(bucket.as_deref()).await?;

        let metadata_provider = S3MetadataProvider::new(shared.clone());
        Ok(Self {
            connection_info,
            shared,
            metadata_provider,
        })
    }

    fn unsupported(&self, feature: impl Into<String>) -> DatabaseError {
        DatabaseError::FeatureNotSupported {
            database_type: DatabaseType::S3,
            feature: feature.into(),
        }
    }

    /// Run one command line; the result says whether a bound cut it short.
    pub async fn execute_limited(&self, line: &str) -> Result<LimitedResult, DatabaseError> {
        let command = s3_command::parse(line)
            .map_err(|error| DatabaseError::QueryError(error.to_string()))?;
        let out = self.shared.execute(command).await?;
        // A cut-short result must not go unnoticed; the rest is for debugging
        for notice in &out.notices {
            if out.truncated {
                tracing::warn!("S3: {notice}");
            } else {
                tracing::debug!("S3: {notice}");
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl DatabaseClient for S3Client {
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

    /// Validates the command line; nothing is requested.
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError> {
        s3_command::parse(sql)
            .map(|_| ())
            .map_err(|error| DatabaseError::QueryError(error.to_string()))
    }

    async fn explain_query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(self.unsupported("EXPLAIN (an object store has no query planner)"))
    }

    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        self.explain_query(sql).await
    }

    /// The account's buckets (`\l`).
    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError> {
        let mut rows = vec![vec![
            "Bucket".to_string(),
            "Created".to_string(),
            "Region".to_string(),
        ]];
        for (name, created, region) in self.shared.buckets().await? {
            rows.push(vec![
                name,
                created.unwrap_or_default(),
                region.unwrap_or_default(),
            ]);
        }
        Ok(rows)
    }

    /// Switch to another bucket (`\c BUCKET`), checking that it answers.
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError> {
        let bucket = database
            .trim()
            .trim_start_matches("s3://")
            .trim_end_matches('/');
        if bucket.is_empty()
            || bucket
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(DatabaseError::InvalidUrl(format!(
                "'{database}' is not a bucket name (\\c BUCKET)"
            )));
        }
        self.shared.preflight(Some(bucket)).await?;
        self.shared
            .set_location(Some(bucket.to_string()), String::new());
        self.connection_info.database = Some(bucket.to_string());
        self.connection_info.file_path = None;
        Ok(())
    }

    fn get_current_database(&self) -> String {
        self.shared.bucket().unwrap_or_default()
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
                self.shared.preflight(self.shared.bucket().as_deref())
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
            Some(endpoint) => format!("S3-compatible store at {endpoint}"),
            None => "Amazon S3".to_string(),
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
            .insert("location".to_string(), self.shared.location_url());
        info.additional_info.insert(
            "get_cap".to_string(),
            format::human_bytes(self.shared.limits.max_get_bytes),
        );
        Ok(info)
    }
}

#[cfg(test)]
mod tests;
