use dirs::home_dir;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use crate::database::DatabaseType;

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
pub struct SavedSession {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub dbname: String,
    // No password here - passwords will be stored in .pgpass
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub dbname: String,
    pub save_password: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub ssh_tunnel: Option<SSHTunnelConfig>,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        ConnectionConfig {
            host: "localhost".to_string(),
            port: 5432,
            user: "postgres".to_string(),
            dbname: "postgres".to_string(),
            save_password: false,
            password: None,
            ssh_tunnel: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    #[serde(default)]
    pub connection: ConnectionConfig,
    pub default_limit: usize,
    pub expanded_display_default: bool,
    pub autocomplete_enabled: bool,
    pub explain_mode_default: bool,
    #[serde(default)]
    pub column_selection_mode_default: bool,
    #[serde(default = "default_column_selection_threshold")]
    pub column_selection_threshold: usize,
    #[serde(default)]
    pub named_queries: HashMap<String, String>,
    #[serde(default)]
    pub saved_sessions: HashMap<String, SavedSession>,
    #[serde(default)]
    pub ssh_tunnel_patterns: HashMap<String, String>,

    #[serde(default = "default_pager_enabled")]
    pub pager_enabled: bool,
    #[serde(default = "default_pager_command")]
    pub pager_command: String,
    #[serde(default = "default_pager_threshold_lines")]
    pub pager_threshold_lines: usize, // 0 means use terminal height

    #[serde(default = "default_debug_logging")]
    pub debug_logging_enabled: bool,

    #[serde(default = "default_show_banner")]
    pub show_banner_default: bool,

    #[serde(default = "default_multiline_prompt_indicator")]
    pub multiline_prompt_indicator: String,

    // Legacy fields - support deserializing from old config format
    // These will be skipped during serialization
    #[serde(skip_serializing, default)]
    pub host: String,
    #[serde(skip_serializing, default)]
    pub port: u16,
    #[serde(skip_serializing, default)]
    pub user: String,
    #[serde(skip_serializing, default)]
    pub dbname: String,
    #[serde(skip_serializing, default)]
    pub save_password: bool,
    #[serde(skip_serializing, skip_deserializing, default)]
    pub password: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let connection = ConnectionConfig::default();
        Config {
            connection: connection.clone(),
            default_limit: 100,
            expanded_display_default: false,
            autocomplete_enabled: true,
            explain_mode_default: false,
            column_selection_mode_default: false,
            column_selection_threshold: default_column_selection_threshold(),
            named_queries: HashMap::new(),
            saved_sessions: HashMap::new(),
            ssh_tunnel_patterns: HashMap::new(),
            pager_enabled: default_pager_enabled(),
            pager_command: default_pager_command(),
            pager_threshold_lines: default_pager_threshold_lines(),
            debug_logging_enabled: default_debug_logging(),
            show_banner_default: default_show_banner(),
            multiline_prompt_indicator: default_multiline_prompt_indicator(),
            // Legacy fields initialized from connection
            host: connection.host.clone(),
            port: connection.port,
            user: connection.user.clone(),
            dbname: connection.dbname.clone(),
            save_password: connection.save_password,
            password: connection.password.clone(),
        }
    }
}

