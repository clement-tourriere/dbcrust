use crate::config::Config as DbCrustConfig;
use crate::db::Database;
use crate::format::{format_query_results_expanded, format_query_results_psql, format_table_details};
use crate::script::edit_multiline_script;
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use crate::prompt::DbPrompt;
use std::fs;
use arboard::Clipboard;

/// Registry of all backslash commands
pub struct BackslashCommandRegistry;

impl BackslashCommandRegistry {
    pub fn new() -> Self {
        Self
    }
    
    pub async fn execute(
        &self,
        command: &str,
        database: &Arc<Mutex<Database>>,
        config: &mut DbCrustConfig,
        last_script: &mut String,
        _interrupt_flag: &Arc<AtomicBool>,
        prompt: &mut DbPrompt,
    ) -> Result<bool, Box<dyn StdError>> {
        // Parse command and args
        let mut parts = command.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("");
        
        match cmd {
            "\\q" => {
                Ok(true) // Exit
            }
            "\\h" => {
                print_help_commands(config);
                println!();
                Ok(false)
            }
            "\\l" => {
                let mut db = database.lock().unwrap();
                match db.list_databases().await {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No databases found.");
                        } else {
                            if db.is_expanded_display() {
                                let expanded_tables = format_query_results_expanded(&results);
                                for table in expanded_tables {
                                    println!("{}", table);
                                }
                            } else {
                                let output = format_query_results_psql(&results);
                                print!("{}", output);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error listing databases: {}", e);
                    }
                }
                Ok(false)
            }
            "\\dt" => {
                let mut db = database.lock().unwrap();
                match db.list_tables().await {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No tables found.");
                        } else {
                            if db.is_expanded_display() {
                                let expanded_tables = format_query_results_expanded(&results);
                                for table in expanded_tables {
                                    println!("{}", table);
                                }
                            } else {
                                let output = format_query_results_psql(&results);
                                print!("{}", output);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error listing tables: {}", e);
                    }
                }
                Ok(false)
            }
            "\\d" => {
                let mut db = database.lock().unwrap();
                
                if args.trim().is_empty() {
                    // No table specified, list all tables
                    match db.list_tables().await {
                        Ok(results) => {
                            if results.is_empty() {
                                println!("No tables found.");
                            } else {
                                if db.is_expanded_display() {
                                    let expanded_tables = format_query_results_expanded(&results);
                                    for table in expanded_tables {
                                        println!("{}", table);
                                    }
                                } else {
                                    let output = format_query_results_psql(&results);
                                    print!("{}", output);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error listing tables: {}", e);
                        }
                    }
                } else {
                    // Table specified, describe it
                    let table_name = args.trim();
                    
                    // Use the comprehensive get_table_details method
                    match db.get_table_details(table_name).await {
                        Ok(details) => {
                            // Use the comprehensive formatting function
                            let formatted = format_table_details(&details);
                            print!("{}", formatted);
                        }
                        Err(e) => {
                            eprintln!("Error describing table '{}': {}", table_name, e);
                        }
                    }
                }
                Ok(false)
            }
            "\\c" => {
                let db_name = args.trim();
                
                if db_name.is_empty() {
                    eprintln!("Usage: \\c <database_name>");
                    return Ok(false);
                }
                
                let mut db = database.lock().unwrap();
                match db.connect_to_db(db_name).await {
                    Ok(()) => {
                        println!("Connected to database '{}'", db_name);
                        prompt.update_database(db_name);
                    }
                    Err(e) => {
                        eprintln!("Error connecting to database '{}': {}", db_name, e);
                    }
                }
                Ok(false)
            }
            "\\x" => {
                let mut db = database.lock().unwrap();
                db.toggle_expanded_display();
                println!("Expanded display is now {}", 
                         if db.is_expanded_display() { "ON" } else { "OFF" });
                Ok(false)
            }
            "\\e" => {
                let mut db = database.lock().unwrap();
                db.toggle_explain_mode();
                println!("EXPLAIN mode is now {}", 
                         if db.is_explain_mode() { "ON" } else { "OFF" });
                Ok(false)
            }
            "\\w" => {
                let filename = args.trim();
                
                if filename.is_empty() {
                    eprintln!("Usage: \\w <filename>");
                    return Ok(false);
                }
                
                if last_script.is_empty() {
                    eprintln!("No script to write. Execute a query or use \\ed to create a script first.");
                    return Ok(false);
                }
                
                match fs::write(filename, last_script.as_bytes()) {
                    Ok(()) => {
                        println!("Script written to '{}' ({} bytes)", filename, last_script.len());
                    }
                    Err(e) => {
                        eprintln!("Error writing script to '{}': {}", filename, e);
                    }
                }
                Ok(false)
            }
            "\\i" => {
                let filename = args.trim();
                
                if filename.is_empty() {
                    eprintln!("Usage: \\i <filename>");
                    return Ok(false);
                }
                
                let script_content = match fs::read_to_string(filename) {
                    Ok(content) => content,
                    Err(e) => {
                        eprintln!("Error reading script from '{}': {}", filename, e);
                        return Ok(false);
                    }
                };
                
                // Store the script for future reference
                *last_script = script_content.clone();
                
                // Execute the script
                let mut db = database.lock().unwrap();
                match db.execute_query(&script_content).await {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("Script executed successfully from '{}' (no results)", filename);
                        } else {
                            println!("Script executed successfully from '{}':", filename);
                            if db.is_expanded_display() {
                                let expanded_tables = format_query_results_expanded(&results);
                                for table in expanded_tables {
                                    println!("{}", table);
                                }
                            } else {
                                let output = format_query_results_psql(&results);
                                print!("{}", output);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error executing script from '{}': {}", filename, e);
                    }
                }
                Ok(false)
            }
            "\\ed" => {
                println!("Entering multiline edit mode...");
                
                // Show current content if any
                if !last_script.is_empty() {
                    println!("Editing existing script ({} bytes):", last_script.len());
                    if last_script.lines().count() <= 5 {
                        for line in last_script.lines() {
                            println!("  {}", line);
                        }
                    } else {
                        // Show first few lines
                        for line in last_script.lines().take(3) {
                            println!("  {}", line);
                        }
                        println!("  ... ({} more lines) ...", last_script.lines().count() - 3);
                    }
                }
                
                match edit_multiline_script(last_script) {
                    Ok(script) => {
                        if script.is_empty() {
                            println!("No changes made (empty script).");
                        } else {
                            *last_script = script;
                            println!("Script ready ({} bytes, {} lines). Use \\q to quit edit mode.", 
                                     last_script.len(), last_script.lines().count());
                        }
                    }
                    Err(e) => {
                        eprintln!("Error editing script: {}", e);
                    }
                }
                Ok(false)
            }
            "\\ecopy" => {
                let db = database.lock().unwrap();
                match db.get_last_json_plan() {
                    Some(json_plan) => {
                        match Clipboard::new() {
                            Ok(mut clipboard) => {
                                match clipboard.set_text(json_plan.clone()) {
                                    Ok(()) => {
                                        println!("EXPLAIN JSON plan copied to clipboard ({} characters)", json_plan.len());
                                    }
                                    Err(e) => {
                                        eprintln!("Error copying to clipboard: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Error accessing clipboard: {}", e);
                            }
                        }
                    }
                    None => {
                        eprintln!("No EXPLAIN JSON plan available. Run an EXPLAIN query first.");
                    }
                }
                Ok(false)
            }
            "\\config" => {
                println!("Current configuration:");
                println!("  Default limit: {}", config.default_limit);
                println!("  Expanded display default: {}", config.expanded_display_default);
                println!("  Autocomplete enabled: {}", config.autocomplete_enabled);
                println!("  EXPLAIN mode default: {}", config.explain_mode_default);
                println!("  Column selection mode default: {}", config.column_selection_mode_default);
                println!("  Column selection threshold: {}", config.column_selection_threshold);
                println!("  Pager enabled: {}", config.pager_enabled);
                println!("  Pager command: {}", config.pager_command);
                println!("  Pager threshold lines: {}", 
                    if config.pager_threshold_lines == 0 { "terminal height".to_string() } else { config.pager_threshold_lines.to_string() });
                println!("  Debug logging enabled: {}", config.debug_logging_enabled);
                println!("  Show banner default: {}", config.show_banner_default);
                println!("  Multiline prompt indicator: '{}'", config.multiline_prompt_indicator);
                println!("  Named queries: {}", config.named_queries.len());
                println!("  Saved sessions: {}", config.saved_sessions.len());
                println!("  SSH tunnel patterns: {}", config.ssh_tunnel_patterns.len());
                Ok(false)
            }
            "\\n" => {
                if config.named_queries.is_empty() {
                    println!("No named queries defined.");
                } else {
                    println!("Named queries:");
                    let mut queries: Vec<_> = config.named_queries.iter().collect();
                    queries.sort_by(|a, b| a.0.cmp(b.0));
                    for (name, query) in queries {
                        let display_query = if query.len() > 80 {
                            format!("{}...", &query[..77])
                        } else {
                            query.clone()
                        };
                        println!("  {}: {}", name, display_query);
                    }
                }
                Ok(false)
            }
            "\\ns" => {
                let trimmed_args = args.trim();
                if trimmed_args.is_empty() {
                    eprintln!("Usage: \\ns <name> <query>");
                    return Ok(false);
                }
                
                // Find the first space to separate name from query
                let mut parts = trimmed_args.splitn(2, ' ');
                let name = parts.next().unwrap_or("").trim();
                let query = parts.next().unwrap_or("").trim();
                
                if name.is_empty() || query.is_empty() {
                    eprintln!("Usage: \\ns <name> <query>");
                    return Ok(false);
                }
                
                // Save the named query
                let was_update = config.named_queries.contains_key(name);
                config.named_queries.insert(name.to_string(), query.to_string());
                
                // Save the config to disk
                match config.save() {
                    Ok(()) => {
                        if was_update {
                            println!("Updated named query '{}': {}", name, query);
                        } else {
                            println!("Saved named query '{}': {}", name, query);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error saving named query: {}", e);
                    }
                }
                Ok(false)
            }
            "\\nd" => {
                let name = args.trim();
                if name.is_empty() {
                    eprintln!("Usage: \\nd <name>");
                    return Ok(false);
                }
                
                if config.named_queries.remove(name).is_some() {
                    // Save the config to disk
                    match config.save() {
                        Ok(()) => {
                            println!("Deleted named query '{}'", name);
                        }
                        Err(e) => {
                            eprintln!("Error saving config after deleting named query: {}", e);
                        }
                    }
                } else {
                    eprintln!("Named query '{}' not found", name);
                }
                Ok(false)
            }
            _ => {
                eprintln!("Unknown command: {}. Type \\h for help.", cmd);
                Ok(false)
            }
        }
    }
    
    pub fn get_command_names(&self) -> Vec<&'static str> {
        vec!["\\q", "\\h", "\\l", "\\dt", "\\d", "\\c", "\\x", "\\e", "\\w", "\\i", "\\ed", "\\ecopy", "\\config", "\\n", "\\ns", "\\nd"]
    }
    
    pub fn get_command_info(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("\\q", "Quit dbcrust"),
            ("\\h", "Show help"),
            ("\\l", "List databases"),
            ("\\dt", "List tables in the current database"),
            ("\\d", "List all tables or describe a specific table (e.g., \\d tablename)"),
            ("\\c", "Connect to a different database (e.g., \\c newdb)"),
            ("\\x", "Toggle expanded display mode"),
            ("\\e", "Toggle EXPLAIN mode (prepend EXPLAIN to queries)"),
            ("\\w", "Write last script to file (e.g., \\w script.sql)"),
            ("\\i", "Load and execute script from file (e.g., \\i script.sql)"),
            ("\\ed", "Enter multiline edit mode"),
            ("\\ecopy", "Copy last EXPLAIN JSON plan to clipboard"),
            ("\\config", "Show current configuration"),
            ("\\n", "List all named queries"),
            ("\\ns", "Save a named query (e.g., \\ns myquery SELECT * FROM users)"),
            ("\\nd", "Delete a named query (e.g., \\nd myquery)"),
        ]
    }
}

// Helper function to print help
fn print_help_commands(_config: &DbCrustConfig) {
    println!("Available commands:");
    println!("  \\q               Quit dbcrust");
    println!("  \\h               Show this help");
    println!("  \\l               List databases");
    println!("  \\dt              List tables");
    println!("  \\d [table]       List all tables or describe a specific table");
    println!("  \\x               Toggle expanded display");
    println!("  \\e               Toggle EXPLAIN mode");
    println!("  \\c <database>    Connect to a different database");
    println!("  \\w <file>        Write last script to file");
    println!("  \\i <file>        Load and execute script from file");
    println!("  \\ed              Enter multiline edit mode");
    println!("  \\ecopy           Copy last EXPLAIN JSON plan to clipboard");
    println!("  \\config          Show current configuration");
    println!("  \\n               List all named queries");
    println!("  \\ns <name> <query> Save a named query");
    println!("  \\nd <name>        Delete a named query");
    println!();
    println!("SQL queries are executed immediately when you press Enter.");
    println!("Use Alt+Enter to add newlines for multi-line queries.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config as DbCrustConfig;
    use crate::db::Database;
    use crate::prompt::DbPrompt;
    use rstest::rstest;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;
    use tokio;

    /// Helper function to create a test database
    fn create_test_database() -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(Database::new_for_test()))
    }

    /// Helper function to create a test config
    fn create_test_config() -> DbCrustConfig {
        let mut config = DbCrustConfig::default();
        config.named_queries = HashMap::new();
        config.named_queries.insert("test_query".to_string(), "SELECT 1".to_string());
        config.named_queries.insert("another_query".to_string(), "SELECT * FROM users".to_string());
        config
    }

    /// Helper function to create test prompt
    fn create_test_prompt() -> DbPrompt {
        DbPrompt::with_config("testuser".to_string(), "testdb".to_string(), "->".to_string())
    }

    /// Helper function to create command registry
    fn create_command_registry() -> BackslashCommandRegistry {
        BackslashCommandRegistry::new()
    }

    // ===================
    // Phase 1: Simple Commands
    // ===================

    #[rstest]
    #[tokio::test]
    async fn test_quit_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let result = registry.execute(
            "\\q",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), true); // Should return true to indicate exit
    }

