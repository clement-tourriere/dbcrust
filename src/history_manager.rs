use reedline::{FileBackedHistory, History};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use tracing::{debug, warn};

use crate::config::Config;
use crate::database::{ConnectionInfo, DatabaseTypeExt};
use crate::db::Database;

/// Session identifier for generating unique history files
#[derive(Debug, Clone, PartialEq)]
pub struct SessionId {
    pub identifier: String,
    pub display_name: String,
}

impl SessionId {
    /// Generate a session ID from connection information
    pub fn from_connection_info(connection_info: &ConnectionInfo) -> Self {
        let identifier = if connection_info.database_type.is_file_based() {
            if let Some(file_path) = &connection_info.file_path {
                format!("sqlite:{file_path}")
            } else {
                "sqlite:memory".to_string()
            }
        } else {
            let host = connection_info.host.as_deref().unwrap_or("localhost");
            let port = connection_info
                .port
                .unwrap_or_else(|| connection_info.database_type.default_port().unwrap_or(0));
            let username = connection_info.username.as_deref().unwrap_or("unknown");
            let database = connection_info.database.as_deref().unwrap_or("unknown");

            // Check for special cases
            if let Some(container) = &connection_info.docker_container {
                format!("docker:{container}:{database}")
            } else if let (Some(vault_mount), Some(vault_database), Some(vault_role)) = (
                connection_info.options.get("vault_mount"),
                connection_info.options.get("vault_database"),
                connection_info.options.get("vault_role"),
            ) {
                format!("vault:{vault_mount}:{vault_database}:{vault_role}")
            } else {
                format!(
                    "{}:{}:{}:{}:{}",
                    connection_info.database_type.display_name(),
                    host,
                    port,
                    username,
                    database
                )
            }
        };

        let display_name = if connection_info.database_type.is_file_based() {
            if let Some(file_path) = &connection_info.file_path {
                format!("sqlite:{file_path}")
            } else {
                "sqlite:memory".to_string()
            }
        } else {
            let host = connection_info.host.as_deref().unwrap_or("localhost");
            let port = connection_info
                .port
                .unwrap_or_else(|| connection_info.database_type.default_port().unwrap_or(0));
            let username = connection_info.username.as_deref().unwrap_or("unknown");
            let database = connection_info.database.as_deref().unwrap_or("unknown");

            if let Some(container) = &connection_info.docker_container {
                format!("{username}@docker:{container}/{database}")
            } else if let (Some(vault_mount), Some(vault_database), Some(_vault_role)) = (
                connection_info.options.get("vault_mount"),
                connection_info.options.get("vault_database"),
                connection_info.options.get("vault_role"),
            ) {
                format!("{username}@vault:{vault_mount}/{vault_database}")
            } else {
                format!("{username}@{host}:{port}/{database}")
            }
        };

        Self {
            identifier,
            display_name,
        }
    }

    /// Generate a session ID from Database instance
    pub fn from_database(database: &Database) -> Option<Self> {
        database
            .get_connection_info()
            .map(Self::from_connection_info)
    }

    /// Generate file-safe hash for the session identifier
    pub fn to_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.identifier.as_bytes());
        let result = hasher.finalize();
        format!("{result:x}")[..16].to_string() // Use first 16 chars for brevity
    }

    /// Get the history filename for this session
    pub fn history_filename(&self) -> String {
        format!("history_{}", self.to_hash())
    }
}

/// Manages per-session command histories
pub struct SessionHistoryManager {
    config_dir: PathBuf,
    per_session_enabled: bool,
    max_history_files: usize,
    cleanup_after_days: u64,
    /// Cache of loaded history instances
    history_cache: HashMap<String, Box<dyn History>>,
}

impl SessionHistoryManager {
    /// Create a new session history manager
    pub fn new(config: &Config) -> Result<Self, Box<dyn std::error::Error>> {
        let config_dir = Config::get_config_dir()?;

        Ok(Self {
            config_dir,
            per_session_enabled: config.history.per_session_enabled,
            max_history_files: config.history.max_history_files,
            cleanup_after_days: config.history.cleanup_after_days,
            history_cache: HashMap::new(),
        })
    }

    /// Get or create history for a specific session
    pub fn get_session_history(&mut self, session_id: &SessionId) -> Box<dyn History> {
        if !self.per_session_enabled {
            debug!("Per-session history disabled, using default history");
            return self.get_default_history();
        }

        let history_filename = session_id.history_filename();

        // Check cache first
        if let Some(cached_history) = self.history_cache.remove(&history_filename) {
            debug!(
                "Using cached history for session: {}",
                session_id.display_name
            );
            return cached_history;
        }

        let history_path = self.config_dir.join(&history_filename);
        debug!(
            "Creating history for session '{}' at path: {:?}",
            session_id.display_name, history_path
        );

        Box::new(
            FileBackedHistory::with_file(50, history_path).unwrap_or_else(|e| {
                warn!(
                    "Failed to create session history file: {}, using default",
                    e
                );
                FileBackedHistory::default()
            }),
        )
    }

    /// Get default/fallback history (single shared history)
    pub fn get_default_history(&self) -> Box<dyn History> {
        let history_path = self.config_dir.join("history");
        debug!("Using default history at path: {:?}", history_path);

        Box::new(
            FileBackedHistory::with_file(50, history_path).unwrap_or_else(|e| {
                warn!(
                    "Failed to create default history file: {}, using in-memory",
                    e
                );
                FileBackedHistory::default()
            }),
        )
    }

