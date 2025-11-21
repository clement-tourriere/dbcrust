//! AI provider client implementations

use crate::ai_sql::config::{AiProviderType, AiSqlConfig};
use crate::ai_sql::dialect::SqlDialectProvider;
use crate::ai_sql::error::{AiError, AiResult};
use crate::ai_sql::oauth_pkce::AnthropicOAuthPkce;
use crate::ai_sql::schema::SchemaContext;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, info, warn};

/// AI response containing generated SQL and metadata
#[derive(Debug, Clone)]
pub struct AiResponse {
    pub sql: String,
    pub explanation: Option<String>,
    pub confidence: f32,
    pub warnings: Vec<String>,
    pub suggestions: Vec<String>,
}

/// AI model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiModel {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
}

/// Trait for AI providers
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Generate SQL from natural language prompt
    async fn generate_sql(
        &self,
        prompt: &str,
        schema_context: &SchemaContext,
        dialect: &dyn SqlDialectProvider,
        config: &AiSqlConfig,
    ) -> AiResult<AiResponse>;

    /// Refine existing SQL with feedback
    async fn refine_sql(
        &self,
        original_sql: &str,
        feedback: &str,
        schema_context: &SchemaContext,
        dialect: &dyn SqlDialectProvider,
        config: &AiSqlConfig,
    ) -> AiResult<AiResponse>;

    /// Get provider name
    fn name(&self) -> &str;

    /// Check if provider supports streaming
    fn supports_streaming(&self) -> bool {
        false
    }

    /// List available models for this provider
    async fn list_models(&self) -> AiResult<Vec<AiModel>>;
}

/// Authentication method for Anthropic
#[derive(Debug, Clone)]
enum AuthMethod {
    ApiKey(String),
    OAuth,
}

/// Anthropic Claude provider implementation
pub struct AnthropicProvider {
    client: Client,
    auth_method: AuthMethod,
    oauth_manager: Option<AnthropicOAuthPkce>,
    base_url: String,
    model: String,
}

