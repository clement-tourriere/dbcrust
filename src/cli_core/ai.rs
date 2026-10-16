//! The AI assistant half of the interactive CLI: `??` text-to-SQL, `???`
//! agentic investigations and the `\ai` setup/login/provider/model wizards.
//!
//! Compiled only with the `ai` feature (the parent declares the module under
//! `#[cfg(feature = "ai")]`); without it the REPL answers `??`/`\ai` with a
//! "not compiled in" message instead.

use super::{CliCore, CliError, DbCrustConfig};
use crate::db::Database;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Marker: the user pressed Ctrl-C inside an interactive AI wizard. Callers
/// must abort the whole flow without saving anything.
struct WizardCancelled;

/// Map one inquire prompt result to wizard semantics: Esc skips the step
/// (`Ok(None)`), Ctrl-C aborts the whole wizard (`Err(WizardCancelled)`).
fn wizard_step<T>(res: Result<T, inquire::InquireError>) -> Result<Option<T>, WizardCancelled> {
    match res {
        Ok(v) => Ok(Some(v)),
        Err(inquire::InquireError::OperationCanceled) => Ok(None),
        Err(inquire::InquireError::OperationInterrupted) => Err(WizardCancelled),
        Err(e) => {
            eprintln!("Prompt error: {e}");
            Err(WizardCancelled)
        }
    }
}

/// Drop key events stranded in crossterm's process-global event queue by an
/// interrupted inquire prompt. inquire and reedline share that queue (single
/// crossterm copy), so without this a rapid Ctrl-C inside a wizard instantly
/// fails the next inquire prompt and leaks into reedline's next read_line,
/// re-rendering the REPL prompt twice ("db=> =>").
fn drain_stale_terminal_events() {
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = crossterm::event::read();
    }
}

/// Single exit path for a cancelled AI wizard: clean the terminal state so
/// reedline repaints on a fresh line with no stale key events.
fn cancel_ai_wizard() {
    drain_stale_terminal_events();
    println!("\nCancelled — nothing was saved.");
}

impl CliCore {
    // ==================== AI Assistant Methods ====================

