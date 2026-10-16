//! Catalog browsing for DuckDB: `\dt`, `\d`, `\l` and completion.
//!
//! Everything comes from DuckDB's own catalog functions (`duckdb_schemas()`,
//! `duckdb_tables()`, `duckdb_views()`, `duckdb_columns()`,
//! `duckdb_indexes()`, `duckdb_constraints()`, `duckdb_functions()`) scoped to
//! the *current* database (`current_database()`): attached databases and
//! DuckLake catalogs are reached with `\c name` (`USE name`). User-supplied
//! names only ever travel as bound parameters.

use crate::conn::Session;
use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::model::{
    CheckConstraintInfo, ColumnInfo, ForeignKeyInfo, IndexInfo, ReferencedByInfo, TableDetails,
};
use std::sync::Arc;

/// The schema a table reference lands in when none is given.
const DEFAULT_SCHEMA: &str = "main";

pub struct DuckDbMetadataProvider {
    session: Arc<Session>,
}

impl DuckDbMetadataProvider {
    pub(crate) fn new(session: Arc<Session>) -> Self {
        Self { session }
    }

    /// First column of every row, NULLs dropped.
    async fn names(&self, sql: &str, params: &[&str]) -> Result<Vec<String>, DatabaseError> {
        Ok(self
            .session
            .query_params(sql, params)
            .await?
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect())
    }

    /// `schema`, or the session's current schema when none was given.
    async fn schema_or_current(&self, schema: Option<&str>) -> Result<String, DatabaseError> {
        if let Some(schema) = schema {
            return Ok(schema.to_string());
        }
        let current = self.names("SELECT current_schema()", &[]).await?;
        Ok(current
            .into_iter()
            .next()
            .unwrap_or_else(|| DEFAULT_SCHEMA.to_string()))
    }

