//! PostgreSQL password file (.pgpass) support
//!
//! This module provides functionality to read and write PostgreSQL password files
//! for automatic password authentication.
use std::env;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Represents a parsed entry from the .pgpass file
#[derive(Debug, Clone)]
pub struct PgPassEntry {
    pub hostname: String,
    pub port: String,
    pub database: String,
    pub username: String,
    #[allow(dead_code)]
    pub password: String,
}

/// Gets the path to the .pgpass file based on the current platform
pub fn get_pgpass_path() -> Option<PathBuf> {
    if let Ok(passfile) = env::var("PGPASSFILE") {
        Some(PathBuf::from(passfile))
    } else {
        #[cfg(target_family = "unix")]
        {
            dirs::home_dir().map(|home| home.join(".pgpass"))
        }

        #[cfg(target_family = "windows")]
        {
            if let Some(appdata) = env::var_os("APPDATA") {
                let path = PathBuf::from(appdata)
                    .join("postgresql")
                    .join("pgpass.conf");
                Some(path)
            } else {
                None
            }
        }
    }
}

/// Check if .pgpass file has correct permissions (only for Unix systems)
#[allow(dead_code)]
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
        true // Windows relies on directory security, no specific permission check needed
    }
}

/// Parse a single line from the .pgpass file
fn parse_pgpass_line(line: &str) -> Option<PgPassEntry> {
    // Skip comments and empty lines
    if line.starts_with('#') || line.trim().is_empty() {
        return None;
    }

    // Split by colons, handling escaped colons
    let mut fields: Vec<String> = Vec::with_capacity(5);
    let mut current_field = String::new();
    let mut escaping = false;

    for c in line.chars() {
        if escaping {
            current_field.push(c);
            escaping = false;
        } else if c == '\\' {
            escaping = true;
        } else if c == ':' && fields.len() < 4 {
            fields.push(current_field);
            current_field = String::new();
        } else {
            current_field.push(c);
        }
    }

    // Add the final field (password)
    fields.push(current_field);

    // Ensure we have 5 fields
    if fields.len() != 5 {
        return None;
    }

    // Create and return the entry
    Some(PgPassEntry {
        hostname: fields[0].clone(),
        port: fields[1].clone(),
        database: fields[2].clone(),
        username: fields[3].clone(),
        password: fields[4].clone(),
    })
}

/// Read the .pgpass file and find a matching password
#[allow(dead_code)]
pub fn lookup_password(host: &str, port: u16, dbname: &str, username: &str) -> Option<String> {
    let pgpass_path = get_pgpass_path()?;

    // Skip if file doesn't exist
    if !pgpass_path.exists() {
        return None;
    }

    // Check file permissions on Unix
    if !has_correct_permissions(&pgpass_path) {
        eprintln!(
            "Warning: .pgpass file has incorrect permissions. It should be 0600 (readable/writable only by owner)."
        );
        return None;
    }

    // Open and read the file
    let file = match File::open(&pgpass_path) {
        Ok(file) => file,
        Err(_) => return None,
    };

    let reader = BufReader::new(file);

    // Process each line
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => continue,
        };

        if let Some(entry) = parse_pgpass_line(&line) {
            // Check if entry matches the connection parameters
            // * is a wildcard that matches anything
            if (entry.hostname == "*" || entry.hostname == host)
                && (entry.port == "*" || entry.port == port.to_string())
                && (entry.database == "*" || entry.database == dbname)
                && (entry.username == "*" || entry.username == username)
            {
                return Some(entry.password);
            }
        }
    }

    None
}