    /// Handle ?? text-to-SQL generation
    #[allow(clippy::await_holding_lock)]
    pub(super) async fn handle_ai_text_to_sql(
        &mut self,
        natural_language: &str,
        db_arc: &Arc<Mutex<Database>>,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<(), CliError> {
        let config = config_arc.lock().unwrap().clone();

        if !config.ai.enabled {
            return Err(CliError::CommandError(
                "AI assistant is disabled. Run \\ai on or \\ai setup to configure.".to_string(),
            ));
        }

        // Fresh cancellation state (a previous Ctrl-C must not abort us)
        interrupt_flag.store(false, std::sync::atomic::Ordering::SeqCst);

        // Immediate feedback: schema introspection + generation can take a
        // few seconds, and the user must know Ctrl-C works the whole time.
        let auth_label = match config.ai.auth_method {
            crate::ai::config::AiAuthMethod::ChatgptSubscription => " via ChatGPT subscription",
            crate::ai::config::AiAuthMethod::ApiKey => "",
        };
        println!(
            "\x1b[2mGenerating with {}{auth_label}… (Ctrl-C cancels)\x1b[0m",
            config.ai.model
        );

        // Build schema context, reusing the session cache when it's valid for the
        // current database. The build is many catalog round-trips — slow over a
        // tunnel — so we avoid repeating it. Only the query-independent build is
        // cacheable (see `build_schema_context`); a query-specific selection on a
        // large database is rebuilt every time.
        let schema_context = {
            let mut db_guard = db_arc.lock().unwrap();
            let db_name = db_guard.get_current_db();
            match &self.ai_schema_cache {
                Some((cached_db, ctx)) if *cached_db == db_name => ctx.clone(),
                _ => {
                    let (ctx, cacheable) = crate::ai::schema_context::build_schema_context(
                        &mut *db_guard,
                        natural_language,
                        config.ai.max_schema_tables,
                    )
                    .await;
                    if cacheable {
                        self.ai_schema_cache = Some((db_name, ctx.clone()));
                    } else {
                        // Stale entry from a previous database must not linger.
                        self.ai_schema_cache = None;
                    }
                    ctx
                }
            }
        };

        // Build system prompt
        let db_type = {
            let db_guard = db_arc.lock().unwrap();
            db_guard.get_database_type()
        };
        let system_prompt =
            crate::ai::prompt_templates::build_system_prompt(&db_type, &schema_context);

        // Build messages from conversation history
        let messages = self.ai_conversation.to_messages(natural_language);

        // Generate SQL. Provider/model handling is delegated to genai.
        let sql = if config.ai.streaming {
            // Streaming mode
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            let ai_config = config.ai.clone();
            let system_prompt_clone = system_prompt.clone();
            let messages_clone = messages.clone();
            let interrupt_clone = interrupt_flag.clone();

            let generate_handle = tokio::spawn(async move {
                crate::ai::generate_stream(&ai_config, &system_prompt_clone, &messages_clone, tx)
                    .await
            });

            let response =
                match crate::ai::streaming::stream_to_terminal(rx, &interrupt_clone).await {
                    Ok(response) => response,
                    Err(crate::ai::AiError::Cancelled) => {
                        // Stop the provider request too — without the abort,
                        // "cancel" would still wait out the full generation
                        generate_handle.abort();
                        eprintln!("AI generation cancelled.");
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(CliError::CommandError(format!("Streaming error: {e}")));
                    }
                };

            // Wait for generation to complete
            if let Err(e) = generate_handle
                .await
                .map_err(|e| CliError::CommandError(format!("Generation task error: {e}")))?
            {
                return Err(CliError::CommandError(format!("AI generation error: {e}")));
            }

            crate::ai::streaming::extract_sql(&response)
        } else {
            // Non-streaming mode, raced against Ctrl-C (dropping the future
            // aborts the underlying request)
            let gen_fut = crate::ai::generate(&config.ai, &system_prompt, &messages);
            tokio::pin!(gen_fut);
            let response = loop {
                tokio::select! {
                    res = &mut gen_fut => {
                        break res.map_err(|e| {
                            CliError::CommandError(format!("AI generation error: {e}"))
                        })?;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        if interrupt_flag.load(std::sync::atomic::Ordering::Relaxed) {
                            eprintln!("AI generation cancelled.");
                            return Ok(());
                        }
                    }
                }
            };

            let sql = crate::ai::streaming::extract_sql(&response.content);

            if config.ai.show_generated_sql {
                println!("\x1b[2m{sql}\x1b[0m");
            }

            sql
        };

        if sql.is_empty() {
            return Err(CliError::CommandError(
                "AI returned empty response".to_string(),
            ));
        }

        // Add to conversation history
        self.ai_conversation.add_exchange(natural_language, &sql);

        // The user must SEE the SQL before being asked to confirm it —
        // show_generated_sql only governs display for auto-execution.
        // (Streaming mode already printed the response while it arrived.)
        let sql_already_shown = config.ai.streaming || config.ai.show_generated_sql;
        let needs_confirmation = !matches!(
            config.ai.execution_mode,
            crate::ai::config::AiExecutionMode::AutoExecute
        );
        if !sql_already_shown && needs_confirmation {
            println!("\x1b[2m{sql}\x1b[0m");
        }

        // Determine whether to execute; writes never default to Yes
        let is_read_only = crate::ai::streaming::is_select_query(&sql);
        // Esc/Ctrl-C collapse to "do not execute" (the safe default), but stale
        // terminal events must still be drained or they leak into the REPL.
        let confirm_or_no = |prompt: inquire::Confirm| match prompt.prompt() {
            Ok(v) => v,
            Err(_) => {
                drain_stale_terminal_events();
                false
            }
        };
        let should_execute = match config.ai.execution_mode {
            crate::ai::config::AiExecutionMode::Confirm => {
                confirm_or_no(inquire::Confirm::new("Execute this SQL?").with_default(is_read_only))
            }
            crate::ai::config::AiExecutionMode::AutoSelect => {
                is_read_only
                    || confirm_or_no(
                        inquire::Confirm::new("Execute this write query?").with_default(false),
                    )
            }
            crate::ai::config::AiExecutionMode::AutoExecute => true,
        };

        if should_execute {
            self.execute_sql_interactive(&sql, db_arc, interrupt_flag)
                .await?;
        }

        Ok(())
    }

    /// Handle `??? <question>` — the agentic investigation loop. The model calls
    /// read-only tools (list/describe/run_sql/explain), observes results, and
    /// iterates until it produces a structured analysis. It can never mutate data.
    #[allow(clippy::await_holding_lock)]
    pub(super) async fn handle_ai_agentic(
        &mut self,
        question: &str,
        db_arc: &Arc<Mutex<Database>>,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
        interrupt_flag: &Arc<AtomicBool>,
    ) -> Result<(), CliError> {
        let config = config_arc.lock().unwrap().clone();

        if !config.ai.enabled {
            return Err(CliError::CommandError(
                "AI assistant is disabled. Run \\ai on or \\ai setup to configure.".to_string(),
            ));
        }

        // Fresh cancellation state (a previous Ctrl-C must not abort us).
        interrupt_flag.store(false, std::sync::atomic::Ordering::SeqCst);

        println!(
            "\x1b[2m🔍 Investigating with {}… (Ctrl-C cancels)\x1b[0m",
            config.ai.model
        );

        // Build a lightweight seed (table-name list only) — lock+drop the guard
        // so no DB mutex is held across the agent's genai round-trips.
        let (db_type, seed) = {
            let mut db_guard = db_arc.lock().unwrap();
            let db_type = db_guard.get_database_type();
            let seed = crate::ai::schema_context::build_agent_seed_context(&mut *db_guard).await;
            (db_type, seed)
        };

        let system_prompt =
            crate::ai::prompt_templates::build_agentic_system_prompt(&db_type, &seed, None);

        let executor = crate::ai::agent::DbToolExecutor::new(
            db_arc.clone(),
            interrupt_flag.clone(),
            config.ai.agentic_max_rows_per_tool,
        );
        let progress = crate::ai::agent::StdoutProgress;

        // Thread prior-investigation history (separate from the `??` history) so
        // `???` follow-ups recall earlier findings. The current question is the
        // trailing user message.
        let messages = self.agentic_conversation.to_messages(question);

        match crate::ai::agent::run_agent(
            &config.ai,
            &system_prompt,
            &messages,
            config.ai.agentic_max_iterations,
            &executor,
            &progress,
            interrupt_flag,
        )
        .await
        {
            Ok(answer) => {
                // Final analysis in normal weight so it stands out as the answer.
                // Recorded in the dedicated agentic history (NOT `ai_conversation`,
                // which feeds `??` text-to-SQL — prose would pollute those prompts).
                println!("\n{answer}");
                self.agentic_conversation.add_exchange(question, &answer);
                Ok(())
            }
            Err(crate::ai::AiError::Cancelled) => {
                eprintln!("AI investigation cancelled.");
                Ok(())
            }
            Err(e) => Err(CliError::CommandError(format!(
                "AI investigation error: {e}"
            ))),
        }
    }

    /// Prompt for an API key and persist it for `adapter`. `Ok(false)` means the
    /// key step was skipped (Esc / empty input) — the wizard may continue.
    fn configure_provider_key(adapter: crate::ai::AdapterKind) -> Result<bool, WizardCancelled> {
        use crate::ai::key_storage::{self, KeyStorageMethod, store_api_key};

        if !key_storage::requires_api_key(adapter) {
            println!(
                "{} runs locally and does not require an API key.",
                adapter.as_str()
            );
            return Ok(true);
        }

        if let Some(env_name) = key_storage::env_var_name(adapter) {
            println!(
                "\nAPI key needed for {}. You can also set the {} environment variable.",
                adapter.as_str(),
                env_name
            );
        }

        let api_key = match wizard_step(
            inquire::Password::new("Enter API key:")
                .without_confirmation()
                .prompt(),
        )? {
            Some(key) if !key.is_empty() => key,
            _ => {
                println!("Skipped API key configuration.");
                return Ok(false);
            }
        };

        let storage_options = vec![
            "Secure store (OS keychain, or passphrase-encrypted file on Linux)",
            "Environment variable (show command)",
        ];
        let method = match wizard_step(
            inquire::Select::new("How to store the API key?", storage_options).prompt(),
        )? {
            Some(choice) if choice.starts_with("Secure store") => KeyStorageMethod::SecretStore,
            Some(_) => KeyStorageMethod::EnvVarHint,
            None => {
                println!("API key not stored.");
                return Ok(false);
            }
        };

        match store_api_key(adapter, &api_key, &method) {
            Ok(()) => {
                if method != KeyStorageMethod::EnvVarHint {
                    println!("API key stored successfully via {method}.");
                }
                Ok(true)
            }
            Err(e) => {
                eprintln!("Failed to store API key: {e}");
                Ok(false)
            }
        }
    }

    /// Fetch live models for `adapter` (curated fallback on any failure) and
    /// let the user pick one. `Ok(None)` means keep the current model (Esc).
    async fn pick_model(
        adapter: crate::ai::AdapterKind,
        endpoint: Option<&str>,
        current: &str,
    ) -> Result<Option<String>, WizardCancelled> {
        use crate::ai::model_listing;

        println!("Fetching available models from {}…", adapter.as_str());
        let live = model_listing::list_models(adapter, endpoint).await;
        if let Err(e) = &live {
            println!(
                "\x1b[2mCould not fetch the live model list ({e}); showing suggestions.\x1b[0m"
            );
        }
        let curated = model_listing::curated_models(adapter);
        let (options, _) = model_listing::build_model_options(live, &curated, current);
        Self::pick_model_from(options, current)
    }

    /// Model Select (type to filter) with a free-text escape hatch.
    fn pick_model_from(
        options: Vec<String>,
        current: &str,
    ) -> Result<Option<String>, WizardCancelled> {
        let starting = options.iter().position(|m| m == current).unwrap_or(0);
        let choice = match wizard_step(
            inquire::Select::new("Model:", options)
                .with_starting_cursor(starting)
                .with_page_size(12)
                .with_help_message("type to filter; Esc keeps the current model")
                .prompt(),
        )? {
            Some(c) => c,
            None => return Ok(None),
        };
        if choice != crate::ai::model_listing::CUSTOM_MODEL_OPTION {
            return Ok(Some(choice));
        }
        match wizard_step(
            inquire::Text::new("Model:")
                .with_default(current)
                .with_help_message("any genai-supported model, e.g. provider::model")
                .prompt(),
        )? {
            Some(m) if !m.trim().is_empty() => Ok(Some(m.trim().to_string())),
            _ => Ok(None),
        }
    }

    /// Interactively pick a provider from the curated wizard list. This is the
    /// first structural step of a wizard, so Esc aborts just like Ctrl-C.
    fn select_provider(prompt: &str) -> Result<crate::ai::AdapterKind, WizardCancelled> {
        let providers = crate::ai::suggested_providers();
        let names: Vec<String> = providers.iter().map(|p| p.as_str().to_string()).collect();
        let names_ref: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        match wizard_step(inquire::Select::new(prompt, names_ref.clone()).prompt())? {
            Some(choice) => Ok(providers[names_ref.iter().position(|&n| n == choice).unwrap_or(0)]),
            None => Err(WizardCancelled),
        }
    }

    /// Handle \ai setup - interactive setup wizard
    pub(super) async fn handle_ai_setup(&mut self, config_arc: &Arc<Mutex<DbCrustConfig>>) {
        if let Err(WizardCancelled) = self.run_ai_setup_wizard(config_arc).await {
            cancel_ai_wizard();
        }
    }

    /// The actual `\ai setup` flow. Every prompt propagates Ctrl-C via `?`, so
    /// config is only touched when the user completes the whole wizard.
    async fn run_ai_setup_wizard(
        &mut self,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
    ) -> Result<(), WizardCancelled> {
        use crate::ai::config::AiAuthMethod;

        // The chosen provider is stored in config and drives auth + routing;
        // the curated list is a UX convenience — any genai provider works.
        let adapter = Self::select_provider("Select AI provider:")?;

        // Auth step. OpenAI additionally offers "Sign in with ChatGPT" so the
        // user's subscription pays for requests instead of an API key.
        let auth_method = Self::configure_auth(adapter).await?;

        let (endpoint, model) = if auth_method == AiAuthMethod::ChatgptSubscription {
            // Subscription mode: the endpoint is forced to the Codex backend
            // and only its model set applies — no endpoint/listing questions.
            let current_model = config_arc.lock().unwrap().ai.model.clone();
            let subscription = crate::ai::model_listing::chatgpt_subscription_models();
            let suggested = if subscription.contains(&current_model.as_str()) {
                current_model
            } else {
                subscription[0].to_string()
            };
            let (options, _) = crate::ai::model_listing::build_model_options(
                Ok(subscription.iter().map(|s| s.to_string()).collect()),
                &[],
                &suggested,
            );
            let model = Self::pick_model_from(options, &suggested)?.unwrap_or(suggested);
            (None, model)
        } else {
            // Optional custom endpoint, asked before the model step so the
            // live model listing below can query it (self-hosted gateways,
            // remote Ollama). Esc keeps the previously configured value.
            let current_endpoint = config_arc
                .lock()
                .unwrap()
                .ai
                .endpoint
                .clone()
                .unwrap_or_default();
            let endpoint = match wizard_step(
                inquire::Text::new("Custom endpoint URL (optional):")
                    .with_default(&current_endpoint)
                    .with_help_message("Leave empty to use the provider's default endpoint")
                    .prompt(),
            )? {
                Some(u) => {
                    let u = u.trim().to_string();
                    if u.is_empty() { None } else { Some(u) }
                }
                None => config_arc.lock().unwrap().ai.endpoint.clone(),
            };

            // Model: live list from the chosen provider (curated fallback),
            // with a provider-appropriate default — keeping the previous model
            // only makes sense if it already belongs to this provider.
            let current_model = config_arc.lock().unwrap().ai.model.clone();
            let suggested_model =
                if crate::ai::same_provider(crate::ai::provider_for_model(&current_model), adapter)
                {
                    current_model.clone()
                } else {
                    crate::ai::default_model_for(adapter)
                        .map(str::to_string)
                        .unwrap_or_else(|| current_model.clone())
                };
            let model = Self::pick_model(adapter, endpoint.as_deref(), &suggested_model)
                .await?
                .unwrap_or(suggested_model);
            (endpoint, model)
        };

        {
            let mut config = config_arc.lock().unwrap();
            config.ai.enabled = true;
            config.ai.provider = adapter.as_lower_str().to_string();
            config.ai.model = model.clone();
            config.ai.endpoint = endpoint;
            config.ai.auth_method = auth_method;
            config.save_with_documentation().ok();
        }

        println!(
            "\nAI assistant configured (provider: {}, model: {model}). Use ?? to generate SQL from natural language.",
            adapter.as_str()
        );
        Ok(())
    }

    /// Auth step of the setup wizard. Returns the auth method to persist.
    /// For OpenAI this offers API key / browser sign-in / Codex CLI import;
    /// every other provider goes straight to the API-key flow.
    async fn configure_auth(
        adapter: crate::ai::AdapterKind,
    ) -> Result<crate::ai::config::AiAuthMethod, WizardCancelled> {
        use crate::ai::config::AiAuthMethod;

        if !crate::ai::same_provider(adapter, crate::ai::AdapterKind::OpenAI) {
            Self::configure_provider_key(adapter)?;
            return Ok(AiAuthMethod::ApiKey);
        }

        const OPT_API_KEY: &str = "API key";
        const OPT_CHATGPT: &str = "ChatGPT subscription (sign in with your browser)";
        const OPT_CODEX: &str = "Reuse Codex CLI login (~/.codex/auth.json)";

        let mut options = vec![OPT_API_KEY, OPT_CHATGPT];
        if crate::ai::chatgpt_auth::codex_auth_path().is_some() {
            options.push(OPT_CODEX);
        }

        // Structural step: Esc aborts like Ctrl-C — there is no sane default.
        let choice = match wizard_step(
            inquire::Select::new("How do you want to authenticate?", options).prompt(),
        )? {
            Some(c) => c,
            None => return Err(WizardCancelled),
        };

        match choice {
            OPT_CHATGPT => match crate::ai::chatgpt_auth::login().await {
                Ok(tokens) => {
                    println!("Signed in to ChatGPT (account {}).", tokens.account_id);
                    Ok(AiAuthMethod::ChatgptSubscription)
                }
                Err(e) => {
                    eprintln!("ChatGPT sign-in failed: {e}");
                    println!("Falling back to API-key setup.");
                    Self::configure_provider_key(adapter)?;
                    Ok(AiAuthMethod::ApiKey)
                }
            },
            OPT_CODEX => match crate::ai::chatgpt_auth::import_from_codex() {
                Ok(tokens) => {
                    println!("Imported Codex CLI login (account {}).", tokens.account_id);
                    Ok(AiAuthMethod::ChatgptSubscription)
                }
                Err(e) => {
                    eprintln!("Codex import failed: {e}");
                    println!("Falling back to API-key setup.");
                    Self::configure_provider_key(adapter)?;
                    Ok(AiAuthMethod::ApiKey)
                }
            },
            _ => {
                Self::configure_provider_key(adapter)?;
                Ok(AiAuthMethod::ApiKey)
            }
        }
    }

    /// Handle \ai login — standalone "Sign in with ChatGPT".
    pub(super) async fn handle_ai_login(&mut self, config_arc: &Arc<Mutex<DbCrustConfig>>) {
        use crate::ai::config::AiAuthMethod;

        println!(
            "Signing in with ChatGPT (requests will use your ChatGPT plan via the Codex backend)."
        );
        match crate::ai::chatgpt_auth::login().await {
            Ok(tokens) => {
                let needs_model_pick = {
                    let mut config = config_arc.lock().unwrap();
                    config.ai.enabled = true;
                    config.ai.provider = "openai".to_string();
                    config.ai.auth_method = AiAuthMethod::ChatgptSubscription;
                    let subscription = crate::ai::model_listing::chatgpt_subscription_models();
                    let needs = !subscription.contains(&config.ai.model.as_str());
                    config.save_with_documentation().ok();
                    needs
                };
                println!("Signed in to ChatGPT (account {}).", tokens.account_id);

                if needs_model_pick {
                    let subscription = crate::ai::model_listing::chatgpt_subscription_models();
                    let suggested = subscription[0].to_string();
                    let (options, _) = crate::ai::model_listing::build_model_options(
                        Ok(subscription.iter().map(|s| s.to_string()).collect()),
                        &[],
                        &suggested,
                    );
                    match Self::pick_model_from(options, &suggested) {
                        Ok(Some(model)) => {
                            let mut config = config_arc.lock().unwrap();
                            config.ai.model = model.clone();
                            config.save_with_documentation().ok();
                            println!("Model set to {model}.");
                        }
                        Ok(None) => {}
                        // Sign-in already succeeded and was saved; only the
                        // model pick was abandoned.
                        Err(WizardCancelled) => cancel_ai_wizard(),
                    }
                }
            }
            Err(e) => eprintln!("ChatGPT sign-in failed: {e}"),
        }
    }

    /// Handle \ai provider [name] — set the active provider in config (and
    /// offer to store its API key). `auto` returns to inferring the provider
    /// from the model name.
    pub(super) async fn handle_ai_select_provider(
        &mut self,
        arg: &str,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
    ) {
        use crate::ai::AdapterKind;

        let arg = arg.trim();
        let adapter: Option<AdapterKind> = if arg.is_empty() {
            match Self::select_provider("Select AI provider:") {
                Ok(a) => Some(a),
                Err(WizardCancelled) => {
                    cancel_ai_wizard();
                    return;
                }
            }
        } else if arg.eq_ignore_ascii_case("auto") {
            None
        } else {
            match AdapterKind::from_lower_str(&arg.to_lowercase()) {
                Some(a) => Some(a),
                None => {
                    eprintln!(
                        "Unknown provider: {arg}. Use \"auto\" or any genai provider key (anthropic, openai, gemini, ollama, groq, ...)."
                    );
                    return;
                }
            }
        };

        // Mutate + save without holding the lock across the prompts below.
        let model = {
            let mut config = config_arc.lock().unwrap();
            config.ai.provider = adapter
                .map(|a| a.as_lower_str().to_string())
                .unwrap_or_else(|| "auto".to_string());
            let effective = crate::ai::effective_provider(&config.ai);
            if config.ai.auth_method == crate::ai::config::AiAuthMethod::ChatgptSubscription
                && !crate::ai::same_provider(effective, AdapterKind::OpenAI)
            {
                config.ai.auth_method = crate::ai::config::AiAuthMethod::ApiKey;
                println!("Auth method reset to api_key (ChatGPT subscription is OpenAI-only).");
            }
            config.save_with_documentation().ok();
            config.ai.model.clone()
        };

        match adapter {
            Some(a) => {
                println!("Provider set to {}.", a.as_str());
                if crate::ai::key_storage::requires_api_key(a)
                    && crate::ai::key_storage::detect_key_storage(a).is_none()
                    && let Err(WizardCancelled) = Self::configure_provider_key(a)
                {
                    cancel_ai_wizard();
                    return;
                }
                let inferred = crate::ai::provider_for_model(&model);
                if !model.contains("::") && !crate::ai::same_provider(inferred, a) {
                    println!(
                        "Note: model \"{model}\" looks like a {} model — requests will be forced to {}. Run \\ai model to pick another.",
                        inferred.as_str(),
                        a.as_str()
                    );
                }
            }
            None => println!("Provider set to auto (inferred from the model name)."),
        }
    }

    /// Handle \ai model [name] — set the model (provider is inferred from it).
    pub(super) async fn handle_ai_select_model(
        &mut self,
        arg: &str,
        config_arc: &Arc<Mutex<DbCrustConfig>>,
        _db_arc: &Arc<Mutex<Database>>,
    ) {
        let model = if arg.is_empty() {
            // Clone what the picker needs and drop the guard — it does
            // network I/O and must not hold the config lock across awaits.
            let (current, adapter, endpoint) = {
                let config = config_arc.lock().unwrap();
                (
                    config.ai.model.clone(),
                    crate::ai::effective_provider(&config.ai),
                    config.ai.endpoint.clone(),
                )
            };
            match Self::pick_model(adapter, endpoint.as_deref(), &current).await {
                Ok(Some(m)) => m,
                Ok(None) => return,
                Err(WizardCancelled) => {
                    cancel_ai_wizard();
                    return;
                }
            }
        } else {
            arg.to_string()
        };

        let mut config = config_arc.lock().unwrap();
        config.ai.model = model.clone();
        config.save_with_documentation().ok();
        let adapter = crate::ai::effective_provider(&config.ai);
        let source = if crate::ai::explicit_provider(&config.ai).is_some() {
            "from config"
        } else {
            "inferred"
        };
        println!(
            "Model set to {model} (provider: {}, {source}).",
            adapter.as_str()
        );
    }

    // ==================== End AI Assistant Methods ====================
}
