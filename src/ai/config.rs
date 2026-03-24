//! AI configuration types for DBCrust.
//!
//! Provider and model handling is delegated to the `genai` crate, so dbcrust
//! does not maintain its own provider enum or model lists. The configuration
//! here is intentionally minimal: a model identifier (which `genai` maps to a
//! provider), an optional custom endpoint, and behaviour toggles.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiExecutionMode {
    #[default]
    Confirm,
    AutoSelect,
    AutoExecute,
}

impl std::fmt::Display for AiExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiExecutionMode::Confirm => write!(f, "confirm"),
            AiExecutionMode::AutoSelect => write!(f, "auto_select"),
            AiExecutionMode::AutoExecute => write!(f, "auto_execute"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AiConfig {
    #[serde(default = "default_ai_enabled")]
    pub enabled: bool,

    /// Model identifier passed to the provider library. The provider is inferred
    /// from the model name (`claude-*` → Anthropic, `gpt-*` → OpenAI, …), or can
    /// be forced with `provider::model` syntax (e.g. `groq::llama-3.1-70b`).
    #[serde(default = "default_model")]
    pub model: String,

    /// Optional custom endpoint base URL — for self-hosted gateways, Ollama,
    /// LM Studio, or any OpenAI-compatible service. Empty uses the provider default.
    #[serde(default)]
    pub endpoint: Option<String>,

    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    #[serde(default = "default_temperature")]
    pub temperature: f32,

    #[serde(default = "default_streaming")]
    pub streaming: bool,

    #[serde(default = "default_max_schema_tables")]
    pub max_schema_tables: usize,

    #[serde(default = "default_show_generated_sql")]
    pub show_generated_sql: bool,

    #[serde(default)]
    pub execution_mode: AiExecutionMode,

    #[serde(default = "default_history_length")]
    pub history_length: usize,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            enabled: default_ai_enabled(),
            model: default_model(),
            endpoint: None,
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            streaming: default_streaming(),
            max_schema_tables: default_max_schema_tables(),
            show_generated_sql: default_show_generated_sql(),
            execution_mode: AiExecutionMode::default(),
            history_length: default_history_length(),
        }
    }
}

/// Opt-in: AI features are disabled until the user runs `\ai setup` or `\ai on`.
fn default_ai_enabled() -> bool {
    false
}

fn default_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_temperature() -> f32 {
    0.0
}

fn default_streaming() -> bool {
    true
}

fn default_max_schema_tables() -> usize {
    50
}

fn default_show_generated_sql() -> bool {
    true
}

fn default_history_length() -> usize {
    5
}
