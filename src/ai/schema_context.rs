//! Schema context builder for AI prompts
//! Collects database metadata into prompt-friendly CREATE TABLE pseudo-DDL.

use crate::database::{DatabaseType, DatabaseTypeExt};
use crate::db::Database;

/// Build schema context string for the AI system prompt.
/// For databases with many tables, uses keyword matching to focus on relevant tables.
///
/// Returns `(context, cacheable)`. `cacheable` is true only when the context is
/// independent of `user_query` (the small-database "all tables" case) — for large
/// databases the table selection is query-specific, so the caller must NOT reuse it
/// across different questions.
pub async fn build_schema_context(
    db: &mut Database,
    user_query: &str,
    max_tables: usize,
) -> (String, bool) {
    let db_type = db.get_database_type();
    let db_name = db.get_current_db();
    let server_version = get_server_version(db).await;

    let mut context = format!(
        "Database: {} ({} {})\n\n",
        db_name,
        db_type.display_name(),
        server_version
    );

    // Get all table names (schema-qualified for non-public PostgreSQL schemas, so
    // analytics.orders is fetched/described correctly rather than as public.orders).
    let tables = match collect_table_names(db, &db_type).await {
        Ok(t) => t,
        Err(e) => {
            context.push_str(&format!("-- Error fetching tables: {e}\n"));
            // Transient error — do not cache.
            return (context, false);
        }
    };

    if tables.is_empty() {
        context.push_str("-- No tables found in database\n");
        return (context, true);
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

        // Build DDL for selected tables (fetched concurrently to cut tunnel latency)
        for (table_name, details) in db.get_table_details_bulk(&selected).await {
            match details {
                Some(details) => {
                    context.push_str(&format_table_ddl(&details, &db_type));
                    context.push('\n');
                }
                None => {
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

        // Query-specific selection — not safe to reuse for a different question.
        return (context, false);
    };

    // Build DDL for all tables (small database case), fetched concurrently
    for (table_name, details) in db.get_table_details_bulk(&selected_tables).await {
        match details {
            Some(details) => {
                context.push_str(&format_table_ddl(&details, &db_type));
                context.push('\n');
            }
            None => {
                context.push_str(&format!("-- Table: {table_name} (details unavailable)\n\n"));
            }
        }
    }

    // All tables included regardless of the query — safe to cache for the session.
    (context, true)
}

/// Build a *lightweight* seed context for the agentic assistant: database
/// identity plus the table/view name list, with NO per-table catalog queries.
/// The agent pulls full details on demand via its `describe_table` tool, so the
/// first turn stays cheap even over a slow SSH tunnel.
///
/// For PostgreSQL with non-`public` schemas, names are **schema-qualified**
/// (`analytics.orders`) so the agent knows the full namespace up front; the agent
/// can pass those straight to `describe_table` / `run_sql`.
pub async fn build_agent_seed_context(db: &mut Database) -> String {
    let db_type = db.get_database_type();
    let db_name = db.get_current_db();
    let server_version = get_server_version(db).await;

    let mut context = format!(
        "Database: {} ({} {})\n\n",
        db_name,
        db_type.display_name(),
        server_version
    );

    match collect_table_names(db, &db_type).await {
        Ok(tables) if !tables.is_empty() => {
            context.push_str(&format!("Tables and views ({}):\n", tables.len()));
            for name in &tables {
                context.push_str(&format!("  {name}\n"));
            }
        }
        Ok(_) => context.push_str("-- No tables found in database\n"),
        Err(e) => context.push_str(&format!("-- Error fetching tables: {e}\n")),
    }

    context
}

/// Collect the table list for AI context. PostgreSQL with non-public schemas gets
/// schema-qualified names (`schema.table`); public tables and every other backend
/// stay unqualified — and the per-schema round-trips are skipped entirely when
/// there is nothing to disambiguate. Shared by the `??` schema context and the
/// `???` seed so both describe non-public tables correctly.
async fn collect_table_names(
    db: &mut Database,
    db_type: &DatabaseType,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if !matches!(db_type, DatabaseType::PostgreSQL) {
        return db.get_tables_and_views(None).await;
    }

    let schemas = db.get_schemas().await.unwrap_or_default();
    if !schemas.iter().any(|s| s != "public") {
        // Only the default schema — bare names, a single round-trip.
        return db.get_tables_and_views(None).await;
    }

    let mut entries = Vec::new();
    for schema in &schemas {
        let tables = db
            .get_tables_and_views(Some(schema))
            .await
            .unwrap_or_default();
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
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));

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

pub(crate) fn format_table_ddl(
    details: &crate::db::TableDetails,
    _db_type: &DatabaseType,
) -> String {
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
