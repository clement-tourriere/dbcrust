use crate::config::{Config, SavedSession};
use crate::db::Database;
use crate::debug_log;
use crate::commands::CommandParser;
use nu_ansi_term::{Color, Style};
use reedline::{Completer, Span, Suggestion};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio;

// The hardcoded BACKSLASH_COMMANDS array has been removed.
// Commands are now dynamically retrieved from CommandParser.

pub struct SqlCompleter {
    database: Arc<Mutex<Database>>,
    sql_keywords: Vec<String>,
    config: Config,
    schemas_cache: Option<Vec<String>>,
    tables_cache: Option<HashMap<String, Vec<String>>>,
    columns_cache: Option<HashMap<String, Vec<String>>>,
    functions_cache: Option<HashMap<String, Vec<String>>>,
    cache_last_updated: Option<Instant>,
    cache_ttl: Duration,
    ssh_tunnel_cache_ttl: Duration, // Extended TTL for SSH tunnel connections
    cached_for_dbname: Option<String>,
    cached_for_host: Option<String>,
    cached_for_port: Option<u16>,
    batch_fetch_in_progress: Arc<AtomicBool>,
}

// NoopCompleter that does nothing - used when autocomplete is disabled
pub struct NoopCompleter {}

impl Completer for NoopCompleter {
    fn complete(&mut self, _line: &str, _pos: usize) -> Vec<Suggestion> {
        // Return empty suggestions - no autocompletion
        Vec::new()
    }
}