impl AnthropicProvider {
    /// Create provider with API key
    pub fn new(api_key: String, base_url: String, model: String) -> AiResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| AiError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            auth_method: AuthMethod::ApiKey(api_key),
            oauth_manager: None,
            base_url,
            model,
        })
    }

    /// Create provider with OAuth
    pub fn new_with_oauth(
        config_dir: PathBuf,
        base_url: String,
        model: String,
    ) -> AiResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| AiError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        let oauth_manager = AnthropicOAuthPkce::new(config_dir)?;

        Ok(Self {
            client,
            auth_method: AuthMethod::OAuth,
            oauth_manager: Some(oauth_manager),
            base_url,
            model,
        })
    }

    /// Get authentication token (OAuth or API key)
    async fn get_auth_token(&self) -> AiResult<String> {
        match &self.auth_method {
            AuthMethod::ApiKey(key) => Ok(key.clone()),
            AuthMethod::OAuth => {
                let manager = self
                    .oauth_manager
                    .as_ref()
                    .ok_or_else(|| AiError::ConfigurationError("OAuth manager not initialized".to_string()))?;

                manager.get_valid_token().await
            }
        }
    }

    async fn call_api(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> AiResult<String> {
        let url = format!("{}/v1/messages", self.base_url);

        let request_body = AnthropicRequest {
            model: self.model.clone(),
            max_tokens,
            temperature,
            system: Some(system_prompt.to_string()),
            messages: vec![Message {
                role: "user".to_string(),
                content: user_prompt.to_string(),
            }],
        };

        debug!(
            "Calling Anthropic API with model: {}, max_tokens: {}, temperature: {}",
            self.model, max_tokens, temperature
        );

        // Get auth token (OAuth or API key)
        let auth_token = self.get_auth_token().await?;

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &auth_token)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: error_text,
            });
        }

        let response_body: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| AiError::ProviderError(format!("Failed to parse API response: {}", e)))?;

        // Extract text content from the first content block
        if let Some(content) = response_body.content.first() {
            Ok(content.text.clone())
        } else {
            Err(AiError::ProviderError(
                "No content in response".to_string(),
            ))
        }
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    async fn generate_sql(
        &self,
        prompt: &str,
        schema_context: &SchemaContext,
        dialect: &dyn SqlDialectProvider,
        config: &AiSqlConfig,
    ) -> AiResult<AiResponse> {
        info!("Generating SQL with Anthropic Claude");

        // Build system prompt
        let system_prompt = dialect.system_prompt();

        // Build user prompt with schema context
        let schema_info = if config.include_schema {
            dialect.format_schema_context(schema_context)
        } else {
            format!("Database: {}", schema_context.current_database)
        };

        let user_prompt = format!(
            "{}\n\nUser Query: {}\n\nGenerate the SQL query to answer this request.",
            schema_info, prompt
        );

        debug!("System prompt length: {} chars", system_prompt.len());
        debug!("User prompt length: {} chars", user_prompt.len());

        // Call API
        let sql = self
            .call_api(
                &system_prompt,
                &user_prompt,
                config.temperature,
                config.max_tokens,
            )
            .await?;

        // Clean up the SQL (remove markdown, extra whitespace)
        let cleaned_sql = Self::clean_sql_response(&sql);

        // Basic validation
        dialect
            .validate_sql(&cleaned_sql)
            .map_err(|e| AiError::ValidationError(e))?;

        Ok(AiResponse {
            sql: cleaned_sql,
            explanation: None,
            confidence: 0.85, // Default confidence for Anthropic
            warnings: vec![],
            suggestions: vec![],
        })
    }

    async fn refine_sql(
        &self,
        original_sql: &str,
        feedback: &str,
        schema_context: &SchemaContext,
        dialect: &dyn SqlDialectProvider,
        config: &AiSqlConfig,
    ) -> AiResult<AiResponse> {
        info!("Refining SQL with Anthropic Claude");

        let system_prompt = dialect.system_prompt();

        let schema_info = if config.include_schema {
            dialect.format_schema_context(schema_context)
        } else {
            format!("Database: {}", schema_context.current_database)
        };

        let user_prompt = format!(
            "{}\n\nOriginal SQL:\n{}\n\nUser Feedback: {}\n\nGenerate the refined SQL query based on the feedback.",
            schema_info, original_sql, feedback
        );

        let sql = self
            .call_api(
                &system_prompt,
                &user_prompt,
                config.temperature,
                config.max_tokens,
            )
            .await?;

        let cleaned_sql = Self::clean_sql_response(&sql);

        dialect
            .validate_sql(&cleaned_sql)
            .map_err(|e| AiError::ValidationError(e))?;

        Ok(AiResponse {
            sql: cleaned_sql,
            explanation: None,
            confidence: 0.85,
            warnings: vec![],
            suggestions: vec![],
        })
    }

    fn name(&self) -> &str {
        "Anthropic Claude"
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn list_models(&self) -> AiResult<Vec<AiModel>> {
        info!("Fetching available models from Anthropic API");

        // Get auth token (OAuth or API key)
        let auth_token = self.get_auth_token().await?;

        // Call Anthropic /v1/models API
        let response = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &auth_token)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Failed to fetch models: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: format!("Failed to fetch models: {}", error_text),
            });
        }

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelData>,
        }

        #[derive(Deserialize)]
        struct ModelData {
            id: String,
            display_name: String,
        }

        let models_response: ModelsResponse = response.json().await.map_err(|e| {
            AiError::ProviderError(format!("Failed to parse models response: {}", e))
        })?;

        let models: Vec<AiModel> = models_response
            .data
            .into_iter()
            .map(|m| {
                // Add descriptions based on model family
                let description = if m.id.contains("sonnet-4") {
                    Some("Smartest model for complex agents and coding".to_string())
                } else if m.id.contains("haiku-4") {
                    Some("Fastest with near-frontier intelligence".to_string())
                } else if m.id.contains("opus-4") {
                    Some("Exceptional for specialized reasoning tasks".to_string())
                } else {
                    None
                };

                AiModel {
                    id: m.id,
                    display_name: m.display_name,
                    description,
                }
            })
            .collect();

        info!("Successfully fetched {} models", models.len());
        Ok(models)
    }
}

