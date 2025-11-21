//! Configuration for AI SQL generation

use serde::{Deserialize, Serialize};
use std::env;

/// AI provider type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProviderType {
    Anthropic,
    OpenAI,
    Ollama,
    Custom,
}

impl Default for AiProviderType {
    fn default() -> Self {
        Self::Anthropic
    }
}

/// Schema context depth
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaDepth {
    /// Only table names
    Minimal,
    /// Table and column names
    TablesOnly,
    /// Full schema with indexes, constraints, relationships
    Full,
}

impl Default for SchemaDepth {
    fn default() -> Self {
        Self::Full
    }
}

/// AI interaction mode
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InteractionMode {
    /// Interactive mode with confirmation
    Interactive,
    /// Quick generation and display
    Quick,
    /// Multi-turn chat mode
    Chat,
}

impl Default for InteractionMode {
    fn default() -> Self {
        Self::Interactive
    }
}

/// Configuration for AI SQL generation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSqlConfig {
    /// Enable AI SQL feature
    pub enabled: bool,

    /// AI provider to use
    pub provider: AiProviderType,

    // === Anthropic Configuration ===
    /// Prefer OAuth authentication over API key
    pub anthropic_use_oauth: bool,

    /// Anthropic API key (can also use ANTHROPIC_API_KEY env var)
    /// Falls back to this if OAuth is not configured or fails
    pub anthropic_api_key: Option<String>,

    /// Anthropic model name
    pub anthropic_model: String,

    /// Anthropic base URL (for custom endpoints)
    pub anthropic_base_url: String,

    // === OpenAI Configuration ===
    /// OpenAI API key (can also use OPENAI_API_KEY env var)
    pub openai_api_key: Option<String>,

    /// OpenAI model name
    pub openai_model: String,

    /// OpenAI base URL (for custom endpoints)
    pub openai_base_url: String,

    // === Ollama Configuration ===
    /// Ollama base URL
    pub ollama_base_url: String,

    /// Ollama model name
    pub ollama_model: String,

    // === Custom Endpoint Configuration ===
    /// Custom API base URL
    pub custom_base_url: Option<String>,

    /// Custom API key
    pub custom_api_key: Option<String>,

    /// Custom model name
    pub custom_model: Option<String>,

    // === Fallback Configuration ===
    /// Fallback providers to try in order
    pub fallback_providers: Vec<AiProviderType>,

    // === Generation Parameters ===
    /// Temperature (0.0 = deterministic, 1.0 = creative)
    pub temperature: f32,

    /// Maximum tokens to generate
    pub max_tokens: u32,

    /// Request timeout in seconds
    pub timeout_seconds: u64,

    // === Schema Context Configuration ===
    /// Include schema context in prompts
    pub include_schema: bool,

    /// Schema depth to include
    pub schema_depth: SchemaDepth,

    /// Maximum number of tables to include
    pub max_tables: usize,

    /// Include index information
    pub include_indexes: bool,

    /// Include constraint information
    pub include_constraints: bool,

    /// Include sample data rows (privacy risk!)
    pub include_sample_data: bool,

    /// Number of sample rows if enabled
    pub sample_data_rows: usize,

    // === Query Refinement Configuration ===
    /// Enable query refinement
    pub enable_refinement: bool,

    /// Maximum refinement iterations
    pub max_refinement_iterations: usize,

    // === Caching Configuration ===
    /// Enable query caching
    pub cache_enabled: bool,

    /// Cache TTL in seconds
    pub cache_ttl_seconds: u64,

    // === UI Preferences ===
    /// Default interaction mode
    pub default_mode: InteractionMode,

    /// Auto-execute generated queries
    pub auto_execute: bool,

    /// Show AI explanation of generated SQL
    pub show_explanation: bool,

    /// Syntax highlighting for generated SQL
    pub syntax_highlighting: bool,

    /// Show optimization hints
    pub show_optimization_hints: bool,
}

