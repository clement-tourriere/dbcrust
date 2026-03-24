//! TUI-based Interactive Schema Viewer
//!
//! This module provides an interactive terminal user interface for exploring
//! database schemas. It uses ratatui for rendering and provides:
//!
//! - Three-panel layout: table list, details, and FK relationships
//! - Search/filter for tables
//! - FK navigation (follow relationships between tables)
//! - Toggle visibility for indexes and constraints
//! - Keyboard-driven navigation
//!
//! # Usage
//!
//! Run the `\sv` command in the REPL to launch the schema viewer.

mod app;
pub mod schema_data;
mod ui;

pub use app::SchemaTuiApp;
pub use schema_data::{SchemaData, load_schema_data};

use crate::db::Database;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use std::io::stdout;
use std::panic;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Result of running the schema TUI
#[derive(Debug)]
pub enum TuiResult {
    Quit,
}

/// Run the interactive schema TUI viewer
pub fn run_schema_tui(
    schema_data: SchemaData,
    database: Arc<Mutex<Database>>,
) -> Result<TuiResult, String> {
    let mut app = SchemaTuiApp::new(schema_data, database);

    // Set up panic hook to restore terminal on panic
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    let result = run_tui_loop(&mut app);

    // Restore panic hook
    let _ = panic::take_hook();

    result
}

/// Internal function to run the TUI event loop
fn run_tui_loop(app: &mut SchemaTuiApp) -> Result<TuiResult, String> {
    enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {e}"))?;

    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .map_err(|e| format!("Failed to enter alternate screen: {e}"))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to create terminal: {e}"))?;

    terminal
        .clear()
        .map_err(|e| format!("Failed to clear terminal: {e}"))?;

    let result = loop {
        terminal
            .draw(|frame| ui::render(frame, &mut *app))
            .map_err(|e| format!("Failed to draw: {e}"))?;

        match app.poll_events(Duration::from_millis(100)) {
            Ok(_) => {
                if app.should_quit {
                    break Ok(TuiResult::Quit);
                }
            }
            Err(e) => {
                break Err(format!("Event error: {e}"));
            }
        }
    };

    // Restore terminal
    disable_raw_mode().map_err(|e| format!("Failed to disable raw mode: {e}"))?;

    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .map_err(|e| format!("Failed to leave alternate screen: {e}"))?;

    terminal
        .show_cursor()
        .map_err(|e| format!("Failed to show cursor: {e}"))?;

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;

    fn test_database() -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(Database::new_for_test()))
    }

    #[test]
    fn test_schema_tui_app_creation() {
        let schema_data = SchemaData {
            database_name: "testdb".to_string(),
            database_type: DatabaseType::PostgreSQL,
            schemas: vec![],
            relationships: vec![],
        };

        let app = SchemaTuiApp::new(schema_data, test_database());
        assert!(!app.should_quit);
        assert!(!app.show_help);
        assert!(app.table_list.is_empty());
    }

    #[test]
    fn test_schema_tui_app_with_data() {
        use schema_data::{Relationship, SchemaInfo, TableSummary};

        let schema_data = SchemaData {
            database_name: "testdb".to_string(),
            database_type: DatabaseType::PostgreSQL,
            schemas: vec![SchemaInfo {
                name: "public".to_string(),
                tables: vec![
                    TableSummary {
                        name: "users".to_string(),
                        schema: "public".to_string(),
                        outgoing_fk_count: 0,
                        incoming_fk_count: 1,
                    },
                    TableSummary {
                        name: "posts".to_string(),
                        schema: "public".to_string(),
                        outgoing_fk_count: 1,
                        incoming_fk_count: 0,
                    },
                ],
            }],
            relationships: vec![Relationship {
                constraint_name: "fk_posts_author".to_string(),
                source_schema: "public".to_string(),
                source_table: "posts".to_string(),
                source_columns: vec!["author_id".to_string()],
                target_schema: "public".to_string(),
                target_table: "users".to_string(),
                target_columns: vec!["id".to_string()],
            }],
        };

        let app = SchemaTuiApp::new(schema_data, test_database());
        assert_eq!(app.table_count(), 2);
        assert_eq!(app.schema_count(), 1);
        // First entry is schema header, so first table should be at index 1
        assert_eq!(app.table_list.len(), 3); // 1 header + 2 tables
    }
}
