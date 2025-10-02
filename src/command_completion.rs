//! Trait-based command completion system
//! Provides specialized autocompletion for different types of backslash commands

use crate::commands::CommandParser;
use crate::config::Config;
use crate::db::Database;
use async_trait::async_trait;
use reedline::{Span, Suggestion};
use std::error::Error;
use std::sync::{Arc, Mutex};

/// Result type for completion operations
pub type CompletionResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Base trait for command argument completion
#[async_trait]
pub trait CommandCompleter: Send + Sync {
    /// Get completions for a command's arguments
    async fn complete_arguments(
        &self,
        command: &str,
        args: &str,
        pos: usize,
    ) -> CompletionResult<Vec<Suggestion>>;

    /// Check if this completer handles the given command
    fn handles_command(&self, command: &str) -> bool;

    /// Get the completer's name for debugging
    fn name(&self) -> &'static str;

    /// Helper method to build suggestions from a list of items
    /// This eliminates code duplication across all completers
    fn build_suggestions_from_items(
        &self,
        items: Vec<(String, String)>, // (value, description)
        args: &str,
        pos: usize,
        case_sensitive: bool,
    ) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();

        // Parse the current word being completed - same logic used by all completers
        let word_start = args[..pos.min(args.len())].rfind(' ').map_or(0, |i| i + 1);
        let current_word = &args[word_start..pos.min(args.len())];

        // Filter items that start with the current word
        for (value, description) in items {
            let matches = if case_sensitive {
                value.starts_with(current_word)
            } else {
                value
                    .to_lowercase()
                    .starts_with(&current_word.to_lowercase())
            };

            if matches {
                suggestions.push(Suggestion {
                    value,
                    description: Some(description),
                    span: Span {
                        start: word_start,
                        end: pos,
                    },
                    append_whitespace: true,
                    extra: None,
                    style: None,
                });
            }
        }

        suggestions
    }
}

/// Completer for basic commands that don't take arguments
pub struct BasicCommandCompleter;

#[async_trait]
impl CommandCompleter for BasicCommandCompleter {
    async fn complete_arguments(
        &self,
        _command: &str,
        _args: &str,
        _pos: usize,
    ) -> CompletionResult<Vec<Suggestion>> {
        // Basic commands don't have arguments
        Ok(Vec::new())
    }

    fn handles_command(&self, command: &str) -> bool {
        matches!(
            command,
            "\\q"
                | "\\h"
                | "\\x"
                | "\\e"
                | "\\config"
                | "\\ed"
                | "\\ecopy"
                | "\\pager"
                | "\\banner"
                | "\\a"
                | "\\cs"
                | "\\clrcs"
                | "\\resetview"
                | "\\vdc"
                | "\\vs"
                | "\\du"
                | "\\di"
                | "\\dp"
                | "\\pgpass"
                | "\\myconf"
                | "\\docker"
                | "\\ps"
                | "\\vc"
                | "\\vcc"
                | "\\vce"
                | "\\r"
                | "\\rc"
        )
    }

    fn name(&self) -> &'static str {
        "BasicCommandCompleter"
    }
}

/// Completer for commands that need database access but don't take arguments
pub struct DatabaseBasicCompleter {
    #[allow(dead_code)]
    database: Arc<Mutex<Database>>,
}

impl DatabaseBasicCompleter {
    pub fn new(database: Arc<Mutex<Database>>) -> Self {
        Self { database }
    }
}

#[async_trait]
impl CommandCompleter for DatabaseBasicCompleter {
    async fn complete_arguments(
        &self,
        _command: &str,
        _args: &str,
        _pos: usize,
    ) -> CompletionResult<Vec<Suggestion>> {
        // These commands don't take arguments
        Ok(Vec::new())
    }

    fn handles_command(&self, command: &str) -> bool {
        matches!(command, "\\l" | "\\dt")
    }

    fn name(&self) -> &'static str {
        "DatabaseBasicCompleter"
    }
}

/// Completer for database-aware commands
pub struct DatabaseAwareCompleter {
    database: Arc<Mutex<Database>>,
}

impl DatabaseAwareCompleter {
    pub fn new(database: Arc<Mutex<Database>>) -> Self {
        Self { database }
    }