impl Default for AiSqlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: AiProviderType::Anthropic,

            // Anthropic defaults
            anthropic_use_oauth: true, // Prefer OAuth over API key
            anthropic_api_key: None,
            anthropic_model: "claude-sonnet-4-5-20250929".to_string(),
            anthropic_base_url: "https://api.anthropic.com".to_string(),

            // OpenAI defaults
            openai_api_key: None,
            openai_model: "gpt-4".to_string(),
            openai_base_url: "https://api.openai.com/v1".to_string(),

            // Ollama defaults
            ollama_base_url: "http://localhost:11434".to_string(),
            ollama_model: "codellama:70b".to_string(),

            // Custom endpoint defaults
            custom_base_url: None,
            custom_api_key: None,
            custom_model: None,

            // Fallback chain
            fallback_providers: vec![
                AiProviderType::Anthropic,
                AiProviderType::OpenAI,
                AiProviderType::Ollama,
            ],

            // Generation parameters
            temperature: 0.0, // Deterministic for SQL generation
            max_tokens: 4096,
            timeout_seconds: 30,

            // Schema context
            include_schema: true,
            schema_depth: SchemaDepth::Full,
            max_tables: 50,
            include_indexes: true,
            include_constraints: true,
            include_sample_data: false, // Privacy default
            sample_data_rows: 3,

            // Refinement
            enable_refinement: true,
            max_refinement_iterations: 3,

            // Caching
            cache_enabled: true,
            cache_ttl_seconds: 3600, // 1 hour

            // UI preferences
            default_mode: InteractionMode::Interactive,
            auto_execute: false, // Safety default
            show_explanation: true,
            syntax_highlighting: true,
            show_optimization_hints: true,
        }
    }
}

impl AiSqlConfig {
    /// Get Anthropic API key from config or environment
    pub fn get_anthropic_api_key(&self) -> Option<String> {
        self.anthropic_api_key
            .clone()
            .or_else(|| env::var("ANTHROPIC_API_KEY").ok())
    }

    /// Get OpenAI API key from config or environment
    pub fn get_openai_api_key(&self) -> Option<String> {
        self.openai_api_key
            .clone()
            .or_else(|| env::var("OPENAI_API_KEY").ok())
    }

    /// Get custom API key from config or environment
    pub fn get_custom_api_key(&self) -> Option<String> {
        self.custom_api_key
            .clone()
            .or_else(|| env::var("CUSTOM_AI_API_KEY").ok())
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Err("AI SQL feature is disabled".to_string());
        }

        // Validate provider-specific configuration
        match self.provider {
            AiProviderType::Anthropic => {
                if self.get_anthropic_api_key().is_none() {
                    return Err(
                        "Anthropic API key not found. Set ANTHROPIC_API_KEY environment variable or configure in config file"
                            .to_string(),
                    );
                }
            }
            AiProviderType::OpenAI => {
                if self.get_openai_api_key().is_none() {
                    return Err(
                        "OpenAI API key not found. Set OPENAI_API_KEY environment variable or configure in config file"
                            .to_string(),
                    );
                }
            }
            AiProviderType::Ollama => {
                // Ollama doesn't require API key, but validate URL
                if self.ollama_base_url.is_empty() {
                    return Err("Ollama base URL is required".to_string());
                }
            }
            AiProviderType::Custom => {
                if self.custom_base_url.is_none() {
                    return Err("Custom base URL is required for custom provider".to_string());
                }
            }
        }

        // Validate generation parameters
        if self.temperature < 0.0 || self.temperature > 1.0 {
            return Err("Temperature must be between 0.0 and 1.0".to_string());
        }

        if self.max_tokens == 0 {
            return Err("max_tokens must be greater than 0".to_string());
        }

        if self.timeout_seconds == 0 {
            return Err("timeout_seconds must be greater than 0".to_string());
        }

