use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::ptr;
use std::sync::{Mutex, Once};
use std::time::SystemTime;

use crate::config;

// Debug logging macro for consistent debug output
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        // For now, always log debug messages if debug logging is available
        // In the future, this could check configuration
        let _ = $crate::logging::debug(&format!($($arg)*));
    };
}

static INIT: Once = Once::new();
// Use a single Mutex instead of trying to manage raw pointers
static mut LOG_FILE: Option<Mutex<Option<(File, PathBuf)>>> = None;

/// Get the log file path as a string if available
pub fn get_log_file_path_string() -> Option<String> {
    unsafe {
        // Use ptr::addr_of to get a raw pointer without creating a reference
        let log_file_ptr = ptr::addr_of!(LOG_FILE);
        if let Some(log_file) = &*log_file_ptr {
            if let Ok(guard) = log_file.lock() {
                if let Some((_, path)) = &*guard {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

/// Initialize the logging system, creating the log file in the config directory
pub fn init() -> io::Result<()> {
    let mut result = Ok(());

    INIT.call_once(|| {
        if let Ok(config_dir) = config::Config::get_config_dir() {
            // Create config directory if it doesn't exist
            if let Some(parent) = config_dir.parent() {
                if !parent.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        eprintln!("Failed to create parent directory for logs: {e}");
                        result = Err(e);
                        return;
                    }
                }
            }

            if !config_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&config_dir) {
                    eprintln!("Failed to create config directory for logs: {e}");
                    result = Err(e);
                    return;
                }
            }

            // Create log file
            let log_file_path = get_log_file_path(&config_dir);

            match OpenOptions::new()
                .create(true)
                .append(true)
                
                .open(&log_file_path)
            {
                Ok(file) => {
                    // Store file and path together safely
                    unsafe {
                        LOG_FILE = Some(Mutex::new(Some((file, log_file_path.clone()))));
                    }

                    // Write initial log entry
                    let _ = log_message(&format!("Log initialized at {}", timestamp()));
                    let _ = log_message(&format!(
                        "Log file path: {}",
                        log_file_path.to_string_lossy()
                    ));

                    // Don't print to console during initialization
                    // We'll only show this when specifically requested
                }
                Err(e) => {
                    eprintln!("Failed to open log file: {e}");
                    result = Err(e);
                }
            }
        } else {
            eprintln!("Could not determine config directory for logs");
            result = Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Config directory not found",
            ));
        }
    });

    result
}

/// Get the path to the log file
fn get_log_file_path(config_dir: &PathBuf) -> PathBuf {
    config_dir.join("debug.log")
}

/// Get the current timestamp as a string
fn timestamp() -> String {
    let now = SystemTime::now();
    let datetime = chrono::DateTime::<chrono::Local>::from(now);
    datetime.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Write a message to the log file
fn log_message(message: &str) -> io::Result<()> {
    unsafe {
        // Use ptr::addr_of to get a raw pointer without creating a reference
        let log_file_ptr = ptr::addr_of!(LOG_FILE);
        if let Some(log_file) = &*log_file_ptr {
            if let Ok(mut file_guard) = log_file.lock() {
                if let Some((file, _)) = &mut *file_guard {
                    let log_entry = format!("[{}] {}\n", timestamp(), message);
                    file.write_all(log_entry.as_bytes())?;
                    file.flush()?;
                }
            }
        }
    }
    Ok(())
}

/// Log a debug message
pub fn debug(message: &str) -> io::Result<()> {
    // Check if debug logging is enabled in the config before logging
    let config = config::Config::load();
    if !matches!(config.logging.level, crate::config::LogLevel::Debug | crate::config::LogLevel::Trace) {
        return Ok(());
    }

    // Initialize logging if not already initialized
    init()?;
    log_message(&format!("DEBUG {message}"))
}
