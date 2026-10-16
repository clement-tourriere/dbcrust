//! Legacy machine-key reader for `enc:` values.
//!
//! Older DBCrust versions "encrypted" `~/.dbcrust` passwords and
//! `ai_keys.enc` with an AES key derived from machine identifiers. Any
//! process of the same user can re-derive that key, so it protects nothing
//! and has been retired: this module only *decrypts* such values, for
//! migration to the secret store (`dbcrust secrets migrate`) and the
//! read-only legacy lookups. See `dbcrust_secrets` for the real store.

#[cfg(any(test, feature = "test-support"))]
use aes_gcm::aead::{OsRng, rand_core::RngCore};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use sha2::{Digest, Sha256};
use std::fs;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PasswordEncryptionError {
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Invalid encrypted data format")]
    InvalidFormat,
    #[error("Failed to generate machine key: {0}")]
    MachineKeyError(String),
}

/// Re-derive the legacy machine-specific key (decrypt-only).
fn derive_machine_key() -> Result<[u8; 32], PasswordEncryptionError> {
    let mut hasher = Sha256::new();

    // Platform-specific machine ID
    #[cfg(target_os = "linux")]
    {
        // Linux: Use systemd machine-id
        if let Ok(machine_id) = fs::read_to_string("/etc/machine-id") {
            hasher.update(machine_id.trim().as_bytes());
        } else if let Ok(machine_id) = fs::read_to_string("/var/lib/dbus/machine-id") {
            hasher.update(machine_id.trim().as_bytes());
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: Use IOKit Hardware UUID
        if let Ok(output) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            // Extract IOPlatformUUID from the output
            let output_str = String::from_utf8_lossy(&output.stdout);
            if let Some(uuid_line) = output_str
                .lines()
                .find(|line| line.contains("IOPlatformUUID"))
            {
                hasher.update(uuid_line.as_bytes());
            } else {
                // Fallback: use the whole output
                hasher.update(&output.stdout);
            }
        } else {
            // Alternative: use system_profiler
            if let Ok(output) = std::process::Command::new("system_profiler")
                .args(["SPHardwareDataType"])
                .output()
            {
                let output_str = String::from_utf8_lossy(&output.stdout);
                if let Some(uuid_line) = output_str
                    .lines()
                    .find(|line| line.contains("Hardware UUID"))
                {
                    hasher.update(uuid_line.as_bytes());
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: Use Machine GUID from registry or WMI
        if let Ok(output) = std::process::Command::new("powershell")
            .args(&["-Command", "Get-CimInstance -Class Win32_ComputerSystemProduct | Select-Object -ExpandProperty UUID"])
            .output()
        {
            hasher.update(&output.stdout);
        } else if let Ok(output) = std::process::Command::new("wmic")
            .args(&["csproduct", "get", "UUID", "/value"])
            .output()
        {
            hasher.update(&output.stdout);
        }
    }

    // Common elements for all platforms

    // Add user home directory as part of the key
    if let Some(home) = dirs::home_dir() {
        hasher.update(home.to_string_lossy().as_bytes());
    }

    // Add hostname
    if let Ok(hostname) = hostname::get() {
        hasher.update(hostname.to_string_lossy().as_bytes());
    }

    // Add username for additional uniqueness
    if let Ok(username) = std::env::var("USER").or_else(|_| std::env::var("USERNAME"))
    // Windows uses USERNAME
    {
        hasher.update(username.as_bytes());
    }

    // Add current user ID (Unix systems)
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Some(home) = dirs::home_dir() {
            if let Ok(metadata) = fs::metadata(&home) {
                let uid = metadata.uid();
                hasher.update(uid.to_le_bytes());
            }
        }
    }

    // Fixed salt for additional security
    hasher.update(b"dbcrust-password-encryption-v1");

    let hash = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    Ok(key)
}

/// Decrypt a legacy `enc:<hex>` value written by older DBCrust versions.
///
/// **Decrypt-only.** The machine-derived key is no protection against another
/// process of the same user, so DBCrust never produces `enc:` values any
/// more; this reader exists so `dbcrust secrets migrate` and the read-only
/// legacy lookups can still open what is already on disk. Values without the
/// `enc:` prefix are returned as-is (legacy files could hold plaintext).
/// Expects `enc:` + hex(nonce[12] ‖ ciphertext ‖ tag).
pub fn legacy_decrypt(value: &str) -> Result<String, PasswordEncryptionError> {
    let Some(hex_data) = value.strip_prefix("enc:") else {
        return Ok(value.to_string());
    };
    let encrypted_data =
        hex::decode(hex_data).map_err(|_| PasswordEncryptionError::InvalidFormat)?;

    if encrypted_data.len() < 12 {
        return Err(PasswordEncryptionError::InvalidFormat);
    }

    let key = derive_machine_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PasswordEncryptionError::DecryptionFailed(e.to_string()))?;

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext_bytes = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| PasswordEncryptionError::DecryptionFailed(e.to_string()))?;

    String::from_utf8(plaintext_bytes)
        .map_err(|e| PasswordEncryptionError::DecryptionFailed(e.to_string()))
}

/// True for a value carrying the legacy `enc:` prefix.
pub fn is_legacy_encrypted(value: &str) -> bool {
    value.starts_with("enc:")
}

/// Produce a legacy `enc:` value. Test-only: it exists so the legacy reader
/// can be exercised; no production code path may create such values.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn legacy_encrypt_for_tests(plaintext: &str) -> String {
    let key = derive_machine_key().expect("machine key");
    let cipher = Aes256Gcm::new_from_slice(&key).expect("key length");
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
        .expect("encrypt");
    let mut out = nonce_bytes.to_vec();
    out.extend_from_slice(&ciphertext);
    format!("enc:{}", hex::encode(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_values_decrypt_but_are_never_produced_by_production_code() {
        let original = "my_secret_password_123!";
        let legacy = legacy_encrypt_for_tests(original);
        assert!(is_legacy_encrypted(&legacy));
        assert_ne!(legacy, original);
        assert_eq!(legacy_decrypt(&legacy).unwrap(), original);
    }

    #[test]
    fn plaintext_passes_through() {
        assert_eq!(
            legacy_decrypt("plaintext_password").unwrap(),
            "plaintext_password"
        );
    }

    #[test]
    fn is_legacy_encrypted_checks_the_prefix() {
        assert!(is_legacy_encrypted("enc:abcdef123456"));
        assert!(!is_legacy_encrypted("plaintext_password"));
        assert!(!is_legacy_encrypted(""));
    }

    #[test]
    fn invalid_formats_are_rejected() {
        assert!(matches!(
            legacy_decrypt("enc:invalid-hex-data"),
            Err(PasswordEncryptionError::InvalidFormat)
        ));
        assert!(matches!(
            legacy_decrypt("enc:0123456789"),
            Err(PasswordEncryptionError::InvalidFormat)
        ));
        // Well-formed but encrypted under another key: a decryption failure.
        let foreign = format!("enc:{}", hex::encode([7u8; 40]));
        assert!(matches!(
            legacy_decrypt(&foreign),
            Err(PasswordEncryptionError::DecryptionFailed(_))
        ));
    }

    #[test]
    fn module_source_has_no_encrypt_entry_point() {
        // The decrypt-only contract: nothing public can create `enc:` values.
        let source = include_str!("password_encryption.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!production.contains("pub fn encrypt"));
        assert!(!production.contains("pub fn encrypt_password"));
    }
}
