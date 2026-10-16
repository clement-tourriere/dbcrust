//! The CloudWatch Logs requests behind each command and Logs Insights query.
//!
//! An Insights query is started (`StartQuery`), polled (`GetQueryResults`, with
//! a backoff) and **stopped (`StopQuery`) on every path that leaves it
//! running**: Ctrl-C, the timeout, a scan budget, a failing poll. Billing is per
//! GB scanned, so a query that nobody waits for must not keep scanning. Every
//! other request is bounded before it is sent and raced against Ctrl-C and the
//! timeout too (dropping the SDK future aborts the HTTP request).

use crate::errors::{has_code, map_error};
use crate::results::{human_bytes, map_results, timestamp_ms};
use crate::{LimitedResult, Shared};
use aws_sdk_cloudwatchlogs::types::{OrderBy, QueryStatistics, QueryStatus, ResultField};
use dbcrust_core::cloudwatch_command::{
    CloudWatchCommand, CloudWatchInput, InsightsQuery, TimeEdge, TimeWindow, Verb,
};
use dbcrust_core::database::{DatabaseError, StructuredQueryResult, interrupt_flag};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often a running request re-checks the interrupt flag and the deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// The first wait between two `GetQueryResults`, and the longest.
const FIRST_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(2);
/// How long `StopQuery` gets, however the query ended.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// The most log groups one Insights query may name (the service quota).
pub(crate) const MAX_QUERY_GROUPS: usize = 50;
/// Items per DescribeLogGroups / DescribeLogStreams page (the maximum).
const LIST_PAGE: usize = 50;
/// Events per FilterLogEvents page (the maximum).
const EVENTS_PAGE: usize = 10_000;
const DEFAULT_TAIL_SINCE: u64 = 300;
const DEFAULT_TAIL_LIMIT: usize = 100;
const DEFAULT_STREAMS: usize = 20;

type Row = Vec<Option<String>>;

fn cell(value: impl Into<String>) -> Option<String> {
    Some(value.into())
}

fn result(columns: &[&str], rows: Vec<Row>) -> StructuredQueryResult {
    StructuredQueryResult {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        rows,
    }
}

pub(crate) fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