    async fn constraints(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<Vec<Vec<Option<String>>>, DatabaseError> {
        self.session
            .query_params(
                "SELECT constraint_type, constraint_text, constraint_column_names, \
                        referenced_table, referenced_column_names, constraint_name \
                 FROM duckdb_constraints() \
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ? \
                 ORDER BY constraint_index",
                &[schema, table],
            )
            .await
    }

    async fn referenced_by(&self, table: &str) -> Vec<ReferencedByInfo> {
        let rows = self
            .session
            .query_params(
                "SELECT schema_name, table_name, constraint_name, constraint_column_names, \
                        referenced_column_names \
                 FROM duckdb_constraints() \
                 WHERE database_name = current_database() \
                   AND constraint_type = 'FOREIGN KEY' AND referenced_table = ? \
                 ORDER BY schema_name, table_name, constraint_index",
                &[table],
            )
            .await
            .unwrap_or_default();
        rows.into_iter()
            .map(|row| {
                let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
                let columns = json_list(&cell(3)).join(", ");
                let referenced = json_list(&cell(4)).join(", ");
                ReferencedByInfo {
                    schema: cell(0),
                    table: cell(1),
                    constraint_name: cell(2),
                    definition: format!("FOREIGN KEY ({columns}) REFERENCES {table}({referenced})"),
                }
            })
            .collect()
    }
}

/// A `["a","b"]` cell (a DuckDB list rendered as JSON) as names.
pub(crate) fn json_list(text: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(text).unwrap_or_default()
}

/// A list a catalog function returns as *text* (`['"name"']`, as
/// `duckdb_indexes().expressions` does): its elements without brackets and
/// quotes. JSON lists work too.
pub(crate) fn list_text(text: &str) -> Vec<String> {
    let parsed = json_list(text);
    if !parsed.is_empty() {
        return parsed;
    }
    let Some(inner) = text
        .trim()
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Vec::new();
    };
    inner
        .split(',')
        .map(|part| part.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

#[async_trait]
impl MetadataProvider for DuckDbMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        self.names(
            "SELECT schema_name FROM duckdb_schemas() \
             WHERE database_name = current_database() AND (NOT internal OR schema_name = 'main') \
             ORDER BY schema_name",
            &[],
        )
        .await
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let schema = self.schema_or_current(schema).await?;
        self.names(
            "SELECT table_name FROM duckdb_tables() \
               WHERE database_name = current_database() AND schema_name = ? AND NOT internal \
             UNION ALL \
             SELECT view_name FROM duckdb_views() \
               WHERE database_name = current_database() AND schema_name = ? AND NOT internal \
             ORDER BY 1",
            &[&schema, &schema],
        )
        .await
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let schema = self.schema_or_current(schema).await?;
        self.names(
            "SELECT column_name FROM duckdb_columns() \
             WHERE database_name = current_database() AND schema_name = ? AND table_name = ? \
             ORDER BY column_index",
            &[&schema, table],
        )
        .await
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        self.names(
            "SELECT DISTINCT function_name FROM duckdb_functions() \
             WHERE regexp_matches(function_name, '^[a-z_][a-z0-9_]*$') \
             ORDER BY function_name",
            &[],
        )
        .await
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let schema = self.schema_or_current(schema).await?;

        let column_rows = self
            .session
            .query_params(
                "SELECT column_name, data_type, is_nullable, column_default \
                 FROM duckdb_columns() \
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ? \
                 ORDER BY column_index",
                &[&schema, table],
            )
            .await?;
        if column_rows.is_empty() {
            return Err(DatabaseError::QueryError(format!(
                "Table '{table}' does not exist"
            )));
        }
        let columns: Vec<ColumnInfo> = column_rows
            .into_iter()
            .map(|row| {
                let cell = |i: usize| row.get(i).cloned().flatten();
                ColumnInfo {
                    name: cell(0).unwrap_or_default(),
                    data_type: cell(1).unwrap_or_default(),
                    collation: String::new(),
                    nullable: cell(2).is_none_or(|nullable| nullable != "false"),
                    default_value: cell(3),
                    enum_values: None,
                }
            })
            .collect();

        // Explicit CREATE INDEX statements; keys and uniqueness come from the
        // constraints below (DuckDB backs them with ART indexes of their own)
        let index_rows = self
            .session
            .query_params(
                "SELECT index_name, is_unique, expressions FROM duckdb_indexes() \
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ? \
                 ORDER BY index_name",
                &[&schema, table],
            )
            .await
            .unwrap_or_default();
        let mut indexes: Vec<IndexInfo> = Vec::new();
        let mut check_constraints = Vec::new();
        let mut foreign_keys = Vec::new();

        for row in self.constraints(&schema, table).await.unwrap_or_default() {
            let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
            let kind = cell(0);
            let key_columns = json_list(&cell(2)).join(", ");
            match kind.as_str() {
                "PRIMARY KEY" => indexes.push(IndexInfo {
                    name: format!("{table}_pkey"),
                    definition: format!("({key_columns})"),
                    index_type: "art".to_string(),
                    is_primary: true,
                    is_unique: true,
                    predicate: None,
                    constraint_def: Some(format!("PRIMARY KEY ({key_columns})")),
                }),
                "UNIQUE" => indexes.push(IndexInfo {
                    name: {
                        let name = cell(5);
                        if name.is_empty() {
                            format!("{table}_{}_key", key_columns.replace(", ", "_"))
                        } else {
                            name
                        }
                    },
                    definition: format!("({key_columns})"),
                    index_type: "art".to_string(),
                    is_primary: false,
                    is_unique: true,
                    predicate: None,
                    constraint_def: Some(format!("UNIQUE ({key_columns})")),
                }),
                "CHECK" => check_constraints.push(CheckConstraintInfo {
                    name: {
                        let name = cell(5);
                        if name.is_empty() {
                            format!("{table}_check")
                        } else {
                            name
                        }
                    },
                    definition: cell(1),
                }),
                "FOREIGN KEY" => {
                    let referenced = json_list(&cell(4)).join(", ");
                    foreign_keys.push(ForeignKeyInfo {
                        name: {
                            let name = cell(5);
                            if name.is_empty() {
                                format!("fk_{table}_{}", key_columns.replace(", ", "_"))
                            } else {
                                name
                            }
                        },
                        definition: format!(
                            "FOREIGN KEY ({key_columns}) REFERENCES {}({referenced})",
                            cell(3)
                        ),
                    });
                }
                _ => {}
            }
        }
        for row in index_rows {
            let cell = |i: usize| row.get(i).cloned().flatten().unwrap_or_default();
            let expressions = cell(2);
            let definition = {
                let list = list_text(&expressions);
                if list.is_empty() {
                    expressions
                } else {
                    format!("({})", list.join(", "))
                }
            };
            indexes.push(IndexInfo {
                name: cell(0),
                definition,
                index_type: "art".to_string(),
                is_primary: false,
                is_unique: cell(1) == "true",
                predicate: None,
                constraint_def: None,
            });
        }

        Ok(TableDetails {
            name: table.to_string(),
            full_name: format!("{schema}.{table}"),
            schema,
            columns,
            indexes,
            check_constraints,
            foreign_keys,
            referenced_by: self.referenced_by(table).await,
            nested_field_details: std::collections::HashMap::new(),
        })
    }

    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        Some(DEFAULT_SCHEMA.to_string())
    }
}