    #[rstest]
    #[tokio::test]
    async fn test_help_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let result = registry.execute(
            "\\h",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue
    }

    #[rstest]
    #[tokio::test]
    async fn test_expanded_display_toggle() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Get initial state
        let initial_expanded = {
            let db = database.lock().unwrap();
            db.is_expanded_display()
        };

        let result = registry.execute(
            "\\x",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue

        // Verify state changed
        let new_expanded = {
            let db = database.lock().unwrap();
            db.is_expanded_display()
        };
        assert_ne!(initial_expanded, new_expanded);
    }

    #[rstest]
    #[tokio::test]
    async fn test_explain_mode_toggle() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Get initial state
        let initial_explain = {
            let db = database.lock().unwrap();
            db.is_explain_mode()
        };

        let result = registry.execute(
            "\\e",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue

        // Verify state changed
        let new_explain = {
            let db = database.lock().unwrap();
            db.is_explain_mode()
        };
        assert_ne!(initial_explain, new_explain);
    }

    #[rstest]
    #[tokio::test]
    async fn test_config_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let result = registry.execute(
            "\\config",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue
    }

    #[rstest]
    #[tokio::test]
    async fn test_list_named_queries_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let result = registry.execute(
            "\\n",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue
    }

    #[rstest]
    #[tokio::test]
    async fn test_list_named_queries_empty() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = DbCrustConfig::default(); // Empty config
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let result = registry.execute(
            "\\n",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue
    }

