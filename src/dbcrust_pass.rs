//! Universal password file (.dbcrust) support for all database types
//!
//! This module provides functionality to read and write database credentials
//! in a universal format that works with all supported database types.
//! Format: database_type:host:port:database:username:password

use crate::password_encryption::{
    PasswordEncryptionError, decrypt_password, encrypt_password, is_encrypted,
};
use std::env;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbcrustPassError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Password encryption error: {0}")]
    Encryption(#[from] PasswordEncryptionError),
    #[error("Invalid entry format: {0}")]
    InvalidFormat(String),
    #[error("Permission error: .dbcrust file must have 0600 permissions")]
    PermissionError,
}

/// Database type identifier for .dbcrust entries
#[derive(Debug, Clone, PartialEq)]
pub enum DatabaseType {
    PostgreSQL,
    MySQL,
    MongoDB,
    Elasticsearch,
    ClickHouse,
    SQLite,
}

impl DatabaseType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "postgresql" | "postgres" | "pg" => Some(DatabaseType::PostgreSQL),
            "mysql" | "mariadb" => Some(DatabaseType::MySQL),
            "mongodb" | "mongo" => Some(DatabaseType::MongoDB),
            "elasticsearch" | "elastic" => Some(DatabaseType::Elasticsearch),
            "clickhouse" | "ch" => Some(DatabaseType::ClickHouse),
            "sqlite" | "sqlite3" => Some(DatabaseType::SQLite),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DatabaseType::PostgreSQL => "postgresql",
            DatabaseType::MySQL => "mysql",
            DatabaseType::MongoDB => "mongodb",
            DatabaseType::Elasticsearch => "elasticsearch",
            DatabaseType::ClickHouse => "clickhouse",
            DatabaseType::SQLite => "sqlite",
        }
    }
}

/// Represents a parsed entry from the .dbcrust file
#[derive(Debug, Clone)]
pub struct DbcrustPassEntry {
    pub database_type: DatabaseType,
    pub hostname: String,
    pub port: String,
    pub database: String,
    pub username: String,
    pub password: String, // Decrypted password
}

impl DbcrustPassEntry {
    /// Create a new entry
    pub fn new(
        database_type: DatabaseType,
        hostname: String,
        port: String,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self {
            database_type,
            hostname,
            port,
            database,
            username,
            password,
        }
    }

    /// Check if this entry matches the given connection parameters
    /// Supports wildcard matching with "*"
    pub fn matches(
        &self,
        db_type: &DatabaseType,
        host: &str,
        port: u16,
        dbname: &str,
        username: &str,
    ) -> bool {
        // Database type must match exactly
        if self.database_type != *db_type {
            return false;
        }

        // Check hostname (support wildcards)
        if self.hostname != "*" && self.hostname != host {
            return false;
        }

        // Check port (support wildcards)
        if self.port != "*" && self.port != port.to_string() {
            return false;
        }

        // Check database name (support wildcards)
        if self.database != "*" && self.database != dbname {
            return false;
        }

        // Check username (support wildcards)
        if self.username != "*" && self.username != username {
            return false;
        }

        true
    }

    /// Convert to file format line with encrypted password
    pub fn to_file_line(&self, encrypt: bool) -> Result<String, DbcrustPassError> {
        let password = if encrypt && !is_encrypted(&self.password) {
            encrypt_password(&self.password)?
        } else {
            self.password.clone()
        };

        Ok(format!(
            "{}:{}:{}:{}:{}:{}",
            self.database_type.as_str(),
            escape_field(&self.hostname),
            escape_field(&self.port),
            escape_field(&self.database),
            escape_field(&self.username),
            escape_field(&password)
        ))
    }
}

/// Gets the path to the .dbcrust file
pub fn get_dbcrust_pass_path() -> Option<PathBuf> {
    // Check environment variable first
    if let Ok(passfile) = env::var("DBCRUST_PASSFILE") {
        return Some(PathBuf::from(passfile));
    }

    // Default to ~/.dbcrust
    dirs::home_dir().map(|home| home.join(".dbcrust"))
}

/// Check if .dbcrust file has correct permissions (Unix only)
fn has_correct_permissions(path: &Path) -> bool {
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let permissions = metadata.permissions();
            let mode = permissions.mode();
            // Check if file permissions are 0600 (only user can read/write)
            return (mode & 0o077) == 0;
        }
        false
    }

    #[cfg(target_family = "windows")]
    {
        true // Windows relies on directory security
    }
}

