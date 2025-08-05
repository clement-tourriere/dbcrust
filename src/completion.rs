//! Enhanced SQL autocompletion system
//! Complete rewrite with proper SQL parsing and context awareness

use crate::command_completion::CommandCompletionManager;
use crate::commands::CommandParser;
use crate::completion_provider::TableInfo;
use crate::config::Config;
use crate::database::DatabaseType;
use crate::db::Database;
use crate::sql_parser::{SqlContext, ExpectedElement, SqlClause};
use crate::sql_parser_trait::{SqlParserEngine, SqlParserFactory, EnhancedSqlContext, CompletionHintCategory};
use nu_ansi_term::{Color, Style};
use reedline::{Completer, Span, Suggestion};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{debug, error};

/// No-op completer when autocomplete is disabled
pub struct NoopCompleter {}

impl Completer for NoopCompleter {
    fn complete(&mut self, _line: &str, _pos: usize) -> Vec<Suggestion> {
        Vec::new()
    }
}


/// Enhanced SQL completer with proper parsing and context awareness
pub struct SqlCompleter {
    database: Arc<Mutex<Database>>,
    config: Arc<Mutex<Config>>,
    command_manager: CommandCompletionManager,
    /// Cache for schemas
    schema_cache: Option<Vec<String>>,
    /// Cache for tables by schema
    table_cache: HashMap<String, Vec<TableInfo>>,
    /// Cache for columns by table
    column_cache: HashMap<String, Vec<String>>,
    /// Last database name for cache invalidation
    last_db_name: Option<String>,
}

/// FROM clause completion states
#[derive(Debug, PartialEq)]
enum FromClauseState {
    /// Right after FROM keyword - expecting table name
    ExpectingTable,
    /// Partially typing a table name (e.g., "cat" → "categories")
    TypingTable,
    /// Complete table specified - expecting keywords (WHERE, JOIN, etc.)
    AfterTable,
    /// Partially typing a keyword after table (e.g., "wh" → "WHERE")
    TypingKeyword,
}

impl SqlCompleter {
    pub fn new(database: Arc<Mutex<Database>>, config: Arc<Mutex<Config>>) -> Self {
        let command_manager = CommandCompletionManager::new(
            Arc::clone(&database),
            Arc::clone(&config),
        );
        
        Self {
            database,
            config,
            command_manager,
            schema_cache: None,
            table_cache: HashMap::new(),
            column_cache: HashMap::new(),
            last_db_name: None,
        }
    }

    /// Clear all caches
    pub fn clear_cache(&mut self) {
        self.schema_cache = None;
        self.table_cache.clear();
        self.column_cache.clear();
        debug!("SqlCompleter cache cleared");
    }

    /// Check if cache needs invalidation
    fn check_cache_validity(&mut self) {
        let (current_db, has_connection) = {
            let db_guard = self.database.lock().unwrap();
            (db_guard.get_current_db(), db_guard.has_database_connection())
        };

        debug!("[SqlCompleter] Cache validity check: db='{}', has_connection={}", 
               current_db, has_connection);

        if self.last_db_name.as_ref() != Some(&current_db) {
            debug!("[SqlCompleter] Database changed from {:?} to {}, clearing cache", 
                   self.last_db_name, current_db);
            self.clear_cache();
            self.last_db_name = Some(current_db);
        }

        if !has_connection {
            debug!("[SqlCompleter] No database connection available, completion may be limited");
        }
    }

