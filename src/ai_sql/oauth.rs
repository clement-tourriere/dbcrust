//! OAuth authentication for Anthropic subscription-based access
//!
//! This module provides OAuth-based authentication for users with Anthropic subscriptions,
//! allowing them to use dbcrust without managing API keys directly.
//!
//! Supports both:
//! - Subscription-based OAuth (device flow for CLI)
//! - Traditional API key authentication (fallback)

use crate::ai_sql::error::{AiError, AiResult};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration as StdDuration;
use tokio::time::sleep;
use tracing::{debug, info};

/// OAuth token information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

impl OAuthToken {
    /// Check if the token is expired or will expire soon (within 5 minutes)
    pub fn is_expired(&self) -> bool {
        let now = Utc::now();
        let buffer = Duration::minutes(5);
        self.expires_at - buffer < now
    }

    /// Check if the token can be refreshed
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// OAuth authentication manager for Anthropic
pub struct AnthropicOAuthManager {
    client: Client,
    auth_url: String,
    token_url: String,
    client_id: String,
    token_storage_path: PathBuf,
}

/// Device authorization response
#[derive(Debug, Deserialize)]
struct DeviceAuthResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

/// Token response from OAuth server
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: Option<String>,
    scope: Option<String>,
}

/// Token refresh response
#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

impl AnthropicOAuthManager {
    /// Create a new OAuth manager
    pub fn new(config_dir: PathBuf) -> AiResult<Self> {
        let client = Client::builder()
            .timeout(StdDuration::from_secs(30))
            .build()
            .map_err(|e| AiError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        // Anthropic OAuth endpoints (these are placeholders - adjust to actual endpoints)
        let auth_url = "https://auth.anthropic.com/oauth2/device/authorize".to_string();
        let token_url = "https://auth.anthropic.com/oauth2/token".to_string();
        let client_id = "dbcrust-cli".to_string(); // This would be registered with Anthropic

        let token_storage_path = config_dir.join("anthropic_oauth_token.json");

        Ok(Self {
            client,
            auth_url,
            token_url,
            client_id,
            token_storage_path,
        })
    }

    /// Start device authorization flow
    pub async fn authenticate(&self) -> AiResult<OAuthToken> {
        info!("Starting Anthropic OAuth device authorization flow");

        // Step 1: Request device code
        let device_auth = self.request_device_code().await?;

        // Step 2: Display user code to user
        println!("\n┌─────────────────────────────────────────────────────────┐");
        println!("│ 🔐 Anthropic OAuth Authentication                      │");
        println!("├─────────────────────────────────────────────────────────┤");
        println!("│                                                         │");
        println!("│  Please visit: {}                         │", device_auth.verification_uri);
        println!("│                                                         │");
        println!("│  Enter code: {}                                  │", device_auth.user_code);
        println!("│                                                         │");
        println!("│  Waiting for authorization...                           │");
        println!("└─────────────────────────────────────────────────────────┘\n");

        // Step 3: Poll for token
        let token = self
            .poll_for_token(&device_auth.device_code, device_auth.interval)
            .await?;

        // Step 4: Save token
        self.save_token(&token).await?;

        info!("OAuth authentication successful");
        println!("✅ Authentication successful!\n");

        Ok(token)
    }

    /// Request device code from OAuth server
    async fn request_device_code(&self) -> AiResult<DeviceAuthResponse> {
        debug!("Requesting device authorization code");

        let params = [
            ("client_id", self.client_id.as_str()),
            ("scope", "api:read api:write"),
        ];

        let response = self
            .client
            .post(&self.auth_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Device auth request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: format!("Device auth failed: {}", error_text),
            });
        }

