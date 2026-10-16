//! Catalog browsing through the SQLite catalog, over Hrana.
//!
//! The server is SQLite (`sqlite_schema`, the `pragma_*` table-valued
//! functions), so the queries are the ones the SQLite driver runs, with their
//! parameters spliced in as escaped literals (Hrana statements here carry no
//! arguments). Independent queries travel in one pipeline: `\d` costs two round
//! trips, not one per index.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider, escape_sql_string, quote_sql_ident};
use dbcrust_core::model::{ColumnInfo, ForeignKeyInfo, IndexInfo, ReferencedByInfo, TableDetails};

use crate::hrana::{HranaClient, StreamRequest, StreamResponse, StreamResult};
use crate::{rows_of, statement_error};

/// SQLite's default (and only) schema name.
const DEFAULT_SCHEMA: &str = "main";

/// Cells of a result set: `None` is NULL.
pub(crate) type Rows = Vec<Vec<Option<String>>>;

/// A string literal (SQLite has no backslash escapes).
fn lit(value: &str) -> String {
    format!("'{}'", escape_sql_string(value))
}

pub(crate) fn schemas_sql() -> &'static str {
    "SELECT name FROM pragma_database_list ORDER BY seq"
}

pub(crate) fn tables_sql(schema: &str) -> String {
    // Internal bookkeeping tables of SQLite and of the replication layers
    format!(
        "SELECT name FROM {}.sqlite_schema WHERE type IN ('table', 'view') \
         AND substr(name, 1, 7) <> 'sqlite_' \
         AND name NOT IN ('libsql_wasm_func_table', '_litestream_seq', '_litestream_lock') \
         ORDER BY name",
        quote_sql_ident(schema)
    )
}

pub(crate) fn columns_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT name FROM pragma_table_info({}, {}) ORDER BY cid",
        lit(table),
        lit(schema)
    )
}

pub(crate) fn table_exists_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT name FROM {}.sqlite_schema WHERE type IN ('table', 'view') AND name = {}",
        quote_sql_ident(schema),
        lit(table)
    )
}

pub(crate) fn detail_columns_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info({}, {}) ORDER BY cid",
        lit(table),
        lit(schema)
    )
}

pub(crate) fn index_list_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT name, \"unique\", origin FROM pragma_index_list({}, {}) ORDER BY seq",
        lit(table),
        lit(schema)
    )
}

pub(crate) fn index_info_sql(schema: &str, index: &str) -> String {
    format!(
        "SELECT name FROM pragma_index_info({}, {}) ORDER BY seqno",
        lit(index),
        lit(schema)
    )
}

pub(crate) fn foreign_keys_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list({}, {}) ORDER BY id, seq",
        lit(table),
        lit(schema)
    )
}

pub(crate) fn referenced_by_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT m.name, p.\"from\", p.\"to\" FROM {}.sqlite_schema m \
         JOIN pragma_foreign_key_list(m.name, {}) p \
         WHERE m.type = 'table' AND p.\"table\" = {} ORDER BY m.name, p.id, p.seq",
        quote_sql_ident(schema),
        lit(schema),
        lit(table)
    )
}

/// libSQL metadata provider
pub struct LibSqlMetadataProvider {
    hrana: Arc<HranaClient>,
}

impl LibSqlMetadataProvider {
    pub(crate) fn new(hrana: Arc<HranaClient>) -> Self {
        Self { hrana }
    }

    /// The statements run as one stateless pipeline; one outcome per statement.
    async fn run_many(
        &self,
        statements: &[String],
    ) -> Result<Vec<Result<Rows, String>>, DatabaseError> {
        let mut requests: Vec<StreamRequest> = statements
            .iter()
            .map(|sql| StreamRequest::execute(sql))
            .collect();
        requests.push(StreamRequest::Close);
        let outcome = self.hrana.pipeline(None, None, &requests).await?;
        let mut results = outcome.results.into_iter();
        let mut answers = Vec::with_capacity(statements.len());
        for _ in statements {
            let answer = match results.next() {
                Some(StreamResult::Ok {
                    response: StreamResponse::Execute { result },
                }) => rows_of(&result)
                    .map(|table| table.rows)
                    .map_err(|error| error.to_string()),
                Some(StreamResult::Error { error }) => Err(statement_error(&error).to_string()),
                _ => Err("the server answered with the wrong kind of response".to_string()),
            };
            answers.push(answer);
        }
        Ok(answers)
    }

    async fn run_one(&self, sql: String) -> Result<Rows, DatabaseError> {
        match self.run_many(&[sql]).await?.pop() {
            Some(Ok(rows)) => Ok(rows),
            Some(Err(message)) => Err(DatabaseError::MetadataError(message)),
            None => Ok(Vec::new()),
        }
    }
}

fn text(row: &[Option<String>], index: usize) -> String {
    row.get(index).cloned().flatten().unwrap_or_default()
}

fn optional(row: &[Option<String>], index: usize) -> Option<String> {
    row.get(index).cloned().flatten()
}

