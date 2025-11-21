//! Type-safe enum-based command system with traits for compile-time validation
//! This replaces the string-based BackslashCommandRegistry with a robust type system

use crate::config::{Config as DbCrustConfig, NamedQueryScope};
use crate::database::{DatabaseType, DatabaseTypeExt};
use crate::db::Database;
use crate::history_manager::SessionId;
use crate::prompt::DbPrompt;
use std::error::Error as StdError;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use strum::{Display, EnumIter, IntoEnumIterator};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // Core commands
    Quit,
    Help,

    // Database navigation
    ListDatabases,
    ListTables,
    DescribeTable {
        table_name: Option<String>,
    },
    ConnectDatabase {
        database_name: String,
    },

    // Display options
    ToggleExpandedDisplay,
    ToggleExplainMode,
    ShowConfig,

    // Script handling
    WriteScript {
        filename: String,
    },
    LoadScript {
        filename: String,
    },
    EditMultiline,
    CopyExplainPlan,

    // Named queries
    ListNamedQueries,
    SaveNamedQuery {
        name: String,
        query: String,
        global: bool,
        postgres: bool,
        mysql: bool,
        sqlite: bool,
    },
    DeleteNamedQuery {
        name: String,
    },
    ExecuteNamedQuery {
        name: String,
        args: Vec<String>,
    },

    // Session management
    ListSessions,
    SaveSession {
        name: String,
    },
    DeleteSession {
        name: String,
    },
    ConnectSession {
        name: String,
    },

    // Connection history
    ListRecentConnections,
    ClearRecentConnections,

    // History management
    ClearSessionHistory {
        session_hash: Option<String>,
    },

    // Advanced commands (future expansion)
    SetMultilineIndicator {
        indicator: String,
    },
    TogglePager,
    ToggleBanner,
    ToggleServerInfo,
    ToggleAutocomplete,
    ToggleColumnSelection,
    SetColumnSelectionThreshold {
        threshold: usize,
    },
    ClearColumnViews,
    ResetView,

    // Vector display configuration commands
    SetVectorDisplayMode {
        mode: String,
    },
    ShowVectorDisplayConfig,
    ToggleVectorStatistics,

    // Vault credential caching commands
    VaultCacheStatus,
    VaultCacheClear,
    VaultCacheRefresh {
        role: Option<String>,
    },
    VaultCacheExpired,

    // Password management commands (.dbcrust file)
    SavePassword {
        db_type: Option<String>,
        host: Option<String>,
        port: Option<u16>,
        database: Option<String>,
        username: Option<String>,
        encrypt: bool,
    },
    ListPasswords,
    DeletePassword {
        db_type: Option<String>,
        host: Option<String>,
        port: Option<u16>,
        database: Option<String>,
        username: Option<String>,
    },
    EncryptPasswords,

    // Database-specific commands
    ListUsers,
    ListIndexes,
    ListPragmas,
    ShowPgpass,
    ShowMyconf,
    ListDockerContainers,

    // MongoDB-specific commands
    ListCollections,
    DescribeCollection {
        collection_name: String,
    },
    ListMongoIndexes,
    CreateMongoIndex {
        collection: String,
        field: String,
        index_type: Option<String>,
    },
    DropMongoIndex {
        collection: String,
        index_name: String,
    },
    MongoStats,
    MongoFind {
        collection: String,
        filter: Option<String>,
        projection: Option<String>,
        limit: Option<i64>,
    },
    MongoAggregate {
        collection: String,
        pipeline: String,
    },
    MongoTextSearch {
        collection: String,
        search_term: String,
    },

    // EXPLAIN variants
    ExplainRaw {
        query: String,
    },
    ExplainFormatted {
        query: String,
    },
    ExplainExport {
        query: String,
        filename: String,
    },

    // Connection pool monitoring
    ShowPoolStats,

    // Complex display commands
    ComplexDisplayMode {
        mode: Option<String>,
    },
    ComplexDisplayJsonToggle,

    // AI SQL generation commands
    AiAuthenticate,
    AiLogout,
    AiStatus,
    AiGenerate {
        prompt: String,
    },
    AiConfig,
    AiClearCache,
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
    HistoryManagement,
    DatabaseSpecific,
    VaultManagement,
    PasswordManagement,
    Advanced,
    AiSqlGeneration,
}

