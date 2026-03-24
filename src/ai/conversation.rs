//! Conversation history for multi-turn AI queries
//! Stored in session memory (not persisted to disk).

use crate::ai::MessageRole;
use std::collections::VecDeque;

pub struct AiConversation {
    /// Recent exchanges (user query, assistant SQL response)
    history: VecDeque<(String, String)>,
    /// Max exchanges to keep
    max_length: usize,
}

impl AiConversation {
    pub fn new(max_length: usize) -> Self {
        AiConversation {
            history: VecDeque::new(),
            max_length,
        }
    }

    pub fn add_exchange(&mut self, user_query: &str, ai_response: &str) {
        if self.max_length == 0 {
            return;
        }
        if self.history.len() >= self.max_length {
            self.history.pop_front();
        }
        self.history
            .push_back((user_query.to_string(), ai_response.to_string()));
    }

    /// Build the message list for the AI provider, including conversation history
    /// plus the current user query.
    pub fn to_messages(&self, current_query: &str) -> Vec<(MessageRole, String)> {
        let mut messages = Vec::new();

        // Add previous exchanges
        for (user_msg, assistant_msg) in &self.history {
            messages.push((MessageRole::User, user_msg.clone()));
            messages.push((MessageRole::Assistant, assistant_msg.clone()));
        }

        // Add current query
        messages.push((MessageRole::User, current_query.to_string()));

        messages
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_new() {
        let conv = AiConversation::new(5);
        assert!(conv.is_empty());
        assert_eq!(conv.len(), 0);
    }

    #[test]
    fn test_conversation_add_exchange() {
        let mut conv = AiConversation::new(3);
        conv.add_exchange("show users", "SELECT * FROM users LIMIT 100;");
        assert_eq!(conv.len(), 1);
    }

    #[test]
    fn test_conversation_max_length() {
        let mut conv = AiConversation::new(2);
        conv.add_exchange("q1", "a1");
        conv.add_exchange("q2", "a2");
        conv.add_exchange("q3", "a3");
        assert_eq!(conv.len(), 2);

        let msgs = conv.to_messages("q4");
        // Should have: q2, a2, q3, a3, q4
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].1, "q2");
        assert_eq!(msgs[4].1, "q4");
    }

    #[test]
    fn test_conversation_zero_length() {
        let mut conv = AiConversation::new(0);
        conv.add_exchange("q1", "a1");
        assert!(conv.is_empty());

        let msgs = conv.to_messages("q2");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].1, "q2");
    }

    #[test]
    fn test_conversation_to_messages() {
        let mut conv = AiConversation::new(5);
        conv.add_exchange("show me users", "SELECT * FROM users LIMIT 100;");

        let msgs = conv.to_messages("now filter by active");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].0, MessageRole::User);
        assert_eq!(msgs[0].1, "show me users");
        assert_eq!(msgs[1].0, MessageRole::Assistant);
        assert_eq!(msgs[1].1, "SELECT * FROM users LIMIT 100;");
        assert_eq!(msgs[2].0, MessageRole::User);
        assert_eq!(msgs[2].1, "now filter by active");
    }

    #[test]
    fn test_conversation_clear() {
        let mut conv = AiConversation::new(5);
        conv.add_exchange("q1", "a1");
        conv.add_exchange("q2", "a2");
        conv.clear();
        assert!(conv.is_empty());
    }
}