    /// List all session histories with metadata
    pub fn list_session_histories(
        &self,
    ) -> Result<Vec<SessionHistoryInfo>, Box<dyn std::error::Error>> {
        let mut histories = Vec::new();

        if !self.config_dir.exists() {
            return Ok(histories);
        }

        for entry in fs::read_dir(&self.config_dir)? {
            let entry = entry?;
            let path = entry.path();

            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.starts_with("history_") && filename != "history" {
                    let metadata = entry.metadata()?;
                    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let age_days = SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default()
                        .as_secs()
                        / 86400;

                    // Try to estimate entry count by file size (rough heuristic)
                    let estimated_entries = (metadata.len() / 50) as usize; // ~50 bytes per entry average

                    histories.push(SessionHistoryInfo {
                        filename: filename.to_string(),
                        session_hash: filename.strip_prefix("history_").unwrap_or("").to_string(),
                        path: path.clone(),
                        last_modified: modified,
                        age_days,
                        estimated_entries,
                        file_size: metadata.len(),
                    });
                }
            }
        }

        // Sort by last modified (most recent first)
        histories.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));

        Ok(histories)
    }

    /// Clean up old unused history files
    pub fn cleanup_old_histories(&self) -> Result<usize, Box<dyn std::error::Error>> {
        let histories = self.list_session_histories()?;
        let mut cleaned_count = 0;

        // Clean up files older than the configured threshold
        for history in &histories {
            if history.age_days > self.cleanup_after_days {
                debug!(
                    "Cleaning up old history file: {} (age: {} days)",
                    history.filename, history.age_days
                );
                if let Err(e) = fs::remove_file(&history.path) {
                    warn!(
                        "Failed to remove old history file {}: {}",
                        history.filename, e
                    );
                } else {
                    cleaned_count += 1;
                }
            }
        }

        // If we still have too many files, remove the oldest ones
        let remaining_histories: Vec<_> = histories
            .into_iter()
            .filter(|h| h.age_days <= self.cleanup_after_days)
            .collect();

        if remaining_histories.len() > self.max_history_files {
            let excess_count = remaining_histories.len() - self.max_history_files;
            for history in remaining_histories.iter().rev().take(excess_count) {
                debug!("Cleaning up excess history file: {}", history.filename);
                if let Err(e) = fs::remove_file(&history.path) {
                    warn!(
                        "Failed to remove excess history file {}: {}",
                        history.filename, e
                    );
                } else {
                    cleaned_count += 1;
                }
            }
        }

        if cleaned_count > 0 {
            debug!("Cleaned up {} old history files", cleaned_count);
        }

        Ok(cleaned_count)
    }

    /// Migrate from single history file to per-session history
    pub fn migrate_single_history_to_default(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let old_history_path = self.config_dir.join("history");
        let default_history_path = self.config_dir.join("history_default");

        if old_history_path.exists() && !default_history_path.exists() {
            debug!("Migrating single history file to default session history");
            fs::rename(&old_history_path, &default_history_path)?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Check if per-session history is enabled
    pub fn is_per_session_enabled(&self) -> bool {
        self.per_session_enabled
    }
}

/// Information about a session history file
#[derive(Debug, Clone)]
pub struct SessionHistoryInfo {
    pub filename: String,
    pub session_hash: String,
    pub path: PathBuf,
    pub last_modified: SystemTime,
    pub age_days: u64,
    pub estimated_entries: usize,
    pub file_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{ConnectionInfo, DatabaseType};
    use std::collections::HashMap;

    #[test]
    fn test_session_id_postgresql() {
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("user".to_string()),
            password: None,
            database: Some("mydb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let session_id = SessionId::from_connection_info(&connection_info);
        assert_eq!(session_id.identifier, "PostgreSQL:localhost:5432:user:mydb");
        assert_eq!(session_id.display_name, "user@localhost:5432/mydb");
        assert!(!session_id.to_hash().is_empty());
        assert_eq!(
            session_id.history_filename(),
            format!("history_{}", session_id.to_hash())
        );
    }

    #[test]
    fn test_session_id_sqlite() {
        let connection_info = ConnectionInfo {
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

        let session_id = SessionId::from_connection_info(&connection_info);
        assert_eq!(session_id.identifier, "sqlite:/path/to/db.sqlite");
        assert_eq!(session_id.display_name, "sqlite:/path/to/db.sqlite");
    }

    #[test]
    fn test_session_id_docker() {
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("user".to_string()),
            password: None,
            database: Some("mydb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: Some("my-postgres-container".to_string()),
        };

        let session_id = SessionId::from_connection_info(&connection_info);
        assert_eq!(session_id.identifier, "docker:my-postgres-container:mydb");
        assert_eq!(
            session_id.display_name,
            "user@docker:my-postgres-container/mydb"
        );
    }

    #[test]
    fn test_session_id_vault() {
        let mut connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("vault-postgres.internal".to_string()),
            port: Some(5432),
            username: Some("v-root-readonly-xyz".to_string()),
            password: None,
            database: Some("myapp".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        connection_info
            .options
            .insert("vault_mount".to_string(), "database".to_string());
        connection_info
            .options
            .insert("vault_database".to_string(), "myapp-prod".to_string());
        connection_info
            .options
            .insert("vault_role".to_string(), "readonly".to_string());

        let session_id = SessionId::from_connection_info(&connection_info);
        assert_eq!(session_id.identifier, "vault:database:myapp-prod:readonly");
        assert_eq!(
            session_id.display_name,
            "v-root-readonly-xyz@vault:database/myapp-prod"
        );
    }

    #[test]
    fn test_session_id_hash_stability() {
        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("user".to_string()),
            password: None,
            database: Some("mydb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        let session_id1 = SessionId::from_connection_info(&connection_info);
        let session_id2 = SessionId::from_connection_info(&connection_info);

        assert_eq!(session_id1.to_hash(), session_id2.to_hash());
        assert_eq!(
            session_id1.history_filename(),
            session_id2.history_filename()
        );
    }
}