/// Command shortcuts with associated metadata for automatic generation
#[derive(Debug, Clone, PartialEq, EnumIter)]
pub enum CommandShortcut {
    // Core commands
    Q,
    H,
    // Database navigation
    L,
    Dt,
    D,
    C,
    // Display options
    X,
    E,
    Config,
    // Script handling
    W,
    I,
    Ed,
    Ecopy,
    // Named queries
    N,
    Ns,
    Nd,
    // Session management
    S,
    Ss,
    Sd,
    // Connection history
    R,
    Rc,
    // History management
    Hc,
    // Database-specific commands
    Du,
    Di,
    Dp,
    Pgpass,
    Myconf,
    Docker,
    // EXPLAIN variants (Advanced)
    Er,
    Ef,
    Ex,
    // Advanced commands
    Setmulti,
    Pager,
    Banner,
    ServerInfo,
    A,
    Cs,
    Csthreshold,
    Clrcs,
    Resetview,
    // Vector display commands
    Vd,
    Vdc,
    Vs,
    // Connection pool monitoring
    Ps,
    // Vault credential cache commands
    Vc,
    Vcc,
    Vcr,
    Vce,
    // Password management commands
    Savepass,
    Listpass,
    Deletepass,
    Encryptpass,
    // Complex display commands (minimal set)
    Cd,
    Cdj,
    // AI SQL generation commands
    Aiauth,
    Ailogout,
    Aistatus,
    Ai,
    Aiconfig,
    Aiclearcache,
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
            // History management
            CommandShortcut::Hc => "\\hc",
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
            CommandShortcut::ServerInfo => "\\serverinfo",
            CommandShortcut::A => "\\a",
            CommandShortcut::Cs => "\\cs",
            CommandShortcut::Csthreshold => "\\csthreshold",
            CommandShortcut::Clrcs => "\\clrcs",
            CommandShortcut::Resetview => "\\resetview",
            // Vector display commands
            CommandShortcut::Vd => "\\vd",
            CommandShortcut::Vdc => "\\vdc",
            CommandShortcut::Vs => "\\vs",
            // Connection pool monitoring
            CommandShortcut::Ps => "\\ps",
            // Vault credential cache commands
            CommandShortcut::Vc => "\\vc",
            CommandShortcut::Vcc => "\\vcc",
            CommandShortcut::Vcr => "\\vcr",
            CommandShortcut::Vce => "\\vce",
            // Password management commands
            CommandShortcut::Savepass => "\\savepass",
            CommandShortcut::Listpass => "\\listpass",
            CommandShortcut::Deletepass => "\\deletepass",
            CommandShortcut::Encryptpass => "\\encryptpass",
            // Complex display commands (minimal set)
            CommandShortcut::Cd => "\\cd",
            CommandShortcut::Cdj => "\\cdj",
            // AI SQL generation commands
            CommandShortcut::Aiauth => "\\aiauth",
            CommandShortcut::Ailogout => "\\ailogout",
            CommandShortcut::Aistatus => "\\aistatus",
            CommandShortcut::Ai => "\\ai",
            CommandShortcut::Aiconfig => "\\aiconfig",
            CommandShortcut::Aiclearcache => "\\aiclearcache",
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
            // History management
            CommandShortcut::Hc => "Clear session history",
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
            CommandShortcut::ServerInfo => "Toggle server info display",
            CommandShortcut::A => "Toggle autocomplete",
            CommandShortcut::Cs => "Toggle column selection",
            CommandShortcut::Csthreshold => "Set column selection threshold",
            CommandShortcut::Clrcs => "Clear column views",
            CommandShortcut::Resetview => "Reset view",
            // Vector display commands
            CommandShortcut::Vd => "Set vector display mode",
            CommandShortcut::Vdc => "Show vector display config",
            CommandShortcut::Vs => "Toggle vector statistics",
            // Connection pool monitoring
            CommandShortcut::Ps => "Show connection pool statistics",
            // Vault credential cache commands
            CommandShortcut::Vc => "Show vault credential cache status",
            CommandShortcut::Vcc => "Clear all cached vault credentials",
            CommandShortcut::Vcr => "Force refresh vault credentials",
            CommandShortcut::Vce => "Show expired vault credentials",
            // Password management commands
            CommandShortcut::Savepass => "Save password to .dbcrust file",
            CommandShortcut::Listpass => "List stored passwords (without showing passwords)",
            CommandShortcut::Deletepass => "Delete stored password",
            CommandShortcut::Encryptpass => "Encrypt all plaintext passwords in .dbcrust",
            // Complex display commands (minimal set)
            CommandShortcut::Cd => "Set complex data display mode",
            CommandShortcut::Cdj => "Toggle JSON pretty printing",
            // AI SQL generation commands
            CommandShortcut::Aiauth => "Authenticate with Anthropic using OAuth (subscription)",
            CommandShortcut::Ailogout => "Logout from Anthropic OAuth session",
            CommandShortcut::Aistatus => "Show current AI authentication status",
            CommandShortcut::Ai => "Generate SQL from natural language prompt",
            CommandShortcut::Aiconfig => "Show AI SQL configuration",
            CommandShortcut::Aiclearcache => "Clear AI query cache",
        }
    }

    /// Get the command category
    pub fn category(&self) -> CommandCategory {
        match self {
            // Core commands
            CommandShortcut::Q | CommandShortcut::H => CommandCategory::Core,
            // Database navigation
            CommandShortcut::L | CommandShortcut::Dt | CommandShortcut::D | CommandShortcut::C => {
                CommandCategory::DatabaseNavigation
            }
            // Display options (including some advanced display commands)
            CommandShortcut::X
            | CommandShortcut::E
            | CommandShortcut::Config
            | CommandShortcut::Setmulti
            | CommandShortcut::Pager
            | CommandShortcut::Banner
            | CommandShortcut::ServerInfo
            | CommandShortcut::A
            | CommandShortcut::Cs
            | CommandShortcut::Csthreshold
            | CommandShortcut::Clrcs
            | CommandShortcut::Resetview => CommandCategory::DisplayOptions,
            // Vector display commands
            CommandShortcut::Vd | CommandShortcut::Vdc | CommandShortcut::Vs => {
                CommandCategory::DisplayOptions
            }
            // Script handling
            CommandShortcut::W
            | CommandShortcut::I
            | CommandShortcut::Ed
            | CommandShortcut::Ecopy => CommandCategory::ScriptHandling,
            // Named queries
            CommandShortcut::N | CommandShortcut::Ns | CommandShortcut::Nd => {
                CommandCategory::NamedQueries
            }
            // Session management
            CommandShortcut::S | CommandShortcut::Ss | CommandShortcut::Sd => {
                CommandCategory::SessionManagement
            }
            // Connection history
            CommandShortcut::R | CommandShortcut::Rc => CommandCategory::ConnectionHistory,
            // History management
            CommandShortcut::Hc => CommandCategory::HistoryManagement,
            // Database-specific commands
            CommandShortcut::Du
            | CommandShortcut::Di
            | CommandShortcut::Dp
            | CommandShortcut::Pgpass
            | CommandShortcut::Myconf
            | CommandShortcut::Docker => CommandCategory::DatabaseSpecific,
            // Vault management
            CommandShortcut::Vc
            | CommandShortcut::Vcc
            | CommandShortcut::Vcr
            | CommandShortcut::Vce => CommandCategory::VaultManagement,
            // Password management
            CommandShortcut::Savepass
            | CommandShortcut::Listpass
            | CommandShortcut::Deletepass
            | CommandShortcut::Encryptpass => CommandCategory::PasswordManagement,
            // EXPLAIN variants (Advanced)
            CommandShortcut::Er
            | CommandShortcut::Ef
            | CommandShortcut::Ex
            | CommandShortcut::Ps => CommandCategory::Advanced,
            // Complex display commands
            CommandShortcut::Cd | CommandShortcut::Cdj => CommandCategory::DisplayOptions,
            // AI SQL generation commands
            CommandShortcut::Aiauth
            | CommandShortcut::Ailogout
            | CommandShortcut::Aistatus
            | CommandShortcut::Ai
            | CommandShortcut::Aiconfig
            | CommandShortcut::Aiclearcache => CommandCategory::AiSqlGeneration,
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
            return Err(CommandError::InvalidSyntax(
                "Commands must start with '\\'".to_string(),
            ));
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
                    Ok(Command::DescribeTable {
                        table_name: Some(args.to_string()),
                    })
                }
            }
            "c" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("database name".to_string()))
                } else {
                    Ok(Command::ConnectDatabase {
                        database_name: args.to_string(),
                    })
                }
            }

            // Display options
            "x" => Ok(Command::ToggleExpandedDisplay),
            "e" => Ok(Command::ToggleExplainMode),
            "config" => Ok(Command::ShowConfig),

            // Script handling
            "w" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("filename".to_string()))
                } else {
                    Ok(Command::WriteScript {
                        filename: args.to_string(),
                    })
                }
            }
            "i" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("filename".to_string()))
                } else {
                    Ok(Command::LoadScript {
                        filename: args.to_string(),
                    })
                }
            }
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
                    let exec_args = name_parts
                        .next()
                        .map(|s| s.split_whitespace().map(|s| s.to_string()).collect())
                        .unwrap_or_default();
                    Ok(Command::ExecuteNamedQuery {
                        name,
                        args: exec_args,
                    })
                }
            }
            "ns" => {
                // Parse scope flags
                let args_parts: Vec<&str> = args.split_whitespace().collect();
                if args_parts.len() < 2 {
                    return Err(CommandError::MissingArgument(
                        "query name and query".to_string(),
                    ));
                }

                let mut global = false;
                let mut postgres = false;
                let mut mysql = false;
                let mut sqlite = false;
                let mut name_index = 0;

                // Check for scope flags at the beginning
                for (i, part) in args_parts.iter().enumerate() {
                    match *part {
                        "-g" | "--global" => {
                            global = true;
                            name_index = i + 1;
                        }
                        "--postgres" => {
                            postgres = true;
                            name_index = i + 1;
                        }
                        "--mysql" => {
                            mysql = true;
                            name_index = i + 1;
                        }
                        "--sqlite" => {
                            sqlite = true;
                            name_index = i + 1;
                        }
                        _ => break,
                    }
                }

                // Ensure we have at least name and query after flags
                if name_index + 1 >= args_parts.len() {
                    return Err(CommandError::MissingArgument(
                        "query name and query after flags".to_string(),
                    ));
                }

                let name = args_parts[name_index].to_string();
                let query = args_parts[name_index + 1..].join(" ");

                Ok(Command::SaveNamedQuery {
                    name,
                    query,
                    global,
                    postgres,
                    mysql,
                    sqlite,
                })
            }
            "nd" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("query name".to_string()))
                } else {
                    Ok(Command::DeleteNamedQuery {
                        name: args.to_string(),
                    })
                }
            }

            // Session management
            "s" => {
                if args.is_empty() {
                    Ok(Command::ListSessions)
                } else {
                    Ok(Command::ConnectSession {
                        name: args.to_string(),
                    })
                }
            }
            "ss" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("session name".to_string()))
                } else {
                    Ok(Command::SaveSession {
                        name: args.to_string(),
                    })
                }
            }
            "sd" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("session name".to_string()))
                } else {
                    Ok(Command::DeleteSession {
                        name: args.to_string(),
                    })
                }
            }

            // Connection history
            "r" => Ok(Command::ListRecentConnections),
            "rc" => Ok(Command::ClearRecentConnections),

            // History management
            "hc" => {
                if args.is_empty() {
                    Ok(Command::ClearSessionHistory { session_hash: None })
                } else {
                    Ok(Command::ClearSessionHistory {
                        session_hash: Some(args.to_string()),
                    })
                }
            }

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
                    Ok(Command::ExplainRaw {
                        query: args.to_string(),
                    })
                }
            }
            "ef" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("query".to_string()))
                } else {
                    Ok(Command::ExplainFormatted {
                        query: args.to_string(),
                    })
                }
            }
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
                            filename: filename.to_string(),
                        })
                    }
                } else {
                    Err(CommandError::MissingArgument("filename".to_string()))
                }
            }

            // Advanced commands
            "setmulti" => Ok(Command::SetMultilineIndicator {
                indicator: args.to_string(),
            }),
            "pager" => Ok(Command::TogglePager),
            "banner" => Ok(Command::ToggleBanner),
            "serverinfo" => Ok(Command::ToggleServerInfo),
            "a" => Ok(Command::ToggleAutocomplete),
            "cs" => Ok(Command::ToggleColumnSelection),
            "csthreshold" => {
                let threshold = args.parse::<usize>().map_err(|_| {
                    CommandError::InvalidSyntax("Invalid threshold number".to_string())
                })?;
                Ok(Command::SetColumnSelectionThreshold { threshold })
            }
            "clrcs" => Ok(Command::ClearColumnViews),
            "resetview" => Ok(Command::ResetView),

            // Vector display commands
            "vd" => Ok(Command::SetVectorDisplayMode {
                mode: args.to_string(),
            }),
            "vdc" => Ok(Command::ShowVectorDisplayConfig),
            "vs" => Ok(Command::ToggleVectorStatistics),

            // Connection pool monitoring
            "ps" => Ok(Command::ShowPoolStats),

            // Complex display commands
            "cd" => {
                if args.is_empty() {
                    Ok(Command::ComplexDisplayMode { mode: None })
                } else {
                    Ok(Command::ComplexDisplayMode {
                        mode: Some(args.to_string()),
                    })
                }
            }
            "cdj" => Ok(Command::ComplexDisplayJsonToggle),

            // Vault credential cache commands
            "vc" => Ok(Command::VaultCacheStatus),
            "vcc" => Ok(Command::VaultCacheClear),
            "vcr" => {
                let role = if args.is_empty() {
                    None
                } else {
                    Some(args.to_string())
                };
                Ok(Command::VaultCacheRefresh { role })
            }
            "vce" => Ok(Command::VaultCacheExpired),

            // Password management commands
            "savepass" => {
                // Interactive mode - all parameters will be collected interactively
                Ok(Command::SavePassword {
                    db_type: None,
                    host: None,
                    port: None,
                    database: None,
                    username: None,
                    encrypt: true, // Default to encrypted
                })
            }
            "listpass" => Ok(Command::ListPasswords),
            "deletepass" => {
                // Interactive mode - parameters will be selected interactively
                Ok(Command::DeletePassword {
                    db_type: None,
                    host: None,
                    port: None,
                    database: None,
                    username: None,
                })
            }
            "encryptpass" => Ok(Command::EncryptPasswords),

            // MongoDB-specific commands
            "collections" => Ok(Command::ListCollections),
            "dc" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("collection name".to_string()))
                } else {
                    Ok(Command::DescribeCollection {
                        collection_name: args.to_string(),
                    })
                }
            }
            "dmi" => Ok(Command::ListMongoIndexes),
            "cmi" => {
                // Parse collection field [type]
                let parts: Vec<&str> = args.split_whitespace().collect();
                if parts.len() < 2 {
                    return Err(CommandError::MissingArgument(
                        "collection and field name".to_string(),
                    ));
                }
                let collection = parts[0].to_string();
                let field = parts[1].to_string();
                let index_type = parts.get(2).map(|s| s.to_string());
                Ok(Command::CreateMongoIndex {
                    collection,
                    field,
                    index_type,
                })
            }
            "ddmi" => {
                // Parse collection index_name
                let parts: Vec<&str> = args.split_whitespace().collect();
                if parts.len() < 2 {
                    return Err(CommandError::MissingArgument(
                        "collection and index name".to_string(),
                    ));
                }
                let collection = parts[0].to_string();
                let index_name = parts[1].to_string();
                Ok(Command::DropMongoIndex {
                    collection,
                    index_name,
                })
            }
            "mstats" => Ok(Command::MongoStats),
            "find" => {
                // Parse collection [filter] [projection] [limit]
                let parts: Vec<&str> = args.split_whitespace().collect();
                if parts.is_empty() {
                    return Err(CommandError::MissingArgument("collection name".to_string()));
                }
                let collection = parts[0].to_string();
                let filter = parts.get(1).map(|s| s.to_string());
                let projection = parts.get(2).map(|s| s.to_string());
                let limit = parts.get(3).and_then(|s| s.parse::<i64>().ok());
                Ok(Command::MongoFind {
                    collection,
                    filter,
                    projection,
                    limit,
                })
            }
            "aggregate" => {
                // Parse collection pipeline
                let parts: Vec<&str> = args.splitn(2, ' ').collect();
                if parts.len() < 2 {
                    return Err(CommandError::MissingArgument(
                        "collection and aggregation pipeline".to_string(),
                    ));
                }
                let collection = parts[0].to_string();
                let pipeline = parts[1].to_string();
                Ok(Command::MongoAggregate {
                    collection,
                    pipeline,
                })
            }
            "search" => {
                // Parse collection search_term
                let parts: Vec<&str> = args.splitn(2, ' ').collect();
                if parts.len() < 2 {
                    return Err(CommandError::MissingArgument(
                        "collection and search term".to_string(),
                    ));
                }
                let collection = parts[0].to_string();
                let search_term = parts[1].to_string();
                Ok(Command::MongoTextSearch {
                    collection,
                    search_term,
                })
            }

            // AI SQL generation commands
            "aiauth" => Ok(Command::AiAuthenticate),
            "ailogout" => Ok(Command::AiLogout),
            "aistatus" => Ok(Command::AiStatus),
            "ai" => {
                if args.is_empty() {
                    Err(CommandError::MissingArgument("prompt".to_string()))
                } else {
                    Ok(Command::AiGenerate {
                        prompt: args.to_string(),
                    })
                }
            }
            "aiconfig" => Ok(Command::AiConfig),
            "aiclearcache" => Ok(Command::AiClearCache),

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
        let mut categories: HashMap<CommandCategory, Vec<(&'static str, &'static str)>> =
            HashMap::new();

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
                let status = if db.is_expanded_display() {
                    "on"
                } else {
                    "off"
                };
                Ok(CommandResult::Output(format!(
                    "Expanded display is {status}."
                )))
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
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to list databases: {e}"
                    ))),
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
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
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
                    Some(name) => match db.get_table_details(name).await {
                        Ok(details) => {
                            let output = crate::format::format_table_details(&details);
                            Ok(CommandResult::Output(output))
                        }
                        Err(e) => Ok(CommandResult::Error(format!(
                            "Failed to describe table '{name}': {e}"
                        ))),
                    },
                    None => {
                        // List all tables when no table name provided
                        match db.list_tables().await {
                            Ok(results) => {
                                if results.is_empty() {
                                    Ok(CommandResult::Output("No tables found.".to_string()))
                                } else {
                                    let output = if db.is_expanded_display() {
                                        let tables =
                                            crate::format::format_query_results_expanded(&results);
                                        tables
                                            .into_iter()
                                            .map(|t| t.to_string())
                                            .collect::<Vec<_>>()
                                            .join("\n")
                                    } else {
                                        crate::format::format_query_results_psql(&results)
                                    };
                                    Ok(CommandResult::Output(output))
                                }
                            }
                            Err(e) => {
                                Ok(CommandResult::Error(format!("Failed to list tables: {e}")))
                            }
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
                        Ok(CommandResult::Output(format!(
                            "Connected to database '{new_db_name}'."
                        )))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to connect to database '{database_name}': {e}"
                    ))),
                }
            }

            Command::ListSessions => {
                let sessions = config.list_sessions();
                if sessions.is_empty() {
                    Ok(CommandResult::Output(
                        "No saved sessions found. Use \\ss <name> to save a session.".to_string(),
                    ))
                } else {
                    let mut output = String::new();
                    output.push_str("Saved sessions:\n");
                    for (name, session) in sessions.iter() {
                        let db_type = session.database_type.display_name();
                        if session.database_type.is_file_based() {
                            if let Some(ref file_path) = session.file_path {
                                output.push_str(&format!("  {name} - {file_path} ({db_type})\n"));
                            } else {
                                output.push_str(&format!("  {name} - SQLite (no path)\n"));
                            }
                        } else {
                            output.push_str(&format!(
                                "  {} - {}@{}:{}/{} ({})\n",
                                name,
                                session.user,
                                session.host,
                                session.port,
                                session.dbname,
                                db_type
                            ));
                        }
                    }
                    Ok(CommandResult::Output(output))
                }
            }

            Command::ListRecentConnections => {
                let recent = config.get_recent_connections();
                if recent.is_empty() {
                    Ok(CommandResult::Output(
                        "No recent connections found.".to_string(),
                    ))
                } else {
                    let mut output = String::new();
                    output.push_str("Recent connections:\n");
                    for (i, conn) in recent.iter().take(20).enumerate() {
                        let status = if conn.success { "✅" } else { "❌" };
                        let timestamp = conn.timestamp.format("%Y-%m-%d %H:%M");
                        let db_type = conn.database_type.display_name();
                        output.push_str(&format!(
                            "  {}: {} {} - {} ({})\n",
                            i + 1,
                            status,
                            conn.display_name,
                            timestamp,
                            db_type
                        ));
                    }
                    Ok(CommandResult::Output(output))
                }
            }

            Command::ClearRecentConnections => {
                if let Err(e) = config.clear_recent_connections() {
                    Ok(CommandResult::Error(format!(
                        "Failed to clear recent connections: {e}"
                    )))
                } else {
                    Ok(CommandResult::Output(
                        "Recent connections cleared.".to_string(),
                    ))
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
                    Ok(_) => Ok(CommandResult::Output(format!(
                        "Session '{name}' saved successfully."
                    ))),
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to save session '{name}': {e}"
                    ))),
                }
            }

            Command::DeleteSession { name } => match config.delete_session(name) {
                Ok(_) => Ok(CommandResult::Output(format!(
                    "Session '{name}' deleted successfully."
                ))),
                Err(e) => Ok(CommandResult::Error(format!(
                    "Failed to delete session '{name}': {e}"
                ))),
            },

            Command::ConnectSession { name } => {
                // This would require full connection logic - for now show available info
                match config.get_session(name) {
                    Some(session) => Ok(CommandResult::Output(format!(
                        "Session '{}' found: {}@{}:{}/{}",
                        name, session.user, session.host, session.port, session.dbname
                    ))),
                    None => Ok(CommandResult::Error(format!(
                        "Session '{name}' not found. Use \\s to list available sessions."
                    ))),
                }
            }

            Command::ListNamedQueries => {
                // Get current context for filtering
                let (current_database_type, current_session_id) = {
                    let db = database.lock().unwrap();
                    let db_type = db
                        .get_connection_info()
                        .map(|info| info.database_type.clone());
                    let session_id = SessionId::from_database(&db).map(|sid| sid.identifier);
                    (db_type, session_id)
                };

                // Use new API to list available queries with scope information
                let available_queries = config.list_available_named_queries(
                    current_database_type.as_ref(),
                    current_session_id.as_deref(),
                );

                if available_queries.is_empty() {
                    Ok(CommandResult::Output("No named queries available in current context.\nUse \\ns <name> <query> to save a session query.\nUse \\ns -g <name> <query> for global queries.\nUse \\ns --postgres <name> <query> for PostgreSQL-only queries.".to_string()))
                } else {
                    let mut output = String::new();
                    output.push_str("Named queries:\n");
                    for (name, query, scope) in available_queries.iter() {
                        let preview = if query.len() > 45 {
                            format!("{}...", &query[..42])
                        } else {
                            query.clone()
                        };

                        let scope_str = match scope {
                            NamedQueryScope::Global => "[global]".to_string(),
                            NamedQueryScope::DatabaseType(db_type) => {
                                format!("[{}]", db_type.to_string().to_lowercase())
                            }
                            NamedQueryScope::Session(_) => "[session]".to_string(),
                        };

                        output.push_str(&format!("  {name:<15} {scope_str:<10} - {preview}\n"));
                    }
                    Ok(CommandResult::Output(output))
                }
            }

            Command::SaveNamedQuery {
                name,
                query,
                global,
                postgres,
                mysql,
                sqlite,
            } => {
                // Determine scope based on flags
                let scope = if *global {
                    NamedQueryScope::Global
                } else if *postgres {
                    NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL)
                } else if *mysql {
                    NamedQueryScope::DatabaseType(DatabaseType::MySQL)
                } else if *sqlite {
                    NamedQueryScope::DatabaseType(DatabaseType::SQLite)
                } else {
                    // Default to session scope
                    let session_id = {
                        let db = database.lock().unwrap();
                        SessionId::from_database(&db)
                            .map(|sid| sid.identifier)
                            .unwrap_or_else(|| "unknown".to_string())
                    };
                    NamedQueryScope::Session(session_id)
                };

                // Test query before saving if enabled
                if config.test_named_query_before_saving {
                    let mut db = database.lock().unwrap();
                    // Try to execute the query in a transaction (rollback to avoid side effects)
                    match db.test_query_execution(query).await {
                        Ok(_) => {
                            // Query is valid, proceed with saving
                        }
                        Err(e) => {
                            return Ok(CommandResult::Error(format!(
                                "Query test failed: {e}\nQuery not saved. Use config option 'test_named_query_before_saving = false' to disable testing."
                            )));
                        }
                    }
                }

                match config.add_named_query_with_scope(name, query, scope.clone()) {
                    Ok(_) => {
                        let scope_str = match scope {
                            NamedQueryScope::Global => "global".to_string(),
                            NamedQueryScope::DatabaseType(db_type) => {
                                db_type.to_string().to_lowercase()
                            }
                            NamedQueryScope::Session(_) => "session".to_string(),
                        };
                        Ok(CommandResult::Output(format!(
                            "Named query '{name}' saved successfully (scope: {scope_str})."
                        )))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to save named query '{name}': {e}"
                    ))),
                }
            }

            Command::DeleteNamedQuery { name } => {
                // Get current context for scoped query lookup
                let (current_database_type, current_session_id) = {
                    let db = database.lock().unwrap();
                    let db_type = db
                        .get_connection_info()
                        .map(|info| info.database_type.clone());
                    let session_id = db
                        .get_connection_info()
                        .map(|info| SessionId::from_connection_info(info).identifier);
                    (db_type, session_id)
                };

                // Try to find the query first to determine which scope to delete from
                if let Some(query) = config.get_available_named_query(
                    name,
                    current_database_type.as_ref(),
                    current_session_id.as_deref(),
                ) {
                    let scope = query.scope.clone();
                    match config.delete_named_query_with_scope(name, &scope) {
                        Ok(true) => {
                            let scope_str = match scope {
                                crate::config::NamedQueryScope::Global => "global",
                                crate::config::NamedQueryScope::DatabaseType(ref db_type) => {
                                    &format!("{db_type}")
                                }
                                crate::config::NamedQueryScope::Session(_) => "session-local",
                            };
                            Ok(CommandResult::Output(format!(
                                "Named query '{name}' deleted successfully (scope: {scope_str})."
                            )))
                        }
                        Ok(false) => Ok(CommandResult::Error(format!(
                            "Named query '{name}' not found."
                        ))),
                        Err(e) => Ok(CommandResult::Error(format!(
                            "Failed to delete named query '{name}': {e}"
                        ))),
                    }
                } else {
                    Ok(CommandResult::Error(format!(
                        "Named query '{name}' not found in current context."
                    )))
                }
            }

            Command::ExecuteNamedQuery { name, args } => {
                // Get current context for scoped query lookup
                let (current_database_type, current_session_id) = {
                    let db = database.lock().unwrap();
                    let db_type = db
                        .get_connection_info()
                        .map(|info| info.database_type.clone());
                    let session_id = SessionId::from_database(&db).map(|sid| sid.identifier);
                    (db_type, session_id)
                };

                match config.get_available_named_query(
                    name,
                    current_database_type.as_ref(),
                    current_session_id.as_deref(),
                ) {
                    Some(named_query) => {
                        let mut db = database.lock().unwrap();
                        // Apply parameter substitution
                        let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                        let final_query =
                            crate::named_queries::process_query(&named_query.query, &args_refs);

                        // Execute the query
                        match db.execute_query(&final_query).await {
                            Ok(results) => {
                                if results.is_empty() {
                                    Ok(CommandResult::Output(
                                        "Query executed successfully (no results).".to_string(),
                                    ))
                                } else {
                                    let output = if db.is_expanded_display() {
                                        let tables =
                                            crate::format::format_query_results_expanded(&results);
                                        tables
                                            .into_iter()
                                            .map(|t| t.to_string())
                                            .collect::<Vec<_>>()
                                            .join("\n")
                                    } else {
                                        crate::format::format_query_results_psql(&results)
                                    };
                                    Ok(CommandResult::Output(output))
                                }
                            }
                            Err(e) => Ok(CommandResult::Error(format!(
                                "Error executing named query '{name}': {e}"
                            ))),
                        }
                    }
                    None => Ok(CommandResult::Error(format!(
                        "Named query '{name}' not found or not available in current context.\nUse \\n to list available queries."
                    ))),
                }
            }

            Command::WriteScript { filename } => {
                if last_script.is_empty() {
                    Ok(CommandResult::Error(
                        "No script content to write. Use \\ed to edit a script first.".to_string(),
                    ))
                } else {
                    match std::fs::write(filename, last_script) {
                        Ok(_) => Ok(CommandResult::Output(format!(
                            "Script saved to '{filename}'."
                        ))),
                        Err(e) => Ok(CommandResult::Error(format!(
                            "Failed to write script to '{filename}': {e}"
                        ))),
                    }
                }
            }

            Command::LoadScript { filename } => match std::fs::read_to_string(filename) {
                Ok(content) => {
                    *last_script = content.clone();
                    let line_count = content.lines().count();
                    Ok(CommandResult::Output(format!(
                        "Script loaded from '{filename}' ({line_count} lines). Press Enter to execute, \\ed to edit, or \\w to save elsewhere."
                    )))
                }
                Err(e) => Ok(CommandResult::Error(format!(
                    "Failed to load script from '{filename}': {e}"
                ))),
            },

            Command::EditMultiline => {
                // Launch external editor with current script content
                match crate::script::edit_multiline_script(last_script) {
                    Ok(edited_content) => {
                        *last_script = edited_content.clone();
                        if edited_content.trim().is_empty() {
                            Ok(CommandResult::Output(
                                "Editor closed with empty content.".to_string(),
                            ))
                        } else {
                            let line_count = edited_content.lines().count();
                            Ok(CommandResult::Output(format!(
                                "Script edited ({line_count} lines). Execute it by pressing Enter, or save with \\w filename"
                            )))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Editor error: {e}"))),
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
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
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
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list indexes: {e}"))),
                }
            }

            Command::ShowPgpass => match crate::pgpass::get_pgpass_path() {
                Some(path) => {
                    let exists = std::path::Path::new(&path).exists();
                    Ok(CommandResult::Output(format!(
                        "PostgreSQL .pgpass file: {} (exists: {})",
                        path.to_string_lossy(),
                        exists
                    )))
                }
                None => Ok(CommandResult::Output(
                    "No .pgpass file path configured.".to_string(),
                )),
            },

            Command::ShowMyconf => match crate::myconf::get_mysql_config_path() {
                Some(path) => {
                    let exists = std::path::Path::new(&path).exists();
                    Ok(CommandResult::Output(format!(
                        "MySQL .my.cnf file: {} (exists: {})",
                        path.to_string_lossy(),
                        exists
                    )))
                }
                None => Ok(CommandResult::Output(
                    "No .my.cnf file path configured.".to_string(),
                )),
            },

            Command::CopyExplainPlan => {
                use arboard::Clipboard;
                let db = database.lock().unwrap();
                match db.get_last_json_plan() {
                    Some(json_plan) => match Clipboard::new() {
                        Ok(mut clipboard) => match clipboard.set_text(json_plan.clone()) {
                            Ok(()) => Ok(CommandResult::Output(format!(
                                "EXPLAIN JSON plan copied to clipboard ({} characters)",
                                json_plan.len()
                            ))),
                            Err(e) => Ok(CommandResult::Error(format!(
                                "Error copying to clipboard: {e}"
                            ))),
                        },
                        Err(e) => Ok(CommandResult::Error(format!(
                            "Error accessing clipboard: {e}"
                        ))),
                    },
                    None => {
                        // Check if we have a database client to provide a more specific error message
                        if let Some(database_client) = db.get_database_client() {
                            if database_client
                                .get_connection_info()
                                .database_type
                                .is_file_based()
                            {
                                Ok(CommandResult::Error("\\ecopy is not supported for SQLite databases. SQLite EXPLAIN queries don't produce JSON plans.".to_string()))
                            } else {
                                Ok(CommandResult::Error("No EXPLAIN JSON plan available. Run an EXPLAIN query first with \\ef or \\er.".to_string()))
                            }
                        } else {
                            Ok(CommandResult::Error("No EXPLAIN JSON plan available. Run an EXPLAIN query first with \\ef or \\er.".to_string()))
                        }
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
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list pragmas: {e}"))),
                }
            }

            Command::ListDockerContainers => match crate::docker::DockerClient::new() {
                Ok(docker_client) => match docker_client.list_database_containers().await {
                    Ok(containers) => {
                        if containers.is_empty() {
                            Ok(CommandResult::Output(
                                "No database containers found.".to_string(),
                            ))
                        } else {
                            let output = containers
                                .iter()
                                .map(|c| {
                                    let db_type = c
                                        .database_type
                                        .as_ref()
                                        .map(|dt| dt.to_string())
                                        .unwrap_or_else(|| "Unknown".to_string());
                                    format!("{} ({})", c.name, db_type)
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            Ok(CommandResult::Output(format!(
                                "Available database containers:\n{output}"
                            )))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to list Docker containers: {e}"
                    ))),
                },
                Err(e) => Ok(CommandResult::Error(format!(
                    "Failed to connect to Docker: {e}"
                ))),
            },

            Command::ExplainRaw { query } => {
                let mut db = database.lock().unwrap();
                match db.execute_explain_query_raw(query).await {
                    Ok(results) => {
                        let output = crate::format::format_query_results_psql(&results);
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to explain query: {e}"
                    ))),
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
                            tables
                                .into_iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            crate::format::format_query_results_psql(&results)
                        };
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to explain query: {e}"
                    ))),
                }
            }

            Command::ExplainExport { query, filename } => {
                let mut db = database.lock().unwrap();
                match db.execute_explain_query_formatted(query).await {
                    Ok(results) => {
                        let output = crate::format::format_query_results_psql(&results);
                        match std::fs::write(filename, &output) {
                            Ok(_) => Ok(CommandResult::Output(format!(
                                "EXPLAIN results exported to {filename}"
                            ))),
                            Err(e) => Ok(CommandResult::Error(format!(
                                "Failed to write to {filename}: {e}"
                            ))),
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to explain query: {e}"
                    ))),
                }
            }

            Command::SetMultilineIndicator { indicator } => {
                config.multiline_prompt_indicator = indicator.clone();
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                Ok(CommandResult::Output(format!(
                    "Multiline indicator set to: {indicator} (will take effect on next restart)"
                )))
            }

            Command::TogglePager => {
                config.pager_enabled = !config.pager_enabled;
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.pager_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                Ok(CommandResult::Output(format!("Pager is now {status}.")))
            }

            Command::ToggleBanner => {
                config.show_banner = !config.show_banner;
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.show_banner {
                    "enabled"
                } else {
                    "disabled"
                };
                Ok(CommandResult::Output(format!("Banner is now {status}.")))
            }

            Command::ToggleServerInfo => {
                config.show_server_info = !config.show_server_info;
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.show_server_info {
                    "enabled"
                } else {
                    "disabled"
                };
                Ok(CommandResult::Output(format!(
                    "Server info display is now {status}."
                )))
            }

            Command::ToggleAutocomplete => {
                config.autocomplete_enabled = !config.autocomplete_enabled;
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.autocomplete_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                Ok(CommandResult::Output(format!(
                    "Autocomplete is now {status}."
                )))
            }

            Command::ToggleColumnSelection => {
                let mut db = database.lock().unwrap();
                let new_status = db.toggle_column_select_mode();
                let status = if new_status { "enabled" } else { "disabled" };
                Ok(CommandResult::Output(format!(
                    "Column selection mode is now {status}."
                )))
            }

            Command::SetColumnSelectionThreshold { threshold } => {
                config.column_selection_threshold = *threshold;
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                Ok(CommandResult::Output(format!(
                    "Column selection threshold set to: {threshold}"
                )))
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
                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                Ok(CommandResult::Output(
                    "View settings reset to defaults.".to_string(),
                ))
            }

            // Vector display commands
            Command::SetVectorDisplayMode { mode } => {
                use crate::vector_display::VectorDisplayMode;

                if mode.is_empty() {
                    return Ok(CommandResult::Output(format!(
                        "Current vector display mode: {}\nAvailable modes: {}",
                        config.vector_display.display_mode,
                        VectorDisplayMode::all_modes().join(", ")
                    )));
                }

                match VectorDisplayMode::from_str(mode) {
                    Some(new_mode) => {
                        config.vector_display.display_mode = new_mode;

                        // Update global config so formatters can use it immediately
                        crate::vector_display::set_global_vector_config(
                            config.vector_display.clone(),
                        );

                        config
                            .save()
                            .map_err(|e| CommandError::DatabaseError(e.into()))?;
                        Ok(CommandResult::Output(format!(
                            "Vector display mode set to: {}",
                            mode
                        )))
                    }
                    None => Ok(CommandResult::Error(format!(
                        "Invalid vector display mode: '{}'\nAvailable modes: {}",
                        mode,
                        VectorDisplayMode::all_modes().join(", ")
                    ))),
                }
            }

            Command::ShowVectorDisplayConfig => {
                let config_display = format!(
                    "Vector Display Configuration:\n\
                    • Display mode: {}\n\
                    • Truncation length: {}\n\
                    • Viz width: {}\n\
                    • Show statistics: {}\n\
                    • Dimension threshold: {}\n\
                    • Show dimensions: {}",
                    config.vector_display.display_mode,
                    config.vector_display.truncation_length,
                    config.vector_display.viz_width,
                    config.vector_display.show_statistics,
                    config.vector_display.dimension_threshold,
                    config.vector_display.show_dimensions
                );
                Ok(CommandResult::Output(config_display))
            }

            Command::ToggleVectorStatistics => {
                config.vector_display.show_statistics = !config.vector_display.show_statistics;

                // Update global config so formatters can use it immediately
                crate::vector_display::set_global_vector_config(config.vector_display.clone());

                config
                    .save()
                    .map_err(|e| CommandError::DatabaseError(e.into()))?;
                let status = if config.vector_display.show_statistics {
                    "enabled"
                } else {
                    "disabled"
                };
                Ok(CommandResult::Output(format!(
                    "Vector statistics display is now {}.",
                    status
                )))
            }

            // Vault credential cache commands
            Command::VaultCacheStatus => {
                if !config.vault_credential_cache_enabled {
                    return Ok(CommandResult::Output(
                        "Vault credential caching is disabled.".to_string(),
                    ));
                }

                let cached_creds = config.list_cached_vault_credentials();
                if cached_creds.is_empty() {
                    Ok(CommandResult::Output(
                        "No vault credentials cached.".to_string(),
                    ))
                } else {
                    let mut output = format!(
                        "Vault credential cache status (showing {} entries):\n",
                        cached_creds.len()
                    );
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

            Command::VaultCacheClear => match config.clear_vault_credentials() {
                Ok(()) => Ok(CommandResult::Output(
                    "All vault credentials cleared from cache.".to_string(),
                )),
                Err(e) => Ok(CommandResult::Error(format!(
                    "Failed to clear vault credentials: {e}"
                ))),
            },

            Command::VaultCacheRefresh { role } => {
                match role {
                    Some(role_key) => {
                        // Force refresh specific role - would need to implement role-specific refresh
                        Ok(CommandResult::Output(format!(
                            "Force refresh for role '{role_key}' not yet implemented."
                        )))
                    }
                    None => {
                        // Reload from file
                        config.reload_vault_credentials();
                        Ok(CommandResult::Output(
                            "Vault credential cache reloaded from file.".to_string(),
                        ))
                    }
                }
            }

            Command::VaultCacheExpired => {
                let cached_creds = config.list_cached_vault_credentials();
                let now = chrono::Utc::now();
                let expired_creds: Vec<_> = cached_creds
                    .into_iter()
                    .filter(|(_, creds)| now >= creds.expire_time)
                    .collect();

                if expired_creds.is_empty() {
                    Ok(CommandResult::Output(
                        "No expired vault credentials found.".to_string(),
                    ))
                } else {
                    let mut output = format!(
                        "Expired vault credentials ({} entries):\n",
                        expired_creds.len()
                    );
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

            // Password management commands
            Command::SavePassword { .. } => {
                use crate::dbcrust_pass::{DatabaseType, save_password};
                use inquire::{Select, Text};
                use std::io::Write;

                // Interactive prompts for all parameters
                let database_types = vec![
                    "postgresql",
                    "mysql",
                    "mongodb",
                    "elasticsearch",
                    "clickhouse",
                    "sqlite",
                ];
                let db_type_str = Select::new("Database type:", database_types)
                    .prompt()
                    .map_err(|e| CommandError::InvalidSyntax(format!("Input error: {e}")))?;

                let db_type = DatabaseType::from_str(db_type_str).unwrap();

                let host = Text::new("Host:")
                    .with_default("localhost")
                    .prompt()
                    .map_err(|e| CommandError::InvalidSyntax(format!("Input error: {e}")))?;

                let port_str = Text::new("Port:")
                    .with_default(match db_type {
                        DatabaseType::PostgreSQL => "5432",
                        DatabaseType::MySQL => "3306",
                        DatabaseType::MongoDB => "27017",
                        DatabaseType::Elasticsearch => "9200",
                        DatabaseType::ClickHouse => "8123",
                        DatabaseType::SQLite => "0", // Not used for SQLite
                    })
                    .prompt()
                    .map_err(|e| CommandError::InvalidSyntax(format!("Input error: {e}")))?;

                let port: u16 = port_str
                    .parse()
                    .map_err(|_| CommandError::InvalidSyntax("Invalid port number".to_string()))?;

                let database = Text::new("Database name:")
                    .prompt()
                    .map_err(|e| CommandError::InvalidSyntax(format!("Input error: {e}")))?;

                let username = Text::new("Username:")
                    .prompt()
                    .map_err(|e| CommandError::InvalidSyntax(format!("Input error: {e}")))?;

                print!("Password: ");
                std::io::stdout().flush().unwrap();
                let password = rpassword::read_password().map_err(|e| {
                    CommandError::InvalidSyntax(format!("Password input error: {e}"))
                })?;

                // Always encrypt passwords by default (no confirmation prompt)
                let encrypt = true;

                match save_password(
                    db_type.clone(),
                    &host,
                    port,
                    &database,
                    &username,
                    &password,
                    encrypt,
                ) {
                    Ok(()) => Ok(CommandResult::Output(format!(
                        "Password saved for {}:{}@{}:{}/{} (encrypted)",
                        db_type.as_str(),
                        username,
                        host,
                        port,
                        database
                    ))),
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to save password: {e}"
                    ))),
                }
            }

            Command::ListPasswords => {
                use crate::dbcrust_pass::list_entries;

                match list_entries() {
                    Ok(entries) => {
                        if entries.is_empty() {
                            Ok(CommandResult::Output(
                                "No saved passwords found.".to_string(),
                            ))
                        } else {
                            let mut output = "Saved passwords:\n".to_string();
                            for (db_type, host, port, database, username) in entries {
                                output.push_str(&format!(
                                    "  {}:{}@{}:{}/{}\n",
                                    db_type.as_str(),
                                    username,
                                    host,
                                    port,
                                    database
                                ));
                            }
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to list passwords: {e}"
                    ))),
                }
            }

            Command::DeletePassword { .. } => {
                use crate::dbcrust_pass::{delete_password, list_entries};
                use inquire::Select;

                // First list available passwords
                let entries = match list_entries() {
                    Ok(entries) => entries,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to list passwords: {e}"
                        )));
                    }
                };

                if entries.is_empty() {
                    return Ok(CommandResult::Output(
                        "No saved passwords to delete.".to_string(),
                    ));
                }

                // Create selection list
                let options: Vec<String> = entries
                    .iter()
                    .map(|(db_type, host, port, database, username)| {
                        format!(
                            "{}:{}@{}:{}/{}",
                            db_type.as_str(),
                            username,
                            host,
                            port,
                            database
                        )
                    })
                    .collect();

                let selection = Select::new("Select password to delete:", options)
                    .prompt()
                    .map_err(|e| CommandError::InvalidSyntax(format!("Input error: {e}")))?;

                // Find the selected entry
                let selected_entry =
                    entries
                        .iter()
                        .find(|(db_type, host, port, database, username)| {
                            format!(
                                "{}:{}@{}:{}/{}",
                                db_type.as_str(),
                                username,
                                host,
                                port,
                                database
                            ) == selection
                        });

                if let Some((db_type, host, port_str, database, username)) = selected_entry {
                    let port: u16 = port_str.parse().map_err(|_| {
                        CommandError::InvalidSyntax("Invalid port number".to_string())
                    })?;

                    match delete_password(db_type.clone(), host, port, database, username) {
                        Ok(true) => Ok(CommandResult::Output(format!(
                            "Password deleted: {}",
                            selection
                        ))),
                        Ok(false) => Ok(CommandResult::Output(
                            "No matching password found to delete.".to_string(),
                        )),
                        Err(e) => Ok(CommandResult::Error(format!(
                            "Failed to delete password: {e}"
                        ))),
                    }
                } else {
                    Ok(CommandResult::Error(
                        "Selected entry not found.".to_string(),
                    ))
                }
            }

            Command::EncryptPasswords => {
                use crate::dbcrust_pass::encrypt_all_passwords;

                match encrypt_all_passwords() {
                    Ok(0) => Ok(CommandResult::Output(
                        "No plaintext passwords found to encrypt.".to_string(),
                    )),
                    Ok(count) => Ok(CommandResult::Output(format!(
                        "Encrypted {} password(s) in .dbcrust file.",
                        count
                    ))),
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to encrypt passwords: {e}"
                    ))),
                }
            }

            Command::ShowPoolStats => {
                let db = database.lock().unwrap();
                let connection_status = if db.is_connected().await {
                    "✅ Connected"
                } else {
                    "❌ Disconnected"
                };

                let output = format!(
                    "Connection Status: {connection_status}\n\nNote: Detailed pool statistics are no longer available.\nConnection pooling is now managed by database-specific clients."
                );

                Ok(CommandResult::Output(output))
            }

            Command::ComplexDisplayMode { mode } => {
                match mode {
                    None => {
                        // Show current mode
                        let current_mode = config.complex_display.display_mode.clone();
                        Ok(CommandResult::Output(format!(
                            "Current complex display mode: {}",
                            current_mode
                        )))
                    }
                    Some(mode_str) => {
                        // Set new mode
                        use crate::complex_display::ComplexDisplayMode;
                        match ComplexDisplayMode::from_str(mode_str) {
                            Some(new_mode) => {
                                config.complex_display.display_mode = new_mode.clone();
                                // Update global config
                                crate::complex_display::set_global_complex_config(
                                    config.complex_display.clone(),
                                );

                                // Save config
                                if let Err(e) = config.save_with_documentation() {
                                    return Ok(CommandResult::Error(format!(
                                        "Failed to save config: {}",
                                        e
                                    )));
                                }

                                Ok(CommandResult::Output(format!(
                                    "Complex display mode set to: {}",
                                    new_mode
                                )))
                            }
                            None => Ok(CommandResult::Error(
                                "Invalid mode. Available modes: full, truncated, summary, viz"
                                    .to_string(),
                            )),
                        }
                    }
                }
            }

            Command::ComplexDisplayJsonToggle => {
                // Toggle JSON pretty printing
                config.complex_display.json_pretty_print =
                    !config.complex_display.json_pretty_print;

                // Update global config
                crate::complex_display::set_global_complex_config(config.complex_display.clone());

                // Save config
                if let Err(e) = config.save_with_documentation() {
                    return Ok(CommandResult::Error(format!(
                        "Failed to save config: {}",
                        e
                    )));
                }

                let status = if config.complex_display.json_pretty_print {
                    "enabled"
                } else {
                    "disabled"
                };
                Ok(CommandResult::Output(format!(
                    "JSON pretty printing {}",
                    status
                )))
            }

            // MongoDB-specific commands
            Command::ListCollections => {
                let mut db = database.lock().unwrap();
                match db.list_collections().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No collections found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to list collections: {e}"
                    ))),
                }
            }

            Command::DescribeCollection { collection_name } => {
                let mut db = database.lock().unwrap();
                match db.describe_collection(collection_name).await {
                    Ok(details) => {
                        let output = crate::format::format_table_details(&details);
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to describe collection '{collection_name}': {e}"
                    ))),
                }
            }

            Command::ListMongoIndexes => {
                let mut db = database.lock().unwrap();
                match db.list_mongo_indexes().await {
                    Ok(results) => {
                        if results.is_empty() {
                            Ok(CommandResult::Output("No indexes found.".to_string()))
                        } else {
                            let output = if db.is_expanded_display() {
                                let tables = crate::format::format_query_results_expanded(&results);
                                tables
                                    .into_iter()
                                    .map(|t| t.to_string())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else {
                                crate::format::format_query_results_psql(&results)
                            };
                            Ok(CommandResult::Output(output))
                        }
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to list indexes: {e}"))),
                }
            }

            Command::CreateMongoIndex {
                collection,
                field,
                index_type,
            } => {
                let mut db = database.lock().unwrap();
                match db
                    .create_mongo_index(collection, field, index_type.as_deref())
                    .await
                {
                    Ok(_) => Ok(CommandResult::Output(format!(
                        "Index created successfully on collection '{}' field '{}'",
                        collection, field
                    ))),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to create index: {e}"))),
                }
            }

            Command::DropMongoIndex {
                collection,
                index_name,
            } => {
                let mut db = database.lock().unwrap();
                match db.drop_mongo_index(collection, index_name).await {
                    Ok(_) => Ok(CommandResult::Output(format!(
                        "Index '{}' dropped successfully from collection '{}'",
                        index_name, collection
                    ))),
                    Err(e) => Ok(CommandResult::Error(format!("Failed to drop index: {e}"))),
                }
            }

            Command::MongoStats => {
                let mut db = database.lock().unwrap();
                match db.mongo_stats().await {
                    Ok(results) => {
                        let output = if db.is_expanded_display() {
                            let tables = crate::format::format_query_results_expanded(&results);
                            tables
                                .into_iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            crate::format::format_query_results_psql(&results)
                        };
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to get MongoDB stats: {e}"
                    ))),
                }
            }

            Command::MongoFind {
                collection,
                filter,
                projection,
                limit,
            } => {
                let mut db = database.lock().unwrap();
                match db
                    .mongo_find(collection, filter.as_deref(), projection.as_deref(), *limit)
                    .await
                {
                    Ok(results) => {
                        let output = if db.is_expanded_display() {
                            let tables = crate::format::format_query_results_expanded(&results);
                            tables
                                .into_iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            crate::format::format_query_results_psql(&results)
                        };
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!("Failed to execute find: {e}"))),
                }
            }

            Command::MongoAggregate {
                collection,
                pipeline,
            } => {
                let mut db = database.lock().unwrap();
                match db.mongo_aggregate(collection, pipeline).await {
                    Ok(results) => {
                        let output = if db.is_expanded_display() {
                            let tables = crate::format::format_query_results_expanded(&results);
                            tables
                                .into_iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            crate::format::format_query_results_psql(&results)
                        };
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to execute aggregation: {e}"
                    ))),
                }
            }
            Command::MongoTextSearch {
                collection,
                search_term,
            } => {
                let mut db = database.lock().unwrap();
                match db.mongo_text_search(collection, search_term).await {
                    Ok(results) => {
                        let output = if db.is_expanded_display() {
                            let tables = crate::format::format_query_results_expanded(&results);
                            tables
                                .into_iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                                .join("\n")
                        } else {
                            crate::format::format_query_results_psql(&results)
                        };
                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to execute text search: {e}"
                    ))),
                }
            }

            // History management commands
            Command::ClearSessionHistory { session_hash } => {
                let history_manager =
                    match crate::history_manager::SessionHistoryManager::new(config) {
                        Ok(manager) => manager,
                        Err(e) => {
                            return Ok(CommandResult::Error(format!(
                                "Failed to create history manager: {e}"
                            )));
                        }
                    };

                match session_hash {
                    Some(hash) => {
                        // Clear specific session history
                        let histories = match history_manager.list_session_histories() {
                            Ok(h) => h,
                            Err(e) => {
                                return Ok(CommandResult::Error(format!(
                                    "Failed to list histories: {e}"
                                )));
                            }
                        };

                        if let Some(history) = histories.iter().find(|h| h.session_hash == *hash) {
                            match std::fs::remove_file(&history.path) {
                                Ok(_) => Ok(CommandResult::Output(format!(
                                    "Cleared history for session hash: {hash}"
                                ))),
                                Err(e) => Ok(CommandResult::Error(format!(
                                    "Failed to clear history: {e}"
                                ))),
                            }
                        } else {
                            Ok(CommandResult::Error(format!(
                                "No history found for session hash: {hash}"
                            )))
                        }
                    }
                    None => {
                        // Clear current session history
                        let db_guard = database.lock().unwrap();
                        if let Some(session_id) =
                            crate::history_manager::SessionId::from_database(&db_guard)
                        {
                            let history_filename = session_id.history_filename();
                            let config_dir = match crate::config::Config::get_config_dir() {
                                Ok(dir) => dir,
                                Err(e) => {
                                    return Ok(CommandResult::Error(format!(
                                        "Failed to get config directory: {e}"
                                    )));
                                }
                            };
                            let history_path = config_dir.join(&history_filename);

                            if history_path.exists() {
                                match std::fs::remove_file(&history_path) {
                                    Ok(_) => Ok(CommandResult::Output(format!(
                                        "Cleared history for current session: {}",
                                        session_id.display_name
                                    ))),
                                    Err(e) => Ok(CommandResult::Error(format!(
                                        "Failed to clear current session history: {e}"
                                    ))),
                                }
                            } else {
                                Ok(CommandResult::Output(
                                    "No history found for current session.".to_string(),
                                ))
                            }
                        } else {
                            Ok(CommandResult::Error(
                                "No session information available for current connection."
                                    .to_string(),
                            ))
                        }
                    }
                }
            }

            // AI SQL generation commands
            Command::AiAuthenticate => {
                // Check if AI SQL is enabled
                if !config.ai_sql.enabled {
                    return Ok(CommandResult::Error(
                        "AI SQL generation is not enabled. Set 'enabled = true' in [ai_sql] section of config.".to_string(),
                    ));
                }

                // Get config directory for OAuth token storage
                let config_dir = match crate::config::Config::get_config_directory() {
                    Ok(dir) => dir,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to get config directory: {}",
                            e
                        )));
                    }
                };

                // Create OAuth manager with PKCE
                let oauth_manager = match crate::ai_sql::AnthropicOAuthPkce::new(config_dir) {
                    Ok(m) => m,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to create OAuth manager: {}",
                            e
                        )));
                    }
                };

                // Start authentication flow with PKCE
                println!("🔐 Authenticating with Anthropic OAuth...\n");
                println!("This will use your Anthropic subscription for Claude API access.\n");

                // Generate PKCE challenge
                let pkce = crate::ai_sql::PkceChallenge::generate();

                // Get authorization URL
                let auth_url = oauth_manager.start_authorization(&pkce);

                println!("Please follow these steps:");
                println!("1. Open your browser and visit:");
                println!("\n   {}\n", auth_url);
                println!("2. Sign in with your Anthropic account (Claude Pro/Team)");
                println!("3. Authorize dbcrust to access your account");
                println!("4. You'll be redirected to a URL starting with:");
                println!("   https://console.anthropic.com/oauth/code/callback?code=...&state=...");
                println!("\n5. Copy the ENTIRE redirect URL and paste it here:\n");

                // Prompt for the complete redirect URL
                use inquire::Text;
                let redirect_url = match Text::new("Redirect URL:")
                    .with_help_message("Paste the complete URL from your browser address bar")
                    .prompt()
                {
                    Ok(url) => url,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to get redirect URL: {}",
                            e
                        )));
                    }
                };

                // Parse the URL to extract code and state
                println!("\nParsing authorization response...");
                let parsed_url = match url::Url::parse(redirect_url.trim()) {
                    Ok(url) => url,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Invalid URL format: {}. Please paste the complete redirect URL.",
                            e
                        )));
                    }
                };

                // Extract code parameter
                let code = match parsed_url
                    .query_pairs()
                    .find(|(key, _)| key == "code")
                    .map(|(_, value)| value.to_string())
                {
                    Some(c) => c,
                    None => {
                        return Ok(CommandResult::Error(
                            "No 'code' parameter found in URL. Please make sure you copied the complete redirect URL.".to_string()
                        ));
                    }
                };

                // Extract state parameter
                let returned_state = match parsed_url
                    .query_pairs()
                    .find(|(key, _)| key == "state")
                    .map(|(_, value)| value.to_string())
                {
                    Some(s) => s,
                    None => {
                        return Ok(CommandResult::Error(
                            "No 'state' parameter found in URL. This might indicate a problem with the OAuth flow.".to_string()
                        ));
                    }
                };

                // Validate state parameter (CSRF protection)
                if returned_state != pkce.state {
                    return Ok(CommandResult::Error(
                        "❌ Authentication failed: State parameter mismatch!\n\
                         This could indicate a security issue (CSRF attack).\n\
                         Please try again with \\aiauth".to_string()
                    ));
                }

                // Exchange code for token
                println!("Exchanging authorization code for access token...");
                match oauth_manager.exchange_code(&code, &pkce.verifier).await {
                    Ok(token) => {
                        let expires_at = token
                            .expires_at
                            .format("%Y-%m-%d %H:%M:%S UTC")
                            .to_string();
                        Ok(CommandResult::Output(format!(
                            "✅ Successfully authenticated with Anthropic!\n\
                             Token expires at: {}\n\
                             You can now use \\ai command to generate SQL queries.",
                            expires_at
                        )))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Authentication failed: {}\n\
                         Hint: Make sure you have an active Anthropic subscription.",
                        e
                    ))),
                }
            }

            Command::AiLogout => {
                // Get config directory
                let config_dir = match crate::config::Config::get_config_directory() {
                    Ok(dir) => dir,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to get config directory: {}",
                            e
                        )));
                    }
                };

                // Create OAuth manager with PKCE
                let oauth_manager = match crate::ai_sql::AnthropicOAuthPkce::new(config_dir) {
                    Ok(m) => m,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to create OAuth manager: {}",
                            e
                        )));
                    }
                };

                // Logout
                match oauth_manager.logout().await {
                    Ok(_) => Ok(CommandResult::Output(
                        "✅ Successfully logged out from Anthropic.\n\
                         Your OAuth token has been removed.".to_string(),
                    )),
                    Err(e) => Ok(CommandResult::Error(format!("Logout failed: {}", e))),
                }
            }

            Command::AiStatus => {
                // Check if AI SQL is enabled
                if !config.ai_sql.enabled {
                    return Ok(CommandResult::Output(
                        "AI SQL generation is DISABLED.\n\
                         Enable it in config: [ai_sql] enabled = true".to_string(),
                    ));
                }

                let mut status = String::new();
                status.push_str("=== AI SQL Generation Status ===\n\n");

                // Check authentication status
                status.push_str(&format!("Provider: {:?}\n", config.ai_sql.provider));
                status.push_str(&format!("Enabled: {}\n\n", config.ai_sql.enabled));

                // Check Anthropic OAuth status
                if config.ai_sql.anthropic_use_oauth {
                    let config_dir = match crate::config::Config::get_config_directory() {
                        Ok(dir) => dir,
                        Err(e) => {
                            return Ok(CommandResult::Error(format!(
                                "Failed to get config directory: {}",
                                e
                            )));
                        }
                    };

                    let oauth_manager = match crate::ai_sql::AnthropicOAuthPkce::new(config_dir) {
                        Ok(m) => m,
                        Err(e) => {
                            return Ok(CommandResult::Error(format!(
                                "Failed to create OAuth manager: {}",
                                e
                            )));
                        }
                    };
                    match oauth_manager.get_valid_token().await {
                        Ok(_token) => {
                            // Note: get_valid_token returns String, not OAuthToken
                            // We need to load the token file to get the expiry
                            match oauth_manager.load_token().await {
                                Ok(token) => {
                                    let expires_at = token
                                        .expires_at
                                        .format("%Y-%m-%d %H:%M:%S UTC")
                                        .to_string();
                                    status.push_str(&format!(
                                        "Authentication: OAuth (Authenticated ✅)\n"
                                    ));
                                    status.push_str(&format!("Token expires: {}\n", expires_at));
                                }
                                Err(_) => {
                                    status.push_str("Authentication: OAuth (Error reading token)\n");
                                    status.push_str("Run \\aiauth to re-authenticate\n");
                                }
                            }
                        }
                        Err(_) => {
                            status.push_str("Authentication: OAuth (Not authenticated ❌)\n");
                            status.push_str("Run \\aiauth to authenticate\n");
                        }
                    }
                } else if config.ai_sql.anthropic_api_key.is_some() {
                    status.push_str("Authentication: API Key (Configured ✓)\n");
                } else {
                    status.push_str("Authentication: Not configured ✗\n");
                    status.push_str("Set anthropic_api_key in config or run \\aiauth\n");
                }

                status.push_str(&format!("\nModel: {}\n", config.ai_sql.anthropic_model));
                status.push_str(&format!("Cache: {}\n", if config.ai_sql.cache_enabled { "Enabled" } else { "Disabled" }));

                Ok(CommandResult::Output(status))
            }

            Command::AiGenerate { prompt } => {
                // Check if AI SQL is enabled
                if !config.ai_sql.enabled {
                    return Ok(CommandResult::Error(
                        "AI SQL generation is not enabled. Set 'enabled = true' in [ai_sql] section of config.".to_string(),
                    ));
                }

                // Create AI SQL engine
                println!("Generating SQL query from natural language...\n");

                let mut engine = match crate::ai_sql::AiSqlEngine::new(
                    config.ai_sql.clone(),
                    Arc::clone(database),
                )
                .await
                {
                    Ok(e) => e,
                    Err(e) => {
                        return Ok(CommandResult::Error(format!(
                            "Failed to initialize AI SQL engine: {}\n\
                             Hint: Make sure you're authenticated. Run \\aiauth or configure API key.",
                            e
                        )));
                    }
                };

                // Generate SQL
                match engine.generate_sql(prompt).await {
                    Ok(response) => {
                        let mut output = String::new();
                        output.push_str("=== Generated SQL ===\n\n");
                        output.push_str(&response.sql);
                        output.push_str("\n\n");

                        if let Some(ref explanation) = response.explanation {
                            output.push_str("=== Explanation ===\n");
                            output.push_str(explanation);
                            output.push_str("\n\n");
                        }

                        output.push_str("Tip: Copy and execute the SQL above, or refine with another \\ai prompt.\n");

                        Ok(CommandResult::Output(output))
                    }
                    Err(e) => Ok(CommandResult::Error(format!(
                        "Failed to generate SQL: {}\n\
                         Your prompt: {}",
                        e, prompt
                    ))),
                }
            }

            Command::AiConfig => {
                let mut output = String::new();
                output.push_str("=== AI SQL Configuration ===\n\n");

                output.push_str(&format!("Enabled: {}\n", config.ai_sql.enabled));
                output.push_str(&format!("Provider: {:?}\n\n", config.ai_sql.provider));

                output.push_str("--- Anthropic Settings ---\n");
                output.push_str(&format!("Use OAuth: {}\n", config.ai_sql.anthropic_use_oauth));
                output.push_str(&format!(
                    "API Key: {}\n",
                    if config.ai_sql.anthropic_api_key.is_some() {
                        "Configured"
                    } else {
                        "Not set"
                    }
                ));
                output.push_str(&format!("Model: {}\n", config.ai_sql.anthropic_model));
                output.push_str(&format!("Base URL: {}\n\n", config.ai_sql.anthropic_base_url));

                output.push_str("--- Cache Settings ---\n");
                output.push_str(&format!("Enabled: {}\n", config.ai_sql.cache_enabled));
                output.push_str(&format!(
                    "TTL: {} seconds ({} minutes)\n\n",
                    config.ai_sql.cache_ttl_seconds,
                    config.ai_sql.cache_ttl_seconds / 60
                ));

                output.push_str("--- Generation Settings ---\n");
                output.push_str(&format!("Temperature: {}\n", config.ai_sql.temperature));
                output.push_str(&format!("Max tokens: {}\n", config.ai_sql.max_tokens));
                output.push_str(&format!("Timeout: {} seconds\n", config.ai_sql.timeout_seconds));
                output.push_str(&format!("Include schema: {}\n", config.ai_sql.include_schema));
                output.push_str(&format!("Schema depth: {:?}\n", config.ai_sql.schema_depth));

                output.push_str("\nEdit ~/.config/dbcrust/config.toml to change these settings.\n");

                Ok(CommandResult::Output(output))
            }

            Command::AiClearCache => {
                // Note: We can't actually clear the cache here since we don't have a persistent engine
                // The cache is per-engine instance, which is created on-demand
                Ok(CommandResult::Output(
                    "✓ AI query cache will be cleared on next query generation.\n\
                     Note: Cache is per-session and automatically expires after the configured TTL.".to_string(),
                ))
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
            Command::ClearSessionHistory { .. } => "Clear session command history",
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
            Command::ExplainFormatted { .. } => {
                "Execute EXPLAIN query (same as explain mode, supports \\ecopy)"
            }
            Command::ExplainExport { .. } => "Execute EXPLAIN query and export to file",
            Command::SetMultilineIndicator { .. } => "Set custom multiline prompt indicator",
            Command::TogglePager => "Toggle pager for long output",
            Command::ToggleBanner => "Toggle startup banner display",
            Command::ToggleServerInfo => "Toggle server info display on connection",
            Command::ToggleAutocomplete => "Toggle autocomplete functionality",
            Command::ToggleColumnSelection => "Toggle forced column selection mode (on/off)",
            Command::SetColumnSelectionThreshold { .. } => "Set column selection threshold",
            Command::ClearColumnViews => "Clear saved column views",
            Command::ResetView => "Reset all view settings to defaults",
            // Vector display commands
            Command::SetVectorDisplayMode { .. } => {
                "Set vector display mode (full, truncated, summary, viz)"
            }
            Command::ShowVectorDisplayConfig => "Show current vector display configuration",
            Command::ToggleVectorStatistics => "Toggle vector statistics display",
            Command::ShowPoolStats => "Show connection pool statistics",
            // Complex display commands
            Command::ComplexDisplayMode { .. } => "Set complex data display mode",
            Command::ComplexDisplayJsonToggle => "Toggle JSON pretty printing",
            // Vault credential cache commands
            Command::VaultCacheStatus => "Show vault credential cache status",
            Command::VaultCacheClear => "Clear all cached vault credentials",
            Command::VaultCacheRefresh { .. } => "Refresh vault credential cache",
            Command::VaultCacheExpired => "Show expired vault credentials",
            // Password management commands
            Command::SavePassword { .. } => "Save password to .dbcrust file (interactive)",
            Command::ListPasswords => "List stored passwords (without showing passwords)",
            Command::DeletePassword { .. } => "Delete stored password (interactive)",
            Command::EncryptPasswords => "Encrypt all plaintext passwords in .dbcrust",
            // MongoDB-specific commands
            Command::ListCollections => "List collections in current database",
            Command::DescribeCollection { .. } => "Describe collection structure",
            Command::ListMongoIndexes => "List MongoDB indexes",
            Command::CreateMongoIndex { .. } => "Create MongoDB index",
            Command::DropMongoIndex { .. } => "Drop MongoDB index",
            Command::MongoStats => "Show MongoDB database statistics",
            Command::MongoFind { .. } => "Execute MongoDB find query",
            Command::MongoAggregate { .. } => "Execute MongoDB aggregation pipeline",
            Command::MongoTextSearch { .. } => "Execute MongoDB text search",
            // AI SQL generation commands
            Command::AiAuthenticate => "Authenticate with Anthropic using OAuth",
            Command::AiLogout => "Logout from Anthropic OAuth session",
            Command::AiStatus => "Show AI authentication status",
            Command::AiGenerate { .. } => "Generate SQL from natural language prompt",
            Command::AiConfig => "Show AI SQL configuration",
            Command::AiClearCache => "Clear AI query cache",
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
            Command::SaveNamedQuery { .. } => {
                "\\ns [--global|--postgres|--mysql|--sqlite] <name> <query>"
            }
            Command::DeleteNamedQuery { .. } => "\\nd <name>",
            Command::ExecuteNamedQuery { .. } => "\\n <name> [args...]",
            Command::ListNamedQueries => "\\n",
            Command::ListSessions => "\\s",
            Command::SaveSession { .. } => "\\ss <name>",
            Command::DeleteSession { .. } => "\\sd <name>",
            Command::ConnectSession { .. } => "\\s <name>",
            Command::ListRecentConnections => "\\r",
            Command::ClearRecentConnections => "\\rc",
            Command::ClearSessionHistory { .. } => "\\hc [session_hash]",
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
            Command::ToggleServerInfo => "\\serverinfo",
            Command::ToggleAutocomplete => "\\a",
            Command::ToggleColumnSelection => "\\cs",
            Command::SetColumnSelectionThreshold { .. } => "\\csthreshold <number>",
            Command::ClearColumnViews => "\\clrcs",
            Command::ResetView => "\\resetview",
            // Vector display commands
            Command::SetVectorDisplayMode { .. } => "\\vd <mode>",
            Command::ShowVectorDisplayConfig => "\\vdc",
            Command::ToggleVectorStatistics => "\\vs",
            Command::ShowPoolStats => "\\ps",
            // Complex display commands
            Command::ComplexDisplayMode { .. } => "\\cd [mode]",
            Command::ComplexDisplayJsonToggle => "\\cdj",
            // Vault credential cache commands
            Command::VaultCacheStatus => "\\vc",
            Command::VaultCacheClear => "\\vcc",
            Command::VaultCacheRefresh { .. } => "\\vcr [role]",
            Command::VaultCacheExpired => "\\vce",
            // Password management commands
            Command::SavePassword { .. } => "\\savepass",
            Command::ListPasswords => "\\listpass",
            Command::DeletePassword { .. } => "\\deletepass",
            Command::EncryptPasswords => "\\encryptpass",
            // MongoDB-specific commands
            Command::ListCollections => "\\collections",
            Command::DescribeCollection { .. } => "\\dc <collection_name>",
            Command::ListMongoIndexes => "\\dmi",
            Command::CreateMongoIndex { .. } => "\\cmi <collection> <field> [type]",
            Command::DropMongoIndex { .. } => "\\ddmi <collection> <index_name>",
            Command::MongoStats => "\\mstats",
            Command::MongoFind { .. } => "\\find <collection> [filter] [projection] [limit]",
            Command::MongoAggregate { .. } => "\\aggregate <collection> <pipeline>",
            Command::MongoTextSearch { .. } => "\\search <collection> <search_term>",
            // AI SQL generation commands
            Command::AiAuthenticate => "\\aiauth",
            Command::AiLogout => "\\ailogout",
            Command::AiStatus => "\\aistatus",
            Command::AiGenerate { .. } => "\\ai <prompt>",
            Command::AiConfig => "\\aiconfig",
            Command::AiClearCache => "\\aiclearcache",
        }
    }

    fn category(&self) -> CommandCategory {
        match self {
            Command::Quit | Command::Help => CommandCategory::Core,
            Command::ListDatabases
            | Command::ListTables
            | Command::DescribeTable { .. }
            | Command::ConnectDatabase { .. } => CommandCategory::DatabaseNavigation,
            Command::ToggleExpandedDisplay | Command::ToggleExplainMode | Command::ShowConfig => {
                CommandCategory::DisplayOptions
            }
            Command::WriteScript { .. }
            | Command::LoadScript { .. }
            | Command::EditMultiline
            | Command::CopyExplainPlan => CommandCategory::ScriptHandling,
            Command::ListNamedQueries
            | Command::SaveNamedQuery { .. }
            | Command::DeleteNamedQuery { .. }
            | Command::ExecuteNamedQuery { .. } => CommandCategory::NamedQueries,
            Command::ListSessions
            | Command::SaveSession { .. }
            | Command::DeleteSession { .. }
            | Command::ConnectSession { .. } => CommandCategory::SessionManagement,
            Command::ListRecentConnections | Command::ClearRecentConnections => {
                CommandCategory::ConnectionHistory
            }
            Command::ClearSessionHistory { .. } => CommandCategory::HistoryManagement,
            Command::ListUsers
            | Command::ListIndexes
            | Command::ListPragmas
            | Command::ShowPgpass
            | Command::ShowMyconf
            | Command::ListDockerContainers => CommandCategory::DatabaseSpecific,
            Command::ExplainRaw { .. }
            | Command::ExplainFormatted { .. }
            | Command::ExplainExport { .. }
            | Command::ShowPoolStats => CommandCategory::Advanced,
            // Complex display commands
            Command::ComplexDisplayMode { .. } | Command::ComplexDisplayJsonToggle => {
                CommandCategory::DisplayOptions
            }
            Command::SetMultilineIndicator { .. }
            | Command::TogglePager
            | Command::ToggleBanner
            | Command::ToggleServerInfo
            | Command::ToggleAutocomplete
            | Command::ToggleColumnSelection
            | Command::SetColumnSelectionThreshold { .. }
            | Command::ClearColumnViews
            | Command::ResetView => CommandCategory::DisplayOptions,
            // Vector display commands
            Command::SetVectorDisplayMode { .. }
            | Command::ShowVectorDisplayConfig
            | Command::ToggleVectorStatistics => CommandCategory::DisplayOptions,
            Command::VaultCacheStatus
            | Command::VaultCacheClear
            | Command::VaultCacheRefresh { .. }
            | Command::VaultCacheExpired => CommandCategory::VaultManagement,
            // Password management commands
            Command::SavePassword { .. }
            | Command::ListPasswords
            | Command::DeletePassword { .. }
            | Command::EncryptPasswords => CommandCategory::PasswordManagement,
            // MongoDB-specific commands
            Command::ListCollections => CommandCategory::DatabaseNavigation,
            Command::DescribeCollection { .. } => CommandCategory::DatabaseNavigation,
            Command::ListMongoIndexes => CommandCategory::DatabaseSpecific,
            Command::CreateMongoIndex { .. } => CommandCategory::DatabaseSpecific,
            Command::DropMongoIndex { .. } => CommandCategory::DatabaseSpecific,
            Command::MongoStats => CommandCategory::DatabaseSpecific,
            Command::MongoFind { .. } => CommandCategory::DatabaseSpecific,
            Command::MongoAggregate { .. } => CommandCategory::DatabaseSpecific,
            Command::MongoTextSearch { .. } => CommandCategory::DatabaseSpecific,
            // AI SQL generation commands
            Command::AiAuthenticate
            | Command::AiLogout
            | Command::AiStatus
            | Command::AiGenerate { .. }
            | Command::AiConfig
            | Command::AiClearCache => CommandCategory::AiSqlGeneration,
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
        assert_eq!(
            CommandParser::parse("\\d").unwrap(),
            Command::DescribeTable { table_name: None }
        );
        assert_eq!(
            CommandParser::parse("\\d users").unwrap(),
            Command::DescribeTable {
                table_name: Some("users".to_string())
            }
        );

        // Test commands with arguments
        assert_eq!(
            CommandParser::parse("\\c testdb").unwrap(),
            Command::ConnectDatabase {
                database_name: "testdb".to_string()
            }
        );
        assert_eq!(
            CommandParser::parse("\\w script.sql").unwrap(),
            Command::WriteScript {
                filename: "script.sql".to_string()
            }
        );

        // Test named queries
        assert_eq!(
            CommandParser::parse("\\ns test SELECT 1").unwrap(),
            Command::SaveNamedQuery {
                name: "test".to_string(),
                query: "SELECT 1".to_string(),
                global: false,
                postgres: false,
                mysql: false,
                sqlite: false,
            }
        );

        // Test error cases
        assert!(matches!(
            CommandParser::parse("\\c"),
            Err(CommandError::MissingArgument(_))
        ));
        assert!(matches!(
            CommandParser::parse("\\unknown"),
            Err(CommandError::UnknownCommand(_))
        ));
        assert!(matches!(
            CommandParser::parse("not_a_command"),
            Err(CommandError::InvalidSyntax(_))
        ));
    }

    #[test]
    fn test_individual_command_categories() {
        assert_eq!(Command::Quit.category(), CommandCategory::Core);
        assert_eq!(
            Command::ListDatabases.category(),
            CommandCategory::DatabaseNavigation
        );
        assert_eq!(
            Command::ToggleExpandedDisplay.category(),
            CommandCategory::DisplayOptions
        );
    }

    #[test]
    fn test_get_command_names() {
        let names = CommandParser::get_command_names();
        assert!(names.contains(&"\\q"));
        assert!(names.contains(&"\\h"));
        assert!(names.contains(&"\\l"));
        assert!(names.contains(&"\\dt"));
        // Test our new complex display commands
        assert!(
            names.contains(&"\\cd"),
            "\\cd command should be in autocomplete"
        );
        assert!(
            names.contains(&"\\cdj"),
            "\\cdj command should be in autocomplete"
        );
        assert!(names.len() > 20); // Should have all our commands
    }

    #[test]
    fn test_command_categories() {
        let categories = CommandParser::get_commands_by_category();
        assert!(categories.len() >= 6); // We have 6+ categories

        // Verify each category has commands
        for (category, commands) in categories {
            assert!(
                !commands.is_empty(),
                "Category {category:?} should have commands"
            );
        }
    }

    #[test]
    fn test_command_descriptions_and_usage() {
        let test_commands = vec![
            Command::Quit,
            Command::Help,
            Command::ListDatabases,
            Command::ListTables,
            Command::DescribeTable {
                table_name: Some("test".to_string()),
            },
            Command::SaveSession {
                name: "test".to_string(),
            },
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
        assert_eq!(
            CommandParser::parse("\\s production").unwrap(),
            Command::ConnectSession {
                name: "production".to_string()
            }
        );

        // Test session saving
        assert_eq!(
            CommandParser::parse("\\ss staging").unwrap(),
            Command::SaveSession {
                name: "staging".to_string()
            }
        );

        // Test session deletion
        assert_eq!(
            CommandParser::parse("\\sd old_session").unwrap(),
            Command::DeleteSession {
                name: "old_session".to_string()
            }
        );
    }

    #[test]
    fn test_named_query_commands() {
        // Test named query listing
        assert_eq!(
            CommandParser::parse("\\n").unwrap(),
            Command::ListNamedQueries
        );

        // Test named query execution
        assert_eq!(
            CommandParser::parse("\\n get_users").unwrap(),
            Command::ExecuteNamedQuery {
                name: "get_users".to_string(),
                args: vec![]
            }
        );

        // Test named query execution with args
        assert_eq!(
            CommandParser::parse("\\n get_user_by_id 123").unwrap(),
            Command::ExecuteNamedQuery {
                name: "get_user_by_id".to_string(),
                args: vec!["123".to_string()]
            }
        );

        // Test named query saving
        assert_eq!(
            CommandParser::parse("\\ns get_all SELECT * FROM users").unwrap(),
            Command::SaveNamedQuery {
                name: "get_all".to_string(),
                query: "SELECT * FROM users".to_string(),
                global: false,
                postgres: false,
                mysql: false,
                sqlite: false,
            }
        );

        // Test named query deletion
        assert_eq!(
            CommandParser::parse("\\nd old_query").unwrap(),
            Command::DeleteNamedQuery {
                name: "old_query".to_string()
            }
        );

        // Test named query saving with global scope
        assert_eq!(
            CommandParser::parse("\\ns -g global_query SELECT 1").unwrap(),
            Command::SaveNamedQuery {
                name: "global_query".to_string(),
                query: "SELECT 1".to_string(),
                global: true,
                postgres: false,
                mysql: false,
                sqlite: false,
            }
        );

        // Test named query saving with database type scope
        assert_eq!(
            CommandParser::parse("\\ns --postgres pg_query SELECT version()").unwrap(),
            Command::SaveNamedQuery {
                name: "pg_query".to_string(),
                query: "SELECT version()".to_string(),
                global: false,
                postgres: true,
                mysql: false,
                sqlite: false,
            }
        );
    }

    #[test]
    fn test_database_specific_commands() {
        assert_eq!(CommandParser::parse("\\du").unwrap(), Command::ListUsers);
        assert_eq!(CommandParser::parse("\\di").unwrap(), Command::ListIndexes);
        assert_eq!(
            CommandParser::parse("\\pgpass").unwrap(),
            Command::ShowPgpass
        );
        assert_eq!(
            CommandParser::parse("\\myconf").unwrap(),
            Command::ShowMyconf
        );
    }

    #[test]
    fn test_script_commands() {
        assert_eq!(
            CommandParser::parse("\\w script.sql").unwrap(),
            Command::WriteScript {
                filename: "script.sql".to_string()
            }
        );

        assert_eq!(
            CommandParser::parse("\\i load.sql").unwrap(),
            Command::LoadScript {
                filename: "load.sql".to_string()
            }
        );

        assert_eq!(
            CommandParser::parse("\\ed").unwrap(),
            Command::EditMultiline
        );
        assert_eq!(
            CommandParser::parse("\\ecopy").unwrap(),
            Command::CopyExplainPlan
        );
    }

    #[test]
    fn test_error_cases() {
        // Missing required arguments
        assert!(matches!(
            CommandParser::parse("\\c"),
            Err(CommandError::MissingArgument(_))
        ));
        assert!(matches!(
            CommandParser::parse("\\w"),
            Err(CommandError::MissingArgument(_))
        ));
        assert!(matches!(
            CommandParser::parse("\\ss"),
            Err(CommandError::MissingArgument(_))
        ));
        assert!(matches!(
            CommandParser::parse("\\ns test"),
            Err(CommandError::MissingArgument(_))
        ));

        // Invalid command syntax
        assert!(matches!(
            CommandParser::parse("not_a_command"),
            Err(CommandError::InvalidSyntax(_))
        ));
        assert!(matches!(
            CommandParser::parse("regular text"),
            Err(CommandError::InvalidSyntax(_))
        ));

        // Unknown commands
        assert!(matches!(
            CommandParser::parse("\\unknown"),
            Err(CommandError::UnknownCommand(_))
        ));
        assert!(matches!(
            CommandParser::parse("\\xyz"),
            Err(CommandError::UnknownCommand(_))
        ));
    }

    #[test]
    fn test_advanced_commands() {
        // Test EXPLAIN variants
        assert_eq!(
            CommandParser::parse("\\er SELECT 1").unwrap(),
            Command::ExplainRaw {
                query: "SELECT 1".to_string()
            }
        );

        assert_eq!(
            CommandParser::parse("\\ef SELECT 1").unwrap(),
            Command::ExplainFormatted {
                query: "SELECT 1".to_string()
            }
        );

        assert_eq!(
            CommandParser::parse("\\ex SELECT 1 output.txt").unwrap(),
            Command::ExplainExport {
                query: "SELECT 1".to_string(),
                filename: "output.txt".to_string()
            }
        );

        // Test threshold setting
        assert_eq!(
            CommandParser::parse("\\csthreshold 50").unwrap(),
            Command::SetColumnSelectionThreshold { threshold: 50 }
        );

        // Test multiline indicator
        assert_eq!(
            CommandParser::parse("\\setmulti >").unwrap(),
            Command::SetMultilineIndicator {
                indicator: ">".to_string()
            }
        );
    }
}