    #[rstest]
    #[tokio::test]
    async fn test_unknown_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let result = registry.execute(
            "\\unknown",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false); // Should return false to continue
    }

    #[rstest]
    #[tokio::test]
    async fn test_command_names_length() {
        let registry = create_command_registry();
        let command_names = registry.get_command_names();
        
        // Verify we have the expected number of commands
        assert_eq!(command_names.len(), 16);
        
        // Verify specific commands exist
        assert!(command_names.contains(&"\\q"));
        assert!(command_names.contains(&"\\h"));
        assert!(command_names.contains(&"\\x"));
        assert!(command_names.contains(&"\\e"));
        assert!(command_names.contains(&"\\config"));
        assert!(command_names.contains(&"\\n"));
        assert!(command_names.contains(&"\\ns"));
        assert!(command_names.contains(&"\\nd"));
        assert!(command_names.contains(&"\\ecopy"));
    }

    #[rstest]
    #[tokio::test]
    async fn test_command_info_consistency() {
        let registry = create_command_registry();
        let command_names = registry.get_command_names();
        let command_info = registry.get_command_info();
        
        // Every command should have info
        assert_eq!(command_names.len(), command_info.len());
        
        // Every command name should have corresponding info
        for name in command_names {
            assert!(command_info.iter().any(|(info_name, _)| info_name == &name));
        }
    }

    // ===================
    // Phase 2: File Operations and Configuration Commands
    // ===================

    #[rstest]
    #[tokio::test]
    async fn test_save_named_query_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = DbCrustConfig::default(); // Start with empty config
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test saving a new named query
        let result = registry.execute(
            "\\ns test_save SELECT * FROM users WHERE id = 1",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the query was saved
        assert!(config.named_queries.contains_key("test_save"));
        assert_eq!(config.named_queries.get("test_save").unwrap(), "SELECT * FROM users WHERE id = 1");
    }

    #[rstest]
    #[tokio::test]
    async fn test_save_named_query_update_existing() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config(); // Has existing queries
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        let original_query = config.named_queries.get("test_query").unwrap().clone();
        
        // Test updating an existing named query
        let result = registry.execute(
            "\\ns test_query SELECT * FROM new_table",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the query was updated
        assert_ne!(config.named_queries.get("test_query").unwrap(), &original_query);
        assert_eq!(config.named_queries.get("test_query").unwrap(), "SELECT * FROM new_table");
    }

    #[rstest]
    #[tokio::test]
    async fn test_save_named_query_invalid_syntax() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test with empty command
        let result = registry.execute(
            "\\ns",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);

        // Test with name only, no query
        let result = registry.execute(
            "\\ns test_name",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete_named_query_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config(); // Has existing queries
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Verify the query exists first
        assert!(config.named_queries.contains_key("test_query"));
        
        // Test deleting the named query
        let result = registry.execute(
            "\\nd test_query",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the query was deleted
        assert!(!config.named_queries.contains_key("test_query"));
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete_named_query_not_found() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test deleting a non-existent named query
        let result = registry.execute(
            "\\nd nonexistent_query",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify existing queries are still there
        assert!(config.named_queries.contains_key("test_query"));
        assert!(config.named_queries.contains_key("another_query"));
    }

    #[rstest]
    #[tokio::test]
    async fn test_delete_named_query_invalid_syntax() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test with empty command
        let result = registry.execute(
            "\\nd",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify no queries were deleted
        assert!(config.named_queries.contains_key("test_query"));
        assert!(config.named_queries.contains_key("another_query"));
    }

    #[rstest]
    #[tokio::test]
    async fn test_write_script_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = "SELECT * FROM test_table;".to_string();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Create a temporary file
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_str().unwrap();

        // Test writing script to file
        let result = registry.execute(
            &format!("\\w {}", temp_path),
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the file was written
        let content = std::fs::read_to_string(temp_path).unwrap();
        assert_eq!(content, "SELECT * FROM test_table;");
    }

    #[rstest]
    #[tokio::test]
    async fn test_write_script_empty_script() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new(); // Empty script
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Create a temporary file
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_str().unwrap();

        // Test writing empty script to file
        let result = registry.execute(
            &format!("\\w {}", temp_path),
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_write_script_invalid_syntax() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = "SELECT * FROM test_table;".to_string();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test with empty filename
        let result = registry.execute(
            "\\w",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_load_script_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Create a temporary file with content
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_str().unwrap();
        let test_content = "SELECT * FROM test_table WHERE id = 1;";
        std::fs::write(temp_path, test_content).unwrap();

        // Test loading script from file
        let result = registry.execute(
            &format!("\\i {}", temp_path),
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the script was loaded
        assert_eq!(last_script, test_content);
    }

    #[rstest]
    #[tokio::test]
    async fn test_load_script_invalid_syntax() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test with empty filename
        let result = registry.execute(
            "\\i",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_load_script_nonexistent_file() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test loading from non-existent file
        let result = registry.execute(
            "\\i /path/to/nonexistent/file.sql",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify script remains empty
        assert!(last_script.is_empty());
    }

    // ===================
    // Phase 3: Complex Operations
    // ===================

    #[rstest]
    #[tokio::test]
    async fn test_ecopy_command_no_plan() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test ecopy when no EXPLAIN plan is available
        let result = registry.execute(
            "\\ecopy",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Note: We can't easily test the actual clipboard functionality in unit tests
        // as it requires system integration, but we can verify the command runs
    }

    // Note: \ed (multiline edit) command is skipped in unit tests as it launches 
    // external editor which would cause tests to hang. This is integration-tested separately.

    #[rstest]
    #[tokio::test]
    async fn test_database_list_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test list databases command
        let result = registry.execute(
            "\\l",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Note: In test mode, the database is mocked and this will show appropriate
        // test behavior (likely no databases or test databases)
    }

    #[rstest]
    #[tokio::test]
    async fn test_database_list_tables_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test list tables command
        let result = registry.execute(
            "\\dt",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_database_describe_table_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test describe table command without table name (should list all tables)
        let result = registry.execute(
            "\\d",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_database_describe_specific_table_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test describe specific table command
        let result = registry.execute(
            "\\d users",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_database_connect_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test connect to database command
        let result = registry.execute(
            "\\c testdb",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_database_connect_invalid_syntax() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test connect command with empty database name
        let result = registry.execute(
            "\\c",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    // ===================
    // Phase 4: Error Handling and Edge Cases
    // ===================

    #[rstest]
    #[tokio::test]
    async fn test_command_with_leading_whitespace() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test command with leading whitespace
        let result = registry.execute(
            "   \\h   ",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_command_with_arguments_whitespace() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test command with arguments containing whitespace
        let result = registry.execute(
            "\\ns   test_whitespace   SELECT * FROM users WHERE name = 'test'   ",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the query was saved correctly
        assert!(config.named_queries.contains_key("test_whitespace"));
        assert_eq!(config.named_queries.get("test_whitespace").unwrap(), "SELECT * FROM users WHERE name = 'test'");
    }

    #[rstest]
    #[tokio::test]
    async fn test_empty_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test empty command
        let result = registry.execute(
            "",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_backslash_only_command() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test just backslash command
        let result = registry.execute(
            "\\",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
    }

    #[rstest]
    #[tokio::test]
    async fn test_concurrent_access_simulation() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test multiple commands to ensure proper state management
        let commands = vec!["\\x", "\\e", "\\x", "\\e", "\\config"];
        
        for cmd in commands {
            let result = registry.execute(
                cmd,
                &database,
                &mut config,
                &mut last_script,
                &interrupt_flag,
                &mut prompt,
            ).await;

            assert!(result.is_ok());
            assert_eq!(result.unwrap(), false);
        }
    }

    #[rstest]
    #[tokio::test]
    async fn test_named_query_with_special_characters() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = String::new();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test named query with special characters
        let result = registry.execute(
            "\\ns special_chars_query SELECT * FROM users WHERE name LIKE '%test%' AND id > 10",
            &database,
            &mut config,
            &mut last_script,
            &interrupt_flag,
            &mut prompt,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), false);
        
        // Verify the query was saved correctly
        assert!(config.named_queries.contains_key("special_chars_query"));
        assert_eq!(config.named_queries.get("special_chars_query").unwrap(), "SELECT * FROM users WHERE name LIKE '%test%' AND id > 10");
    }

    #[rstest]
    #[tokio::test]
    async fn test_all_commands_return_continue() {
        let registry = create_command_registry();
        let database = create_test_database();
        let mut config = create_test_config();
        let mut last_script = "SELECT 1;".to_string();
        let interrupt_flag = Arc::new(AtomicBool::new(false));
        let mut prompt = create_test_prompt();

        // Test that all commands except \q return false (continue)
        // Note: \ed is excluded as it launches external editor
        let continue_commands = vec![
            "\\h", "\\l", "\\dt", "\\d", "\\d users", "\\c testdb", "\\x", "\\e", 
            "\\w /tmp/test.sql", "\\i /tmp/test.sql", "\\ecopy", "\\config", 
            "\\n", "\\ns test_all SELECT 1", "\\nd test_all"
        ];
        
        for cmd in continue_commands {
            let result = registry.execute(
                cmd,
                &database,
                &mut config,
                &mut last_script,
                &interrupt_flag,
                &mut prompt,
            ).await;

            assert!(result.is_ok(), "Command '{}' should succeed", cmd);
            assert_eq!(result.unwrap(), false, "Command '{}' should return false (continue)", cmd);
        }
    }
}