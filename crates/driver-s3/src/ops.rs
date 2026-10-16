//! The S3 requests behind each command. Every request is bounded before it is
//! sent (a `Range` on every GetObject, `max-keys` and a page budget on every
//! listing) and raced against Ctrl-C and the query timeout: dropping the SDK
//! future aborts the HTTP request.

use crate::errors::{has_code, map_error};
use crate::format::{
    Content, base64_rows, classify, hex_rows, human_bytes, parse_content_range, text_rows,
    timestamp,
};
use crate::{LimitedResult, Shared};
use dbcrust_core::database::{DatabaseError, StructuredQueryResult, interrupt_flag, query_timeout};
use dbcrust_core::s3_command::{ByteRange, S3Command};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// How often a running request re-checks the interrupt flag and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Keys per ListObjectsV2 page (the S3 maximum).
const PAGE_KEYS: usize = 1000;

type Row = Vec<Option<String>>;

fn cell(value: impl Into<String>) -> Option<String> {
    Some(value.into())
}

/// One listed entry: a common prefix ("directory") or an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Prefix(String),
    Object {
        key: String,
        size: Option<i64>,
        last_modified: Option<String>,
        storage_class: Option<String>,
        etag: Option<String>,
    },
}

impl Entry {
    pub(crate) fn name(&self) -> &str {
        match self {
            Entry::Prefix(prefix) => prefix,
            Entry::Object { key, .. } => key,
        }
    }

    fn row(&self) -> Row {
        match self {
            Entry::Prefix(prefix) => vec![cell(prefix), cell("dir"), None, None, None, None],
            Entry::Object {
                key,
                size,
                last_modified,
                storage_class,
                etag,
            } => vec![
                cell(key),
                cell("object"),
                size.map(|size| size.to_string()),
                last_modified.clone(),
                storage_class.clone(),
                etag.as_ref().map(|etag| etag.trim_matches('"').to_string()),
            ],
        }
    }
}

pub(crate) const LIST_COLUMNS: [&str; 6] = [
    "key",
    "type",
    "size",
    "last_modified",
    "storage_class",
    "etag",
];

/// A listing: the entries and whether the bounds cut it short.
pub(crate) struct Listing {
    pub(crate) entries: Vec<Entry>,
    pub(crate) more: bool,
    pub(crate) requests: usize,
}

fn result(columns: &[&str], rows: Vec<Row>) -> StructuredQueryResult {
    StructuredQueryResult {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows,
    }
}

/// `bytes=A-B` for a request, with the number of bytes it can return.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RangePlan {
    pub(crate) header: String,
    /// Upper bound of the bytes the answer may hold (the read stops there).
    pub(crate) limit: u64,
    /// Offset of the first requested byte, when known (None for a suffix range).
    pub(crate) start: Option<u64>,
}

/// Turn what the user asked into a bounded range, or explain why not.
pub(crate) fn plan_range(
    range: ByteRange,
    default_bytes: u64,
    max_bytes: u64,
) -> Result<RangePlan, String> {
    let over = |asked: u64| {
        format!(
            "{} requested but get is capped at {} per call (raise the max_get_bytes option, or read it in several ranges)",
            human_bytes(asked),
            human_bytes(max_bytes)
        )
    };
    match range {
        ByteRange::Default => {
            let n = default_bytes.min(max_bytes);
            Ok(RangePlan {
                header: format!("bytes=0-{}", n - 1),
                limit: n,
                start: Some(0),
            })
        }
        ByteRange::First(n) => {
            if n > max_bytes {
                return Err(over(n));
            }
            Ok(RangePlan {
                header: format!("bytes=0-{}", n - 1),
                limit: n,
                start: Some(0),
            })
        }
        ByteRange::From {
            start,
            end: Some(end),
        } => {
            let length = end - start + 1;
            if length > max_bytes {
                return Err(over(length));
            }
            Ok(RangePlan {
                header: format!("bytes={start}-{end}"),
                limit: length,
                start: Some(start),
            })
        }
        ByteRange::From { start, end: None } => {
            let end = start.saturating_add(max_bytes - 1);
            Ok(RangePlan {
                header: format!("bytes={start}-{end}"),
                limit: max_bytes,
                start: Some(start),
            })
        }
        ByteRange::Suffix(n) => {
            if n > max_bytes {
                return Err(over(n));
            }
            Ok(RangePlan {
                header: format!("bytes=-{n}"),
                limit: n,
                start: None,
            })
        }
    }
}

