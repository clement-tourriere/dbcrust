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
// For `std::io::stdout().flush()`








/// Main async workflow that can be called from both main() and Python
pub async fn async_main() -> Result<(), Box<dyn StdError>> {
    let args = Args::parse();
    dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}

/// Main async workflow with pre-parsed arguments (for Python integration)
pub async fn async_main_with_args(args: Args) -> Result<(), Box<dyn StdError>> {
    dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}



#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    let args = Args::parse();
    let exit_code = dbcrust::cli_core::CliCore::run_with_args(args).await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    std::process::exit(exit_code);
}
