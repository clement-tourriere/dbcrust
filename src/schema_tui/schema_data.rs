//! Data structures and metadata loading for the schema TUI viewer

use crate::database::DatabaseType;
use crate::db::Database;

/// Top-level schema data for the TUI viewer
#[derive(Debug)]
pub struct SchemaData {
    pub database_name: String,
    pub database_type: DatabaseType,
    pub schemas: Vec<SchemaInfo>,
    pub relationships: Vec<Relationship>,
}

/// A database schema containing tables
#[derive(Debug)]
pub struct SchemaInfo {
    pub name: String,
    pub tables: Vec<TableSummary>,
}

/// Summary of a single table (details loaded lazily on selection)
#[derive(Debug)]
pub struct TableSummary {
    pub name: String,
    pub schema: String,
    pub outgoing_fk_count: usize,
    pub incoming_fk_count: usize,
}

/// A foreign key relationship between two tables
#[derive(Debug, Clone)]
pub struct Relationship {
    pub constraint_name: String,
    pub source_schema: String,
    pub source_table: String,
    pub source_columns: Vec<String>,
    pub target_schema: String,
    pub target_table: String,
    pub target_columns: Vec<String>,
}

/// Flattened entry for the table list panel
#[derive(Debug, Clone)]
pub enum TableListEntry {
    SchemaHeader {
        name: String,
    },
    Table {
        name: String,
        schema: String,
        outgoing_fk: usize,
        incoming_fk: usize,
    },
}

impl TableListEntry {
    pub fn is_table(&self) -> bool {
        matches!(self, TableListEntry::Table { .. })
    }

    pub fn table_key(&self) -> Option<(String, String)> {
        match self {
            TableListEntry::Table { name, schema, .. } => Some((schema.clone(), name.clone())),
            _ => None,
        }
    }
}

/// Load all schema data from the database (fast: only schema/table names + FK relationships)
pub async fn load_schema_data(database: &mut Database) -> Result<SchemaData, String> {
    let db_name = database.get_current_db();
    let db_type = database.get_database_type();

    eprintln!("Loading schema metadata...");

    // Load relationships first using execute_query (needs &mut self)
    let relationships = load_relationships(database, &db_type).await;

    // Now use the metadata provider (immutable borrow) for schema/table info
    let client = database
        .get_database_client()
        .ok_or_else(|| "No database client available".to_string())?;

    let metadata = client.get_metadata_provider();

    // Get all schemas
    let schema_names = metadata
        .get_schemas()
        .await
        .map_err(|e| format!("Failed to get schemas: {e}"))?;

    // Load table names for each schema (no per-table detail queries)
    let mut schemas = Vec::new();
    for schema_name in &schema_names {
        let tables = metadata
            .get_tables(Some(schema_name))
            .await
            .unwrap_or_default();

        if tables.is_empty() {
            continue;
        }

        let mut table_summaries: Vec<TableSummary> = tables
            .iter()
            .map(|table_name| {
                let outgoing_fk_count = relationships
                    .iter()
                    .filter(|r| r.source_schema == *schema_name && r.source_table == *table_name)
                    .count();
                let incoming_fk_count = relationships
                    .iter()
                    .filter(|r| r.target_schema == *schema_name && r.target_table == *table_name)
                    .count();

                TableSummary {
                    name: table_name.clone(),
                    schema: schema_name.clone(),
                    outgoing_fk_count,
                    incoming_fk_count,
                }
            })
            .collect();

        table_summaries.sort_by(|a, b| a.name.cmp(&b.name));

        schemas.push(SchemaInfo {
            name: schema_name.clone(),
            tables: table_summaries,
        });
    }

    schemas.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(SchemaData {
        database_name: db_name,
        database_type: db_type,
        schemas,
        relationships,
    })
}

/// Load FK relationships using bulk queries where possible
async fn load_relationships(database: &mut Database, db_type: &DatabaseType) -> Vec<Relationship> {
    match db_type {
        DatabaseType::PostgreSQL => load_postgresql_relationships(database).await,
        DatabaseType::MySQL => load_mysql_relationships(database).await,
        DatabaseType::SQLite => load_sqlite_relationships(database).await,
        _ => Vec::new(),
    }
}

async fn load_postgresql_relationships(database: &mut Database) -> Vec<Relationship> {
    let query = r#"
        SELECT
            n1.nspname AS source_schema,
            t1.relname AS source_table,
            array_agg(a1.attname ORDER BY pos.ord) AS source_columns,
            n2.nspname AS target_schema,
            t2.relname AS target_table,
            array_agg(a2.attname ORDER BY pos.ord) AS target_columns,
            c.conname AS constraint_name
        FROM pg_constraint c
        JOIN pg_class t1 ON c.conrelid = t1.oid
        JOIN pg_namespace n1 ON t1.relnamespace = n1.oid
        JOIN pg_class t2 ON c.confrelid = t2.oid
        JOIN pg_namespace n2 ON t2.relnamespace = n2.oid
        CROSS JOIN LATERAL unnest(c.conkey, c.confkey) WITH ORDINALITY AS pos(src, tgt, ord)
        JOIN pg_attribute a1 ON a1.attrelid = t1.oid AND a1.attnum = pos.src
        JOIN pg_attribute a2 ON a2.attrelid = t2.oid AND a2.attnum = pos.tgt
        WHERE c.contype = 'f'
          AND n1.nspname NOT LIKE 'pg_%'
          AND n1.nspname NOT IN ('information_schema', 'pg_toast')
        GROUP BY n1.nspname, t1.relname, n2.nspname, t2.relname, c.conname
        ORDER BY n1.nspname, t1.relname, c.conname
    "#;

    parse_relationship_results(database.execute_query(query).await.ok())
}

