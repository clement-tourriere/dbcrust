use crate::database::{ConnectionInfo, DatabaseType, DatabaseTypeExt};
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use dirs::home_dir;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::debug;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum VerbosityLevel {
    Quiet,   // Only essential info and SSH tunnels
    Normal,  // Default - minimal connection info
    Verbose, // Current behavior - all connection steps
}

impl Default for VerbosityLevel {
    fn default() -> Self {
        VerbosityLevel::Normal
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SSHTunnelConfig {
    pub enabled: bool,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_username: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_key_path: Option<String>,
}

impl Default for SSHTunnelConfig {
    fn default() -> Self {
        SSHTunnelConfig {
            enabled: false,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_username: None,
            ssh_password: None,
            ssh_key_path: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RecentConnection {
    pub connection_url: String, // Full URL with all details except password
    pub display_name: String,   // Human-readable description for selection
    pub timestamp: DateTime<Utc>,
    pub database_type: DatabaseType,
    pub success: bool,
    // Additional connection options (includes vault metadata for vault connections)
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// Recent connections storage - stored in a separate file
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RecentConnectionsStorage {
    #[serde(default)]
    pub connections: Vec<RecentConnection>,
}

/// Saved sessions storage - stored in a separate file
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SavedSessionsStorage {
    #[serde(default)]
    pub sessions: HashMap<String, SavedSession>,
}

/// Cached Vault credentials - stored in encrypted file
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CachedVaultCredentials {
    pub username: String,
    pub password: String, // Encrypted at rest in the file
    pub lease_id: String,
    pub lease_duration: u64, // Original TTL in seconds
    pub issue_time: DateTime<Utc>,
    pub expire_time: DateTime<Utc>,
    pub renewable: bool,
    pub mount_path: String,
    pub database_name: String,
    pub role_name: String,
}

/// Vault credential storage - stored in a separate encrypted file
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct VaultCredentialStorage {
    // Key: "mount_path/database_name/role_name"
    #[serde(default)]
    pub cached_credentials: HashMap<String, CachedVaultCredentials>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedSession {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub dbname: String,
    // No password here - passwords are stored in database-specific credential files:
    // - PostgreSQL: ~/.pgpass file
    // - MySQL: ~/.my.cnf file
    // - SQLite: no password needed (file-based)
    #[serde(default)]
    pub ssh_tunnel: Option<SSHTunnelConfig>,
    // Database type for multi-database support
    #[serde(default = "default_database_type")]
    pub database_type: DatabaseType,
    // File path for SQLite databases
    #[serde(default)]
    pub file_path: Option<String>,
    // Additional connection options (query parameters)
    #[serde(default)]
    pub options: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    #[serde(rename = "trace")]
    Trace,
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warn")]
    Warn,
    #[serde(rename = "error")]
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "trace"),
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoggingConfig {
    #[serde(default)]
    pub level: LogLevel,
    #[serde(default = "default_console_output")]
    pub console_output: bool,
    #[serde(default = "default_file_output")]
    pub file_output: bool,
    #[serde(default = "default_log_file_path")]
    pub file_path: String,
    #[serde(default = "default_max_file_size")]
    pub max_file_size_mb: u64,
    #[serde(default = "default_max_files")]
    pub max_files: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            level: LogLevel::Info,
            console_output: default_console_output(),
            file_output: default_file_output(),
            file_path: default_log_file_path(),
            max_file_size_mb: default_max_file_size(),
            max_files: default_max_files(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HistoryConfig {
    /// Enable per-session history (default: true)
    #[serde(default = "default_per_session_enabled")]
    pub per_session_enabled: bool,
    /// Maximum number of history files to keep (default: 50)
    #[serde(default = "default_max_history_files")]
    pub max_history_files: usize,
    /// Clean up old unused history files after N days (default: 90)
    #[serde(default = "default_cleanup_after_days")]
    pub cleanup_after_days: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            per_session_enabled: default_per_session_enabled(),
            max_history_files: default_max_history_files(),
            cleanup_after_days: default_cleanup_after_days(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_default_limit")]
    pub default_limit: usize,
    #[serde(default = "default_expanded_display_default")]
    pub expanded_display_default: bool,
    #[serde(default = "default_autocomplete_enabled")]
    pub autocomplete_enabled: bool,
    #[serde(default = "default_explain_mode_default")]
    pub explain_mode_default: bool,
    #[serde(default = "default_column_selection_threshold")]
    pub column_selection_threshold: usize,
    #[serde(default = "default_column_selection_default_all")]
    pub column_selection_default_all: bool,
    #[serde(default)]
    pub named_queries: HashMap<String, String>,
    #[serde(default)]
    pub ssh_tunnel_patterns: HashMap<String, String>,
    #[serde(default = "default_max_recent_connections")]
    pub max_recent_connections: usize,

    #[serde(default = "default_pager_enabled")]
    pub pager_enabled: bool,
    #[serde(default = "default_pager_command")]
    pub pager_command: String,
    #[serde(default = "default_pager_threshold_lines")]
    pub pager_threshold_lines: usize, // 0 means use terminal height

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub history: HistoryConfig,

    #[serde(default = "default_show_banner")]
    pub show_banner: bool,

    #[serde(default = "default_verbosity_level")]
    pub verbosity_level: VerbosityLevel,

    #[serde(default = "default_multiline_prompt_indicator")]
    pub multiline_prompt_indicator: String,

    // Vault credential caching settings
    #[serde(default = "default_vault_cache_enabled")]
    pub vault_credential_cache_enabled: bool,
    #[serde(default = "default_vault_renewal_threshold")]
    pub vault_cache_renewal_threshold: f64, // 0.25 = 25%
    #[serde(default = "default_vault_min_ttl")]
    pub vault_cache_min_ttl_seconds: u64, // 300 = 5 minutes

    // Query timeout settings
    #[serde(default = "default_query_timeout")]
    pub query_timeout_seconds: u64, // 30 = 30 seconds
    #[serde(default = "default_metadata_timeout")]
    pub metadata_timeout_seconds: u64, // 10 = 10 seconds

    // Recent connections - not serialized with main config, stored separately
    #[serde(skip)]
    recent_connections_storage: RecentConnectionsStorage,

    // Saved sessions - not serialized with main config, stored separately
    #[serde(skip)]
    saved_sessions_storage: SavedSessionsStorage,

    // Vault credentials - not serialized with main config, stored separately in encrypted file
    #[serde(skip)]
    vault_credential_storage: VaultCredentialStorage,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_limit: 100,
            expanded_display_default: false,
            autocomplete_enabled: true,
            explain_mode_default: false,
            column_selection_threshold: default_column_selection_threshold(),
            column_selection_default_all: default_column_selection_default_all(),
            named_queries: HashMap::new(),
            ssh_tunnel_patterns: HashMap::new(),
            max_recent_connections: default_max_recent_connections(),
            pager_enabled: default_pager_enabled(),
            pager_command: default_pager_command(),
            pager_threshold_lines: default_pager_threshold_lines(),
            logging: LoggingConfig::default(),
            history: HistoryConfig::default(),
            show_banner: default_show_banner(),
            verbosity_level: default_verbosity_level(),
            multiline_prompt_indicator: default_multiline_prompt_indicator(),
            vault_credential_cache_enabled: default_vault_cache_enabled(),
            vault_cache_renewal_threshold: default_vault_renewal_threshold(),
            vault_cache_min_ttl_seconds: default_vault_min_ttl(),
            query_timeout_seconds: default_query_timeout(),
            metadata_timeout_seconds: default_metadata_timeout(),
            recent_connections_storage: {
                // For tests, use empty storage to avoid loading user data
                let is_test = std::env::var("RUST_TEST_MODE").is_ok()
                    || std::thread::current()
                        .name()
                        .map(|name| name.contains("test"))
                        .unwrap_or(false);

                if is_test {
                    RecentConnectionsStorage::default()
                } else {
                    Self::load_recent_connections()
                }
            },
            saved_sessions_storage: {
                // For tests, use empty storage to avoid loading user data
                let is_test = std::env::var("RUST_TEST_MODE").is_ok()
                    || std::thread::current()
                        .name()
                        .map(|name| name.contains("test"))
                        .unwrap_or(false);

                if is_test {
                    SavedSessionsStorage::default()
                } else {
                    Self::load_saved_sessions()
                }
            },
            vault_credential_storage: {
                // For tests, use empty storage to avoid loading user data
                let is_test = std::env::var("RUST_TEST_MODE").is_ok()
                    || std::thread::current()
                        .name()
                        .map(|name| name.contains("test"))
                        .unwrap_or(false);

                if is_test {
                    VaultCredentialStorage::default()
                } else {
                    Self::load_vault_credentials()
                }
            },
        }
    }
}

fn default_default_limit() -> usize {
    100
}

fn default_expanded_display_default() -> bool {
    false
}

fn default_autocomplete_enabled() -> bool {
    true
}

fn default_explain_mode_default() -> bool {
    false
}

fn default_column_selection_threshold() -> usize {
    10
}

fn default_column_selection_default_all() -> bool {
    false // Default to no columns pre-selected (opt-in behavior)
}

fn default_max_recent_connections() -> usize {
    10
}

fn default_pager_enabled() -> bool {
    true
}

fn default_pager_command() -> String {
    "less -R".to_string()
}

fn default_pager_threshold_lines() -> usize {
    0 // 0 interpreted as: use terminal height if available, else default to 25-30 lines
}

fn default_console_output() -> bool {
    true
}

fn default_file_output() -> bool {
    false
}

fn default_log_file_path() -> String {
    // Use config directory + logs/dbcrust.log
    if let Ok(config_dir) = Config::get_config_dir() {
        let log_dir = config_dir.join("logs");
        log_dir.join("dbcrust.log").to_string_lossy().to_string()
    } else {
        "dbcrust.log".to_string()
    }
}

fn default_max_file_size() -> u64 {
    10 // 10 MB
}

fn default_max_files() -> usize {
    5 // Keep 5 rotated files
}

fn default_show_banner() -> bool {
    false
}

fn default_verbosity_level() -> VerbosityLevel {
    VerbosityLevel::Normal
}

fn default_multiline_prompt_indicator() -> String {
    String::new() // Empty string by default (no indicator)
}

fn default_vault_cache_enabled() -> bool {
    true
}

fn default_vault_renewal_threshold() -> f64 {
    0.25 // Renew when 25% of TTL remains
}

fn default_vault_min_ttl() -> u64 {
    300 // Don't cache credentials with less than 5 minutes TTL
}

fn default_query_timeout() -> u64 {
    30 // 30 seconds default query timeout
}

fn default_metadata_timeout() -> u64 {
    10 // 10 seconds default metadata timeout
}

fn default_per_session_enabled() -> bool {
    true // Enable per-session history by default
}

fn default_max_history_files() -> usize {
    50 // Keep up to 50 history files
}

fn default_cleanup_after_days() -> u64 {
    90 // Clean up history files older than 90 days
}

fn default_database_type() -> DatabaseType {
    DatabaseType::PostgreSQL
}

// Global verbosity override for command-line arguments
static VERBOSITY_OVERRIDE: std::sync::OnceLock<std::sync::Mutex<Option<VerbosityLevel>>> =
    std::sync::OnceLock::new();

// Global config loading lock to prevent simultaneous config loads during error recovery
static CONFIG_LOADING_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// Global config saving lock to prevent recursive config saves during error recovery
static CONFIG_SAVING_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Set a global verbosity override that will be used instead of the config file setting
pub fn set_global_verbosity_override(level: Option<VerbosityLevel>) {
    if let Ok(mut override_val) = VERBOSITY_OVERRIDE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *override_val = level;
    }
}

/// Get the current verbosity override, if any
pub fn get_global_verbosity_override() -> Option<VerbosityLevel> {
    VERBOSITY_OVERRIDE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()
        .and_then(|val| *val)
}

impl Config {
    /// Get the configuration directory path - single source of truth for all config files
    /// Returns a temp directory during tests, real config directory otherwise
    pub fn get_config_directory() -> Result<PathBuf, Box<dyn Error>> {
        // Detect test mode using multiple strategies since cfg!(test) doesn't work across crate boundaries
        let is_test = std::env::var("RUST_TEST_MODE").is_ok()
            || std::thread::current()
                .name()
                .map(|name| name.contains("test"))
                .unwrap_or(false);

        if is_test {
            // For tests, use a temp directory based on process ID
            let temp_dir = std::env::temp_dir();
            let pid = std::process::id();
            let test_dir = temp_dir.join(format!("dbcrust_test_{pid}"));

            if !test_dir.exists() {
                fs::create_dir_all(&test_dir)?;
            }
            Ok(test_dir)
        } else {
            // For production, use the real config directory
            if let Some(config_dir) = get_config_dir_impl() {
                if !config_dir.exists() {
                    fs::create_dir_all(&config_dir)?;
                }
                Ok(config_dir)
            } else {
                Err("Failed to get configuration directory".into())
            }
        }
    }

    /// DEPRECATED: Use get_config_directory() instead
    pub fn get_config_dir() -> Result<PathBuf, Box<dyn Error>> {
        Self::get_config_directory()
    }

    /// Get the path to the recent connections file
    pub fn get_recent_connections_path() -> Result<PathBuf, Box<dyn Error>> {
        Ok(Self::get_config_directory()?.join("recent.toml"))
    }

    /// Get the path to the saved sessions file
    pub fn get_saved_sessions_path() -> Result<PathBuf, Box<dyn Error>> {
        Ok(Self::get_config_directory()?.join("sessions.toml"))
    }

    /// Get the path to the vault credentials file (encrypted)
    pub fn get_vault_credentials_path() -> Result<PathBuf, Box<dyn Error>> {
        Ok(Self::get_config_directory()?.join("vault_credentials.enc"))
    }

    /// Load recent connections from separate file
    fn load_recent_connections() -> RecentConnectionsStorage {
        match Self::get_recent_connections_path() {
            Ok(path) => {
                if path.exists() {
                    match fs::read_to_string(&path) {
                        Ok(content) => match toml::from_str(&content) {
                            Ok(storage) => storage,
                            Err(e) => {
                                eprintln!("Error parsing recent connections file: {e}");
                                RecentConnectionsStorage::default()
                            }
                        },
                        Err(e) => {
                            eprintln!("Error reading recent connections file: {e}");
                            RecentConnectionsStorage::default()
                        }
                    }
                } else {
                    // File doesn't exist, check if we need to migrate from old config format
                    let migrated_connections = Self::migrate_recent_connections_if_needed();
                    if !migrated_connections.is_empty() {
                        let storage = RecentConnectionsStorage {
                            connections: migrated_connections,
                        };
                        // Save the migrated connections to the new file
                        if let Ok(content) = toml::to_string_pretty(&storage) {
                            if let Err(e) = fs::write(&path, content) {
                                eprintln!("Error saving migrated recent connections: {e}");
                            }
                        }
                        storage
                    } else {
                        RecentConnectionsStorage::default()
                    }
                }
            }
            Err(e) => {
                eprintln!("Error getting recent connections path: {e}");
                RecentConnectionsStorage::default()
            }
        }
    }

    /// Save recent connections to separate file
    fn save_recent_connections(&self) -> Result<(), Box<dyn Error>> {
        let path = Self::get_recent_connections_path()?;
        let content = toml::to_string_pretty(&self.recent_connections_storage)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Load saved sessions from separate file
    fn load_saved_sessions() -> SavedSessionsStorage {
        match Self::get_saved_sessions_path() {
            Ok(path) => {
                if path.exists() {
                    match fs::read_to_string(&path) {
                        Ok(content) => match toml::from_str(&content) {
                            Ok(storage) => storage,
                            Err(e) => {
                                eprintln!("Error parsing saved sessions file: {e}");
                                SavedSessionsStorage::default()
                            }
                        },
                        Err(e) => {
                            eprintln!("Error reading saved sessions file: {e}");
                            SavedSessionsStorage::default()
                        }
                    }
                } else {
                    // File doesn't exist, check if we need to migrate from old config format
                    let migrated_sessions = Self::migrate_saved_sessions_if_needed();
                    if !migrated_sessions.is_empty() {
                        let storage = SavedSessionsStorage {
                            sessions: migrated_sessions,
                        };
                        // Save the migrated sessions to the new file
                        if let Ok(content) = toml::to_string_pretty(&storage) {
                            if let Err(e) = fs::write(&path, content) {
                                eprintln!("Error saving migrated saved sessions: {e}");
                            }
                        }
                        storage
                    } else {
                        SavedSessionsStorage::default()
                    }
                }
            }
            Err(e) => {
                eprintln!("Error getting saved sessions path: {e}");
                SavedSessionsStorage::default()
            }
        }
    }

    /// Save saved sessions to separate file
    fn save_saved_sessions(&self) -> Result<(), Box<dyn Error>> {
        let path = Self::get_saved_sessions_path()?;
        let content = toml::to_string_pretty(&self.saved_sessions_storage)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Load vault credentials from encrypted file
    fn load_vault_credentials() -> VaultCredentialStorage {
        match Self::get_vault_credentials_path() {
            Ok(path) => {
                if path.exists() {
                    match fs::read(&path) {
                        Ok(encrypted_data) => {
                            match crate::vault_encryption::decrypt_data(&encrypted_data) {
                                Ok(decrypted_data) => {
                                    match toml::from_slice::<VaultCredentialStorage>(
                                        &decrypted_data,
                                    ) {
                                        Ok(storage) => storage,
                                        Err(e) => {
                                            eprintln!("Error parsing vault credentials file: {e}");
                                            VaultCredentialStorage::default()
                                        }
                                    }
                                }
                                Err(e) => {
                                    // Delete the corrupted file since vault token has likely changed
                                    if let Err(remove_err) = fs::remove_file(&path) {
                                        eprintln!("Error decrypting vault credentials file: {e}");
                                        eprintln!(
                                            "Failed to remove corrupted vault credentials file: {remove_err}"
                                        );
                                    } else {
                                        debug!(
                                            "Removed corrupted vault credentials file due to decryption failure: {e}"
                                        );
                                    }
                                    VaultCredentialStorage::default()
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading vault credentials file: {e}");
                            VaultCredentialStorage::default()
                        }
                    }
                } else {
                    VaultCredentialStorage::default()
                }
            }
            Err(e) => {
                eprintln!("Error getting vault credentials path: {e}");
                VaultCredentialStorage::default()
            }
        }
    }

    /// Save vault credentials to encrypted file
    fn save_vault_credentials(&self) -> Result<(), Box<dyn Error>> {
        let path = Self::get_vault_credentials_path()?;

        // Ensure the parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Serialize to TOML
        let content = toml::to_string_pretty(&self.vault_credential_storage)?;

        // Encrypt the content
        let encrypted_data = crate::vault_encryption::encrypt_data(content.as_bytes())
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;

        // Write encrypted data to file
        fs::write(&path, encrypted_data)?;

        // Set restrictive file permissions (600 - owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }

        Ok(())
    }

    /// Migrate recent connections from main config file to separate file
    /// This reads the old config format and extracts recent_connections
    fn migrate_recent_connections_if_needed() -> Vec<RecentConnection> {
        if let Some(config_path) = get_config_path() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                // Check if the config file contains recent_connections
                if content.contains("[[recent_connections]]") {
                    // Try to parse it as a TOML value to extract just the recent connections
                    if let Ok(toml_value) = toml::from_str::<toml::Value>(&content) {
                        if let Some(table) = toml_value.as_table() {
                            if let Some(recent_array) = table.get("recent_connections") {
                                if let Some(connections) = recent_array.as_array() {
                                    let mut migrated_connections = Vec::new();
                                    for conn_value in connections {
                                        if let Ok(connection) =
                                            conn_value.clone().try_into::<RecentConnection>()
                                        {
                                            migrated_connections.push(connection);
                                        }
                                    }
                                    if !migrated_connections.is_empty() {
                                        println!(
                                            "📦 Migrating {} recent connections to separate file",
                                            migrated_connections.len()
                                        );
                                        return migrated_connections;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Vec::new()
    }

    /// Migrate saved sessions from main config file to separate file
    /// This reads the old config format and extracts saved_sessions
    fn migrate_saved_sessions_if_needed() -> HashMap<String, SavedSession> {
        if let Some(config_path) = get_config_path() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                // Check if the config file contains saved_sessions
                if content.contains("[saved_sessions") || content.contains("saved_sessions") {
                    // Try to parse it as a TOML value to extract just the saved sessions
                    if let Ok(toml_value) = toml::from_str::<toml::Value>(&content) {
                        if let Some(table) = toml_value.as_table() {
                            if let Some(sessions_table) = table.get("saved_sessions") {
                                if let Some(sessions) = sessions_table.as_table() {
                                    let mut migrated_sessions = HashMap::new();
                                    for (name, session_value) in sessions {
                                        if let Ok(session) =
                                            session_value.clone().try_into::<SavedSession>()
                                        {
                                            migrated_sessions.insert(name.clone(), session);
                                        }
                                    }
                                    if !migrated_sessions.is_empty() {
                                        println!(
                                            "📦 Migrating {} saved sessions to separate file",
                                            migrated_sessions.len()
                                        );
                                        return migrated_sessions;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        HashMap::new()
    }

    pub fn load() -> Self {
        // Check if another config load is already in progress
        if CONFIG_LOADING_IN_PROGRESS
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Another config load is in progress, return default config silently
            let mut config = Config::default();
            config.recent_connections_storage = Self::load_recent_connections();
            config.saved_sessions_storage = Self::load_saved_sessions();
            config.vault_credential_storage = Self::load_vault_credentials();

            // Apply global verbosity override if set
            if let Some(override_level) = get_global_verbosity_override() {
                config.verbosity_level = override_level;
            }

            return config;
        }

        // We now own the lock, proceed with normal loading
        // Use a guard pattern to ensure the lock is released even if there's a panic
        struct LockGuard;
        impl Drop for LockGuard {
            fn drop(&mut self) {
                CONFIG_LOADING_IN_PROGRESS.store(false, Ordering::Release);
            }
        }
        let _guard = LockGuard;

        Self::load_with_retry_count(0)
    }

    fn load_with_retry_count(retry_count: u32) -> Self {
        const MAX_RETRIES: u32 = 3;

        if retry_count >= MAX_RETRIES {
            eprintln!(
                "Maximum config load retries ({MAX_RETRIES}) exceeded. Using default configuration."
            );
            eprintln!(
                "Please check your config file manually or delete it to regenerate: ~/.config/dbcrust/config.toml"
            );
            let mut config = Config::default();
            config.recent_connections_storage = Self::load_recent_connections();
            config.saved_sessions_storage = Self::load_saved_sessions();
            config.vault_credential_storage = Self::load_vault_credentials();

            // Apply global verbosity override if set
            if let Some(override_level) = get_global_verbosity_override() {
                config.verbosity_level = override_level;
            }

            return config;
        }

        if retry_count > 0 {
            eprintln!("Config load retry attempt {retry_count}/{MAX_RETRIES}");
        }

        if let Some(config_path) = get_config_path() {
            match fs::read_to_string(&config_path) {
                Ok(content) => {
                    let config_result: Result<Config, toml::de::Error> = toml::from_str(&content);

                    // Debug: If parsing fails, show detailed information about the error
                    if let Err(ref e) = config_result {
                        eprintln!("TOML parsing error details:");
                        eprintln!("  Error: {}", e);

                        // Check if it's specifically about autocomplete_enabled
                        if e.to_string().contains("autocomplete_enabled") {
                            eprintln!("  This is an autocomplete_enabled field type issue");

                            // Show the line content for debugging
                            if let Some(line_num) = e
                                .to_string()
                                .lines()
                                .next()
                                .and_then(|l| l.split("line ").nth(1))
                                .and_then(|l| l.split(',').next())
                                .and_then(|l| l.parse::<usize>().ok())
                            {
                                if let Some(line) = content.lines().nth(line_num - 1) {
                                    eprintln!("  Problem line {}: {}", line_num, line.trim());
                                }
                            }
                        }
                    }

                    match config_result {
                        Ok(mut config) => {
                            // Load recent connections, saved sessions, and vault credentials from separate files
                            config.recent_connections_storage = Self::load_recent_connections();
                            config.saved_sessions_storage = Self::load_saved_sessions();
                            config.vault_credential_storage = Self::load_vault_credentials();

                            // Apply global verbosity override if set
                            if let Some(override_level) = get_global_verbosity_override() {
                                config.verbosity_level = override_level;
                            }

                            // Check if config file is missing any fields and upgrade if needed
                            if !config.has_all_fields(&content) {
                                if let Err(e) = config.save_with_documentation() {
                                    // Silent fallback - don't spam user with config save errors
                                    eprintln!(
                                        "Warning: Could not update config file with missing fields: {e}"
                                    );
                                    eprintln!("Continuing with existing configuration...");
                                }
                            }

                            config
                        }
                        Err(e) => {
                            if retry_count == 0 {
                                eprintln!(
                                    "Error parsing config file ({e}), backing up and creating fresh config"
                                );

                                // Backup corrupted file and extract SSH tunnel patterns safely
                                let backup_path = config_path.with_extension("toml.backup");
                                if let Err(backup_err) = std::fs::copy(&config_path, &backup_path) {
                                    eprintln!(
                                        "Warning: Could not backup corrupted config: {backup_err}"
                                    );
                                } else {
                                    println!(
                                        "Backed up corrupted config to: {}",
                                        backup_path.display()
                                    );
                                }

                                // Extract SSH tunnel patterns using regex (safer than TOML parsing)
                                let mut config = Config::default();

                                // Use regex to safely extract SSH tunnel patterns from corrupted file
                                let ssh_section_regex =
                                    regex::Regex::new(r"(?m)^\[ssh_tunnel_patterns\]$").unwrap();
                                let pattern_regex = regex::Regex::new(
                                    r#"(?m)^"([^"]+(?:\\.[^"]*)*)" = "([^"]+(?:\\.[^"]*)*)"$"#,
                                )
                                .unwrap();

                                if let Some(ssh_match) = ssh_section_regex.find(&content) {
                                    let ssh_section_start = ssh_match.end();

                                    // Find the next section or end of file
                                    let next_section_regex =
                                        regex::Regex::new(r"(?m)^\[[^\]]+\]$").unwrap();
                                    let ssh_section_end = next_section_regex
                                        .find_at(&content, ssh_section_start)
                                        .map(|m| m.start())
                                        .unwrap_or(content.len());

                                    let ssh_section = &content[ssh_section_start..ssh_section_end];

                                    // Extract only valid SSH tunnel patterns (skip corrupted config fields)
                                    for cap in pattern_regex.captures_iter(ssh_section) {
                                        let pattern = &cap[1];
                                        let tunnel_config = &cap[2];

                                        // Only accept patterns that look like SSH tunnel patterns
                                        // Valid patterns contain dots, backslashes, or tunnel configs contain '@'
                                        if (pattern.contains('.') || pattern.contains("\\\\"))
                                            && tunnel_config.contains('@')
                                        {
                                            // Unescape the pattern for internal storage
                                            let unescaped_pattern = pattern.replace("\\\\", "\\");
                                            config.ssh_tunnel_patterns.insert(
                                                unescaped_pattern,
                                                tunnel_config.to_string(),
                                            );
                                            println!(
                                                "Preserved SSH tunnel pattern: {} -> {}",
                                                pattern, tunnel_config
                                            );
                                        } else {
                                            eprintln!(
                                                "Skipping invalid SSH tunnel pattern: {} -> {}",
                                                pattern, tunnel_config
                                            );
                                        }
                                    }
                                }

                                // Create fresh config with preserved SSH tunnel patterns
                                if let Err(save_err) = config.save_with_documentation() {
                                    eprintln!("Unable to create fresh config file: {save_err}");
                                    eprintln!("Using default configuration in memory...");

                                    // If save fails, return default config to prevent infinite loop
                                    config.recent_connections_storage =
                                        Self::load_recent_connections();
                                    config.saved_sessions_storage = Self::load_saved_sessions();
                                    config.vault_credential_storage =
                                        Self::load_vault_credentials();

                                    if let Some(override_level) = get_global_verbosity_override() {
                                        config.verbosity_level = override_level;
                                    }

                                    return config;
                                }

                                // Load separate storage files
                                config.recent_connections_storage = Self::load_recent_connections();
                                config.saved_sessions_storage = Self::load_saved_sessions();
                                config.vault_credential_storage = Self::load_vault_credentials();

                                // Apply global verbosity override if set
                                if let Some(override_level) = get_global_verbosity_override() {
                                    config.verbosity_level = override_level;
                                }

                                // Retry loading the freshly created config
                                eprintln!("Retrying config load after creating fresh config...");
                                return Self::load_with_retry_count(retry_count + 1);
                            } else {
                                // On subsequent retries, show detailed error and fail
                                eprintln!(
                                    "Config parsing failed again on retry {retry_count}: {e}"
                                );
                                eprintln!(
                                    "This suggests a persistent issue with the config structure."
                                );

                                // Return default config to prevent further retries
                                let mut config = Config::default();
                                config.recent_connections_storage = Self::load_recent_connections();
                                config.saved_sessions_storage = Self::load_saved_sessions();
                                config.vault_credential_storage = Self::load_vault_credentials();

                                if let Some(override_level) = get_global_verbosity_override() {
                                    config.verbosity_level = override_level;
                                }

                                config
                            }
                        }
                    }
                }
                Err(_) => {
                    // Config file doesn't exist, create it with comprehensive documentation
                    let mut config = Config::default();
                    config.recent_connections_storage = Self::load_recent_connections();
                    config.saved_sessions_storage = Self::load_saved_sessions();
                    config.vault_credential_storage = Self::load_vault_credentials();

                    // Apply global verbosity override if set
                    if let Some(override_level) = get_global_verbosity_override() {
                        config.verbosity_level = override_level;
                    }

                    // Create the config file with comprehensive documentation
                    if let Err(e) = config.save_with_documentation() {
                        eprintln!("Warning: Could not create config file: {e}");
                        eprintln!("Continuing with default configuration...");
                    }

                    config
                }
            }
        } else {
            // No config path available, return default config without creating file
            let mut config = Config::default();
            config.recent_connections_storage = Self::load_recent_connections();
            config.saved_sessions_storage = Self::load_saved_sessions();
            config.vault_credential_storage = Self::load_vault_credentials();

            // Apply global verbosity override if set
            if let Some(override_level) = get_global_verbosity_override() {
                config.verbosity_level = override_level;
            }

            config
        }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(config_path) = get_config_path() {
            ensure_config_dir(&config_path)?;

            let toml = toml::to_string(self)
                .map_err(|e| io::Error::other(format!("Serialization error: {e}")))?;

            let mut file = File::create(&config_path)?;
            file.write_all(toml.as_bytes())?;
        }
        Ok(())
    }

    /// Save config with comprehensive documentation for all options
    pub fn save_with_documentation(&self) -> io::Result<()> {
        // Check if another config save is already in progress
        if CONFIG_SAVING_IN_PROGRESS
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Another config save is in progress, return error to prevent recursion
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Config save already in progress - preventing recursion",
            ));
        }

        // Use a guard pattern to ensure the lock is released even if there's a panic
        struct SaveGuard;
        impl Drop for SaveGuard {
            fn drop(&mut self) {
                CONFIG_SAVING_IN_PROGRESS.store(false, Ordering::Release);
            }
        }
        let _guard = SaveGuard;

        if let Some(config_path) = get_config_path() {
            ensure_config_dir(&config_path)?;

            let mut content = String::new();

            // Header
            content.push_str("# DBCrust Configuration File\n");
            content.push_str("# This file contains all available configuration options with their current values.\n");
            content.push_str("# Lines starting with '#' are comments. Uncomment and modify values as needed.\n\n");

            // ROOT LEVEL FIELDS FIRST (to avoid TOML structure issues)
            // ================================================================================

            // Display Settings
            content.push_str("# ================================================================================\n");
            content.push_str("# DISPLAY SETTINGS\n");
            content.push_str("# Control how query results and interface elements are displayed\n");
            content.push_str("# ================================================================================\n\n");

            content.push_str(&format!(
                "# Default row limit for query results (default: 100)\n"
            ));
            content.push_str(&format!("default_limit = {}\n\n", self.default_limit));

            content.push_str(&format!(
                "# Use expanded display mode by default (default: false)\n"
            ));
            content.push_str(&format!(
                "expanded_display_default = {}\n\n",
                self.expanded_display_default
            ));

            content.push_str(&format!("# Show banner on startup (default: false)\n"));
            content.push_str(&format!("show_banner = {}\n\n", self.show_banner));

            content.push_str(&format!(
                "# Verbosity level: \"Quiet\", \"Normal\", or \"Verbose\" (default: Normal)\n"
            ));
            content.push_str(&format!(
                "verbosity_level = \"{:?}\"\n\n",
                self.verbosity_level
            ));

            content.push_str(&format!(
                "# Indicator for multiline prompts (default: empty)\n"
            ));
            content.push_str(&format!(
                "multiline_prompt_indicator = \"{}\"\n\n",
                self.multiline_prompt_indicator
            ));

            content.push_str(&format!(
                "# Number of columns to trigger interactive column selection (default: 10)\n"
            ));
            content.push_str(&format!(
                "# Set to 0 to disable automatic column selection\n"
            ));
            content.push_str(&format!(
                "column_selection_threshold = {}\n\n",
                self.column_selection_threshold
            ));

            content.push_str(&format!("# Default all columns selected in column selection (true = opt-out, false = opt-in) (default: false)\n"));
            content.push_str(&format!(
                "column_selection_default_all = {}\n\n",
                self.column_selection_default_all
            ));

            // Pager Settings
            content.push_str("# ================================================================================\n");
            content.push_str("# PAGER SETTINGS\n");
            content.push_str("# Configure output paging for large result sets\n");
            content.push_str("# ================================================================================\n\n");

            content.push_str(&format!(
                "# Enable pager for large outputs (default: true)\n"
            ));
            content.push_str(&format!("pager_enabled = {}\n\n", self.pager_enabled));

            content.push_str(&format!("# Pager command (default: \"less -R\")\n"));
            content.push_str(&format!("pager_command = \"{}\"\n\n", self.pager_command));

            content.push_str(&format!(
                "# Lines before triggering pager, 0 = terminal height (default: 0)\n"
            ));
            content.push_str(&format!(
                "pager_threshold_lines = {}\n\n",
                self.pager_threshold_lines
            ));

            // Features
            content.push_str("# ================================================================================\n");
            content.push_str("# FEATURES\n");
            content.push_str("# Enable or disable various features\n");
            content.push_str("# ================================================================================\n\n");

            content.push_str(&format!("# Enable SQL autocomplete (default: true)\n"));
            content.push_str(&format!(
                "autocomplete_enabled = {}\n\n",
                self.autocomplete_enabled
            ));

            content.push_str(&format!(
                "# Enable EXPLAIN mode by default (default: false)\n"
            ));
            content.push_str(&format!(
                "explain_mode_default = {}\n\n",
                self.explain_mode_default
            ));

            content.push_str(&format!(
                "# Maximum number of recent connections to remember (default: 10)\n"
            ));
            content.push_str(&format!(
                "max_recent_connections = {}\n\n",
                self.max_recent_connections
            ));

            // Query Timeouts
            content.push_str("# ================================================================================\n");
            content.push_str("# TIMEOUT SETTINGS\n");
            content.push_str("# Configure query and metadata operation timeouts\n");
            content.push_str("# ================================================================================\n\n");

            content.push_str(&format!(
                "# Query execution timeout in seconds (default: 30)\n"
            ));
            content.push_str(&format!(
                "query_timeout_seconds = {}\n\n",
                self.query_timeout_seconds
            ));

            content.push_str(&format!(
                "# Metadata query timeout in seconds (default: 10)\n"
            ));
            content.push_str(&format!(
                "metadata_timeout_seconds = {}\n\n",
                self.metadata_timeout_seconds
            ));

            // Vault Settings
            content.push_str("# ================================================================================\n");
            content.push_str("# VAULT INTEGRATION\n");
            content.push_str("# HashiCorp Vault credential caching settings\n");
            content.push_str("# ================================================================================\n\n");

            content.push_str(&format!(
                "# Enable Vault credential caching (default: true)\n"
            ));
            content.push_str(&format!(
                "vault_credential_cache_enabled = {}\n\n",
                self.vault_credential_cache_enabled
            ));

            content.push_str(&format!(
                "# Renew credentials when this fraction of TTL remains (default: 0.25 = 25%)\n"
            ));
            content.push_str(&format!(
                "vault_cache_renewal_threshold = {}\n\n",
                self.vault_cache_renewal_threshold
            ));

            content.push_str(&format!(
                "# Don't cache credentials with TTL less than this (seconds, default: 300)\n"
            ));
            content.push_str(&format!(
                "vault_cache_min_ttl_seconds = {}\n\n",
                self.vault_cache_min_ttl_seconds
            ));

            // NOW ADD TABLE SECTIONS AFTER ALL ROOT-LEVEL FIELDS
            // ================================================================================

            // SSH Tunnel Patterns
            content.push_str("# ================================================================================\n");
            content.push_str("# SSH TUNNEL PATTERNS\n");
            content
                .push_str("# Automatically open SSH tunnels for hosts matching these patterns\n");
            content.push_str("# Format: \"regex_pattern\" = \"user@ssh_host:port\"\n");
            content.push_str("# Supports command substitution with backticks: \"pattern\" = \"user@`command`\"\n");
            content.push_str("# ================================================================================\n\n");
            content.push_str("[ssh_tunnel_patterns]\n");
            if self.ssh_tunnel_patterns.is_empty() {
                content.push_str("# Example patterns:\n");
                content
                    .push_str("# \".*\\.rds\\.amazonaws\\.com\" = \"user@bastion.example.com\"\n");
                content.push_str("# \"prod-.*\\.internal\" = \"user@`get-bastion-ip.sh`\"\n");
            } else {
                for (pattern, tunnel_config) in &self.ssh_tunnel_patterns {
                    // Properly escape strings for TOML format
                    let escaped_pattern = pattern.replace('\\', "\\\\").replace('"', "\\\"");
                    let escaped_config = tunnel_config.replace('\\', "\\\\").replace('"', "\\\"");
                    content.push_str(&format!(
                        "\"{}\" = \"{}\"\n",
                        escaped_pattern, escaped_config
                    ));
                }
            }
            content.push_str("\n");

            // Logging Configuration
            content.push_str("# ================================================================================\n");
            content.push_str("# LOGGING CONFIGURATION\n");
            content.push_str("# Configure application logging for debugging\n");
            content.push_str("# ================================================================================\n\n");
            content.push_str("[logging]\n");
            content.push_str(&format!(
                "# Log level: \"trace\", \"debug\", \"info\", \"warn\", \"error\" (default: info)\n"
            ));
            content.push_str(&format!("level = \"{}\"\n\n", self.logging.level));
            content.push_str(&format!("# Output logs to console (default: true)\n"));
            content.push_str(&format!(
                "console_output = {}\n\n",
                self.logging.console_output
            ));
            content.push_str(&format!("# Output logs to file (default: false)\n"));
            content.push_str(&format!("file_output = {}\n\n", self.logging.file_output));
            content.push_str(&format!(
                "# Log file path (default: ~/.config/dbcrust/logs/dbcrust.log)\n"
            ));
            content.push_str(&format!("file_path = \"{}\"\n\n", self.logging.file_path));
            content.push_str(&format!(
                "# Maximum log file size in MB before rotation (default: 10)\n"
            ));
            content.push_str(&format!(
                "max_file_size_mb = {}\n\n",
                self.logging.max_file_size_mb
            ));
            content.push_str(&format!(
                "# Number of rotated log files to keep (default: 5)\n"
            ));
            content.push_str(&format!("max_files = {}\n\n", self.logging.max_files));

            // History Configuration
            content.push_str("# ================================================================================\n");
            content.push_str("# HISTORY CONFIGURATION\n");
            content.push_str("# Configure per-session command history management\n");
            content.push_str("# ================================================================================\n\n");
            content.push_str("[history]\n");
            content.push_str(&format!(
                "# Enable per-session history (separate history per connection) (default: true)\n"
            ));
            content.push_str(&format!(
                "per_session_enabled = {}\n\n",
                self.history.per_session_enabled
            ));
            content.push_str(&format!(
                "# Maximum number of history files to keep (default: 50)\n"
            ));
            content.push_str(&format!(
                "max_history_files = {}\n\n",
                self.history.max_history_files
            ));
            content.push_str(&format!(
                "# Clean up old unused history files after N days (default: 90)\n"
            ));
            content.push_str(&format!(
                "cleanup_after_days = {}\n\n",
                self.history.cleanup_after_days
            ));

            // Named Queries
            if !self.named_queries.is_empty() {
                content.push_str("# ================================================================================\n");
                content.push_str("# NAMED QUERIES\n");
                content.push_str("# Save frequently used queries with memorable names\n");
                content.push_str("# Use \\nq <name> to execute, \\nq to list all\n");
                content.push_str("# ================================================================================\n\n");
                content.push_str("[named_queries]\n");
                for (name, query) in &self.named_queries {
                    content.push_str(&format!("{} = \"{}\"\n", name, query.replace('"', "\\\"")));
                }
                content.push_str("\n");
            }

            let mut file = File::create(&config_path)?;
            file.write_all(content.as_bytes())?;
        }
        Ok(())
    }

    /// Check if the config file contains all expected fields
    /// This helps detect when a config needs to be upgraded with missing options
    fn has_all_fields(&self, file_content: &str) -> bool {
        // List of key fields/sections that should exist in a complete config
        let required_fields = [
            "default_limit",
            "expanded_display_default",
            "autocomplete_enabled",
            "explain_mode_default",
            "column_selection_threshold",
            "pager_enabled",
            "pager_command",
            "pager_threshold_lines",
            "show_banner",
            "verbosity_level",
            "multiline_prompt_indicator",
            "vault_credential_cache_enabled",
            "vault_cache_renewal_threshold",
            "vault_cache_min_ttl_seconds",
            "query_timeout_seconds",
            "metadata_timeout_seconds",
            "max_recent_connections",
            "[logging]",
            "[history]",
            "per_session_enabled",
            "max_history_files",
            "cleanup_after_days",
            "[connection]",
        ];

        // Check if all required fields are present in the file content
        required_fields
            .iter()
            .all(|field| file_content.contains(field))
    }

    pub fn add_named_query(&mut self, name: &str, query: &str) -> Result<(), Box<dyn Error>> {
        self.named_queries
            .insert(name.to_string(), query.to_string());
        self.save()?;
        Ok(())
    }

    pub fn delete_named_query(&mut self, name: &str) -> Result<bool, Box<dyn Error>> {
        let existed = self.named_queries.remove(name).is_some();
        if existed {
            self.save()?;
        }
        Ok(existed)
    }

    pub fn get_named_query(&self, name: &str) -> Option<&String> {
        self.named_queries.get(name)
    }

    pub fn list_named_queries(&self) -> Vec<(String, String)> {
        self.named_queries
            .iter()
            .map(|(name, query)| (name.clone(), query.clone()))
            .collect()
    }

    // Session management methods

    /// Save session from actual connection info (for Docker and other resolved connections)
    pub fn save_session_from_connection_info(
        &mut self,
        name: &str,
        connection_info: &crate::database::ConnectionInfo,
    ) -> Result<(), Box<dyn Error>> {
        // For Docker connections, we want to save a special marker that can be re-resolved
        let (host, port, user, dbname) = if connection_info.is_docker_connection() {
            // For Docker connections, save a special format that can be re-resolved
            (
                format!(
                    "DOCKER:{}",
                    connection_info
                        .docker_container
                        .as_ref()
                        .unwrap_or(&"unknown".to_string())
                ),
                0, // Port 0 indicates Docker connection
                connection_info
                    .username
                    .as_ref()
                    .unwrap_or(&"".to_string())
                    .clone(),
                connection_info
                    .database
                    .as_ref()
                    .unwrap_or(&"".to_string())
                    .clone(),
            )
        } else {
            // For regular connections, use the actual connection details
            (
                connection_info
                    .host
                    .as_ref()
                    .unwrap_or(&"localhost".to_string())
                    .clone(),
                connection_info.port.unwrap_or(5432),
                connection_info
                    .username
                    .as_ref()
                    .unwrap_or(&"".to_string())
                    .clone(),
                connection_info
                    .database
                    .as_ref()
                    .unwrap_or(&"".to_string())
                    .clone(),
            )
        };

        // Normalize SQLite file paths to absolute paths
        let normalized_file_path = if connection_info.database_type.is_file_based() {
            match &connection_info.file_path {
                Some(path) => Some(Self::normalize_sqlite_path(path)?),
                None => None,
            }
        } else {
            connection_info.file_path.clone()
        };

        let session = SavedSession {
            host,
            port,
            user,
            dbname,
            ssh_tunnel: None, // SSH tunnel info not available in ConnectionInfo
            database_type: connection_info.database_type.clone(),
            file_path: normalized_file_path,
            options: connection_info.options.clone(),
        };

        self.saved_sessions_storage
            .sessions
            .insert(name.to_string(), session);
        self.save_saved_sessions()?;
        Ok(())
    }

    pub fn delete_session(&mut self, name: &str) -> Result<bool, Box<dyn Error>> {
        let existed = self.saved_sessions_storage.sessions.remove(name).is_some();
        if existed {
            self.save_saved_sessions()?;
        }
        Ok(existed)
    }

    pub fn get_session(&self, name: &str) -> Option<&SavedSession> {
        self.saved_sessions_storage.sessions.get(name)
    }

    pub fn list_sessions(&self) -> Vec<(String, SavedSession)> {
        self.saved_sessions_storage
            .sessions
            .iter()
            .map(|(name, session)| (name.clone(), session.clone()))
            .collect()
    }

    pub fn parse_ssh_tunnel_string(&self, ssh_tunnel_str: &str) -> Option<SSHTunnelConfig> {
        // Format: [user[:password]@]ssh_host[:ssh_port]
        let mut ssh_config = SSHTunnelConfig {
            enabled: true,
            ..Default::default()
        };

        // Check if string is empty
        if ssh_tunnel_str.trim().is_empty() {
            return None;
        }

        // Parse the string
        if ssh_tunnel_str.contains('@') {
            let parts: Vec<&str> = ssh_tunnel_str.split('@').collect();
            if parts.len() == 2 {
                // Parse user[:password] part
                let credentials = parts[0];
                if credentials.contains(':') {
                    let cred_parts: Vec<&str> = credentials.split(':').collect();
                    ssh_config.ssh_username = Some(cred_parts[0].to_string());
                    ssh_config.ssh_password = Some(cred_parts[1].to_string());
                } else {
                    ssh_config.ssh_username = Some(credentials.to_string());
                }

                // Parse host[:port] part
                let host_port = parts[1];
                if host_port.contains(':') {
                    let hp_parts: Vec<&str> = host_port.split(':').collect();
                    ssh_config.ssh_host = hp_parts[0].to_string();
                    if let Ok(port) = hp_parts[1].parse::<u16>() {
                        ssh_config.ssh_port = port;
                    }
                } else {
                    ssh_config.ssh_host = host_port.to_string();
                }
            }
        } else {
            // Only hostname[:port] provided
            if ssh_tunnel_str.contains(':') {
                let parts: Vec<&str> = ssh_tunnel_str.split(':').collect();
                ssh_config.ssh_host = parts[0].to_string();
                if let Ok(port) = parts[1].parse::<u16>() {
                    ssh_config.ssh_port = port;
                }
            } else {
                ssh_config.ssh_host = ssh_tunnel_str.to_string();
            }
        }

        Some(ssh_config)
    }

    fn resolve_command_in_tunnel_config(
        &self,
        tunnel_config: &str,
    ) -> Result<String, Box<dyn Error>> {
        use std::process::Command;

        let mut result = tunnel_config.to_string();

        // Find all command patterns between backticks
        let mut start = 0;
        while let Some(start_pos) = result[start..].find('`') {
            let absolute_start = start + start_pos;
            if let Some(end_pos) = result[absolute_start + 1..].find('`') {
                let absolute_end = absolute_start + 1 + end_pos;

                // Extract the command between backticks
                let command_str = &result[absolute_start + 1..absolute_end];

                // Execute the command
                let output = if cfg!(target_os = "windows") {
                    Command::new("cmd")
                        .args(["/C", command_str])
                        .output()
                        .map_err(|e| format!("Failed to execute command '{command_str}': {e}"))?
                } else {
                    Command::new("sh")
                        .args(["-c", command_str])
                        .output()
                        .map_err(|e| format!("Failed to execute command '{command_str}': {e}"))?
                };

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!(
                        "Command '{}' failed with exit code {}: {}",
                        command_str,
                        output.status.code().unwrap_or(-1),
                        stderr
                    )
                    .into());
                }

                // Get the command output and trim whitespace
                let command_output = String::from_utf8_lossy(&output.stdout).trim().to_string();

                // Replace the command pattern with the output
                result.replace_range(absolute_start..=absolute_end, &command_output);

                // Continue searching from the end of the replacement
                start = absolute_start + command_output.len();
            } else {
                // No matching closing backtick found
                return Err("Unmatched backtick in SSH tunnel pattern".into());
            }
        }

        Ok(result)
    }

    pub fn get_ssh_tunnel_for_host(&self, host: &str) -> Option<SSHTunnelConfig> {
        for (pattern, tunnel_config) in &self.ssh_tunnel_patterns {
            if let Ok(regex) = Regex::new(pattern) {
                if regex.is_match(host) {
                    // Check if tunnel_config contains command syntax with backticks
                    let resolved_config = if tunnel_config.contains('`') {
                        match self.resolve_command_in_tunnel_config(tunnel_config) {
                            Ok(resolved) => resolved,
                            Err(e) => {
                                eprintln!("Error executing command in SSH tunnel pattern: {e}");
                                return None;
                            }
                        }
                    } else {
                        tunnel_config.clone()
                    };
                    return self.parse_ssh_tunnel_string(&resolved_config);
                }
            }
        }
        None
    }

    // Helper function to convert SQLite file paths to absolute paths for storage
    // This function expects relative paths (from sqlite:/// URLs) and converts them to absolute
    fn normalize_sqlite_path(file_path: &str) -> Result<String, Box<dyn Error>> {
        let path = Path::new(file_path);

        // Make the relative path absolute based on current directory
        let current_dir = std::env::current_dir()?;
        let absolute_path = current_dir.join(path);

        // Try to canonicalize, fall back to the absolute path if that fails (e.g., file doesn't exist)
        match absolute_path.canonicalize() {
            Ok(canonical) => Ok(canonical.to_string_lossy().to_string()),
            Err(_) => Ok(absolute_path.to_string_lossy().to_string()),
        }
    }

    // Helper function to generate display name from connection URL
    fn generate_display_name_from_url(url: &str, _database_type: &DatabaseType) -> String {
        // Extract meaningful parts from the URL for display
        if url.starts_with("sqlite://") {
            // For SQLite, show just the file path without the scheme
            let path = url.strip_prefix("sqlite://").unwrap_or(url);
            // Handle absolute paths: sqlite:////Users/... becomes //Users/..., strip one slash
            if path.starts_with("//") {
                path[1..].to_string()
            } else {
                path.to_string()
            }
        } else if url.starts_with("session://") {
            // For session URLs, show the session name
            url.strip_prefix("session://").unwrap_or(url).to_string()
        } else if url.starts_with("vault://") {
            // For vault URLs, show the vault path
            url.to_string()
        } else {
            // For standard database URLs (including Docker-resolved ones), extract user@host:port/database
            if let Some(scheme_end) = url.find("://") {
                let after_scheme = &url[scheme_end + 3..];

                // Check for Docker suffix and extract main URL part
                let main_part = if let Some(docker_pos) = after_scheme.find(" # Docker: ") {
                    &after_scheme[..docker_pos]
                } else {
                    after_scheme
                };

                if let Some(at_pos) = main_part.find('@') {
                    let user = &main_part[..at_pos];
                    let after_user = &main_part[at_pos + 1..];

                    if let Some(slash_pos) = after_user.find('/') {
                        let host_port = &after_user[..slash_pos];
                        let database = &after_user[slash_pos + 1..];
                        // Remove query parameters
                        let database = database.split('?').next().unwrap_or(database);

                        // Include Docker container info if present
                        if let Some(docker_pos) = url.find(" # Docker: ") {
                            let container = &url[docker_pos + 11..]; // Skip " # Docker: "
                            format!("{user}@{host_port}/{database} (Docker: {container})")
                        } else {
                            format!("{user}@{host_port}/{database}")
                        }
                    } else {
                        // No database in URL, just user@host:port
                        if let Some(docker_pos) = url.find(" # Docker: ") {
                            let container = &url[docker_pos + 11..];
                            format!("{user}@{after_user} (Docker: {container})")
                        } else {
                            format!("{user}@{after_user}")
                        }
                    }
                } else {
                    // No user in URL, show host:port/database or just the main part
                    if let Some(docker_pos) = url.find(" # Docker: ") {
                        let container = &url[docker_pos + 11..];
                        format!("{main_part} (Docker: {container})")
                    } else {
                        main_part.to_string()
                    }
                }
            } else {
                // No scheme found, return as-is
                url.to_string()
            }
        }
    }

    // Recent connection history methods
    pub fn add_recent_connection(
        &mut self,
        connection_url: String,
        display_name: String,
        database_type: DatabaseType,
        success: bool,
    ) -> Result<(), Box<dyn Error>> {
        // Normalize SQLite URLs to use absolute paths
        let normalized_url = if database_type.is_file_based()
            && connection_url.starts_with("sqlite:///")
            && !connection_url.starts_with("sqlite:////")
        {
            // sqlite:///path (3 slashes) = relative path that needs normalization
            let relative_path = connection_url.strip_prefix("sqlite:///").unwrap_or("");
            let normalized_path = Self::normalize_sqlite_path(relative_path)?;
            format!("sqlite:///{}", normalized_path)
        } else {
            // sqlite:////path (4 slashes) = absolute path, keep as is
            // or non-SQLite URLs, keep as is
            connection_url
        };

        let connection = RecentConnection {
            connection_url: normalized_url,
            display_name,
            timestamp: Utc::now(),
            database_type,
            success,
            options: HashMap::new(),
        };

        // Add to the beginning of the list (most recent first)
        self.recent_connections_storage
            .connections
            .insert(0, connection);

        // Keep only the configured number of recent connections
        if self.recent_connections_storage.connections.len() > self.max_recent_connections {
            self.recent_connections_storage
                .connections
                .truncate(self.max_recent_connections);
        }

        // Save recent connections to separate file
        self.save_recent_connections()?;
        Ok(())
    }

    // Convenience method that auto-generates display name from URL
    pub fn add_recent_connection_auto_display(
        &mut self,
        connection_url: String,
        database_type: DatabaseType,
        success: bool,
    ) -> Result<(), Box<dyn Error>> {
        // Normalize SQLite URLs to use absolute paths
        let normalized_url = if database_type.is_file_based()
            && connection_url.starts_with("sqlite:///")
            && !connection_url.starts_with("sqlite:////")
        {
            // sqlite:///path (3 slashes) = relative path that needs normalization
            let relative_path = connection_url.strip_prefix("sqlite:///").unwrap_or("");
            let normalized_path = Self::normalize_sqlite_path(relative_path)?;
            format!("sqlite:///{}", normalized_path)
        } else {
            // sqlite:////path (4 slashes) = absolute path, keep as is
            // or non-SQLite URLs, keep as is
            connection_url
        };

        let display_name = Self::generate_display_name_from_url(&normalized_url, &database_type);
        self.add_recent_connection(normalized_url, display_name, database_type, success)
    }

    /// Add a recent connection with vault metadata (for vault connections)
    pub fn add_recent_connection_with_options(
        &mut self,
        connection_url: String,
        database_type: DatabaseType,
        success: bool,
        options: HashMap<String, String>,
    ) -> Result<(), Box<dyn Error>> {
        let display_name = Self::generate_display_name_from_url(&connection_url, &database_type);

        let connection = RecentConnection {
            connection_url,
            display_name,
            timestamp: Utc::now(),
            database_type,
            success,
            options,
        };

        // Add to the beginning of the list (most recent first)
        self.recent_connections_storage
            .connections
            .insert(0, connection);

        // Keep only the configured number of recent connections
        if self.recent_connections_storage.connections.len() > self.max_recent_connections {
            self.recent_connections_storage
                .connections
                .truncate(self.max_recent_connections);
        }

        self.save_recent_connections()?;
        Ok(())
    }

    pub fn get_recent_connections(&self) -> &Vec<RecentConnection> {
        &self.recent_connections_storage.connections
    }

    pub fn clear_recent_connections(&mut self) -> Result<(), Box<dyn Error>> {
        self.recent_connections_storage.connections.clear();
        self.save_recent_connections()?;
        Ok(())
    }

    // Vault credential caching methods

    /// Generate cache key for vault credentials
    pub fn vault_cache_key(mount_path: &str, database_name: &str, role_name: &str) -> String {
        format!("{}/{}/{}", mount_path, database_name, role_name)
    }

    /// Get cached vault credentials if valid and not expired
    pub fn get_cached_vault_credentials(
        &self,
        mount_path: &str,
        database_name: &str,
        role_name: &str,
    ) -> Option<&CachedVaultCredentials> {
        if !self.vault_credential_cache_enabled {
            return None;
        }

        let key = Self::vault_cache_key(mount_path, database_name, role_name);
        let credentials = self.vault_credential_storage.cached_credentials.get(&key)?;

        let now = chrono::Utc::now();

        // Check if credentials are expired
        if now >= credentials.expire_time {
            return None;
        }

        // Check if remaining TTL is above minimum threshold
        let remaining_seconds = (credentials.expire_time - now).num_seconds() as u64;
        if remaining_seconds < self.vault_cache_min_ttl_seconds {
            return None;
        }

        Some(credentials)
    }

    /// Cache vault credentials
    pub fn cache_vault_credentials(
        &mut self,
        mount_path: &str,
        database_name: &str,
        role_name: &str,
        credentials: CachedVaultCredentials,
    ) -> Result<(), Box<dyn Error>> {
        if !self.vault_credential_cache_enabled {
            return Ok(());
        }

        let key = Self::vault_cache_key(mount_path, database_name, role_name);

        // Clean up expired credentials before adding new ones
        self.cleanup_expired_vault_credentials();

        // Add the new credentials
        self.vault_credential_storage
            .cached_credentials
            .insert(key, credentials);

        // Limit cache size (LRU eviction based on issue_time)
        const MAX_CACHE_SIZE: usize = 50;
        if self.vault_credential_storage.cached_credentials.len() > MAX_CACHE_SIZE {
            // Find oldest credential by issue_time and remove it
            if let Some((oldest_key, _)) = self
                .vault_credential_storage
                .cached_credentials
                .iter()
                .min_by_key(|(_, creds)| creds.issue_time)
                .map(|(k, v)| (k.clone(), v.clone()))
            {
                self.vault_credential_storage
                    .cached_credentials
                    .remove(&oldest_key);
            }
        }

        self.save_vault_credentials()
    }

    /// Check if credentials need renewal (TTL below threshold)
    pub fn vault_credentials_need_renewal(
        &self,
        mount_path: &str,
        database_name: &str,
        role_name: &str,
    ) -> bool {
        if !self.vault_credential_cache_enabled {
            return false;
        }

        let key = Self::vault_cache_key(mount_path, database_name, role_name);
        if let Some(credentials) = self.vault_credential_storage.cached_credentials.get(&key) {
            let now = chrono::Utc::now();
            let remaining_seconds = (credentials.expire_time - now).num_seconds() as u64;
            let total_ttl = credentials.lease_duration;

            // Renew if below threshold percentage of original TTL
            let renewal_threshold_seconds =
                (total_ttl as f64 * self.vault_cache_renewal_threshold) as u64;

            credentials.renewable && remaining_seconds < renewal_threshold_seconds
        } else {
            false
        }
    }

    /// Clean up expired vault credentials
    pub fn cleanup_expired_vault_credentials(&mut self) -> usize {
        let now = chrono::Utc::now();
        let initial_count = self.vault_credential_storage.cached_credentials.len();

        self.vault_credential_storage
            .cached_credentials
            .retain(|_, creds| now < creds.expire_time);

        let removed_count = initial_count - self.vault_credential_storage.cached_credentials.len();

        // Save if we removed any credentials
        if removed_count > 0 {
            if let Err(e) = self.save_vault_credentials() {
                eprintln!("Error saving vault credentials after cleanup: {}", e);
            }
        }

        removed_count
    }

    /// Clear all cached vault credentials
    pub fn clear_vault_credentials(&mut self) -> Result<(), Box<dyn Error>> {
        self.vault_credential_storage.cached_credentials.clear();
        self.save_vault_credentials()
    }

    /// Get all cached vault credentials (for display/debugging)
    pub fn list_cached_vault_credentials(&self) -> Vec<(String, &CachedVaultCredentials)> {
        self.vault_credential_storage
            .cached_credentials
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect()
    }

    /// Force refresh vault credentials storage from file
    pub fn reload_vault_credentials(&mut self) {
        self.vault_credential_storage = Self::load_vault_credentials();
    }
}

// Removed get_test_config_path - now using Config::get_config_directory() for all paths

#[allow(dead_code)]
fn get_config_dir_impl() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".config").join("dbcrust"))
}

fn get_config_path() -> Option<PathBuf> {
    Config::get_config_directory()
        .ok()
        .map(|dir| dir.join("config.toml"))
}

fn ensure_config_dir(config_path: &Path) -> io::Result<()> {
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // Test helper function to get a clean config for tests
    fn get_test_config() -> Config {
        // Start with a default config, don't load from disk
        let mut config = Config::default();

        // Add some test data that we can verify is preserved
        config.ssh_tunnel_patterns.insert(
            "^test-pattern$".to_string(),
            "user@testhost:2222".to_string(),
        );

        config
    }

    #[rstest]
    fn test_save_and_get_session() {
        let mut config = get_test_config();

        // Set up test session data
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("testhost".to_string()),
            port: Some(5433),
            username: Some("testuser".to_string()),
            password: None,
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // Save session
        config
            .save_session_from_connection_info("test_session", &connection_info)
            .unwrap();

        // Verify session was saved
        let sessions = config.list_sessions();
        assert_eq!(sessions.len(), 1);

        // Verify session can be retrieved
        let session = config.get_session("test_session").unwrap();
        assert_eq!(session.host, "testhost");
        assert_eq!(session.port, 5433);
        assert_eq!(session.user, "testuser");
        assert_eq!(session.dbname, "testdb");
        assert_eq!(session.database_type, DatabaseType::PostgreSQL);
        assert_eq!(session.file_path, None);
        assert_eq!(session.options.len(), 0);
    }

    #[rstest]
    fn test_delete_session() {
        let mut config = get_test_config();

        // Save a test session
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("testhost".to_string()),
            port: Some(5432),
            username: Some("testuser".to_string()),
            password: None,
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };
        config
            .save_session_from_connection_info("test_session", &connection_info)
            .unwrap();

        // Verify session exists
        assert!(config.get_session("test_session").is_some());

        // Delete the session
        let deleted = config.delete_session("test_session").unwrap();
        assert!(deleted);

        // Verify session no longer exists
        assert!(config.get_session("test_session").is_none());

        // Try to delete non-existent session
        let deleted = config.delete_session("nonexistent").unwrap();
        assert!(!deleted);
    }

    // This test was removed as it tested legacy connection parameter functionality
    // that has been replaced with the session-based architecture using ConnectionInfo

    #[rstest]
    fn test_resolve_command_in_tunnel_config_simple() {
        let config = get_test_config();

        // Test with simple echo command
        let result = config.resolve_command_in_tunnel_config("user@`echo '192.168.1.100'`");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "user@192.168.1.100");
    }

    #[rstest]
    fn test_resolve_command_in_tunnel_config_multiple_commands() {
        let config = get_test_config();

        // Test with multiple commands
        let result = config.resolve_command_in_tunnel_config("user@`echo '192.168.1.100'`:2222");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "user@192.168.1.100:2222");
    }

    #[rstest]
    fn test_resolve_command_in_tunnel_config_complex_command() {
        let config = get_test_config();

        // Test with more complex command (echo with JSON-like output)
        let result =
            config.resolve_command_in_tunnel_config("user@`echo '10.200.29.189' | tr -d '\n'`");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "user@10.200.29.189");
    }

    #[rstest]
    fn test_resolve_command_in_tunnel_config_unmatched_backtick() {
        let config = get_test_config();

        // Test with unmatched backtick
        let result = config.resolve_command_in_tunnel_config("user@`echo '192.168.1.100'");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unmatched backtick")
        );
    }

    #[rstest]
    fn test_resolve_command_in_tunnel_config_failing_command() {
        let config = get_test_config();

        // Test with command that should fail
        let result = config.resolve_command_in_tunnel_config("user@`false`");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Command 'false' failed")
        );
    }

    #[rstest]
    fn test_get_ssh_tunnel_for_host_with_command() {
        let mut config = get_test_config();

        // Add a pattern that uses command substitution
        config.ssh_tunnel_patterns.insert(
            "test-host-with-command".to_string(),
            "user@`echo '192.168.1.100'`".to_string(),
        );

        // Test that the command is resolved
        let tunnel_config = config.get_ssh_tunnel_for_host("test-host-with-command");
        assert!(tunnel_config.is_some());

        let tunnel = tunnel_config.unwrap();
        assert_eq!(tunnel.ssh_host, "192.168.1.100");
        assert_eq!(tunnel.ssh_username, Some("user".to_string()));
        assert_eq!(tunnel.ssh_port, 22);
        assert!(tunnel.enabled);
    }

    #[rstest]
    fn test_aws_rds_example_pattern() {
        let mut config = get_test_config();

        // Add a pattern similar to the user's example
        config.ssh_tunnel_patterns.insert(
            ".*\\.c7aht5uvgwcu\\.us-west-2\\.rds\\.amazonaws\\.com".to_string(),
            "app@`echo '10.200.29.189'`".to_string(),
        );

        // Test that the command is resolved for AWS RDS hostname
        let tunnel_config =
            config.get_ssh_tunnel_for_host("mydb.c7aht5uvgwcu.us-west-2.rds.amazonaws.com");
        assert!(tunnel_config.is_some());

        let tunnel = tunnel_config.unwrap();
        assert_eq!(tunnel.ssh_host, "10.200.29.189");
        assert_eq!(tunnel.ssh_username, Some("app".to_string()));
        assert_eq!(tunnel.ssh_port, 22);
        assert!(tunnel.enabled);
    }

    // ===================
    // Session Management Tests
    // ===================

    #[rstest]
    fn test_recent_connection_add_and_retrieve() {
        let mut config = get_test_config();

        // Add a recent connection
        let result = config.add_recent_connection_auto_display(
            "postgres://user@localhost:5432/testdb".to_string(),
            DatabaseType::PostgreSQL,
            true,
        );
        assert!(result.is_ok());

        // Verify it was added
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 1);
        assert_eq!(
            recent[0].connection_url,
            "postgres://user@localhost:5432/testdb"
        );
        assert_eq!(recent[0].database_type, DatabaseType::PostgreSQL);
        assert!(recent[0].success);
        // No session_name field in the new separated architecture
    }

    #[rstest]
    fn test_recent_connection_with_display_name() {
        let mut config = get_test_config();

        // Add a recent connection and verify display name is generated
        let result = config.add_recent_connection_auto_display(
            "mysql://user@localhost:3306/testdb".to_string(),
            DatabaseType::MySQL,
            true,
        );
        assert!(result.is_ok());

        // Verify display name was generated from URL
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].display_name, "user@localhost:3306/testdb");
        assert_eq!(
            recent[0].connection_url,
            "mysql://user@localhost:3306/testdb"
        );
    }

    #[rstest]
    fn test_recent_connection_max_limit() {
        let mut config = get_test_config();

        // Add more connections than the configured limit
        let limit = config.max_recent_connections;
        for i in 0..(limit + 5) {
            let url = format!("postgres://user@localhost:5432/testdb{i}");
            let result =
                config.add_recent_connection_auto_display(url, DatabaseType::PostgreSQL, true);
            assert!(result.is_ok());
        }

        // Verify only the configured number are kept
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), limit);

        // Verify most recent is first
        assert!(
            recent[0]
                .connection_url
                .contains(&format!("testdb{}", limit + 4))
        );
        assert!(
            recent[limit - 1]
                .connection_url
                .contains(&format!("testdb{}", 5))
        );
    }

    #[rstest]
    fn test_configurable_max_recent_connections() {
        let mut config = get_test_config();

        // Test with default value (10)
        assert_eq!(config.max_recent_connections, 10);

        // Change the configuration
        config.max_recent_connections = 5;

        // Add 8 connections (more than the new limit of 5)
        for i in 0..8 {
            let url = format!("postgres://user@localhost:5432/testdb{i}");
            let result =
                config.add_recent_connection_auto_display(url, DatabaseType::PostgreSQL, true);
            assert!(result.is_ok());
        }

        // Verify only 5 are kept
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 5);

        // Verify most recent connections are kept (testdb7, testdb6, ..., testdb3)
        assert!(recent[0].connection_url.contains("testdb7"));
        assert!(recent[4].connection_url.contains("testdb3"));
    }

    #[rstest]
    fn test_recent_connection_ordering() {
        let mut config = get_test_config();

        // Add connections in order
        for i in 0..3 {
            let url = format!("postgres://user@localhost:5432/testdb{i}");
            let result =
                config.add_recent_connection_auto_display(url, DatabaseType::PostgreSQL, true);
            assert!(result.is_ok());

            // Small delay to ensure different timestamps
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // Verify most recent is first
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 3);
        assert!(recent[0].connection_url.contains("testdb2"));
        assert!(recent[1].connection_url.contains("testdb1"));
        assert!(recent[2].connection_url.contains("testdb0"));
    }

    #[rstest]
    fn test_clear_recent_connections() {
        let mut config = get_test_config();

        // Add some connections
        for i in 0..3 {
            let url = format!("postgres://user@localhost:5432/testdb{i}");
            let result =
                config.add_recent_connection_auto_display(url, DatabaseType::PostgreSQL, true);
            assert!(result.is_ok());
        }

        // Verify they were added
        assert_eq!(config.get_recent_connections().len(), 3);

        // Clear them
        let result = config.clear_recent_connections();
        assert!(result.is_ok());

        // Verify they were cleared
        assert_eq!(config.get_recent_connections().len(), 0);
    }

    #[rstest]
    fn test_save_session_with_database_types() {
        let mut config = get_test_config();

        // Test PostgreSQL session
        let pg_connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("pg.example.com".to_string()),
            port: Some(5432),
            username: Some("pguser".to_string()),
            password: None,
            database: Some("pgdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let result = config.save_session_from_connection_info("pg_session", &pg_connection_info);
        assert!(result.is_ok());

        // Test MySQL session
        let mysql_connection_info = ConnectionInfo {
            database_type: DatabaseType::MySQL,
            host: Some("mysql.example.com".to_string()),
            port: Some(3306),
            username: Some("mysqluser".to_string()),
            password: None,
            database: Some("mysqldb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let result =
            config.save_session_from_connection_info("mysql_session", &mysql_connection_info);
        assert!(result.is_ok());

        // Test SQLite session
        let sqlite_connection_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some("/path/to/db.sqlite".to_string()),
            options: HashMap::new(),
            docker_container: None,
        };

        let result =
            config.save_session_from_connection_info("sqlite_session", &sqlite_connection_info);
        assert!(result.is_ok());

        // Verify all sessions were saved
        let sessions = config.list_sessions();
        assert_eq!(sessions.len(), 3);

        // Verify PostgreSQL session
        let pg_session = config.get_session("pg_session").unwrap();
        assert_eq!(pg_session.database_type, DatabaseType::PostgreSQL);
        assert_eq!(pg_session.host, "pg.example.com");
        assert_eq!(pg_session.port, 5432);
        assert_eq!(pg_session.user, "pguser");
        assert_eq!(pg_session.dbname, "pgdb");
        assert_eq!(pg_session.file_path, None);

        // Verify MySQL session
        let mysql_session = config.get_session("mysql_session").unwrap();
        assert_eq!(mysql_session.database_type, DatabaseType::MySQL);
        assert_eq!(mysql_session.host, "mysql.example.com");
        assert_eq!(mysql_session.port, 3306);

        // Verify SQLite session
        let sqlite_session = config.get_session("sqlite_session").unwrap();
        assert_eq!(sqlite_session.database_type, DatabaseType::SQLite);
        assert_eq!(
            sqlite_session.file_path,
            Some("/path/to/db.sqlite".to_string())
        );
    }

    #[rstest]
    fn test_session_serialization_with_recent_connections() {
        let mut config = get_test_config();

        // Add a session and recent connections
        let test_connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("testhost".to_string()),
            port: Some(5432),
            username: Some("testuser".to_string()),
            password: None,
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let result =
            config.save_session_from_connection_info("test_session", &test_connection_info);
        assert!(result.is_ok());

        let result = config.add_recent_connection_auto_display(
            "postgres://user@localhost:5432/testdb".to_string(),
            DatabaseType::PostgreSQL,
            true,
        );
        assert!(result.is_ok());

        // Save config (sessions are saved to config.toml, recent connections to recent.toml)
        let save_result = config.save();
        assert!(save_result.is_ok());

        // For testing, just verify that sessions and recent connections are managed separately
        // In real usage, Config::load() would reload both files properly

        // Verify session was saved to the main config
        assert!(config.get_session("test_session").is_some());
        let session = config.get_session("test_session").unwrap();
        assert_eq!(session.database_type, DatabaseType::PostgreSQL);

        // Verify recent connections are in separate storage
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 1);
        assert_eq!(
            recent[0].connection_url,
            "postgres://user@localhost:5432/testdb"
        );
    }

    #[rstest]
    fn test_sqlite_path_normalization() {
        let mut config = get_test_config();

        // Test that relative paths get converted to absolute paths for saved sessions
        let sqlite_relative_connection_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some("test.db".to_string()),
            options: HashMap::new(),
            docker_container: None,
        };

        let result = config
            .save_session_from_connection_info("sqlite_relative", &sqlite_relative_connection_info);
        assert!(result.is_ok());

        let session = config.get_session("sqlite_relative").unwrap();
        assert!(session.file_path.as_ref().unwrap().starts_with("/"));
        assert!(session.file_path.as_ref().unwrap().ends_with("test.db"));

        // Test that absolute paths are preserved/canonicalized for saved sessions
        let temp_dir = std::env::temp_dir();
        let abs_path = temp_dir.join("absolute_test.db");
        let abs_path_str = abs_path.to_string_lossy().to_string();

        let sqlite_absolute_connection_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some(abs_path_str.clone()),
            options: HashMap::new(),
            docker_container: None,
        };

        let result = config
            .save_session_from_connection_info("sqlite_absolute", &sqlite_absolute_connection_info);
        assert!(result.is_ok());

        let session = config.get_session("sqlite_absolute").unwrap();
        assert!(session.file_path.as_ref().unwrap().starts_with("/"));

        // Test recent connections path normalization
        let result = config.add_recent_connection_auto_display(
            "sqlite://relative_test.db".to_string(),
            DatabaseType::SQLite,
            true,
        );
        assert!(result.is_ok());

        // Test the problematic case from the user's report: sqlite:///test_data/test_sample.db (3 slashes = relative)
        let result = config.add_recent_connection_auto_display(
            "sqlite:///test_data/test_sample.db".to_string(),
            DatabaseType::SQLite,
            true,
        );
        assert!(result.is_ok());

        // Test absolute path case: sqlite:////absolute/path (4 slashes = absolute, should be kept as is)
        let result = config.add_recent_connection_auto_display(
            "sqlite:////tmp/absolute_test.db".to_string(),
            DatabaseType::SQLite,
            true,
        );
        assert!(result.is_ok());

        let recent = config.get_recent_connections();

        // Find the sqlite:///test_data/test_sample.db connection (3 slashes = relative, should be normalized to 4 slashes)
        let sqlite_relative = recent
            .iter()
            .find(|r| {
                r.database_type == DatabaseType::SQLite
                    && r.connection_url.contains("test_sample.db")
            })
            .unwrap();
        assert!(sqlite_relative.connection_url.starts_with("sqlite:///"));
        // Should be normalized to absolute path with 4 slashes total: sqlite:////absolute/path
        let path_part = sqlite_relative
            .connection_url
            .strip_prefix("sqlite://")
            .unwrap();
        assert!(path_part.starts_with("//")); // This indicates 4 slashes total
        assert!(path_part.ends_with("test_data/test_sample.db"));

        // Find the sqlite:////tmp/absolute_test.db connection (4 slashes = absolute, should be unchanged)
        let sqlite_absolute = recent
            .iter()
            .find(|r| {
                r.database_type == DatabaseType::SQLite
                    && r.connection_url.contains("absolute_test.db")
            })
            .unwrap();
        assert_eq!(
            sqlite_absolute.connection_url,
            "sqlite:////tmp/absolute_test.db"
        ); // Should be unchanged

        // Test that non-SQLite databases are not affected
        let postgres_connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("testuser".to_string()),
            password: None,
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let result =
            config.save_session_from_connection_info("postgres_test", &postgres_connection_info);
        assert!(result.is_ok());

        let result = config.add_recent_connection_auto_display(
            "postgres://user@localhost:5432/test".to_string(),
            DatabaseType::PostgreSQL,
            true,
        );
        assert!(result.is_ok());
    }

    #[rstest]
    fn test_recent_connection_database_types() {
        let mut config = get_test_config();

        // Add connections for each database type
        let result1 = config.add_recent_connection_auto_display(
            "postgres://user@localhost:5432/pgdb".to_string(),
            DatabaseType::PostgreSQL,
            true,
        );
        assert!(result1.is_ok());

        let result2 = config.add_recent_connection_auto_display(
            "mysql://user@localhost:3306/mysqldb".to_string(),
            DatabaseType::MySQL,
            true,
        );
        assert!(result2.is_ok());

        let result3 = config.add_recent_connection_auto_display(
            "sqlite:///path/to/db.sqlite".to_string(),
            DatabaseType::SQLite,
            true,
        );
        assert!(result3.is_ok());

        // Verify all database types are tracked
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 3);

        // Verify database types (most recent first)
        assert_eq!(recent[0].database_type, DatabaseType::SQLite);
        assert_eq!(recent[1].database_type, DatabaseType::MySQL);
        assert_eq!(recent[2].database_type, DatabaseType::PostgreSQL);
    }

