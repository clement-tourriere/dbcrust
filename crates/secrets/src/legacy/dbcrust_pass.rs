//! Legacy universal password file (`~/.dbcrust`) reader.
//!
//! Format: `database_type:host:port:database:username:password`, optionally
//! with an `enc:` machine-key value from older versions.
//!
//! **Read, list, delete and migrate only.** New passwords are written to the
//! secret store (`dbcrust_secrets`), never to this file, and nothing in this
//! module creates `enc:` values any more. Lookups here are the fallback
//! after the store (see `config::lookup_connection_password`) and
//! `dbcrust secrets migrate` moves what is found into the store.

use crate::legacy::password_encryption::{PasswordEncryptionError, legacy_decrypt};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
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
#[derive(Clone)]
pub struct DbcrustPassEntry {
    pub database_type: DatabaseType,
    pub hostname: String,
    pub port: String,
    pub database: String,
    pub username: String,
    pub password: String, // Decrypted password
}

impl std::fmt::Debug for DbcrustPassEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbcrustPassEntry")
            .field("database_type", &self.database_type)
            .field("hostname", &self.hostname)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
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

    /// Render as a file line (the password verbatim, escaped).
    pub fn to_file_line(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.database_type.as_str(),
            escape_field(&self.hostname),
            escape_field(&self.port),
            escape_field(&self.database),
            escape_field(&self.username),
            escape_field(&self.password)
        )
    }

    /// True when any of host/port/database/user is the `*` wildcard.
    pub fn is_wildcard(&self) -> bool {
        [&self.hostname, &self.port, &self.database, &self.username]
            .iter()
            .any(|field| field.as_str() == "*")
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

    // Decrypt a legacy `enc:` value (plaintext passes through)
    let password = legacy_decrypt(&fields[5])?;

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

        // One unreadable line (foreign `enc:` value, typo) must not hide
        // the entries after it.
        if let Ok(Some(entry)) = parse_dbcrust_line(&line) {
            // Check if entry matches the connection parameters
            if entry.matches(&db_type, host, port, dbname, username) {
                return Ok(Some(entry.password));
            }
        }
    }

    Ok(None)
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

        if let Ok(Some(entry)) = parse_dbcrust_line(&line) {
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
        write_lines_atomically(&dbcrust_path, &entries)?;
    }

    Ok(entry_deleted)
}

fn write_lines_atomically(path: &Path, lines: &[String]) -> Result<(), DbcrustPassError> {
    let mut content = lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    crate::fsutil::write_private_atomic(path, content.as_bytes())
        .map_err(|e| DbcrustPassError::Io(std::io::Error::other(e.to_string())))
}

/// How one line of a legacy `.dbcrust` file reads.
#[derive(Debug, Clone)]
pub enum LegacyLineKind {
    Blank,
    Comment,
    /// A well-formed entry; `password` is decrypted when it was `enc:`.
    Entry {
        entry: DbcrustPassEntry,
        /// The stored password carried the legacy `enc:` prefix.
        was_encrypted: bool,
    },
    /// A line that could not be used (wrong shape, foreign `enc:` value…).
    /// The reason never contains the line's content.
    Unreadable(String),
}

/// One line of the legacy file with its raw text (for exact removal).
#[derive(Debug, Clone)]
pub struct LegacyLine {
    /// 1-based line number.
    pub number: usize,
    pub raw: String,
    pub kind: LegacyLineKind,
}

/// Largest legacy file DBCrust will read.
const MAX_LEGACY_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Read every line of the legacy password file at `path`, never following a
/// symlink and without any permission-based refusal (audit reports modes).
/// An absent file is an empty list.
pub fn read_legacy_lines(path: &Path) -> Result<Vec<LegacyLine>, DbcrustPassError> {
    let Some(bytes) = crate::fsutil::read_bounded(path, MAX_LEGACY_FILE_BYTES)
        .map_err(|e| DbcrustPassError::Io(std::io::Error::other(e.to_string())))?
    else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let kind = if raw.trim().is_empty() {
            LegacyLineKind::Blank
        } else if raw.starts_with('#') {
            LegacyLineKind::Comment
        } else {
            let fields = parse_fields(raw);
            if fields.len() != 6 {
                LegacyLineKind::Unreadable(format!("expected 6 fields, found {}", fields.len()))
            } else if let Some(database_type) = DatabaseType::from_str(&fields[0]) {
                let was_encrypted =
                    crate::legacy::password_encryption::is_legacy_encrypted(&fields[5]);
                match legacy_decrypt(&fields[5]) {
                    Ok(password) => LegacyLineKind::Entry {
                        entry: DbcrustPassEntry::new(
                            database_type,
                            fields[1].clone(),
                            fields[2].clone(),
                            fields[3].clone(),
                            fields[4].clone(),
                            password,
                        ),
                        was_encrypted,
                    },
                    Err(_) => LegacyLineKind::Unreadable(
                        "encrypted value cannot be decrypted on this machine".to_string(),
                    ),
                }
            } else {
                LegacyLineKind::Unreadable("unknown database type".to_string())
            }
        };
        lines.push(LegacyLine {
            number: index + 1,
            raw: raw.to_string(),
            kind,
        });
    }
    Ok(lines)
}

