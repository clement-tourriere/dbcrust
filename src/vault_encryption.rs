use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EncryptionError {
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Invalid encrypted data format")]
    InvalidFormat,
    #[error("Failed to get vault token for key derivation")]
    NoVaultToken,
}

/// Generate encryption key from Vault token
/// Uses SHA-256 to derive a 32-byte key from the vault token
fn derive_key_from_vault_token() -> Result<[u8; 32], EncryptionError> {
    // Get vault token - try environment variable first, then token file
    let vault_token = match std::env::var("VAULT_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token.trim().to_string(),
        _ => {
            // Try reading from ~/.vault-token file
            let token_path = dirs::home_dir()
                .ok_or(EncryptionError::NoVaultToken)?
                .join(".vault-token");

            std::fs::read_to_string(token_path)
                .map(|s| s.trim().to_string())
                .map_err(|_| EncryptionError::NoVaultToken)?
        }
    };

    if vault_token.is_empty() {
        return Err(EncryptionError::NoVaultToken);
    }

    // Use SHA-256 to derive a deterministic key from the vault token
    let mut hasher = Sha256::new();
    hasher.update(vault_token.as_bytes());
    hasher.update(b"dbcrust-vault-credentials"); // Salt for additional security
    let hash = hasher.finalize();

    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    Ok(key)
}

/// Encrypt data using AES-256-GCM
/// Returns: nonce (12 bytes) + encrypted_data + tag (combined by aes-gcm)
pub fn encrypt_data(plaintext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    let key = derive_key_from_vault_token()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| EncryptionError::EncryptionFailed(e.to_string()))?;

    // Generate random nonce
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt the data
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| EncryptionError::EncryptionFailed(e.to_string()))?;

    // Combine nonce + ciphertext for storage
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypt data using AES-256-GCM
/// Expects: nonce (12 bytes) + encrypted_data + tag (combined by aes-gcm)
pub fn decrypt_data(encrypted_data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    if encrypted_data.len() < 12 {
        return Err(EncryptionError::InvalidFormat);
    }

    let key = derive_key_from_vault_token()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))?;

    // Extract nonce and ciphertext
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    // Decrypt the data
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))?;

    Ok(plaintext)
}

/// Encrypt a string and return base64-encoded result
pub fn encrypt_string(plaintext: &str) -> Result<String, EncryptionError> {
    let encrypted_data = encrypt_data(plaintext.as_bytes())?;
    Ok(hex::encode(encrypted_data))
}

/// Decrypt a base64-encoded string
pub fn decrypt_string(encrypted_hex: &str) -> Result<String, EncryptionError> {
    let encrypted_data = hex::decode(encrypted_hex).map_err(|_e| EncryptionError::InvalidFormat)?;
    let plaintext_bytes = decrypt_data(&encrypted_data)?;
    String::from_utf8(plaintext_bytes).map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Global mutex to ensure vault encryption tests don't interfere with each other
    static VAULT_TOKEN_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_encrypt_decrypt_data() {
        let _guard = VAULT_TOKEN_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Save current token
        let original_token = std::env::var("VAULT_TOKEN").ok();

        // Set a test vault token
        unsafe {
            std::env::set_var("VAULT_TOKEN", "test-vault-token-consistent");
        }

        let original_data = b"Hello, World! This is a test message.";

        // Encrypt
        let encrypted = encrypt_data(original_data).expect("Encryption should succeed");
        assert!(encrypted.len() > original_data.len()); // Should be larger due to nonce + tag

        // Decrypt
        let decrypted = decrypt_data(&encrypted).expect("Decryption should succeed");
        assert_eq!(decrypted, original_data);

        // Restore original token or clean up
        match original_token {
            Some(token) => unsafe {
                std::env::set_var("VAULT_TOKEN", token);
            },
            None => unsafe {
                std::env::remove_var("VAULT_TOKEN");
            },
        }
    }

    #[test]
    fn test_encrypt_decrypt_string() {
        let _guard = VAULT_TOKEN_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Save current token
        let original_token = std::env::var("VAULT_TOKEN").ok();

        // Set a test vault token (same as other tests for consistency)
        unsafe {
            std::env::set_var("VAULT_TOKEN", "test-vault-token-consistent");
        }

        let original_string = "This is a secret vault credential password!";

        // Encrypt
        let encrypted_hex =
            encrypt_string(original_string).expect("String encryption should succeed");
        assert!(!encrypted_hex.is_empty());
        assert_ne!(encrypted_hex, original_string);

        // Decrypt
        let decrypted_string =
            decrypt_string(&encrypted_hex).expect("String decryption should succeed");
        assert_eq!(decrypted_string, original_string);

        // Restore original token or clean up
        match original_token {
            Some(token) => unsafe {
                std::env::set_var("VAULT_TOKEN", token);
            },
            None => unsafe {
                std::env::remove_var("VAULT_TOKEN");
            },
        }
    }

    #[test]
    fn test_invalid_format() {
        let _guard = VAULT_TOKEN_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // Save current token
        let original_token = std::env::var("VAULT_TOKEN").ok();

        unsafe {
            std::env::set_var("VAULT_TOKEN", "test-vault-token-consistent");
        }

        // Test with data too short (less than 12 bytes for nonce)
        let result = decrypt_data(&[1, 2, 3, 4, 5]);
        assert!(matches!(result, Err(EncryptionError::InvalidFormat)));

        // Test with invalid hex string
        let result = decrypt_string("invalid-hex-string");
        assert!(matches!(result, Err(EncryptionError::InvalidFormat)));

        // Restore original token or clean up
        match original_token {
            Some(token) => unsafe {
                std::env::set_var("VAULT_TOKEN", token);
            },
            None => unsafe {
                std::env::remove_var("VAULT_TOKEN");
            },
        }
    }

    #[test]
    fn test_no_vault_token() {
        let _guard = VAULT_TOKEN_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

        // This test verifies the NoVaultToken error case
        // Skip if ~/.vault-token exists (common in development environments)
        let vault_token_file = dirs::home_dir().map(|d| d.join(".vault-token"));
        if let Some(path) = vault_token_file
            && path.exists()
        {
            eprintln!("Skipping test because ~/.vault-token exists");
            return;
        }

        // Save current token if it exists
        let original_token = std::env::var("VAULT_TOKEN").ok();

        // Ensure no vault token is set
        unsafe {
            std::env::remove_var("VAULT_TOKEN");
        }

        let result = encrypt_string("test");
        assert!(matches!(result, Err(EncryptionError::NoVaultToken)));

        // Restore original token if it existed
        if let Some(token) = original_token {
            unsafe {
                std::env::set_var("VAULT_TOKEN", token);
            }
        }
    }
}
