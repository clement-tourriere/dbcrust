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

pub mod chatgpt_auth;
pub mod config;
pub mod conversation;
pub mod key_storage;
pub mod model_listing;
pub mod prompt_templates;
pub mod schema_context;
pub mod streaming;

use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent};
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};
use thiserror::Error;
use tokio::sync::mpsc;

use self::config::{AiAuthMethod, AiConfig};

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
    #[error("generation cancelled")]
    Cancelled,
    #[error("OAuth error: {0}")]
    OAuth(String),
    #[error("Not signed in to ChatGPT. Run \\ai login first.")]
    NotLoggedIn,
    #[error("ChatGPT token refresh failed: {0} Run \\ai login again.")]
    TokenRefreshFailed(String),
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

/// Provider forced via `ai.provider`, unless it is `"auto"` (or invalid).
pub fn explicit_provider(ai_config: &AiConfig) -> Option<AdapterKind> {
    let provider = ai_config.provider.trim().to_lowercase();
    if provider.is_empty() || provider == "auto" {
        return None;
    }
    AdapterKind::from_lower_str(&provider)
}

/// The provider requests actually use: explicit `ai.provider`, else inferred
/// from the model name (the behavior of configs predating the field).
pub fn effective_provider(ai_config: &AiConfig) -> AdapterKind {
    explicit_provider(ai_config).unwrap_or_else(|| provider_for_model(&ai_config.model))
}

/// Whether two adapters are the same provider for routing purposes. OpenAI and
/// OpenAIResp are one provider: genai routes `gpt-5*`/`*codex*` models through
/// the Responses API (OpenAIResp), and forcing them back under an `openai::`
/// namespace would demote them to the legacy chat endpoint.
pub fn same_provider(a: AdapterKind, b: AdapterKind) -> bool {
    let openai_family = |k: AdapterKind| matches!(k, AdapterKind::OpenAI | AdapterKind::OpenAIResp);
    a == b || (openai_family(a) && openai_family(b))
}

/// Model string handed to genai. When the explicit provider disagrees with what
/// genai would infer from the bare name, force it with the `provider::model`
/// namespace genai understands. User-supplied namespaces always win.
pub fn qualified_model(ai_config: &AiConfig) -> String {
    let model = ai_config.model.clone();
    if model.contains("::") {
        return model;
    }
    let Some(chosen) = explicit_provider(ai_config) else {
        return model;
    };
    if same_provider(chosen, provider_for_model(&model)) {
        model
    } else {
        format!("{}::{}", chosen.as_lower_str(), model)
    }
}

/// Curated default model per provider — a wizard suggestion only, never a
/// restriction (free text always wins, and `\ai model` lists live models).
/// `None` for providers we don't curate a default for.
pub fn default_model_for(adapter: AdapterKind) -> Option<&'static str> {
    Some(match adapter {
        AdapterKind::Anthropic => "claude-sonnet-4-6",
        AdapterKind::OpenAI | AdapterKind::OpenAIResp => "gpt-5.1",
        AdapterKind::Gemini => "gemini-2.5-pro",
        AdapterKind::Ollama => "llama3.1",
        AdapterKind::Groq => "llama-3.3-70b-versatile",
        AdapterKind::DeepSeek => "deepseek-chat",
        AdapterKind::Xai => "grok-4",
        AdapterKind::Cohere => "command-a-03-2025",
        _ => return None,
    })
}

/// Auth resolved for one request. In subscription mode the token refresh has
/// already happened here, BEFORE the genai client is built, so the service
/// target resolver can stay a plain sync closure.
pub enum ResolvedAuthMode {
    ApiKey,
    ChatGptSubscription {
        access_token: String,
        account_id: String,
        session_id: String,
    },
}

/// Stable per-process session id sent to the Codex backend.
fn session_id() -> &'static str {
    static SESSION_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION_ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

pub async fn resolve_auth_mode(ai_config: &AiConfig) -> Result<ResolvedAuthMode, AiError> {
    match ai_config.auth_method {
        AiAuthMethod::ApiKey => Ok(ResolvedAuthMode::ApiKey),
        AiAuthMethod::ChatgptSubscription => {
            let (access_token, account_id) = chatgpt_auth::current_access().await?;
            Ok(ResolvedAuthMode::ChatGptSubscription {
                access_token,
                account_id,
                session_id: session_id().to_string(),
            })
        }
    }
}