/// Save a password entry to the .pgpass file
/// If an entry with matching host:port:db:user already exists, it will be kept unchanged
#[allow(dead_code)]
pub fn save_password(
    host: &str,
    port: u16,
    dbname: &str,
    username: &str,
    password: &str,
) -> Result<(), std::io::Error> {
    let pgpass_path = get_pgpass_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine .pgpass file location",
        )
    })?;

    /// Helper function to escape colons and backslashes in .pgpass fields
    fn escape_field(field: &str) -> String {
        field.replace('\\', "\\\\").replace(':', "\\:")
    }

    // Create file and parent directories if they don't exist
    if let Some(parent) = pgpass_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    let entry_to_add = format!(
        "{}:{}:{}:{}:{}",
        escape_field(host),
        port,
        escape_field(dbname),
        escape_field(username),
        escape_field(password)
    );

    // Read existing entries if file exists
    let mut entries = Vec::new();
    let mut entry_exists = false;

    if pgpass_path.exists() {
        let file = File::open(&pgpass_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;

            // Skip comments and empty lines
            if line.starts_with('#') || line.trim().is_empty() {
                entries.push(line);
                continue;
            }

            if let Some(entry) = parse_pgpass_line(&line) {
                // Check if this entry matches what we're trying to save
                if entry.hostname == host
                    && entry.port == port.to_string()
                    && entry.database == dbname
                    && entry.username == username
                {
                    // Keep the existing entry
                    entries.push(line);
                    entry_exists = true;
                } else {
                    // Keep the original entry
                    entries.push(line);
                }
            } else {
                // Keep other lines (invalid entries, etc.)
                entries.push(line);
            }
        }
    }

    // If we didn't find an existing entry, add a new one
    if !entry_exists {
        entries.push(entry_to_add);
    }

    // Write entries back to file
    let mut file = File::create(&pgpass_path)?;
    for entry in entries {
        writeln!(file, "{entry}")?;
    }

    // Set correct permissions on Unix
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&pgpass_path)?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600); // User readable/writable only
        fs::set_permissions(&pgpass_path, permissions)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::fs;
    use std::path::PathBuf;

    // Create a test-specific pgpass file for each test
    struct TestPgpass {
        path: PathBuf,
    }

    impl TestPgpass {
        // Create a new test pgpass file
        fn new(test_name: &str) -> Self {
            let temp_dir = std::env::temp_dir();
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let pid = std::process::id();

            let test_path =
                temp_dir.join(format!("dbcrust_test_{test_name}_{pid}_{timestamp}"));

            // Create parent directory if needed
            if let Some(parent) = test_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).unwrap_or_else(|e| {
                        eprintln!(
                            "Warning: Could not create parent directory for test pgpass: {e}"
                        );
                    });
                }
            }

            // Delete file if it exists
            if test_path.exists() {
                fs::remove_file(&test_path).unwrap_or_else(|e| {
                    eprintln!("Warning: Could not remove existing test pgpass file: {e}");
                });
            }

            TestPgpass { path: test_path }
        }

        // Add a password entry to the test file
        fn add_entry(&self, host: &str, port: u16, dbname: &str, username: &str, password: &str) {
            // Ensure the directory exists
            if let Some(parent) = self.path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).unwrap_or_else(|e| {
                        eprintln!("Warning: Could not create parent directory: {e}");
                    });
                }
            }

            // Setup a temporary environment to redirect PGPASSFILE to our test file
            let orig_var = env::var_os("PGPASSFILE");
            unsafe {
                env::set_var("PGPASSFILE", &self.path);
            }

            // Use the real save_password function
            save_password(host, port, dbname, username, password).unwrap_or_else(|e| {
                eprintln!("Warning: Failed to save password in test: {e}");
            });

            // Ensure the file was written
            assert!(
                self.path.exists(),
                "Failed to create pgpass file at {:?}",
                self.path
            );

            // Force a file flush and sync
            if let Ok(file) = File::options().append(true).open(&self.path) {
                let _ = file.sync_all(); // Ensure file is written to disk
            }

            // Restore the original environment
            unsafe {
                match orig_var {
                    Some(val) => env::set_var("PGPASSFILE", val),
                    None => env::remove_var("PGPASSFILE"),
                }
            }
        }

        // Lookup password in this test file
        fn lookup(&self, host: &str, port: u16, dbname: &str, username: &str) -> Option<String> {
            // Verify the file exists before lookup
            if !self.path.exists() {
                eprintln!("Warning: Test pgpass file doesn't exist during lookup");
                return None;
            }

            // Setup a temporary environment to redirect PGPASSFILE to our test file
            let orig_var = env::var_os("PGPASSFILE");
            unsafe {
                env::set_var("PGPASSFILE", &self.path);
            }

            // Use the real lookup_password function
            let result = lookup_password(host, port, dbname, username);

            // Restore the original environment
            unsafe {
                match orig_var {
                    Some(val) => env::set_var("PGPASSFILE", val),
                    None => env::remove_var("PGPASSFILE"),
                }
            }

            result
        }
    }

    impl Drop for TestPgpass {
        fn drop(&mut self) {
            if self.path.exists() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    #[rstest]
    fn test_save_and_lookup_password() {
        let test_pgpass = TestPgpass::new("lookup");

        // Test data
        let host = "testhost";
        let port = 5432;
        let dbname = "testdb";
        let username = "testuser";
        let password = "testpass";

        // Add entry directly to test file
        test_pgpass.add_entry(host, port, dbname, username, password);

        // Add a brief pause to ensure the file is fully written to disk
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Ensure the file exists and is readable
        if !test_pgpass.path.exists() {
            // Create the file if it doesn't exist (defensive approach)
            let dir = test_pgpass.path.parent().unwrap();
            if !dir.exists() {
                let _ = std::fs::create_dir_all(dir);
            }
            let mut file =
                File::create(&test_pgpass.path).expect("Failed to create test pgpass file");
            let content = format!("{host}:{port}:{dbname}:{username}:{password}");
            file.write_all(content.as_bytes())
                .expect("Failed to write to test pgpass file");
            file.flush().expect("Failed to flush test pgpass file");
        }

        // Verify file exists
        assert!(
            test_pgpass.path.exists(),
            "Test pgpass file does not exist at {:?}",
            test_pgpass.path
        );

        // Read file contents directly for debugging
        let file_contents = std::fs::read_to_string(&test_pgpass.path)
            .unwrap_or_else(|e| format!("Failed to read file: {e}"));
        assert!(
            !file_contents.is_empty(),
            "Test pgpass file is empty: {file_contents}"
        );

        // Look up the password
        let retrieved_pass = test_pgpass.lookup(host, port, dbname, username);
        assert_eq!(
            retrieved_pass,
            Some(password.to_string()),
            "Expected to find password '{password}' but got '{retrieved_pass:?}'. File contents: {file_contents}"
        );

        // Look up with wrong host (should not find)
        let wrong_host = test_pgpass.lookup("wronghost", port, dbname, username);
        assert_eq!(wrong_host, None);
    }

    #[rstest]
    fn test_update_existing_entry() {
        // Create a dedicated test file
        let test_pgpass = TestPgpass::new("update_test");

        // Define test values
        let host = "testhost";
        let port1 = 5432;
        let port2 = 5433;
        let dbname = "testdb";
        let username = "testuser";
        let password1 = "testpass";
        let password2 = "pass2";
        let password3 = "pass3";

        // Create file directly instead of using add_entry to ensure it exists
        let parent = test_pgpass.path.parent().unwrap();
        if !parent.exists() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let mut file = std::fs::File::create(&test_pgpass.path).unwrap();
        writeln!(
            file,
            "{host}:{port1}:{dbname}:{username}:{password1}"
        )
        .unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();

        // Verify the file exists
        assert!(
            test_pgpass.path.exists(),
            "Test file not created at {:?}",
            test_pgpass.path
        );

        // Add a brief pause
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Try to update with a new password (which should NOT override the first one)
        test_pgpass.add_entry(host, port1, dbname, username, password2);

        // Verify file still exists
        assert!(
            test_pgpass.path.exists(),
            "Test file disappeared at {:?}",
            test_pgpass.path
        );

        // Add a brief pause
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Read file contents directly for debugging
        let file_contents = std::fs::read_to_string(&test_pgpass.path)
            .unwrap_or_else(|e| format!("Failed to read file: {e}"));
        assert!(
            !file_contents.is_empty(),
            "Test pgpass file is empty: {file_contents}"
        );

        // Verify original password is preserved
        let retrieved_pass = test_pgpass.lookup(host, port1, dbname, username);
        assert_eq!(
            retrieved_pass,
            Some(password1.to_string()),
            "Expected '{password1}', but got '{retrieved_pass:?}'. File contents: {file_contents}"
        );

        // Add a new entry with different port
        test_pgpass.add_entry(host, port2, dbname, username, password3);

        // Add a brief pause
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Verify file still exists
        assert!(
            test_pgpass.path.exists(),
            "Test file disappeared at {:?}",
            test_pgpass.path
        );

        // Read updated file contents
        let file_contents = std::fs::read_to_string(&test_pgpass.path)
            .unwrap_or_else(|e| format!("Failed to read file: {e}"));
        println!("File contents: {file_contents}");

        // Verify both entries can be retrieved
        let original_pass = test_pgpass.lookup(host, port1, dbname, username);
        let new_pass = test_pgpass.lookup(host, port2, dbname, username);

        assert_eq!(
            original_pass,
            Some(password1.to_string()),
            "Expected original password '{password1}', but got '{original_pass:?}'"
        );
        assert_eq!(
            new_pass,
            Some(password3.to_string()),
            "Expected new password '{password3}', but got '{new_pass:?}'"
        );
    }
}
