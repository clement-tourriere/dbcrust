//! AI integration module for DBCrust.
//!
//! Provider and model handling is delegated to the [`genai`] crate, which
//! supports 25+ providers (OpenAI, Anthropic, Gemini, Ollama, Groq, DeepSeek,
//! xAI, OpenRouter, Z.AI, GitHub Copilot, …) over their native protocols.
//! dbcrust only supplies a model string, optional custom endpoint, and an
//! [`key_storage`]-backed credential resolver.
//!
//! The [`generate`] / [`generate_stream`] helpers are deliberately task-agnostic
//! so AI assistance can be reused beyond text-to-SQL (query optimization,
//! error explanation, etc.).

pub mod config;
pub mod conversation;
pub mod key_storage;
pub mod prompt_templates;
pub mod schema_context;
pub mod streaming;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use thiserror::Error;
use tokio::sync::mpsc;

use self::config::AiConfig;

#[derive(Error, Debug)]
pub enum AiError {
    #[error("Missing API key for {0}. Run \\ai setup to configure.")]
    MissingApiKey(String),
    #[error("Request failed: {0}")]
    RequestFailed(String),
    #[error("AI not configured. Run \\ai setup to get started.")]
    NotConfigured,
    #[error("Key storage error: {0}")]
    KeyStorageError(String),
}

