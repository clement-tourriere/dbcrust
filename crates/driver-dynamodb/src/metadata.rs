//! Browsing an account: `\l` and `\dt` list the tables, `\d TABLE` is a
//! DescribeTable (keys, indexes, billing, size). Nothing here scans: DynamoDB
//! has no schema beyond the key attributes, and sampling items costs capacity.

use crate::Shared;
use crate::ops::TableInfo;
use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::dynamodb_command::{PARTIQL_KEYWORDS, command_names};
use dbcrust_core::model::{ColumnInfo, IndexInfo, TableDetails};
use std::sync::Arc;

pub struct DynamoMetadataProvider {
    shared: Arc<Shared>,
}

fn column(name: &str, data_type: String) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type,
        collation: String::new(),
        nullable: true,
        default_value: None,
        enum_values: None,
    }
}

fn type_name(code: char) -> &'static str {
    match code {
        'N' => "number",
        'B' => "binary",
        _ => "string",
    }
}

impl DynamoMetadataProvider {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// The attributes DynamoDB knows about: the keys of the table and its
    /// indexes (every other attribute is free-form per item).
    fn key_attributes(info: &TableInfo) -> Vec<ColumnInfo> {
        let role = |name: &str| -> String {
            let mut roles: Vec<String> = Vec::new();
            if info.keys.partition == name {
                roles.push("partition key".to_string());
            }
            if info.keys.sort.as_deref() == Some(name) {
                roles.push("sort key".to_string());
            }
            for (index, schema) in &info.indexes {
                if schema.partition == name {
                    roles.push(format!("partition key of {index}"));
                }
                if schema.sort.as_deref() == Some(name) {
                    roles.push(format!("sort key of {index}"));
                }
            }
            roles.join(", ")
        };
        let mut names = info.keys.names();
        for schema in info.indexes.values() {
            for name in schema.names() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
            .iter()
            .map(|name| {
                let kind = type_name(info.types.get(name).copied().unwrap_or('S'));
                column(name, format!("{kind} ({})", role(name)))
            })
            .collect()
    }
}

#[async_trait]
impl MetadataProvider for DynamoMetadataProvider {
    /// The region the requests are signed for: DynamoDB has no schemas.
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        Ok(vec![
            self.shared
                .region
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        ])
    }

    /// The account's tables (bounded by `scan_limit`).
    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        match self.shared.list_tables(self.shared.limits.scan_limit).await {
            Ok((names, more)) => {
                if more {
                    tracing::debug!(
                        "DynamoDB table listing stopped at {} (raise `scan_limit`)",
                        self.shared.limits.scan_limit
                    );
                }
                Ok(names)
            }
            // Keys limited to one table still know that table
            Err(error) => match self.shared.table() {
                Some(table) => {
                    tracing::debug!("ListTables failed ({error}); showing the connected table");
                    Ok(vec![table])
                }
                None => Err(error),
            },
        }
    }

    /// The key attributes (and those of its indexes) of a table.
    async fn get_columns(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let info = self.shared.table_info(table).await?;
        Ok(Self::key_attributes(&info)
            .into_iter()
            .map(|column| column.name)
            .collect())
    }

    /// The command and PartiQL words, for completion.
    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let mut words: Vec<String> = command_names().into_iter().map(str::to_string).collect();
        words.extend(PARTIQL_KEYWORDS.iter().map(|word| word.to_string()));
        Ok(words)
    }

    async fn get_table_details(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let info = self.shared.table_info(table).await?;
        let mut indexes = vec![IndexInfo {
            name: "PRIMARY".to_string(),
            index_type: "key".to_string(),
            is_primary: true,
            is_unique: true,
            predicate: None,
            definition: info.keys.describe_for_metadata(),
            constraint_def: None,
        }];
        for (name, schema) in &info.indexes {
            indexes.push(IndexInfo {
                name: name.clone(),
                index_type: "secondary index".to_string(),
                is_primary: false,
                is_unique: false,
                predicate: None,
                definition: schema.describe_for_metadata(),
                constraint_def: None,
            });
        }
        Ok(TableDetails {
            name: table.to_string(),
            schema: self
                .shared
                .region
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            full_name: table.to_string(),
            columns: Self::key_attributes(&info),
            indexes,
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
        self.shared.table()
    }
}
