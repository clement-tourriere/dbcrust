//! Browsing an account: `\l` and `\dt` list the log groups, `\d GROUP` shows a
//! group's retention, size, class, metric filters and its most recent streams.
//! Nothing here starts an Insights query (that would bill).

use crate::Shared;
use crate::results::timestamp_ms;
use async_trait::async_trait;
use dbcrust_core::cloudwatch_command::{INSIGHTS_COMMANDS, command_names};
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::model::{ColumnInfo, TableDetails};
use std::sync::Arc;

/// The fields every log event has in Logs Insights.
const DEFAULT_FIELDS: [&str; 5] = [
    "@timestamp",
    "@message",
    "@logStream",
    "@log",
    "@ingestionTime",
];
/// Streams `\d` shows.
const DETAIL_STREAMS: usize = 5;

pub struct CloudWatchMetadataProvider {
    shared: Arc<Shared>,
}

fn member(name: &str, value: impl Into<String>) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type: value.into(),
        collation: String::new(),
        nullable: false,
        default_value: None,
        enum_values: None,
    }
}

impl CloudWatchMetadataProvider {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }
}

#[async_trait]
impl MetadataProvider for CloudWatchMetadataProvider {
    /// The region the requests are signed for: Logs has no schemas.
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        Ok(vec![
            self.shared
                .region
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        ])
    }

    /// The account's log groups (bounded by `scan_limit`); when listing them is
    /// not allowed, the selected ones.
    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        match self
            .shared
            .describe_groups(None, self.shared.limits.scan_limit)
            .await
        {
            Ok((groups, more)) => {
                if more {
                    tracing::debug!(
                        "log group listing stopped at {} (raise `scan_limit`)",
                        self.shared.limits.scan_limit
                    );
                }
                Ok(groups
                    .into_iter()
                    .filter_map(|group| group.log_group_name)
                    .collect())
            }
            Err(error) => {
                let selected = self.shared.selected();
                if selected.is_empty() {
                    Err(error)
                } else {
                    tracing::debug!(
                        "DescribeLogGroups failed ({error}); showing the selected groups"
                    );
                    Ok(selected)
                }
            }
        }
    }

    /// The fields every event has (discovering a group's own fields would run a
    /// billable query).
    async fn get_columns(
        &self,
        _table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        Ok(DEFAULT_FIELDS
            .iter()
            .map(|field| field.to_string())
            .collect())
    }

    /// The commands of this client and the Insights commands, for completion.
    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let mut words: Vec<String> = command_names().into_iter().map(str::to_string).collect();
        words.extend(INSIGHTS_COMMANDS.iter().map(|word| word.to_string()));
        words.extend(["since", "between", "--group"].map(str::to_string));
        Ok(words)
    }

    async fn get_table_details(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let (groups, _) = self.shared.describe_groups(Some(table), 50).await?;
        let group = groups
            .into_iter()
            .find(|group| group.log_group_name.as_deref() == Some(table))
            .ok_or_else(|| {
                DatabaseError::MetadataError(format!(
                    "no log group is named '{table}' (`groups` lists them)"
                ))
            })?;
        let mut columns = vec![member(
            "retention",
            group.retention_in_days.map_or_else(
                || "never expires".to_string(),
                |days| format!("{days} days"),
            ),
        )];
        if let Some(bytes) = group.stored_bytes {
            columns.push(member("stored bytes", bytes.to_string()));
        }
        if let Some(class) = &group.log_group_class {
            columns.push(member("class", class.as_str()));
        }
        columns.push(member(
            "metric filters",
            group.metric_filter_count.unwrap_or(0).to_string(),
        ));
        if let Some(key) = &group.kms_key_id {
            columns.push(member("kms key", key.clone()));
        }
        if let Some(status) = &group.data_protection_status {
            columns.push(member("data protection", status.as_str()));
        }
        if let Some(protected) = group.deletion_protection_enabled {
            columns.push(member("deletion protection", protected.to_string()));
        }
        if let Some(created) = group.creation_time {
            columns.push(member("created", timestamp_ms(created)));
        }
        if let Some(arn) = &group.log_group_arn {
            columns.push(member("arn", arn.clone()));
        }
        // A few recent streams; not being allowed to list them is not an error
        match self.shared.recent_streams(table, DETAIL_STREAMS).await {
            Ok((streams, _)) => {
                for stream in streams {
                    columns.push(member(
                        &format!("stream {}", stream.log_stream_name.unwrap_or_default()),
                        stream.last_event_timestamp.map_or_else(
                            || "no events".to_string(),
                            |ms| format!("last event {}", timestamp_ms(ms)),
                        ),
                    ));
                }
            }
            Err(error) => tracing::debug!("DescribeLogStreams for {table}: {error}"),
        }
        Ok(TableDetails {
            name: table.to_string(),
            schema: self
                .shared
                .region
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            full_name: table.to_string(),
            columns,
            indexes: Vec::new(),
            check_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
            nested_field_details: Default::default(),
        })
    }

    fn supports_explain(&self) -> bool {
        false
    }

    fn default_schema(&self) -> Option<String> {
        self.shared.selected().into_iter().next()
    }
}
