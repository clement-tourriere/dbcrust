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
            "\\q" | "\\h" | "\\x" | "\\e" | "\\config" | "\\ed" | "\\ecopy" | 
            "\\pager" | "\\banner" | "\\a" | "\\cs" | "\\clrcs" | "\\resetview" |
            "\\du" | "\\di" | "\\dp" | "\\pgpass" | "\\myconf" | "\\docker" |
            "\\ps" | "\\vc" | "\\vcc" | "\\vce" | "\\r" | "\\rc"
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
                let mut db_guard = db_arc.lock().unwrap();
                db_guard.get_tables_and_views(None).await
            })
        }).unwrap_or_else(|_| Vec::new());
        
        Ok(tables)
    }
    
    async fn get_database_names(&self) -> CompletionResult<Vec<String>> {
        let db = self.database.lock().unwrap();
        if !db.has_database_connection() {
            return Ok(Vec::new());
        }
        
        let db_arc = Arc::clone(&self.database);
        let databases = tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async {
                let mut db_guard = db_arc.lock().unwrap();
                db_guard.list_databases().await
            })
        }).unwrap_or_else(|_| Vec::new());
        
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
        let mut suggestions = Vec::new();
        
        // Find the word being completed
        let word_start = args[..pos.min(args.len())].rfind(' ').map_or(0, |i| i + 1);
        let current_word = &args[word_start..pos.min(args.len())];
        
        match command {
            "\\d" => {
                // Complete table names
                let tables = self.get_table_names().await?;
                for table in tables {
                    if table.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        suggestions.push(Suggestion {
                            value: table,
                            description: Some("Table".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: None,
                        });
                    }
                }
            }
            "\\c" => {
                // Complete database names
                let databases = self.get_database_names().await?;
                for db in databases {
                    if db.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        suggestions.push(Suggestion {
                            value: db,
                            description: Some("Database".to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: None,
                        });
                    }
                }
            }
            _ => {}
        }
        
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
        config.list_sessions().into_iter().map(|(name, _)| name).collect()
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
        let mut suggestions = Vec::new();
        
        let word_start = args[..pos.min(args.len())].rfind(' ').map_or(0, |i| i + 1);
        let current_word = &args[word_start..pos.min(args.len())];
        
        let sessions = self.get_session_names();
        
        match command {
            "\\s" | "\\ss" | "\\sd" => {
                for session in sessions {
                    if session.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        let description = match command {
                            "\\s" => "Connect to session",
                            "\\ss" => "Overwrite session", 
                            "\\sd" => "Delete session",
                            _ => "Session",
                        };
                        
                        suggestions.push(Suggestion {
                            value: session,
                            description: Some(description.to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: None,
                        });
                    }
                }
            }
            _ => {}
        }
        
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
        config.get_available_named_queries().into_iter().map(|(name, _)| name).collect()
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
                for query_name in queries {
                    if query_name.to_lowercase().starts_with(&current_word.to_lowercase()) {
                        let description = match command {
                            "\\n" => "Execute named query",
                            "\\nd" => "Delete named query",
                            _ => "Named query",
                        };
                        
                        suggestions.push(Suggestion {
                            value: query_name,
                            description: Some(description.to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: None,
                        });
                    }
                }
            }
            "\\ns" => {
                // For \ns, we need to parse the arguments to determine what to complete
                let args_parts: Vec<&str> = args.split_whitespace().collect();
                
                if args_parts.is_empty() || (args_parts.len() == 1 && pos <= args_parts[0].len()) {
                    // First argument: complete with existing named query names for overwriting
                    let queries = self.get_named_query_names();
                    for query_name in queries {
                        if query_name.to_lowercase().starts_with(&current_word.to_lowercase()) {
                            suggestions.push(Suggestion {
                                value: query_name,
                                description: Some("Overwrite existing named query".to_string()),
                                span: Span { start: word_start, end: pos },
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
                            if flag.to_lowercase().starts_with(&current_word.to_lowercase()) {
                                suggestions.push(Suggestion {
                                    value: flag.to_string(),
                                    description: Some(description.to_string()),
                                    span: Span { start: word_start, end: pos },
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
        let mut suggestions = Vec::new();
        
        let word_start = args[..pos.min(args.len())].rfind(' ').map_or(0, |i| i + 1);
        let current_word = &args[word_start..pos.min(args.len())];
        
        match command {
            "\\setmulti" => {
                let indicators = vec![
                    ("->", "Arrow indicator"),
                    ("...", "Ellipsis indicator"),
                    ("»", "Double right angle"),
                    ("|", "Pipe indicator"),
                    (">>", "Double arrow"),
                ];
                
                for (indicator, desc) in indicators {
                    if indicator.starts_with(current_word) {
                        suggestions.push(Suggestion {
                            value: indicator.to_string(),
                            description: Some(desc.to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: None,
                        });
                    }
                }
            }
            "\\csthreshold" => {
                let thresholds = vec![
                    ("5", "Very low threshold"),
                    ("10", "Default threshold"),
                    ("15", "Medium threshold"), 
                    ("20", "High threshold"),
                    ("25", "Very high threshold"),
                ];
                
                for (threshold, desc) in thresholds {
                    if threshold.starts_with(current_word) {
                        suggestions.push(Suggestion {
                            value: threshold.to_string(),
                            description: Some(desc.to_string()),
                            span: Span { start: word_start, end: pos },
                            append_whitespace: true,
                            extra: None,
                            style: None,
                        });
                    }
                }
            }
            _ => {}
        }
        
        Ok(suggestions)
    }
    
    fn handles_command(&self, command: &str) -> bool {
        matches!(command, "\\setmulti" | "\\csthreshold")
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
    pub fn new(
        database: Arc<Mutex<Database>>,
        config: Arc<Mutex<Config>>,
    ) -> Self {
        let mut completers: Vec<Box<dyn CommandCompleter>> = Vec::new();
        
        // Add completers in priority order
        completers.push(Box::new(DatabaseAwareCompleter::new(Arc::clone(&database))));
        completers.push(Box::new(DatabaseBasicCompleter::new(database)));
        completers.push(Box::new(SessionCompleter::new(Arc::clone(&config))));
        completers.push(Box::new(NamedQueryCompleter::new(config)));
        completers.push(Box::new(ConfigCompleter));
        completers.push(Box::new(BasicCommandCompleter)); // Fallback
        
        Self { completers }
    }
    
    /// Get command name completions (backslash commands)
    pub fn get_command_completions(&self, current_word: &str, word_start: usize, pos: usize) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();
        
        // Use the existing CommandParser to get all commands
        for (_category, commands) in CommandParser::get_commands_by_category() {
            for (cmd_name, cmd_description) in commands {
                if cmd_name.starts_with(current_word) {
                    suggestions.push(Suggestion {
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
                        eprintln!("Warning: {} failed for command {}: {}", 
                                 completer.name(), command, e);
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
        let command = &line[..command_end + 1]; // Include the backslash
        
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
        CommandCompletionManager::new(
            Arc::new(Mutex::new(db)),
            Arc::new(Mutex::new(config)),
        )
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