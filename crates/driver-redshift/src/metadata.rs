//! Catalog browsing from Redshift's own system views.
//!
//! `pg_catalog` on Redshift is PostgreSQL 8.0.2's: no `pg_get_serial_sequence`,
//! `pg_range`, `prokind` or `information_schema` coverage of Spectrum tables.
//! Browsing therefore goes through `SVV_TABLES` / `SVV_COLUMNS` (local and
//! external tables and views, late-binding views included), `SVV_TABLE_INFO`
//! (distribution style, size, row count) and `SVV_EXTERNAL_SCHEMAS`, and
//! enriches `\d` from `PG_TABLE_DEF` (encodings, sort keys) and `PG_CONSTRAINT`
//! (informational primary/unique/foreign keys) on a best-effort basis: those
//! two answer only for what the session can see (`PG_TABLE_DEF` lists the
//! schemas on the `search_path`), and a failure there never fails `\d`.
//!
//! Every statement is a simple query with its parameters spliced in as quoted
//! literals ([`lit`]): the extended protocol is not relied on anywhere, and
//! the system views are leader-node relations that dislike bind parameters.

use std::collections::HashMap;

use async_trait::async_trait;
use dbcrust_core::database::{DatabaseError, MetadataProvider};
use dbcrust_core::model::{
    CheckConstraintInfo, ColumnInfo, ForeignKeyInfo, IndexInfo, ReferencedByInfo, TableDetails,
};
use sqlx::{Executor, PgPool};
use tracing::debug;

use crate::conn::db_err;
use crate::value;

/// The schema unqualified names live in.
const DEFAULT_SCHEMA: &str = "public";

/// A string literal. Redshift (PostgreSQL 8.0.2) reads a backslash in `'…'`
/// as an escape, so it is doubled along with the quote: otherwise a name
/// ending in `\` would escape the closing quote.
pub(crate) fn lit(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

/// System schemas hidden from browsing unless named explicitly.
const SYSTEM_SCHEMA_FILTER: &str = "LEFT({col}, 3) <> 'pg_' \
     AND {col} NOT IN ('information_schema', 'catalog_history')";

fn system_schema_filter(column: &str) -> String {
    SYSTEM_SCHEMA_FILTER.replace("{col}", column)
}

pub(crate) fn schemas_sql() -> String {
    format!(
        "SELECT nspname AS schema_name FROM pg_namespace WHERE {} \
         UNION \
         SELECT schemaname FROM svv_external_schemas \
         ORDER BY 1",
        system_schema_filter("nspname")
    )
}

pub(crate) fn tables_sql(schema: Option<&str>) -> String {
    let filter = match schema {
        Some(schema) => format!("table_schema = {}", lit(schema)),
        None => system_schema_filter("table_schema"),
    };
    format!(
        "SELECT table_schema, table_name FROM svv_tables WHERE {filter} \
         ORDER BY table_schema, table_name"
    )
}

pub(crate) fn columns_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT column_name FROM svv_columns \
         WHERE table_schema = {} AND table_name = {} ORDER BY ordinal_position",
        lit(schema),
        lit(table)
    )
}

pub(crate) fn functions_sql(schema: Option<&str>) -> String {
    let filter = match schema {
        Some(schema) => format!("n.nspname = {}", lit(schema)),
        None => system_schema_filter("n.nspname"),
    };
    format!(
        "SELECT DISTINCT n.nspname, p.proname FROM pg_proc p \
         JOIN pg_namespace n ON n.oid = p.pronamespace WHERE {filter} \
         ORDER BY n.nspname, p.proname"
    )
}

pub(crate) fn detail_columns_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT column_name, data_type, character_maximum_length, numeric_precision, \
         numeric_scale, is_nullable, column_default FROM svv_columns \
         WHERE table_schema = {} AND table_name = {} ORDER BY ordinal_position",
        lit(schema),
        lit(table)
    )
}

pub(crate) fn table_def_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT \"column\", encoding, distkey, sortkey FROM pg_table_def \
         WHERE schemaname = {} AND tablename = {}",
        lit(schema),
        lit(table)
    )
}

pub(crate) fn table_info_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT diststyle, sortkey1, sortkey_num, size, tbl_rows FROM svv_table_info \
         WHERE \"schema\" = {} AND \"table\" = {}",
        lit(schema),
        lit(table)
    )
}