impl SqlCompleter {
    #[allow(dead_code)]
    pub fn new(database: Arc<Mutex<Database>>) -> Self {
        // Common SQL keywords for PostgreSQL
        let sql_keywords = vec![
            "SELECT",
            "FROM",
            "WHERE",
            "INSERT",
            "UPDATE",
            "DELETE",
            "DROP",
            "CREATE",
            "ALTER",
            "TABLE",
            "VIEW",
            "INDEX",
            "TRIGGER",
            "FUNCTION",
            "PROCEDURE",
            "SCHEMA",
            "DATABASE",
            "GROUP BY",
            "ORDER BY",
            "HAVING",
            "JOIN",
            "LEFT JOIN",
            "RIGHT JOIN",
            "INNER JOIN",
            "FULL JOIN",
            "CROSS JOIN",
            "UNION",
            "INTERSECT",
            "EXCEPT",
            "LIMIT",
            "OFFSET",
            "ASC",
            "DESC",
            "DISTINCT",
            "ALL",
            "IN",
            "BETWEEN",
            "LIKE",
            "ILIKE",
            "SIMILAR TO",
            "IS NULL",
            "IS NOT NULL",
            "AND",
            "OR",
            "NOT",
            "TRUE",
            "FALSE",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        Self {
            database,
            sql_keywords,
            config: Config::load(),
            schemas_cache: None,
            tables_cache: None,
            columns_cache: None,
            functions_cache: None,
            cache_last_updated: None,
            cache_ttl: Duration::from_secs(300), // 5 minutes for direct connections
            ssh_tunnel_cache_ttl: Duration::from_secs(1800), // 30 minutes for SSH tunnel connections
            cached_for_dbname: None,
            cached_for_host: None,
            cached_for_port: None,
            batch_fetch_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn clear_cache(&mut self) {
        // println!("DEBUG: Clearing SqlCompleter schema cache.");
        self.schemas_cache = None;
        self.tables_cache = None;
        self.columns_cache = None;
        self.functions_cache = None;
        self.cache_last_updated = None;
        // Also clear the dbname associated with the cleared cache
        self.cached_for_dbname = None;
        self.cached_for_host = None;
        self.cached_for_port = None;
    }

    // Method to update cache metadata (like dbname, host, port, and last_updated time)
    // Call this whenever the cache is successfully populated/updated.
    fn update_cache_metadata(&mut self, dbname: &str, host: &str, port: u16) {
        self.cached_for_dbname = Some(dbname.to_string());
        self.cached_for_host = Some(host.to_string());
        self.cached_for_port = Some(port);
        self.cache_last_updated = Some(Instant::now());
    }

    // Helper method to detect if we're using SSH tunnels
    fn is_using_ssh_tunnel(&self, host: &str, port: u16) -> bool {
        // If we're connecting to localhost/127.0.0.1 on a non-standard port,
        // it's likely an SSH tunnel
        (host == "127.0.0.1" || host == "localhost") && port != 5432
    }

    // Method to check if the cache is still valid for the current DB connection
    // and if the TTL has expired. Uses intelligent invalidation logic.
    fn ensure_cache_validity(
        &mut self,
        current_dbname: &str,
        current_host: &str,
        current_port: u16,
    ) {
        let cached_dbname = self.cached_for_dbname.as_ref();
        let cached_host = self.cached_for_host.as_ref();
        let cached_port = self.cached_for_port;

        // Check if this is a meaningful change that requires cache invalidation
        let database_changed = cached_dbname.is_none_or(|name| name != current_dbname);
        
        // For SSH tunnels, be more lenient about host/port changes since they're local tunnels
        let using_ssh_tunnel = self.is_using_ssh_tunnel(current_host, current_port);
        let was_using_ssh_tunnel = cached_host.is_some_and(|host| 
            cached_port.is_some_and(|port| self.is_using_ssh_tunnel(host, port))
        );
        
        // Smart connection change detection
        let connection_changed = if using_ssh_tunnel && was_using_ssh_tunnel {
            // For SSH tunnels, only care about database name changes, not port changes
            false
        } else if using_ssh_tunnel || was_using_ssh_tunnel {
            // Changed from/to SSH tunnel - this is a significant change
            true
        } else {
            // Direct connections - check host and port strictly
            cached_host.is_none_or(|host| host != current_host) ||
            (cached_port != Some(current_port))
        };

        // Use extended TTL for SSH tunnel connections
        let effective_ttl = if using_ssh_tunnel {
            self.ssh_tunnel_cache_ttl
        } else {
            self.cache_ttl
        };

        let ttl_expired = self
            .cache_last_updated
            .is_none_or(|updated| updated.elapsed() >= effective_ttl);

        // Only clear cache if there's a meaningful change or TTL expired
        if database_changed || connection_changed || ttl_expired {
            if database_changed {
                debug_log!("[ensure_cache_validity] Database changed, clearing cache");
            } else if connection_changed {
                debug_log!("[ensure_cache_validity] Connection type changed, clearing cache");
            } else if ttl_expired {
                debug_log!(
                    "[ensure_cache_validity] Cache TTL expired (using {} seconds for SSH tunnel: {})",
                    effective_ttl.as_secs(),
                    using_ssh_tunnel
                );
            }
            
            // Selective cache clearing - only clear what's necessary
            if database_changed || connection_changed {
                // Clear all caches if database or connection type changed
                self.clear_cache();
            } else if ttl_expired {
                // For TTL expiration, do a soft refresh by just clearing timestamps
                // This allows reuse of cached data if queries fail
                self.cache_last_updated = None;
            }
        }
    }

    fn get_named_queries(&self) -> Vec<(String, String)> {
        self.config.list_named_queries()
    }

    fn get_saved_sessions(&self) -> Vec<(String, SavedSession)> {
        self.config.list_sessions()
    }

    /// Batch fetch all metadata in a single optimized operation
    fn batch_fetch_all_metadata(&mut self) -> Result<(), String> {
        let start_time = std::time::Instant::now();
        debug_log!("[batch_fetch_all_metadata] Starting batch fetch");

        // Check if batch fetch is already in progress
        if self.batch_fetch_in_progress.load(Ordering::Relaxed) {
            debug_log!("[batch_fetch_all_metadata] Batch fetch already in progress, skipping");
            return Ok(());
        }

        let (current_dbname, current_host, current_port) = {
            let db_guard = self.database.lock().unwrap();
            (
                db_guard.get_current_db(),
                db_guard.get_host().to_string(),
                db_guard.get_port(),
            )
        };
        self.ensure_cache_validity(&current_dbname, &current_host, current_port);

        // If cache is still valid, no need to fetch
        if self.schemas_cache.is_some() && self.tables_cache.is_some() {
            debug_log!("[batch_fetch_all_metadata] Cache is valid, skipping batch fetch");
            return Ok(());
        }

        // Set batch fetch in progress
        self.batch_fetch_in_progress.store(true, Ordering::Relaxed);

        let db_clone: Arc<Mutex<Database>> = Arc::clone(&self.database);
        let batch_fetch_flag = Arc::clone(&self.batch_fetch_in_progress);
        
        let fetched_data = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let db_guard = db_clone.lock().unwrap();
            debug_log!("[batch_fetch_all_metadata:thread] Starting batch queries");

            // Execute all queries concurrently using tokio::try_join! with timeout
            let batch_start = std::time::Instant::now();
            let result = rt.block_on(async {
                let pool = match db_guard.get_pool() {
                    Some(p) => p,
                    None => return Err("No database pool available".to_string()),
                };

                // Use faster pg_catalog queries instead of information_schema views
                let schemas_future = sqlx::query(
                    r#"
                    SELECT nspname as schema_name
                    FROM pg_namespace
                    WHERE nspname NOT LIKE 'pg_%' 
                      AND nspname NOT IN ('information_schema', 'pg_toast')
                    ORDER BY nspname
                    "#,
                ).fetch_all(pool);

                let tables_future = sqlx::query(
                    r#"
                    SELECT c.relname as table_name, n.nspname as table_schema
                    FROM pg_class c
                    INNER JOIN pg_namespace n ON c.relnamespace = n.oid
                    WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p')  -- tables, views, materialized views, foreign tables, partitioned tables
                      AND n.nspname NOT LIKE 'pg_%'
                      AND n.nspname NOT IN ('information_schema', 'pg_toast')
                    ORDER BY n.nspname, c.relname
                    "#,
                ).fetch_all(pool);

                let functions_future = sqlx::query(
                    r#"
                    SELECT p.proname as routine_name, n.nspname as routine_schema
                    FROM pg_proc p
                    INNER JOIN pg_namespace n ON p.pronamespace = n.oid
                    WHERE p.prokind = 'f'  -- only functions, not procedures or aggregates
                      AND n.nspname NOT LIKE 'pg_%'
                      AND n.nspname NOT IN ('information_schema', 'pg_toast')
                    ORDER BY n.nspname, p.proname
                    "#,
                ).fetch_all(pool);

                // For large databases, execute queries sequentially to reduce connection pressure
                // For smaller databases, execute concurrently for speed
                let batch_future = async {
                    // First get a quick table count to determine strategy
                    let table_count_result = sqlx::query(
                        r#"
                        SELECT COUNT(*)::int as table_count
                        FROM pg_class c
                        INNER JOIN pg_namespace n ON c.relnamespace = n.oid
                        WHERE c.relkind IN ('r', 'v', 'm', 'f', 'p')
                          AND n.nspname NOT LIKE 'pg_%'
                          AND n.nspname NOT IN ('information_schema', 'pg_toast')
                        "#,
                    ).fetch_one(pool).await;

                    let use_sequential = match table_count_result {
                        Ok(row) => {
                            let count: i32 = row.get("table_count");
                            count > 50 // Use sequential for databases with >50 tables
                        }
                        Err(_) => false, // Default to concurrent if count fails
                    };

                    if use_sequential {
                        debug_log!("[batch_fetch_all_metadata] Using sequential queries for large database");
                        // Execute queries sequentially to reduce connection pressure
                        let schemas_result = schemas_future.await?;
                        let tables_result = tables_future.await?;
                        let functions_result = functions_future.await?;
                        Ok((schemas_result, tables_result, functions_result))
                    } else {
                        debug_log!("[batch_fetch_all_metadata] Using concurrent queries for small database");
                        // Execute queries concurrently for speed
                        tokio::try_join!(
                            schemas_future,
                            tables_future,
                            functions_future
                        )
                    }
                };

                match tokio::time::timeout(std::time::Duration::from_secs(45), batch_future).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => Err("Batch metadata fetch timed out after 45 seconds".to_string()),
                }
            });

            debug_log!("[batch_fetch_all_metadata:thread] Batch queries completed in {:?}", batch_start.elapsed());

            // Reset the flag when done
            batch_fetch_flag.store(false, Ordering::Relaxed);

            result.map_err(|e| e.to_string())
        })
        .join();

        match fetched_data {
            Ok(Ok((schemas_rows, tables_rows, functions_rows))) => {
                // Process schemas
                let schemas: Vec<String> = schemas_rows
                    .iter()
                    .map(|row| row.get::<String, _>(0))
                    .collect();

                // Process tables by schema
                let mut tables_by_schema: HashMap<String, Vec<String>> = HashMap::new();
                for row in tables_rows {
                    let table_name: String = row.get(0);
                    let schema_name: String = row.get(1);
                    tables_by_schema
                        .entry(schema_name)
                        .or_default()
                        .push(table_name);
                }

                // Process functions by schema
                let mut functions_by_schema: HashMap<String, Vec<String>> = HashMap::new();
                for row in functions_rows {
                    let function_name: String = row.get(0);
                    let schema_name: String = row.get(1);
                    functions_by_schema
                        .entry(schema_name)
                        .or_default()
                        .push(function_name);
                }

                // Update all caches
                self.schemas_cache = Some(schemas);
                self.tables_cache = Some(tables_by_schema);
                self.functions_cache = Some(functions_by_schema);
                self.update_cache_metadata(&current_dbname, &current_host, current_port);

                let total_duration = start_time.elapsed();
                debug_log!(
                    "[batch_fetch_all_metadata] Successfully batch fetched all metadata in {:?}",
                    total_duration
                );

                Ok(())
            }
            Ok(Err(e)) => {
                debug_log!("[batch_fetch_all_metadata] Database error: {}", e);
                Err(e)
            }
            Err(e) => {
                debug_log!("[batch_fetch_all_metadata] Thread panic: {:?}", e);
                Err("Thread panicked during batch fetch".to_string())
            }
        }
    }