pub(crate) fn human_duration(secs: u64) -> String {
    match secs {
        s if s % 604_800 == 0 && s >= 604_800 => format!("{}w", s / 604_800),
        s if s % 86_400 == 0 && s >= 86_400 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 && s >= 3600 => format!("{}h", s / 3600),
        s if s % 60 == 0 && s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// The window of a query as `(start, end)` epoch seconds: the one given, else
/// the default. Refused when it ends before it starts or is wider than
/// `max_window`.
pub(crate) fn resolve_window(
    window: Option<TimeWindow>,
    default_secs: u64,
    max_secs: u64,
    now: i64,
) -> Result<(i64, i64), String> {
    let edge = |edge: TimeEdge| match edge {
        TimeEdge::Epoch(epoch) => epoch,
        TimeEdge::Now => now,
    };
    let (start, end) = match window.unwrap_or(TimeWindow::Since(default_secs)) {
        TimeWindow::Since(secs) => (now.saturating_sub(secs as i64), now),
        TimeWindow::Between(start, end) => (edge(start), edge(end)),
    };
    if end <= start {
        return Err("the window ends before it starts".to_string());
    }
    let width = (end - start) as u64;
    if width > max_secs {
        return Err(format!(
            "a window of {} is wider than max_window ({}): Logs Insights bills per GB scanned, so \
             narrow it or raise the max_window option",
            human_duration(width),
            human_duration(max_secs)
        ));
    }
    Ok((start, end))
}

/// What ended a poll loop early, and whether the query is still running.
struct PollFailure {
    error: DatabaseError,
    running: bool,
}

impl Shared {
    /// Run a request future until `deadline`, giving up on Ctrl-C or when the
    /// deadline passes. Dropping the future aborts the HTTP request.
    pub(crate) async fn guard_until<T>(
        &self,
        deadline: Option<Instant>,
        request: impl Future<Output = Result<T, DatabaseError>>,
    ) -> Result<T, DatabaseError> {
        let interrupt = interrupt_flag();
        tokio::pin!(request);
        loop {
            tokio::select! {
                answer = &mut request => return answer,
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if interrupt.load(Ordering::Relaxed) {
                        return Err(DatabaseError::QueryError("query interrupted".to_string()));
                    }
                    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        return Err(DatabaseError::QueryError(format!(
                            "request timed out after {}s",
                            self.limits.timeout_secs()
                        )));
                    }
                }
            }
        }
    }

    pub(crate) async fn guard<T>(
        &self,
        request: impl Future<Output = Result<T, DatabaseError>>,
    ) -> Result<T, DatabaseError> {
        self.guard_until(self.limits.deadline(), request).await
    }

    /// Run one parsed line.
    pub(crate) async fn execute(
        &self,
        input: CloudWatchInput,
    ) -> Result<LimitedResult, DatabaseError> {
        match input {
            CloudWatchInput::Query(query) => self.insights(query).await,
            CloudWatchInput::Command(command) => match command {
                CloudWatchCommand::Help => Ok(LimitedResult::new(help())),
                CloudWatchCommand::Groups { prefix, limit } => {
                    self.groups_result(prefix.as_deref(), limit).await
                }
                CloudWatchCommand::Streams { group, limit } => {
                    self.streams_result(group, limit).await
                }
                CloudWatchCommand::Tail {
                    group,
                    since,
                    filter,
                    limit,
                } => self.tail(group, since, filter, limit).await,
                CloudWatchCommand::Use { groups } => self.use_result(&groups).await,
            },
        }
    }

    /// Log groups under `prefix`, at most `limit`; with whether more exist.
    pub(crate) async fn describe_groups(
        &self,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<aws_sdk_cloudwatchlogs::types::LogGroup>, bool), DatabaseError> {
        let secrets = self.secrets();
        let mut groups = Vec::new();
        let mut token: Option<String> = None;
        let mut more = false;
        for page in 0..self.limits.max_pages {
            let mut request = self
                .client
                .describe_log_groups()
                .limit((limit.saturating_sub(groups.len())).clamp(1, LIST_PAGE) as i32);
            if let Some(prefix) = prefix.filter(|prefix| !prefix.is_empty()) {
                request = request.log_group_name_prefix(prefix);
            }
            if let Some(token) = token.take() {
                request = request.next_token(token);
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            groups.extend(output.log_groups.unwrap_or_default());
            token = output.next_token;
            if groups.len() >= limit {
                more = token.is_some() || groups.len() > limit;
                groups.truncate(limit);
                break;
            }
            if token.is_none() {
                break;
            }
            if page + 1 == self.limits.max_pages {
                more = true;
            }
        }
        Ok((groups, more))
    }

    /// Turn what the user typed (`/a`, `/b*`) into concrete log group names. A
    /// name stands for itself (nothing is requested); a name ending in `*` is
    /// a prefix, looked up. More than [`MAX_QUERY_GROUPS`] is refused.
    pub(crate) async fn resolve_groups(
        &self,
        specs: &[String],
    ) -> Result<Vec<String>, DatabaseError> {
        let mut names: Vec<String> = Vec::new();
        for spec in specs {
            match spec.strip_suffix('*') {
                Some(prefix) => {
                    let (found, more) = self
                        .describe_groups(Some(prefix), MAX_QUERY_GROUPS + 1)
                        .await?;
                    if found.is_empty() {
                        return Err(DatabaseError::QueryError(format!(
                            "no log group starts with '{prefix}' (`groups` lists them)"
                        )));
                    }
                    if more || found.len() > MAX_QUERY_GROUPS {
                        return Err(DatabaseError::QueryError(format!(
                            "more than {MAX_QUERY_GROUPS} log groups start with '{prefix}': one query \
                             may name at most {MAX_QUERY_GROUPS}, use a longer prefix"
                        )));
                    }
                    for group in found {
                        names.extend(group.log_group_name);
                    }
                }
                None => names.push(spec.clone()),
            }
        }
        let mut seen = std::collections::HashSet::new();
        names.retain(|name| seen.insert(name.clone()));
        if names.len() > MAX_QUERY_GROUPS {
            return Err(DatabaseError::QueryError(format!(
                "{} log groups selected: one query may name at most {MAX_QUERY_GROUPS}",
                names.len()
            )));
        }
        Ok(names)
    }

    /// The groups of the URL path: a group name, or a prefix selecting several.
    /// An exact match wins over longer names sharing the prefix.
    pub(crate) async fn resolve_url_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        let (found, more) = self
            .describe_groups(Some(prefix), MAX_QUERY_GROUPS + 1)
            .await?;
        let names: Vec<String> = found
            .into_iter()
            .filter_map(|group| group.log_group_name)
            .collect();
        if names.iter().any(|name| name == prefix) {
            return Ok(vec![prefix.to_string()]);
        }
        if names.is_empty() {
            return Err(DatabaseError::QueryError(format!(
                "no log group is named or starts with '{prefix}'"
            )));
        }
        if more || names.len() > MAX_QUERY_GROUPS {
            return Err(DatabaseError::QueryError(format!(
                "more than {MAX_QUERY_GROUPS} log groups start with '{prefix}': one query may name \
                 at most {MAX_QUERY_GROUPS}, use a longer prefix"
            )));
        }
        Ok(names)
    }

    fn require_selection(&self) -> Result<Vec<String>, DatabaseError> {
        let groups = self.selected();
        if groups.is_empty() {
            return Err(DatabaseError::QueryError(
                "no log group selected: connect to cloudwatch://REGION/GROUP, choose with `use \
                 /aws/lambda/orders` (or `--group` on the query), or list them with `groups`"
                    .to_string(),
            ));
        }
        Ok(groups)
    }

    /// The one group a `streams` / `tail` acts on.
    fn single_group(&self, group: Option<String>) -> Result<String, DatabaseError> {
        if let Some(group) = group {
            return Ok(group);
        }
        let mut groups = self.require_selection()?;
        if groups.len() == 1 {
            return Ok(groups.remove(0));
        }
        Err(DatabaseError::QueryError(format!(
            "{} log groups are selected: name the one to read",
            groups.len()
        )))
    }

    async fn groups_result(
        &self,
        prefix: Option<&str>,
        limit: Option<usize>,
    ) -> Result<LimitedResult, DatabaseError> {
        let wanted = limit
            .unwrap_or(self.limits.max_rows)
            .min(self.limits.max_rows);
        let (groups, more) = self.describe_groups(prefix, wanted).await?;
        let rows = groups
            .iter()
            .map(|group| {
                vec![
                    group.log_group_name.clone(),
                    group.retention_in_days.map_or_else(
                        || cell("never expires"),
                        |days| cell(format!("{days} days")),
                    ),
                    group.stored_bytes.map(|bytes| bytes.to_string()),
                    group
                        .log_group_class
                        .as_ref()
                        .map(|c| c.as_str().to_string()),
                    group.metric_filter_count.map(|count| count.to_string()),
                    group.creation_time.map(timestamp_ms),
                ]
            })
            .collect();
        let mut out = LimitedResult::new(result(
            &[
                "log_group",
                "retention",
                "stored_bytes",
                "class",
                "metric_filters",
                "created",
            ],
            rows,
        ));
        if more {
            out.truncated = true;
            out.warnings.push(format!(
                "only the first {wanted} log groups are shown (give a longer prefix, or raise the \
                 limit / max_rows / max_pages)"
            ));
        }
        Ok(out)
    }

    async fn streams_result(
        &self,
        group: Option<String>,
        limit: Option<usize>,
    ) -> Result<LimitedResult, DatabaseError> {
        let group = self.single_group(group)?;
        let wanted = limit.unwrap_or(DEFAULT_STREAMS).min(self.limits.max_rows);
        let (streams, more) = self.recent_streams(&group, wanted).await?;
        let rows = streams
            .iter()
            .map(|stream| {
                vec![
                    stream.log_stream_name.clone(),
                    stream.last_event_timestamp.map(timestamp_ms),
                    stream.first_event_timestamp.map(timestamp_ms),
                    stream.creation_time.map(timestamp_ms),
                ]
            })
            .collect();
        let mut out = LimitedResult::new(result(
            &["log_stream", "last_event", "first_event", "created"],
            rows,
        ));
        if more {
            out.truncated = true;
            out.warnings.push(format!(
                "only the {wanted} most recently active streams of {group} are shown (raise --limit)"
            ));
        }
        Ok(out)
    }

    /// The most recently active streams of `group`, at most `limit`.
    pub(crate) async fn recent_streams(
        &self,
        group: &str,
        limit: usize,
    ) -> Result<(Vec<aws_sdk_cloudwatchlogs::types::LogStream>, bool), DatabaseError> {
        let secrets = self.secrets();
        let mut streams = Vec::new();
        let mut token: Option<String> = None;
        let mut more = false;
        for page in 0..self.limits.max_pages {
            let mut request = self
                .client
                .describe_log_streams()
                .log_group_name(group)
                .order_by(OrderBy::LastEventTime)
                .descending(true)
                .limit(limit.saturating_sub(streams.len()).clamp(1, LIST_PAGE) as i32);
            if let Some(token) = token.take() {
                request = request.next_token(token);
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            streams.extend(output.log_streams.unwrap_or_default());
            token = output.next_token;
            if streams.len() >= limit {
                more = token.is_some() || streams.len() > limit;
                streams.truncate(limit);
                break;
            }
            if token.is_none() {
                break;
            }
            if page + 1 == self.limits.max_pages {
                more = true;
            }
        }
        Ok((streams, more))
    }

    async fn tail(
        &self,
        group: Option<String>,
        since: Option<u64>,
        filter: Option<String>,
        limit: Option<usize>,
    ) -> Result<LimitedResult, DatabaseError> {
        let secrets = self.secrets();
        let group = self.single_group(group)?;
        let since = since.unwrap_or(DEFAULT_TAIL_SINCE);
        if since > self.limits.max_window {
            return Err(DatabaseError::QueryError(format!(
                "--since {} is wider than max_window ({}): narrow it or raise the max_window option",
                human_duration(since),
                human_duration(self.limits.max_window)
            )));
        }
        let wanted = limit
            .unwrap_or(DEFAULT_TAIL_LIMIT)
            .min(self.limits.max_rows);
        let start_ms = (now_epoch() - since as i64) * 1000;
        let mut events: Vec<Row> = Vec::new();
        let mut token: Option<String> = None;
        let mut more = false;
        for page in 0..self.limits.max_pages {
            let mut request = self
                .client
                .filter_log_events()
                .log_group_name(&group)
                .start_time(start_ms)
                .limit(wanted.saturating_sub(events.len()).clamp(1, EVENTS_PAGE) as i32);
            if let Some(pattern) = &filter {
                request = request.filter_pattern(pattern);
            }
            if let Some(token) = token.take() {
                request = request.next_token(token);
            }
            let output = self
                .guard(async { request.send().await.map_err(|e| map_error(e, &secrets)) })
                .await?;
            for event in output.events.unwrap_or_default() {
                events.push(vec![
                    event.timestamp.map(timestamp_ms),
                    event.log_stream_name,
                    event.message.map(|m| m.trim_end_matches('\n').to_string()),
                ]);
            }
            token = output.next_token;
            if events.len() >= wanted {
                more = token.is_some() || events.len() > wanted;
                events.truncate(wanted);
                break;
            }
            if token.is_none() {
                break;
            }
            if page + 1 == self.limits.max_pages {
                more = true;
            }
        }
        let mut out = LimitedResult::new(result(&["timestamp", "log_stream", "message"], events));
        if more {
            out.truncated = true;
            out.warnings.push(format!(
                "stopped after {} events: the last {} of {group} hold more (narrow --since, add \
                 --filter, or raise --limit); tail shows the window oldest first",
                out.result.rows.len(),
                human_duration(since)
            ));
        }
        Ok(out)
    }

    async fn use_result(&self, specs: &[String]) -> Result<LimitedResult, DatabaseError> {
        if !specs.is_empty() {
            let groups = self.resolve_groups(specs).await?;
            self.set_selected(groups);
        }
        let rows = self
            .selected()
            .into_iter()
            .map(|group| vec![cell(group)])
            .collect();
        Ok(LimitedResult::new(result(&["log_group"], rows)))
    }

    /// `StopQuery`, best effort, however the query ended and whatever Ctrl-C
    /// says (the interrupt flag is exactly why we are here).
    async fn stop_query(&self, query_id: &str) -> bool {
        let request = self.client.stop_query().query_id(query_id);
        match tokio::time::timeout(STOP_TIMEOUT, request.send()).await {
            Ok(Ok(_)) => true,
            Ok(Err(error)) => {
                // already finished: nothing left to stop
                tracing::debug!(
                    "StopQuery {query_id}: {}",
                    crate::errors::describe(&error, &self.secrets())
                );
                has_code(
                    &error,
                    &["ResourceNotFoundException", "InvalidParameterException"],
                )
            }
            Err(_) => {
                tracing::warn!("StopQuery {query_id} did not answer in {STOP_TIMEOUT:?}");
                false
            }
        }
    }

    async fn insights(&self, query: InsightsQuery) -> Result<LimitedResult, DatabaseError> {
        let secrets = self.secrets();
        let groups = if query.groups.is_empty() {
            self.require_selection()?
        } else {
            self.resolve_groups(&query.groups).await?
        };
        let (start, end) = resolve_window(
            query.window,
            self.limits.default_window,
            self.limits.max_window,
            now_epoch(),
        )
        .map_err(DatabaseError::QueryError)?;

        let deadline = self.limits.deadline();
        let request = self
            .client
            .start_query()
            .set_log_group_names(Some(groups.clone()))
            .start_time(start)
            .end_time(end)
            .query_string(&query.query)
            .limit(self.limits.max_rows as i32);
        let started = self
            .guard_until(deadline, async {
                request.send().await.map_err(|e| map_error(e, &secrets))
            })
            .await?;
        let Some(query_id) = started.query_id else {
            return Err(DatabaseError::QueryError(
                "StartQuery returned no query id".to_string(),
            ));
        };

        match self.poll(&query_id, deadline).await {
            Ok((rows, statistics)) => {
                let mut out = LimitedResult::new(map_results(&rows));
                if let Some(statistics) = &statistics {
                    out.notices.push(format!(
                        "Logs Insights matched {} records, scanned {} records ({}) in {} log group{}; \
                         window {} ({} .. {})",
                        statistics.records_matched() as u64,
                        statistics.records_scanned() as u64,
                        human_bytes(statistics.bytes_scanned()),
                        groups.len(),
                        if groups.len() == 1 { "" } else { "s" },
                        human_duration((end - start) as u64),
                        timestamp_ms(start * 1000),
                        timestamp_ms(end * 1000),
                    ));
                }
                if rows.len() >= self.limits.max_rows {
                    out.truncated = true;
                    out.warnings.push(format!(
                        "the result reached its limit of {} rows and may be cut short: narrow the \
                         window or the filter, or raise max_rows (at most 10000)",
                        self.limits.max_rows
                    ));
                }
                Ok(out)
            }
            Err(failure) => {
                let mut error = failure.error;
                if failure.running {
                    let stopped = self.stop_query(&query_id).await;
                    tracing::debug!("StopQuery {query_id}: stopped={stopped}");
                    if let DatabaseError::QueryError(message) = &mut error {
                        if stopped {
                            message.push_str(" (the query was stopped on the server)");
                        } else {
                            message.push_str(&format!(
                                " (could not confirm that query {query_id} was stopped: it may \
                                 keep scanning until the service's own 60 minute limit)"
                            ));
                        }
                    }
                }
                Err(error)
            }
        }
    }

    /// Poll `GetQueryResults` until the query ends. `running` in the failure
    /// says whether the query may still be scanning (and so needs a StopQuery).
    async fn poll(
        &self,
        query_id: &str,
        deadline: Option<Instant>,
    ) -> Result<(Vec<Vec<ResultField>>, Option<QueryStatistics>), PollFailure> {
        let secrets = self.secrets();
        let mut delay = FIRST_BACKOFF;
        loop {
            let request = self.client.get_query_results().query_id(query_id);
            let output = self
                .guard_until(deadline, async {
                    request.send().await.map_err(|e| map_error(e, &secrets))
                })
                .await
                .map_err(|error| PollFailure {
                    error,
                    running: true,
                })?;
            let status = output.status.clone();
            let statistics = output.statistics.clone();
            match status {
                Some(QueryStatus::Complete) => {
                    return Ok((output.results.unwrap_or_default(), statistics));
                }
                Some(QueryStatus::Failed) => {
                    return Err(PollFailure {
                        error: DatabaseError::QueryError(
                            "the Logs Insights query failed (the service gives no reason: check \
                             the query, the groups and the window)"
                                .to_string(),
                        ),
                        running: false,
                    });
                }
                Some(QueryStatus::Cancelled) => {
                    return Err(PollFailure {
                        error: DatabaseError::QueryError(
                            "the Logs Insights query was cancelled".to_string(),
                        ),
                        running: false,
                    });
                }
                Some(QueryStatus::Timeout) => {
                    return Err(PollFailure {
                        error: DatabaseError::QueryError(
                            "the Logs Insights query timed out on the server".to_string(),
                        ),
                        running: false,
                    });
                }
                _ => {}
            }
            if let (Some(budget), Some(statistics)) = (self.limits.max_scan_bytes, &statistics) {
                if statistics.bytes_scanned() > budget {
                    return Err(PollFailure {
                        error: DatabaseError::QueryError(format!(
                            "stopped: the query had already scanned {} ({} records), over the \
                             max_scan_gb budget of {}; narrow the window or the groups, or raise \
                             max_scan_gb",
                            human_bytes(statistics.bytes_scanned()),
                            statistics.records_scanned() as u64,
                            human_bytes(budget)
                        )),
                        running: true,
                    });
                }
            }
            self.guard_until(deadline, async {
                tokio::time::sleep(delay).await;
                Ok(())
            })
            .await
            .map_err(|error| PollFailure {
                error,
                running: true,
            })?;
            delay = (delay * 3 / 2).min(MAX_BACKOFF);
        }
    }
}

fn help() -> StructuredQueryResult {
    let mut rows: Vec<Row> = dbcrust_core::cloudwatch_command::verbs()
        .map(|verb: Verb| vec![cell(verb.usage()), cell(verb.description())])
        .collect();
    rows.push(vec![
        cell("[since DURATION | between TIME and TIME] [--group G] fields … | filter … | stats …"),
        cell("a Logs Insights query; the window defaults to the last hour (the since option), is capped by max_window, and the query is stopped on Ctrl-C and on timeout"),
    ]);
    result(&["command", "description"], rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const NOW: i64 = 1_792_137_600;

    #[rstest]
    #[case(None, Ok((NOW - 3600, NOW)))]
    #[case(Some(TimeWindow::Since(300)), Ok((NOW - 300, NOW)))]
    #[case(Some(TimeWindow::Between(TimeEdge::Epoch(NOW - 7200), TimeEdge::Now)), Ok((NOW - 7200, NOW)))]
    #[case(Some(TimeWindow::Between(TimeEdge::Epoch(NOW - 7200), TimeEdge::Epoch(NOW - 3600))), Ok((NOW - 7200, NOW - 3600)))]
    fn windows_resolve(
        #[case] window: Option<TimeWindow>,
        #[case] expected: Result<(i64, i64), ()>,
    ) {
        let resolved = resolve_window(window, 3600, 86_400, NOW).map_err(|_| ());
        assert_eq!(resolved, expected);
    }

    #[test]
    fn windows_are_checked() {
        let error =
            resolve_window(Some(TimeWindow::Since(2 * 86_400)), 3600, 86_400, NOW).unwrap_err();
        assert!(
            error.contains("2d") && error.contains("max_window (1d)"),
            "{error}"
        );
        let error = resolve_window(
            Some(TimeWindow::Between(
                TimeEdge::Epoch(NOW),
                TimeEdge::Epoch(NOW - 5),
            )),
            3600,
            86_400,
            NOW,
        )
        .unwrap_err();
        assert!(error.contains("ends before"), "{error}");
        assert!(
            resolve_window(
                Some(TimeWindow::Between(TimeEdge::Now, TimeEdge::Now)),
                3600,
                86_400,
                NOW
            )
            .is_err()
        );
    }

    #[rstest]
    #[case(30, "30s")]
    #[case(90, "90s")]
    #[case(120, "2m")]
    #[case(3600, "1h")]
    #[case(5400, "90m")]
    #[case(86_400, "1d")]
    #[case(604_800, "1w")]
    fn durations_print_short(#[case] secs: u64, #[case] expected: &str) {
        assert_eq!(human_duration(secs), expected);
    }
}
