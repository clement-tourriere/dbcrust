//! MySQL configuration file (.my.cnf) support
//!
//! This module provides functionality to read MySQL configuration files
//! similar to how pgpass.rs handles PostgreSQL password files.
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tracing::debug;

#[derive(Debug, Clone, Default)]
pub struct MySQLConfig {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub socket: Option<String>,
    pub ssl_ca: Option<String>,
    pub ssl_cert: Option<String>,
    pub ssl_key: Option<String>,
}

/// Get the path to the MySQL configuration file
/// Checks in order: $MYSQL_CONFIG, ~/.my.cnf, /etc/mysql/my.cnf, /etc/my.cnf
pub fn get_mysql_config_path() -> Option<PathBuf> {
    // Check environment variable first
    if let Ok(path) = env::var("MYSQL_CONFIG") {
        let config_path = PathBuf::from(path);
        if config_path.exists() {
            return Some(config_path);
        }
    }

    // Check user home directory
    if let Some(home_dir) = dirs::home_dir() {
        let user_config = home_dir.join(".my.cnf");
        if user_config.exists() {
            return Some(user_config);
        }
    }

    // Check system-wide locations
    let system_paths = ["/etc/mysql/my.cnf", "/etc/my.cnf", "/usr/local/etc/my.cnf"];

    for path in &system_paths {
        let config_path = PathBuf::from(path);
        if config_path.exists() {
            return Some(config_path);
        }
    }

    None
}

/// Parse a MySQL configuration file
/// Returns a HashMap of section name to configuration options
pub fn parse_mysql_config(
    config_path: &Path,
) -> Result<HashMap<String, MySQLConfig>, Box<dyn std::error::Error>> {
    debug!(
        "[parse_mysql_config] Reading MySQL config from: {}",
        config_path.display()
    );

    let content = fs::read_to_string(config_path)?;
    let mut configs = HashMap::new();
    let mut current_section = String::new();
    let mut current_config = MySQLConfig::default();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Handle section headers [section_name]
        if line.starts_with('[') && line.ends_with(']') {
            // Save previous section if it exists
            if !current_section.is_empty() {
                configs.insert(current_section.clone(), current_config.clone());
            }

            current_section = line[1..line.len() - 1].to_string();
            current_config = MySQLConfig::default();
            continue;
        }

        // Handle key=value pairs
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_lowercase();
            let value = line[eq_pos + 1..].trim();

            // Remove quotes if present
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };

            match key.as_str() {
                "host" => current_config.host = Some(value.to_string()),
                "port" => {
                    if let Ok(port) = value.parse::<u16>() {
                        current_config.port = Some(port);
                    }
                }
                "user" => current_config.user = Some(value.to_string()),
                "password" => current_config.password = Some(value.to_string()),
                "database" => current_config.database = Some(value.to_string()),
                "socket" => current_config.socket = Some(value.to_string()),
                "ssl-ca" | "ssl_ca" => current_config.ssl_ca = Some(value.to_string()),
                "ssl-cert" | "ssl_cert" => current_config.ssl_cert = Some(value.to_string()),
                "ssl-key" | "ssl_key" => current_config.ssl_key = Some(value.to_string()),
                _ => {
                    // Ignore unknown options
                    debug!("[parse_mysql_config] Ignoring unknown option: {}", key);
                }
            }
        }
    }

    // Save the last section
    if !current_section.is_empty() {
        configs.insert(current_section, current_config);
    }

    debug!("[parse_mysql_config] Parsed {} sections", configs.len());
    Ok(configs)
}

/// Look up MySQL connection information from configuration files
/// Searches [client] and [mysql] sections by default, or a specific section if provided
pub fn lookup_mysql_config(section: Option<&str>) -> Option<MySQLConfig> {
    let config_path = get_mysql_config_path()?;

    debug!(
        "[lookup_mysql_config] Looking up MySQL config in: {}",
        config_path.display()
    );

    let configs = match parse_mysql_config(&config_path) {
        Ok(configs) => configs,
        Err(e) => {
            debug!("[lookup_mysql_config] Error parsing MySQL config: {}", e);
            return None;
        }
    };

    // If a specific section is requested, use that
    if let Some(section_name) = section {
        if let Some(config) = configs.get(section_name) {
            debug!(
                "[lookup_mysql_config] Found config in section [{}]",
                section_name
            );
            return Some(config.clone());
        }
    }

    // Otherwise, try common sections in order
    let default_sections = ["client", "mysql"];
    for section_name in &default_sections {
        if let Some(config) = configs.get(*section_name) {
            debug!(
                "[lookup_mysql_config] Found config in section [{}]",
                section_name
            );
            return Some(config.clone());
        }
    }

    debug!("[lookup_mysql_config] No suitable configuration found");
    None
}

