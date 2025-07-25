//! Type-safe enum-based command system with traits for compile-time validation
//! This replaces the string-based BackslashCommandRegistry with a robust type system

use crate::config::Config as DbCrustConfig;
use crate::db::Database;
use crate::prompt::DbPrompt;
use std::error::Error as StdError;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use thiserror::Error;
use strum::{EnumIter, IntoEnumIterator, Display};

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // Core commands
    Quit,
    Help,
    
    // Database navigation
    ListDatabases,
    ListTables,
    DescribeTable { table_name: Option<String> },
    ConnectDatabase { database_name: String },
    
    // Display options
    ToggleExpandedDisplay,
    ToggleExplainMode,
    ShowConfig,
    
    // Script handling
    WriteScript { filename: String },
    LoadScript { filename: String },
    EditMultiline,
    CopyExplainPlan,
    
    // Named queries
    ListNamedQueries,
    SaveNamedQuery { name: String, query: String },
    DeleteNamedQuery { name: String },
    ExecuteNamedQuery { name: String, args: Vec<String> },
    
    // Session management
    ListSessions,
    SaveSession { name: String },
    DeleteSession { name: String },
    ConnectSession { name: String },
    
    // Connection history
    ListRecentConnections,
    ClearRecentConnections,
    
    // Advanced commands (future expansion)
    SetMultilineIndicator { indicator: String },
    TogglePager,
    ToggleBanner,
    ToggleAutocomplete,
    ToggleColumnSelection,
    SetColumnSelectionThreshold { threshold: usize },
    ClearColumnViews,
    ResetView,
    
    // Vault credential caching commands
    VaultCacheStatus,
    VaultCacheClear,
    VaultCacheRefresh { role: Option<String> },
    VaultCacheExpired,
    
    // Database-specific commands
    ListUsers,
    ListIndexes,
    ListPragmas,
    ShowPgpass,
    ShowMyconf,
    ListDockerContainers,
    
    // EXPLAIN variants
    ExplainRaw { query: String },
    ExplainFormatted { query: String },
    ExplainExport { query: String, filename: String },
}

#[derive(Error, Debug)]
pub enum CommandError {
    #[error("Invalid command syntax: {0}")]
    InvalidSyntax(String),
    #[error("Missing required argument: {0}")]
    MissingArgument(String),
    #[error("Database error: {0}")]
    DatabaseError(#[from] Box<dyn StdError>),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Unknown command: {0}")]
    UnknownCommand(String),
}

/// Trait for command execution with proper error handling and context
#[allow(async_fn_in_trait)]
pub trait CommandExecutor {
    async fn execute(
        &self,
        database: &Arc<Mutex<Database>>,
        config: &mut DbCrustConfig,
        last_script: &mut String,
        interrupt_flag: &Arc<AtomicBool>,
        prompt: &mut DbPrompt,
    ) -> Result<CommandResult, CommandError>;
    
    fn description(&self) -> &'static str;
    fn usage(&self) -> &'static str;
    fn category(&self) -> CommandCategory;
}

#[derive(Debug, Clone)]
pub enum CommandResult {
    Exit,
    Continue,
    Output(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EnumIter, Display)]
pub enum CommandCategory {
    Core,
    DatabaseNavigation,
    DisplayOptions,
    ScriptHandling,
    NamedQueries,
    SessionManagement,
    ConnectionHistory,
    DatabaseSpecific,
    VaultManagement,
    Advanced,
}

/// Command shortcuts with associated metadata for automatic generation
#[derive(Debug, Clone, PartialEq, EnumIter)]
pub enum CommandShortcut {
    // Core commands
    Q, H,
    // Database navigation
    L, Dt, D, C,
    // Display options
    X, E, Config,
    // Script handling
    W, I, Ed, Ecopy,
    // Named queries
    N, Ns, Nd,
    // Session management
    S, Ss, Sd,
    // Connection history
    R, Rc,
    // Database-specific commands
    Du, Di, Dp, Pgpass, Myconf, Docker,
    // EXPLAIN variants (Advanced)
    Er, Ef, Ex,
    // Advanced commands
    Setmulti, Pager, Banner, A, Cs, Csthreshold, Clrcs, Resetview,
    // Vault credential cache commands
    Vc, Vcc, Vcr, Vce,
}

impl CommandShortcut {
    /// Get the command string (with backslash prefix)
    pub fn command(&self) -> &'static str {
        match self {
            // Core commands
            CommandShortcut::Q => "\\q",
            CommandShortcut::H => "\\h",
            // Database navigation
            CommandShortcut::L => "\\l",
            CommandShortcut::Dt => "\\dt",
            CommandShortcut::D => "\\d",
            CommandShortcut::C => "\\c",
            // Display options
            CommandShortcut::X => "\\x",
            CommandShortcut::E => "\\e",
            CommandShortcut::Config => "\\config",
            // Script handling
            CommandShortcut::W => "\\w",
            CommandShortcut::I => "\\i",
            CommandShortcut::Ed => "\\ed",
            CommandShortcut::Ecopy => "\\ecopy",
            // Named queries
            CommandShortcut::N => "\\n",
            CommandShortcut::Ns => "\\ns",
            CommandShortcut::Nd => "\\nd",
            // Session management
            CommandShortcut::S => "\\s",
            CommandShortcut::Ss => "\\ss",
            CommandShortcut::Sd => "\\sd",
            // Connection history
            CommandShortcut::R => "\\r",
            CommandShortcut::Rc => "\\rc",
            // Database-specific commands
            CommandShortcut::Du => "\\du",
            CommandShortcut::Di => "\\di",
            CommandShortcut::Dp => "\\dp",
            CommandShortcut::Pgpass => "\\pgpass",
            CommandShortcut::Myconf => "\\myconf",
            CommandShortcut::Docker => "\\docker",
            // EXPLAIN variants (Advanced)
            CommandShortcut::Er => "\\er",
            CommandShortcut::Ef => "\\ef",
            CommandShortcut::Ex => "\\ex",
            // Advanced commands
            CommandShortcut::Setmulti => "\\setmulti",
            CommandShortcut::Pager => "\\pager",
            CommandShortcut::Banner => "\\banner",
            CommandShortcut::A => "\\a",
            CommandShortcut::Cs => "\\cs",
            CommandShortcut::Csthreshold => "\\csthreshold",
            CommandShortcut::Clrcs => "\\clrcs",
            CommandShortcut::Resetview => "\\resetview",
            // Vault credential cache commands
            CommandShortcut::Vc => "\\vc",
            CommandShortcut::Vcc => "\\vcc",
            CommandShortcut::Vcr => "\\vcr",
            CommandShortcut::Vce => "\\vce",
        }
    }

