//! Schema context builder for AI prompts
//! Collects database metadata into prompt-friendly CREATE TABLE pseudo-DDL.

use crate::AiSession;
use async_trait::async_trait;
use dbcrust_core::database::{DatabaseType, DatabaseTypeExt};
pub use dbcrust_core::ddl::format_table_ddl;
use dbcrust_core::ddl::{self, TableCatalog};

/// Build schema context string for the AI system prompt.
/// For databases with many tables, uses keyword matching to focus on relevant tables.
///
/// Returns `(context, cacheable)`. `cacheable` is true only when the context is
/// independent of `user_query` (the small-database "all tables" case) — for large
/// databases the table selection is query-specific, so the caller must NOT reuse it
/// across different questions.
pub async fn build_schema_context(
    db: &mut dyn AiSession,
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
pub async fn build_agent_seed_context(db: &mut dyn AiSession) -> String {
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

/// Collect the table list for AI context (see
/// [`dbcrust_core::ddl::collect_table_names`]: PostgreSQL with non-public
/// schemas gets schema-qualified names). Shared by the `??` schema context, the
/// `???` seed, and the `\ddl` command.
pub async fn collect_table_names(
    db: &mut dyn AiSession,
    db_type: &DatabaseType,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    ddl::collect_table_names(&mut SessionCatalog(db), db_type).await
}

/// [`AiSession`] seen as a [`TableCatalog`].
struct SessionCatalog<'a>(&'a mut dyn AiSession);

#[async_trait(?Send)]
impl TableCatalog for SessionCatalog<'_> {
    async fn catalog_schemas(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        self.0.get_schemas().await
    }

    async fn catalog_tables(
        &mut self,
        schema_filter: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        self.0.get_tables_and_views(schema_filter).await
    }
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

async fn get_server_version(db: &dyn AiSession) -> String {
    if let Some(client) = db.get_database_client() {
        match client.get_server_info().await {
            Ok(info) => info.server_version,
            Err(_) => String::new(),
        }
    } else {
        String::new()
    }
}