/// Set correct permissions for .dbcrust file (Unix only)
fn set_correct_permissions(path: &Path) -> Result<(), DbcrustPassError> {
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600); // User readable/writable only
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// Parse a single line from the .dbcrust file
fn parse_dbcrust_line(line: &str) -> Result<Option<DbcrustPassEntry>, DbcrustPassError> {
    // Skip comments and empty lines
    if line.starts_with('#') || line.trim().is_empty() {
        return Ok(None);
    }

    // Split by colons, handling escaped colons
    let fields = parse_fields(line);

    // Ensure we have 6 fields: database_type:host:port:database:username:password
    if fields.len() != 6 {
        return Err(DbcrustPassError::InvalidFormat(format!(
            "Expected 6 fields (database_type:host:port:database:username:password), got {}",
            fields.len()
        )));
    }

    let database_type = DatabaseType::from_str(&fields[0]).ok_or_else(|| {
        DbcrustPassError::InvalidFormat(format!("Invalid database type: {}", fields[0]))
    })?;

    // Decrypt password if encrypted
    let password = decrypt_password(&fields[5])?;

    Ok(Some(DbcrustPassEntry {
        database_type,
        hostname: fields[1].clone(),
        port: fields[2].clone(),
        database: fields[3].clone(),
        username: fields[4].clone(),
        password,
    }))
}

/// Parse fields from a line, handling escaped colons and backslashes
fn parse_fields(line: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let mut current_field = String::new();
    let mut escaping = false;

    for c in line.chars() {
        if escaping {
            current_field.push(c);
            escaping = false;
        } else if c == '\\' {
            escaping = true;
        } else if c == ':' {
            fields.push(current_field);
            current_field = String::new();
        } else {
            current_field.push(c);
        }
    }

    // Add the final field
    fields.push(current_field);
    fields
}

/// Escape colons and backslashes in .dbcrust fields
fn escape_field(field: &str) -> String {
    field.replace('\\', "\\\\").replace(':', "\\:")
}

/// Read the .dbcrust file and find a matching password
pub fn lookup_password(
    db_type: DatabaseType,
    host: &str,
    port: u16,
    dbname: &str,
    username: &str,
) -> Result<Option<String>, DbcrustPassError> {
    let dbcrust_path = match get_dbcrust_pass_path() {
        Some(path) => path,
        None => return Ok(None),
    };

    // Skip if file doesn't exist
    if !dbcrust_path.exists() {
        return Ok(None);
    }

    // Check file permissions on Unix
    if !has_correct_permissions(&dbcrust_path) {
        eprintln!(
            "Warning: .dbcrust file has incorrect permissions. It should be 0600 (readable/writable only by owner)."
        );
        return Err(DbcrustPassError::PermissionError);
    }

    // Open and read the file
    let file = File::open(&dbcrust_path)?;
    let reader = BufReader::new(file);

    // Process each line
    for line in reader.lines() {
        let line = line?;

        if let Some(entry) = parse_dbcrust_line(&line)? {
            // Check if entry matches the connection parameters
            if entry.matches(&db_type, host, port, dbname, username) {
                return Ok(Some(entry.password));
            }
        }
    }

    Ok(None)
}

/// Save a password entry to the .dbcrust file
/// If an entry with matching parameters already exists, it will be updated
pub fn save_password(
    db_type: DatabaseType,
    host: &str,
    port: u16,
    dbname: &str,
    username: &str,
    password: &str,
    encrypt: bool,
) -> Result<(), DbcrustPassError> {
    let dbcrust_path = get_dbcrust_pass_path().ok_or_else(|| {
        DbcrustPassError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine .dbcrust file location",
        ))
    })?;

    // Create file and parent directories if they don't exist
    if let Some(parent) = dbcrust_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    let new_entry = DbcrustPassEntry::new(
        db_type,
        host.to_string(),
        port.to_string(),
        dbname.to_string(),
        username.to_string(),
        password.to_string(),
    );

    // Read existing entries if file exists
    let mut entries = Vec::new();
    let mut entry_updated = false;

    if dbcrust_path.exists() {
        let file = File::open(&dbcrust_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;

            // Skip comments and empty lines - preserve them
            if line.starts_with('#') || line.trim().is_empty() {
                entries.push(line);
                continue;
            }

            if let Some(existing_entry) = parse_dbcrust_line(&line)? {
                // Check if this entry matches what we're trying to save
                if existing_entry.database_type == new_entry.database_type
                    && existing_entry.hostname == new_entry.hostname
                    && existing_entry.port == new_entry.port
                    && existing_entry.database == new_entry.database
                    && existing_entry.username == new_entry.username
                {
                    // Update with new password
                    entries.push(new_entry.to_file_line(encrypt)?);
                    entry_updated = true;
                } else {
                    // Keep the existing entry
                    entries.push(line);
                }
            } else {
                // Keep other lines (invalid entries, etc.)
                entries.push(line);
            }
        }
    }

    // If we didn't update an existing entry, add a new one
    if !entry_updated {
        entries.push(new_entry.to_file_line(encrypt)?);
    }

    // Write entries back to file
    let mut file = File::create(&dbcrust_path)?;
    for entry in entries {
        writeln!(file, "{}", entry)?;
    }

    // Set correct permissions
    set_correct_permissions(&dbcrust_path)?;

    Ok(())
}