    #[allow(clippy::await_holding_lock)]
    async fn get_table_names(&self) -> CompletionResult<Vec<String>> {
        let db = self.database.lock().unwrap();
        if !db.has_database_connection() {
            return Ok(Vec::new());
        }

        // Use tokio's block_in_place for async operations
        let db_arc = Arc::clone(&self.database);
        let tables = tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async {
                // Lock held across await for fetching table metadata
                let mut db_guard = db_arc.lock().unwrap();
                db_guard.get_tables_and_views(None).await
            })
        })
        .unwrap_or_else(|_| Vec::new());

        Ok(tables)
    }

    #[allow(clippy::await_holding_lock)]
    async fn get_database_names(&self) -> CompletionResult<Vec<String>> {
        let db = self.database.lock().unwrap();
        if !db.has_database_connection() {
            return Ok(Vec::new());
        }

        let db_arc = Arc::clone(&self.database);
        let databases = tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async {
                // Lock held across await for fetching database metadata
                let mut db_guard = db_arc.lock().unwrap();
                db_guard.list_databases().await
            })
        })
        .unwrap_or_else(|_| Vec::new());

        // list_databases returns Vec<Vec<String>>, we need to flatten it
        let flattened_databases: Vec<String> = databases.into_iter().flatten().collect();
        Ok(flattened_databases)
    }
}

#[async_trait]
impl CommandCompleter for DatabaseAwareCompleter {
    async fn complete_arguments(
        &self,
        command: &str,
        args: &str,
        pos: usize,
    ) -> CompletionResult<Vec<Suggestion>> {
        let suggestions = match command {
            "\\d" => {
                // Complete table names
                let tables = self.get_table_names().await?;
                let items: Vec<(String, String)> = tables
                    .into_iter()
                    .map(|table| (table, "Table".to_string()))
                    .collect();
                self.build_suggestions_from_items(items, args, pos, false) // case insensitive
            }
            "\\c" => {
                // Complete database names
                let databases = self.get_database_names().await?;
                let items: Vec<(String, String)> = databases
                    .into_iter()
                    .map(|db| (db, "Database".to_string()))
                    .collect();
                self.build_suggestions_from_items(items, args, pos, false) // case insensitive
            }
            _ => Vec::new(),
        };

        Ok(suggestions)
    }

    fn handles_command(&self, command: &str) -> bool {
        matches!(command, "\\d" | "\\c")
    }

    fn name(&self) -> &'static str {
        "DatabaseAwareCompleter"
    }
}

/// Completer for session management commands
pub struct SessionCompleter {
    config: Arc<Mutex<Config>>,
}

impl SessionCompleter {
    pub fn new(config: Arc<Mutex<Config>>) -> Self {
        Self { config }
    }

    fn get_session_names(&self) -> Vec<String> {
        let config = self.config.lock().unwrap();
        config
            .list_sessions()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }
}

#[async_trait]
impl CommandCompleter for SessionCompleter {
    async fn complete_arguments(
        &self,
        command: &str,
        args: &str,
        pos: usize,
    ) -> CompletionResult<Vec<Suggestion>> {
        let suggestions = match command {
            "\\s" | "\\ss" | "\\sd" => {
                let sessions = self.get_session_names();
                let description = match command {
                    "\\s" => "Connect to session",
                    "\\ss" => "Overwrite session",
                    "\\sd" => "Delete session",
                    _ => "Session",
                };

                let items: Vec<(String, String)> = sessions
                    .into_iter()
                    .map(|session| (session, description.to_string()))
                    .collect();
                self.build_suggestions_from_items(items, args, pos, false) // case insensitive
            }
            _ => Vec::new(),
        };

        Ok(suggestions)
    }

    fn handles_command(&self, command: &str) -> bool {
        matches!(command, "\\s" | "\\ss" | "\\sd")
    }

    fn name(&self) -> &'static str {
        "SessionCompleter"
    }
}

/// Completer for named query commands
pub struct NamedQueryCompleter {
    config: Arc<Mutex<Config>>,
}

impl NamedQueryCompleter {
    pub fn new(config: Arc<Mutex<Config>>) -> Self {
        Self { config }
    }

    fn get_named_query_names(&self) -> Vec<String> {
        let config = self.config.lock().unwrap();
        // Use the new scoped named queries API
        config
            .get_available_named_queries()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    fn get_scope_flags(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("--global", "Save to global scope"),
            ("--postgres", "Save to PostgreSQL scope"),
            ("--mysql", "Save to MySQL scope"),
            ("--sqlite", "Save to SQLite scope"),
        ]
    }
}

#[async_trait]
impl CommandCompleter for NamedQueryCompleter {
    async fn complete_arguments(
        &self,
        command: &str,
        args: &str,
        pos: usize,
    ) -> CompletionResult<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        let word_start = args[..pos.min(args.len())].rfind(' ').map_or(0, |i| i + 1);
        let current_word = &args[word_start..pos.min(args.len())];

