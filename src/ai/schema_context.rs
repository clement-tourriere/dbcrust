//! Schema context builder for AI prompts
//! Collects database metadata into prompt-friendly CREATE TABLE pseudo-DDL.

use crate::database::{DatabaseType, DatabaseTypeExt};
use crate::db::Database;

/// Build schema context string for the AI system prompt.
/// For databases with many tables, uses keyword matching to focus on relevant tables.
pub async fn build_schema_context(
    db: &mut Database,
    user_query: &str,
    max_tables: usize,
) -> String {
    let db_type = db.get_database_type();
    let db_name = db.get_current_db();
    let server_version = get_server_version(db).await;

    let mut context = format!(
        "Database: {} ({} {})\n\n",
        db_name,
        db_type.display_name(),
        server_version
    );

    // Get all table names
    let tables = match db.get_tables_and_views(None).await {
        Ok(t) => t,
        Err(e) => {
            context.push_str(&format!("-- Error fetching tables: {e}\n"));
            return context;
        }
    };

    if tables.is_empty() {
        context.push_str("-- No tables found in database\n");
        return context;
    }

    // Select which tables to include in context
    let total_count = tables.len();
    let selected_tables = if total_count <= max_tables.min(30) {
        // Small database: include all tables
        tables
    } else {
        // Large database: use keyword matching to focus on relevant tables
        let selected = select_relevant_tables(&tables, user_query, max_tables);
        let remaining: Vec<_> = tables
            .iter()
            .filter(|t| !selected.contains(t))
            .take(20)
            .cloned()
            .collect();

        // Build DDL for selected tables
        for table_name in &selected {
            match db.get_table_details(table_name).await {
                Ok(details) => {
                    context.push_str(&format_table_ddl(&details, &db_type));
                    context.push('\n');
                }
                Err(_) => {
                    context.push_str(&format!("-- Table: {table_name} (details unavailable)\n\n"));
                }
            }
        }

        if selected.len() < total_count {
            context.push_str(&format!(
                "-- Showing {}/{} tables (most relevant to query). Other tables: {}\n",
                selected.len(),
                total_count,
                remaining.join(", ")
            ));
        }

        return context;
    };

    // Build DDL for all tables (small database case)
    for table_name in &selected_tables {
        match db.get_table_details(table_name).await {
            Ok(details) => {
                context.push_str(&format_table_ddl(&details, &db_type));
                context.push('\n');
            }
            Err(_) => {
                context.push_str(&format!("-- Table: {table_name} (details unavailable)\n\n"));
            }
        }
    }

    context
}

/// Select tables most relevant to the user's natural language query
fn select_relevant_tables(
    all_tables: &[String],
    user_query: &str,
    max_tables: usize,
) -> Vec<String> {
    let query_lower = user_query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    // Score each table by keyword match relevance
    let mut scored: Vec<(usize, &String)> = all_tables
        .iter()
        .map(|table| {
            let table_lower = table.to_lowercase();
            let mut score = 0usize;

            // Exact table name mention in query
            if query_lower.contains(&table_lower) {
                score += 100;
            }

            // Partial word matches
            for word in &query_words {
                if word.len() >= 3 && table_lower.contains(word) {
                    score += 10;
                }
                // Table name contains query word stem (simple stemming)
                let stem = word.strip_suffix('s').unwrap_or(word);
                if stem.len() >= 3 && table_lower.contains(stem) {
                    score += 5;
                }
            }

            // Table name words match query words
            let table_words: Vec<&str> = table_lower.split('_').collect();
            for tw in &table_words {
                for qw in &query_words {
                    if tw == qw || (tw.len() >= 3 && qw.starts_with(tw)) {
                        score += 8;
                    }
                }
            }

            (score, table)
        })
        .collect();

    // Sort by score descending
    scored.sort_by(|a, b| b.0.cmp(&a.0));

    // Take top N tables, but always include tables with non-zero score
    let mut selected: Vec<String> = scored
        .iter()
        .filter(|(score, _)| *score > 0)
        .take(max_tables)
        .map(|(_, table)| (*table).clone())
        .collect();

    // If we have room, fill with additional tables (they might be FK neighbors)
    if selected.len() < max_tables.min(15) {
        for (_, table) in &scored {
            if selected.len() >= max_tables.min(15) {
                break;
            }
            if !selected.contains(table) {
                selected.push((*table).clone());
            }
        }
    }

    selected
}

fn format_table_ddl(details: &crate::db::TableDetails, _db_type: &DatabaseType) -> String {
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

async fn get_server_version(db: &Database) -> String {
    if let Some(client) = db.get_database_client() {
        match client.get_server_info().await {
            Ok(info) => info.server_version,
            Err(_) => String::new(),
        }
    } else {
        String::new()
    }
}