async fn load_mysql_relationships(database: &mut Database) -> Vec<Relationship> {
    let query = r#"
        SELECT
            kcu.TABLE_SCHEMA AS source_schema,
            kcu.TABLE_NAME AS source_table,
            GROUP_CONCAT(kcu.COLUMN_NAME ORDER BY kcu.ORDINAL_POSITION) AS source_columns,
            kcu.REFERENCED_TABLE_SCHEMA AS target_schema,
            kcu.REFERENCED_TABLE_NAME AS target_table,
            GROUP_CONCAT(kcu.REFERENCED_COLUMN_NAME ORDER BY kcu.ORDINAL_POSITION) AS target_columns,
            kcu.CONSTRAINT_NAME AS constraint_name
        FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
        WHERE kcu.REFERENCED_TABLE_NAME IS NOT NULL
          AND kcu.TABLE_SCHEMA = DATABASE()
        GROUP BY kcu.TABLE_SCHEMA, kcu.TABLE_NAME,
                 kcu.REFERENCED_TABLE_SCHEMA, kcu.REFERENCED_TABLE_NAME,
                 kcu.CONSTRAINT_NAME
        ORDER BY kcu.TABLE_SCHEMA, kcu.TABLE_NAME, kcu.CONSTRAINT_NAME
    "#;

    parse_relationship_results(database.execute_query(query).await.ok())
}

async fn load_sqlite_relationships(database: &mut Database) -> Vec<Relationship> {
    // SQLite needs per-table PRAGMA queries
    let tables_result = database
        .execute_query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .await;

    let tables = match tables_result {
        Ok(rows) => rows
            .into_iter()
            .skip(1) // skip header
            .filter_map(|row| row.first().cloned())
            .collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };

    let mut relationships = Vec::new();
    for table in &tables {
        let query = format!(
            "PRAGMA foreign_key_list(\"{}\")",
            table.replace('"', "\"\"")
        );
        if let Ok(rows) = database.execute_query(&query).await {
            // PRAGMA foreign_key_list returns: id, seq, table, from, to, on_update, on_delete, match
            for row in rows.into_iter().skip(1) {
                if row.len() >= 5 {
                    relationships.push(Relationship {
                        constraint_name: format!("fk_{}_{}", table, row[0]),
                        source_schema: "main".to_string(),
                        source_table: table.clone(),
                        source_columns: vec![row[3].clone()],
                        target_schema: "main".to_string(),
                        target_table: row[2].clone(),
                        target_columns: vec![row[4].clone()],
                    });
                }
            }
        }
    }

    relationships
}

/// Parse relationship query results (PostgreSQL/MySQL format)
/// Expected columns: source_schema, source_table, source_columns, target_schema, target_table, target_columns, constraint_name
fn parse_relationship_results(results: Option<Vec<Vec<String>>>) -> Vec<Relationship> {
    let rows = match results {
        Some(rows) if rows.len() > 1 => rows,
        _ => return Vec::new(),
    };

    rows.into_iter()
        .skip(1) // skip header
        .filter_map(|row| {
            if row.len() >= 7 {
                let source_cols = parse_column_list(&row[2]);
                let target_cols = parse_column_list(&row[5]);
                Some(Relationship {
                    source_schema: row[0].clone(),
                    source_table: row[1].clone(),
                    source_columns: source_cols,
                    target_schema: row[3].clone(),
                    target_table: row[4].clone(),
                    target_columns: target_cols,
                    constraint_name: row[6].clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Parse a column list from either comma-separated or PostgreSQL array format
fn parse_column_list(s: &str) -> Vec<String> {
    let s = s.trim();
    // Handle PostgreSQL array format: {col1,col2}
    let s = s
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(s);
    s.split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_column_list_csv() {
        assert_eq!(parse_column_list("id,name"), vec!["id", "name"]);
    }

    #[test]
    fn test_parse_column_list_pg_array() {
        assert_eq!(parse_column_list("{id,name}"), vec!["id", "name"]);
    }

    #[test]
    fn test_parse_column_list_single() {
        assert_eq!(parse_column_list("id"), vec!["id"]);
    }

    #[test]
    fn test_table_list_entry() {
        let header = TableListEntry::SchemaHeader {
            name: "public".to_string(),
        };
        assert!(!header.is_table());
        assert!(header.table_key().is_none());

        let table = TableListEntry::Table {
            name: "users".to_string(),
            schema: "public".to_string(),
            outgoing_fk: 2,
            incoming_fk: 1,
        };
        assert!(table.is_table());
        assert_eq!(
            table.table_key(),
            Some(("public".to_string(), "users".to_string()))
        );
    }

    #[test]
    fn test_parse_relationship_results_empty() {
        assert!(parse_relationship_results(None).is_empty());
        assert!(parse_relationship_results(Some(vec![])).is_empty());
    }

    #[test]
    fn test_parse_relationship_results() {
        let rows = vec![
            vec![
                "source_schema".into(),
                "source_table".into(),
                "source_columns".into(),
                "target_schema".into(),
                "target_table".into(),
                "target_columns".into(),
                "constraint_name".into(),
            ],
            vec![
                "public".into(),
                "posts".into(),
                "{author_id}".into(),
                "public".into(),
                "users".into(),
                "{id}".into(),
                "fk_posts_author".into(),
            ],
        ];
        let rels = parse_relationship_results(Some(rows));
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].source_table, "posts");
        assert_eq!(rels[0].target_table, "users");
        assert_eq!(rels[0].source_columns, vec!["author_id"]);
        assert_eq!(rels[0].target_columns, vec!["id"]);
    }
}
