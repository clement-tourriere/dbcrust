//! TUI-based Query Plan Visualizer
//!
//! This module provides an interactive terminal user interface for visualizing
//! PostgreSQL EXPLAIN plans. It uses ratatui for rendering and provides:
//!
//! - Hierarchical tree view of the query plan
//! - Color-coded performance indicators
//! - Detailed node information panel
//! - Keyboard navigation and interactive exploration
//!
//! # Usage
//!
//! Enable TUI explain mode with the `\ev` command in the REPL, then run any query.
//! The plan will be displayed in an interactive TUI instead of plain text.
//!
//! # Example
//!
//! ```text
//! dbcrust> \ev
//! Explain TUI mode: ON
//! dbcrust> SELECT * FROM users WHERE email = 'test@example.com';
//! [Opens TUI visualizer]
//! ```

mod app;
mod plan_tree;
mod ui;

pub use app::{ExplainTuiApp, TuiResult};
pub use plan_tree::{PlanNode, PlanStatistics, parse_postgresql_plan};

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use serde_json::Value as JsonValue;
use std::io::{self, stdout};
use std::panic;
use std::time::Duration;

/// Run the TUI explain visualizer for a PostgreSQL plan
///
/// This function takes a PostgreSQL EXPLAIN (FORMAT JSON) output and displays
/// it in an interactive TUI. The function blocks until the user exits the TUI.
///
/// # Arguments
///
/// * `plan_json` - The JSON output from PostgreSQL's EXPLAIN (FORMAT JSON) command
///
/// # Returns
///
/// * `Ok(TuiResult)` - The result of the TUI session
/// * `Err(String)` - An error message if the TUI could not be started
///
/// # Example
///
/// ```ignore
/// let plan_json = serde_json::json!([{
///     "Plan": {
///         "Node Type": "Seq Scan",
///         "Relation Name": "users",
///         "Total Cost": 100.0,
///         "Plan Rows": 1000
///     }
/// }]);
///
/// run_explain_tui(&plan_json)?;
/// ```
pub fn run_explain_tui(plan_json: &JsonValue) -> Result<TuiResult, String> {
    // Parse the plan JSON into our tree structure
    let plan_root = parse_postgresql_plan(plan_json).ok_or_else(|| {
        "Failed to parse plan JSON. Expected PostgreSQL EXPLAIN (FORMAT JSON) output.".to_string()
    })?;

    // Create the application state
    let mut app = ExplainTuiApp::new(plan_root);

    // Set up panic hook to restore terminal on panic
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // Attempt to restore terminal
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Initialize terminal
    let result = run_tui_loop(&mut app);

    // Restore panic hook
    let _ = panic::take_hook();

    result
}

/// Internal function to run the TUI event loop
fn run_tui_loop(app: &mut ExplainTuiApp) -> Result<TuiResult, String> {
    // Setup terminal
    enable_raw_mode().map_err(|e| format!("Failed to enable raw mode: {}", e))?;

    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .map_err(|e| format!("Failed to enter alternate screen: {}", e))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal =
        Terminal::new(backend).map_err(|e| format!("Failed to create terminal: {}", e))?;

    // Clear the terminal
    terminal
        .clear()
        .map_err(|e| format!("Failed to clear terminal: {}", e))?;

    // Main event loop
    let result = loop {
        // Draw the UI
        terminal
            .draw(|frame| ui::render(frame, app))
            .map_err(|e| format!("Failed to draw: {}", e))?;

        // Poll for events with a timeout
        match app.poll_events(Duration::from_millis(100)) {
            Ok(_) => {
                if app.should_quit {
                    break Ok(TuiResult::Quit);
                }
            }
            Err(e) => {
                break Err(format!("Event error: {}", e));
            }
        }
    };

    // Restore terminal
    disable_raw_mode().map_err(|e| format!("Failed to disable raw mode: {}", e))?;

    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .map_err(|e| format!("Failed to leave alternate screen: {}", e))?;

    terminal
        .show_cursor()
        .map_err(|e| format!("Failed to show cursor: {}", e))?;

    result
}

/// Check if the terminal supports the TUI
///
/// Returns true if the terminal is capable of running the TUI visualizer.
/// This checks for:
/// - Terminal is a TTY
/// - Terminal has sufficient size
pub fn can_run_tui() -> bool {
    // Check if stdout is a TTY
    if !io::IsTerminal::is_terminal(&io::stdout()) {
        return false;
    }

    // Check terminal size (need at least 80x24)
    if let Ok((width, height)) = crossterm::terminal::size() {
        width >= 60 && height >= 15
    } else {
        false
    }
}

/// Get a message explaining why the TUI cannot run
pub fn tui_unavailable_reason() -> Option<String> {
    if !io::IsTerminal::is_terminal(&io::stdout()) {
        return Some("TUI requires an interactive terminal (stdout is not a TTY)".to_string());
    }

    if let Ok((width, height)) = crossterm::terminal::size() {
        if width < 60 || height < 15 {
            return Some(format!(
                "Terminal too small ({}x{}, need at least 60x15)",
                width, height
            ));
        }
    } else {
        return Some("Could not determine terminal size".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plan_from_json() {
        let plan_json = serde_json::json!([{
            "Plan": {
                "Node Type": "Seq Scan",
                "Relation Name": "test_table",
                "Schema": "public",
                "Total Cost": 100.0,
                "Plan Rows": 500,
                "Actual Rows": 450,
                "Actual Total Time": 5.5
            }
        }]);

        let plan = parse_postgresql_plan(&plan_json);
        assert!(plan.is_some());

        let plan = plan.unwrap();
        assert_eq!(plan.node_type, "Seq Scan");
        assert_eq!(plan.relation_name, Some("test_table".to_string()));
    }

    #[test]
    fn test_invalid_plan_json() {
        let invalid_json = serde_json::json!({
            "not": "a plan"
        });

        let plan = parse_postgresql_plan(&invalid_json);
        assert!(plan.is_none());
    }

    #[test]
    fn test_plan_statistics() {
        let plan_json = serde_json::json!([{
            "Plan": {
                "Node Type": "Hash Join",
                "Total Cost": 500.0,
                "Plan Rows": 100,
                "Plans": [
                    {
                        "Node Type": "Index Scan",
                        "Relation Name": "orders",
                        "Index Name": "orders_pkey",
                        "Total Cost": 50.0,
                        "Plan Rows": 10
                    },
                    {
                        "Node Type": "Hash",
                        "Total Cost": 200.0,
                        "Plan Rows": 100,
                        "Plans": [
                            {
                                "Node Type": "Seq Scan",
                                "Relation Name": "users",
                                "Total Cost": 150.0,
                                "Plan Rows": 1000
                            }
                        ]
                    }
                ]
            }
        }]);

        let plan = parse_postgresql_plan(&plan_json).unwrap();
        let stats = PlanStatistics::from_plan(&plan);

        assert_eq!(stats.total_nodes, 4);
        assert_eq!(stats.max_depth, 3);
        assert!(stats.tables_involved.contains(&"orders".to_string()));
        assert!(stats.tables_involved.contains(&"users".to_string()));
        assert!(stats.indexes_used.contains(&"orders_pkey".to_string()));
        assert!(stats.has_seq_scans); // users table has seq scan with 1000 rows
    }
}