        response
            .json()
            .await
            .map_err(|e| AiError::ProviderError(format!("Failed to parse device auth response: {}", e)))
    }

    /// Poll for access token
    async fn poll_for_token(&self, device_code: &str, interval: u64) -> AiResult<OAuthToken> {
        let poll_interval = StdDuration::from_secs(interval);
        let max_attempts = 60; // 5 minutes max with 5s interval

        for attempt in 1..=max_attempts {
            debug!("Polling for token (attempt {}/{})", attempt, max_attempts);

            match self.request_token(device_code).await {
                Ok(token_response) => {
                    let expires_at = Utc::now() + Duration::seconds(token_response.expires_in as i64);
                    return Ok(OAuthToken {
                        access_token: token_response.access_token,
                        token_type: token_response.token_type,
                        expires_at,
                        refresh_token: token_response.refresh_token,
                        scope: token_response.scope,
                    });
                }
                Err(e) => {
                    // Check if error is "authorization_pending" (expected while waiting)
                    if e.to_string().contains("authorization_pending") {
                        sleep(poll_interval).await;
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(AiError::TimeoutError {
            timeout_secs: (interval * max_attempts),
        })
    }

    /// Request access token using device code
    async fn request_token(&self, device_code: &str) -> AiResult<TokenResponse> {
        let params = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", self.client_id.as_str()),
        ];

        let response = self
            .client
            .post(&self.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Token request failed: {}", e)))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            // Check for specific OAuth errors
            if body.contains("authorization_pending") {
                return Err(AiError::ProviderError("authorization_pending".to_string()));
            }
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: format!("Token request failed: {}", body),
            });
        }

        serde_json::from_str(&body)
            .map_err(|e| AiError::ProviderError(format!("Failed to parse token response: {}", e)))
    }

    /// Refresh access token
    pub async fn refresh_token(&self, refresh_token: &str) -> AiResult<OAuthToken> {
        debug!("Refreshing OAuth token");

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.client_id.as_str()),
        ];

        let response = self
            .client
            .post(&self.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Token refresh failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: format!("Token refresh failed: {}", error_text),
            });
        }

        let refresh_response: RefreshTokenResponse = response
            .json()
            .await
            .map_err(|e| AiError::ProviderError(format!("Failed to parse refresh response: {}", e)))?;

        let expires_at = Utc::now() + Duration::seconds(refresh_response.expires_in as i64);
        let token = OAuthToken {
            access_token: refresh_response.access_token,
            token_type: refresh_response.token_type,
            expires_at,
            refresh_token: refresh_response.refresh_token.or(Some(refresh_token.to_string())),
            scope: None,
        };

        self.save_token(&token).await?;

        info!("Token refreshed successfully");
        Ok(token)
    }

    /// Save token to disk (encrypted)
    async fn save_token(&self, token: &OAuthToken) -> AiResult<()> {
        let token_json = serde_json::to_string_pretty(token)
            .map_err(|e| AiError::ConfigurationError(format!("Failed to serialize token: {}", e)))?;

        // TODO: Encrypt token before saving using dbcrust's password encryption
        // For now, we'll save as-is with restrictive permissions

        tokio::fs::write(&self.token_storage_path, token_json)
            .await
            .map_err(|e| AiError::IoError(e))?;

        // Set restrictive permissions (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&self.token_storage_path)
                .await
                .map_err(|e| AiError::IoError(e))?
                .permissions();
            perms.set_mode(0o600); // Owner read/write only
            tokio::fs::set_permissions(&self.token_storage_path, perms)
                .await
                .map_err(|e| AiError::IoError(e))?;
        }

        debug!("Token saved to: {:?}", self.token_storage_path);
        Ok(())
    }

    /// Load token from disk
    pub async fn load_token(&self) -> AiResult<OAuthToken> {
        let token_json = tokio::fs::read_to_string(&self.token_storage_path)
            .await
            .map_err(|e| {
                AiError::ConfigurationError(format!("Failed to load token: {}", e))
            })?;

        // TODO: Decrypt token if encrypted

        let token: OAuthToken = serde_json::from_str(&token_json)
            .map_err(|e| AiError::ConfigurationError(format!("Failed to parse token: {}", e)))?;

        Ok(token)
    }

    /// Get valid access token (refresh if needed)
    pub async fn get_valid_token(&self) -> AiResult<String> {
        match self.load_token().await {
            Ok(mut token) => {
                if token.is_expired() {
                    if token.can_refresh() {
                        info!("Token expired, refreshing...");
                        token = self.refresh_token(token.refresh_token.as_ref().unwrap()).await?;
                    } else {
                        return Err(AiError::ConfigurationError(
                            "Token expired and cannot be refreshed. Please re-authenticate.".to_string(),
                        ));
                    }
                }
                Ok(token.access_token)
            }
            Err(_) => Err(AiError::ConfigurationError(
                "No OAuth token found. Please authenticate first using \\aiauth".to_string(),
            )),
        }
    }

    /// Check if user is authenticated
    pub async fn is_authenticated(&self) -> bool {
        self.load_token().await.is_ok()
    }

    /// Logout (remove stored token)
    pub async fn logout(&self) -> AiResult<()> {
        if self.token_storage_path.exists() {
            tokio::fs::remove_file(&self.token_storage_path)
                .await
                .map_err(|e| AiError::IoError(e))?;
            info!("OAuth token removed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_expiry() {
        let token = OAuthToken {
            access_token: "test".to_string(),
            token_type: "Bearer".to_string(),
            expires_at: Utc::now() - Duration::minutes(10), // Expired 10 minutes ago
            refresh_token: Some("refresh".to_string()),
            scope: None,
        };

        assert!(token.is_expired());
        assert!(token.can_refresh());
    }

    #[test]
    fn test_token_valid() {
        let token = OAuthToken {
            access_token: "test".to_string(),
            token_type: "Bearer".to_string(),
            expires_at: Utc::now() + Duration::hours(1), // Expires in 1 hour
            refresh_token: None,
            scope: None,
        };

        assert!(!token.is_expired());
        assert!(!token.can_refresh());
    }
}