    #[rstest]
    fn test_recent_connection_success_failure_tracking() {
        let mut config = get_test_config();

        // Add successful connection
        let result1 = config.add_recent_connection_auto_display(
            "postgres://user@localhost:5432/testdb".to_string(),
            DatabaseType::PostgreSQL,
            true,
        );
        assert!(result1.is_ok());

        // Add failed connection
        let result2 = config.add_recent_connection_auto_display(
            "postgres://user@badhost:5432/testdb".to_string(),
            DatabaseType::PostgreSQL,
            false,
        );
        assert!(result2.is_ok());

        // Verify success/failure tracking
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 2);
        assert!(!recent[0].success); // Most recent (failed)
        assert!(recent[1].success); // Previous (successful)
    }

    #[rstest]
    fn test_generate_display_name_docker_connections() {
        // Test Docker connection with complete resolved URL
        let docker_url = "postgres://user@container.orb.local:5432/myapp # Docker: tt2-postgres";
        let display_name =
            Config::generate_display_name_from_url(docker_url, &DatabaseType::PostgreSQL);
        assert_eq!(
            display_name,
            "user@container.orb.local:5432/myapp (Docker: tt2-postgres)"
        );

        // Test MySQL Docker connection
        let mysql_docker_url = "mysql://root@localhost:3306/testdb # Docker: mysql-container";
        let mysql_display_name =
            Config::generate_display_name_from_url(mysql_docker_url, &DatabaseType::MySQL);
        assert_eq!(
            mysql_display_name,
            "root@localhost:3306/testdb (Docker: mysql-container)"
        );
    }

    #[rstest]
    fn test_generate_display_name_standard_connections() {
        // Test standard PostgreSQL connection
        let pg_url = "postgres://user@host.example.com:5432/database";
        let pg_display_name =
            Config::generate_display_name_from_url(pg_url, &DatabaseType::PostgreSQL);
        assert_eq!(pg_display_name, "user@host.example.com:5432/database");

        // Test SQLite connection
        let sqlite_url = "sqlite:///path/to/database.db";
        let sqlite_display_name =
            Config::generate_display_name_from_url(sqlite_url, &DatabaseType::SQLite);
        assert_eq!(sqlite_display_name, "/path/to/database.db");

        // Test session URL
        let session_url = "session://production";
        let session_display_name =
            Config::generate_display_name_from_url(session_url, &DatabaseType::PostgreSQL);
        assert_eq!(session_display_name, "production");
    }

    #[rstest]
    fn test_docker_resolved_url_storage() {
        let mut config = get_test_config();

        // Simulate a Docker connection that gets resolved to a complete URL
        let resolved_docker_url =
            "postgres://postgres@myapp-postgres.orb.local:5432/myapp # Docker: myapp-postgres";

        let result = config.add_recent_connection_auto_display(
            resolved_docker_url.to_string(),
            DatabaseType::PostgreSQL,
            true,
        );
        assert!(result.is_ok());

        // Verify the connection was stored with complete details
        let recent = config.get_recent_connections();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].connection_url, resolved_docker_url);
        assert_eq!(
            recent[0].display_name,
            "postgres@myapp-postgres.orb.local:5432/myapp (Docker: myapp-postgres)"
        );
        assert_eq!(recent[0].database_type, DatabaseType::PostgreSQL);
        assert!(recent[0].success);
    }

    #[rstest]
    fn test_session_update_existing() {
        let mut config = get_test_config();

        // Save initial session
        let initial_connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("oldhost".to_string()),
            port: Some(5432),
            username: Some("olduser".to_string()),
            password: None,
            database: Some("olddb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let result =
            config.save_session_from_connection_info("updatable_session", &initial_connection_info);
        assert!(result.is_ok());

        // Update connection details
        let updated_connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("newhost".to_string()),
            port: Some(5433),
            username: Some("newuser".to_string()),
            password: None,
            database: Some("newdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // Save session with same name (should update)
        let result =
            config.save_session_from_connection_info("updatable_session", &updated_connection_info);
        assert!(result.is_ok());

        // Verify session was updated, not duplicated
        let sessions = config.list_sessions();
        assert_eq!(sessions.len(), 1);

        let session = config.get_session("updatable_session").unwrap();
        assert_eq!(session.host, "newhost");
        assert_eq!(session.port, 5433);
        assert_eq!(session.user, "newuser");
        assert_eq!(session.dbname, "newdb");
    }
}
