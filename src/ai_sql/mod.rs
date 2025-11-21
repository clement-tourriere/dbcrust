//! AI-powered SQL generation from natural language
//!
//! This module provides intelligent SQL query generation from natural language prompts,
//! with support for multiple database systems and AI providers.
//!
//! # Features
//!
//! - Multi-database SQL dialect support (PostgreSQL, MySQL, SQLite, ClickHouse, etc.)
//! - Multiple AI providers (Anthropic Claude, OpenAI, Ollama, custom endpoints)
//! - Intelligent schema discovery and context extraction
//! - Interactive query refinement workflow
//! - Query caching for performance
//! - MongoDB aggregation pipeline generation
//!
//! # Usage
//!
//! ```rust
//! use dbcrust::ai_sql::{AiSqlEngine, AiSqlConfig};
//!
//! let config = AiSqlConfig::default();
//! let engine = AiSqlEngine::new(config, database);
//! let result = engine.generate_sql("top 10 users by orders").await?;
//! ```

pub mod cache;
pub mod client;
pub mod config;
pub mod dialect;
pub mod error;
pub mod oauth;
pub mod oauth_pkce;
pub mod prompt;
pub mod schema;
pub mod ui;

pub use cache::QueryCache;
pub use client::{AiProvider, AiResponse};
pub use config::AiSqlConfig;
pub use dialect::{SqlDialectProvider, SqlFeatures};
pub use error::{AiError, AiResult};
pub use oauth::{AnthropicOAuthManager, OAuthToken};
pub use oauth_pkce::{AnthropicOAuthPkce, PkceChallenge};
pub use prompt::PromptGenerator;
pub use schema::{SchemaContext, SchemaExtractor};
pub use ui::InteractiveMode;

use crate::db::Database;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

/// Main AI SQL engine that orchestrates SQL generation
pub struct AiSqlEngine {
    config: AiSqlConfig,
    database: Arc<Mutex<Database>>,
    schema_extractor: SchemaExtractor,
    query_cache: QueryCache,
    ai_client: Box<dyn AiProvider>,
    dialect_provider: Box<dyn SqlDialectProvider>,
}

impl AiSqlEngine {
    /// Create a new AI SQL engine
    pub async fn new(
        config: AiSqlConfig,
        database: Arc<Mutex<Database>>,
    ) -> AiResult<Self> {
        let database_type = {
            let db = database.lock().unwrap();
            db.get_connection_info()
                .map(|info| info.database_type.clone())
                .ok_or_else(|| AiError::SchemaError("No connection info available".to_string()))?
        };

        // Create AI client based on configuration
        let ai_client = client::create_ai_client(&config)?;

        // Create dialect provider for the database type
        let dialect_provider = dialect::create_dialect_provider(database_type);

        Ok(Self {
            config,
            database: Arc::clone(&database),
            schema_extractor: SchemaExtractor::new(),
            query_cache: QueryCache::new(),
            ai_client,
            dialect_provider,
        })
    }

    /// Generate SQL from natural language prompt
    pub async fn generate_sql(&mut self, prompt: &str) -> AiResult<AiResponse> {
        info!("Generating SQL for prompt: {}", prompt);

        // Check cache first
        if self.config.cache_enabled {
            let cache_key = self.generate_cache_key(prompt);
            if let Some(cached) = self.query_cache.get(&cache_key) {
                debug!("Cache hit for prompt");
                return Ok(cached);
            }
        }

        // Extract schema context
        let schema_context = self
            .schema_extractor
            .extract_context(&self.database, Some(prompt))
            .await?;

        // Generate SQL using AI provider
        let response = self
            .ai_client
            .generate_sql(
                prompt,
                &schema_context,
                self.dialect_provider.as_ref(),
                &self.config,
            )
            .await?;

        // Cache the result
        if self.config.cache_enabled {
            let cache_key = self.generate_cache_key(prompt);
            self.query_cache.insert(cache_key, response.clone());
        }

        Ok(response)
    }

    /// Refine existing SQL with additional feedback
    pub async fn refine_sql(
        &mut self,
        original_sql: &str,
        feedback: &str,
    ) -> AiResult<AiResponse> {
        info!("Refining SQL with feedback: {}", feedback);

        let schema_context = self
            .schema_extractor
            .extract_context(&self.database, None)
            .await?;

        self.ai_client
            .refine_sql(
                original_sql,
                feedback,
                &schema_context,
                self.dialect_provider.as_ref(),
                &self.config,
            )
            .await
    }

    /// Start interactive chat mode for complex query building
    pub async fn interactive_chat(&mut self) -> AiResult<()> {
        let mut interactive = InteractiveMode::new(
            Arc::clone(&self.database),
            &mut self.ai_client,
            self.dialect_provider.as_ref(),
            &self.config,
        );

        interactive.run().await
    }

    /// Clear the query cache
    pub fn clear_cache(&mut self) {
        self.query_cache.clear();
        info!("Query cache cleared");
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        self.query_cache.stats()
    }

    /// Generate cache key from prompt and database context
    fn generate_cache_key(&self, prompt: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let db_type = self.dialect_provider.database_type();
        let db_name = {
            let db = self.database.lock().unwrap();
            db.get_current_db()
        };

        let mut hasher = DefaultHasher::new();
        prompt.hash(&mut hasher);
        db_type.to_string().hash(&mut hasher);
        db_name.hash(&mut hasher);

        format!("{:x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_structure() {
        // Ensure all submodules are accessible
        let _ = AiSqlConfig::default();
        let _ = AiError::ConfigurationError("test".to_string());
    }
}
