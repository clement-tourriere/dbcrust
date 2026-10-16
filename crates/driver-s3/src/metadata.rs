//! Browsing a bucket: `\l` lists buckets, `\dt` the prefixes and objects at the
//! current prefix (a `/`-delimited, bounded listing), `\d KEY` is a HeadObject.

use crate::Shared;
use crate::ops::{Entry, LIST_COLUMNS, normalize_prefix};
use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::model::{ColumnInfo, TableDetails};
use dbcrust_core::s3_command::command_names;
use std::collections::HashMap;
use std::sync::Arc;

pub struct S3MetadataProvider {
    shared: Arc<Shared>,
}

fn member(name: &str, value: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type: value.to_string(),
        collation: String::new(),
        nullable: false,
        default_value: None,
        enum_values: None,
    }
}

impl S3MetadataProvider {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// The bucket `schema` names, or the connected one.
    fn bucket_for(&self, schema: Option<&str>) -> Result<String, DatabaseError> {
        match schema.filter(|schema| !schema.is_empty()) {
            Some(bucket) => Ok(bucket.to_string()),
            None => self.shared.require_bucket(),
        }
    }
}

#[async_trait]
impl MetadataProvider for S3MetadataProvider {
    /// The account's buckets; when listing them is not allowed, just the
    /// connected one.
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        match self.shared.buckets().await {
            Ok(buckets) => Ok(buckets.into_iter().map(|(name, _, _)| name).collect()),
            Err(error) => match self.shared.bucket() {
                Some(bucket) => {
                    tracing::debug!("ListBuckets failed ({error}); showing the connected bucket");
                    Ok(vec![bucket])
                }
                None => Err(error),
            },
        }
    }

    /// The prefixes and objects at the current prefix (bounded by `scan_limit`),
    /// as full keys: `logs/` for a prefix, `logs/a.txt` for an object, so a name
    /// can be pasted into `ls`, `head` or `get`.
    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let bucket = self.bucket_for(schema)?;
        // The connected bucket (named or not) is listed at the current prefix,
        // another bucket at its root
        let prefix = if self.shared.bucket().as_deref() == Some(bucket.as_str()) {
            self.shared.prefix()
        } else {
            String::new()
        };
        let listing = self
            .shared
            .list(&bucket, &prefix, false, self.shared.limits.scan_limit)
            .await?;
        if listing.more {
            tracing::debug!(
                "S3 listing of s3://{bucket}/{prefix} stopped at {} entries (raise `scan_limit`)",
                self.shared.limits.scan_limit
            );
        }
        Ok(listing
            .entries
            .iter()
            .map(|e| e.name().to_string())
            .collect())
    }

    /// `dir` for a prefix, `object` for a key: no request needed.
    async fn get_table_kinds(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, String>, DatabaseError> {
        Ok(names
            .iter()
            .map(|name| {
                let kind = if name.ends_with('/') { "dir" } else { "object" };
                (name.clone(), kind.to_string())
            })
            .collect())
    }

    /// The attributes of an object (HeadObject), or the listing columns of a prefix.
    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        self.bucket_for(schema)?;
        if table.ends_with('/') || table.is_empty() {
            return Ok(LIST_COLUMNS.iter().map(|c| c.to_string()).collect());
        }
        Ok(self
            .shared
            .head_attributes(table)
            .await?
            .into_iter()
            .map(|(name, _)| name)
            .collect())
    }

    /// The command words, for completion.
    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        Ok(command_names().into_iter().map(str::to_string).collect())
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let bucket = self.bucket_for(schema)?;
        let columns = if table.ends_with('/') || table.is_empty() {
            // A prefix has no metadata of its own: say what it is and sample it
            let prefix = if table.is_empty() {
                self.shared.prefix()
            } else {
                normalize_prefix(table)
            };
            let listing = self.shared.list(&bucket, &prefix, false, 20).await?;
            let mut columns = vec![member("type", "prefix"), member("prefix", &prefix)];
            for entry in &listing.entries {
                let kind = match entry {
                    Entry::Prefix(_) => "dir",
                    Entry::Object { .. } => "object",
                };
                columns.push(member(entry.name(), kind));
            }
            if listing.more {
                columns.push(member("…", "more entries (use ls)"));
            }
            columns
        } else {
            self.shared
                .head_attributes(table)
                .await?
                .into_iter()
                .map(|(name, value)| member(&name, &value))
                .collect()
        };
        Ok(TableDetails {
            name: table.to_string(),
            schema: bucket.clone(),
            full_name: format!("{bucket}/{table}"),
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
        self.shared.bucket()
    }
}