/// Model string handed to genai for this request.
fn request_model(ai_config: &AiConfig, mode: &ResolvedAuthMode) -> String {
    match mode {
        ResolvedAuthMode::ApiKey => qualified_model(ai_config),
        // The target resolver pins adapter + endpoint; bare names route fine.
        ResolvedAuthMode::ChatGptSubscription { .. } => ai_config.model.clone(),
    }
}

/// Build a `genai` client wired to dbcrust's credential chain (env → OS keyring
/// → encrypted file) and an optional custom endpoint. In ChatGPT-subscription
/// mode the client instead targets the Codex backend with the OAuth bearer.
pub fn build_client(ai_config: &AiConfig, mode: &ResolvedAuthMode) -> Result<Client, AiError> {
    if !ai_config.enabled {
        return Err(AiError::NotConfigured);
    }

    if let ResolvedAuthMode::ChatGptSubscription { access_token, .. } = mode {
        // No API-key preflight: auth is the OAuth bearer, and endpoint + adapter
        // are forced — the Codex backend only speaks the Responses API.
        let access = access_token.clone();
        let target_resolver = ServiceTargetResolver::from_resolver_fn(
            move |target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
                Ok(ServiceTarget {
                    endpoint: Endpoint::from_static(chatgpt_auth::CHATGPT_CODEX_BASE),
                    auth: AuthData::from_single(access.clone()),
                    model: ModelIden::new(AdapterKind::OpenAIResp, target.model.model_name.clone()),
                })
            },
        );
        return Ok(Client::builder()
            .with_service_target_resolver(target_resolver)
            .build());
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
    let adapter = effective_provider(ai_config);
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

fn chat_options(ai_config: &AiConfig, mode: &ResolvedAuthMode) -> ChatOptions {
    match mode {
        ResolvedAuthMode::ApiKey => ChatOptions::default()
            .with_max_tokens(ai_config.max_tokens)
            .with_temperature(ai_config.temperature as f64),
        ResolvedAuthMode::ChatGptSubscription {
            account_id,
            session_id,
            ..
        } => {
            // The Codex backend rejects sampling parameters with
            // 400 "Unsupported parameter" (verified live for both
            // temperature and max_output_tokens) — send neither.
            ChatOptions::default().with_extra_headers(vec![
                ("chatgpt-account-id".to_string(), account_id.clone()),
                ("OpenAI-Beta".to_string(), "responses=experimental".to_string()),
                ("originator".to_string(), "dbcrust".to_string()),
                ("session_id".to_string(), session_id.clone()),
            ])
        }
    }
}

/// Provider silence budget: time to the first stream event (covers connect,
/// auth, and model spin-up), then a longer idle budget between events. A
/// stalled or silently-retrying transport must become a visible error, never
/// an indefinite hang.
const FIRST_EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
const IDLE_EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

async fn next_stream_event<S, T>(stream: &mut S, received_any: bool) -> Result<Option<T>, AiError>
where
    S: futures_util::Stream<Item = T> + Unpin,
{
    use futures_util::StreamExt;
    let budget = if received_any {
        IDLE_EVENT_TIMEOUT
    } else {
        FIRST_EVENT_TIMEOUT
    };
    tokio::time::timeout(budget, stream.next()).await.map_err(|_| {
        AiError::RequestFailed(format!(
            "no response from the provider after {}s",
            budget.as_secs()
        ))
    })
}

/// Non-streaming completion. Reusable for any AI task (not just SQL).
pub async fn generate(
    ai_config: &AiConfig,
    system_prompt: &str,
    messages: &[(MessageRole, String)],
) -> Result<AiResponse, AiError> {
    let mode = resolve_auth_mode(ai_config).await?;

    // The Codex backend is SSE-only — aggregate the stream for callers that
    // asked for a non-streaming completion.
    if matches!(mode, ResolvedAuthMode::ChatGptSubscription { .. }) {
        return generate_via_stream(ai_config, &mode, system_prompt, messages).await;
    }

    let client = build_client(ai_config, &mode)?;
    let req = build_request(system_prompt, messages);
    let opts = chat_options(ai_config, &mode);
    let model = request_model(ai_config, &mode);

    let res = client
        .exec_chat(model.as_str(), req, Some(&opts))
        .await
        .map_err(|e| AiError::RequestFailed(e.to_string()))?;

    Ok(AiResponse {
        content: res.first_text().unwrap_or_default().to_string(),
        model: ai_config.model.clone(),
    })
}

/// Run the streaming path and fold the deltas into one response (no terminal
/// output) — used where the backend does not accept non-streaming requests.
async fn generate_via_stream(
    ai_config: &AiConfig,
    mode: &ResolvedAuthMode,
    system_prompt: &str,
    messages: &[(MessageRole, String)],
) -> Result<AiResponse, AiError> {
    let client = build_client(ai_config, mode)?;
    let req = build_request(system_prompt, messages);
    let opts = chat_options(ai_config, mode);
    let model = request_model(ai_config, mode);

    let chat_res = client
        .exec_chat_stream(model.as_str(), req, Some(&opts))
        .await
        .map_err(|e| AiError::RequestFailed(e.to_string()))?;

    let mut stream = chat_res.stream;
    let mut content = String::new();
    let mut received_any = false;
    while let Some(event) = next_stream_event(&mut stream, received_any).await? {
        received_any = true;
        match event {
            Ok(ChatStreamEvent::Chunk(chunk)) => content.push_str(&chunk.content),
            Ok(ChatStreamEvent::End(_)) => break,
            Ok(_) => {}
            Err(e) => return Err(AiError::RequestFailed(e.to_string())),
        }
    }

    Ok(AiResponse {
        content,
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
    let mode = resolve_auth_mode(ai_config).await?;
    let client = build_client(ai_config, &mode)?;
    let req = build_request(system_prompt, messages);
    let opts = chat_options(ai_config, &mode);
    let model = request_model(ai_config, &mode);

    let chat_res = client
        .exec_chat_stream(model.as_str(), req, Some(&opts))
        .await
        .map_err(|e| AiError::RequestFailed(e.to_string()))?;

    let mut stream = chat_res.stream;
    let mut received_any = false;
    loop {
        let event = match next_stream_event(&mut stream, received_any).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(e) => {
                // The consumer learns via the channel; the caller via Err.
                let _ = tx.send(AiStreamEvent::Error(e.to_string())).await;
                return Err(e);
            }
        };
        received_any = true;
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
        assert!(matches!(
            build_client(&config, &ResolvedAuthMode::ApiKey),
            Err(AiError::NotConfigured)
        ));
    }

    fn config_with(provider: &str, model: &str) -> AiConfig {
        AiConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_effective_provider() {
        // auto → inferred from model (legacy behavior)
        let config = config_with("auto", "claude-sonnet-4-6");
        assert!(explicit_provider(&config).is_none());
        assert_eq!(effective_provider(&config), AdapterKind::Anthropic);

        // explicit provider wins over the model name
        let config = config_with("groq", "llama-3.3-70b-versatile");
        assert_eq!(effective_provider(&config), AdapterKind::Groq);

        // invalid provider value falls back to inference, not a panic
        let config = config_with("not-a-provider", "gpt-4o");
        assert!(explicit_provider(&config).is_none());
        assert_eq!(effective_provider(&config), AdapterKind::OpenAI);
    }

    #[rstest::rstest]
    // auto: bare model passes through untouched
    #[case("auto", "gpt-4o", "gpt-4o")]
    // provider agrees with inference: no namespace needed
    #[case("anthropic", "claude-sonnet-4-6", "claude-sonnet-4-6")]
    // OpenAI family: gpt-5* infers OpenAIResp — must NOT be namespaced back
    // under openai:: or it loses the Responses API routing
    #[case("openai", "gpt-5.1", "gpt-5.1")]
    // provider disagrees with inference: force the namespace
    #[case("groq", "llama-3.3-70b-versatile", "groq::llama-3.3-70b-versatile")]
    // a user-supplied namespace always wins, even a conflicting one
    #[case("openai", "anthropic::claude-sonnet-4-6", "anthropic::claude-sonnet-4-6")]
    fn test_qualified_model(#[case] provider: &str, #[case] model: &str, #[case] expected: &str) {
        assert_eq!(qualified_model(&config_with(provider, model)), expected);
    }

    #[test]
    fn test_default_model_for_prefix_inference() {
        // Where the curated default has a recognizable prefix, genai must infer
        // the same provider back — guards against typos in the curated ids.
        for adapter in [
            AdapterKind::Anthropic,
            AdapterKind::Gemini,
            AdapterKind::DeepSeek,
        ] {
            let model = default_model_for(adapter).unwrap();
            assert_eq!(provider_for_model(model), adapter, "default for {adapter}");
        }
        // OpenAI's default routes through the Responses API — same family.
        let model = default_model_for(AdapterKind::OpenAI).unwrap();
        assert!(same_provider(provider_for_model(model), AdapterKind::OpenAI));
    }
}