pub(crate) fn constraints_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT con.conname, con.contype, pg_get_constraintdef(con.oid) \
         FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = {} AND c.relname = {} ORDER BY con.conname",
        lit(schema),
        lit(table)
    )
}

pub(crate) fn referenced_by_sql(schema: &str, table: &str) -> String {
    format!(
        "SELECT n.nspname, c.relname, con.conname, pg_get_constraintdef(con.oid) \
         FROM pg_constraint con \
         JOIN pg_class c ON c.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_class rc ON rc.oid = con.confrelid \
         JOIN pg_namespace rn ON rn.oid = rc.relnamespace \
         WHERE con.contype = 'f' AND rn.nspname = {} AND rc.relname = {} \
         ORDER BY n.nspname, c.relname, con.conname",
        lit(schema),
        lit(table)
    )
}

/// `schema.table` packing of `get_tables`, quoting a part that holds a dot or
/// a quote so every consumer that re-splits it stays unambiguous.
fn pack_part(part: &str) -> String {
    if part.contains('.') || part.contains('"') {
        format!("\"{}\"", part.replace('"', "\"\""))
    } else {
        part.to_string()
    }
}

/// `character varying(256)`, `numeric(10,2)`: the type as written in DDL.
pub(crate) fn display_type(
    data_type: &str,
    char_length: Option<&str>,
    precision: Option<&str>,
    scale: Option<&str>,
) -> String {
    match (data_type, char_length, precision, scale) {
        (_, Some(length), _, _) if data_type.starts_with("character") && !length.is_empty() => {
            format!("{data_type}({length})")
        }
        ("numeric", _, Some(precision), Some(scale)) if !precision.is_empty() => {
            format!("numeric({precision},{scale})")
        }
        _ => data_type.to_string(),
    }
}

/// `col1, col2` from the `SORTKEY` columns in sort order (a negative position
/// is a descending key; interleaved keys keep the same order).
pub(crate) fn sort_key_columns(mut keyed: Vec<(i64, String)>) -> Vec<String> {
    keyed.retain(|(position, _)| *position != 0);
    keyed.sort_by_key(|(position, _)| position.abs());
    keyed.into_iter().map(|(_, column)| column).collect()
}

/// Redshift metadata provider
pub struct RedshiftMetadataProvider {
    pool: PgPool,
}

impl RedshiftMetadataProvider {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn query(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, DatabaseError> {
        let rows = Executor::fetch_all(&self.pool, sql).await.map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                use sqlx::Row;
                (0..row.len()).map(|i| value::cell(row, i)).collect()
            })
            .collect())
    }

    /// An enrichment query: what it cannot answer is logged and left out.
    async fn query_best_effort(&self, what: &str, sql: &str) -> Vec<Vec<Option<String>>> {
        match self.query(sql).await {
            Ok(rows) => rows,
            Err(error) => {
                debug!("[RedshiftMetadataProvider] {what} unavailable: {error}");
                Vec::new()
            }
        }
    }
}

fn text(row: &[Option<String>], index: usize) -> String {
    row.get(index).cloned().flatten().unwrap_or_default()
}

fn present(row: &[Option<String>], index: usize) -> Option<&str> {
    row.get(index)
        .and_then(|cell| cell.as_deref())
        .filter(|cell| !cell.is_empty())
}

