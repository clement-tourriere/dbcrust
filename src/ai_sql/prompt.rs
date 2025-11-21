//! Prompt generation utilities for AI SQL generation

use crate::ai_sql::schema::SchemaContext;

/// Prompt generator for AI SQL queries
pub struct PromptGenerator;

impl PromptGenerator {
    /// Generate a system prompt for SQL generation
    pub fn system_prompt(database_type: &str) -> String {
        format!(
            "You are an expert {} SQL query generator. Generate efficient, idiomatic queries.",
            database_type
        )
    }

    /// Generate a user prompt with schema context
    pub fn user_prompt(query: &str, schema: &SchemaContext) -> String {
        format!(
            "Database: {}\nCurrent Schema: {:?}\n\nUser Query: {}\n\nGenerate the SQL query.",
            schema.current_database, schema.current_schema, query
        )
    }

    /// Generate refinement prompt
    pub fn refinement_prompt(original_sql: &str, feedback: &str) -> String {
        format!(
            "Original SQL:\n{}\n\nUser Feedback: {}\n\nGenerate refined SQL based on feedback.",
            original_sql, feedback
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DatabaseType;

    #[test]
    fn test_system_prompt() {
        let prompt = PromptGenerator::system_prompt("PostgreSQL");
        assert!(prompt.contains("PostgreSQL"));
        assert!(prompt.contains("SQL query generator"));
    }

    #[test]
    fn test_user_prompt() {
        let schema = SchemaContext {
            database_type: DatabaseType::PostgreSQL,
            current_database: "testdb".to_string(),
            current_schema: Some("public".to_string()),
            tables: vec![],
            relationships: vec![],
            common_patterns: vec![],
        };

        let prompt = PromptGenerator::user_prompt("top 10 users", &schema);
        assert!(prompt.contains("testdb"));
        assert!(prompt.contains("top 10 users"));
    }
}