        match command {
            "\\n" | "\\nd" => {
                // For \n and \nd, complete with existing named query names only
                let queries = self.get_named_query_names();
                let description = match command {
                    "\\n" => "Execute named query",
                    "\\nd" => "Delete named query",
                    _ => "Named query",
                };

                let items: Vec<(String, String)> = queries
                    .into_iter()
                    .map(|query_name| (query_name, description.to_string()))
                    .collect();
                suggestions.extend(self.build_suggestions_from_items(items, args, pos, false));
            }
            "\\ns" => {
                // For \ns, we need to parse the arguments to determine what to complete
                let args_parts: Vec<&str> = args.split_whitespace().collect();

                if args_parts.is_empty() || (args_parts.len() == 1 && pos <= args_parts[0].len()) {
                    // First argument: complete with existing named query names for overwriting
                    let queries = self.get_named_query_names();
                    for query_name in queries {
                        if query_name
                            .to_lowercase()
                            .starts_with(&current_word.to_lowercase())
                        {
                            suggestions.push(Suggestion {
                                value: query_name,
                                description: Some("Overwrite existing named query".to_string()),
                                span: Span {
                                    start: word_start,
                                    end: pos,
                                },
                                append_whitespace: true,
                                extra: None,
                                style: None,
                            });
                        }
                    }
                } else {
                    // Check if we're completing a flag
                    if current_word.starts_with('-') {
                        let scope_flags = self.get_scope_flags();
                        for (flag, description) in scope_flags {
                            if flag
                                .to_lowercase()
                                .starts_with(&current_word.to_lowercase())
                            {
                                suggestions.push(Suggestion {
                                    value: flag.to_string(),
                                    description: Some(description.to_string()),
                                    span: Span {
                                        start: word_start,
                                        end: pos,
                                    },
                                    append_whitespace: true,
                                    extra: None,
                                    style: None,
                                });
                            }
                        }
                    }
                    // For SQL completion after the query name and flags, we don't provide suggestions
                    // The SQL autocomplete system will handle that
                }
            }
            _ => {}
        }

        Ok(suggestions)
    }

    fn handles_command(&self, command: &str) -> bool {
        matches!(command, "\\n" | "\\ns" | "\\nd")
    }

    fn name(&self) -> &'static str {
        "NamedQueryCompleter"
    }
}

/// Completer for configuration commands with predefined values
pub struct ConfigCompleter;

#[async_trait]
impl CommandCompleter for ConfigCompleter {
    async fn complete_arguments(
        &self,
        command: &str,
        args: &str,
        pos: usize,
    ) -> CompletionResult<Vec<Suggestion>> {
        let suggestions = match command {
            "\\setmulti" => {
                let items = vec![
                    ("->".to_string(), "Arrow indicator".to_string()),
                    ("...".to_string(), "Ellipsis indicator".to_string()),
                    ("»".to_string(), "Double right angle".to_string()),
                    ("|".to_string(), "Pipe indicator".to_string()),
                    (">>".to_string(), "Double arrow".to_string()),
                ];
                self.build_suggestions_from_items(items, args, pos, true)
            }
            "\\csthreshold" => {
                let items = vec![
                    ("5".to_string(), "Very low threshold".to_string()),
                    ("10".to_string(), "Default threshold".to_string()),
                    ("15".to_string(), "Medium threshold".to_string()),
                    ("20".to_string(), "High threshold".to_string()),
                    ("25".to_string(), "Very high threshold".to_string()),
                ];
                self.build_suggestions_from_items(items, args, pos, true)
            }
            "\\vd" => {
                let items = vec![
                    (
                        "full".to_string(),
                        "Show all elements in matrix-style layout".to_string(),
                    ),
                    (
                        "truncated".to_string(),
                        "Show first/last few elements with ellipsis".to_string(),
                    ),
                    (
                        "summary".to_string(),
                        "Show statistical summary (min, max, mean, std, norm)".to_string(),
                    ),
                    (
                        "viz".to_string(),
                        "ASCII visualization using Unicode blocks".to_string(),
                    ),
                ];
                self.build_suggestions_from_items(items, args, pos, true)
            }
            "\\cd" => {
                let items = vec![
                    (
                        "full".to_string(),
                        "Show complete data structure with all elements".to_string(),
                    ),
                    (
                        "truncated".to_string(),
                        "Show first few characters with ellipsis".to_string(),
                    ),
                    (
                        "summary".to_string(),
                        "Show structure overview with element counts".to_string(),
                    ),
                    (
                        "viz".to_string(),
                        "ASCII art representation of data structure".to_string(),
                    ),
                ];
                self.build_suggestions_from_items(items, args, pos, true)
            }
            _ => Vec::new(),
        };

        Ok(suggestions)
    }

    fn handles_command(&self, command: &str) -> bool {
        matches!(command, "\\setmulti" | "\\csthreshold" | "\\vd" | "\\cd")
    }

