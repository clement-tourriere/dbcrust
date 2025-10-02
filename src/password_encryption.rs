use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, rand_core::RngCore},
};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PasswordEncryptionError {
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Invalid encrypted data format")]
    InvalidFormat,
    #[error("Failed to generate machine key: {0}")]
    MachineKeyError(String),
}

/// Generate machine-specific encryption key
/// Uses multiple machine-specific identifiers to create a deterministic key across platforms
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

/// Encrypt password data using AES-256-GCM with machine-specific key
/// Returns: nonce (12 bytes) + encrypted_data + tag (combined by aes-gcm)
pub fn encrypt_password(plaintext: &str) -> Result<String, PasswordEncryptionError> {
    let key = derive_machine_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PasswordEncryptionError::EncryptionFailed(e.to_string()))?;

    // Generate random nonce
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt the password
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| PasswordEncryptionError::EncryptionFailed(e.to_string()))?;

    // Combine nonce + ciphertext for storage
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    // Return as hex string with "enc:" prefix
    Ok(format!("enc:{}", hex::encode(result)))
}

/// Decrypt password data using AES-256-GCM with machine-specific key
/// Expects format: "enc:hex_encoded_data" where hex_encoded_data is nonce (12 bytes) + encrypted_data + tag
pub fn decrypt_password(encrypted_password: &str) -> Result<String, PasswordEncryptionError> {
    // Check if password is encrypted (has "enc:" prefix)
    if !encrypted_password.starts_with("enc:") {
        // Not encrypted - return as-is (plaintext password)
        return Ok(encrypted_password.to_string());
    }

    // Remove "enc:" prefix
    let hex_data = &encrypted_password[4..];
    let encrypted_data =
        hex::decode(hex_data).map_err(|_| PasswordEncryptionError::InvalidFormat)?;

    if encrypted_data.len() < 12 {
        return Err(PasswordEncryptionError::InvalidFormat);
    }

    let key = derive_machine_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| PasswordEncryptionError::DecryptionFailed(e.to_string()))?;

    // Extract nonce and ciphertext
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    // Decrypt the password
    let plaintext_bytes = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| PasswordEncryptionError::DecryptionFailed(e.to_string()))?;

    String::from_utf8(plaintext_bytes)
        .map_err(|e| PasswordEncryptionError::DecryptionFailed(e.to_string()))
}

/// Check if a password string is encrypted (starts with "enc:" prefix)
pub fn is_encrypted(password: &str) -> bool {
    password.starts_with("enc:")
}

/// Encrypt a plaintext password file in place
/// Reads the file, encrypts all passwords, and writes back
pub fn encrypt_password_file<P: AsRef<Path>>(file_path: P) -> Result<(), PasswordEncryptionError> {
    let content = fs::read_to_string(&file_path).map_err(|e| {
        PasswordEncryptionError::MachineKeyError(format!("Failed to read file: {e}"))
    })?;

    let mut encrypted_lines = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            // Keep comments and empty lines as-is
            encrypted_lines.push(line.to_string());
            continue;
        }

        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 6 {
            // Format: database_type:host:port:database:username:password
            let password = parts[5];
            if !is_encrypted(password) {
                // Encrypt the password
                let encrypted_password = encrypt_password(password)?;
                let new_line = format!(
                    "{}:{}:{}:{}:{}:{}",
                    parts[0], parts[1], parts[2], parts[3], parts[4], encrypted_password
                );
                encrypted_lines.push(new_line);
            } else {
                // Already encrypted
                encrypted_lines.push(line.to_string());
            }
        } else {
            // Invalid format - keep as-is
            encrypted_lines.push(line.to_string());
        }
    }

    // Write back encrypted content
    fs::write(&file_path, encrypted_lines.join("\n")).map_err(|e| {
        PasswordEncryptionError::MachineKeyError(format!("Failed to write file: {e}"))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Global mutex to ensure password encryption tests don't interfere with each other
    static PASSWORD_ENCRYPTION_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_encrypt_decrypt_password() {
        let _guard = PASSWORD_ENCRYPTION_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let original_password = "my_secret_password_123!";

        // Encrypt
        let encrypted = encrypt_password(original_password).expect("Encryption should succeed");
        assert!(encrypted.starts_with("enc:"));
        assert!(encrypted.len() > original_password.len());
        assert_ne!(encrypted, original_password);

        // Decrypt
        let decrypted = decrypt_password(&encrypted).expect("Decryption should succeed");
        assert_eq!(decrypted, original_password);
    }

    #[test]
    fn test_plaintext_password_passthrough() {
        let _guard = PASSWORD_ENCRYPTION_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let plaintext_password = "plaintext_password";

        // Plaintext passwords should be returned as-is
        let result = decrypt_password(plaintext_password).expect("Should handle plaintext");
        assert_eq!(result, plaintext_password);
    }

    #[test]
    fn test_is_encrypted() {
        assert!(is_encrypted("enc:abcdef123456"));
        assert!(!is_encrypted("plaintext_password"));
        assert!(!is_encrypted(""));
    }

    #[test]
    fn test_invalid_encrypted_format() {
        let _guard = PASSWORD_ENCRYPTION_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // Test with invalid hex
        let result = decrypt_password("enc:invalid-hex-data");
        assert!(matches!(
            result,
            Err(PasswordEncryptionError::InvalidFormat)
        ));

        // Test with data too short
        let result = decrypt_password("enc:0123456789");
        assert!(matches!(
            result,
            Err(PasswordEncryptionError::InvalidFormat)
        ));
    }

    #[test]
    fn test_multiple_encryptions_are_different() {
        let _guard = PASSWORD_ENCRYPTION_MUTEX
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let password = "same_password";

        let encrypted1 = encrypt_password(password).expect("First encryption should succeed");
        let encrypted2 = encrypt_password(password).expect("Second encryption should succeed");

        // Different nonces should make encryptions different
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same password
        assert_eq!(decrypt_password(&encrypted1).unwrap(), password);
        assert_eq!(decrypt_password(&encrypted2).unwrap(), password);
    }
}
