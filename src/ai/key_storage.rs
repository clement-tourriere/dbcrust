//! API key and token resolution: env vars -> the secret store.
//!
//! Keyed by `genai`'s [`AdapterKind`] (the provider), so dbcrust does not keep
//! its own provider enum. Env var names come from genai's `default_key_env_name`.
//!
//! Keys live in the shared secret store (`crate::secrets`) under
//! `ai:<provider>` (API keys) and `oauth:<name>` (OAuth/token blobs). There
//! is **no** fallback to a weaker store: a locked or unavailable store is an
//! error. The pre-store locations — the old `dbcrust` keychain entries and
//! the machine-key `ai_keys.enc` file — are read only so existing users
//! keep working until `dbcrust secrets migrate` has moved them.

use crate::ai::AiError;
use crate::config::Config;
use crate::secrets::{RecordId, SecretError};
use genai::adapter::AdapterKind;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyStorageMethod {
    /// The DBCrust secret store (OS keychain or passphrase-encrypted file).
    SecretStore,
    EnvVarHint,
}

impl std::fmt::Display for KeyStorageMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyStorageMethod::SecretStore => write!(f, "the secure store"),
            KeyStorageMethod::EnvVarHint => write!(f, "Environment variable"),
        }
    }
}

/// Standard env var name for a provider's key (e.g. `ANTHROPIC_API_KEY`).
/// `None` for providers genai treats as keyless (e.g. Ollama).
pub fn env_var_name(adapter: AdapterKind) -> Option<&'static str> {
    adapter.default_key_env_name()
}

/// Whether this provider needs an API key at all (local providers don't).
pub fn requires_api_key(adapter: AdapterKind) -> bool {
    env_var_name(adapter).is_some()
}

/// Store id of a provider's API key: `ai:<provider>`.
pub fn store_id(adapter: AdapterKind) -> String {
    RecordId::ai(adapter.as_lower_str())
}

/// Name the *legacy* keychain entry and `ai_keys.enc` line used.
fn legacy_key_name(adapter: AdapterKind) -> String {
    format!("{}_api_key", adapter.as_lower_str())
}

fn storage_error(error: SecretError) -> AiError {
    AiError::KeyStorageError(error.to_string())
}