impl Shared {
    /// Run a request future, giving up on Ctrl-C or when the query timeout
    /// passes. Dropping the future aborts the HTTP request.
    pub(crate) async fn guard<T>(
        &self,
        request: impl Future<Output = Result<T, DatabaseError>>,
    ) -> Result<T, DatabaseError> {
        let interrupt = interrupt_flag();
        let started = Instant::now();
        tokio::pin!(request);
        loop {
            tokio::select! {
                answer = &mut request => return answer,
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if interrupt.load(Ordering::Relaxed) {
                        return Err(DatabaseError::QueryError("query interrupted".to_string()));
                    }
                    if let Some(limit) = query_timeout() {
                        if started.elapsed() >= limit {
                            return Err(DatabaseError::QueryError(format!(
                                "request timed out after {}s",
                                limit.as_secs()
                            )));
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn require_bucket(&self) -> Result<String, DatabaseError> {
        self.bucket().ok_or_else(|| {
            DatabaseError::QueryError(
                "no bucket selected: connect to s3://BUCKET, or switch with \\c BUCKET (`buckets` lists them)"
                    .to_string(),
            )
        })
    }

    /// Run one parsed command.
    pub(crate) async fn execute(&self, command: S3Command) -> Result<LimitedResult, DatabaseError> {
        match command {
            S3Command::Help => Ok(LimitedResult::new(help())),
            S3Command::Pwd => Ok(LimitedResult::new(result(
                &["location"],
                vec![vec![cell(self.location_url())]],
            ))),
            S3Command::Cd { prefix } => self.cd(prefix),
            S3Command::Buckets => self.buckets_result().await,
            S3Command::Ls {
                prefix,
                recursive,
                limit,
            } => self.ls(prefix, recursive, limit).await,
            S3Command::Head { key } => self.head(&key).await,
            S3Command::Get { key, range, base64 } => self.get(&key, range, base64).await,
        }
    }

    fn cd(&self, target: Option<String>) -> Result<LimitedResult, DatabaseError> {
        self.require_bucket()?;
        let current = self.prefix();
        let next = match target.as_deref() {
            None | Some("/") | Some("") => String::new(),
            Some("..") => parent_prefix(&current),
            Some(path) => normalize_prefix(path),
        };
        self.set_prefix(next);
        Ok(LimitedResult::new(result(
            &["location"],
            vec![vec![cell(self.location_url())]],
        )))
    }

    async fn ls(
        &self,
        prefix: Option<String>,
        recursive: bool,
        limit: Option<usize>,
    ) -> Result<LimitedResult, DatabaseError> {
        let bucket = self.require_bucket()?;
        let prefix = match prefix {
            Some(prefix) => prefix.trim_start_matches('/').to_string(),
            None => self.prefix(),
        };
        let row_limit = limit
            .unwrap_or(self.limits.max_rows)
            .min(self.limits.max_rows);
        let listing = self.list(&bucket, &prefix, recursive, row_limit).await?;
        let mut out = LimitedResult::new(result(
            &LIST_COLUMNS,
            listing.entries.iter().map(Entry::row).collect(),
        ));
        if listing.more {
            out.truncated = true;
            out.notices.push(format!(
                "listing stopped at {} entries after {} request(s); there is more under '{prefix}' \
                 (narrow the prefix, or raise the limit with `ls -n N` up to the max_rows option, now {})",
                listing.entries.len(),
                listing.requests,
                self.limits.max_rows
            ));
        }
        Ok(out)
    }

    /// ListObjectsV2 under `prefix`, `/`-delimited unless `recursive`, at most
    /// `row_limit` entries and `max_pages` requests.
    pub(crate) async fn list(
        &self,
        bucket: &str,
        prefix: &str,
        recursive: bool,
        row_limit: usize,
    ) -> Result<Listing, DatabaseError> {
        let secrets = self.secrets();
        let mut entries: Vec<Entry> = Vec::new();
        let mut token: Option<String> = None;
        let mut requests = 0;
        let mut more = false;
        loop {
            let remaining = row_limit.saturating_sub(entries.len());
            if remaining == 0 {
                more = true;
                break;
            }
            requests += 1;
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(bucket)
                .max_keys(remaining.min(PAGE_KEYS) as i32);
            if !prefix.is_empty() {
                request = request.prefix(prefix);
            }
            if !recursive {
                request = request.delimiter("/");
            }
            if let Some(token) = &token {
                request = request.continuation_token(token);
            }
            let page = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            for common in page.common_prefixes() {
                if let Some(p) = common.prefix() {
                    entries.push(Entry::Prefix(p.to_string()));
                }
            }
            for object in page.contents() {
                entries.push(Entry::Object {
                    key: object.key().unwrap_or_default().to_string(),
                    size: object.size(),
                    last_modified: object.last_modified().and_then(timestamp),
                    storage_class: object.storage_class().map(|c| c.as_str().to_string()),
                    etag: object.e_tag().map(str::to_string),
                });
            }
            let truncated = page.is_truncated() == Some(true);
            token = page.next_continuation_token().map(str::to_string);
            if !truncated || token.is_none() {
                break;
            }
            if requests >= self.limits.max_pages {
                more = true;
                break;
            }
        }
        // A budget hit exactly at the end of the data is not "more"
        entries.truncate(row_limit);
        Ok(Listing {
            entries,
            more,
            requests,
        })
    }

    /// Every bucket of the account, bounded by `max_rows`.
    pub(crate) async fn buckets(
        &self,
    ) -> Result<Vec<(String, Option<String>, Option<String>)>, DatabaseError> {
        let secrets = self.secrets();
        let mut buckets = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..self.limits.max_pages {
            let mut request = self
                .client
                .list_buckets()
                .max_buckets(self.limits.max_rows.min(10_000) as i32);
            if let Some(token) = &token {
                request = request.continuation_token(token);
            }
            let page = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            for bucket in page.buckets() {
                buckets.push((
                    bucket.name().unwrap_or_default().to_string(),
                    bucket.creation_date().and_then(timestamp),
                    bucket.bucket_region().map(str::to_string),
                ));
            }
            token = page.continuation_token().map(str::to_string);
            if token.is_none() || buckets.len() >= self.limits.max_rows {
                break;
            }
        }
        buckets.truncate(self.limits.max_rows);
        Ok(buckets)
    }

    async fn buckets_result(&self) -> Result<LimitedResult, DatabaseError> {
        let rows = self
            .buckets()
            .await?
            .into_iter()
            .map(|(name, created, region)| vec![cell(name), created, region])
            .collect();
        Ok(LimitedResult::new(result(
            &["bucket", "created", "region"],
            rows,
        )))
    }

    /// HeadObject as `(attribute, value)` pairs, present attributes only.
    pub(crate) async fn head_attributes(
        &self,
        key: &str,
    ) -> Result<Vec<(String, String)>, DatabaseError> {
        let bucket = self.require_bucket()?;
        let secrets = self.secrets();
        let request = self.client.head_object().bucket(&bucket).key(key);
        let head = self
            .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
            .await?;
        let mut attributes: Vec<(String, String)> = Vec::new();
        let mut add = |name: &str, value: Option<String>| {
            if let Some(value) = value.filter(|v| !v.is_empty()) {
                attributes.push((name.to_string(), value));
            }
        };
        add("bucket", Some(bucket.clone()));
        add("key", Some(key.to_string()));
        add("size", head.content_length().map(|n| n.to_string()));
        add("last_modified", head.last_modified().and_then(timestamp));
        add(
            "etag",
            head.e_tag().map(|e| e.trim_matches('"').to_string()),
        );
        add("content_type", head.content_type().map(str::to_string));
        add(
            "content_encoding",
            head.content_encoding().map(str::to_string),
        );
        add(
            "content_language",
            head.content_language().map(str::to_string),
        );
        add(
            "content_disposition",
            head.content_disposition().map(str::to_string),
        );
        add("cache_control", head.cache_control().map(str::to_string));
        add("expires", head.expires_string().map(str::to_string));
        add(
            "storage_class",
            head.storage_class().map(|c| c.as_str().to_string()),
        );
        add("version_id", head.version_id().map(str::to_string));
        add(
            "server_side_encryption",
            head.server_side_encryption()
                .map(|e| e.as_str().to_string()),
        );
        add("sse_kms_key_id", head.ssekms_key_id().map(str::to_string));
        add("restore", head.restore().map(str::to_string));
        add(
            "replication_status",
            head.replication_status().map(|s| s.as_str().to_string()),
        );
        add(
            "object_lock_mode",
            head.object_lock_mode().map(|m| m.as_str().to_string()),
        );
        add("accept_ranges", head.accept_ranges().map(str::to_string));
        if let Some(metadata) = head.metadata() {
            let mut pairs: Vec<_> = metadata.iter().collect();
            pairs.sort();
            for (name, value) in pairs {
                add(&format!("metadata.{name}"), Some(value.clone()));
            }
        }
        Ok(attributes)
    }

    async fn head(&self, key: &str) -> Result<LimitedResult, DatabaseError> {
        let rows = self
            .head_attributes(key)
            .await?
            .into_iter()
            .map(|(name, value)| vec![cell(name), cell(value)])
            .collect();
        Ok(LimitedResult::new(result(&["attribute", "value"], rows)))
    }

    async fn get(
        &self,
        key: &str,
        range: ByteRange,
        base64: bool,
    ) -> Result<LimitedResult, DatabaseError> {
        let bucket = self.require_bucket()?;
        let plan = plan_range(
            range,
            self.limits.default_get_bytes,
            self.limits.max_get_bytes,
        )
        .map_err(DatabaseError::QueryError)?;
        let secrets = self.secrets();
        let request = self
            .client
            .get_object()
            .bucket(&bucket)
            .key(key)
            .range(plan.header.clone());
        let limit = plan.limit as usize;

        let fetched = self
            .guard(async {
                let output = match request.send().await {
                    Ok(output) => output,
                    // A range over an empty object is "not satisfiable"
                    Err(error) if has_code(&error, &["InvalidRange"]) && plan.start == Some(0) => {
                        return Ok(None);
                    }
                    Err(error) => return Err(map_error(error, &secrets)),
                };
                let content_range = output.content_range().and_then(parse_content_range);
                let content_length = output.content_length();
                let mut body = output.body;
                let mut bytes: Vec<u8> = Vec::new();
                let mut cut_by_cap = false;
                loop {
                    let chunk = body.try_next().await.map_err(|error| {
                        DatabaseError::ConnectionError(dbcrust_aws::redact_error(&error, &secrets))
                    })?;
                    let Some(chunk) = chunk else { break };
                    let room = limit - bytes.len();
                    if chunk.len() > room {
                        bytes.extend_from_slice(&chunk[..room]);
                        cut_by_cap = true;
                        break;
                    }
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() == limit {
                        break;
                    }
                }
                Ok(Some((bytes, content_range, content_length, cut_by_cap)))
            })
            .await?;

        let Some((bytes, content_range, content_length, cut_by_cap)) = fetched else {
            let mut empty = LimitedResult::new(result(&["line", "text"], Vec::new()));
            empty
                .notices
                .push(format!("s3://{bucket}/{key} is empty (0 bytes)"));
            return Ok(empty);
        };

        // Where the bytes start, and the object's total size when known
        let (offset, total) = match content_range {
            Some((start, _, total)) => (start, total),
            None => (0, content_length.map(|n| n as u64)),
        };
        let mut notice = format!(
            "{} of {}",
            human_bytes(bytes.len() as u64),
            total.map_or_else(|| "an unknown size".to_string(), human_bytes)
        );
        notice.push_str(&format!(" shown from byte {offset}"));
        let more_remains =
            total.is_some_and(|total| offset + (bytes.len() as u64) < total) || cut_by_cap;

        let mut out = if base64 {
            LimitedResult::new(result(&["offset", "base64"], base64_rows(&bytes, offset)))
        } else {
            match classify(&bytes) {
                Content::Text(text) => {
                    let (rows, cut) = text_rows(&text, self.limits.max_rows);
                    let mut out = LimitedResult::new(result(&["line", "text"], rows));
                    if cut {
                        out.truncated = true;
                        out.notices.push(format!(
                            "only the first {} lines are shown (max_rows option)",
                            self.limits.max_rows
                        ));
                    }
                    out
                }
                Content::Binary => {
                    notice.push_str(" (binary: hex dump; --base64 for base64)");
                    LimitedResult::new(result(
                        &["offset", "hex", "ascii"],
                        hex_rows(&bytes, offset),
                    ))
                }
            }
        };
        if more_remains {
            out.truncated = true;
            notice.push_str(
                "; more remains: ask for the next part with bytes=START-END (get is capped per call)",
            );
        }
        out.notices.insert(0, notice);
        Ok(out)
    }
}

/// The parent of a `/`-terminated prefix (`a/b/` → `a/`, `a/` → ``).
pub(crate) fn parent_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(index) => format!("{}/", &trimmed[..index]),
        None => String::new(),
    }
}

/// A prefix as `cd` stores it: no leading `/`, one trailing `/` unless empty.
pub(crate) fn normalize_prefix(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

fn help() -> StructuredQueryResult {
    result(
        &["command", "usage", "description"],
        dbcrust_core::s3_command::verbs()
            .map(|verb| {
                vec![
                    cell(verb.name()),
                    cell(verb.usage()),
                    cell(verb.description()),
                ]
            })
            .collect(),
    )
}
