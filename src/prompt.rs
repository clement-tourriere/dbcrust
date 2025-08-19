use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;

pub struct DbPrompt {
    username: String,
    db_name: String,
    multiline_indicator: String,
}

impl DbPrompt {
    #[allow(dead_code)]
    pub fn new(username: String, db_name: String) -> Self {
        Self {
            username,
            db_name,
            multiline_indicator: String::new(), // Default to empty
        }
    }

    pub fn with_config(username: String, db_name: String, multiline_indicator: String) -> Self {
        Self {
            username,
            db_name,
            multiline_indicator,
        }
    }

    #[allow(dead_code)]
    pub fn update_database(&mut self, new_db_name: &str) {
        self.db_name = new_db_name.to_string();
    }
}

impl Prompt for DbPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(format!("{}@{}=> ", self.username, self.db_name))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => Cow::Borrowed(""),
            PromptEditMode::Vi(vi_mode) => match vi_mode {
                reedline::PromptViMode::Insert => Cow::Borrowed("[INS] "),
                reedline::PromptViMode::Normal => Cow::Borrowed("[NOR] "),
            },
            PromptEditMode::Custom(_) => Cow::Borrowed(""),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.multiline_indicator)
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let _prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "?",
        };
        match history_search.term.as_str() {
            "" => Cow::Borrowed("(reverse-i-search): "),
            _ => Cow::Owned(format!("(reverse-i-search '{}'): ", history_search.term)),
        }
    }
}

/// Simple continuation prompt for multiline SQL queries
pub struct ContinuationPrompt {
    prompt_text: String,
}

impl ContinuationPrompt {
    pub fn new(_main_prompt_length: usize, indicator: &str) -> Self {
        // For truly empty continuation lines, just use the indicator or empty string
        let prompt_text = if indicator.is_empty() {
            "".to_string() // Completely empty prompt
        } else {
            indicator.to_string() // Just the indicator without arrows or padding
        };

        Self { prompt_text }
    }
}

impl Prompt for ContinuationPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.prompt_text)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => Cow::Borrowed(""),
            PromptEditMode::Vi(vi_mode) => match vi_mode {
                reedline::PromptViMode::Insert => Cow::Borrowed("[INS] "),
                reedline::PromptViMode::Normal => Cow::Borrowed("[NOR] "),
            },
            PromptEditMode::Custom(_) => Cow::Borrowed(""),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let _prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "?",
        };
        match history_search.term.as_str() {
            "" => Cow::Borrowed("(reverse-i-search): "),
            _ => Cow::Owned(format!("(reverse-i-search '{}'): ", history_search.term)),
        }
    }
}
