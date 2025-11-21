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
use dbcrust::config::{Config, LogLevel};
use std::error::Error as StdError;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
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
    let registry = tracing_subscriber::registry().with(EnvFilter::new(level_filter));

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
                .with(fmt::layer().compact()) // Console layer
                .with(
                    fmt::layer() // File layer
                        .with_writer(file_appender)
                        .with_ansi(false)
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true),
                )
                .init();
        }
        (true, false) => {
            // Console output only
            registry.with(fmt::layer().compact()).init();
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
                .with(
                    fmt::layer()
                        .with_writer(file_appender)
                        .with_ansi(false)
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true),
                )
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
        eprintln!("Failed to initialize logging: {e}");
    }

    let args = Args::parse();
    dbcrust::cli_core::CliCore::run_with_args(args)
        .await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}

/// Main async workflow with pre-parsed arguments (for Python integration)
pub async fn async_main_with_args(args: Args) -> Result<(), Box<dyn StdError>> {
    // Initialize tracing system
    if let Err(e) = init_tracing() {
        eprintln!("Failed to initialize logging: {e}");
    }

    dbcrust::cli_core::CliCore::run_with_args(args)
        .await
        .map_err(|e| -> Box<dyn StdError> { Box::new(e) })?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    // Initialize tracing system before anything else
    if let Err(e) = init_tracing() {
        eprintln!("Failed to initialize logging: {e}");
        // Continue without logging rather than exit
    }

    let args = Args::parse();

    // Handle subcommands first
    if let Some(subcommand) = &args.subcommand {
        match subcommand {
            dbcrust::cli::SubCommand::AiAuth { action } => {
                handle_ai_auth_subcommand(action).await?;
                std::process::exit(0);
            }
        }
    }

    // Regular database connection flow
    match dbcrust::cli_core::CliCore::run_with_args(args).await {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(e) => {
            // Display user-friendly error message instead of Debug representation
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// Handle AI authentication subcommands
async fn handle_ai_auth_subcommand(action: &dbcrust::cli::AiAuthAction) -> Result<(), Box<dyn StdError>> {
    use dbcrust::ai_sql::{AnthropicOAuthPkce, PkceChallenge};
    use dbcrust::cli::AiAuthAction;
    use dbcrust::config::Config;

    let config_dir = Config::get_config_directory()?;
    let oauth = AnthropicOAuthPkce::new(config_dir)?;

    match action {
        AiAuthAction::Login { provider } => {
            println!("🔐 Authenticating with {} OAuth...\n", provider);

            // Generate PKCE challenge
            let pkce = PkceChallenge::generate();

            // Get authorization URL
            let auth_url = oauth.start_authorization(&pkce);

            println!("Please follow these steps:");
            println!("1. Open your browser and visit this URL:");
            println!("\n   {}\n", auth_url);
            println!("2. Sign in with your Anthropic account (Claude Pro/Team)");
            println!("3. Authorize dbcrust to access your account");
            println!("4. Anthropic will display an authorization code");
            println!("5. Copy the authorization code and paste it below\n");

            // Prompt for authorization code
            use inquire::Text;
            let auth_response = Text::new("Authorization code:")
                .with_help_message("Paste the code shown on the Anthropic authorization page")
                .prompt()?;

            // Validate authorization response format: "CODE#STATE"
            let auth_response = auth_response.trim();

            // Validate state parameter if present (CSRF protection)
            if let Some((_, returned_state)) = auth_response.split_once('#') {
                if returned_state != pkce.state {
                    eprintln!("\n❌ Authentication failed: State parameter mismatch!");
                    eprintln!("Expected: {}", pkce.state);
                    eprintln!("Received: {}", returned_state);
                    eprintln!("This could indicate a security issue (CSRF attack).");
                    eprintln!("Please try again with 'dbcrust ai-auth login'");
                    std::process::exit(1);
                }
            }

            // Exchange code for token (pass full CODE#STATE string)
            println!("\nExchanging authorization code for access token...");
            match oauth.exchange_code(auth_response, &pkce.verifier).await {
                Ok(token) => {
                    println!("\n✅ Successfully authenticated!");
                    println!("Token expires: {}", token.expires_at.format("%Y-%m-%d %H:%M:%S UTC"));
                    println!("\nYou can now use \\ai commands in dbcrust to generate SQL!");
                }
                Err(e) => {
                    eprintln!("\n❌ Authentication failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        AiAuthAction::Logout => {
            println!("Logging out...");
            oauth.logout().await?;
            println!("✅ Successfully logged out. OAuth token removed.");
        }

        AiAuthAction::Status => {
            if oauth.is_authenticated().await {
                match oauth.load_token().await {
                    Ok(token) => {
                        println!("✅ Authenticated");
                        println!("Token expires: {}", token.expires_at.format("%Y-%m-%d %H:%M:%S UTC"));

                        if token.is_expired() {
                            println!("\n⚠️  Token is expired or will expire soon!");
                            if token.can_refresh() {
                                println!("   It will be refreshed automatically on next use.");
                            } else {
                                println!("   Please run 'dbcrust ai-auth login' to re-authenticate.");
                            }
                        } else {
                            println!("\n🟢 Token is valid");
                        }
                    }
                    Err(e) => {
                        println!("❌ Not authenticated: {}", e);
                        println!("\nRun 'dbcrust ai-auth login' to authenticate.");
                    }
                }
            } else {
                println!("❌ Not authenticated");
                println!("\nRun 'dbcrust ai-auth login' to authenticate with Anthropic OAuth.");
            }
        }
    }

    Ok(())
}
