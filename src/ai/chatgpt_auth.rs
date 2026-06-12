//! "Sign in with ChatGPT": OAuth 2.0 + PKCE against auth.openai.com using the
//! public Codex CLI client, so OpenAI requests can ride the user's ChatGPT
//! plan (through the Codex backend) instead of a pay-per-use API key.
//!
//! This mirrors what Codex CLI, opencode and llm-openai-via-codex do. It is
//! not an officially documented OpenAI API surface — OpenAI has publicly
//! tolerated subscription use outside Codex since April 2026, but the backend
//! can change without notice. Tokens are stored in dbcrust's own secret
//! storage (OS keyring → encrypted file); the Codex CLI auth file is only
//! ever read, never written.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ai::{AiError, key_storage};

pub const OPENAI_AUTH_BASE: &str = "https://auth.openai.com";
/// Public client id of the official Codex CLI (the flow only works with it).
pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Fixed by the client registration — no fallback port is possible.
pub const REDIRECT_PORT: u16 = 1455;
pub const REDIRECT_PATH: &str = "/auth/callback";
pub const OAUTH_SCOPES: &str = "openid profile email offline_access";
/// Chat endpoint base in subscription mode. The trailing slash matters:
/// genai's Responses adapter joins `responses` onto it.
pub const CHATGPT_CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex/";

/// Every endpoint derives from `auth_base_url`, so tests can point the whole
/// flow at a local mock server instead of auth.openai.com.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub auth_base_url: String,
    pub client_id: String,
    pub redirect_port: u16,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        OAuthConfig {
            auth_base_url: OPENAI_AUTH_BASE.to_string(),
            client_id: CODEX_CLIENT_ID.to_string(),
            redirect_port: REDIRECT_PORT,
        }
    }
}

impl OAuthConfig {
    fn token_url(&self) -> String {
        format!("{}/oauth/token", self.auth_base_url)
    }

    fn redirect_uri(&self) -> String {
        format!("http://localhost:{}{}", self.redirect_port, REDIRECT_PATH)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGptTokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
    /// ChatGPT account id, sent as the `chatgpt-account-id` request header.
    pub account_id: String,
    /// Unix seconds. `None` when neither `expires_in` nor a JWT `exp` claim
    /// was available — treated as "always refresh".
    #[serde(default)]
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- PKCE / URL building ---

fn random_urlsafe(bytes: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// RFC 7636: verifier is random, challenge = base64url(SHA-256(verifier)).
fn generate_pkce() -> (String, String) {
    let verifier = random_urlsafe(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn build_authorize_url(
    config: &OAuthConfig,
    challenge: &str,
    state: &str,
) -> Result<String, AiError> {
    let mut url = url::Url::parse(&format!("{}/oauth/authorize", config.auth_base_url))
        .map_err(|e| AiError::OAuth(format!("invalid auth base URL: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &config.redirect_uri())
        .append_pair("scope", OAUTH_SCOPES)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        // Flags the auth server expects from the Codex client.
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true");
    Ok(url.to_string())
}

// --- JWT helpers (payload inspection only — no signature verification needed,
// the token comes straight from the token endpoint over TLS) ---

fn b64url_decode(input: &str) -> Result<Vec<u8>, AiError> {
    URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| URL_SAFE.decode(input))
        .map_err(|e| AiError::OAuth(format!("base64url decode failed: {e}")))
}

fn jwt_payload(token: &str) -> Result<serde_json::Value, AiError> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| AiError::OAuth("token is not a JWT".to_string()))?;
    serde_json::from_slice(&b64url_decode(payload)?)
        .map_err(|e| AiError::OAuth(format!("JWT payload is not JSON: {e}")))
}

fn extract_account_id(access_token: &str) -> Result<String, AiError> {
    let payload = jwt_payload(access_token)?;
    payload["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .or_else(|| payload["chatgpt_account_id"].as_str())
        .or_else(|| payload["auth"]["chatgpt_account_id"].as_str())
        .map(str::to_string)
        .ok_or_else(|| AiError::OAuth("chatgpt_account_id not found in access token".to_string()))
}

fn jwt_exp(token: &str) -> Option<i64> {
    jwt_payload(token).ok()?.get("exp")?.as_i64()
}

// --- Local callback server ---

/// Parse the first line of an HTTP request hitting the callback port.
/// `Ok(None)` = some other request (favicon, …) — keep waiting.
fn parse_callback_request(request_line: &str) -> Result<Option<(String, String)>, AiError> {
    let path = match request_line.split_whitespace().nth(1) {
        Some(p) => p,
        None => return Ok(None),
    };
    if !path.starts_with(REDIRECT_PATH) {
        return Ok(None);
    }
    let url = url::Url::parse(&format!("http://localhost{path}"))
        .map_err(|e| AiError::OAuth(format!("malformed callback URL: {e}")))?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.to_string()),
            "state" => state = Some(v.to_string()),
            "error" => error = Some(v.to_string()),
            _ => {}
        }
    }
    if let Some(e) = error {
        return Err(AiError::OAuth(format!("authorization was denied: {e}")));
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok(Some((code, state))),
        _ => Err(AiError::OAuth(
            "callback was missing code or state".to_string(),
        )),
    }
}

async fn bind_callback_listener(port: u16) -> Result<tokio::net::TcpListener, AiError> {
    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                AiError::OAuth(format!(
                    "port {port} is already in use — is another sign-in (dbcrust or Codex CLI) still running?"
                ))
            } else {
                AiError::OAuth(format!("could not start the local callback server: {e}"))
            }
        })
}