    /// Get the command description
    pub fn description(&self) -> &'static str {
        match self {
            // Core commands
            CommandShortcut::Q => "Quit dbcrust",
            CommandShortcut::H => "Show help",
            // Database navigation
            CommandShortcut::L => "List databases",
            CommandShortcut::Dt => "List tables",
            CommandShortcut::D => "Describe table or list all tables",
            CommandShortcut::C => "Connect to database",
            // Display options
            CommandShortcut::X => "Toggle expanded display",
            CommandShortcut::E => "Toggle EXPLAIN mode",
            CommandShortcut::Config => "Show configuration",
            // Script handling
            CommandShortcut::W => "Write script to file",
            CommandShortcut::I => "Load script from file",
            CommandShortcut::Ed => "Edit multiline script",
            CommandShortcut::Ecopy => "Copy EXPLAIN plan to clipboard",
            // Named queries
            CommandShortcut::N => "List or execute named queries",
            CommandShortcut::Ns => "Save named query",
            CommandShortcut::Nd => "Delete named query",
            // Session management
            CommandShortcut::S => "List or connect to sessions",
            CommandShortcut::Ss => "Save session",
            CommandShortcut::Sd => "Delete session",
            // Connection history
            CommandShortcut::R => "List recent connections",
            CommandShortcut::Rc => "Clear recent connections",
            // Database-specific commands
            CommandShortcut::Du => "List users",
            CommandShortcut::Di => "List indexes",
            CommandShortcut::Dp => "List pragmas",
            CommandShortcut::Pgpass => "Show .pgpass info",
            CommandShortcut::Myconf => "Show .my.cnf info",
            CommandShortcut::Docker => "List Docker containers",
            // EXPLAIN variants (Advanced)
            CommandShortcut::Er => "Run EXPLAIN query in raw format",
            CommandShortcut::Ef => "Run EXPLAIN query in formatted output",
            CommandShortcut::Ex => "Export EXPLAIN result to file",
            // Advanced commands
            CommandShortcut::Setmulti => "Set multiline prompt indicator",
            CommandShortcut::Pager => "Toggle pager for long output",
            CommandShortcut::Banner => "Toggle banner display",
            CommandShortcut::A => "Toggle autocomplete",
            CommandShortcut::Cs => "Toggle column selection",
            CommandShortcut::Csthreshold => "Set column selection threshold",
            CommandShortcut::Clrcs => "Clear column views",
            CommandShortcut::Resetview => "Reset view",
            // Vault credential cache commands
            CommandShortcut::Vc => "Show vault credential cache status",
            CommandShortcut::Vcc => "Clear all cached vault credentials",
            CommandShortcut::Vcr => "Force refresh vault credentials",
            CommandShortcut::Vce => "Show expired vault credentials",
        }
    }

    /// Get the command category
    pub fn category(&self) -> CommandCategory {
        match self {
            // Core commands
            CommandShortcut::Q | CommandShortcut::H => CommandCategory::Core,
            // Database navigation
            CommandShortcut::L | CommandShortcut::Dt | CommandShortcut::D | CommandShortcut::C => CommandCategory::DatabaseNavigation,
            // Display options (including some advanced display commands)
            CommandShortcut::X | CommandShortcut::E | CommandShortcut::Config | CommandShortcut::Setmulti | CommandShortcut::Pager | CommandShortcut::Banner | CommandShortcut::A | CommandShortcut::Cs | CommandShortcut::Csthreshold | CommandShortcut::Clrcs | CommandShortcut::Resetview => CommandCategory::DisplayOptions,
            // Script handling
            CommandShortcut::W | CommandShortcut::I | CommandShortcut::Ed | CommandShortcut::Ecopy => CommandCategory::ScriptHandling,
            // Named queries
            CommandShortcut::N | CommandShortcut::Ns | CommandShortcut::Nd => CommandCategory::NamedQueries,
            // Session management
            CommandShortcut::S | CommandShortcut::Ss | CommandShortcut::Sd => CommandCategory::SessionManagement,
            // Connection history
            CommandShortcut::R | CommandShortcut::Rc => CommandCategory::ConnectionHistory,
            // Database-specific commands
            CommandShortcut::Du | CommandShortcut::Di | CommandShortcut::Dp | CommandShortcut::Pgpass | CommandShortcut::Myconf | CommandShortcut::Docker => CommandCategory::DatabaseSpecific,
            // Vault management
            CommandShortcut::Vc | CommandShortcut::Vcc | CommandShortcut::Vcr | CommandShortcut::Vce => CommandCategory::VaultManagement,
            // EXPLAIN variants (Advanced)
            CommandShortcut::Er | CommandShortcut::Ef | CommandShortcut::Ex => CommandCategory::Advanced,
        }
    }
}