    /// Get schemas (with caching)
    #[allow(dead_code)]
    fn get_schemas(&mut self) -> Vec<String> {
        if let Some(ref schemas) = self.schema_cache {
            return schemas.clone();
        }

        let db_clone = Arc::clone(&self.database);
        let schemas = match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    handle.block_on(async {
                        let mut db_guard = db_clone.lock().unwrap();
                        db_guard.get_schemas().await.unwrap_or_default()
                    })
                })
            }
            Err(_) => {
                error!("No tokio runtime for schema fetch");
                vec![]
            }
        };

        self.schema_cache = Some(schemas.clone());
        schemas
    }

    /// Get tables for a schema (with caching)
    fn get_tables(&mut self, schema: Option<&str>) -> Vec<TableInfo> {
        let cache_key = schema.unwrap_or("").to_string();
        
        if let Some(tables) = self.table_cache.get(&cache_key) {
            return tables.clone();
        }

        let db_clone = Arc::clone(&self.database);
        let schema_owned = schema.map(|s| s.to_string());
        
        let tables = match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    handle.block_on(async {
                        let mut db_guard = db_clone.lock().unwrap();
                        
                        // Get table names
                        let table_names = if let Some(client) = db_guard.get_database_client() {
                            client.get_metadata_provider()
                                .get_tables(schema_owned.as_deref())
                                .await
                                .unwrap_or_default()
                        } else {
                            db_guard.get_tables_and_views(schema_owned.as_deref())
                                .await
                                .unwrap_or_default()
                        };
                        
                        // Convert to TableInfo
                        table_names.into_iter()
                            .map(|name| TableInfo {
                                schema: schema_owned.clone(),
                                name,
                                table_type: crate::completion_provider::TableType::Table,
                            })
                            .collect()
                    })
                })
            }
            Err(_) => {
                error!("No tokio runtime for table fetch");
                vec![]
            }
        };

        self.table_cache.insert(cache_key, tables.clone());
        tables
    }

    /// Get the database type from the connection info
    fn get_database_type(&self) -> DatabaseType {
        let db_guard = self.database.lock().unwrap();
        if let Some(connection_info) = db_guard.get_connection_info() {
            connection_info.database_type.clone()
        } else {
            // Default to PostgreSQL if we can't determine the type
            DatabaseType::PostgreSQL
        }
    }

    /// Analyze the current state in FROM clause for accurate completion
    fn analyze_from_clause_state(&self, sql: &str, cursor_pos: usize, context: &SqlContext, current_word: &str) -> FromClauseState {
        debug!("[SqlCompleter] Analyzing FROM clause state: sql='{}', cursor_pos={}, current_word='{}'", 
               sql, cursor_pos, current_word);
               
        // If no tables parsed yet, we're expecting/typing a table name
        if context.tables.is_empty() {
            if current_word.is_empty() {
                return FromClauseState::ExpectingTable;
            } else {
                return FromClauseState::TypingTable;
            }
        }
        
        // We have parsed tables - check if cursor is after a complete table reference
        for table_ref in &context.tables {
            let table_end_pos = table_ref.position + table_ref.table.len();
            
            // Add alias length if present
            let total_table_ref_end = if let Some(alias) = &table_ref.alias {
                // Account for "table alias" pattern (table + space + alias)
                table_end_pos + 1 + alias.len()
            } else {
                table_end_pos
            };
            
            debug!("[SqlCompleter] Table '{}' ends at position {}, cursor at {}", 
                   table_ref.table, total_table_ref_end, cursor_pos);
            
            // If cursor is after this table reference
            if cursor_pos > total_table_ref_end {
                // Check if we're typing a keyword
                if !current_word.is_empty() {
                    // Check if current word could be a SQL keyword
                    if self.could_be_sql_keyword(current_word) {
                        return FromClauseState::TypingKeyword;
                    }
                }
                return FromClauseState::AfterTable;
            }
        }
        
        // Fallback: if we have tables but cursor position analysis failed
        if current_word.is_empty() {
            FromClauseState::AfterTable
        } else if self.could_be_sql_keyword(current_word) {
            FromClauseState::TypingKeyword
        } else {
            FromClauseState::TypingTable
        }
    }
    
    /// Check if a partial word could be a SQL keyword
    fn could_be_sql_keyword(&self, word: &str) -> bool {
        let upper_word = word.to_uppercase();
        let sql_keywords = [
            "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "CROSS",
            "ON", "USING", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET",
            "UNION", "INTERSECT", "EXCEPT", "AND", "OR", "NOT"
        ];
        
        // Check if any SQL keyword starts with this word
        sql_keywords.iter().any(|keyword| keyword.starts_with(&upper_word))
    }


    /// Get columns for a table (with caching)
    fn get_columns(&mut self, table: &str) -> Vec<String> {
        debug!("[SqlCompleter] get_columns for table: '{}'", table);
        
        if let Some(columns) = self.column_cache.get(table) {
            debug!("[SqlCompleter] Cache hit! Returning {} columns", columns.len());
            return columns.clone();
        }

        debug!("[SqlCompleter] Cache miss, fetching columns from database");
        let db_clone = Arc::clone(&self.database);
        let table_owned = table.to_string();
        
        // Check database connection first
        let has_connection = {
            let db_guard = db_clone.lock().unwrap();
            db_guard.has_database_connection()
        };

        let columns = if !has_connection {
            debug!("[SqlCompleter] No database connection, using empty column list");
            vec![]
        } else {
            match tokio::runtime::Handle::try_current() {
                Ok(_) => {
                    tokio::task::block_in_place(|| {
                        let handle = tokio::runtime::Handle::current();
                        handle.block_on(async {
                            let mut db_guard = db_clone.lock().unwrap();
                            match db_guard.get_columns(&table_owned).await {
                                Ok(cols) => {
                                    debug!("[SqlCompleter] Successfully fetched {} columns: {:?}", 
                                           cols.len(), cols);
                                    cols
                                }
                                Err(e) => {
                                    error!("[SqlCompleter] Failed to fetch columns for '{}': {}", 
                                           table_owned, e);
                                    // Return empty list on error, don't crash
                                    vec![]
                                }
                            }
                        })
                    })
                }
                Err(_) => {
                    error!("[SqlCompleter] No tokio runtime for column fetch");
                    vec![]
                }
            }
        };

        debug!("[SqlCompleter] Caching {} columns for table '{}'", columns.len(), table);
        self.column_cache.insert(table.to_string(), columns.clone());
        columns
    }


    /// Complete backslash commands using the new trait-based system
    fn complete_backslash_commands(&self, line: &str, pos: usize) -> Vec<Suggestion> {
        // Parse the command line to determine if we're completing command name or arguments
        if let Some((command, args, args_pos)) = self.command_manager.parse_command_line(line, pos) {
            if pos <= command.len() {
                // Completing command name (cursor is still within the command itself)
                let word_start = line[..pos].rfind(' ').map_or(0, |idx| idx + 1);
                let current_word = &line[word_start..pos];
                return self.command_manager.get_command_completions(current_word, word_start, pos);
            } else {
                // Completing command arguments using tokio runtime
                let argument_completions = match tokio::runtime::Handle::try_current() {
                    Ok(_) => {
                        tokio::task::block_in_place(|| {
                            let handle = tokio::runtime::Handle::current();
                            handle.block_on(async {
                                self.command_manager.get_argument_completions(&command, &args, args_pos).await
                            })
                        })
                    }
                    Err(_) => {
                        debug!("No tokio runtime for command argument completion");
                        Vec::new()
                    }
                };
                
                // Return argument completions (even if empty - don't fall back to command completions)
                return argument_completions;
            }
        }
        
        // Fallback to old behavior if parsing fails
        let mut completions = Vec::new();
        let word_start = line[..pos].rfind(' ').map_or(0, |idx| idx + 1);
        let current_word = &line[word_start..pos];

        // Get basic command completions
        for (_category, commands) in CommandParser::get_commands_by_category() {
            for (cmd_name, cmd_description) in commands {
                if cmd_name.starts_with(current_word) {
                    completions.push(Suggestion {
                        value: cmd_name.to_string(),
                        description: Some(cmd_description.to_string()),
                        span: Span { start: word_start, end: pos },
                        append_whitespace: true,
                        extra: None,
                        style: None,
                    });
                }
            }
        }

        completions
    }

    /// Get enhanced SQL keywords based on context using database-specific parser
    fn get_enhanced_contextual_keywords(&self, context: &EnhancedSqlContext, parser: &Box<dyn SqlParserEngine>) -> Vec<&'static str> {
        // Use database-specific keywords based on the current clause, but prioritize basic SQL structure
        use crate::sql_parser_trait::KeywordCategory;
        
        match context.base_context.current_clause {
            SqlClause::Select => {
                // For SELECT, prioritize structural keywords, then add functions
                let mut keywords = vec!["FROM", "WHERE", "GROUP", "ORDER", "LIMIT", "UNION", "DISTINCT", "*"];
                let functions = parser.get_keywords_by_category(KeywordCategory::Functions);
                keywords.extend(functions);
                keywords
            },
            SqlClause::From => {
                // Only suggest keywords after a table has been specified
                vec!["WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "ORDER", "GROUP"]
            },
            SqlClause::Where => {
                let mut keywords = vec!["AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "IS", "NULL", "ORDER", "GROUP"];
                let operators = parser.get_keywords_by_category(KeywordCategory::Operators);
                keywords.extend(operators);
                keywords
            },
            SqlClause::Join => vec!["ON", "USING"],
            SqlClause::On => {
                let mut keywords = vec!["AND", "OR"];
                let operators = parser.get_keywords_by_category(KeywordCategory::Operators);
                keywords.extend(operators);
                keywords
            },
            SqlClause::OrderBy => vec!["ASC", "DESC", "LIMIT"],
            SqlClause::GroupBy => vec!["HAVING", "ORDER"],
            SqlClause::Having => {
                let mut keywords = vec!["AND", "OR", "ORDER"];
                let operators = parser.get_keywords_by_category(KeywordCategory::Operators);
                keywords.extend(operators);
                keywords
            },
            SqlClause::Update => vec!["SET"],
            SqlClause::UpdateSet => vec!["WHERE"],
            SqlClause::Insert => vec!["INTO", "VALUES"],
            SqlClause::Delete => vec!["FROM"],
            _ => {
                let mut keywords = vec!["SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP"];
                let dml = parser.get_keywords_by_category(KeywordCategory::DML);
                keywords.extend(dml);
                keywords
            },
        }
    }


    /// Generate suggestions based on enhanced SQL context with database-specific parsing
    fn generate_enhanced_sql_suggestions(
        &mut self,
        context: &EnhancedSqlContext,
        parser: &Box<dyn SqlParserEngine>,
        current_word: &str,
        word_start: usize,
        pos: usize,
        sql: &str,
    ) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();
        let lower_word = current_word.to_lowercase();

        // PRIORITY 1: Columns first in WHERE clause, then handle specific context logic
        let mut columns_added = false;
        
        // Add columns first if we're in WHERE clause
        if context.base_context.current_clause == SqlClause::Where {
            debug!("[SqlCompleter] WHERE clause: prioritizing columns over keywords");
            for table_ref in &context.base_context.tables {
                debug!("[SqlCompleter] Fetching columns for table: {}", table_ref.table);
                let columns = self.get_columns(&table_ref.table);
                debug!("[SqlCompleter] Got {} columns from {}", columns.len(), table_ref.table);
                
                // If user typed table prefix (e.g., "users.")
                if current_word.contains('.') {
                    let parts: Vec<&str> = current_word.splitn(2, '.').collect();
                    if parts.len() == 2 {
                        let table_prefix = parts[0];
                        let column_prefix = parts[1];
                        
                        debug!("[SqlCompleter] Table-qualified column completion: {}.{}", 
                               table_prefix, column_prefix);
                        
                        // Check if this table matches
                        let matches = table_ref.alias.as_ref()
                            .map(|a| a == table_prefix)
                            .unwrap_or(false) || table_ref.table == table_prefix;
                        
                        debug!("[SqlCompleter] Table match for '{}': {}", table_prefix, matches);
                        
                        if matches {
                            let mut added_count = 0;
                            for column in columns {
                                if column.to_lowercase().starts_with(&column_prefix.to_lowercase()) {
                                    suggestions.push(Suggestion {
                                        value: format!("{}.{}", table_prefix, column),
                                        description: Some(format!("Column from {}", table_ref.table)),
                                        span: Span { start: word_start, end: pos },
                                        append_whitespace: true,
                                        extra: None,
                                        style: Some(Style::new().fg(Color::Cyan)),
                                    });
                                    added_count += 1;
                                    columns_added = true;
                                }
                            }
                            debug!("[SqlCompleter] Added {} qualified column suggestions", added_count);
                        }
                    }
                } else {
                    // No table prefix, suggest all columns
                    debug!("[SqlCompleter] Unqualified column completion, filtering with: '{}'", lower_word);
                    let mut added_count = 0;
                    
                    for column in columns {
                        if lower_word.is_empty() || column.to_lowercase().starts_with(&lower_word) {
                            let desc = if let Some(alias) = &table_ref.alias {
                                format!("Column from {} ({})", alias, table_ref.table)
                            } else {
                                format!("Column from {}", table_ref.table)
                            };
                            
                            debug!("[SqlCompleter] Adding column suggestion: {} -> {}", column, desc);
                            suggestions.push(Suggestion {
                                value: column,
                                description: Some(desc),
                                span: Span { start: word_start, end: pos },
                                append_whitespace: true,
                                extra: None,
                                style: Some(Style::new().fg(Color::Cyan)),
                            });
                            added_count += 1;
                            columns_added = true;
                        }
                    }
                    debug!("[SqlCompleter] Added {} unqualified column suggestions from {}", 
                           added_count, table_ref.table);
                }
            }
        }

        // PRIORITY 2: Context-specific logic - handle different SQL clauses
        match context.base_context.current_clause {
            SqlClause::From => {
                // Use the enhanced FROM clause state machine
                let state = self.analyze_from_clause_state(sql, pos, &context.base_context, current_word);
                debug!("[SqlCompleter] FROM clause state: {:?}", state);
                
                match state {
                    FromClauseState::ExpectingTable | FromClauseState::TypingTable => {
                        // Only show tables, no keywords
                        debug!("[SqlCompleter] FROM clause: showing tables only");
                        let tables = self.get_tables(None);
                        for table in tables {
                            if table.name.to_lowercase().starts_with(&lower_word) {
                                suggestions.push(Suggestion {
                                    value: table.name.clone(),
                                    description: Some("Table".to_string()),
                                    span: Span { start: word_start, end: pos },
                                    append_whitespace: true,
                                    extra: None,
                                    style: Some(Style::new().fg(Color::Green)),
                                });
                            }
                        }
                    },
                    FromClauseState::AfterTable | FromClauseState::TypingKeyword => {
                        // After table name, show JOIN/WHERE keywords only
                        debug!("[SqlCompleter] FROM clause: showing keywords only");
                        let from_keywords = vec!["WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "ORDER", "GROUP", "LIMIT"];
                        for keyword in from_keywords {
                            if keyword.to_lowercase().starts_with(&lower_word) {
                                suggestions.push(Suggestion {
                                    value: keyword.to_string(),
                                    description: Some("SQL Keyword".to_string()),
                                    span: Span { start: word_start, end: pos },
                                    append_whitespace: true,
                                    extra: None,
                                    style: Some(Style::new().fg(Color::Blue)),
                                });
                            }
                        }
                    }
                }
            }
            SqlClause::Where => {
                // Add WHERE keywords after columns (columns already added above)
                debug!("[SqlCompleter] WHERE clause: adding keywords after columns");
                let where_keywords = vec!["AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "IS", "NULL", "ORDER", "GROUP"];
                for keyword in where_keywords {
                    if keyword.to_lowercase().starts_with(&lower_word) {
                        suggestions.push(Suggestion {
                            value: keyword.to_string(),
                            description: Some("SQL Keyword".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Blue)),
                        });
                    }
                }
            }
            SqlClause::Select => {
                // For SELECT, add structural keywords
                let select_keywords = vec!["FROM", "WHERE", "GROUP", "ORDER", "LIMIT", "UNION", "INTERSECT", "EXCEPT"];
                for keyword in select_keywords {
                    if keyword.to_lowercase().starts_with(&lower_word) {
                        suggestions.push(Suggestion {
                            value: keyword.to_string(),
                            description: Some("SQL Clause".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Blue)),
                        });
                    }
                }
            }
            _ => {
                // For other clauses, add contextual keywords
                let basic_keywords = self.get_enhanced_contextual_keywords(context, parser);
                for keyword in basic_keywords {
                    if keyword.to_lowercase().starts_with(&lower_word) && !keyword.is_empty() {
                        suggestions.push(Suggestion {
                            value: keyword.to_string(),
                            description: Some("SQL Keyword".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Blue)),
                        });
                    }
                }
            }
        }

        // Process each expected element type (using base context for compatibility)
        for expected in &context.base_context.expecting {
            match expected {
                ExpectedElement::Table => {
                    // Get all tables
                    let tables = self.get_tables(None);
                    for table in tables {
                        if table.name.to_lowercase().starts_with(&lower_word) {
                            suggestions.push(Suggestion {
                                value: table.name.clone(),
                                description: Some("Table".to_string()),
                                span: Span { start: word_start, end: pos },
                                append_whitespace: true,
                                extra: None,
                                style: Some(Style::new().fg(Color::Green)),
                            });
                        }
                    }
                }
                ExpectedElement::Column => {
                    // Skip if we already added columns for WHERE clause
                    if context.base_context.current_clause == SqlClause::Where && columns_added {
                        debug!("[SqlCompleter] Skipping duplicate column processing for WHERE clause");
                        continue;
                    }
                    
                    debug!("[SqlCompleter] Processing Column suggestions for non-WHERE clause");
                    // Get columns from tables in context
                    for table_ref in &context.base_context.tables {
                        debug!("[SqlCompleter] Fetching columns for table: {}", table_ref.table);
                        let columns = self.get_columns(&table_ref.table);
                        debug!("[SqlCompleter] Got {} columns from {}", columns.len(), table_ref.table);
                        
                        // If user typed table prefix (e.g., "users.")
                        if current_word.contains('.') {
                            let parts: Vec<&str> = current_word.splitn(2, '.').collect();
                            if parts.len() == 2 {
                                let table_prefix = parts[0];
                                let column_prefix = parts[1];
                                
                                debug!("[SqlCompleter] Table-qualified column completion: {}.{}", 
                                       table_prefix, column_prefix);
                                
                                // Check if this table matches
                                let matches = table_ref.alias.as_ref()
                                    .map(|a| a == table_prefix)
                                    .unwrap_or(false) || table_ref.table == table_prefix;
                                
                                debug!("[SqlCompleter] Table match for '{}': {}", table_prefix, matches);
                                
                                if matches {
                                    let mut added_count = 0;
                                    for column in columns {
                                        if column.to_lowercase().starts_with(&column_prefix.to_lowercase()) {
                                            suggestions.push(Suggestion {
                                                value: format!("{}.{}", table_prefix, column),
                                                description: Some(format!("Column from {}", table_ref.table)),
                                                span: Span { start: word_start, end: pos },
                                                append_whitespace: true,
                                                extra: None,
                                                style: Some(Style::new().fg(Color::Cyan)),
                                            });
                                            added_count += 1;
                                        }
                                    }
                                    debug!("[SqlCompleter] Added {} qualified column suggestions", added_count);
                                }
                            }
                        } else {
                            // No table prefix, suggest all columns
                            debug!("[SqlCompleter] Unqualified column completion, filtering with: '{}'", lower_word);
                            let mut added_count = 0;
                            let mut filtered_count = 0;
                            
                            if columns.is_empty() {
                                debug!("[SqlCompleter] WARNING: No columns available for table '{}'", table_ref.table);
                            }
                            
                            for column in columns {
                                if lower_word.is_empty() || column.to_lowercase().starts_with(&lower_word) {
                                    let desc = if let Some(alias) = &table_ref.alias {
                                        format!("Column from {} ({})", alias, table_ref.table)
                                    } else {
                                        format!("Column from {}", table_ref.table)
                                    };
                                    
                                    debug!("[SqlCompleter] Adding column suggestion: {} -> {}", column, desc);
                                    suggestions.push(Suggestion {
                                        value: column,
                                        description: Some(desc),
                                        span: Span { start: word_start, end: pos },
                                        append_whitespace: true,
                                        extra: None,
                                        style: Some(Style::new().fg(Color::Cyan)),
                                    });
                                    added_count += 1;
                                } else {
                                    filtered_count += 1;
                                }
                            }
                            debug!("[SqlCompleter] Added {} unqualified column suggestions from {} (filtered out {})", 
                                   added_count, table_ref.table, filtered_count);
                        }
                    }
                }
                ExpectedElement::Keyword(keywords) => {
                    for keyword in keywords {
                        if keyword.to_lowercase().starts_with(&lower_word) {
                            suggestions.push(Suggestion {
                                value: keyword.to_string(),
                                description: Some("SQL Keyword".to_string()),
                                span: Span { start: word_start, end: pos },
                                append_whitespace: true,
                                extra: None,
                                style: Some(Style::new().fg(Color::Blue)),
                            });
                        }
                    }
                }
                ExpectedElement::Function => {
                    // Use database-specific functions instead of hardcoded ones
                    let functions = parser.get_functions();
                    for func_name in functions {
                        if func_name.to_lowercase().starts_with(&lower_word) {
                            let requires_parens = parser.database_type() != DatabaseType::PostgreSQL || 
                                                 !matches!(func_name.to_uppercase().as_str(), 
                                                          "CURRENT_DATE" | "CURRENT_TIME" | "CURRENT_TIMESTAMP");
                            let display_name = if requires_parens && !func_name.ends_with('(') {
                                format!("{}(", func_name)
                            } else {
                                func_name.to_string()
                            };
                            
                            suggestions.push(Suggestion {
                                value: display_name,
                                description: Some(format!("{} function", func_name)),
                                span: Span { start: word_start, end: pos },
                                append_whitespace: !requires_parens,
                                extra: None,
                                style: Some(Style::new().fg(Color::Magenta)),
                            });
                        }
                    }
                }
                _ => {} // Value, Operator, Identifier handled elsewhere
            }
        }

        // PRIORITY 3: Get database-specific completion hints (lower priority)
        let hints = parser.get_completion_hints(context);
        
        // Convert hints to suggestions
        for hint in hints {
            if hint.text.to_lowercase().starts_with(&lower_word) {
                // Skip if we already have this suggestion
                if suggestions.iter().any(|s| s.value == hint.text) {
                    continue;
                }
                
                let style = match hint.category {
                    CompletionHintCategory::Keyword => Some(Style::new().fg(Color::Blue)),
                    CompletionHintCategory::Function => Some(Style::new().fg(Color::Magenta)),
                    CompletionHintCategory::Operator => Some(Style::new().fg(Color::Yellow)),
                    CompletionHintCategory::DataType => Some(Style::new().fg(Color::Green)),
                    CompletionHintCategory::DatabaseSpecific => Some(Style::new().fg(Color::Cyan)),
                    _ => Some(Style::new().fg(Color::White)),
                };
                
                suggestions.push(Suggestion {
                    value: hint.text,
                    description: Some(hint.description),
                    span: Span { start: word_start, end: pos },
                    append_whitespace: !hint.requires_parentheses,
                    extra: None,
                    style,
                });
            }
        }

        // PRIORITY 4: Get database-specific context suggestions
        let context_suggestions = parser.get_context_suggestions(context, current_word);
        for suggestion_text in context_suggestions {
            if !suggestions.iter().any(|s| s.value == suggestion_text) {
                suggestions.push(Suggestion {
                    value: suggestion_text.clone(),
                    description: Some("Database-specific suggestion".to_string()),
                    span: Span { start: word_start, end: pos },
                    append_whitespace: true,
                    extra: None,
                    style: Some(Style::new().fg(Color::Cyan)),
                });
            }
        }

        // Remove duplicates while preserving order
        let mut seen = HashSet::new();
        suggestions.retain(|s| seen.insert(s.value.clone()));

        suggestions
    }

}

