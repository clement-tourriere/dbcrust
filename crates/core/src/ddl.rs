//! Table listing and `CREATE TABLE` pseudo-DDL rendering, shared by the `\ddl`
//! command and the AI assistant's schema context.
//!
//! Neither needs the AI stack, so they live here: a build without the `ai`
//! feature still has `\ddl`.

use crate::database::DatabaseType;
use async_trait::async_trait;

/// The two catalog lookups [`collect_table_names`] needs; implemented by the
/// session type (`Database`) and by the AI assistant's session trait adapter.
///
/// `?Send` like the AI session: callers await these while holding the
/// session's `std::sync::Mutex` guard, so the futures never cross threads.
#[async_trait(?Send)]
pub trait TableCatalog {
    /// Schema names.
    async fn catalog_schemas(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>>;

    /// Table and view names, optionally restricted to one schema.
    async fn catalog_tables(
        &mut self,
        schema_filter: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>>;
}

/// Collect the table list for AI context and `\ddl`. PostgreSQL with non-public schemas gets
/// schema-qualified names (`schema.table`); public tables and every other backend
/// stay unqualified — and the per-schema round-trips are skipped entirely when
/// there is nothing to disambiguate. Shared by the `??` schema context, the
/// `???` seed, and the `\ddl` command so all describe non-public tables correctly.
pub async fn collect_table_names(
    db: &mut dyn TableCatalog,
    db_type: &DatabaseType,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if !matches!(db_type, DatabaseType::PostgreSQL) {
        return db.catalog_tables(None).await;
    }

    let schemas = db.catalog_schemas().await.unwrap_or_default();
    if !schemas.iter().any(|s| s != "public") {
        // Only the default schema — bare names, a single round-trip.
        return db.catalog_tables(None).await;
    }

    let mut entries = Vec::new();
    for schema in &schemas {
        let tables = db.catalog_tables(Some(schema)).await.unwrap_or_default();
        for table in tables {
            if schema == "public" {
                entries.push(table);
            } else {
                entries.push(format!("{schema}.{table}"));
            }
        }
    }
    Ok(entries)
}

/// `CREATE TABLE` pseudo-DDL for one table: columns, then indexes, foreign keys
/// and referencing tables as `--` comments.
pub fn format_table_ddl(details: &crate::model::TableDetails, _db_type: &DatabaseType) -> String {
    let mut ddl = String::new();

    let full_name = if details.schema.is_empty() || details.schema == "public" {
        details.name.clone()
    } else {
        format!("{}.{}", details.schema, details.name)
    };

    ddl.push_str(&format!("CREATE TABLE {} (\n", full_name));

    for (i, col) in details.columns.iter().enumerate() {
        let nullable = if col.nullable { "" } else { " NOT NULL" };
        let default = col
            .default_value
            .as_ref()
            .map(|d| format!(" DEFAULT {d}"))
            .unwrap_or_default();

        ddl.push_str(&format!(
            "  {} {}{}{}",
            col.name, col.data_type, nullable, default
        ));

        if i < details.columns.len() - 1 {
            ddl.push(',');
        }
        ddl.push('\n');
    }

    ddl.push_str(");\n");

    // Indexes
    for idx in &details.indexes {
        if idx.is_primary {
            ddl.push_str(&format!("-- PK: {}\n", idx.definition));
        } else if idx.is_unique {
            ddl.push_str(&format!("-- UNIQUE: {} ({})\n", idx.name, idx.definition));
        } else {
            ddl.push_str(&format!("-- INDEX: {} ({})\n", idx.name, idx.definition));
        }
    }

    // Foreign keys
    for fk in &details.foreign_keys {
        ddl.push_str(&format!("-- FK: {} → {}\n", fk.name, fk.definition));
    }

    // Referenced by
    for ref_by in &details.referenced_by {
        ddl.push_str(&format!("-- Referenced by: {}\n", ref_by.definition));
    }

    ddl
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ColumnInfo, TableDetails};

    /// The catalog futures never suspend, so polling once is enough: no
    /// executor dev-dependency for this crate.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => panic!("the test catalog never suspends"),
        }
    }

    #[derive(Default)]
    struct Catalog {
        schemas: Vec<&'static str>,
        calls: Vec<Option<String>>,
    }

    #[async_trait(?Send)]
    impl TableCatalog for Catalog {
        async fn catalog_schemas(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
            Ok(self.schemas.iter().map(|s| s.to_string()).collect())
        }

        async fn catalog_tables(
            &mut self,
            schema_filter: Option<&str>,
        ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
            self.calls.push(schema_filter.map(str::to_string));
            Ok(match schema_filter {
                Some("audit") => vec!["log".to_string()],
                _ => vec!["users".to_string()],
            })
        }
    }

    #[test]
    fn public_only_postgres_takes_one_round_trip() {
        let mut catalog = Catalog {
            schemas: vec!["public"],
            ..Default::default()
        };
        let tables =
            block_on(collect_table_names(&mut catalog, &DatabaseType::PostgreSQL)).unwrap();
        assert_eq!(tables, ["users"]);
        assert_eq!(catalog.calls, [None]);
    }

    #[test]
    fn non_public_schemas_are_qualified() {
        let mut catalog = Catalog {
            schemas: vec!["public", "audit"],
            ..Default::default()
        };
        let tables =
            block_on(collect_table_names(&mut catalog, &DatabaseType::PostgreSQL)).unwrap();
        assert_eq!(tables, ["users", "audit.log"]);
    }

    #[test]
    fn other_engines_never_list_schemas() {
        let mut catalog = Catalog {
            schemas: vec!["main", "extra"],
            ..Default::default()
        };
        let tables = block_on(collect_table_names(&mut catalog, &DatabaseType::SQLite)).unwrap();
        assert_eq!(tables, ["users"]);
        assert_eq!(catalog.calls, [None]);
    }

    #[test]
    fn ddl_lists_columns_and_qualifies_non_public_schemas() {
        let details = TableDetails {
            name: "log".to_string(),
            schema: "audit".to_string(),
            full_name: "audit.log".to_string(),
            columns: vec![ColumnInfo {
                name: "id".to_string(),
                data_type: "integer".to_string(),
                collation: String::new(),
                nullable: false,
                default_value: None,
                enum_values: None,
            }],
            indexes: Vec::new(),
            check_constraints: Vec::new(),
            foreign_keys: Vec::new(),
            referenced_by: Vec::new(),
            nested_field_details: Default::default(),
        };
        let ddl = format_table_ddl(&details, &DatabaseType::PostgreSQL);
        assert!(ddl.starts_with("CREATE TABLE audit.log (\n"), "{ddl}");
        assert!(ddl.contains("  id integer NOT NULL\n"), "{ddl}");
    }
}