/// Parser for converting string commands to typed Command enums
pub struct CommandParser;

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandParser {
    pub fn new() -> Self {
        Self
    }
    
    /// Parse a string command into a typed Command enum
    pub fn parse(input: &str) -> Result<Command, CommandError> {
        let trimmed = input.trim();
        if !trimmed.starts_with('\\') {
            return Err(CommandError::InvalidSyntax("Commands must start with '\\'".to_string()));
        }
        
        let mut parts = trimmed[1..].splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("").trim();
        
        match cmd {
            // Core commands
            "q" => Ok(Command::Quit),
            "h" => Ok(Command::Help),
            
            // Database navigation
            "l" => Ok(Command::ListDatabases),
            "dt" => Ok(Command::ListTables),
            "d" => {
                if args.is_empty() {
                    Ok(Command::DescribeTable { table_name: None })
                } else {
                    Ok(Command::DescribeTable { table_name: Some(args.to_string()) })
                }
            },
            "c" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("database name".to_string()))
                } else {
                    Ok(Command::ConnectDatabase { database_name: args.to_string() })
                }
            },
            
            // Display options
            "x" => Ok(Command::ToggleExpandedDisplay),
            "e" => Ok(Command::ToggleExplainMode),
            "config" => Ok(Command::ShowConfig),
            
            // Script handling
            "w" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("filename".to_string()))
                } else {
                    Ok(Command::WriteScript { filename: args.to_string() })
                }
            },
            "i" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("filename".to_string()))
                } else {
                    Ok(Command::LoadScript { filename: args.to_string() })
                }
            },
            "ed" => Ok(Command::EditMultiline),
            "ecopy" => Ok(Command::CopyExplainPlan),
            
            // Named queries
            "n" => {
                if args.is_empty() {
                    Ok(Command::ListNamedQueries)
                } else {
                    // Parse \n <name> [args...] for execution
                    let mut name_parts = args.splitn(2, ' ');
                    let name = name_parts.next().unwrap().to_string();
                    let exec_args = name_parts.next()
                        .map(|s| s.split_whitespace().map(|s| s.to_string()).collect())
                        .unwrap_or_default();
                    Ok(Command::ExecuteNamedQuery { name, args: exec_args })
                }
            },
            "ns" => {
                let mut ns_parts = args.splitn(2, ' ');
                let name = ns_parts.next()
                    .ok_or_else(|| CommandError::MissingArgument("query name".to_string()))?;
                let query = ns_parts.next()
                    .ok_or_else(|| CommandError::MissingArgument("query".to_string()))?;
                Ok(Command::SaveNamedQuery { 
                    name: name.to_string(), 
                    query: query.to_string() 
                })
            },
            "nd" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("query name".to_string()))
                } else {
                    Ok(Command::DeleteNamedQuery { name: args.to_string() })
                }
            },
            
            // Session management
            "s" => {
                if args.is_empty() {
                    Ok(Command::ListSessions)
                } else {
                    Ok(Command::ConnectSession { name: args.to_string() })
                }
            },
            "ss" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("session name".to_string()))
                } else {
                    Ok(Command::SaveSession { name: args.to_string() })
                }
            },
            "sd" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("session name".to_string()))
                } else {
                    Ok(Command::DeleteSession { name: args.to_string() })
                }
            },
            
            // Connection history
            "r" => Ok(Command::ListRecentConnections),
            "rc" => Ok(Command::ClearRecentConnections),
            
            // Database-specific commands
            "du" => Ok(Command::ListUsers),
            "di" => Ok(Command::ListIndexes),
            "dp" => Ok(Command::ListPragmas),
            "pgpass" => Ok(Command::ShowPgpass),
            "myconf" => Ok(Command::ShowMyconf),
            "docker" => Ok(Command::ListDockerContainers),
            
            // EXPLAIN variants
            "er" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("query".to_string()))
                } else {
                    Ok(Command::ExplainRaw { query: args.to_string() })
                }
            },
            "ef" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("query".to_string()))
                } else {
                    Ok(Command::ExplainFormatted { query: args.to_string() })
                }
            },
            "ex" => {
                // Split on the last space to separate query from filename
                if let Some(last_space_pos) = args.rfind(' ') {
                    let query = &args[..last_space_pos];
                    let filename = &args[last_space_pos + 1..];
                    if query.is_empty() {
                        Err(CommandError::MissingArgument("query".to_string()))
                    } else if filename.is_empty() {
                        Err(CommandError::MissingArgument("filename".to_string()))
                    } else {
                        Ok(Command::ExplainExport { 
                            query: query.to_string(), 
                            filename: filename.to_string() 
                        })
                    }
                } else {
                    Err(CommandError::MissingArgument("filename".to_string()))
                }
            },
            
            // Advanced commands
            "setmulti" => Ok(Command::SetMultilineIndicator { 
                indicator: args.to_string() 
            }),
            "pager" => Ok(Command::TogglePager),
            "banner" => Ok(Command::ToggleBanner),
            "a" => Ok(Command::ToggleAutocomplete),
            "cs" => Ok(Command::ToggleColumnSelection),
            "csthreshold" => {
                let threshold = args.parse::<usize>()
                    .map_err(|_| CommandError::InvalidSyntax("Invalid threshold number".to_string()))?;
                Ok(Command::SetColumnSelectionThreshold { threshold })
            },
            "clrcs" => Ok(Command::ClearColumnViews),
            "resetview" => Ok(Command::ResetView),
            
            // Vault credential cache commands
            "vc" => Ok(Command::VaultCacheStatus),
            "vcc" => Ok(Command::VaultCacheClear),
            "vcr" => {
                let role = if args.is_empty() { None } else { Some(args.to_string()) };
                Ok(Command::VaultCacheRefresh { role })
            },
            "vce" => Ok(Command::VaultCacheExpired),
            
            _ => Err(CommandError::UnknownCommand(cmd.to_string())),
        }
    }
    
    /// Get all available command names for autocomplete - automatically derived using strum
    pub fn get_command_names() -> Vec<&'static str> {
        CommandShortcut::iter()
            .map(|shortcut| shortcut.command())
            .collect()
    }
    
    /// Get commands grouped by category for help display - automatically derived using strum
    pub fn get_commands_by_category() -> Vec<(CommandCategory, Vec<(&'static str, &'static str)>)> {
        use std::collections::HashMap;
        
        // Group commands by category using strum iteration
        let mut categories: HashMap<CommandCategory, Vec<(&'static str, &'static str)>> = HashMap::new();
        
        // Process all command shortcuts using automatic iteration
        for shortcut in CommandShortcut::iter() {
            let category = shortcut.category();
            let cmd = shortcut.command();
            let desc = shortcut.description();
            categories.entry(category).or_default().push((cmd, desc));
        }
        
        // Return in a deterministic order using strum iteration over categories
        CommandCategory::iter()
            .filter_map(|cat| {
                categories.get(&cat).map(|commands| {
                    let mut sorted_commands = commands.clone();
                    sorted_commands.sort_by_key(|(cmd, _)| *cmd);
                    (cat, sorted_commands)
                })
            })
            .collect()
    }
}

impl CommandExecutor for Command {
    async fn execute(
        &self,
        database: &Arc<Mutex<Database>>,
        config: &mut DbCrustConfig,
        last_script: &mut String,
        _interrupt_flag: &Arc<AtomicBool>,
        prompt: &mut DbPrompt,
    ) -> Result<CommandResult, CommandError> {
        match self {
            Command::Quit => Ok(CommandResult::Exit),
            Command::Help => {
                let help_text = generate_help_text();
                Ok(CommandResult::Output(help_text))
            }
            
            Command::ToggleExpandedDisplay => {
                let mut db = database.lock().unwrap();
                db.toggle_expanded_display();
                let status = if db.is_expanded_display() { "on" } else { "off" };
                Ok(CommandResult::Output(format!("Expanded display is {status}.")))
            }
            
            Command::ToggleExplainMode => {
                let mut db = database.lock().unwrap();
                db.toggle_explain_mode();
                let status = if db.is_explain_mode() { "on" } else { "off" };
                Ok(CommandResult::Output(format!("Explain mode is {status}.")))
            }
            
            Command::ShowConfig => {
                let output = format!("Configuration:\n{config:#?}");
                Ok(CommandResult::Output(output))
            }
            
            Command::ListDatabases => {
                let mut db = database.lock().unwrap();
                match db.list_databases().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No databases found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list databases: {e}"))),
                }
            }
            
            Command::ListTables => {
                let mut db = database.lock().unwrap();
                match db.list_tables().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No tables found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list tables: {e}"))),
                }
            }
            
