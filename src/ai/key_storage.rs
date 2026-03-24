//! API key resolution: env vars -> OS keyring -> encrypted file.
//!
//! Keyed by `genai`'s [`AdapterKind`] (the provider), so dbcrust does not keep
//! its own provider enum. Env var names come from genai's `default_key_env_name`.

use crate::ai::AiError;
use crate::config::Config;
use genai::adapter::AdapterKind;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyStorageMethod {
    OsKeyring,
    EncryptedFile,
    EnvVarHint,
}

impl std::fmt::Display for KeyStorageMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyStorageMethod::OsKeyring => write!(f, "OS Keychain"),
            KeyStorageMethod::EncryptedFile => write!(f, "Encrypted file"),
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

/// Stable identifier for keyring entries and encrypted-file lines.
fn key_name(adapter: AdapterKind) -> String {
    format!("{}_api_key", adapter.as_lower_str())
}

/// Resolve API key using 3-layer fallback: env var -> OS keyring -> encrypted file
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

    // 2. OS Keyring (macOS Keychain, Linux Secret Service, Windows Credential Manager)
    if let Ok(key) = get_keyring_key(adapter) {
        return Ok(key);
    }

    // 3. Encrypted file (~/.config/dbcrust/ai_keys.enc)
    if let Ok(key) = load_encrypted_key(adapter) {
        return Ok(key);
    }

    Err(AiError::MissingApiKey(adapter.as_str().to_string()))
}

/// Store API key using the specified method
pub fn store_api_key(
    adapter: AdapterKind,
    key: &str,
    method: &KeyStorageMethod,
) -> Result<(), AiError> {
    match method {
        KeyStorageMethod::OsKeyring => store_keyring_key(adapter, key),
        KeyStorageMethod::EncryptedFile => store_encrypted_key(adapter, key),
        KeyStorageMethod::EnvVarHint => {
            let env_name = env_var_name(adapter).unwrap_or("DBCRUST_AI_API_KEY");
            println!("\nAdd this to your shell profile:\n  export {env_name}={key}");
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
    if get_keyring_key(adapter).is_ok() {
        return Some(KeyStorageMethod::OsKeyring);
    }
    if load_encrypted_key(adapter).is_ok() {
        return Some(KeyStorageMethod::EncryptedFile);
    }
    None
}

// --- OS Keyring ---

fn get_keyring_key(adapter: AdapterKind) -> Result<String, AiError> {
    let entry = keyring::Entry::new("dbcrust", &key_name(adapter))
        .map_err(|e| AiError::KeyStorageError(format!("Keyring init error: {e}")))?;
    entry
        .get_password()
        .map_err(|e| AiError::KeyStorageError(format!("Keyring read error: {e}")))
}

fn store_keyring_key(adapter: AdapterKind, key: &str) -> Result<(), AiError> {
    let entry = keyring::Entry::new("dbcrust", &key_name(adapter))
        .map_err(|e| AiError::KeyStorageError(format!("Keyring init error: {e}")))?;
    entry
        .set_password(key)
        .map_err(|e| AiError::KeyStorageError(format!("Keyring store error: {e}")))
}

// --- Encrypted File ---

fn get_ai_keys_path() -> Result<PathBuf, AiError> {
    Config::get_config_directory()
        .map(|dir| dir.join("ai_keys.enc"))
        .map_err(|e| AiError::KeyStorageError(format!("Config dir error: {e}")))
}

fn load_encrypted_key(adapter: AdapterKind) -> Result<String, AiError> {
    let path = get_ai_keys_path()?;
    if !path.exists() {
        return Err(AiError::KeyStorageError(
            "No encrypted key file".to_string(),
        ));
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| AiError::KeyStorageError(format!("Read error: {e}")))?;

    let key_name = key_name(adapter);
    for line in content.lines() {
        if let Some((name, encrypted_value)) = line.split_once('=')
            && name.trim() == key_name
        {
            let value = encrypted_value.trim();
            if value.starts_with("enc:") {
                match crate::password_encryption::decrypt_password(value) {
                    Ok(decrypted) => return Ok(decrypted),
                    Err(e) => {
                        return Err(AiError::KeyStorageError(format!("Decryption failed: {e}")));
                    }
                }
            } else {
                return Ok(value.to_string());
            }
        }
    }

    Err(AiError::KeyStorageError(format!(
        "Key not found for {}",
        adapter.as_str()
    )))
}

fn store_encrypted_key(adapter: AdapterKind, key: &str) -> Result<(), AiError> {
    let path = get_ai_keys_path()?;
    let key_name = key_name(adapter);

    // Encrypt the key
    let encrypted = crate::password_encryption::encrypt_password(key)
        .map_err(|e| AiError::KeyStorageError(format!("Encryption failed: {e}")))?;

    // Read existing content (if any)
    let mut lines: Vec<String> = if path.exists() {
        fs::read_to_string(&path)
            .map_err(|e| AiError::KeyStorageError(format!("Read error: {e}")))?
            .lines()
            .map(|l| l.to_string())
            .collect()
    } else {
        Vec::new()
    };

    // Update or add the key
    let mut found = false;
    for line in &mut lines {
        if line.starts_with(&format!("{key_name}=")) || line.starts_with(&format!("{key_name} =")) {
            *line = format!("{key_name}={encrypted}");
            found = true;
            break;
        }
    }
    if !found {
        lines.push(format!("{key_name}={encrypted}"));
    }

    // Create/write with restrictive permissions from the start on Unix so the
    // key is never briefly world-readable under the default umask.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| AiError::KeyStorageError(format!("Open error: {e}")))?;
        file.write_all((lines.join("\n") + "\n").as_bytes())
            .map_err(|e| AiError::KeyStorageError(format!("Write error: {e}")))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&path, lines.join("\n") + "\n")
            .map_err(|e| AiError::KeyStorageError(format!("Write error: {e}")))?;
    }

    Ok(())
}
