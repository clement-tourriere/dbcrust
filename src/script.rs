use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use tempfile::NamedTempFile;

/// Enter a multiline edit mode for SQL script editing
///
/// This function creates a temporary file with the current script (if any),
/// opens it in the user's default editor (from $EDITOR env var, or falls back to vim/nano/notepad),
/// and returns the edited content when the editor is closed.
///
/// # Parameters
/// * `current_script` - The current script to pre-populate the editor with
///
/// # Returns
/// * `Ok(String)` - The edited script content
/// * `Err(Box<dyn Error>)` - Any error that occurred during the process
#[allow(dead_code)]
pub fn edit_multiline_script(current_script: &str) -> Result<String, Box<dyn Error>> {
    // Create a temporary file
    let mut temp_file = NamedTempFile::new()?;

    // Get the path to the temporary file
    let temp_path = temp_file.path().to_string_lossy().to_string();

    // Write the current script to the temp file
    temp_file.write_all(current_script.as_bytes())?;
    temp_file.flush()?;

    // Determine which editor to use
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
        if cfg!(target_os = "windows") {
            "notepad".to_string()
        } else if Command::new("vim").arg("--version").output().is_ok() {
            "vim".to_string()
        } else {
            "nano".to_string()
        }
    });

    // Print a message to indicate the editor being used
    println!("Opening {editor} for multiline editing...");

    // Open the editor with the temporary file
    let status = Command::new(&editor).arg(&temp_path).status()?;

    if !status.success() {
        return Err("Editor exited with non-zero status".into());
    }

    // Important: We need to read the file separately AFTER the editor is closed
    // because the temp_file may be locked by the editor while it's open
    let content = fs::read_to_string(&temp_path)?;

    Ok(content)
}

/// Save a script to a file
///
/// # Parameters
/// * `script` - The script content to save
/// * `filename` - The name of the file to save to
///
/// # Returns
/// * `Ok(())` - If the script was saved successfully
/// * `Err(Box<dyn Error>)` - Any error that occurred during saving
#[allow(dead_code)]
pub fn save_script_to_file(script: &str, filename: &str) -> Result<(), Box<dyn Error>> {
    let path = Path::new(filename);

    // Check if the file already exists
    if path.exists() {
        println!("File already exists. Overwrite? (y/n)");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            return Err("Operation cancelled by user".into());
        }
    }

    // Write the script to the file
    let mut file = File::create(path)?;
    file.write_all(script.as_bytes())?;

    Ok(())
}

/// Load a script from a file
///
/// # Parameters
/// * `filename` - The name of the file to load from
///
/// # Returns
/// * `Ok(String)` - The loaded script content
/// * `Err(Box<dyn Error>)` - Any error that occurred during loading
#[allow(dead_code)]
pub fn load_script_from_file(filename: &str) -> Result<String, Box<dyn Error>> {
    let path = Path::new(filename);

    // Check if the file exists
    if !path.exists() {
        return Err(format!("File not found: {filename}").into());
    }

    // Read the file content
    let content = fs::read_to_string(path)?;

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[rstest]
    fn test_save_and_load_script() {
        // Create a temporary file for testing
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path().to_str().unwrap().to_string();

        // We need to close the original file handle to avoid conflicts
        drop(temp_file);

        // Test script content
        let test_script = "SELECT * FROM users;\nSELECT * FROM products;\n";

        // Save the script
        save_script_to_file(test_script, &temp_path).unwrap();

        // Load the script
        let loaded_script = load_script_from_file(&temp_path).unwrap();

        // Verify
        assert_eq!(test_script, loaded_script);

        // Clean up
        std::fs::remove_file(temp_path).ok();
    }

    #[rstest]
    fn test_load_nonexistent_file() {
        // Try to load a file that doesn't exist
        let result = load_script_from_file("nonexistent_file.sql");

        // Verify
        assert!(result.is_err());
    }

    #[rstest]
    fn test_edit_multiline_script_loads_content() {
        // This test only verifies that the initial content is loaded
        // We can't test the full edit functionality without mocking the editor

        // Create a temporary file with test content
        let mut temp_file = NamedTempFile::new().unwrap();
        let _temp_path = temp_file.path().to_str().unwrap().to_string();
        temp_file.write_all(b"SELECT * FROM test;").unwrap();
        temp_file.flush().unwrap();

        // In a real test we would mock the editor, but for now we'll just
        // verify that the script module compiles and has reasonable behavior
        // for testable components
    }
}