        if self.max_tables == 0 {
            return Err("max_tables must be greater than 0".to_string());
        }

        Ok(())
    }

    /// Get documentation for configuration fields
    pub fn documentation() -> Vec<(&'static str, &'static str)> {
        vec![
            ("[ai_sql]", "AI-powered SQL generation from natural language"),
            ("enabled", "Enable AI SQL feature (default: true)"),
            ("provider", "AI provider: anthropic, openai, ollama, custom (default: anthropic)"),
            ("", ""),
            ("# Anthropic Configuration", ""),
            ("anthropic_use_oauth", "Prefer OAuth authentication over API key (default: true)"),
            ("anthropic_api_key", "Anthropic API key - fallback if OAuth not configured (or use ANTHROPIC_API_KEY env var)"),
            ("anthropic_model", "Model name (default: claude-sonnet-4-5-20250929). Use \\aimodel to see available models"),
            ("anthropic_base_url", "Base URL for Anthropic API (default: https://api.anthropic.com)"),
            ("", ""),
            ("# OAuth Authentication", ""),
            ("", "To authenticate with your Anthropic subscription, use: \\aiauth"),
            ("", "To logout: \\ailogout"),
            ("", "OAuth tokens are stored securely and refreshed automatically"),
            ("", ""),
            ("# OpenAI Configuration", ""),
            ("openai_api_key", "OpenAI API key (or use OPENAI_API_KEY env var)"),
            ("openai_model", "Model name (default: gpt-4)"),
            ("openai_base_url", "Base URL for OpenAI API (default: https://api.openai.com/v1)"),
            ("", ""),
            ("# Ollama Configuration (local models)", ""),
            ("ollama_base_url", "Ollama server URL (default: http://localhost:11434)"),
            ("ollama_model", "Model name (default: codellama:70b)"),
            ("", ""),
            ("# Generation Parameters", ""),
            ("temperature", "Creativity (0.0 = deterministic, 1.0 = creative, default: 0.0)"),
            ("max_tokens", "Maximum tokens to generate (default: 4096)"),
            ("timeout_seconds", "Request timeout in seconds (default: 30)"),
            ("", ""),
            ("# Schema Context", ""),
            ("include_schema", "Include schema in AI context (default: true)"),
            ("schema_depth", "Schema detail level: minimal, tables_only, full (default: full)"),
            ("max_tables", "Maximum tables to include (default: 50)"),
            ("include_indexes", "Include index information (default: true)"),
            ("include_constraints", "Include constraints (default: true)"),
            ("include_sample_data", "Include sample rows - privacy risk! (default: false)"),
            ("", ""),
            ("# UI Preferences", ""),
            ("default_mode", "Interaction mode: interactive, quick, chat (default: interactive)"),
            ("auto_execute", "Auto-execute queries without confirmation (default: false)"),
            ("show_explanation", "Show AI explanation (default: true)"),
            ("syntax_highlighting", "Enable syntax highlighting (default: true)"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AiSqlConfig::default();
        assert!(config.enabled);
        assert_eq!(config.provider, AiProviderType::Anthropic);
        assert_eq!(config.temperature, 0.0);
        assert!(!config.auto_execute);
    }

    #[test]
    fn test_validation() {
        let mut config = AiSqlConfig::default();

        // Should fail without API key
        assert!(config.validate().is_err());

        // Set API key
        config.anthropic_api_key = Some("test-key".to_string());
        assert!(config.validate().is_ok());

        // Invalid temperature
        config.temperature = 2.0;
        assert!(config.validate().is_err());

        config.temperature = 0.5;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_api_key_from_env() {
        env::set_var("ANTHROPIC_API_KEY", "env-key");

        let config = AiSqlConfig::default();
        assert_eq!(config.get_anthropic_api_key(), Some("env-key".to_string()));

        env::remove_var("ANTHROPIC_API_KEY");
    }
}