    fn complete_backslash_commands(&self, line: &str, pos: usize) -> Vec<Suggestion> {
        let mut completions = Vec::new();
        // The word being completed (e.g., "\\d" or "\\dt")
        // Find the start of the current word (everything after the last space, or start of line)
        let word_start = line[..pos].rfind(' ').map_or(0, |idx| idx + 1);
        // Ensure we only consider the part after '\' for matching if it's the first word
        let command_start_index = if word_start == 0 && line.starts_with('\\') {
            1
        } else {
            word_start
        };
        let current_command_part = &line[command_start_index..pos];

        // Get command info from the CommandParser system
        for (_category, commands) in CommandParser::get_commands_by_category() {
            for (cmd_name, cmd_description) in commands {
                let cmd_name_no_slash = if cmd_name.starts_with('\\') {
                    &cmd_name[1..] // remove leading '\' for matching
                } else {
                    cmd_name
                };
                
                if cmd_name_no_slash.starts_with(current_command_part) {
                    completions.push(Suggestion {
                        value: cmd_name.to_string(),
                        description: Some(cmd_description.to_string()),
                        span: Span {
                            start: word_start, // Replace from the beginning of the typed command part
                            end: pos,
                        },
                        append_whitespace: !cmd_name.ends_with("threshold"), // Add a space unless it needs a value after
                        extra: None,
                        style: None,
                    });
                }
            }
        }
        completions
    }