/// Shape of one legacy line, classified without decrypting anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineShape {
    /// Blank or comment.
    Ignorable,
    /// A well-formed entry holding a plaintext password.
    Plaintext { wildcard: bool },
    /// A well-formed entry holding a machine-key `enc:` value.
    MachineKey { wildcard: bool },
    /// Not a usable entry.
    Malformed,
}

/// Classify every line of the legacy file at `path` (1-based numbers)
/// without decrypting or returning any value. Symlinks are refused.
pub fn scan_line_shapes(path: &Path) -> Result<Vec<(usize, LineShape)>, DbcrustPassError> {
    let Some(bytes) = crate::fsutil::read_bounded(path, MAX_LEGACY_FILE_BYTES)
        .map_err(|e| DbcrustPassError::Io(std::io::Error::other(e.to_string())))?
    else {
        return Ok(Vec::new());
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .lines()
        .enumerate()
        .map(|(index, raw)| {
            let shape = if raw.trim().is_empty() || raw.starts_with('#') {
                LineShape::Ignorable
            } else {
                let fields = parse_fields(raw);
                if fields.len() != 6 || DatabaseType::from_str(&fields[0]).is_none() {
                    LineShape::Malformed
                } else {
                    let wildcard = fields[1..5].iter().any(|field| field == "*");
                    if crate::legacy::password_encryption::is_legacy_encrypted(&fields[5]) {
                        LineShape::MachineKey { wildcard }
                    } else {
                        LineShape::Plaintext { wildcard }
                    }
                }
            };
            (index + 1, shape)
        })
        .collect())
}