fn default_column_selection_threshold() -> usize {
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

fn default_debug_logging() -> bool {
    false
}

fn default_show_banner() -> bool {
    false
}

fn default_multiline_prompt_indicator() -> String {
    String::new() // Empty string by default (no indicator)
}

fn default_database_type() -> DatabaseType {
    DatabaseType::PostgreSQL
}

impl Config {
    pub fn load() -> Self {
        if let Some(config_path) = get_config_path() {
            match fs::read_to_string(&config_path) {
                Ok(content) => {
                    let config_result: Result<Config, toml::de::Error> = toml::from_str(&content);

                    match config_result {
                        Ok(mut config) => {
                            // Handle legacy config files by copying top-level connection fields to the connection struct
                            // This check detects if we're dealing with an old config format
                            if content.contains("host =") && !content.contains("connection") {
                                config.connection.host = config.host.clone();
                                config.connection.port = config.port;
                                config.connection.user = config.user.clone();
                                config.connection.dbname = config.dbname.clone();
                                config.connection.save_password = config.save_password;

                                // Save the updated format immediately to migrate the file
                                if let Err(e) = config.save() {
                                    eprintln!("Error migrating config file: {}", e);
                                }
                            } else {
                                // If using the new format, make sure legacy fields are in sync
                                config.host = config.connection.host.clone();
                                config.port = config.connection.port;
                                config.user = config.connection.user.clone();
                                config.dbname = config.connection.dbname.clone();
                                config.save_password = config.connection.save_password;
                                config.password = config.connection.password.clone();
                            }
                            config
                        }
                        Err(e) => {
                            eprintln!("Error parsing config file ({}), attempting partial load", e);

                            // Try to load the file with a more lenient approach
                            // First, parse it as a generic TOML Value
                            match toml::from_str::<toml::Value>(&content) {
                                Ok(toml_value) => {
                                    let mut config = Config::default();

                                    // Extract the fields we can from the TOML value
                                    if let Some(table) = toml_value.as_table() {
                                        // Extract simple fields
                                        if let Some(limit) =
                                            table.get("default_limit").and_then(|v| v.as_integer())
                                        {
                                            config.default_limit = limit as usize;
                                        }

                                        if let Some(expanded) = table
                                            .get("expanded_display_default")
                                            .and_then(|v| v.as_bool())
                                        {
                                            config.expanded_display_default = expanded;
                                        }

                                        if let Some(autocomplete) = table
                                            .get("autocomplete_enabled")
                                            .and_then(|v| v.as_bool())
                                        {
                                            config.autocomplete_enabled = autocomplete;
                                        }

                                        if let Some(explain) = table
                                            .get("explain_mode_default")
                                            .and_then(|v| v.as_bool())
                                        {
                                            config.explain_mode_default = explain;
                                        }

                                        // Extract connection info if available
                                        if let Some(conn) =
                                            table.get("connection").and_then(|v| v.as_table())
                                        {
                                            if let Some(host) =
                                                conn.get("host").and_then(|v| v.as_str())
                                            {
                                                config.connection.host = host.to_string();
                                                config.host = host.to_string();
                                            }

                                            if let Some(port) =
                                                conn.get("port").and_then(|v| v.as_integer())
                                            {
                                                config.connection.port = port as u16;
                                                config.port = port as u16;
                                            }

                                            if let Some(user) =
                                                conn.get("user").and_then(|v| v.as_str())
                                            {
                                                config.connection.user = user.to_string();
                                                config.user = user.to_string();
                                            }

                                            if let Some(dbname) =
                                                conn.get("dbname").and_then(|v| v.as_str())
                                            {
                                                config.connection.dbname = dbname.to_string();
                                                config.dbname = dbname.to_string();
                                            }

                                            if let Some(save_pwd) =
                                                conn.get("save_password").and_then(|v| v.as_bool())
                                            {
                                                config.connection.save_password = save_pwd;
                                                config.save_password = save_pwd;
                                            }
                                        }

                                        // Extract legacy connection info if not using connection table
                                        if !table.contains_key("connection") {
                                            if let Some(host) =
                                                table.get("host").and_then(|v| v.as_str())
                                            {
                                                config.connection.host = host.to_string();
                                                config.host = host.to_string();
                                            }

                                            if let Some(port) =
                                                table.get("port").and_then(|v| v.as_integer())
                                            {
                                                config.connection.port = port as u16;
                                                config.port = port as u16;
                                            }

                                            if let Some(user) =
                                                table.get("user").and_then(|v| v.as_str())
                                            {
                                                config.connection.user = user.to_string();
                                                config.user = user.to_string();
                                            }

                                            if let Some(dbname) =
                                                table.get("dbname").and_then(|v| v.as_str())
                                            {
                                                config.connection.dbname = dbname.to_string();
                                                config.dbname = dbname.to_string();
                                            }

                                            if let Some(save_pwd) =
                                                table.get("save_password").and_then(|v| v.as_bool())
                                            {
                                                config.connection.save_password = save_pwd;
                                                config.save_password = save_pwd;
                                            }
                                        }

                                        // Extract named queries
                                        if let Some(queries) =
                                            table.get("named_queries").and_then(|v| v.as_table())
                                        {
                                            for (name, value) in queries {
                                                if let Some(query) = value.as_str() {
                                                    config
                                                        .named_queries
                                                        .insert(name.clone(), query.to_string());
                                                }
                                            }
                                        }

                                        // Extract saved sessions
                                        if let Some(sessions) =
                                            table.get("saved_sessions").and_then(|v| v.as_table())
                                        {
                                            for (name, value) in sessions {
                                                if let Some(session) = value.as_table() {
                                                    let mut session_config = SavedSession {
                                                        host: "localhost".to_string(),
                                                        port: 5432,
                                                        user: "postgres".to_string(),
                                                        dbname: "postgres".to_string(),
                                                        ssh_tunnel: None,
                                                        database_type: DatabaseType::PostgreSQL,
                                                        file_path: None,
                                                        options: HashMap::new(),
                                                    };

                                                    if let Some(host) =
                                                        session.get("host").and_then(|v| v.as_str())
                                                    {
                                                        session_config.host = host.to_string();
                                                    }

                                                    if let Some(port) = session
                                                        .get("port")
                                                        .and_then(|v| v.as_integer())
                                                    {
                                                        session_config.port = port as u16;
                                                    }

                                                    if let Some(user) =
                                                        session.get("user").and_then(|v| v.as_str())
                                                    {
                                                        session_config.user = user.to_string();
                                                    }

                                                    if let Some(dbname) = session
                                                        .get("dbname")
                                                        .and_then(|v| v.as_str())
                                                    {
                                                        session_config.dbname = dbname.to_string();
                                                    }

                                                    config
                                                        .saved_sessions
                                                        .insert(name.clone(), session_config);
                                                }
                                            }
                                        }
                                    }

                                    // Save the updated config to fix the format
                                    if let Err(save_err) = config.save() {
                                        eprintln!("Error saving updated config: {}", save_err);
                                    } else {
                                        println!("Config file updated with new format");
                                    }

                                    config
                                }
                                Err(_) => {
                                    eprintln!(
                                        "Could not parse config file as TOML, using defaults"
                                    );
                                    Config::default()
                                }
                            }
                        }
                    }
                }
                Err(_) => Config::default(),
            }
        } else {
            Config::default()
        }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(config_path) = get_config_path() {
            ensure_config_dir(&config_path)?;

            let toml = toml::to_string(self).map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("Serialization error: {}", e))
            })?;

            let mut file = File::create(&config_path)?;
            file.write_all(toml.as_bytes())?;
        }
        Ok(())
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
    pub fn save_session(&mut self, name: &str) -> Result<(), Box<dyn Error>> {
        let session = SavedSession {
            host: self.connection.host.clone(),
            port: self.connection.port,
            user: self.connection.user.clone(),
            dbname: self.connection.dbname.clone(),
            ssh_tunnel: self.connection.ssh_tunnel.clone(),
            database_type: DatabaseType::PostgreSQL, // Default for legacy support
            file_path: None,
            options: HashMap::new(),
        };

        self.saved_sessions.insert(name.to_string(), session);
        self.save()?;
        Ok(())
    }

    /// Save session with database type information for multi-database support
    pub fn save_session_with_db_type(&mut self, name: &str, database_type: DatabaseType, file_path: Option<String>, options: HashMap<String, String>) -> Result<(), Box<dyn Error>> {
        let session = SavedSession {
            host: self.connection.host.clone(),
            port: self.connection.port,
            user: self.connection.user.clone(),
            dbname: self.connection.dbname.clone(),
            ssh_tunnel: self.connection.ssh_tunnel.clone(),
            database_type,
            file_path,
            options,
        };

        self.saved_sessions.insert(name.to_string(), session);
        self.save()?;
        Ok(())
    }

    pub fn delete_session(&mut self, name: &str) -> Result<bool, Box<dyn Error>> {
        let existed = self.saved_sessions.remove(name).is_some();
        if existed {
            self.save()?;
        }
        Ok(existed)
    }

    pub fn get_session(&self, name: &str) -> Option<&SavedSession> {
        self.saved_sessions.get(name)
    }

    pub fn list_sessions(&self) -> Vec<(String, SavedSession)> {
        self.saved_sessions
            .iter()
            .map(|(name, session)| (name.clone(), session.clone()))
            .collect()
    }

    /// Updates connection parameters without saving the configuration
    /// This is crucial for ensuring we don't accidentally overwrite user
    /// defined settings like ssh_tunnel_patterns when updating connection details
    pub fn update_connection_params(
        &mut self,
        host: String,
        port: u16,
        user: String,
        dbname: String,
    ) {
        self.connection.host = host.clone();
        self.connection.port = port;
        self.connection.user = user.clone();
        self.connection.dbname = dbname.clone();

        // Keep legacy fields in sync
        self.host = host;
        self.port = port;
        self.user = user;
        self.dbname = dbname;
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

    fn resolve_command_in_tunnel_config(&self, tunnel_config: &str) -> Result<String, Box<dyn Error>> {
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
                        .map_err(|e| format!("Failed to execute command '{}': {}", command_str, e))?
                } else {
                    Command::new("sh")
                        .args(["-c", command_str])
                        .output()
                        .map_err(|e| format!("Failed to execute command '{}': {}", command_str, e))?
                };
                
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!(
                        "Command '{}' failed with exit code {}: {}",
                        command_str,
                        output.status.code().unwrap_or(-1),
                        stderr
                    ).into());
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
                                eprintln!("Error executing command in SSH tunnel pattern: {}", e);
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
}

