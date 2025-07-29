// Import the debug_log macro here
extern crate dbcrust;
mod cli;
// completion is now in lib.rs
mod highlighter;
mod named_queries;
mod pager;
mod password_sanitizer;
mod pgpass;
mod script;

use clap::Parser;
use dbcrust::cli::Args;
use std::error::Error as StdError;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use dbcrust::config::{Config, LogLevel};
// For `std::io::stdout().flush()`








/// Initialize the tracing system based on configuration
fn init_tracing() -> Result<(), Box<dyn StdError>> {
    let config = Config::load();
    
    // Convert our LogLevel to tracing filter
    let level_filter = match config.logging.level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    };
    
    // Create the base registry
    let registry = tracing_subscriber::registry()
        .with(EnvFilter::new(level_filter));
    
    // Build and initialize subscriber based on output preferences
    match (config.logging.console_output, config.logging.file_output) {
        (true, true) => {
            // Both console and file output
            // Create log directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(&config.logging.file_path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            let file_appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .max_log_files(config.logging.max_files)
                .build(&config.logging.file_path)?;
            
            registry
                .with(fmt::layer().compact())  // Console layer
                .with(fmt::layer()             // File layer
                    .with_writer(file_appender)
                    .with_ansi(false)
                    .with_target(true)
                    .with_file(true)
                    .with_line_number(true))
                .init();
        }
        (true, false) => {
            // Console output only
            registry
                .with(fmt::layer().compact())
                .init();
        }
        (false, true) => {
            // File output only
            // Create log directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(&config.logging.file_path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            let file_appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .max_log_files(config.logging.max_files)
                .build(&config.logging.file_path)?;
            
            registry
                .with(fmt::layer()
                    .with_writer(file_appender)
                    .with_ansi(false)
                    .with_target(true)
                    .with_file(true)
                    .with_line_number(true))
                .init();
        }
        (false, false) => {
            // No output - just initialize with a no-op layer
            registry.init();
        }
    }
    
    Ok(())
}

/// Main async workflow that can be called from both main() and Python
pub async fn async_main() -> Result<(), Box<dyn StdError>> {
    // Initialize tracing system
    if let Err(e) = init_tracing() {
        eprintln!("Failed to initialize logging: {}", e);
    }
    
    let args = Args::parse();
    dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}

/// Main async workflow with pre-parsed arguments (for Python integration)
pub async fn async_main_with_args(args: Args) -> Result<(), Box<dyn StdError>> {
    // Initialize tracing system
    if let Err(e) = init_tracing() {
        eprintln!("Failed to initialize logging: {}", e);
    }
    
    dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}



#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    // Initialize tracing system before anything else
    if let Err(e) = init_tracing() {
        eprintln!("Failed to initialize logging: {}", e);
        // Continue without logging rather than exit
    }
    
    let args = Args::parse();
    match dbcrust::cli_core::CliCore::run_with_args(args).await {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(e) => {
            // Display user-friendly error message instead of Debug representation
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