impl AnthropicProvider {
    /// Clean SQL response by removing markdown code blocks and extra whitespace
    fn clean_sql_response(sql: &str) -> String {
        let mut cleaned = sql.trim().to_string();

        // Remove markdown code blocks
        if cleaned.starts_with("```sql") {
            cleaned = cleaned.strip_prefix("```sql").unwrap().to_string();
        }
        if cleaned.starts_with("```") {
            cleaned = cleaned.strip_prefix("```").unwrap().to_string();
        }
        if cleaned.ends_with("```") {
            cleaned = cleaned.strip_suffix("```").unwrap().to_string();
        }

        // Clean up whitespace
        cleaned = cleaned.trim().to_string();

        // Remove any leading/trailing semicolons if multiple
        while cleaned.ends_with(";;") {
            cleaned = cleaned.strip_suffix(";").unwrap().to_string();
        }

        cleaned
    }
}

// Anthropic API types
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    text: String,
}

/// Create AI client based on configuration
pub fn create_ai_client(config: &AiSqlConfig) -> AiResult<Box<dyn AiProvider>> {
    match config.provider {
        AiProviderType::Anthropic => {
            // Try OAuth first if enabled
            if config.anthropic_use_oauth {
                info!("Attempting to use OAuth authentication for Anthropic");

                // Get config directory for OAuth token storage
                match crate::config::Config::get_config_directory() {
                    Ok(config_dir) => {
                        match AnthropicProvider::new_with_oauth(
                            config_dir,
                            config.anthropic_base_url.clone(),
                            config.anthropic_model.clone(),
                        ) {
                            Ok(provider) => {
                                info!("Using OAuth authentication for Anthropic");
                                return Ok(Box::new(provider));
                            }
                            Err(e) => {
                                warn!("OAuth authentication failed: {}. Falling back to API key...", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to get config directory: {}. Falling back to API key...", e);
                    }
                }
            }

            // Fall back to API key
            if let Some(api_key) = config.get_anthropic_api_key() {
                info!("Using API key authentication for Anthropic");
                let provider = AnthropicProvider::new(
                    api_key,
                    config.anthropic_base_url.clone(),
                    config.anthropic_model.clone(),
                )?;

                Ok(Box::new(provider))
            } else {
                Err(AiError::ConfigurationError(
                    "Anthropic authentication not configured. Options:\n\
                     1. Use OAuth: Run \\aiauth command to authenticate with your subscription\n\
                     2. Use API Key: Set ANTHROPIC_API_KEY environment variable or add anthropic_api_key to config\n\
                     3. Disable OAuth: Set anthropic_use_oauth = false in config".to_string()
                ))
            }
        }
        AiProviderType::OpenAI => {
            // TODO: Implement OpenAI provider
            Err(AiError::ConfigurationError(
                "OpenAI provider not yet implemented".to_string(),
            ))
        }
        AiProviderType::Ollama => {
            // TODO: Implement Ollama provider
            Err(AiError::ConfigurationError(
                "Ollama provider not yet implemented".to_string(),
            ))
        }
        AiProviderType::Custom => {
            // TODO: Implement custom provider
            Err(AiError::ConfigurationError(
                "Custom provider not yet implemented".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_sql_response() {
        let test_cases = vec![
            (
                "```sql\nSELECT * FROM users;\n```",
                "SELECT * FROM users;",
            ),
            ("```\nSELECT * FROM users;\n```", "SELECT * FROM users;"),
            ("SELECT * FROM users;", "SELECT * FROM users;"),
            ("  SELECT * FROM users;  ", "SELECT * FROM users;"),
            ("SELECT * FROM users;;", "SELECT * FROM users;"),
        ];

        for (input, expected) in test_cases {
            let result = AnthropicProvider::clean_sql_response(input);
            assert_eq!(result, expected, "Failed for input: {}", input);
        }
    }
}
