//! The pre-store locations of AI keys and OAuth tokens: the old unscoped
//! `dbcrust` keychain entries and the machine-key `ai_keys.enc` file.
//!
//! Kept for reading and migration only (`dbcrust secrets migrate`, `secrets
//! audit`, and the read-only fallback in `ai::key_storage`). Nothing here
//! knows about the AI assistant: callers that probe the per-provider keychain
//! accounts pass the provider names in.

use crate::SecretError;
use std::path::PathBuf;

/// Name under which the ChatGPT-subscription OAuth tokens are stored.
pub const CHATGPT_TOKENS_SECRET: &str = "chatgpt_oauth_tokens";

/// Providers of the `\ai setup` wizard (lower-case genai names). The old
/// keychain only ever held entries for these; `ai::suggested_providers()`
/// is asserted equal to this list in the `ai` tests.
pub const SUGGESTED_PROVIDER_NAMES: &[&str] = &[
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
];

/// [`SUGGESTED_PROVIDER_NAMES`] as owned strings, the shape
/// [`keychain_account_names`] takes.
pub fn suggested_provider_names() -> Vec<String> {
    SUGGESTED_PROVIDER_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Name the legacy keychain entry and `ai_keys.enc` line used for a provider
/// (`provider` is the lower-case genai name, e.g. `anthropic`).
pub fn legacy_key_name(provider: &str) -> String {
    format!("{provider}_api_key")
}

/// Service name of the old keychain entries.
const LEGACY_SERVICE: &str = "dbcrust";

/// `~/.config/dbcrust/ai_keys.enc`.
pub fn ai_keys_path() -> Option<PathBuf> {
    crate::paths::config_directory()
        .ok()
        .map(|dir| dir.join("ai_keys.enc"))
}

/// Account names of the old keychain entries worth probing: one per
/// known provider plus the ChatGPT token blob.
pub fn keychain_account_names(suggested_providers: &[String]) -> Vec<String> {
    let mut names: Vec<String> = suggested_providers
        .iter()
        .map(|provider| legacy_key_name(provider))
        .collect();
    names.push(CHATGPT_TOKENS_SECRET.to_string());
    names.sort();
    names.dedup();
    names
}

/// Test builds must never reach the developer's real keychain.
fn keychain_off() -> bool {
    crate::paths::is_test_mode()
}

/// Whether an old keychain entry exists, without reading its secret
/// where the platform allows (attributes only).
pub fn keychain_entry_exists(name: &str) -> bool {
    if keychain_off() {
        return false;
    }
    keyring::Entry::new(LEGACY_SERVICE, name)
        .ok()
        .is_some_and(|entry| entry.get_attributes().is_ok())
}

/// The old keychain entry's value (migration, legacy fallback).
pub fn keychain_get(name: &str) -> Option<zeroize::Zeroizing<String>> {
    if keychain_off() {
        return None;
    }
    keyring::Entry::new(LEGACY_SERVICE, name)
        .ok()?
        .get_password()
        .ok()
        .map(zeroize::Zeroizing::new)
}

pub fn keychain_delete(name: &str) -> bool {
    if keychain_off() {
        return false;
    }
    keyring::Entry::new(LEGACY_SERVICE, name)
        .ok()
        .is_some_and(|entry| entry.delete_credential().is_ok())
}

/// One `name=value` line of the old encrypted file; the value is
/// decrypted through the legacy reader (`Err` when it cannot be).
pub struct FileEntry {
    pub name: String,
    pub raw_line: String,
    pub value: Result<zeroize::Zeroizing<String>, String>,
}

pub fn read_file_entries(path: &std::path::Path) -> Vec<FileEntry> {
    let Ok(Some(bytes)) = crate::fsutil::read_bounded(path, 1024 * 1024) else {
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
            let value = crate::legacy::password_encryption::legacy_decrypt(value.trim())
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
pub fn read_named(name: &str) -> Option<String> {
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
pub fn delete_named(name: &str) {
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
pub fn remove_file_lines(
    path: &std::path::Path,
    drop: &std::collections::HashSet<String>,
) -> Result<usize, SecretError> {
    let Some(bytes) = crate::fsutil::read_bounded(path, 1024 * 1024)? else {
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
        crate::fsutil::write_private_atomic(path, content.as_bytes())?;
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_file_lines_are_read_and_removed_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai_keys.enc");
        let enc = crate::legacy::password_encryption::legacy_encrypt_for_tests("legacy-key");
        std::fs::write(
            &path,
            format!("openai_api_key={enc}\nplain_api_key=plain-value\n"),
        )
        .unwrap();
        let entries = read_file_entries(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(&**entries[0].value.as_ref().unwrap(), "legacy-key");
        assert_eq!(&**entries[1].value.as_ref().unwrap(), "plain-value");

        let drop: std::collections::HashSet<String> =
            [entries[0].raw_line.clone()].into_iter().collect();
        assert_eq!(remove_file_lines(&path, &drop).unwrap(), 1);
        let left = std::fs::read_to_string(&path).unwrap();
        assert_eq!(left, "plain_api_key=plain-value\n");
        let drop: std::collections::HashSet<String> =
            [entries[1].raw_line.clone()].into_iter().collect();
        remove_file_lines(&path, &drop).unwrap();
        assert!(!path.exists(), "an emptied legacy file is removed");
    }

    #[test]
    fn keychain_accounts_are_derived_from_the_given_providers() {
        let providers = vec!["anthropic".to_string(), "openai".to_string()];
        assert_eq!(
            keychain_account_names(&providers),
            vec![
                "anthropic_api_key".to_string(),
                "chatgpt_oauth_tokens".to_string(),
                "openai_api_key".to_string(),
            ]
        );
        // No providers: only the ChatGPT token blob is probed
        assert_eq!(
            keychain_account_names(&[]),
            vec!["chatgpt_oauth_tokens".to_string()]
        );
    }
}