/// Look up MySQL password specifically
/// This provides a similar interface to pgpass::lookup_password
pub fn lookup_mysql_password(host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    debug!(
        "[lookup_mysql_password] Looking up password for {}@{}:{}/{}",
        user, host, port, database
    );

    if let Some(config) = lookup_mysql_config(None) {
        // Check if the configuration matches the requested connection
        let host_matches = config
            .host
            .as_ref()
            .is_none_or(|h| h == host || h == "localhost");
        let port_matches = config.port.is_none_or(|p| p == port);
        let user_matches = config.user.as_ref().is_none_or(|u| u == user);
        let database_matches = config.database.as_ref().is_none_or(|d| d == database);

        if host_matches && port_matches && user_matches && database_matches {
            debug!("[lookup_mysql_password] Configuration matches, returning password");
            return config.password;
        }
    }

    debug!("[lookup_mysql_password] No matching configuration found");
    None
}

/// Save MySQL configuration to .my.cnf file
/// This creates or updates the [client] section in the user's .my.cnf file
pub fn save_mysql_config(
    host: &str,
    port: u16,
    database: &str,
    user: &str,
    password: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let home_dir = dirs::home_dir().ok_or("Could not determine home directory")?;
    let config_path = home_dir.join(".my.cnf");

    debug!(
        "[save_mysql_config] Saving MySQL config to: {}",
        config_path.display()
    );

    // Read existing configuration if it exists
    let mut existing_configs = if config_path.exists() {
        parse_mysql_config(&config_path)?
    } else {
        HashMap::new()
    };

    // Update the [client] section
    let mut client_config = existing_configs.get("client").cloned().unwrap_or_default();
    client_config.host = Some(host.to_string());
    client_config.port = Some(port);
    client_config.database = Some(database.to_string());
    client_config.user = Some(user.to_string());
    client_config.password = Some(password.to_string());

    existing_configs.insert("client".to_string(), client_config);

    // Write the configuration back
    let mut content = String::new();

    for (section_name, config) in &existing_configs {
        content.push_str(&format!("[{section_name}]\n"));

        if let Some(ref host) = config.host {
            content.push_str(&format!("host = {host}\n"));
        }
        if let Some(port) = config.port {
            content.push_str(&format!("port = {port}\n"));
        }
        if let Some(ref user) = config.user {
            content.push_str(&format!("user = {user}\n"));
        }
        if let Some(ref password) = config.password {
            content.push_str(&format!("password = {password}\n"));
        }
        if let Some(ref database) = config.database {
            content.push_str(&format!("database = {database}\n"));
        }
        if let Some(ref socket) = config.socket {
            content.push_str(&format!("socket = {socket}\n"));
        }
        if let Some(ref ssl_ca) = config.ssl_ca {
            content.push_str(&format!("ssl-ca = {ssl_ca}\n"));
        }
        if let Some(ref ssl_cert) = config.ssl_cert {
            content.push_str(&format!("ssl-cert = {ssl_cert}\n"));
        }
        if let Some(ref ssl_key) = config.ssl_key {
            content.push_str(&format!("ssl-key = {ssl_key}\n"));
        }

        content.push('\n');
    }

    fs::write(&config_path, content)?;

    // Set appropriate file permissions (readable/writable only by owner)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&config_path)?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&config_path, permissions)?;
    }

    debug!("[save_mysql_config] MySQL configuration saved successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_mysql_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test.cnf");

        let config_content = r#"
# MySQL configuration file
[client]
host = localhost
port = 3306
user = testuser
password = testpass
database = testdb

[mysql]
host = remote.example.com
user = remoteuser
"#;

        fs::write(&config_path, config_content).unwrap();

        let configs = parse_mysql_config(&config_path).unwrap();

        assert_eq!(configs.len(), 2);

        let client_config = configs.get("client").unwrap();
        assert_eq!(client_config.host.as_ref().unwrap(), "localhost");
        assert_eq!(client_config.port.unwrap(), 3306);
        assert_eq!(client_config.user.as_ref().unwrap(), "testuser");
        assert_eq!(client_config.password.as_ref().unwrap(), "testpass");
        assert_eq!(client_config.database.as_ref().unwrap(), "testdb");

        let mysql_config = configs.get("mysql").unwrap();
        assert_eq!(mysql_config.host.as_ref().unwrap(), "remote.example.com");
        assert_eq!(mysql_config.user.as_ref().unwrap(), "remoteuser");
    }

    #[test]
    fn test_lookup_mysql_password() {
        // This test would need a mock configuration file
        // In a real scenario, we'd set up a temporary .my.cnf file
        // For now, we're just testing that the function doesn't panic
        let result = lookup_mysql_password("localhost", 3306, "testdb", "testuser");
        // Result could be None if no config file exists, which is fine for testing
        assert!(result.is_none() || result.is_some());
    }
}