            Command::DescribeTable { table_name } => {
                let mut db = database.lock().unwrap();
                match table_name {
                    Some(name) => {
                        match db.get_table_details(name).await {
                            Ok(details) => {
                                let output = crate::format::format_table_details(&details);
                                Ok(CommandResult::Output(output))
                            }
                            Err(e) => Ok(CommandResult::Error(format!("Failed to describe table '{name}': {e}"))),
                        }
                    }
                    None => {
                        // List all tables when no table name provided
                        match db.list_tables().await {
                            Ok(results) => {
                                if results.is_empty() {
                                    Ok(CommandResult::Output("No tables found.".to_string()))
                                } else {
                                    let output = if db.is_expanded_display() {
                                        let tables = crate::format::format_query_results_expanded(&results);
                                        tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                                    } else {
                                        crate::format::format_query_results_psql(&results)
                                    };
                                    Ok(CommandResult::Output(output))
                                }
                            }
                            Err(e) => Ok(CommandResult::Error(format!("Failed to list tables: {e}"))),
                        }
                    }
                }
            }
            
            Command::ConnectDatabase { database_name } => {
                let mut db = database.lock().unwrap();
                match db.connect_to_db(database_name).await {
                    Ok(_) => {
                        // Update prompt with new database name
                        let username = db.get_username().to_string();
                        let new_db_name = db.get_current_db();
                        *prompt = DbPrompt::with_config(
                            username,
                            new_db_name.clone(),
                            config.multiline_prompt_indicator.clone(),
                        );
                        Ok(CommandResult::Output(format!("Connected to database '{new_db_name}'.")))
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to connect to database '{database_name}': {e}"))),
                }
            }
            
            Command::ListSessions => {
                let sessions = config.list_sessions();
                if sessions.is_empty() {
                    Ok(CommandResult::Output("No saved sessions found. Use \\ss <name> to save a session.".to_string()))
                } else {
                    let mut output = String::new();
                    output.push_str("Saved sessions:\n");
                    for (name, session) in sessions.iter() {
                        let db_type = match session.database_type {
                            crate::database::DatabaseType::PostgreSQL => "PostgreSQL",
                            crate::database::DatabaseType::MySQL => "MySQL", 
                            crate::database::DatabaseType::SQLite => "SQLite",
                        };
                        if session.database_type == crate::database::DatabaseType::SQLite {
                            if let Some(ref file_path) = session.file_path {
                                output.push_str(&format!("  {name} - {file_path} ({db_type})\n"));
                            } else {
                                output.push_str(&format!("  {name} - SQLite (no path)\n"));
                            }
                        } else {
                            output.push_str(&format!("  {} - {}@{}:{}/{} ({})\n", 
                                name, session.user, session.host, session.port, session.dbname, db_type));
                        }
                    }
                    Ok(CommandResult::Output(output))
                }
            }
            
            Command::ListRecentConnections => {
                let recent = config.get_recent_connections();
                if recent.is_empty() {
                    Ok(CommandResult::Output("No recent connections found.".to_string()))
                } else {
                    let mut output = String::new();
                    output.push_str("Recent connections:\n");
                    for (i, conn) in recent.iter().take(20).enumerate() {
                        let status = if conn.success { "✅" } else { "❌" };
                        let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
                        let db_type = match conn.database_type {
                            crate::database::DatabaseType::PostgreSQL => "PostgreSQL",
                            crate::database::DatabaseType::MySQL => "MySQL",
                            crate::database::DatabaseType::SQLite => "SQLite",
                        };
                        output.push_str(&format!("  {}: {} {} - {} ({})\n", 
                            i + 1, status, conn.display_name, timestamp, db_type));
                    }
                    Ok(CommandResult::Output(output))
                }
            }
            
            Command::ClearRecentConnections => {
                if let Err(e) = config.clear_recent_connections() {
                    Ok(CommandResult::Error(format!("Failed to clear recent connections: {e}")))
                } else {
                    Ok(CommandResult::Output("Recent connections cleared.".to_string()))
                }
            }

            Command::SaveSession { name } => {
                // Extract connection info from database and use the proper save method
                let db = database.lock().unwrap();
                let connection_info = match db.get_connection_info() {
                    Some(info) => info,
                    None => {
                        return Ok(CommandResult::Error(
                            "Cannot save session: connection information not available. This may happen with certain connection types.".to_string()
                        ));
                    }
                };

                match config.save_session_from_connection_info(name, connection_info) {
                    Ok(_) => Ok(CommandResult::Output(format!("Session '{name}' saved successfully."))),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to save session '{name}': {e}"))),
                }
            }

            Command::DeleteSession { name } => {
                match config.delete_session(name) {
                    Ok(_) => Ok(CommandResult::Output(format!("Session '{name}' deleted successfully."))),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to delete session '{name}': {e}"))),
                }
            }

            Command::ConnectSession { name } => {
                // This would require full connection logic - for now show available info
                match config.get_session(name) {
                    Some(session) => {
                        Ok(CommandResult::Output(format!("Session '{}' found: {}@{}:{}/{}", 
                            name, session.user, session.host, session.port, session.dbname)))
                    }
                    None => {
                        Ok(CommandResult::Error(format!("Session '{name}' not found. Use \\s to list available sessions.")))
                    }
                }
            }

            Command::ListNamedQueries => {
                let named_queries = config.list_named_queries();
                if named_queries.is_empty() {
                    Ok(CommandResult::Output("No named queries found. Use \\ns <name> <query> to save a query.".to_string()))
                } else {
                    let mut output = String::new();
                    output.push_str("Named queries:\n");
                    for (name, query) in named_queries.iter() {
                        let preview = if query.len() > 60 {
                            format!("{}...", &query[..57])
                        } else {
                            query.clone()
                        };
                        output.push_str(&format!("  {name} - {preview}\n"));
                    }
                    Ok(CommandResult::Output(output))
                }
            }

            Command::SaveNamedQuery { name, query } => {
                match config.add_named_query(name, query) {
                    Ok(_) => Ok(CommandResult::Output(format!("Named query '{name}' saved successfully."))),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to save named query '{name}': {e}"))),
                }
            }

            Command::DeleteNamedQuery { name } => {
                match config.delete_named_query(name) {
                    Ok(_) => Ok(CommandResult::Output(format!("Named query '{name}' deleted successfully."))),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to delete named query '{name}': {e}"))),
                }
            }

            Command::ExecuteNamedQuery { name, args } => {
                match config.get_named_query(name) {
                    Some(query_template) => {
                        let mut db = database.lock().unwrap();
                        // Apply parameter substitution
                        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                        let final_query = crate::named_queries::process_query(query_template, &args_refs);
                        
                        // Execute the query
                        match db.execute_query(&final_query).await {
                            Ok(results) => {
                                if results.is_empty() {
                                    Ok(CommandResult::Output("Query executed successfully (no results).".to_string()))
                                } else {
                                    let output = if db.is_expanded_display() {
                                        let tables = crate::format::format_query_results_expanded(&results);
                                        tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                                    } else {
                                        crate::format::format_query_results_psql(&results)
                                    };
                                    Ok(CommandResult::Output(output))
                                }
                            }
                            Err(e) => Ok(CommandResult::Error(format!("Error executing named query '{name}': {e}"))),
                        }
                    }
                    None => Ok(CommandResult::Error(format!("Named query '{name}' not found."))),
                }
            }

            Command::WriteScript { filename } => {
                if last_script.is_empty() {
                    Ok(CommandResult::Error("No script content to write. Use \\ed to edit a script first.".to_string()))
                } else {
                    match std::fs::write(filename, last_script) {
                        Ok(_) => Ok(CommandResult::Output(format!("Script saved to '{filename}'."))),
                        Err(e) => Ok(CommandResult::Error(format!("Failed to write script to '{filename}': {e}"))),
                    }
                }
            }

            Command::LoadScript { filename } => {
                match std::fs::read_to_string(filename) {
                    Ok(content) => {
                        *last_script = content.clone();
                        let line_count = content.lines().count();
                        Ok(CommandResult::Output(format!(
                            "Script loaded from '{filename}' ({} lines). Press Enter to execute, \\ed to edit, or \\w to save elsewhere.", 
                            line_count
                        )))
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to load script from '{filename}': {e}"))),
                }
            }

            Command::EditMultiline => {
                // Launch external editor with current script content
                match crate::script::edit_multiline_script(last_script) {
                    Ok(edited_content) => {
                        *last_script = edited_content.clone();
                        if edited_content.trim().is_empty() {
                            Ok(CommandResult::Output("Editor closed with empty content.".to_string()))
                        } else {
                            let line_count = edited_content.lines().count();
                            Ok(CommandResult::Output(format!(
                                "Script edited ({} lines). Execute it by pressing Enter, or save with \\w filename", 
                                line_count
                            )))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Editor error: {}", e)))
                }
            }

            Command::ListUsers => {
                let mut db = database.lock().unwrap();
                match db.list_users().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No users found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list users: {e}"))),
                }
            }

            Command::ListIndexes => {
                let mut db = database.lock().unwrap();
                match db.list_indexes().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No indexes found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list indexes: {e}"))),
                }
            }

            Command::ShowPgpass => {
                match crate::pgpass::get_pgpass_path() {
                    Some(path) => {
                        let exists = std::path::Path::new(&path).exists();
                        Ok(CommandResult::Output(format!("PostgreSQL .pgpass file: {} (exists: {})", path.to_string_lossy(), exists)))
                    }
                    None => Ok(CommandResult::Output("No .pgpass file path configured.".to_string())),
                }
            }

            Command::ShowMyconf => {
                match crate::myconf::get_mysql_config_path() {
                    Some(path) => {
                        let exists = std::path::Path::new(&path).exists();
                        Ok(CommandResult::Output(format!("MySQL .my.cnf file: {} (exists: {})", path.to_string_lossy(), exists)))
                    }
                    None => Ok(CommandResult::Output("No .my.cnf file path configured.".to_string())),
                }
            }

            Command::CopyExplainPlan => {
                use arboard::Clipboard;
                let db = database.lock().unwrap();
                match db.get_last_json_plan() {
                    Some(json_plan) => {
                        match Clipboard::new() {
                            Ok(mut clipboard) => {
                                match clipboard.set_text(json_plan.clone()) {
                                    Ok(()) => {
                                        Ok(CommandResult::Output(format!("EXPLAIN JSON plan copied to clipboard ({} characters)", json_plan.len())))
                                    }
                                    Err(e) => {
                                        Ok(CommandResult::Error(format!("Error copying to clipboard: {e}")))
                                    }
                                }
                            }
                            Err(e) => {
                                Ok(CommandResult::Error(format!("Error accessing clipboard: {e}")))
                            }
                        }
                    }
                    None => {
                        Ok(CommandResult::Error("No EXPLAIN JSON plan available. Run an EXPLAIN query first.".to_string()))
                    }
                }
            }

            Command::ListPragmas => {
                let mut db = database.lock().unwrap();
                match db.list_pragmas().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No pragmas found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list pragmas: {e}"))),
                }
            }

            Command::ListDockerContainers => {
                match crate::docker::DockerClient::new() {
                    Ok(docker_client) => {
                        match docker_client.list_database_containers().await {
                            Ok(containers) => {
                                if containers.is_empty() {
                                    Ok(CommandResult::Output("No database containers found.".to_string()))
                                } else {
                                    let output = containers.iter()
                                        .map(|c| {
                                            let db_type = c.database_type.as_ref()
                                                .map(|dt| dt.to_string())
                                                .unwrap_or_else(|| "Unknown".to_string());
                                            format!("{} ({})", c.name, db_type)
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    Ok(CommandResult::Output(format!("Available database containers:\n{output}")))
                                }
                            }
                            Err(e) => Ok(CommandResult::Error(format!("Failed to list Docker containers: {e}"))),
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to connect to Docker: {e}"))),
                }
            }

            Command::ExplainRaw { query } => {
                let mut db = database.lock().unwrap();
                match db.execute_explain_query_raw(query).await {
                    Ok(results) => {
                        let output = crate::format::format_query_results_psql(&results);
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to explain query: {e}"))),
                }
            }

            Command::ExplainFormatted { query } => {
                let mut db = database.lock().unwrap();
                // Use execute_explain_query_formatted which provides the same output as explain mode
                // and automatically stores the JSON plan for \ecopy
                match db.execute_explain_query_formatted(query).await {
                    Ok(results) => {
                        let output = if db.is_expanded_display() {
                            let tables = crate::format::format_query_results_expanded(&results);
                            tables.into_iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n")
                        } else {
                            crate::format::format_query_results_psql(&results)
                        };
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to explain query: {e}"))),
                }
            }

            Command::ExplainExport { query, filename } => {
                let mut db = database.lock().unwrap();
                match db.execute_explain_query_formatted(query).await {
                    Ok(results) => {
                        let output = crate::format::format_query_results_psql(&results);
                        match std::fs::write(filename, &output) {
                            Ok(_) => Ok(CommandResult::Output(format!("EXPLAIN results exported to {filename}"))),
                            Err(e) => Ok(CommandResult::Error(format!("Failed to write to {filename}: {e}"))),
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to explain query: {e}"))),
                }
            }

            Command::SetMultilineIndicator { indicator } => {
                config.multiline_prompt_indicator = indicator.clone();
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                Ok(CommandResult::Output(format!("Multiline indicator set to: {indicator} (will take effect on next restart)")))
            }

            Command::TogglePager => {
                config.pager_enabled = !config.pager_enabled;
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.pager_enabled { "enabled" } else { "disabled" };
                Ok(CommandResult::Output(format!("Pager is now {status}.")))
            }

            Command::ToggleBanner => {
                config.show_banner = !config.show_banner;
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.show_banner { "enabled" } else { "disabled" };
                Ok(CommandResult::Output(format!("Banner is now {status}.")))
            }

            Command::ToggleAutocomplete => {
                config.autocomplete_enabled = !config.autocomplete_enabled;
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.autocomplete_enabled { "enabled" } else { "disabled" };
                Ok(CommandResult::Output(format!("Autocomplete is now {status}.")))
            }

            Command::ToggleColumnSelection => {
                config.column_selection_mode_default = !config.column_selection_mode_default;
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.column_selection_mode_default { "enabled" } else { "disabled" };
                Ok(CommandResult::Output(format!("Column selection is now {status}.")))
            }

            Command::SetColumnSelectionThreshold { threshold } => {
                config.column_selection_threshold = *threshold;
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                Ok(CommandResult::Output(format!("Column selection threshold set to: {threshold}")))
            }

            Command::ClearColumnViews => {
                let mut db = database.lock().unwrap();
                db.clear_column_views();
                Ok(CommandResult::Output("Column views cleared.".to_string()))
            }

            Command::ResetView => {
                let mut db = database.lock().unwrap();
                if db.is_explain_mode() {
                    db.toggle_explain_mode();
                }
                if db.is_expanded_display() {
                    db.toggle_expanded_display();
                }
                db.reset_column_view();
                config.explain_mode_default = false;
                config.expanded_display_default = false;
                config.save().map_err(|e| CommandError::DatabaseError(e.into()))?;
                Ok(CommandResult::Output("View settings reset to defaults.".to_string()))
            }

            // Vault credential cache commands
            Command::VaultCacheStatus => {
                if !config.vault_credential_cache_enabled {
                    return Ok(CommandResult::Output("Vault credential caching is disabled.".to_string()));
                }
                
                let cached_creds = config.list_cached_vault_credentials();
                if cached_creds.is_empty() {
                    Ok(CommandResult::Output("No vault credentials cached.".to_string()))
                } else {
                    let mut output = format!("Vault credential cache status (showing {} entries):\n", cached_creds.len());
                    let now = chrono::Utc::now();
                    
                    for (key, creds) in cached_creds {
                        let remaining_seconds = (creds.expire_time - now).num_seconds().max(0);
                        let remaining_hours = remaining_seconds / 3600;
                        let remaining_mins = (remaining_seconds % 3600) / 60;
                        
                        let status = if now >= creds.expire_time {
                            "EXPIRED"
                        } else if remaining_seconds < config.vault_cache_min_ttl_seconds as i64 {
                            "EXPIRING SOON"
                        } else {
                            "VALID"
                        };
                        
                        output.push_str(&format!(
                            "  {} ({}) - {}h{}m remaining - {}\n",
                            key, creds.username, remaining_hours, remaining_mins, status
                        ));
                    }
                    Ok(CommandResult::Output(output))
                }
            }

            Command::VaultCacheClear => {
                match config.clear_vault_credentials() {
                    Ok(()) => Ok(CommandResult::Output("All vault credentials cleared from cache.".to_string())),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to clear vault credentials: {}", e)))
                }
            }

            Command::VaultCacheRefresh { role } => {
                match role {
                    Some(role_key) => {
                        // Force refresh specific role - would need to implement role-specific refresh
                        Ok(CommandResult::Output(format!("Force refresh for role '{}' not yet implemented.", role_key)))
                    }
                    None => {
                        // Reload from file
                        config.reload_vault_credentials();
                        Ok(CommandResult::Output("Vault credential cache reloaded from file.".to_string()))
                    }
                }
            }

            Command::VaultCacheExpired => {
                let cached_creds = config.list_cached_vault_credentials();
                let now = chrono::Utc::now();
                let expired_creds: Vec<_> = cached_creds.into_iter()
                    .filter(|(_, creds)| now >= creds.expire_time)
                    .collect();
                
                if expired_creds.is_empty() {
                    Ok(CommandResult::Output("No expired vault credentials found.".to_string()))
                } else {
                    let mut output = format!("Expired vault credentials ({} entries):\n", expired_creds.len());
                    for (key, creds) in expired_creds {
                        let expired_since = (now - creds.expire_time).num_seconds();
                        let expired_hours = expired_since / 3600;
                        let expired_mins = (expired_since % 3600) / 60;
                        
                        output.push_str(&format!(
                            "  {} ({}) - expired {}h{}m ago\n",
                            key, creds.username, expired_hours, expired_mins
                        ));
                    }
                    Ok(CommandResult::Output(output))
                }
            }

        }
    }
    
    fn description(&self) -> &'static str {
        match self {
            Command::Quit => "Quit the application",
            Command::Help => "Show help information",
            Command::ListDatabases => "List all databases",
            Command::ListTables => "List tables in current database",
            Command::DescribeTable { .. } => "Describe table structure",
            Command::ConnectDatabase { .. } => "Connect to a different database",
            Command::ToggleExpandedDisplay => "Toggle expanded/vertical display mode",
            Command::ToggleExplainMode => "Toggle automatic EXPLAIN for queries",
            Command::ShowConfig => "Show current configuration",
            Command::ListSessions => "List saved sessions",
            Command::SaveSession { .. } => "Save current connection as a session",
            Command::DeleteSession { .. } => "Delete a saved session",
            Command::ConnectSession { .. } => "Connect to a saved session",
            Command::ListRecentConnections => "List recent connections",
            Command::ClearRecentConnections => "Clear recent connection history",
            Command::ListNamedQueries => "List named queries",
            Command::SaveNamedQuery { .. } => "Save a named query",
            Command::DeleteNamedQuery { .. } => "Delete a named query",
            Command::ExecuteNamedQuery { .. } => "Execute a named query",
            Command::WriteScript { .. } => "Write script to file",
            Command::LoadScript { .. } => "Load script from file",
            Command::EditMultiline => "Enter multiline edit mode",
            Command::ListUsers => "List database users",
            Command::ListIndexes => "List database indexes",
            Command::ListPragmas => "List database pragmas (SQLite)",
            Command::ShowPgpass => "Show PostgreSQL .pgpass file info",
            Command::ShowMyconf => "Show MySQL .my.cnf file info",
            Command::ListDockerContainers => "List available database containers",
            Command::CopyExplainPlan => "Copy EXPLAIN plan to clipboard",
            Command::ExplainRaw { .. } => "Execute EXPLAIN query (raw output)",
            Command::ExplainFormatted { .. } => "Execute EXPLAIN query (same as explain mode, supports \\ecopy)",
            Command::ExplainExport { .. } => "Execute EXPLAIN query and export to file",
            Command::SetMultilineIndicator { .. } => "Set custom multiline prompt indicator",
            Command::TogglePager => "Toggle pager for long output",
            Command::ToggleBanner => "Toggle startup banner display",
            Command::ToggleAutocomplete => "Toggle autocomplete functionality",
            Command::ToggleColumnSelection => "Toggle column selection mode",
            Command::SetColumnSelectionThreshold { .. } => "Set column selection threshold",
            Command::ClearColumnViews => "Clear saved column views",
            Command::ResetView => "Reset all view settings to defaults",
            // Vault credential cache commands
            Command::VaultCacheStatus => "Show vault credential cache status",
            Command::VaultCacheClear => "Clear all cached vault credentials",
            Command::VaultCacheRefresh { .. } => "Refresh vault credential cache",
            Command::VaultCacheExpired => "Show expired vault credentials",
        }
    }
    
    fn usage(&self) -> &'static str {
        match self {
            Command::Quit => "\\q",
            Command::Help => "\\h",
            Command::ListDatabases => "\\l",
            Command::ListTables => "\\dt",
            Command::DescribeTable { .. } => "\\d [table_name]",
            Command::ConnectDatabase { .. } => "\\c <database_name>",
            Command::ToggleExpandedDisplay => "\\x",
            Command::ToggleExplainMode => "\\e",
            Command::ShowConfig => "\\config",
            Command::WriteScript { .. } => "\\w <filename>",
            Command::LoadScript { .. } => "\\i <filename>",
            Command::EditMultiline => "\\ed",
            Command::SaveNamedQuery { .. } => "\\ns <name> <query>",
            Command::DeleteNamedQuery { .. } => "\\nd <name>",
            Command::ExecuteNamedQuery { .. } => "\\n <name> [args...]",
            Command::ListNamedQueries => "\\n",
            Command::ListSessions => "\\s",
            Command::SaveSession { .. } => "\\ss <name>",
            Command::DeleteSession { .. } => "\\sd <name>",
            Command::ConnectSession { .. } => "\\s <name>",
            Command::ListRecentConnections => "\\r",
            Command::ClearRecentConnections => "\\rc",
            Command::ListUsers => "\\du",
            Command::ListIndexes => "\\di",
            Command::ListPragmas => "\\dp",
            Command::ShowPgpass => "\\pgpass",
            Command::ShowMyconf => "\\myconf",
            Command::ListDockerContainers => "\\docker",
            Command::CopyExplainPlan => "\\ecopy",
            Command::ExplainRaw { .. } => "\\er <query>",
            Command::ExplainFormatted { .. } => "\\ef <query>",
            Command::ExplainExport { .. } => "\\ex <query> <filename>",
            Command::SetMultilineIndicator { .. } => "\\setmulti <indicator>",
            Command::TogglePager => "\\pager",
            Command::ToggleBanner => "\\banner",
            Command::ToggleAutocomplete => "\\a",
            Command::ToggleColumnSelection => "\\cs",
            Command::SetColumnSelectionThreshold { .. } => "\\csthreshold <number>",
            Command::ClearColumnViews => "\\clrcs",
            Command::ResetView => "\\resetview",
            // Vault credential cache commands
            Command::VaultCacheStatus => "\\vc",
            Command::VaultCacheClear => "\\vcc",
            Command::VaultCacheRefresh { .. } => "\\vcr [role]",
            Command::VaultCacheExpired => "\\vce",
        }
    }
    
    fn category(&self) -> CommandCategory {
        match self {
            Command::Quit | Command::Help => CommandCategory::Core,
            Command::ListDatabases | Command::ListTables | Command::DescribeTable { .. } | Command::ConnectDatabase { .. } => CommandCategory::DatabaseNavigation,
            Command::ToggleExpandedDisplay | Command::ToggleExplainMode | Command::ShowConfig => CommandCategory::DisplayOptions,
            Command::WriteScript { .. } | Command::LoadScript { .. } | Command::EditMultiline | Command::CopyExplainPlan => CommandCategory::ScriptHandling,
            Command::ListNamedQueries | Command::SaveNamedQuery { .. } | Command::DeleteNamedQuery { .. } | Command::ExecuteNamedQuery { .. } => CommandCategory::NamedQueries,
            Command::ListSessions | Command::SaveSession { .. } | Command::DeleteSession { .. } | Command::ConnectSession { .. } => CommandCategory::SessionManagement,
            Command::ListRecentConnections | Command::ClearRecentConnections => CommandCategory::ConnectionHistory,
            Command::ListUsers | Command::ListIndexes | Command::ListPragmas | Command::ShowPgpass | Command::ShowMyconf | Command::ListDockerContainers => CommandCategory::DatabaseSpecific,
            Command::ExplainRaw { .. } | Command::ExplainFormatted { .. } | Command::ExplainExport { .. } => CommandCategory::Advanced,
            Command::SetMultilineIndicator { .. } | Command::TogglePager | Command::ToggleBanner | Command::ToggleAutocomplete | Command::ToggleColumnSelection | Command::SetColumnSelectionThreshold { .. } | Command::ClearColumnViews | Command::ResetView => CommandCategory::DisplayOptions,
            Command::VaultCacheStatus | Command::VaultCacheClear | Command::VaultCacheRefresh { .. } | Command::VaultCacheExpired => CommandCategory::VaultManagement,
        }
    }
}

fn generate_help_text() -> String {
    let mut help = String::new();
    help.push_str("Available Commands:\n\n");
    
    for (category, commands) in CommandParser::get_commands_by_category() {
        help.push_str(&format!("{category:?}:\n"));
        for (cmd, desc) in commands {
            help.push_str(&format!("  {cmd:<12} - {desc}\n"));
        }
        help.push('\n');
    }
    
    help
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parsing() {
        // Test core commands
        assert_eq!(CommandParser::parse("\\q").unwrap(), Command::Quit);
        assert_eq!(CommandParser::parse("\\h").unwrap(), Command::Help);
        
        // Test database navigation
        assert_eq!(CommandParser::parse("\\l").unwrap(), Command::ListDatabases);
        assert_eq!(CommandParser::parse("\\dt").unwrap(), Command::ListTables);
        assert_eq!(CommandParser::parse("\\d").unwrap(), Command::DescribeTable { table_name: None });
        assert_eq!(CommandParser::parse("\\d users").unwrap(), Command::DescribeTable { table_name: Some("users".to_string()) });
        
        // Test commands with arguments
        assert_eq!(CommandParser::parse("\\c testdb").unwrap(), Command::ConnectDatabase { database_name: "testdb".to_string() });
        assert_eq!(CommandParser::parse("\\w script.sql").unwrap(), Command::WriteScript { filename: "script.sql".to_string() });
        
        // Test named queries
        assert_eq!(CommandParser::parse("\\ns test SELECT 1").unwrap(), Command::SaveNamedQuery { 
            name: "test".to_string(), 
            query: "SELECT 1".to_string() 
        });
        
        // Test error cases
        assert!(matches!(CommandParser::parse("\\c"), Err(CommandError::MissingArgument(_))));
        assert!(matches!(CommandParser::parse("\\unknown"), Err(CommandError::UnknownCommand(_))));
        assert!(matches!(CommandParser::parse("not_a_command"), Err(CommandError::InvalidSyntax(_))));
    }
    
    #[test]
    fn test_individual_command_categories() {
        assert_eq!(Command::Quit.category(), CommandCategory::Core);
        assert_eq!(Command::ListDatabases.category(), CommandCategory::DatabaseNavigation);
        assert_eq!(Command::ToggleExpandedDisplay.category(), CommandCategory::DisplayOptions);
    }
    
    #[test]
    fn test_get_command_names() {
        let names = CommandParser::get_command_names();
        assert!(names.contains(&"\\q"));
        assert!(names.contains(&"\\h"));
        assert!(names.contains(&"\\l"));
        assert!(names.contains(&"\\dt"));
        assert!(names.len() > 20); // Should have all our commands
    }

    #[test]
    fn test_command_categories() {
        let categories = CommandParser::get_commands_by_category();
        assert!(categories.len() >= 6); // We have 6+ categories
        
        // Verify each category has commands
        for (category, commands) in categories {
            assert!(!commands.is_empty(), "Category {category:?} should have commands");
        }
    }

    #[test]
    fn test_command_descriptions_and_usage() {
        let test_commands = vec![
            Command::Quit,
            Command::Help,
            Command::ListDatabases,
            Command::ListTables,
            Command::DescribeTable { table_name: Some("test".to_string()) },
            Command::SaveSession { name: "test".to_string() },
            Command::ListNamedQueries,
        ];

        for command in test_commands {
            // Verify descriptions and usage are not default
            assert_ne!(command.description(), "Command description not available");
            assert_ne!(command.usage(), "Usage not available");
            
            // Verify they have content
            assert!(!command.description().is_empty());
            assert!(!command.usage().is_empty());
        }
    }

    #[test]
    fn test_session_commands_parsing() {
        // Test session listing
        assert_eq!(CommandParser::parse("\\s").unwrap(), Command::ListSessions);
        
        // Test session connection
        assert_eq!(CommandParser::parse("\\s production").unwrap(), 
                   Command::ConnectSession { name: "production".to_string() });
        
        // Test session saving
        assert_eq!(CommandParser::parse("\\ss staging").unwrap(),
                   Command::SaveSession { name: "staging".to_string() });
        
        // Test session deletion
        assert_eq!(CommandParser::parse("\\sd old_session").unwrap(),
                   Command::DeleteSession { name: "old_session".to_string() });
    }

    #[test]
    fn test_named_query_commands() {
        // Test named query listing
        assert_eq!(CommandParser::parse("\\n").unwrap(), Command::ListNamedQueries);
        
        // Test named query execution
        assert_eq!(CommandParser::parse("\\n get_users").unwrap(),
                   Command::ExecuteNamedQuery { name: "get_users".to_string(), args: vec![] });
        
        // Test named query execution with args
        assert_eq!(CommandParser::parse("\\n get_user_by_id 123").unwrap(),
                   Command::ExecuteNamedQuery { 
                       name: "get_user_by_id".to_string(), 
                       args: vec!["123".to_string()] 
                   });
        
        // Test named query saving
        assert_eq!(CommandParser::parse("\\ns get_all SELECT * FROM users").unwrap(),
                   Command::SaveNamedQuery { 
                       name: "get_all".to_string(),
                       query: "SELECT * FROM users".to_string()
                   });
        
        // Test named query deletion
        assert_eq!(CommandParser::parse("\\nd old_query").unwrap(),
                   Command::DeleteNamedQuery { name: "old_query".to_string() });
    }

    #[test]
    fn test_database_specific_commands() {
        assert_eq!(CommandParser::parse("\\du").unwrap(), Command::ListUsers);
        assert_eq!(CommandParser::parse("\\di").unwrap(), Command::ListIndexes);
        assert_eq!(CommandParser::parse("\\pgpass").unwrap(), Command::ShowPgpass);
        assert_eq!(CommandParser::parse("\\myconf").unwrap(), Command::ShowMyconf);
    }

    #[test]
    fn test_script_commands() {
        assert_eq!(CommandParser::parse("\\w script.sql").unwrap(),
                   Command::WriteScript { filename: "script.sql".to_string() });
        
        assert_eq!(CommandParser::parse("\\i load.sql").unwrap(),
                   Command::LoadScript { filename: "load.sql".to_string() });
        
        assert_eq!(CommandParser::parse("\\ed").unwrap(), Command::EditMultiline);
        assert_eq!(CommandParser::parse("\\ecopy").unwrap(), Command::CopyExplainPlan);
    }

    #[test]
    fn test_error_cases() {
        // Missing required arguments
        assert!(matches!(CommandParser::parse("\\c"), Err(CommandError::MissingArgument(_))));
        assert!(matches!(CommandParser::parse("\\w"), Err(CommandError::MissingArgument(_))));
        assert!(matches!(CommandParser::parse("\\ss"), Err(CommandError::MissingArgument(_))));
        assert!(matches!(CommandParser::parse("\\ns test"), Err(CommandError::MissingArgument(_))));
        
        // Invalid command syntax
        assert!(matches!(CommandParser::parse("not_a_command"), Err(CommandError::InvalidSyntax(_))));
        assert!(matches!(CommandParser::parse("regular text"), Err(CommandError::InvalidSyntax(_))));
        
        // Unknown commands
        assert!(matches!(CommandParser::parse("\\unknown"), Err(CommandError::UnknownCommand(_))));
        assert!(matches!(CommandParser::parse("\\xyz"), Err(CommandError::UnknownCommand(_))));
    }

    #[test]
    fn test_advanced_commands() {
        // Test EXPLAIN variants
        assert_eq!(CommandParser::parse("\\er SELECT 1").unwrap(),
                   Command::ExplainRaw { query: "SELECT 1".to_string() });
        
        assert_eq!(CommandParser::parse("\\ef SELECT 1").unwrap(),
                   Command::ExplainFormatted { query: "SELECT 1".to_string() });
        
        assert_eq!(CommandParser::parse("\\ex SELECT 1 output.txt").unwrap(),
                   Command::ExplainExport { 
                       query: "SELECT 1".to_string(),
                       filename: "output.txt".to_string()
                   });
        
        // Test threshold setting
        assert_eq!(CommandParser::parse("\\csthreshold 50").unwrap(),
                   Command::SetColumnSelectionThreshold { threshold: 50 });
        
        // Test multiline indicator
        assert_eq!(CommandParser::parse("\\setmulti >").unwrap(),
                   Command::SetMultilineIndicator { indicator: ">".to_string() });
    }
}