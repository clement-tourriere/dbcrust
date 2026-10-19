//! `Database` as an [`AiSession`]: forwards to the inherent methods of the
//! same name. Lives next to the session type (this crate, feature `ai`) so the
//! assistant itself never has to name `Database`.

use std::error::Error as StdError;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;

use crate::db::{Database, TableDetails};
use dbcrust_ai::AiSession;
use dbcrust_core::database::{DatabaseClient, DatabaseType};

// Inherent methods take precedence over trait methods of the same name, so
// every `Database::x(self, ..)` below calls the real implementation.
#[async_trait(?Send)]
impl AiSession for Database {
    fn is_explain_mode(&self) -> bool {
        Database::is_explain_mode(self)
    }

    fn toggle_explain_mode(&mut self) -> bool {
        Database::toggle_explain_mode(self)
    }

    async fn execute_query_with_interrupt_no_column_selection(
        &mut self,
        query: &str,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<Vec<Vec<String>>, Box<dyn StdError>> {
        Database::execute_query_with_interrupt_no_column_selection(self, query, interrupt_flag)
            .await
    }

    async fn get_tables_and_views(
        &mut self,
        schema_filter: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn StdError>> {
        Database::get_tables_and_views(self, schema_filter).await
    }

    fn get_database_type(&self) -> DatabaseType {
        Database::get_database_type(self)
    }

    fn get_current_db(&self) -> String {
        Database::get_current_db(self)
    }

    async fn get_table_details_in_schema(
        &mut self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<TableDetails, Box<dyn StdError>> {
        Database::get_table_details_in_schema(self, table_name, schema).await
    }

    async fn get_table_details_bulk(
        &self,
        table_names: &[String],
    ) -> Vec<(String, Option<TableDetails>)> {
        Database::get_table_details_bulk(self, table_names).await
    }

    async fn get_schemas(&mut self) -> Result<Vec<String>, Box<dyn StdError>> {
        Database::get_schemas(self).await
    }

    fn get_database_client(&self) -> Option<&dyn DatabaseClient> {
        Database::get_database_client(self)
    }
}