#[async_trait]
impl MetadataProvider for LibSqlMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        let rows = self.run_one(schemas_sql().to_string()).await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let rows = self
            .run_one(tables_sql(schema.unwrap_or(DEFAULT_SCHEMA)))
            .await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let rows = self
            .run_one(columns_sql(schema.unwrap_or(DEFAULT_SCHEMA), table))
            .await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        // SQLite has no stored functions to discover: the common built-ins
        Ok([
            "abs",
            "avg",
            "coalesce",
            "count",
            "date",
            "datetime",
            "group_concat",
            "ifnull",
            "instr",
            "json_extract",
            "json_valid",
            "length",
            "lower",
            "ltrim",
            "max",
            "min",
            "nullif",
            "printf",
            "random",
            "replace",
            "round",
            "rtrim",
            "strftime",
            "substr",
            "sum",
            "time",
            "trim",
            "typeof",
            "upper",
        ]
        .iter()
        .map(|name| (*name).to_string())
        .collect())
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let schema_name = schema.unwrap_or(DEFAULT_SCHEMA);

        // Round trip 1: everything that needs only the table name
        let mut first = self
            .run_many(&[
                table_exists_sql(schema_name, table),
                detail_columns_sql(schema_name, table),
                index_list_sql(schema_name, table),
                foreign_keys_sql(schema_name, table),
                referenced_by_sql(schema_name, table),
            ])
            .await?
            .into_iter();
        let mut next = |what: &str| -> Result<Rows, DatabaseError> {
            first
                .next()
                .unwrap_or_else(|| Err("no answer".to_string()))
                .map_err(|message| DatabaseError::MetadataError(format!("{what}: {message}")))
        };
        if next("table lookup")?.is_empty() {
            return Err(DatabaseError::QueryError(format!(
                "Table '{table}' does not exist"
            )));
        }
        let column_rows = next("columns")?;
        let index_rows = next("indexes")?;
        let foreign_key_rows = next("foreign keys")?;
        // Best effort, like the SQLite driver: an error is an empty list
        let referenced_rows = next("references").unwrap_or_default();

        let mut columns = Vec::new();
        let mut primary_key: Vec<(i64, String)> = Vec::new();
        for row in &column_rows {
            let name = text(row, 0);
            let pk_position = text(row, 4).parse::<i64>().unwrap_or(0);
            if pk_position > 0 {
                primary_key.push((pk_position, name.clone()));
            }
            columns.push(ColumnInfo {
                name,
                data_type: text(row, 1),
                collation: String::new(),
                nullable: text(row, 2) == "0",
                default_value: optional(row, 3),
                enum_values: None,
            });
        }
        primary_key.sort();
        let primary_key: Vec<String> = primary_key.into_iter().map(|(_, name)| name).collect();
        // A single INTEGER PRIMARY KEY aliases the rowid: never NULL
        if let [only] = primary_key.as_slice()
            && let Some(column) = columns.iter_mut().find(|c| &c.name == only)
            && column.data_type.eq_ignore_ascii_case("integer")
        {
            column.nullable = false;
        }

        // Round trip 2: the columns of every index
        let index_names: Vec<String> = index_rows.iter().map(|row| text(row, 0)).collect();
        let index_columns: Vec<Vec<String>> = if index_names.is_empty() {
            Vec::new()
        } else {
            let statements: Vec<String> = index_names
                .iter()
                .map(|name| index_info_sql(schema_name, name))
                .collect();
            self.run_many(&statements)
                .await?
                .into_iter()
                .map(|answer| {
                    answer
                        .unwrap_or_default()
                        .iter()
                        .map(|row| text(row, 0))
                        .collect()
                })
                .collect()
        };

        let mut indexes = Vec::new();
        let mut primary_key_indexed = false;
        for (row, columns_in_index) in index_rows.iter().zip(&index_columns) {
            let is_primary = text(row, 2) == "pk";
            primary_key_indexed |= is_primary;
            indexes.push(IndexInfo {
                name: text(row, 0),
                definition: format!("({})", columns_in_index.join(", ")),
                index_type: "btree".to_string(),
                is_primary,
                is_unique: text(row, 1) == "1",
                predicate: None,
                constraint_def: None,
            });
        }
        if !primary_key.is_empty() && !primary_key_indexed {
            indexes.insert(
                0,
                IndexInfo {
                    name: format!("{table}_pkey"),
                    definition: format!("({})", primary_key.join(", ")),
                    index_type: "rowid".to_string(),
                    is_primary: true,
                    is_unique: true,
                    predicate: None,
                    constraint_def: Some(format!("PRIMARY KEY ({})", primary_key.join(", "))),
                },
            );
        }

        let foreign_keys = foreign_key_rows
            .iter()
            .map(|row| {
                let (from, to_table, to) = (text(row, 0), text(row, 1), text(row, 2));
                ForeignKeyInfo {
                    name: format!("fk_{table}_{from}"),
                    definition: format!("FOREIGN KEY ({from}) REFERENCES {to_table}({to})"),
                }
            })
            .collect();
        let referenced_by = referenced_rows
            .iter()
            .map(|row| {
                let (source, from, to) = (text(row, 0), text(row, 1), text(row, 2));
                ReferencedByInfo {
                    schema: schema_name.to_string(),
                    constraint_name: format!("fk_{source}_{from}"),
                    definition: format!("FOREIGN KEY ({from}) REFERENCES {table}({to})"),
                    table: source,
                }
            })
            .collect();

        Ok(TableDetails {
            name: table.to_string(),
            schema: schema_name.to_string(),
            full_name: format!("{schema_name}.{table}"),
            columns,
            indexes,
            check_constraints: Vec::new(),
            foreign_keys,
            referenced_by,
            nested_field_details: HashMap::new(),
        })
    }

    fn supports_explain(&self) -> bool {
        true // EXPLAIN QUERY PLAN
    }

    fn default_schema(&self) -> Option<String> {
        Some(DEFAULT_SCHEMA.to_string())
    }
}
