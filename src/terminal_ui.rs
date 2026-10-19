//! The CLI's [`TerminalUi`]: `inquire` prompts and, with the `explain-tui`
//! feature, the ratatui plan viewer. `Database` (in `dbcrust-app`, shared
//! with the desktop app) reaches the terminal only through it.

use dbcrust_app::terminal_ui::{self, TerminalUi};
use inquire::{MultiSelect, Select};

struct InquireUi;

impl TerminalUi for InquireUi {
    fn select_columns(
        &self,
        message: &str,
        columns: &[String],
        preselected: &[usize],
    ) -> Option<Vec<String>> {
        let mut selector = MultiSelect::new(message, columns.to_vec())
            .with_help_message(
                "Use arrow keys to navigate, Space to select/deselect, Enter to confirm, Ctrl-C to cancel",
            )
            .with_vim_mode(false);
        if !preselected.is_empty() {
            selector = selector.with_default(preselected);
        }

        // A panic inside the prompt (terminal gone) counts as a cancel
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| selector.prompt())) {
            Ok(Ok(selected)) => Some(selected),
            Ok(Err(_)) | Err(_) => None,
        }
    }

    fn select_one(&self, message: &str, options: &[String]) -> Result<usize, String> {
        Select::new(message, options.to_vec())
            .raw_prompt()
            .map(|choice| choice.index)
            .map_err(|e| e.to_string())
    }

    #[cfg(feature = "explain-tui")]
    fn has_plan_viewer(&self) -> bool {
        true
    }

    #[cfg(feature = "explain-tui")]
    fn show_plan(&self, plan: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
        crate::explain_tui::run_explain_tui(plan)?;
        Ok(())
    }
}

/// Installs the CLI's prompts and viewers for every `Database` of this
/// process (in [`crate::db::FrontendMode::Cli`]).
pub fn install() {
    terminal_ui::install(Box::new(InquireUi));
}