/// List all entries in the .dbcrust file (without passwords)
pub fn list_entries()
-> Result<Vec<(DatabaseType, String, String, String, String)>, DbcrustPassError> {
    let dbcrust_path = match get_dbcrust_pass_path() {
        Some(path) => path,
        None => return Ok(vec![]),
    };

    if !dbcrust_path.exists() {
        return Ok(vec![]);
    }

    if !has_correct_permissions(&dbcrust_path) {
        return Err(DbcrustPassError::PermissionError);
    }

    let file = File::open(&dbcrust_path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = line?;

        if let Some(entry) = parse_dbcrust_line(&line)? {
            entries.push((
                entry.database_type,
                entry.hostname,
                entry.port,
                entry.database,
                entry.username,
            ));
        }
    }

    Ok(entries)
}

/// Delete a password entry from the .dbcrust file
pub fn delete_password(
    db_type: DatabaseType,
    host: &str,
    port: u16,
    dbname: &str,
    username: &str,
) -> Result<bool, DbcrustPassError> {
    let dbcrust_path = match get_dbcrust_pass_path() {
        Some(path) => path,
        None => return Ok(false),
    };

    if !dbcrust_path.exists() {
        return Ok(false);
    }

    if !has_correct_permissions(&dbcrust_path) {
        return Err(DbcrustPassError::PermissionError);
    }

    let file = File::open(&dbcrust_path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut entry_deleted = false;

    for line in reader.lines() {
        let line = line?;

        // Skip comments and empty lines - preserve them
        if line.starts_with('#') || line.trim().is_empty() {
            entries.push(line);
            continue;
        }

        if let Some(existing_entry) = parse_dbcrust_line(&line)? {
            // Check if this entry should be deleted
            if existing_entry.matches(&db_type, host, port, dbname, username) {
                entry_deleted = true;
                // Skip this line (delete it)
            } else {
                // Keep the existing entry
                entries.push(line);
            }
        } else {
            // Keep other lines (invalid entries, etc.)
            entries.push(line);
        }
    }

    if entry_deleted {
        // Write entries back to file
        let mut file = File::create(&dbcrust_path)?;
        for entry in entries {
            writeln!(file, "{}", entry)?;
        }
        set_correct_permissions(&dbcrust_path)?;
    }

    Ok(entry_deleted)
}

/// Convert all plaintext passwords in .dbcrust file to encrypted format
pub fn encrypt_all_passwords() -> Result<usize, DbcrustPassError> {
    let dbcrust_path = match get_dbcrust_pass_path() {
        Some(path) => path,
        None => return Ok(0),
    };

    if !dbcrust_path.exists() {
        return Ok(0);
    }

    if !has_correct_permissions(&dbcrust_path) {
        return Err(DbcrustPassError::PermissionError);
    }

    let file = File::open(&dbcrust_path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut encrypted_count = 0;

    for line in reader.lines() {
        let line = line?;

        // Skip comments and empty lines - preserve them
        if line.starts_with('#') || line.trim().is_empty() {
            entries.push(line);
            continue;
        }

        if let Some(mut entry) = parse_dbcrust_line(&line)? {
            if !is_encrypted(&entry.password) {
                // Password is plaintext, encrypt it
                entry.password = encrypt_password(&entry.password)?;
                entries.push(entry.to_file_line(false)?); // false because password is already encrypted
                encrypted_count += 1;
            } else {
                // Already encrypted
                entries.push(line);
            }
        } else {
            // Keep other lines (invalid entries, etc.)
            entries.push(line);
        }
    }

    if encrypted_count > 0 {
        // Write entries back to file
        let mut file = File::create(&dbcrust_path)?;
        for entry in entries {
            writeln!(file, "{}", entry)?;
        }
        set_correct_permissions(&dbcrust_path)?;
    }

    Ok(encrypted_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::NamedTempFile;

    // Global mutex to ensure dbcrust pass tests don't interfere with each other
    static DBCRUST_PASS_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_database_type_conversion() {
        assert_eq!(
            DatabaseType::from_str("postgresql"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DatabaseType::from_str("postgres"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(DatabaseType::from_str("pg"), Some(DatabaseType::PostgreSQL));
        assert_eq!(DatabaseType::from_str("mysql"), Some(DatabaseType::MySQL));
        assert_eq!(
            DatabaseType::from_str("mongodb"),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(DatabaseType::from_str("invalid"), None);

        assert_eq!(DatabaseType::PostgreSQL.as_str(), "postgresql");
        assert_eq!(DatabaseType::MySQL.as_str(), "mysql");
    }

    #[test]
    fn test_entry_matching() {
        let entry = DbcrustPassEntry::new(
            DatabaseType::PostgreSQL,
            "localhost".to_string(),
            "5432".to_string(),
            "mydb".to_string(),
            "user".to_string(),
            "password".to_string(),
        );

        // Exact match
        assert!(entry.matches(&DatabaseType::PostgreSQL, "localhost", 5432, "mydb", "user"));

        // Different database type
        assert!(!entry.matches(&DatabaseType::MySQL, "localhost", 5432, "mydb", "user"));

        // Different parameters
        assert!(!entry.matches(&DatabaseType::PostgreSQL, "otherhost", 5432, "mydb", "user"));
        assert!(!entry.matches(&DatabaseType::PostgreSQL, "localhost", 3306, "mydb", "user"));
    }

    #[test]
    fn test_wildcard_matching() {
        let entry = DbcrustPassEntry::new(
            DatabaseType::PostgreSQL,
            "*".to_string(),
            "*".to_string(),
            "mydb".to_string(),
            "*".to_string(),
            "password".to_string(),
        );

        // Should match any host, port, and user
        assert!(entry.matches(&DatabaseType::PostgreSQL, "localhost", 5432, "mydb", "user"));
        assert!(entry.matches(&DatabaseType::PostgreSQL, "remote", 9999, "mydb", "admin"));

        // Should not match different database type or database name
        assert!(!entry.matches(&DatabaseType::MySQL, "localhost", 5432, "mydb", "user"));
        assert!(!entry.matches(
            &DatabaseType::PostgreSQL,
            "localhost",
            5432,
            "otherdb",
            "user"
        ));
    }

    #[test]
    fn test_parse_fields() {
        // Normal fields
        let fields = parse_fields("a:b:c:d:e:f");
        assert_eq!(fields, vec!["a", "b", "c", "d", "e", "f"]);

        // Fields with escaped colons
        let fields = parse_fields("a\\:b:c:d:e:f");
        assert_eq!(fields, vec!["a:b", "c", "d", "e", "f"]);

        // Fields with escaped backslashes
        let fields = parse_fields("a\\\\:b:c:d:e:f");
        assert_eq!(fields, vec!["a\\", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn test_escape_field() {
        assert_eq!(escape_field("normal"), "normal");
        assert_eq!(escape_field("with:colon"), "with\\:colon");
        assert_eq!(escape_field("with\\backslash"), "with\\\\backslash");
        assert_eq!(escape_field("both:and\\"), "both\\:and\\\\");
    }

    #[test]
    fn test_parse_dbcrust_line() {
        let _guard = DBCRUST_PASS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Valid line
        let result = parse_dbcrust_line("postgresql:localhost:5432:mydb:user:password");
        assert!(result.is_ok());
        let entry = result.unwrap().unwrap();
        assert_eq!(entry.database_type, DatabaseType::PostgreSQL);
        assert_eq!(entry.hostname, "localhost");
        assert_eq!(entry.port, "5432");
        assert_eq!(entry.database, "mydb");
        assert_eq!(entry.username, "user");
        assert_eq!(entry.password, "password");

        // Comment line
        let result = parse_dbcrust_line("# This is a comment");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Empty line
        let result = parse_dbcrust_line("");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Invalid format (too few fields)
        let result = parse_dbcrust_line("postgresql:localhost:5432");
        assert!(result.is_err());

        // Invalid database type
        let result = parse_dbcrust_line("invalid:localhost:5432:mydb:user:password");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_and_lookup_password() {
        let _guard = DBCRUST_PASS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Create a temporary file
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_path_buf();

        // Override environment variable for testing
        let original_env = env::var_os("DBCRUST_PASSFILE");
        unsafe {
            env::set_var("DBCRUST_PASSFILE", &temp_path);
        }

        // Save a password
        let result = save_password(
            DatabaseType::PostgreSQL,
            "testhost",
            5432,
            "testdb",
            "testuser",
            "testpass",
            false, // Don't encrypt for this test
        );
        assert!(result.is_ok());

        // Lookup the password
        let result = lookup_password(
            DatabaseType::PostgreSQL,
            "testhost",
            5432,
            "testdb",
            "testuser",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some("testpass".to_string()));

        // Lookup non-existent password
        let result = lookup_password(
            DatabaseType::PostgreSQL,
            "wronghost",
            5432,
            "testdb",
            "testuser",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);

        // Restore environment
        match original_env {
            Some(val) => unsafe { env::set_var("DBCRUST_PASSFILE", val) },
            None => unsafe { env::remove_var("DBCRUST_PASSFILE") },
        }
    }
}