async fn wait_for_callback(listener: tokio::net::TcpListener) -> Result<(String, String), AiError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const SUCCESS_BODY: &str =
        "<html><body><h2>Signed in.</h2><p>You can return to dbcrust.</p></body></html>";
    const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";

    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let (mut stream, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| {
                AiError::OAuth("timed out waiting for the browser sign-in (5 minutes)".to_string())
            })?
            .map_err(|e| AiError::OAuth(format!("callback accept failed: {e}")))?;

        let mut request_line = String::new();
        {
            let mut reader = BufReader::new(&mut stream);
            let _ = reader.read_line(&mut request_line).await;
        }

        match parse_callback_request(&request_line) {
            Ok(Some((code, state))) => {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    SUCCESS_BODY.len(),
                    SUCCESS_BODY
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                return Ok((code, state));
            }
            Ok(None) => {
                let _ = stream.write_all(NOT_FOUND.as_bytes()).await;
            }
            Err(e) => {
                let body = format!("<html><body><h2>Sign-in failed.</h2><p>{e}</p></body></html>");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                return Err(e);
            }
        }
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    // Failure is fine — the URL is always printed for manual use.
    let _ = result;
}

// --- Token endpoint ---

fn http_client() -> Result<reqwest::Client, AiError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AiError::OAuth(format!("HTTP client error: {e}")))
}

fn tokens_from_response(
    response: TokenResponse,
    previous_refresh: Option<String>,
) -> Result<ChatGptTokens, AiError> {
    let account_id = extract_account_id(&response.access_token)?;
    let expires_at = response
        .expires_in
        .map(|secs| now_unix() + secs)
        .or_else(|| jwt_exp(&response.access_token));
    // Refresh-token rotation: the endpoint may omit the refresh token, in
    // which case the previous one stays valid.
    let refresh_token = response
        .refresh_token
        .or(previous_refresh)
        .ok_or_else(|| AiError::OAuth("token response had no refresh token".to_string()))?;
    Ok(ChatGptTokens {
        access_token: response.access_token,
        refresh_token,
        id_token: response.id_token,
        account_id,
        expires_at,
    })
}

async fn post_token_request(
    config: &OAuthConfig,
    http: &reqwest::Client,
    form: &[(&str, &str)],
) -> Result<TokenResponse, AiError> {
    let response = http
        .post(config.token_url())
        .form(form)
        .send()
        .await
        .map_err(|e| AiError::OAuth(format!("token request failed: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AiError::OAuth(format!(
            "token request failed ({status}): {body}"
        )));
    }
    response
        .json()
        .await
        .map_err(|e| AiError::OAuth(format!("invalid token response: {e}")))
}