    fn fetch_schemas(&mut self) -> Vec<String> {
        let start_time = std::time::Instant::now();
        debug_log!("[fetch_schemas] Starting fetch at {:?}", start_time);

        let (current_dbname, current_host, current_port) = {
            let db_guard = self.database.lock().unwrap();
            (
                db_guard.get_current_db(),
                db_guard.get_host().to_string(),
                db_guard.get_port(),
            )
        };
        self.ensure_cache_validity(&current_dbname, &current_host, current_port);

        if let Some(ref schemas) = self.schemas_cache {
            let duration = start_time.elapsed();
            debug_log!(
                "[fetch_schemas] Cache hit! Returning {} schemas in {:?}",
                schemas.len(),
                duration
            );
            return schemas.clone();
        }

        // Try batch fetch first for better performance
        if let Ok(()) = self.batch_fetch_all_metadata() {
            if let Some(ref schemas) = self.schemas_cache {
                let duration = start_time.elapsed();
                debug_log!(
                    "[fetch_schemas] Batch fetch successful! Returning {} schemas in {:?}",
                    schemas.len(),
                    duration
                );
                return schemas.clone();
            }
        }

        // Fallback to individual fetch if batch fails
        debug_log!("[fetch_schemas] Falling back to individual fetch");
        let db_clone: Arc<Mutex<Database>> = Arc::clone(&self.database);
        let fetched_data_from_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let mut db_guard = db_clone.lock().unwrap();
            let result = rt.block_on(db_guard.get_schemas());
            result.map_err(|e| e.to_string())
        })
        .join();

        match fetched_data_from_thread {
            Ok(Ok(schemas)) => {
                self.schemas_cache = Some(schemas.clone());
                self.update_cache_metadata(&current_dbname, &current_host, current_port);
                debug_log!("[fetch_schemas] Individual fetch successful: {} schemas", schemas.len());
                schemas
            }
            Ok(Err(e)) => {
                eprintln!("Error fetching schemas: {e}");
                Vec::new()
            }
            Err(e_join) => {
                eprintln!("Thread panicked fetching schemas: {e_join:?}");
                Vec::new()
            }
        }
    }

    fn fetch_tables(&mut self, schema: &str) -> Vec<String> {
        let _start_time = std::time::Instant::now();
        debug_log!("[fetch_tables] Starting fetch for schema: '{}'", schema);

        let (current_dbname, current_host, current_port) = {
            let db_guard = self.database.lock().unwrap();
            (
                db_guard.get_current_db(),
                db_guard.get_host().to_string(),
                db_guard.get_port(),
            )
        };
        self.ensure_cache_validity(&current_dbname, &current_host, current_port);

        // Check if data is in cache
        if let Some(ref tables_map) = self.tables_cache {
            if let Some(tables_in_schema) = tables_map.get(schema) {
                debug_log!("[fetch_tables] Cache hit! Returning {} tables for schema '{}'", 
                          tables_in_schema.len(), schema);
                return tables_in_schema.clone();
            }
        }

        // Try batch fetch first for better performance
        if let Ok(()) = self.batch_fetch_all_metadata() {
            if let Some(ref tables_map) = self.tables_cache {
                if let Some(tables_in_schema) = tables_map.get(schema) {
                    debug_log!("[fetch_tables] Batch fetch successful! Returning {} tables for schema '{}'",
                              tables_in_schema.len(), schema);
                    return tables_in_schema.clone();
                }
            }
        }

        // Fallback to individual fetch if batch fails or schema not found
        debug_log!("[fetch_tables] Falling back to individual fetch for schema '{}'", schema);
        let db_clone: Arc<Mutex<Database>> = Arc::clone(&self.database);
        let schema_owned = schema.to_string();
        let fetched_data_from_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let mut db_guard = db_clone.lock().unwrap();
            let schema_opt_for_db: Option<&str> = if schema_owned.is_empty() {
                None
            } else {
                Some(&schema_owned)
            };

            let result = rt.block_on(db_guard.get_tables_and_views(schema_opt_for_db));
            result.map_err(|e| e.to_string())
        })
        .join();

        match fetched_data_from_thread {
            Ok(Ok(tables)) => {
                let mut new_tables_map = self.tables_cache.clone().unwrap_or_default();
                new_tables_map.insert(schema.to_string(), tables.clone());
                self.tables_cache = Some(new_tables_map);
                self.update_cache_metadata(&current_dbname, &current_host, current_port);
                debug_log!("[fetch_tables] Individual fetch successful: {} tables for schema '{}'",
                          tables.len(), schema);
                tables
            }
            Ok(Err(e)) => {
                eprintln!("Error fetching tables for schema '{schema}': {e}");
                Vec::new()
            }
            Err(e_join) => {
                eprintln!("Thread panicked fetching tables for schema '{schema}': {e_join:?}");
                Vec::new()
            }
        }
    }

    fn fetch_functions(&mut self, schema: &str) -> Vec<String> {
        let (current_dbname, current_host, current_port) = {
            let db_guard = self.database.lock().unwrap();
            (
                db_guard.get_current_db(),
                db_guard.get_host().to_string(),
                db_guard.get_port(),
            )
        };
        self.ensure_cache_validity(&current_dbname, &current_host, current_port);

        let cache_key = schema.to_string();
        if let Some(ref functions_map) = self.functions_cache {
            if let Some(functions_in_schema) = functions_map.get(&cache_key) {
                return functions_in_schema.clone();
            }
        }

        // Try batch fetch first for better performance
        if let Ok(()) = self.batch_fetch_all_metadata() {
            if let Some(ref functions_map) = self.functions_cache {
                if let Some(functions_in_schema) = functions_map.get(&cache_key) {
                    debug_log!("[fetch_functions] Batch fetch successful for schema '{}'", schema);
                    return functions_in_schema.clone();
                }
            }
        }

        // Fallback to individual fetch if batch fails or schema not found
        debug_log!("[fetch_functions] Falling back to individual fetch for schema '{}'", schema);
        let db_clone: Arc<Mutex<Database>> = Arc::clone(&self.database);
        let schema_owned = schema.to_string();
        let fetched_data_from_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let mut db_guard = db_clone.lock().unwrap();
            rt.block_on(db_guard.get_functions(Some(&schema_owned)))
                .map_err(|e| e.to_string())
        })
        .join();

        match fetched_data_from_thread {
            Ok(Ok(funcs)) => {
                let mut new_functions_map = self.functions_cache.clone().unwrap_or_default();
                new_functions_map.insert(cache_key, funcs.clone());
                self.functions_cache = Some(new_functions_map);
                self.update_cache_metadata(&current_dbname, &current_host, current_port);
                funcs
            }
            Ok(Err(e)) => {
                eprintln!("Error fetching functions for schema '{schema}': {e}");
                Vec::new()
            }
            Err(e_join) => {
                eprintln!("Thread panicked fetching functions for schema '{schema}': {e_join:?}");
                Vec::new()
            }
        }
    }

    fn fetch_columns(&mut self, table_name_with_schema: &str) -> Vec<String> {
        let start_time = std::time::Instant::now();
        debug_log!(
            "[fetch_columns] Starting fetch for table: '{}' at {:?}",
            table_name_with_schema,
            start_time
        );

        let (current_dbname, current_host, current_port) = {
            let db_guard = self.database.lock().unwrap();
            (
                db_guard.get_current_db(),
                db_guard.get_host().to_string(),
                db_guard.get_port(),
            )
        };
        self.ensure_cache_validity(&current_dbname, &current_host, current_port);

        let cache_key = table_name_with_schema.to_string();
        if let Some(ref columns_map) = self.columns_cache {
            if let Some(columns_for_table) = columns_map.get(&cache_key) {
                let duration = start_time.elapsed();
                debug_log!(
                    "[fetch_columns] Cache hit! Returning {} columns for table '{}' in {:?}",
                    columns_for_table.len(),
                    table_name_with_schema,
                    duration
                );
                return columns_for_table.clone();
            }
        }

        // Cache miss
        debug_log!(
            "[fetch_columns] Cache miss for table: '{}', spawning thread",
            table_name_with_schema
        );
        let thread_start = std::time::Instant::now();

        // Cache miss
        let db_clone: Arc<Mutex<Database>> = Arc::clone(&self.database);
        let table_name_with_schema_owned = table_name_with_schema.to_string();
        let fetched_data_from_thread = std::thread::spawn(move || {
            // Create a new runtime for the thread
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            debug_log!("[fetch_columns:thread] Runtime built");

            // Execute the query with the runtime
            let mut db_guard = db_clone.lock().unwrap();
            debug_log!("[fetch_columns:thread] Lock acquired");

            let parts: Vec<&str> = table_name_with_schema_owned.splitn(2, '.').collect();
            let (schema_opt, table_only_name) = if parts.len() == 2 {
                (Some(parts[0]), parts[1])
            } else {
                (None, parts[0])
            };

            let query_start = std::time::Instant::now();
            let result = rt.block_on(db_guard.get_columns_for_table(table_only_name, schema_opt));
            debug_log!(
                "[fetch_columns:thread] DB query completed in {:?}",
                query_start.elapsed()
            );

            result.map_err(|e| e.to_string())
        })
        .join();

        let thread_duration = thread_start.elapsed();
        debug_log!("[fetch_columns] Thread completed in {:?}", thread_duration);

        match fetched_data_from_thread {
            Ok(Ok(cols)) => {
                let mut new_columns_map = self.columns_cache.clone().unwrap_or_default();
                new_columns_map.insert(cache_key, cols.clone());
                self.columns_cache = Some(new_columns_map);
                self.update_cache_metadata(&current_dbname, &current_host, current_port);

                let total_duration = start_time.elapsed();
                debug_log!(
                    "[fetch_columns] Successfully fetched {} columns for table '{}' in {:?} (thread: {:?})",
                    cols.len(),
                    table_name_with_schema,
                    total_duration,
                    thread_duration
                );
                cols
            }
            Ok(Err(e)) => {
                let total_duration = start_time.elapsed();
                eprintln!(
                    "Error fetching columns for table '{table_name_with_schema}' in completer thread: {e} (took {total_duration:?})"
                );
                Vec::new()
            }
            Err(e_join) => {
                let total_duration = start_time.elapsed();
                eprintln!(
                    "Completer thread for columns (table '{table_name_with_schema}') panicked: {e_join:?} (took {total_duration:?})"
                );
                Vec::new()
            }
        }
    }
}

