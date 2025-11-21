//! Interactive UI for AI SQL generation

use crate::ai_sql::client::{AiProvider, AiResponse};
use crate::ai_sql::config::AiSqlConfig;
use crate::ai_sql::dialect::SqlDialectProvider;
use crate::ai_sql::error::{AiError, AiResult};
use crate::db::Database;
use inquire::{Select, Text};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Interactive mode for AI SQL generation
pub struct InteractiveMode<'a> {
    database: Arc<Mutex<Database>>,
    ai_client: &'a mut Box<dyn AiProvider>,
    dialect: &'a dyn SqlDialectProvider,
    config: &'a AiSqlConfig,
}

impl<'a> InteractiveMode<'a> {
    pub fn new(
        database: Arc<Mutex<Database>>,
        ai_client: &'a mut Box<dyn AiProvider>,
        dialect: &'a dyn SqlDialectProvider,
        config: &'a AiSqlConfig,
    ) -> Self {
        Self {
            database,
            ai_client,
            dialect,
            config,
        }
    }

    /// Run interactive mode
    pub async fn run(&mut self) -> AiResult<()> {
        println!("🤖 AI SQL Chat Mode (type 'exit' to quit)\n");

        loop {
            // Get user input
            let user_input = match Text::new("You:").prompt() {
                Ok(input) => input,
                Err(_) => {
                    return Err(AiError::UserCancelled);
                }
            };

            if user_input.trim().eq_ignore_ascii_case("exit")
                || user_input.trim().eq_ignore_ascii_case("quit")
            {
                println!("Goodbye!");
                break;
            }

            // TODO: Implement chat mode
            println!("AI: Chat mode not yet fully implemented. Use \\ai for single queries.");
        }

        Ok(())
    }

    /// Present options for a generated SQL query
    pub fn present_options(_response: &AiResponse) -> AiResult<QueryAction> {
        let options = vec![
            "Execute",
            "Refine",
            "Copy to clipboard",
            "Explain",
            "Cancel",
        ];

        match Select::new("What would you like to do?", options).prompt() {
            Ok("Execute") => Ok(QueryAction::Execute),
            Ok("Refine") => {
                let feedback = Text::new("Refinement feedback:")
                    .prompt()
                    .map_err(|_| AiError::UserCancelled)?;
                Ok(QueryAction::Refine(feedback))
            }
            Ok("Copy to clipboard") => Ok(QueryAction::Copy),
            Ok("Explain") => Ok(QueryAction::Explain),
            _ => Ok(QueryAction::Cancel),
        }
    }

    /// Display generated SQL with formatting
    pub fn display_sql(sql: &str, dialect_name: &str) {
        println!("\n┌─────────────────────────────────────────────────────────┐");
        println!("│ 🤖 AI Generated SQL ({})                     │", dialect_name);
        println!("├─────────────────────────────────────────────────────────┤");
        println!("│                                                         │");

        // Split SQL into lines and display with padding
        for line in sql.lines() {
            println!("│ {:<55} │", line);
        }

        println!("│                                                         │");
        println!("└─────────────────────────────────────────────────────────┘\n");
    }

    /// Display error message
    pub fn display_error(error: &AiError) {
        println!("\n❌ Error: {}\n", error.user_message());
    }

    /// Display success message
    pub fn display_success(message: &str) {
        println!("\n✅ {}\n", message);
    }

    /// Display warning message
    pub fn display_warning(message: &str) {
        println!("\n⚠️  {}\n", message);
    }
}

/// User action for a generated query
#[derive(Debug)]
pub enum QueryAction {
    Execute,
    Refine(String),
    Copy,
    Explain,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_sql() {
        let sql = "SELECT * FROM users WHERE id = 1;";
        InteractiveMode::display_sql(sql, "PostgreSQL");
        // Visual test - should display formatted SQL
    }

    #[test]
    fn test_display_messages() {
        InteractiveMode::display_success("Query executed successfully");
        InteractiveMode::display_warning("Query may be slow");
        InteractiveMode::display_error(&AiError::UserCancelled);
        // Visual tests - should display formatted messages
    }
}