#[derive(Debug, Clone)]
pub enum AiStreamEvent {
    TextDelta(String),
    Done,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct AiResponse {
    pub content: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

/// Curated list of provider keys shown in the `\ai setup` wizard. This is a UX
/// convenience only — any `genai`-supported model string works via `\ai model`,
/// so this list does not need to be exhaustive or kept perfectly in sync.
pub fn suggested_providers() -> Vec<AdapterKind> {
    [
        "anthropic",
        "openai",
        "gemini",
        "ollama",
        "groq",
        "deepseek",
        "xai",
        "open_router",
        "zai",
        "github_copilot",
        "cohere",
        "together",
    ]
    .iter()
    .filter_map(|s| AdapterKind::from_lower_str(s))
    .collect()
}

/// Provider inferred from the configured model string (e.g. `claude-*` → Anthropic).
/// genai maps unrecognized names to Ollama (local); `unwrap_or` is just a safety net.
pub fn provider_for_model(model: &str) -> AdapterKind {
    AdapterKind::from_model(model).unwrap_or(AdapterKind::Ollama)
}

/// Build a `genai` client wired to dbcrust's credential chain (env → OS keyring
/// → encrypted file) and an optional custom endpoint.
pub fn build_client(ai_config: &AiConfig) -> Result<Client, AiError> {
    if !ai_config.enabled {
        return Err(AiError::NotConfigured);
    }

    // Pre-flight the API key: the auth resolver below maps failures to
    // Ok(None), so without this check a missing key surfaces later as a raw
    // provider error that never mentions `\ai setup`. Custom endpoints
    // (self-hosted gateways) may legitimately run keyless — skip for those.
    let has_custom_endpoint = ai_config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();
    let adapter = provider_for_model(&ai_config.model);
    if !has_custom_endpoint
        && key_storage::requires_api_key(adapter)
        && key_storage::resolve_api_key(adapter).is_err()
    {
        return Err(AiError::MissingApiKey(adapter.as_str().to_string()));
    }

    // Resolve API keys through dbcrust's own storage instead of genai's
    // default env-var lookup. Returning `Ok(None)` lets genai proceed without a
    // key (local providers like Ollama) or fall back to its own resolution.
    let auth_resolver = AuthResolver::from_resolver_fn(
        |model_iden: ModelIden| -> Result<Option<AuthData>, genai::resolver::Error> {
            match key_storage::resolve_api_key(model_iden.adapter_kind) {
                Ok(key) => Ok(Some(AuthData::from_single(key))),
                Err(_) => Ok(None),
            }
        },
    );

    let mut builder = Client::builder().with_auth_resolver(auth_resolver);

    // Override the endpoint for self-hosted / OpenAI-compatible gateways.
    if let Some(endpoint) = ai_config.endpoint.clone().filter(|s| !s.trim().is_empty()) {
        let target_resolver = ServiceTargetResolver::from_resolver_fn(
            move |mut target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
                target.endpoint = Endpoint::from_owned(endpoint.clone());
                Ok(target)
            },
        );
        builder = builder.with_service_target_resolver(target_resolver);
    }

    Ok(builder.build())
}

fn build_request(system_prompt: &str, messages: &[(MessageRole, String)]) -> ChatRequest {
    let mut req = ChatRequest::default().with_system(system_prompt);
    for (role, content) in messages {
        let msg = match role {
            MessageRole::System => ChatMessage::system(content.clone()),
            MessageRole::User => ChatMessage::user(content.clone()),
            MessageRole::Assistant => ChatMessage::assistant(content.clone()),
        };
        req = req.append_message(msg);
    }
    req
}

fn chat_options(ai_config: &AiConfig) -> ChatOptions {
    ChatOptions::default()
        .with_max_tokens(ai_config.max_tokens)
        .with_temperature(ai_config.temperature as f64)
}

/// Non-streaming completion. Reusable for any AI task (not just SQL).
pub async fn generate(
    ai_config: &AiConfig,
    system_prompt: &str,
    messages: &[(MessageRole, String)],
) -> Result<AiResponse, AiError> {
    let client = build_client(ai_config)?;
    let req = build_request(system_prompt, messages);
    let opts = chat_options(ai_config);

    let res = client
        .exec_chat(ai_config.model.as_str(), req, Some(&opts))
        .await
        .map_err(|e| AiError::RequestFailed(e.to_string()))?;

    Ok(AiResponse {
        content: res.first_text().unwrap_or_default().to_string(),
        model: ai_config.model.clone(),
    })
}

/// Streaming completion. Sends [`AiStreamEvent`]s on `tx` as text arrives.
/// Reusable for any AI task (not just SQL).
pub async fn generate_stream(
    ai_config: &AiConfig,
    system_prompt: &str,
    messages: &[(MessageRole, String)],
    tx: mpsc::Sender<AiStreamEvent>,
) -> Result<(), AiError> {
    use futures_util::StreamExt;

    let client = build_client(ai_config)?;
    let req = build_request(system_prompt, messages);
    let opts = chat_options(ai_config);

    let chat_res = client
        .exec_chat_stream(ai_config.model.as_str(), req, Some(&opts))
        .await
        .map_err(|e| AiError::RequestFailed(e.to_string()))?;

    let mut stream = chat_res.stream;
    while let Some(event) = stream.next().await {
        match event {
            Ok(ChatStreamEvent::Chunk(chunk)) => {
                let _ = tx.send(AiStreamEvent::TextDelta(chunk.content)).await;
            }
            Ok(ChatStreamEvent::End(_)) => {
                let _ = tx.send(AiStreamEvent::Done).await;
                return Ok(());
            }
            // Start / ReasoningChunk / other events carry no assistant text.
            Ok(_) => {}
            Err(e) => {
                let _ = tx.send(AiStreamEvent::Error(e.to_string())).await;
                return Err(AiError::RequestFailed(e.to_string()));
            }
        }
    }

    let _ = tx.send(AiStreamEvent::Done).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suggested_providers_resolve() {
        // Guards against genai renaming the lowercase provider keys we pass to
        // AdapterKind::from_lower_str — every curated key must still resolve.
        let providers = suggested_providers();
        assert_eq!(
            providers.len(),
            12,
            "a curated provider key failed to resolve"
        );
        assert!(providers.contains(&AdapterKind::Anthropic));
        assert!(providers.contains(&AdapterKind::OpenAI));
    }

    #[test]
    fn test_provider_for_model_inference() {
        assert_eq!(
            provider_for_model("claude-sonnet-4-6"),
            AdapterKind::Anthropic
        );
        assert_eq!(provider_for_model("gpt-4o"), AdapterKind::OpenAI);
        // genai treats unrecognized model names as local Ollama models. For a
        // custom OpenAI-compatible endpoint, use `openai::model` or set `endpoint`.
        assert_eq!(provider_for_model("some-custom-model"), AdapterKind::Ollama);
    }

    #[test]
    fn test_build_client_disabled_is_not_configured() {
        let config = AiConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(matches!(build_client(&config), Err(AiError::NotConfigured)));
    }
}