/// Rewrite the legacy file at `path` without the lines whose raw text is in
/// `drop`, atomically (0600, symlinks refused). Every other line — comments,
/// unreadable and unmigrated entries — is kept verbatim. Returns how many
/// lines were dropped.
pub fn remove_legacy_lines(path: &Path, drop: &HashSet<String>) -> Result<usize, DbcrustPassError> {
    let Some(bytes) = crate::fsutil::read_bounded(path, MAX_LEGACY_FILE_BYTES)
        .map_err(|e| DbcrustPassError::Io(std::io::Error::other(e.to_string())))?
    else {
        return Ok(0);
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut kept = Vec::new();
    let mut dropped = 0;
    for raw in text.lines() {
        if drop.contains(raw) {
            dropped += 1;
        } else {
            kept.push(raw.to_string());
        }
    }
    if dropped > 0 {
        write_lines_atomically(path, &kept)?;
    }
    Ok(dropped)
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
    fn test_lookup_password_and_unreadable_lines_do_not_hide_entries() {
        let _guard = DBCRUST_PASS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_path_buf();
        let good = DbcrustPassEntry::new(
            DatabaseType::PostgreSQL,
            "testhost".to_string(),
            "5432".to_string(),
            "testdb".to_string(),
            "testuser".to_string(),
            "testpass".to_string(),
        );
        std::fs::write(
            &temp_path,
            format!(
                "# comment\nnot-a-valid-line\npostgresql:h:1:d:u:enc\\:00ff\n{}\n",
                good.to_file_line()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let original_env = env::var_os("DBCRUST_PASSFILE");
        unsafe {
            env::set_var("DBCRUST_PASSFILE", &temp_path);
        }

        let found = lookup_password(
            DatabaseType::PostgreSQL,
            "testhost",
            5432,
            "testdb",
            "testuser",
        );
        assert_eq!(found.unwrap(), Some("testpass".to_string()));
        let missing = lookup_password(
            DatabaseType::PostgreSQL,
            "wronghost",
            5432,
            "testdb",
            "testuser",
        );
        assert_eq!(missing.unwrap(), None);

        match original_env {
            Some(val) => unsafe { env::set_var("DBCRUST_PASSFILE", val) },
            None => unsafe { env::remove_var("DBCRUST_PASSFILE") },
        }
    }

    #[test]
    fn test_read_and_remove_legacy_lines_keep_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dbcrust");
        let legacy = crate::legacy::password_encryption::legacy_encrypt_for_tests("old-pw");
        let encrypted_line = DbcrustPassEntry::new(
            DatabaseType::MySQL,
            "db".to_string(),
            "3306".to_string(),
            "app".to_string(),
            "bob".to_string(),
            legacy,
        )
        .to_file_line();
        let plain_line = "postgresql:*:5432:app:alice:wild";
        let content = format!("# keep me\n{encrypted_line}\n{plain_line}\nbroken line\n\n");
        std::fs::write(&path, content).unwrap();

        let lines = read_legacy_lines(&path).unwrap();
        assert_eq!(lines.len(), 5);
        match &lines[1].kind {
            LegacyLineKind::Entry {
                entry,
                was_encrypted,
            } => {
                assert!(*was_encrypted);
                assert_eq!(entry.password, "old-pw");
                assert!(!entry.is_wildcard());
            }
            other => panic!("unexpected {other:?}"),
        }
        match &lines[2].kind {
            LegacyLineKind::Entry {
                entry,
                was_encrypted,
            } => {
                assert!(!*was_encrypted);
                assert!(entry.is_wildcard());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(lines[3].kind, LegacyLineKind::Unreadable(_)));

        let mut drop = HashSet::new();
        drop.insert(lines[1].raw.clone());
        assert_eq!(remove_legacy_lines(&path, &drop).unwrap(), 1);
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# keep me"));
        assert!(after.contains(plain_line));
        assert!(after.contains("broken line"));
        assert!(!after.contains("mysql:"));
    }

    #[test]
    fn test_legacy_reader_refuses_symlinks() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let real = dir.path().join("real");
            std::fs::write(&real, "postgresql:h:1:d:u:p\n").unwrap();
            let link = dir.path().join(".dbcrust");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(read_legacy_lines(&link).is_err());
        }
    }

    #[test]
    fn test_to_file_line_roundtrip() {
        let _guard = DBCRUST_PASS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let entry = DbcrustPassEntry::new(
            DatabaseType::PostgreSQL,
            "myhost.example.com".to_string(),
            "5432".to_string(),
            "production_db".to_string(),
            "admin_user".to_string(),
            "s3cret_pass!".to_string(),
        );

        // Convert to file line without encryption
        let file_line = entry.to_file_line();

        // Parse it back
        let parsed = parse_dbcrust_line(&file_line).unwrap().unwrap();

        // Verify all fields match
        assert_eq!(parsed.database_type, DatabaseType::PostgreSQL);
        assert_eq!(parsed.hostname, "myhost.example.com");
        assert_eq!(parsed.port, "5432");
        assert_eq!(parsed.database, "production_db");
        assert_eq!(parsed.username, "admin_user");
        assert_eq!(parsed.password, "s3cret_pass!");
    }

    #[test]
    fn test_multiple_database_types() {
        let _guard = DBCRUST_PASS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let test_cases = vec![
            (DatabaseType::MySQL, "mysql-host", "3306", "mysql"),
            (DatabaseType::MongoDB, "mongo-host", "27017", "mongodb"),
            (DatabaseType::ClickHouse, "ch-host", "8123", "clickhouse"),
            (
                DatabaseType::Elasticsearch,
                "es-host",
                "9200",
                "elasticsearch",
            ),
        ];

        for (db_type, host, port, expected_type_str) in test_cases {
            let entry = DbcrustPassEntry::new(
                db_type.clone(),
                host.to_string(),
                port.to_string(),
                "testdb".to_string(),
                "testuser".to_string(),
                "testpass".to_string(),
            );

            // Verify database type string representation
            assert_eq!(entry.database_type.as_str(), expected_type_str);

            // Convert to file line and parse back
            let file_line = entry.to_file_line();
            let parsed = parse_dbcrust_line(&file_line).unwrap().unwrap();

            assert_eq!(parsed.database_type, db_type);
            assert_eq!(parsed.hostname, host);
            assert_eq!(parsed.port, port);
            assert_eq!(parsed.database, "testdb");
            assert_eq!(parsed.username, "testuser");
            assert_eq!(parsed.password, "testpass");

            // Verify matching works for each type
            let port_num: u16 = port.parse().unwrap();
            assert!(
                entry.matches(&db_type, host, port_num, "testdb", "testuser"),
                "Entry should match for database type {}",
                expected_type_str
            );
        }
    }

    #[test]
    fn test_password_with_special_characters() {
        let _guard = DBCRUST_PASS_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        let special_passwords = vec![
            "pass:with:colons",
            "pass\\with\\backslashes",
            "pass:with\\both:special\\chars",
            "p@ss!w0rd#$%^&*()",
            "pass with spaces",
            "pass\twith\ttabs",
            "émojis🔑and🔒unicode",
        ];

        for password in special_passwords {
            let entry = DbcrustPassEntry::new(
                DatabaseType::PostgreSQL,
                "localhost".to_string(),
                "5432".to_string(),
                "testdb".to_string(),
                "testuser".to_string(),
                password.to_string(),
            );

            // Convert to file line (without encryption)
            let file_line = entry.to_file_line();

            // Parse it back
            let parsed = parse_dbcrust_line(&file_line).unwrap().unwrap();

            // The password should survive the roundtrip
            assert_eq!(
                parsed.password, password,
                "Password '{}' should survive roundtrip through file format",
                password
            );
        }
    }
}
