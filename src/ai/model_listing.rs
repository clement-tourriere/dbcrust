//! Live model listing for the `\ai setup` wizard and `\ai model` picker.
//!
//! Uses genai's `Client::all_model_names`, which queries the provider's
//! live `/models` endpoint with dbcrust's resolved credentials — so the list
//! reflects what the configured API key can actually access. Every failure
//! (no key, network down, provider without a listing endpoint) falls back to
//! a small curated suggestion list; free text is always allowed.

use std::time::Duration;

use genai::Client;
use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint, ProviderConfig};

use crate::ai::{AiError, key_storage};

/// Sentinel entry appended to every picker list — selecting it falls through
/// to a free-text prompt.
pub const CUSTOM_MODEL_OPTION: &str = "Enter custom model…";

/// Fetch the live model list for `adapter`, authenticating with dbcrust's
/// resolved key (env → keyring → encrypted file) and honoring a custom
/// endpoint when configured. Sorted and deduped.
pub async fn list_models(
    adapter: AdapterKind,
    endpoint: Option<&str>,
) -> Result<Vec<String>, AiError> {
    let auth = key_storage::resolve_api_key(adapter)
        .ok()
        .map(AuthData::from_single);
    let endpoint = endpoint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| Endpoint::from_owned(s.to_string()));
    let provider_config = ProviderConfig::from((endpoint, auth));

    let names = tokio::time::timeout(
        Duration::from_secs(10),
        Client::default().all_model_names(adapter, provider_config),
    )
    .await
    .map_err(|_| AiError::RequestFailed("model listing timed out".to_string()))?
    .map_err(|e| AiError::RequestFailed(e.to_string()))?;

    let mut names: Vec<String> = names
        .into_iter()
        .filter(|n| !is_obviously_non_chat(n))
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Err(AiError::RequestFailed(
            "provider returned no models".to_string(),
        ));
    }
    Ok(names)
}

/// Drop ids that clearly aren't chat models (OpenAI's /models mixes in
/// embeddings, audio, image and moderation endpoints). Conservative on
/// purpose: when in doubt, keep the id.
fn is_obviously_non_chat(name: &str) -> bool {
    const NON_CHAT_MARKERS: &[&str] = &[
        "embedding",
        "whisper",
        "tts-",
        "dall-e",
        "moderation",
        "audio",
        "transcribe",
        "image",
        "realtime",
    ];
    let lower = name.to_lowercase();
    NON_CHAT_MARKERS.iter().any(|m| lower.contains(m))
}

/// Curated fallback suggestions when live listing is unavailable.
/// Non-exhaustive by design — the picker always offers a custom entry.
pub fn curated_models(adapter: AdapterKind) -> Vec<&'static str> {
    match adapter {
        AdapterKind::Anthropic => vec!["claude-sonnet-4-6", "claude-opus-4-8", "claude-haiku-4-5"],
        AdapterKind::OpenAI | AdapterKind::OpenAIResp => {
            vec!["gpt-5.1", "gpt-5", "gpt-5-mini", "gpt-4o"]
        }
        AdapterKind::Gemini => vec!["gemini-2.5-pro", "gemini-2.5-flash"],
        AdapterKind::Ollama => vec!["llama3.1", "qwen2.5-coder", "mistral"],
        AdapterKind::Groq => vec!["llama-3.3-70b-versatile", "llama-3.1-8b-instant"],
        AdapterKind::DeepSeek => vec!["deepseek-chat", "deepseek-reasoner"],
        AdapterKind::Xai => vec!["grok-4", "grok-3-mini"],
        AdapterKind::Cohere => vec!["command-a-03-2025", "command-r-plus"],
        AdapterKind::OpenRouter => vec!["openai/gpt-5.1", "anthropic/claude-sonnet-4.6"],
        AdapterKind::Zai => vec!["glm-4.6"],
        AdapterKind::Together => vec!["meta-llama/Llama-3.3-70B-Instruct-Turbo"],
        AdapterKind::GithubCopilot => vec!["openai/gpt-4.1-mini"],
        _ => crate::ai::default_model_for(adapter)
            .map(|m| vec![m])
            .unwrap_or_default(),
    }
}

/// Models reachable through the ChatGPT-subscription (Codex backend) route.
/// That backend has no /models endpoint, so this curated set is the picker
/// source in subscription mode. Suggestions only — custom entry still works.
pub fn chatgpt_subscription_models() -> Vec<&'static str> {
    vec!["gpt-5.5", "gpt-5.5-codex", "gpt-5-codex", "codex-mini-latest"]
}

/// Assemble the picker options from a live result (or its failure) plus the
/// curated fallback and the currently configured model. Returns the options
/// and whether the curated fallback was used.
pub fn build_model_options(
    live: Result<Vec<String>, AiError>,
    curated: &[&str],
    current: &str,
) -> (Vec<String>, bool) {
    let (mut options, used_fallback) = match live {
        Ok(models) => (models, false),
        Err(_) => (curated.iter().map(|s| s.to_string()).collect(), true),
    };
    let current = current.trim();
    if !current.is_empty() && !options.iter().any(|m| m == current) {
        options.insert(0, current.to_string());
    }
    options.push(CUSTOM_MODEL_OPTION.to_string());
    (options, used_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_model_options_live() {
        let live = Ok(vec!["a-model".to_string(), "b-model".to_string()]);
        let (options, fallback) = build_model_options(live, &["curated"], "b-model");
        assert!(!fallback);
        // current model already present — not duplicated, sentinel last
        assert_eq!(options, vec!["a-model", "b-model", CUSTOM_MODEL_OPTION]);
    }

    #[test]
    fn test_build_model_options_fallback_injects_current() {
        let live = Err(AiError::RequestFailed("nope".to_string()));
        let (options, fallback) = build_model_options(live, &["curated-1"], "my-model");
        assert!(fallback);
        assert_eq!(options, vec!["my-model", "curated-1", CUSTOM_MODEL_OPTION]);
    }

    #[test]
    fn test_build_model_options_empty_current() {
        let live = Err(AiError::RequestFailed("nope".to_string()));
        let (options, _) = build_model_options(live, &["curated-1"], "");
        assert_eq!(options, vec!["curated-1", CUSTOM_MODEL_OPTION]);
    }

    #[test]
    fn test_curated_models_cover_suggested_providers() {
        for adapter in crate::ai::suggested_providers() {
            assert!(
                !curated_models(adapter).is_empty() || !key_storage::requires_api_key(adapter),
                "no curated fallback for {adapter}"
            );
        }
    }

    #[test]
    fn test_non_chat_filter() {
        assert!(is_obviously_non_chat("text-embedding-3-small"));
        assert!(is_obviously_non_chat("whisper-1"));
        assert!(is_obviously_non_chat("gpt-image-1"));
        assert!(!is_obviously_non_chat("gpt-5.1"));
        assert!(!is_obviously_non_chat("claude-sonnet-4-6"));
    }
}