impl Completer for SqlCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        debug!("Completion request: line='{}', pos={}", line, pos);

        // Check cache validity
        self.check_cache_validity();

        // Handle empty line
        if line.is_empty() && pos == 0 {
            return Vec::new();
        }

        // Simple direct pattern matching that WORKS
        if line.starts_with('\\') {
            // Direct pattern matching for common cases
            if line.starts_with("\\d ") && pos > 2 {
                // Complete table names directly
                let word_start = 3; // After "\d "
                let current_word = if pos > word_start {
                    &line[word_start..pos]
                } else {
                    ""
                };
                
                let tables = self.get_tables(None);
                let mut suggestions = Vec::new();
                for table in tables {
                    if table.name.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        suggestions.push(Suggestion {
                            value: table.name.clone(),
                            description: Some("Table".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Green)),
                        });
                    }
                }
                return suggestions;
            }
            
            if line.starts_with("\\c ") && pos > 2 {
                // Complete database names directly
                let word_start = 3; // After "\c "
                let current_word = if pos > word_start {
                    &line[word_start..pos]
                } else {
                    ""
                };
                
                // Get database names using the same pattern as get_tables
                let db_clone = Arc::clone(&self.database);
                let databases = match tokio::runtime::Handle::try_current() {
                    Ok(_) => {
                        tokio::task::block_in_place(|| {
                            let handle = tokio::runtime::Handle::current();
                            handle.block_on(async {
                                let mut db_guard = db_clone.lock().unwrap();
                                db_guard.list_databases().await.unwrap_or_default()
                            })
                        })
                    }
                    Err(_) => {
                        vec![]
                    }
                };
                
                let mut suggestions = Vec::new();
                for db_row in databases {
                    if let Some(db_name) = db_row.get(0) {
                        if db_name.to_lowercase().starts_with(&current_word.to_lowercase()) {
                            suggestions.push(Suggestion {
                                value: db_name.clone(),
                                description: Some("Database".to_string()),
                                span: Span { start: word_start, end: pos },
                                append_whitespace: true,
                                extra: None,
                                style: Some(Style::new().fg(Color::Blue)),
                            });
                        }
                    }
                }
                return suggestions;
            }
            
            if line.starts_with("\\n ") && pos > 2 {
                // Complete named query names
                let word_start = 3; // After "\n "
                let current_word = if pos > word_start {
                    &line[word_start..pos]
                } else {
                    ""
                };
                
                // Get named queries from config
                let config = self.config.lock().unwrap();
                let mut suggestions = Vec::new();
                for (query_name, _query) in &config.named_queries {
                    if query_name.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        suggestions.push(Suggestion {
                            value: query_name.clone(),
                            description: Some("Execute named query".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Magenta)),
                        });
                    }
                }
                return suggestions;
            }
            
            if line.starts_with("\\nd ") && pos > 3 {
                // Complete named query names for deletion
                let word_start = 4; // After "\nd "
                let current_word = if pos > word_start {
                    &line[word_start..pos]
                } else {
                    ""
                };
                
                // Get named queries from config
                let config = self.config.lock().unwrap();
                let mut suggestions = Vec::new();
                for (query_name, _query) in &config.named_queries {
                    if query_name.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        suggestions.push(Suggestion {
                            value: query_name.clone(),
                            description: Some("Delete named query".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: Some(Style::new().fg(Color::Red)),
                        });
                    }
                }
                return suggestions;
            }
            
            // For other backslash commands or when typing the command itself
            return self.complete_backslash_commands(line, pos);
        }

        // Fallback to existing SQL completion logic
        let full_line = line.to_string();
        
        // Check for SQL commands that need special completion
        if line.to_uppercase().starts_with("DROP ") && pos >= 5 {
            let word_start = 5; // After "DROP "
            let current_word = if pos > word_start {
                &line[word_start..pos]
            } else {
                ""
            };
            
            let drop_objects = vec![
                ("TABLE", "Drop a table"),
                ("DATABASE", "Drop a database"),
                ("INDEX", "Drop an index"),
                ("VIEW", "Drop a view"),
                ("SCHEMA", "Drop a schema"),
                ("FUNCTION", "Drop a function"),
                ("PROCEDURE", "Drop a procedure"),
                ("TRIGGER", "Drop a trigger"),
                ("SEQUENCE", "Drop a sequence"),
                ("TYPE", "Drop a type"),
                ("ROLE", "Drop a role"),
                ("USER", "Drop a user"),
            ];
            
            let mut suggestions = Vec::new();
            for (obj_type, desc) in drop_objects {
                if obj_type.starts_with(&current_word.to_uppercase()) {
                    suggestions.push(Suggestion {
                        value: obj_type.to_string(),
                        description: Some(desc.to_string()),
                        span: Span { start: word_start, end: pos },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Blue)),
                    });
                }
            }
            return suggestions;
        }
        
        if line.to_uppercase().starts_with("CREATE ") && pos >= 7 {
            let word_start = 7; // After "CREATE "
            let current_word = if pos > word_start {
                &line[word_start..pos]
            } else {
                ""
            };
            
            let create_objects = vec![
                ("TABLE", "Create a new table"),
                ("DATABASE", "Create a new database"),
                ("INDEX", "Create a new index"),
                ("VIEW", "Create a new view"),
                ("SCHEMA", "Create a new schema"),
                ("FUNCTION", "Create a new function"),
                ("PROCEDURE", "Create a new procedure"),
                ("TRIGGER", "Create a new trigger"),
                ("SEQUENCE", "Create a new sequence"),
                ("TYPE", "Create a new type"),
                ("ROLE", "Create a new role"),
                ("USER", "Create a new user"),
                ("OR REPLACE", "Create or replace object"),
            ];
            
            let mut suggestions = Vec::new();
            for (obj_type, desc) in create_objects {
                if obj_type.starts_with(&current_word.to_uppercase()) {
                    suggestions.push(Suggestion {
                        value: obj_type.to_string(),
                        description: Some(desc.to_string()),
                        span: Span { start: word_start, end: pos },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Blue)),
                    });
                }
            }
            return suggestions;
        }
        
        if line.to_uppercase().starts_with("ALTER ") && pos >= 6 {
            let word_start = 6; // After "ALTER "
            let current_word = if pos > word_start {
                &line[word_start..pos]
            } else {
                ""
            };
            
            let alter_objects = vec![
                ("TABLE", "Alter a table"),
                ("DATABASE", "Alter a database"),
                ("INDEX", "Alter an index"),
                ("VIEW", "Alter a view"),
                ("SCHEMA", "Alter a schema"),
                ("FUNCTION", "Alter a function"),
                ("PROCEDURE", "Alter a procedure"),
                ("SEQUENCE", "Alter a sequence"),
                ("TYPE", "Alter a type"),
                ("ROLE", "Alter a role"),
                ("USER", "Alter a user"),
                ("SYSTEM", "Alter system settings"),
            ];
            
            let mut suggestions = Vec::new();
            for (obj_type, desc) in alter_objects {
                if obj_type.starts_with(&current_word.to_uppercase()) {
                    suggestions.push(Suggestion {
                        value: obj_type.to_string(),
                        description: Some(desc.to_string()),
                        span: Span { start: word_start, end: pos },
                        append_whitespace: true,
                        extra: None,
                        style: Some(Style::new().fg(Color::Blue)),
                    });
                }
            }
            return suggestions;
        }
        
        // Determine word boundaries for SQL completion
        let word_start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ',')
            .map_or(0, |idx| idx + 1);
        let current_word = &line[word_start..pos];
        
        // Get database type and create appropriate parser
        let database_type = self.get_database_type();
        let parser = SqlParserFactory::create_parser(database_type.clone());
        
        // Parse SQL context using database-specific parser
        let enhanced_context = parser.parse_at_cursor(&full_line, pos);
        debug!("[SqlCompleter] Enhanced SQL Context Analysis:");
        debug!("  Database type: {:?}", enhanced_context.database_type);
        debug!("  Current clause: {:?}", enhanced_context.base_context.current_clause);
        debug!("  Tables in context: {} tables", enhanced_context.base_context.tables.len());
        for (i, table) in enhanced_context.base_context.tables.iter().enumerate() {
            debug!("    Table {}: {} (alias: {:?}, schema: {:?})", 
                   i, table.table, table.alias, table.schema);
        }
        debug!("  Expecting: {:?}", enhanced_context.base_context.expecting);
        debug!("  Database-specific context: {:?}", enhanced_context.database_context);
        debug!("  Current word: '{}'", current_word);

        // Generate suggestions based on enhanced context
        let suggestions = self.generate_enhanced_sql_suggestions(
            &enhanced_context,
            &parser,
            current_word,
            word_start,
            pos,
            &full_line,
        );

        debug!("[SqlCompleter] Final results: Generated {} suggestions", suggestions.len());
        for (i, suggestion) in suggestions.iter().enumerate() {
            debug!("  Suggestion {}: '{}' - {}", 
                   i, suggestion.value, 
                   suggestion.description.as_ref().unwrap_or(&"No description".to_string()));
        }
        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    async fn create_test_database_and_config() -> (Arc<Mutex<Database>>, Arc<Mutex<Config>>) {
        let db = Database::new_for_test();
        let config = Config::default();
        (Arc::new(Mutex::new(db)), Arc::new(Mutex::new(config)))
    }

    #[tokio::test]
    async fn test_basic_select_completion() {
        let (db, config) = create_test_database_and_config().await;
        let mut completer = SqlCompleter::new(db, config);
        
        let suggestions = completer.complete("SELECT ", 7);
        
        // Should suggest *, DISTINCT, columns if tables are known
        assert!(suggestions.iter().any(|s| s.value == "*"));
        assert!(suggestions.iter().any(|s| s.value == "DISTINCT"));
    }

    #[tokio::test]
    async fn test_from_clause_completion() {
        let (db, config) = create_test_database_and_config().await;
        let mut completer = SqlCompleter::new(db, config);
        
        let suggestions = completer.complete("SELECT * FROM ", 14);
        
        // Should suggest tables
        // In test mode, might not have real tables but structure should work
        assert!(suggestions.iter().all(|s| 
            s.description.as_ref().map(|d| d.contains("Table")).unwrap_or(false) ||
            s.description.as_ref().map(|d| d.contains("Keyword")).unwrap_or(false)
        ));
    }

    #[tokio::test]
    async fn test_update_statement_completion() {
        let (db, config) = create_test_database_and_config().await;
        let mut completer = SqlCompleter::new(db, config);
        
        // Test UPDATE table name completion
        let _suggestions = completer.complete("UPDATE ", 7);
        // Should suggest tables
        
        // Test SET clause
        let _suggestions = completer.complete("UPDATE users SET ", 17);
        // Should suggest columns
    }

    #[tokio::test]
    async fn test_backslash_command_completion() {
        let (db, config) = create_test_database_and_config().await;
        let mut completer = SqlCompleter::new(db, config);
        
        // Test basic backslash completion
        let suggestions = completer.complete("\\", 1);
        
        // Should suggest backslash commands
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.value == "\\h"));
        assert!(suggestions.iter().any(|s| s.value == "\\q"));
        
        // Test specific backslash command
        let suggestions = completer.complete("\\h", 2);
        assert!(suggestions.iter().any(|s| s.value == "\\h"));
    }
}