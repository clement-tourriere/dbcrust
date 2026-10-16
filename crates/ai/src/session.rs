//! What the AI assistant needs from a database session.
//!
//! The schema-context builder and the agent's tool executor only use this
//! handful of the root crate's `Database` methods. Depending on a trait instead of
//! the concrete session keeps the assistant free of the REPL/connection layer
//! (docker, SSH tunnels, interactive column selection) and lets it be tested
//! with a fake. `impl AiSession for Database` lives in `ai_session_impl`.
//!
//! The futures are deliberately `?Send`, like [`super::agent::AgentToolExecutor`]:
//! every caller already awaits them while holding the session's
//! `std::sync::Mutex` guard, so they never cross threads.

use std::error::Error as StdError;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;

use dbcrust_core::database::{DatabaseClient, DatabaseType};
use dbcrust_core::model::TableDetails;

/// Database session operations used by `ai::schema_context` and `ai::agent`.
#[async_trait(?Send)]
pub trait AiSession {
    /// Whether the user's `\x` explain mode is on.
    fn is_explain_mode(&self) -> bool;

    /// Flip explain mode; returns the new state.
    fn toggle_explain_mode(&mut self) -> bool;

    /// Run a query without the interactive column-selection prompt.
    async fn execute_query_with_interrupt_no_column_selection(
        &mut self,
        query: &str,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<Vec<Vec<String>>, Box<dyn StdError>>;

    /// Table and view names, optionally restricted to one schema.
    async fn get_tables_and_views(
        &mut self,
        schema_filter: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn StdError>>;

    fn get_database_type(&self) -> DatabaseType;

    fn get_current_db(&self) -> String;

    async fn get_table_details_in_schema(
        &mut self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, Box<dyn StdError>>;

    /// Details for many tables; `None` for a table that could not be read.
    async fn get_table_details_bulk(
        &self,
        table_names: &[String],
    ) -> Vec<(String, Option<TableDetails>)>;

    async fn get_schemas(&mut self) -> Result<Vec<String>, Box<dyn StdError>>;

    /// The underlying driver, for server-version lookups. `None` when not connected.
    fn get_database_client(&self) -> Option<&dyn DatabaseClient>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database-free session: proves the assistant's schema code can be
    /// driven (and, later, tested outside the root crate) through the trait.
    #[derive(Default)]
    struct FakeSession {
        explain: bool,
        queries: Vec<String>,
    }

    #[async_trait(?Send)]
    impl AiSession for FakeSession {
        fn is_explain_mode(&self) -> bool {
            self.explain
        }

        fn toggle_explain_mode(&mut self) -> bool {
            self.explain = !self.explain;
            self.explain
        }

        async fn execute_query_with_interrupt_no_column_selection(
            &mut self,
            query: &str,
            _interrupt_flag: &Arc<AtomicBool>,
        ) -> Result<Vec<Vec<String>>, Box<dyn StdError>> {
            self.queries.push(query.to_string());
            Ok(vec![vec!["n".to_string()], vec!["1".to_string()]])
        }

        async fn get_tables_and_views(
            &mut self,
            _schema_filter: Option<&str>,
        ) -> Result<Vec<String>, Box<dyn StdError>> {
            Ok(vec!["users".to_string(), "orders".to_string()])
        }

        fn get_database_type(&self) -> DatabaseType {
            DatabaseType::SQLite
        }

        fn get_current_db(&self) -> String {
            "fake.db".to_string()
        }

        async fn get_table_details_in_schema(
            &mut self,
            table_name: &str,
            _schema: Option<&str>,
        ) -> Result<TableDetails, Box<dyn StdError>> {
            Err(format!("no details for {table_name}").into())
        }

        async fn get_table_details_bulk(
            &self,
            table_names: &[String],
        ) -> Vec<(String, Option<TableDetails>)> {
            table_names.iter().map(|t| (t.clone(), None)).collect()
        }

        async fn get_schemas(&mut self) -> Result<Vec<String>, Box<dyn StdError>> {
            Ok(vec!["main".to_string()])
        }

        fn get_database_client(&self) -> Option<&dyn DatabaseClient> {
            None
        }
    }

    #[tokio::test]
    async fn fake_session_drives_the_schema_context_builders() {
        let mut fake = FakeSession::default();
        let session: &mut dyn AiSession = &mut fake;

        let seed = crate::schema_context::build_agent_seed_context(session).await;
        assert!(seed.starts_with("Database: fake.db (SQLite"), "{seed}");
        assert!(seed.contains("Tables and views (2):"), "{seed}");
        assert!(seed.contains("  users"), "{seed}");

        let (ctx, cacheable) =
            crate::schema_context::build_schema_context(session, "list users", 10).await;
        assert!(ctx.contains("fake.db"), "{ctx}");
        assert!(cacheable, "a small database context is query-independent");
        assert!(
            ctx.contains("-- Table: users (details unavailable)"),
            "{ctx}"
        );
    }

    #[tokio::test]
    async fn fake_session_backs_the_tool_executor() {
        use crate::agent::{AgentToolExecutor, DbToolExecutor};
        use genai::chat::ToolCall;
        use std::sync::Mutex;

        let session: Arc<Mutex<dyn AiSession + Send>> =
            Arc::new(Mutex::new(FakeSession::default()));
        let executor = DbToolExecutor::new(session.clone(), Arc::new(AtomicBool::new(false)), 5);
        let call = ToolCall {
            call_id: "1".to_string(),
            fn_name: "list_tables".to_string(),
            fn_arguments: serde_json::json!({}),
            thought_signatures: None,
        };
        let outcome = executor.execute(&call).await;
        assert!(outcome.summary.contains("2 tables"), "{}", outcome.summary);

        let call = ToolCall {
            fn_name: "run_sql".to_string(),
            fn_arguments: serde_json::json!({"query": "SELECT 1"}),
            ..call
        };
        let outcome = executor.execute(&call).await;
        assert!(outcome.summary.contains("rows"), "{}", outcome.summary);
        // The explain-mode toggle must have been restored
        assert!(!session.lock().unwrap().is_explain_mode());
    }
}