#[async_trait]
impl MetadataProvider for RedshiftMetadataProvider {
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError> {
        let rows = self.query(&schemas_sql()).await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let rows = self.query(&tables_sql(schema)).await?;
        Ok(rows
            .iter()
            .map(|row| {
                let schema_name = text(row, 0);
                let table_name = text(row, 1);
                if schema.is_none() && schema_name != DEFAULT_SCHEMA {
                    format!("{}.{}", pack_part(&schema_name), pack_part(&table_name))
                } else {
                    pack_part(&table_name)
                }
            })
            .collect())
    }

    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError> {
        let rows = self
            .query(&columns_sql(schema.unwrap_or(DEFAULT_SCHEMA), table))
            .await?;
        Ok(rows.iter().map(|row| text(row, 0)).collect())
    }

    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError> {
        let rows = self.query(&functions_sql(schema)).await?;
        Ok(rows.iter().map(|row| text(row, 1)).collect())
    }

    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, DatabaseError> {
        let schema_name = schema.unwrap_or(DEFAULT_SCHEMA);

        let column_rows = self.query(&detail_columns_sql(schema_name, table)).await?;
        // Encodings and the sort key from PG_TABLE_DEF (search_path schemas only)
        let table_def = self
            .query_best_effort("PG_TABLE_DEF", &table_def_sql(schema_name, table))
            .await;
        let encodings: HashMap<String, String> = table_def
            .iter()
            .filter_map(|row| {
                let encoding = present(row, 1)?;
                (!encoding.eq_ignore_ascii_case("none")).then(|| (text(row, 0), encoding.into()))
            })
            .collect();

        let columns: Vec<ColumnInfo> = column_rows
            .iter()
            .map(|row| {
                let name = text(row, 0);
                let mut data_type = display_type(
                    &text(row, 1),
                    present(row, 2),
                    present(row, 3),
                    present(row, 4),
                );
                if let Some(encoding) = encodings.get(&name) {
                    data_type.push_str(&format!(" ENCODE {encoding}"));
                }
                ColumnInfo {
                    name,
                    data_type,
                    collation: String::new(),
                    nullable: !text(row, 5).eq_ignore_ascii_case("NO"),
                    default_value: present(row, 6).map(str::to_string),
                    enum_values: None,
                }
            })
            .collect();

        // No indexes on Redshift: the physical design is the distribution style
        // and the sort key, shown where `\d` shows indexes
        let mut indexes: Vec<IndexInfo> = Vec::new();
        let info = self
            .query_best_effort("SVV_TABLE_INFO", &table_info_sql(schema_name, table))
            .await;
        if let Some(row) = info.first() {
            if let Some(diststyle) = present(row, 0) {
                indexes.push(IndexInfo {
                    name: "DISTSTYLE".to_string(),
                    index_type: "distribution".to_string(),
                    is_primary: false,
                    is_unique: false,
                    predicate: None,
                    definition: diststyle.to_string(),
                    constraint_def: None,
                });
            }
        }
        let sort_columns = sort_key_columns(
            table_def
                .iter()
                .filter_map(|row| Some((present(row, 3)?.parse::<i64>().ok()?, text(row, 0))))
                .collect(),
        );
        let sort_definition = if !sort_columns.is_empty() {
            Some(sort_columns.join(", "))
        } else {
            info.first()
                .and_then(|row| present(row, 1))
                .filter(|first| !first.eq_ignore_ascii_case("auto"))
                .map(str::to_string)
        };
        if let Some(definition) = sort_definition {
            indexes.push(IndexInfo {
                name: "SORTKEY".to_string(),
                index_type: "sort key".to_string(),
                is_primary: false,
                is_unique: false,
                predicate: None,
                definition,
                constraint_def: None,
            });
        }

        // Informational constraints (Redshift does not enforce them)
        let mut foreign_keys = Vec::new();
        let constraints = self
            .query_best_effort("PG_CONSTRAINT", &constraints_sql(schema_name, table))
            .await;
        for row in &constraints {
            let name = text(row, 0);
            let definition = text(row, 2);
            match text(row, 1).as_str() {
                "p" | "u" => indexes.push(IndexInfo {
                    name,
                    index_type: "informational".to_string(),
                    is_primary: text(row, 1) == "p",
                    is_unique: true,
                    predicate: None,
                    definition: definition.clone(),
                    constraint_def: Some(definition),
                }),
                "f" => foreign_keys.push(ForeignKeyInfo { name, definition }),
                _ => {}
            }
        }

        let referenced_by = self
            .query_best_effort("PG_CONSTRAINT", &referenced_by_sql(schema_name, table))
            .await
            .iter()
            .map(|row| ReferencedByInfo {
                schema: text(row, 0),
                table: text(row, 1),
                constraint_name: text(row, 2),
                definition: text(row, 3),
            })
            .collect();

        Ok(TableDetails {
            name: table.to_string(),
            schema: schema_name.to_string(),
            full_name: format!("{schema_name}.{table}"),
            columns,
            indexes,
            check_constraints: Vec::<CheckConstraintInfo>::new(),
            foreign_keys,
            referenced_by,
            nested_field_details: HashMap::new(),
        })
    }

    fn supports_explain(&self) -> bool {
        true
    }

    fn default_schema(&self) -> Option<String> {
        Some(DEFAULT_SCHEMA.to_string())
    }
}
