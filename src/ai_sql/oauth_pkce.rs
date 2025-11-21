//! OAuth authentication with PKCE for Anthropic
//!
//! Implements authorization code flow with PKCE (Proof Key for Code Exchange)
//! matching the implementation from opencode-anthropic-auth.

use crate::ai_sql::error::{AiError, AiResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use rand::distr::{Alphanumeric, SampleString};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration as StdDuration;
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

/// PKCE verifier and challenge
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl PkceChallenge {
    /// Generate a new PKCE challenge
    pub fn generate() -> Self {
        let mut rng = rand::rng();

        // Generate random verifier (43-128 characters)
        let verifier = Alphanumeric.sample_string(&mut rng, 64);

        // Generate random state for CSRF protection
        let state = Alphanumeric.sample_string(&mut rng, 32);

        // Create S256 challenge: BASE64URL(SHA256(ASCII(code_verifier)))
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash_result = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash_result);

        Self {
            verifier,
            challenge,
            state,
        }
    }
}

/// OAuth authentication manager for Anthropic with PKCE
pub struct AnthropicOAuthPkce {
    client: Client,
    auth_url: String,
    token_url: String,
    redirect_uri: String,
    client_id: String,
    scopes: String,
    token_storage_path: PathBuf,
}

impl AnthropicOAuthPkce {
    /// Create a new OAuth manager with real Anthropic endpoints
    pub fn new(config_dir: PathBuf) -> AiResult<Self> {
        let client = Client::builder()
            .timeout(StdDuration::from_secs(30))
            .build()
            .map_err(|e| AiError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        // Real Anthropic OAuth endpoints (from opencode-anthropic-auth)
        let auth_url = "https://claude.ai/oauth/authorize".to_string();
        let token_url = "https://console.anthropic.com/v1/oauth/token".to_string();

        // Anthropic's callback page (displays code for user to copy)
        let redirect_uri = "https://console.anthropic.com/oauth/code/callback".to_string();

        // Public client ID from opencode-anthropic-auth
        let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string();

        // Required scopes
        let scopes = "org:create_api_key user:profile user:inference".to_string();

        let token_storage_path = config_dir.join("anthropic_oauth_token.json");

        Ok(Self {
            client,
            auth_url,
            token_url,
            redirect_uri,
            client_id,
            scopes,
            token_storage_path,
        })
    }

    /// Start authorization code flow with PKCE
    /// Returns the URL for the user to visit
    pub fn start_authorization(&self, pkce: &PkceChallenge) -> String {
        let params = vec![
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", self.scopes.as_str()),
            ("state", pkce.state.as_str()),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
        ];

        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

        let query_string = params
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    k,
                    utf8_percent_encode(v, NON_ALPHANUMERIC)
                )
            })
            .collect::<Vec<_>>()
            .join("&");

        format!("{}?{}", self.auth_url, query_string)
    }

    /// Exchange authorization code for access token
    pub async fn exchange_code(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> AiResult<OAuthToken> {
        info!("Exchanging authorization code for access token");

        let params = [
            ("grant_type", "authorization_code"),
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("code", code),
            ("code_verifier", pkce_verifier),
        ];

        let response = self
            .client
            .post(&self.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Token exchange request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: format!("Token exchange failed: {}", error_text),
            });
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| AiError::ProviderError(format!("Failed to parse token response: {}", e)))?;

        let expires_at = Utc::now() + Duration::seconds(token_response.expires_in as i64);

        let token = OAuthToken {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_at,
            refresh_token: token_response.refresh_token,
            scope: token_response.scope,
        };

        // Save token
        self.save_token(&token).await?;

        Ok(token)
    }

    /// Refresh access token using refresh token
    pub async fn refresh_token(&self, refresh_token: &str) -> AiResult<OAuthToken> {
        info!("Refreshing access token");

        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", self.client_id.as_str()),
            ("refresh_token", refresh_token),
        ];

        let response = self
            .client
            .post(&self.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| AiError::NetworkError(format!("Token refresh request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(AiError::ApiError {
                status_code: status.as_u16(),
                message: format!("Token refresh failed: {}", error_text),
            });
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| AiError::ProviderError(format!("Failed to parse token response: {}", e)))?;

        let expires_at = Utc::now() + Duration::seconds(token_response.expires_in as i64);

        let token = OAuthToken {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_at,
            refresh_token: token_response.refresh_token.or(Some(refresh_token.to_string())),
            scope: token_response.scope,
        };

        // Save refreshed token
        self.save_token(&token).await?;

        Ok(token)
    }

    /// Get a valid token, refreshing if necessary
    pub async fn get_valid_token(&self) -> AiResult<String> {
        // Load existing token
        let token = self.load_token().await?;

        // Check if expired
        if token.is_expired() {
            if let Some(refresh_token) = &token.refresh_token {
                debug!("Token expired, refreshing...");
                let new_token = self.refresh_token(refresh_token).await?;
                return Ok(new_token.access_token);
            } else {
                return Err(AiError::AuthenticationError(
                    "Token expired and no refresh token available. Please re-authenticate.".to_string(),
                ));
            }
        }

        Ok(token.access_token)
    }

    /// Save token to storage
    async fn save_token(&self, token: &OAuthToken) -> AiResult<()> {
        let json = serde_json::to_string_pretty(token)
            .map_err(|e| AiError::ConfigurationError(format!("Failed to serialize token: {}", e)))?;

        tokio::fs::write(&self.token_storage_path, json)
            .await
            .map_err(|e| {
                AiError::ConfigurationError(format!("Failed to write token file: {}", e))
            })?;

        // Set restrictive permissions (Unix only)
        #[cfg(unix)]
        {
            use std::fs;
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&self.token_storage_path, perms)
                .map_err(|e| {
                    AiError::ConfigurationError(format!("Failed to set token file permissions: {}", e))
                })?;
        }

        info!("Token saved to {:?}", self.token_storage_path);
        Ok(())
    }

    /// Load token from storage
    pub async fn load_token(&self) -> AiResult<OAuthToken> {
        let token_json = tokio::fs::read_to_string(&self.token_storage_path)
            .await
            .map_err(|e| {
                AiError::AuthenticationError(format!(
                    "No saved token found. Please run 'dbcrust ai-auth login' to authenticate: {}",
                    e
                ))
            })?;

        serde_json::from_str(&token_json).map_err(|e| {
            AiError::AuthenticationError(format!("Failed to parse saved token: {}", e))
        })
    }

    /// Remove stored token
    pub async fn logout(&self) -> AiResult<()> {
        if self.token_storage_path.exists() {
            tokio::fs::remove_file(&self.token_storage_path)
                .await
                .map_err(|e| AiError::ConfigurationError(format!("Failed to remove token: {}", e)))?;
            info!("Token removed");
        }
        Ok(())
    }

    /// Check if user is authenticated
    pub async fn is_authenticated(&self) -> bool {
        self.load_token().await.is_ok()
    }
}

// Token response structure
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: Option<String>,
    scope: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_generation() {
        let pkce = PkceChallenge::generate();
        assert!(!pkce.verifier.is_empty());
        assert!(!pkce.challenge.is_empty());
        assert_ne!(pkce.verifier, pkce.challenge);
    }

    #[test]
    fn test_authorization_url() {
        let config_dir = std::env::temp_dir();
        let oauth = AnthropicOAuthPkce::new(config_dir).unwrap();
        let pkce = PkceChallenge::generate();

        let url = oauth.start_authorization(&pkce);
        assert!(url.starts_with("https://claude.ai/oauth/authorize"));
        assert!(url.contains("client_id="));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
    }
}