#[cfg(test)]
pub fn get_test_config_path() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let test_dir = temp_dir.join(format!("dbcrust_test_{}_{}", pid, timestamp));

    // Create directory if it doesn't exist
    if !test_dir.exists() {
        let _ = std::fs::create_dir_all(&test_dir);
    }

    test_dir.join("config.toml")
}

pub fn get_config_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".config").join("dbcrust"))
}

fn get_config_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(get_test_config_path())
    }

    #[cfg(not(test))]
    {
        get_config_dir().map(|dir| dir.join("config.toml"))
    }
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
        config.connection.host = "testhost".to_string();
        config.connection.port = 5433;
        config.connection.user = "testuser".to_string();
        config.connection.dbname = "testdb".to_string();

        // Save session
        config.save_session("test_session").unwrap();

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
        config.connection.host = "testhost".to_string();
        config.save_session("test_session").unwrap();

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

    #[rstest]
    fn test_update_connection_params_preserves_custom_sections() {
        // Create a config with ssh_tunnel_patterns
        let mut config = get_test_config();

        // Update connection parameters
        config.update_connection_params(
            "new-host".to_string(),
            5433,
            "new-user".to_string(),
            "new-dbname".to_string(),
        );

        // Verify connection params were updated
        assert_eq!(config.connection.host, "new-host");
        assert_eq!(config.connection.port, 5433);
        assert_eq!(config.connection.user, "new-user");
        assert_eq!(config.connection.dbname, "new-dbname");

        // Verify legacy fields were also updated
        assert_eq!(config.host, "new-host");
        assert_eq!(config.port, 5433);
        assert_eq!(config.user, "new-user");
        assert_eq!(config.dbname, "new-dbname");

        // Verify ssh_tunnel_patterns were preserved
        assert_eq!(config.ssh_tunnel_patterns.len(), 1);
        assert!(config.ssh_tunnel_patterns.contains_key("^test-pattern$"));
        assert_eq!(
            config.ssh_tunnel_patterns.get("^test-pattern$").unwrap(),
            "user@testhost:2222"
        );
    }

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
        let result = config.resolve_command_in_tunnel_config("user@`echo '10.200.29.189' | tr -d '\n'`");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "user@10.200.29.189");
    }

    #[rstest]
    fn test_resolve_command_in_tunnel_config_unmatched_backtick() {
        let config = get_test_config();
        
        // Test with unmatched backtick
        let result = config.resolve_command_in_tunnel_config("user@`echo '192.168.1.100'");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unmatched backtick"));
    }

    #[rstest]
    fn test_resolve_command_in_tunnel_config_failing_command() {
        let config = get_test_config();
        
        // Test with command that should fail
        let result = config.resolve_command_in_tunnel_config("user@`false`");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Command 'false' failed"));
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
        let tunnel_config = config.get_ssh_tunnel_for_host("mydb.c7aht5uvgwcu.us-west-2.rds.amazonaws.com");
        assert!(tunnel_config.is_some());
        
        let tunnel = tunnel_config.unwrap();
        assert_eq!(tunnel.ssh_host, "10.200.29.189");
        assert_eq!(tunnel.ssh_username, Some("app".to_string()));
        assert_eq!(tunnel.ssh_port, 22);
        assert!(tunnel.enabled);
    }
}