    fn name(&self) -> &'static str {
        "ConfigCompleter"
    }
}

/// Main completion coordinator that manages all command completers
pub struct CommandCompletionManager {
    completers: Vec<Box<dyn CommandCompleter>>,
}

impl CommandCompletionManager {
    pub fn new(database: Arc<Mutex<Database>>, config: Arc<Mutex<Config>>) -> Self {
        let completers: Vec<Box<dyn CommandCompleter>> = vec![
            // Add completers in priority order
            Box::new(DatabaseAwareCompleter::new(Arc::clone(&database))),
            Box::new(DatabaseBasicCompleter::new(database)),
            Box::new(SessionCompleter::new(Arc::clone(&config))),
            Box::new(NamedQueryCompleter::new(config)),
            Box::new(ConfigCompleter),
            Box::new(BasicCommandCompleter), // Fallback
        ];

        Self { completers }
    }

    /// Get command name completions (backslash commands)
    pub fn get_command_completions(
        &self,
        current_word: &str,
        word_start: usize,
        pos: usize,
    ) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();

        // Use the existing CommandParser to get all commands
        for (_category, commands) in CommandParser::get_commands_by_category() {
            for (cmd_name, cmd_description) in commands {
                if cmd_name.starts_with(current_word) {
                    suggestions.push(Suggestion {
                        value: cmd_name.to_string(),
                        description: Some(cmd_description.to_string()),
                        span: Span {
                            start: word_start,
                            end: pos,
                        },
                        append_whitespace: true,
                        extra: None,
                        style: None,
                    });
                }
            }
        }

        suggestions
    }

    /// Get argument completions for a specific command
    pub async fn get_argument_completions(
        &self,
        command: &str,
        args: &str,
        pos: usize,
    ) -> Vec<Suggestion> {
        // Find the appropriate completer
        for completer in &self.completers {
            if completer.handles_command(command) {
                match completer.complete_arguments(command, args, pos).await {
                    Ok(suggestions) => return suggestions,
                    Err(e) => {
                        eprintln!(
                            "Warning: {} failed for command {}: {}",
                            completer.name(),
                            command,
                            e
                        );
                        continue;
                    }
                }
            }
        }
        Vec::new()
    }

    /// Parse a backslash command line and determine command vs arguments
    pub fn parse_command_line(&self, line: &str, pos: usize) -> Option<(String, String, usize)> {
        if !line.starts_with('\\') {
            return None;
        }

        // Find the end of the command name (first space or end of line)
        let command_end = line[1..].find(' ').map_or(line.len() - 1, |i| i + 1);
        let command = &line[..command_end + 1]; // Include the backslash but not the space

        if command_end + 1 >= line.len() {
            // No arguments yet
            return Some((command.to_string(), String::new(), 0));
        }

        let args_start = command_end + 1;
        let args = &line[args_start..];
        let args_pos = pos.saturating_sub(args_start);

        Some((command.to_string(), args.to_string(), args_pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    async fn create_test_manager() -> CommandCompletionManager {
        let db = Database::new_for_test();
        let config = Config::default();
        CommandCompletionManager::new(Arc::new(Mutex::new(db)), Arc::new(Mutex::new(config)))
    }

    #[tokio::test]
    async fn test_command_name_completion() {
        let manager = create_test_manager().await;

        let suggestions = manager.get_command_completions("\\h", 0, 2);
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.value == "\\h"));
    }

    #[tokio::test]
    async fn test_basic_command_handling() {
        let _manager = create_test_manager().await;
        let basic_completer = BasicCommandCompleter;

        assert!(basic_completer.handles_command("\\q"));
        assert!(basic_completer.handles_command("\\h"));
        assert!(!basic_completer.handles_command("\\d"));
    }

    #[tokio::test]
    async fn test_command_line_parsing() {
        let manager = create_test_manager().await;

        // Test command only
        let result = manager.parse_command_line("\\h", 2);
        assert_eq!(result, Some(("\\h".to_string(), String::new(), 0)));

        // Test command with arguments
        let result = manager.parse_command_line("\\d users", 8);
        assert_eq!(result, Some(("\\d".to_string(), "users".to_string(), 5)));

        // Test incomplete command with space
        let result = manager.parse_command_line("\\d ", 3);
        assert_eq!(result, Some(("\\d".to_string(), "".to_string(), 0)));
    }

    #[tokio::test]
    async fn test_config_completer() {
        let config_completer = ConfigCompleter;

        let suggestions = config_completer
            .complete_arguments("\\setmulti", "->", 2)
            .await
            .unwrap();

        assert!(suggestions.iter().any(|s| s.value == "->"));
    }
}
