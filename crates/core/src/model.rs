//! Plain metadata DTOs shared by the database drivers, the formatters, the AI
//! schema context and the frontends: table/column/index descriptions and the
//! helpers that split a user-typed table reference into schema and table.
//!
//! The root crate re-exports these from `db` so existing `db::TableDetails`
//! paths keep working.

use crate::database::DatabaseType;

pub fn unquote_metadata_identifier(identifier: &str) -> String {
    let trimmed = identifier.trim();
    if trimmed.len() >= 2 {
        if trimmed.starts_with('"') && trimmed.ends_with('"') {
            return trimmed[1..trimmed.len() - 1].replace("\"\"", "\"");
        }
        if trimmed.starts_with('`') && trimmed.ends_with('`') {
            return trimmed[1..trimmed.len() - 1].replace("``", "`");
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return trimmed[1..trimmed.len() - 1].replace("]]", "]");
        }
    }
    trimmed.to_string()
}

pub fn metadata_table_parts(
    table_name: &str,
    database_type: &DatabaseType,
) -> (Option<String>, String) {
    let supports_schemas = matches!(
        database_type,
        DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::ClickHouse
            | DatabaseType::SqlServer
            | DatabaseType::Redshift
            | DatabaseType::LibSql
            | DatabaseType::DuckDB
    );
    if !supports_schemas {
        return (None, unquote_metadata_identifier(table_name));
    }

    let mut quote = None;
    let mut chars = table_name.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        match quote {
            Some('"') if character == '"' => {
                if chars.peek().is_some_and(|(_, next)| *next == '"') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            Some('`') if character == '`' => {
                if chars.peek().is_some_and(|(_, next)| *next == '`') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            Some(']') if character == ']' => {
                // `]]` is an escaped bracket inside a [bracketed] identifier
                if chars.peek().is_some_and(|(_, next)| *next == ']') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            Some(_) => {}
            None if character == '"' || character == '`' => quote = Some(character),
            None if character == '[' => quote = Some(']'),
            None if character == '.' => {
                let schema = unquote_metadata_identifier(&table_name[..index]);
                let table = unquote_metadata_identifier(&table_name[index + 1..]);
                return (Some(schema), table);
            }
            None => {}
        }
    }

    (None, unquote_metadata_identifier(table_name))
}

/// Column filtering metadata to track when results are filtered
#[derive(Debug, Clone)]
pub struct ColumnFilteringInfo {
    pub total_columns: usize,
    pub displayed_columns: usize,
    pub filtered_column_names: Vec<String>,
}

impl ColumnFilteringInfo {
    pub fn new(
        total_columns: usize,
        displayed_columns: usize,
        filtered_column_names: Vec<String>,
    ) -> Self {
        Self {
            total_columns,
            displayed_columns,
            filtered_column_names,
        }
    }

    pub fn is_filtered(&self) -> bool {
        self.displayed_columns < self.total_columns
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TableDetails {
    pub name: String,
    pub schema: String,
    #[allow(dead_code)]
    pub full_name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub check_constraints: Vec<CheckConstraintInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub referenced_by: Vec<ReferencedByInfo>,
    /// Nested field details for struct/complex types (column_name -> field descriptions)
    pub nested_field_details: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug, serde::Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub collation: String,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub enum_values: Option<Vec<String>>, // For enum types, contains the possible values
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, serde::Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub index_type: String,
    pub is_primary: bool,
    pub is_unique: bool,
    pub predicate: Option<String>,
    pub definition: String,
    #[allow(dead_code)]
    pub constraint_def: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, serde::Serialize)]
pub struct CheckConstraintInfo {
    pub name: String,
    pub definition: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ReferencedByInfo {
    pub schema: String,
    pub table: String,
    pub constraint_name: String,
    pub definition: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(
        "analytics.orders",
        DatabaseType::PostgreSQL,
        Some("analytics"),
        "orders"
    )]
    #[case(
        "\"odd.schema\".\"Order.Items\"",
        DatabaseType::PostgreSQL,
        Some("odd.schema"),
        "Order.Items"
    )]
    #[case("\"v1.events\"", DatabaseType::PostgreSQL, None, "v1.events")]
    #[case("logs-2026.07", DatabaseType::Elasticsearch, None, "logs-2026.07")]
    #[case("events.archive", DatabaseType::MongoDB, None, "events.archive")]
    #[case(
        "[dbo].[Order Items]",
        DatabaseType::SqlServer,
        Some("dbo"),
        "Order Items"
    )]
    #[case("[a.b].[c]]d]", DatabaseType::SqlServer, Some("a.b"), "c]d")]
    #[case("[odd.name]", DatabaseType::SqlServer, None, "odd.name")]
    fn metadata_table_parts_respects_backend_and_identifier_quotes(
        #[case] input: &str,
        #[case] database_type: DatabaseType,
        #[case] schema: Option<&str>,
        #[case] table: &str,
    ) {
        assert_eq!(
            metadata_table_parts(input, &database_type),
            (schema.map(str::to_string), table.to_string())
        );
    }
}