impl Completer for SqlCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        // At the beginning of each completion request, ensure cache validity.
        // This handles both TTL and DB connection changes.
        {
            let (current_dbname, current_host, current_port) = {
                let db_guard = self.database.lock().unwrap();
                (
                    db_guard.get_current_db(),
                    db_guard.get_host().to_string(),
                    db_guard.get_port(),
                )
            };
            self.ensure_cache_validity(&current_dbname, &current_host, current_port);
        }

        // Proactively start batch fetch on first completion for better performance
        if self.schemas_cache.is_none() && !self.batch_fetch_in_progress.load(Ordering::Relaxed) {
            debug_log!("[complete] Starting proactive batch fetch for performance optimization");
            let _ = self.batch_fetch_all_metadata(); // Fire and forget for background caching
        }

        if line.is_empty() && pos == 0 {
            return Vec::new(); // Early exit for empty line
        }
        let mut completions = Vec::new();

        // Determine the start of the word to be completed
        let word_start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == '.' || c == ',')
            .map_or(0, |idx| idx + 1);

        let current_word = &line[word_start..pos]; // Define current_word here

        // If the line starts with `\`, it's a backslash command
        if line.starts_with('\\') {
            // If there's a space after the initial command part (e.g., "\l "),
            // it means the command itself is likely complete, and we might be completing an argument.
            let command_part_end = line.find(' ').unwrap_or(line.len());
            let is_after_command_with_space = line[1..].contains(' ') && pos > command_part_end;

            if is_after_command_with_space {
                // Argument completion logic for specific commands
                if line.starts_with("\\l ") {
                    // Check for \l specifically
                    return Vec::new(); // \l does not take arguments after space
                }
                if line.starts_with("\\c ") || line.starts_with("\\connect ") {
                    let db_clone: Arc<Mutex<Database>> = Arc::clone(&self.database);
                    match std::thread::spawn(move || -> Vec<String> {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        if let Ok(mut db_guard) = db_clone.lock() {
                            match rt.block_on(db_guard.list_database_names()) {
                                Ok(databases) => databases,
                                Err(e) => {
                                    eprintln!("Error listing databases in completer thread: {e}");
                                    Vec::<String>::new()
                                }
                            }
                        } else {
                            eprintln!("Failed to lock database in completer thread for list_database_names");
                            Vec::<String>::new()
                        }
                    }).join() {
                        Ok(databases_from_thread) => {
                            for dbname in databases_from_thread {
                                if dbname.starts_with(current_word) {
                                    completions.push(Suggestion {
                                        value: dbname.clone(),
                                        description: Some("Database".to_string()),
                                        span: Span {
                                            start: word_start,
                                            end: pos,
                                        },
                                        append_whitespace: true,
                                        extra: None,
                                        style: Some(Style::new().fg(Color::Yellow)),
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Completer thread for list_database_names panicked: {e:?}");
                        }
                    }
                    return completions; // Return completions gathered so far for \c
                }
                // For \d <tablename> type completion
                if line.starts_with("\\d ") {
                    let current_word_lower = current_word.to_lowercase();

                    // Handle schema prefixes if the current word contains a dot
                    if current_word_lower.contains('.') {
                        let parts: Vec<&str> = current_word_lower.splitn(2, '.').collect();
                        let schema_prefix = parts[0];
                        let table_prefix = parts[1];

                        let schemas = self.fetch_schemas();
                        for schema in &schemas {
                            if schema.to_lowercase() == schema_prefix {
                                let tables_in_schema = self.fetch_tables(schema);
                                for table in tables_in_schema {
                                    if table.to_lowercase().starts_with(table_prefix) {
                                        completions.push(Suggestion {
                                            value: format!("{schema}.{table}"),
                                            description: Some(format!("Table in {schema}")),
                                            span: Span {
                                                start: word_start,
                                                end: pos,
                                            },
                                            append_whitespace: true,
                                            extra: None,
                                            style: Some(Style::new().fg(Color::Green)),
                                        });
                                    }
                                }
                            }
                        }
                    } else {
                        // For \d command, only suggest tables from all schemas - no schema prefixes
                        let all_tables = self.fetch_tables("");
                        for table in all_tables {
                            if table.to_lowercase().starts_with(&current_word_lower) {
                                completions.push(Suggestion {
                                    value: table.clone(),
                                    description: Some("Table/View".to_string()),
                                    span: Span {
                                        start: word_start,
                                        end: pos,
                                    },
                                    append_whitespace: true,
                                    extra: None,
                                    style: Some(Style::new().fg(Color::Green)),
                                });
                            }
                        }
                    }
                    return completions;
                }
                // For \n <query_name> type completion (named queries)
                if line.starts_with("\\n ") {
                    let prefix = &line[3..]; // Skip "\n "
                    let arg_word_start = prefix[..pos.saturating_sub(3)]
                        .rfind(char::is_whitespace)
                        .map_or(0, |idx| idx + 1)
                        + 3; // Adjust back to line based index
                    let current_arg_word = &line[arg_word_start..pos];

                    let named_queries = self.get_named_queries();
                    for (name, query) in named_queries {
                        if name.starts_with(current_arg_word) {
                            completions.push(Suggestion {
                                value: name.clone(),
                                description: Some(format!("Named query: {query}")),
                                span: Span {
                                    start: arg_word_start,
                                    end: pos,
                                },
                                append_whitespace: true,
                                extra: None,
                                style: None,
                            });
                        }
                    }
                    return completions;
                }
                // For other commands that take arguments like \w <filename>, \i <filename>,
                // \ns <name> <query>, \nd <name>, \s <session_name>, \ss <session_name>, \sd <session_name>
                // We will rely on the existing named query/session completion logic below for some cases.
                // For file paths, reedline's default file completer might be better if enabled.
                // For now, no specific file path completion here.
            } else {
                // Completing the backslash command itself (e.g., user typed "\d")
                return self.complete_backslash_commands(line, pos);
            }
        }

        // Check if we're completing a named query deletion command
        if let Some(prefix) = line.strip_prefix("\\nd ") {
            // Skip "\nd "
            let arg_word_start = prefix[..pos.saturating_sub(4)]
                .rfind(char::is_whitespace)
                .map_or(0, |idx| idx + 1)
                + 4;
            let current_arg_word = &line[arg_word_start..pos];

            let named_queries = self.get_named_queries();
            for (name, _) in named_queries {
                if name.starts_with(current_arg_word) {
                    completions.push(Suggestion {
                        value: name.clone(),
                        description: Some("Named query to delete".to_string()),
                        span: Span {
                            start: arg_word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: None,
                    });
                }
            }
            return completions;
        }

        // Check if we're completing a session command
        if let Some(prefix) = line.strip_prefix("\\s ") {
            // Skip "\s "
            let arg_word_start = prefix[..pos.saturating_sub(3)]
                .rfind(char::is_whitespace)
                .map_or(0, |idx| idx + 1)
                + 3;
            let current_arg_word = &line[arg_word_start..pos];

            let sessions = self.get_saved_sessions();
            for (name, session) in sessions {
                if name.starts_with(current_arg_word) {
                    completions.push(Suggestion {
                        value: name.clone(),
                        description: Some(format!(
                            "Session: {}@{}:{}/{}",
                            session.user, session.host, session.port, session.dbname
                        )),
                        span: Span {
                            start: arg_word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: None,
                    });
                }
            }
            return completions;
        }

        // Check if we're completing a session deletion command
        if let Some(prefix) = line.strip_prefix("\\sd ") {
            // Skip "\sd "
            let arg_word_start = prefix[..pos.saturating_sub(4)]
                .rfind(char::is_whitespace)
                .map_or(0, |idx| idx + 1)
                + 4;
            let current_arg_word = &line[arg_word_start..pos];

            let sessions = self.get_saved_sessions();
            for (name, _) in sessions {
                if name.starts_with(current_arg_word) {
                    completions.push(Suggestion {
                        value: name.clone(),
                        description: Some("Session to delete".to_string()),
                        span: Span {
                            start: arg_word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: None,
                    });
                }
            }
            return completions;
        }

        // NEW LOGIC for column and tables-in-schema completion:
        if word_start > 0 && line.chars().nth(word_start - 1) == Some('.') {
            let dot_position = word_start - 1; // Position of the dot

            let mut identifier_start_before_dot = 0;
            if dot_position > 0 {
                for (i, char_code) in line[..dot_position].char_indices().rev() {
                    if !char_code.is_alphanumeric() && char_code != '_' && char_code != '.' {
                        identifier_start_before_dot = i + 1;
                        break;
                    }
                    if i == 0 {
                        identifier_start_before_dot = 0;
                        break;
                    }
                }
            }

            let object_name_before_dot = &line[identifier_start_before_dot..dot_position];
            let partial_item_after_dot = current_word;

            if !object_name_before_dot.is_empty()
                && !object_name_before_dot.starts_with('.')
                && !object_name_before_dot.ends_with('.')
            {
                // Attempt 1: Complete as columns of object_name_before_dot
                let columns = self.fetch_columns(object_name_before_dot);
                for col in &columns {
                    if col
                        .to_lowercase()
                        .starts_with(&partial_item_after_dot.to_lowercase())
                    {
                        completions.push(Suggestion {
                            value: col.clone(),
                            description: Some("Column".to_string()),
                            span: Span {
                                start: word_start,
                                end: pos,
                            },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Magenta)),
                        });
                    }
                }

                // Attempt 2: Complete as tables/views within object_name_before_dot (treating it as a schema)
                let tables_in_schema = self.fetch_tables(object_name_before_dot);
                for tbl in &tables_in_schema {
                    if tbl
                        .to_lowercase()
                        .starts_with(&partial_item_after_dot.to_lowercase())
                    {
                        completions.push(Suggestion {
                            value: tbl.clone(),
                            description: Some("Table/View in schema".to_string()),
                            span: Span {
                                start: word_start,
                                end: pos,
                            },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Green)),
                        });
                    }
                }

                // If any suggestions were found in this dot-context (columns or tables-in-schema), return them.
                if !completions.is_empty() {
                    return completions;
                }
            }
        }

        // If not a special command or (dot-context completion that found something), proceed with SQL completion
        // This part handles keywords, tables, schemas, functions
        let lower_current_word = current_word.to_lowercase();

        // Suggest schemas if the current word might be a schema - prioritize before keywords
        let schemas = self.fetch_schemas();
        for schema in &schemas {
            if schema.to_lowercase().starts_with(&lower_current_word) {
                completions.push(Suggestion {
                    value: schema.clone(),
                    description: Some("Schema".to_string()),
                    span: Span {
                        start: word_start,
                        end: pos,
                    },
                    append_whitespace: false,
                    extra: None,
                    style: Some(Style::new().fg(Color::Yellow)),
                });
            }
        }

        // Suggest tables and views - REVISED LOGIC
        if lower_current_word.contains('.') {
            // Case 1: current_word is like "schema.table_prefix"
            let parts: Vec<&str> = lower_current_word.splitn(2, '.').collect();
            let schema_context_str = parts[0];
            let table_prefix_str = parts[1];

            let tables_in_specific_schema = self.fetch_tables(schema_context_str);
            for item in tables_in_specific_schema {
                if item.to_lowercase().starts_with(table_prefix_str) {
                    let suggestion_span_start = word_start + schema_context_str.len() + 1;
                    completions.push(Suggestion {
                        value: item.clone(),
                        description: Some(format!("Table/View in {schema_context_str}")),
                        span: Span {
                            start: suggestion_span_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Green)),
                    });
                }
            }
        } else {
            // Case 2: current_word is like "table_prefix" (e.g. "data_") or empty.
            // Suggest tables from all accessible schemas.
            let all_tables_from_all_schemas = self.fetch_tables("");
            for table_name in all_tables_from_all_schemas {
                if table_name.to_lowercase().starts_with(&lower_current_word) {
                    completions.push(Suggestion {
                        value: table_name.clone(),
                        description: Some("Table/View".to_string()),
                        span: Span {
                            start: word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Green)),
                    });
                }
            }
        }
        // END OF REVISED TABLE SUGGESTION LOGIC

        // If we're at the beginning of the line or after a space, suggest keywords, tables, schemas, and functions
        if word_start == 0
            || line
                .chars()
                .nth(word_start - 1)
                .unwrap_or(' ')
                .is_whitespace()
        {
            // Suggest functions first (consider schema context similar to tables if needed)
            let functions = self.fetch_functions("public");
            for func in functions {
                if func.to_lowercase().starts_with(&lower_current_word) {
                    completions.push(Suggestion {
                        value: func.clone(),
                        description: Some("Function".to_string()),
                        span: Span {
                            start: word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Cyan)),
                    });
                }
            }

            // Suggest SQL keywords after tables and functions
            for keyword in &self.sql_keywords {
                if keyword.to_lowercase().starts_with(&lower_current_word) {
                    completions.push(Suggestion {
                        value: keyword.clone(),
                        description: Some("SQL Keyword".to_string()),
                        span: Span {
                            start: word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Blue)),
                    });
                }
            }
        }

        // Suggest named queries if applicable (e.g. SELECT * FROM my_query)
        let named_queries = self.get_named_queries();
        for (name, query) in named_queries {
            if name
                .to_lowercase()
                .starts_with(&current_word.to_lowercase())
            {
                completions.push(Suggestion {
                    value: name.clone(),
                    description: Some(format!("Named query: {query}")),
                    span: Span {
                        start: word_start,
                        end: pos,
                    },
                    append_whitespace: true,
                    extra: None,
                    style: Some(Style::new().fg(Color::Cyan)),
                });
            }
        }

        // Sort suggestions by type priority (Tables/Views first, then Schemas, Functions, then Keywords)
        // and secondarily by alphabetical order
        completions.sort_by(|a, b| {
            let type_priority = |suggestion: &Suggestion| -> i32 {
                match suggestion.description.as_deref() {
                    Some("Table/View") => 1,
                    Some(desc) if desc.starts_with("Table/View") => 1,
                    Some("Column") => 2,
                    Some("Schema") => 3,
                    Some("Function") => 4,
                    Some("SQL Keyword") => 5,
                    _ => 6,
                }
            };
            
            let priority_a = type_priority(a);
            let priority_b = type_priority(b);
            
            if priority_a != priority_b {
                priority_a.cmp(&priority_b)
            } else {
                a.value.cmp(&b.value)
            }
        });
        
        // Deduplicate suggestions based on value, keeping the one with a description if possible
        completions.dedup_by(|a, b| {
            if a.value == b.value {
                if b.description.is_some() && a.description.is_none() {
                    *a = b.clone();
                }
                true
            } else {
                false
            }
        });

        completions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, SavedSession};
    use std::sync::{Arc, Mutex};

    async fn create_test_completer() -> SqlCompleter {
        // Use the new_for_test() constructor from Database
        let db_instance = crate::db::Database::new_for_test();
        let db_arc = Arc::new(Mutex::new(db_instance));

        let mut completer = SqlCompleter {
            database: db_arc,
            sql_keywords: vec![
                "SELECT",
                "FROM",
                "WHERE",
                "INSERT",
                "UPDATE",
                "DELETE",
                "DROP",
                "CREATE",
                "ALTER",
                "TABLE",
                "VIEW",
                "INDEX",
                "TRIGGER",
                "FUNCTION",
                "PROCEDURE",
                "SCHEMA",
                "DATABASE",
                "GROUP BY",
                "ORDER BY",
                "HAVING",
                "JOIN",
                "LEFT JOIN",
                "RIGHT JOIN",
                "INNER JOIN",
                "FULL JOIN",
                "CROSS JOIN",
                "UNION",
                "INTERSECT",
                "EXCEPT",
                "LIMIT",
                "OFFSET",
                "ASC",
                "DESC",
                "DISTINCT",
                "ALL",
                "IN",
                "BETWEEN",
                "LIKE",
                "ILIKE",
                "SIMILAR TO",
                "IS NULL",
                "IS NOT NULL",
                "AND",
                "OR",
                "NOT",
                "TRUE",
                "FALSE",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            config: Config::load(),
            schemas_cache: None,
            tables_cache: None,
            columns_cache: None,
            functions_cache: None,
            cache_last_updated: None,
            cache_ttl: Duration::from_secs(300),
            ssh_tunnel_cache_ttl: Duration::from_secs(1800),
            cached_for_dbname: None,
            cached_for_host: None,
            cached_for_port: None,
            batch_fetch_in_progress: Arc::new(AtomicBool::new(false)),
        };

        // Keep existing mock config data for named queries and sessions
        completer
            .config
            .named_queries
            .insert("my_users".to_string(), "SELECT * FROM users".to_string());
        completer
            .config
            .named_queries
            .insert("my_orders".to_string(), "SELECT * FROM orders".to_string());
        let session_details = SavedSession {
            host: "localhost".to_string(),
            port: 5432,
            user: "test".to_string(),
            dbname: "testdb".to_string(),
            ssh_tunnel: None,
            database_type: crate::database::DatabaseType::PostgreSQL,
            file_path: None,
            options: std::collections::HashMap::new(),
        };
        // Set up connection params and save session
        completer.config.connection.host = session_details.host;
        completer.config.connection.port = session_details.port;
        completer.config.connection.user = session_details.user;
        completer.config.connection.dbname = session_details.dbname;
        completer.config.save_session("dev_session").unwrap();
        completer
    }

    #[tokio::test]
    async fn test_complete_empty_line() {
        let mut completer = create_test_completer().await;
        let suggestions = completer.complete("", 0);
        assert!(suggestions.is_empty());
    }

    #[tokio::test]
    async fn test_complete_backslash_only() {
        let mut completer = create_test_completer().await;
        let suggestions = completer.complete("\\", 1);
        assert_eq!(suggestions.len(), 38); // Should suggest all backslash commands from new enum system
        assert!(suggestions.iter().any(|s| s.value == "\\q"));
        assert!(suggestions.iter().any(|s| s.value == "\\dt"));
        assert!(suggestions.iter().any(|s| s.value == "\\h"));
        assert!(suggestions.iter().any(|s| s.value == "\\l"));
        assert!(suggestions.iter().any(|s| s.value == "\\d"));
        assert!(suggestions.iter().any(|s| s.value == "\\c"));
        assert!(suggestions.iter().any(|s| s.value == "\\x"));
        assert!(suggestions.iter().any(|s| s.value == "\\e"));
        assert!(suggestions.iter().any(|s| s.value == "\\w"));
        assert!(suggestions.iter().any(|s| s.value == "\\i"));
        assert!(suggestions.iter().any(|s| s.value == "\\ed"));
    }

    #[tokio::test]
    async fn test_complete_backslash_partial() {
        let mut completer = create_test_completer().await;
        let suggestions = completer.complete("\\d", 2);
        // Expect \d, \dt. Not \nd, \sd as they don't start with 'd' after the slash.
        assert!(suggestions.iter().any(|s| s.value == "\\d"));
        assert!(suggestions.iter().any(|s| s.value == "\\dt"));
        assert!(
            !suggestions.iter().any(|s| s.value == "\\nd"),
            "\\nd should not be suggested for input \\d"
        );
        assert!(
            !suggestions.iter().any(|s| s.value == "\\sd"),
            "\\sd should not be suggested for input \\d"
        );
        assert!(!suggestions.iter().any(|s| s.value == "\\q")); // Should not suggest \q for \d
    }

    #[tokio::test]
    async fn test_complete_backslash_dt() {
        let mut completer = create_test_completer().await;
        let suggestions = completer.complete("\\dt", 3);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "\\dt");
        assert_eq!(
            suggestions[0].description,
            Some("List tables".to_string())
        );
    }

    #[tokio::test]
    async fn test_complete_backslash_with_space_no_further_suggestion() {
        let mut completer = create_test_completer().await;
        // \l command does not take arguments, so after space, no more \ command suggestions
        let suggestions = completer.complete("\\l ", 3);
        assert!(suggestions.is_empty());
    }

    #[tokio::test]
    async fn test_complete_backslash_c_with_space_suggests_databases() {
        let mut completer = create_test_completer().await;
        let line = "\\c ";
        let suggestions = completer.complete(line, line.len());
        assert!(
            suggestions.iter().any(|s| s.value == "main_db"),
            "Should suggest main_db. Got: {suggestions:?}"
        );
        assert!(
            suggestions.iter().any(|s| s.value == "test_db"),
            "Should suggest test_db. Got: {suggestions:?}"
        );
    }

    #[tokio::test]
    async fn test_complete_backslash_c_partial_db() {
        let mut completer = create_test_completer().await;
        let line = "\\c main";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "main_db"));
        assert!(!suggestions.iter().any(|s| s.value == "test_db"));
    }

    #[tokio::test]
    async fn test_complete_backslash_d_with_space_suggests_tables_only() {
        let mut completer = create_test_completer().await;
        let line = "\\d ";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "users"));
        assert!(suggestions.iter().any(|s| s.value == "orders"));
        // Schema prefixes should not be suggested for \d command
        assert!(!suggestions.iter().any(|s| s.value == "public."));
        assert!(!suggestions.iter().any(|s| s.value == "custom_schema."));
    }

    #[tokio::test]
    async fn test_complete_backslash_d_partial_table() {
        let mut completer = create_test_completer().await;
        let line = "\\d us";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "users"));
        assert!(!suggestions.iter().any(|s| s.value == "orders"));
    }

    // NOTE: These tests are commented out because the commands (\n, \nd, \s, \sd) are not 
    // fully implemented in the enum-based command system yet. The completion logic for these 
    // commands exists but the actual command handlers need to be added to the Command enum.
    
    #[tokio::test]
    async fn test_complete_named_query_execution() {
        let mut completer = create_test_completer().await;
        let line = "\\n my";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "my_users"));
        assert!(suggestions.iter().any(|s| s.value == "my_orders"));
    }

    #[tokio::test]
    async fn test_complete_named_query_delete() {
        let mut completer = create_test_completer().await;
        let line = "\\nd my_u";
        let suggestions = completer.complete(line, line.len());
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "my_users");
    }

    // #[tokio::test]
    // async fn test_complete_session_connect() {
    //     let mut completer = create_test_completer().await;
    //     let line = "\\s dev";
    //     let suggestions = completer.complete(line, line.len());
    //     assert_eq!(suggestions.len(), 1);
    //     assert_eq!(suggestions[0].value, "dev_session");
    // }

    #[tokio::test]
    async fn test_complete_select_keyword() {
        let mut completer = create_test_completer().await;
        let line = "SEL";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "SELECT"));
    }

    #[tokio::test]
    async fn test_complete_table_after_from() {
        let mut completer = create_test_completer().await;
        let line = "SELECT * FROM us";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "users"));
        assert!(!suggestions.iter().any(|s| s.value == "orders")); // "us" matches "users"
    }

    #[tokio::test]
    async fn test_complete_column_after_dot() {
        let mut completer = create_test_completer().await;
        let line = "SELECT users.";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "id"));
        assert!(suggestions.iter().any(|s| s.value == "name"));
        assert!(suggestions.iter().any(|s| s.value == "email"));
    }

    #[tokio::test]
    async fn test_complete_column_after_dot_partial() {
        let mut completer = create_test_completer().await;
        let line = "SELECT users.na";
        let suggestions = completer.complete(line, line.len());
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "name");
    }

    #[tokio::test]
    async fn test_complete_named_query_as_table() {
        let mut completer = create_test_completer().await;
        let line = "SELECT * FROM my_u";
        let suggestions = completer.complete(line, line.len());
        assert!(suggestions.iter().any(|s| s.value == "my_users"));
    }
}
