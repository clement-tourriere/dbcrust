//! Terminal interactions [`crate::db::Database`] asks of its front end.
//!
//! This crate never prompts by itself: the CLI installs an implementation
//! backed by `inquire` and its ratatui plan viewer ([`install`]); the desktop
//! app installs none. Without one, every request takes the non-interactive
//! path: all columns are shown, a `docker://` URL must name its container, and
//! EXPLAIN plans are printed as text.

use std::error::Error as StdError;
use std::sync::OnceLock;

/// Prompts and full-screen viewers of an interactive terminal front end.
pub trait TerminalUi: Send + Sync {
    /// Lets the user pick columns to display among `columns`, with the
    /// `preselected` indices ticked. `None` when the prompt was cancelled or
    /// failed.
    fn select_columns(
        &self,
        message: &str,
        columns: &[String],
        preselected: &[usize],
    ) -> Option<Vec<String>>;

    /// Lets the user pick one of `options`; returns its index.
    fn select_one(&self, message: &str, options: &[String]) -> Result<usize, String>;

    /// True when [`TerminalUi::show_plan`] has a viewer to open.
    fn has_plan_viewer(&self) -> bool {
        false
    }

    /// Shows a PostgreSQL JSON plan (`EXPLAIN (FORMAT JSON)`) until the user
    /// closes the viewer.
    fn show_plan(&self, _plan: &serde_json::Value) -> Result<(), Box<dyn StdError>> {
        Err("this build has no plan viewer".into())
    }
}

static TERMINAL_UI: OnceLock<Box<dyn TerminalUi>> = OnceLock::new();

/// Installs the process-wide terminal front end; later calls are ignored.
pub fn install(ui: Box<dyn TerminalUi>) {
    let _ = TERMINAL_UI.set(ui);
}

/// The installed terminal front end, if any.
pub fn installed() -> Option<&'static dyn TerminalUi> {
    TERMINAL_UI.get().map(|ui| ui.as_ref())
}
