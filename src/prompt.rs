use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;

pub struct DbPrompt {
    username: String,
    db_name: String,
    multiline_indicator: String,
}

impl DbPrompt {
    pub fn new(username: String, db_name: String) -> Self {
        Self { 
            username, 
            db_name, 
            multiline_indicator: String::new() // Default to empty
        }
    }
    
    pub fn with_config(username: String, db_name: String, multiline_indicator: String) -> Self {
        Self { 
            username, 
            db_name, 
            multiline_indicator
        }
    }
    
    pub fn update_database(&mut self, new_db_name: &str) {
        self.db_name = new_db_name.to_string();
    }
}

impl Prompt for DbPrompt {
    fn render_prompt_left(&self) -> Cow<str> {
        Cow::Owned(format!("{}@{}=> ", self.username, self.db_name))
    }

    fn render_prompt_right(&self) -> Cow<str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => Cow::Borrowed(""),
            PromptEditMode::Vi(vi_mode) => match vi_mode {
                reedline::PromptViMode::Insert => Cow::Borrowed("[INS] "),
                reedline::PromptViMode::Normal => Cow::Borrowed("[NOR] "),
            },
            PromptEditMode::Custom(_) => Cow::Borrowed(""),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<str> {
        Cow::Borrowed(&self.multiline_indicator)
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<str> {
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