async fn exchange_code(
    config: &OAuthConfig,
    http: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> Result<ChatGptTokens, AiError> {
    let redirect_uri = config.redirect_uri();
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", verifier),
    ];
    tokens_from_response(post_token_request(config, http, &form).await?, None)
}

async fn refresh(
    config: &OAuthConfig,
    http: &reqwest::Client,
    tokens: &ChatGptTokens,
) -> Result<ChatGptTokens, AiError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", tokens.refresh_token.as_str()),
        ("client_id", config.client_id.as_str()),
        ("scope", "openid profile email"),
    ];
    tokens_from_response(
        post_token_request(config, http, &form).await?,
        Some(tokens.refresh_token.clone()),
    )
}

// --- Token storage (dbcrust's own secret store) ---

pub fn store_tokens(tokens: &ChatGptTokens) -> Result<(), AiError> {
    let json = serde_json::to_string(tokens)
        .map_err(|e| AiError::KeyStorageError(format!("token serialization failed: {e}")))?;
    key_storage::store_named_secret(key_storage::CHATGPT_TOKENS_SECRET, &json)
}

pub fn load_tokens() -> Option<ChatGptTokens> {
    key_storage::load_named_secret(key_storage::CHATGPT_TOKENS_SECRET)
        .and_then(|json| serde_json::from_str(&json).ok())
}

pub fn delete_tokens() {
    key_storage::delete_named_secret(key_storage::CHATGPT_TOKENS_SECRET);
}

// --- Public flows ---

/// Full browser sign-in. Binds the local callback listener BEFORE opening the
/// browser, exchanges the code, and persists the tokens.
pub async fn login() -> Result<ChatGptTokens, AiError> {
    login_with(&OAuthConfig::default()).await
}

pub async fn login_with(config: &OAuthConfig) -> Result<ChatGptTokens, AiError> {
    let (verifier, challenge) = generate_pkce();
    let state = random_urlsafe(32);
    let authorize_url = build_authorize_url(config, &challenge, &state)?;

    let listener = bind_callback_listener(config.redirect_port).await?;
    println!("Opening your browser to sign in with ChatGPT…");
    println!("If it did not open, visit:\n  {authorize_url}\n");
    open_browser(&authorize_url);

    let (code, returned_state) = wait_for_callback(listener).await?;
    if returned_state != state {
        return Err(AiError::OAuth(
            "state mismatch in OAuth callback".to_string(),
        ));
    }

    let http = http_client()?;
    let tokens = exchange_code(config, &http, &code, &verifier).await?;
    store_tokens(&tokens)?;
    Ok(tokens)
}

/// Access token + account id for one request, refreshing (and persisting)
/// when the stored token has no known expiry or is within 60s of it.
pub async fn current_access() -> Result<(String, String), AiError> {
    let tokens = load_tokens().ok_or(AiError::NotLoggedIn)?;
    let fresh = tokens
        .expires_at
        .map(|exp| exp > now_unix() + 60)
        .unwrap_or(false);
    if fresh {
        return Ok((tokens.access_token, tokens.account_id));
    }

    let config = OAuthConfig::default();
    let http = http_client()?;
    let refreshed = refresh(&config, &http, &tokens)
        .await
        .map_err(|e| AiError::TokenRefreshFailed(e.to_string()))?;
    store_tokens(&refreshed)?;
    Ok((refreshed.access_token, refreshed.account_id))
}

// --- Codex CLI import ---

/// Codex CLI's auth store, when present on this machine.
pub fn codex_auth_path() -> Option<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".codex").join("auth.json"))
        .filter(|path| path.exists())
}

/// Import an existing `codex login` session into dbcrust's own token store.
/// The Codex file is only read — dbcrust never writes it back.
pub fn import_from_codex() -> Result<ChatGptTokens, AiError> {
    let path = codex_auth_path().ok_or_else(|| {
        AiError::OAuth("no Codex CLI login found (~/.codex/auth.json)".to_string())
    })?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| AiError::OAuth(format!("could not read {}: {e}", path.display())))?;
    let tokens = parse_codex_auth(&content)?;
    store_tokens(&tokens)?;
    Ok(tokens)
}