/// Resolve the API key: env var first, then the secret store, then (read
/// only, with a migration hint) the legacy locations.
pub fn resolve_api_key(adapter: AdapterKind) -> Result<String, AiError> {
    // 1. Environment variable (highest priority)
    if let Some(env_name) = env_var_name(adapter)
        && let Ok(key) = std::env::var(env_name)
    {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // 2. The secret store
    let store_problem = match crate::secrets::default_store().get_secret(&store_id(adapter)) {
        Ok(Some(key)) => return Ok(key.to_string()),
        Ok(None) => None,
        Err(e) => Some(e),
    };

    // 3. Legacy keychain entry / ai_keys.enc (read-only migration fallback)
    if let Some(key) = legacy::read_named(&legacy_key_name(adapter)) {
        crate::config::hint_legacy_secret_used();
        return Ok(key);
    }

    match store_problem {
        // A locked store is not a missing key: say what to do.
        Some(e) => Err(storage_error(e)),
        None => Err(AiError::MissingApiKey(adapter.as_str().to_string())),
    }
}

/// Store an API key with the chosen method.
pub fn store_api_key(
    adapter: AdapterKind,
    key: &str,
    method: &KeyStorageMethod,
) -> Result<(), AiError> {
    match method {
        KeyStorageMethod::SecretStore => crate::secrets::default_store()
            .put_secret(&store_id(adapter), key)
            .map_err(storage_error),
        KeyStorageMethod::EnvVarHint => {
            // Never echo the key itself — it would persist in terminal
            // scrollback, tmux logs, and session recordings
            let _ = key;
            let env_name = env_var_name(adapter).unwrap_or("DBCRUST_AI_API_KEY");
            println!(
                "\nAdd this to your shell profile (paste your key in place of the placeholder):\n  export {env_name}=<your-api-key>"
            );
            Ok(())
        }
    }
}

/// Detect which storage method currently holds the key
pub fn detect_key_storage(adapter: AdapterKind) -> Option<KeyStorageMethod> {
    if let Some(env_name) = env_var_name(adapter)
        && std::env::var(env_name)
            .ok()
            .filter(|k| !k.is_empty())
            .is_some()
    {
        return Some(KeyStorageMethod::EnvVarHint);
    }
    let in_store = matches!(
        crate::secrets::default_store().get_secret(&store_id(adapter)),
        Ok(Some(_))
    );
    if in_store || legacy::read_named(&legacy_key_name(adapter)).is_some() {
        return Some(KeyStorageMethod::SecretStore);
    }
    None
}

// --- Named secrets (generic storage shared by API keys and OAuth tokens) ---

/// Name under which the ChatGPT-subscription OAuth tokens are stored.
pub const CHATGPT_TOKENS_SECRET: &str = "chatgpt_oauth_tokens";

/// Store a named secret (`oauth:<name>`). Fails closed: a locked or
/// unavailable store is an error, never a weaker fallback.
pub fn store_named_secret(name: &str, value: &str) -> Result<(), AiError> {
    crate::secrets::default_store()
        .put_secret(&RecordId::oauth(name), value)
        .map_err(storage_error)
}

/// Load a named secret: the store, then the legacy locations (read-only).
pub fn load_named_secret(name: &str) -> Option<String> {
    match crate::secrets::default_store().get_secret(&RecordId::oauth(name)) {
        Ok(Some(value)) => return Some(value.to_string()),
        Ok(None) => {}
        Err(e) => tracing::debug!("named secret unavailable: {e}"),
    }
    let legacy = legacy::read_named(name)?;
    crate::config::hint_legacy_secret_used();
    Some(legacy)
}

/// Delete a named secret from the store and the legacy locations. Missing
/// entries are not errors.
pub fn delete_named_secret(name: &str) {
    let _ = crate::secrets::default_store().delete(&RecordId::oauth(name));
    legacy::delete_named(name);
}

/// The pre-store locations, kept for reading and migration only.
pub(crate) mod legacy {
    use super::*;

    /// Service name of the old keychain entries.
    const LEGACY_SERVICE: &str = "dbcrust";

    /// `~/.config/dbcrust/ai_keys.enc`.
    pub(crate) fn ai_keys_path() -> Option<PathBuf> {
        Config::get_config_directory()
            .ok()
            .map(|dir| dir.join("ai_keys.enc"))
    }

    /// Account names of the old keychain entries worth probing: one per
    /// known provider plus the ChatGPT token blob.
    pub(crate) fn keychain_account_names() -> Vec<String> {
        let mut names: Vec<String> = crate::ai::suggested_providers()
            .into_iter()
            .map(legacy_key_name)
            .collect();
        names.push(CHATGPT_TOKENS_SECRET.to_string());
        names.sort();
        names.dedup();
        names
    }

    /// Test builds must never reach the developer's real keychain.
    fn keychain_off() -> bool {
        crate::config::is_test_mode()
    }

    /// Whether an old keychain entry exists, without reading its secret
    /// where the platform allows (attributes only).
    pub(crate) fn keychain_entry_exists(name: &str) -> bool {
        if keychain_off() {
            return false;
        }
        keyring::Entry::new(LEGACY_SERVICE, name)
            .ok()
            .is_some_and(|entry| entry.get_attributes().is_ok())
    }

    /// The old keychain entry's value (migration, legacy fallback).
    pub(crate) fn keychain_get(name: &str) -> Option<zeroize::Zeroizing<String>> {
        if keychain_off() {
            return None;
        }
        keyring::Entry::new(LEGACY_SERVICE, name)
            .ok()?
            .get_password()
            .ok()
            .map(zeroize::Zeroizing::new)
    }

    pub(crate) fn keychain_delete(name: &str) -> bool {
        if keychain_off() {
            return false;
        }
        keyring::Entry::new(LEGACY_SERVICE, name)
            .ok()
            .is_some_and(|entry| entry.delete_credential().is_ok())
    }

    /// One `name=value` line of the old encrypted file; the value is
    /// decrypted through the legacy reader (`Err` when it cannot be).
    pub(crate) struct FileEntry {
        pub(crate) name: String,
        pub(crate) raw_line: String,
        pub(crate) value: Result<zeroize::Zeroizing<String>, String>,
    }

    pub(crate) fn read_file_entries(path: &std::path::Path) -> Vec<FileEntry> {
        let Ok(Some(bytes)) = crate::secrets::fsutil::read_bounded(path, 1024 * 1024) else {
            return Vec::new();
        };
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| {
                let (name, value) = line.split_once('=')?;
                let name = name.trim().to_string();
                if name.is_empty() {
                    return None;
                }
                let value = crate::password_encryption::legacy_decrypt(value.trim())
                    .map(zeroize::Zeroizing::new)
                    .map_err(|_| "cannot be decrypted on this machine".to_string());
                Some(FileEntry {
                    name,
                    raw_line: line.to_string(),
                    value,
                })
            })
            .collect()
    }

    /// Legacy lookup of one named secret: keychain, then `ai_keys.enc`.
    pub(crate) fn read_named(name: &str) -> Option<String> {
        if let Some(value) = keychain_get(name) {
            return Some(value.to_string());
        }
        let path = ai_keys_path()?;
        read_file_entries(&path)
            .into_iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.value.ok())
            .map(|value| value.to_string())
    }

    /// Remove one name from both legacy locations (best effort).
    pub(crate) fn delete_named(name: &str) {
        keychain_delete(name);
        if let Some(path) = ai_keys_path() {
            let drop: std::collections::HashSet<String> = read_file_entries(&path)
                .into_iter()
                .filter(|entry| entry.name == name)
                .map(|entry| entry.raw_line)
                .collect();
            let _ = remove_file_lines(&path, &drop);
        }
    }

    /// Rewrite `ai_keys.enc` without the given raw lines; the file is
    /// deleted when nothing is left. Atomic, 0600, symlinks refused.
    pub(crate) fn remove_file_lines(
        path: &std::path::Path,
        drop: &std::collections::HashSet<String>,
    ) -> Result<usize, SecretError> {
        let Some(bytes) = crate::secrets::fsutil::read_bounded(path, 1024 * 1024)? else {
            return Ok(0);
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut kept = Vec::new();
        let mut dropped = 0;
        for line in text.lines() {
            if drop.contains(line) {
                dropped += 1;
            } else {
                kept.push(line);
            }
        }
        if dropped == 0 {
            return Ok(0);
        }
        if kept.iter().all(|line| line.trim().is_empty()) {
            std::fs::remove_file(path)?;
        } else {
            let mut content = kept.join("\n");
            content.push('\n');
            crate::secrets::fsutil::write_private_atomic(path, content.as_bytes())?;
        }
        Ok(dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::testing::FaultyBackend;
    use crate::secrets::{MemoryBackend, SecretStore, with_store_override};
    use std::sync::Arc;

    // Tests never touch the developer's real keychain: the default store is
    // an injected in-memory backend, and the config dir auto-isolates to
    // /tmp/dbcrust_test_{pid} in test builds.

    #[test]
    fn named_secret_round_trip_uses_the_store_and_never_creates_ai_keys_enc() {
        let store = Arc::new(SecretStore::in_memory());
        with_store_override(store.clone(), || {
            store_named_secret("unit_test_secret", "s3cret-value").unwrap();
            assert_eq!(
                load_named_secret("unit_test_secret").as_deref(),
                Some("s3cret-value")
            );
            assert!(
                store
                    .get_secret("oauth:unit_test_secret")
                    .unwrap()
                    .is_some()
            );
            // overwrite
            store_named_secret("unit_test_secret", "v2").unwrap();
            assert_eq!(load_named_secret("unit_test_secret").as_deref(), Some("v2"));

            delete_named_secret("unit_test_secret");
            assert!(load_named_secret("unit_test_secret").is_none());
            // deleting a missing entry is fine
            delete_named_secret("unit_test_secret");
        });
        // No weak legacy file is ever written
        if let Some(path) = legacy::ai_keys_path() {
            assert!(
                !path.exists()
                    || !std::fs::read_to_string(path)
                        .unwrap()
                        .contains("unit_test_secret")
            );
        }
    }

    #[test]
    fn api_key_is_stored_under_ai_provider_id() {
        let store = Arc::new(SecretStore::in_memory());
        with_store_override(store.clone(), || {
            let adapter = AdapterKind::Anthropic;
            // Guard against a developer's real env var changing the outcome
            if env_var_name(adapter).is_some_and(|name| std::env::var(name).is_ok()) {
                return;
            }
            store_api_key(adapter, "sk-ant-test", &KeyStorageMethod::SecretStore).unwrap();
            assert_eq!(store_id(adapter), "ai:anthropic");
            assert_eq!(
                &*store.get_secret("ai:anthropic").unwrap().unwrap(),
                "sk-ant-test"
            );
            assert_eq!(resolve_api_key(adapter).unwrap(), "sk-ant-test");
            assert_eq!(
                detect_key_storage(adapter),
                Some(KeyStorageMethod::SecretStore)
            );
        });
    }

    #[test]
    fn locked_store_fails_closed_without_a_weak_fallback() {
        let faulty = Arc::new(FaultyBackend::new(MemoryBackend::new()));
        faulty.fail_put_with(SecretError::Locked("passphrase needed".to_string()));
        let store = Arc::new(SecretStore::with_backend(faulty, None));
        with_store_override(store, || {
            let err = store_named_secret("unit_test_locked", "value").unwrap_err();
            let message = err.to_string();
            assert!(message.contains("locked"), "{message}");
            assert!(message.contains("dbcrust secrets unlock"), "{message}");
            if let Some(path) = legacy::ai_keys_path() {
                assert!(
                    !path.exists()
                        || !std::fs::read_to_string(path)
                            .unwrap()
                            .contains("unit_test_locked"),
                    "a locked store must not downgrade to ai_keys.enc"
                );
            }
        });
    }

    #[test]
    fn legacy_file_lines_are_read_and_removed_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai_keys.enc");
        let enc = crate::password_encryption::legacy_encrypt_for_tests("legacy-key");
        std::fs::write(
            &path,
            format!("openai_api_key={enc}\nplain_api_key=plain-value\n"),
        )
        .unwrap();
        let entries = legacy::read_file_entries(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(&**entries[0].value.as_ref().unwrap(), "legacy-key");
        assert_eq!(&**entries[1].value.as_ref().unwrap(), "plain-value");

        let drop: std::collections::HashSet<String> =
            [entries[0].raw_line.clone()].into_iter().collect();
        assert_eq!(legacy::remove_file_lines(&path, &drop).unwrap(), 1);
        let left = std::fs::read_to_string(&path).unwrap();
        assert_eq!(left, "plain_api_key=plain-value\n");
        let drop: std::collections::HashSet<String> =
            [entries[1].raw_line.clone()].into_iter().collect();
        legacy::remove_file_lines(&path, &drop).unwrap();
        assert!(!path.exists(), "an emptied legacy file is removed");
    }
}
