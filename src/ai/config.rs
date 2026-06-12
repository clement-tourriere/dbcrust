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

/// How requests authenticate to the provider.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AiAuthMethod {
    /// Per-provider API key resolved via env var → OS keyring → encrypted file.
    #[default]
    ApiKey,
    /// OpenAI only: "Sign in with ChatGPT" — requests ride the user's ChatGPT
    /// plan through the Codex backend instead of a pay-per-use API key.
    /// Configured with `\ai login`.
    ChatgptSubscription,
}

impl std::fmt::Display for AiAuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiAuthMethod::ApiKey => write!(f, "api_key"),
            AiAuthMethod::ChatgptSubscription => write!(f, "chatgpt_subscription"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AiConfig {
    #[serde(default = "default_ai_enabled")]
    pub enabled: bool,

    /// Provider key (any genai adapter key: `anthropic`, `openai`, `gemini`,
    /// `ollama`, …), or `"auto"` to infer the provider from the model name —
    /// the pre-existing behavior, kept as the serde default for old configs.
    #[serde(default = "default_provider")]
    pub provider: String,

    /// Model identifier passed to the provider library. With `provider = "auto"`
    /// the provider is inferred from the model name (`claude-*` → Anthropic,
    /// `gpt-*` → OpenAI, …); `provider::model` syntax also still works
    /// (e.g. `groq::llama-3.1-70b`).
    #[serde(default = "default_model")]
    pub model: String,

    /// How to authenticate (api_key | chatgpt_subscription).
    #[serde(default)]
    pub auth_method: AiAuthMethod,

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
            provider: default_provider(),
            model: default_model(),
            auth_method: AiAuthMethod::default(),
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

fn default_provider() -> String {
    "auto".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_old_config_without_new_fields_defaults() {
        // Configs written before provider/auth_method existed must keep the
        // legacy behavior: provider inferred from model, API-key auth.
        let config: AiConfig = toml::from_str("enabled = true").unwrap();
        assert_eq!(config.provider, "auto");
        assert_eq!(config.auth_method, AiAuthMethod::ApiKey);
        assert_eq!(config.model, "claude-sonnet-4-6");
    }

    #[test]
    fn test_auth_method_round_trip() {
        let config: AiConfig = toml::from_str("auth_method = \"chatgpt_subscription\"").unwrap();
        assert_eq!(config.auth_method, AiAuthMethod::ChatgptSubscription);
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("auth_method = \"chatgpt_subscription\""));
        // Display must match the serde representation — \config relies on it.
        assert_eq!(
            AiAuthMethod::ChatgptSubscription.to_string(),
            "chatgpt_subscription"
        );
        assert_eq!(AiAuthMethod::ApiKey.to_string(), "api_key");
    }
}