fn parse_codex_auth(content: &str) -> Result<ChatGptTokens, AiError> {
    let value: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| AiError::OAuth(format!("Codex auth file is not valid JSON: {e}")))?;
    let tokens = &value["tokens"];
    let access_token = tokens["access_token"]
        .as_str()
        .ok_or_else(|| AiError::OAuth("Codex auth file has no access_token".to_string()))?
        .to_string();
    let refresh_token = tokens["refresh_token"]
        .as_str()
        .ok_or_else(|| AiError::OAuth("Codex auth file has no refresh_token".to_string()))?
        .to_string();
    let id_token = tokens["id_token"].as_str().map(str::to_string);
    let account_id = match tokens["account_id"].as_str() {
        Some(id) => id.to_string(),
        None => extract_account_id(&access_token)?,
    };
    let expires_at = jwt_exp(&access_token);
    Ok(ChatGptTokens {
        access_token,
        refresh_token,
        id_token,
        account_id,
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JWT with the given JSON payload and a dummy header/signature.
    fn fake_jwt(payload: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let body = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("{header}.{body}.sig")
    }

    fn fake_access_token(account_id: &str, exp: Option<i64>) -> String {
        let mut payload = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
        });
        if let Some(exp) = exp {
            payload["exp"] = serde_json::json!(exp);
        }
        fake_jwt(&payload)
    }

    #[test]
    fn test_pkce_rfc7636_vector() {
        // RFC 7636 appendix B
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn test_generate_pkce_shape() {
        let (verifier, challenge) = generate_pkce();
        assert!(verifier.len() >= 43, "verifier too short for RFC 7636");
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(
            challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
    }

    #[test]
    fn test_build_authorize_url_params() {
        let config = OAuthConfig::default();
        let url_str = build_authorize_url(&config, "chal", "st4te").unwrap();
        let url = url::Url::parse(&url_str).unwrap();
        assert_eq!(url.host_str(), Some("auth.openai.com"));
        assert_eq!(url.path(), "/oauth/authorize");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["client_id"], CODEX_CLIENT_ID);
        assert_eq!(pairs["redirect_uri"], "http://localhost:1455/auth/callback");
        assert_eq!(pairs["code_challenge"], "chal");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["state"], "st4te");
        assert_eq!(pairs["scope"], OAUTH_SCOPES);
    }

    #[test]
    fn test_extract_account_id_variants() {
        let token = fake_access_token("acct-123", None);
        assert_eq!(extract_account_id(&token).unwrap(), "acct-123");

        // top-level fallback claim
        let token = fake_jwt(&serde_json::json!({ "chatgpt_account_id": "acct-9" }));
        assert_eq!(extract_account_id(&token).unwrap(), "acct-9");

        // not a JWT at all
        assert!(matches!(
            extract_account_id("not-a-jwt"),
            Err(AiError::OAuth(_))
        ));
        // JWT without the claim
        let token = fake_jwt(&serde_json::json!({ "sub": "user" }));
        assert!(extract_account_id(&token).is_err());
    }

    #[rstest::rstest]
    #[case("GET /auth/callback?code=abc&state=xyz HTTP/1.1", Some(("abc", "xyz")))]
    #[case("GET /favicon.ico HTTP/1.1", None)]
    #[case("", None)]
    fn test_parse_callback_request_ok(#[case] line: &str, #[case] expected: Option<(&str, &str)>) {
        let parsed = parse_callback_request(line).unwrap();
        assert_eq!(
            parsed,
            expected.map(|(c, s)| (c.to_string(), s.to_string()))
        );
    }

    #[test]
    fn test_parse_callback_request_errors() {
        assert!(matches!(
            parse_callback_request("GET /auth/callback?error=access_denied HTTP/1.1"),
            Err(AiError::OAuth(_))
        ));
        assert!(matches!(
            parse_callback_request("GET /auth/callback?code=only HTTP/1.1"),
            Err(AiError::OAuth(_))
        ));
    }

    #[test]
    fn test_tokens_from_response_refresh_retention() {
        let response = TokenResponse {
            access_token: fake_access_token("acct-1", Some(now_unix() + 3600)),
            refresh_token: None,
            id_token: None,
            expires_in: None,
        };
        // refresh token omitted by the endpoint → previous one is kept
        let tokens = tokens_from_response(response, Some("old-refresh".to_string())).unwrap();
        assert_eq!(tokens.refresh_token, "old-refresh");
        assert_eq!(tokens.account_id, "acct-1");
        // expires_at fell back to the JWT exp claim
        assert!(tokens.expires_at.unwrap() > now_unix());
    }

    #[test]
    fn test_parse_codex_auth_fixture() {
        let access = fake_access_token("acct-from-jwt", Some(1_900_000_000));
        let content = format!(
            r#"{{
                "last_refresh": "2026-06-01T00:00:00Z",
                "OPENAI_API_KEY": null,
                "tokens": {{
                    "access_token": "{access}",
                    "account_id": "acct-plain",
                    "id_token": "id.tok.en",
                    "refresh_token": "refresh-1"
                }}
            }}"#
        );
        let tokens = parse_codex_auth(&content).unwrap();
        // the file's plaintext account_id wins over the JWT claim
        assert_eq!(tokens.account_id, "acct-plain");
        assert_eq!(tokens.refresh_token, "refresh-1");
        assert_eq!(tokens.expires_at, Some(1_900_000_000));

        assert!(parse_codex_auth("{}").is_err());
        assert!(parse_codex_auth("not json").is_err());
    }

    /// One-shot mock token endpoint: accepts a single connection, ignores the
    /// request body, answers with `body` as JSON.
    async fn spawn_token_endpoint(body: String) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        port
    }

    fn test_oauth_config(port: u16) -> OAuthConfig {
        OAuthConfig {
            auth_base_url: format!("http://127.0.0.1:{port}"),
            client_id: "test-client".to_string(),
            redirect_port: 0,
        }
    }

    #[tokio::test]
    async fn test_exchange_code_against_mock() {
        let access = fake_access_token("acct-77", None);
        let body = format!(
            r#"{{"access_token":"{access}","refresh_token":"r-1","id_token":"i.d.t","expires_in":3600}}"#
        );
        let port = spawn_token_endpoint(body).await;
        let http = http_client().unwrap();

        let tokens = exchange_code(&test_oauth_config(port), &http, "code-1", "verifier-1")
            .await
            .unwrap();
        assert_eq!(tokens.account_id, "acct-77");
        assert_eq!(tokens.refresh_token, "r-1");
        let expires_at = tokens.expires_at.unwrap();
        assert!((expires_at - now_unix() - 3600).abs() < 30);
    }

    #[tokio::test]
    async fn test_refresh_against_mock_keeps_old_refresh_token() {
        let access = fake_access_token("acct-77", None);
        // endpoint omits refresh_token → rotation keeps the previous one
        let body = format!(r#"{{"access_token":"{access}","expires_in":600}}"#);
        let port = spawn_token_endpoint(body).await;
        let http = http_client().unwrap();

        let previous = ChatGptTokens {
            access_token: "old".to_string(),
            refresh_token: "keep-me".to_string(),
            id_token: None,
            account_id: "acct-77".to_string(),
            expires_at: Some(0),
        };
        let refreshed = refresh(&test_oauth_config(port), &http, &previous)
            .await
            .unwrap();
        assert_eq!(refreshed.refresh_token, "keep-me");
        assert_ne!(refreshed.access_token, "old");
    }

    #[tokio::test]
    async fn test_exchange_code_error_status() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 9\r\n\r\nbad_grant")
                    .await;
            }
        });
        let http = http_client().unwrap();
        let result = exchange_code(&test_oauth_config(port), &http, "c", "v").await;
        assert!(matches!(result, Err(AiError::OAuth(msg)) if msg.contains("400")));
    }
}
