use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;

use futures_util::stream::{FuturesUnordered, StreamExt};
use regex::Regex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Deserializer};
use thiserror::Error;
use url::Url;

use tracing::debug;

// Parse a vault:// URL and extract vault parameters
// Format: vault://<role_name>@<mount_path:database>/<vault_db_name>
// All components are optional:
// - If role_name is not specified, user will be prompted to select one
// - If mount_path is not specified, defaults to "database"
// - If vault_db_name is not specified, user will be prompted to select one
pub fn parse_vault_url(url_str: &str) -> Option<(Option<String>, String, Option<String>)> {
    if !url_str.starts_with("vault://") {
        return None;
    }

    // Remove the protocol prefix
    let url_without_prefix = &url_str["vault://".len()..];

    // Extract role_name and mount_path from the user/host part
    let (user_host_part, db_part) = match url_without_prefix.find('/') {
        Some(idx) => (
            &url_without_prefix[..idx],
            Some(&url_without_prefix[idx + 1..]),
        ),
        None => (url_without_prefix, None),
    };

    // Parse the user/host part: <role_name>@<mount_path>
    let (role_name, mount_path) = {
        if let Some(at_pos) = user_host_part.find('@') {
            let role = &user_host_part[..at_pos];
            let mount = &user_host_part[at_pos + 1..];

            (
                if role.is_empty() {
                    None
                } else {
                    Some(role.to_string())
                },
                if mount.is_empty() {
                    "database".to_string()
                } else {
                    mount.to_string()
                },
            )
        } else {
            (
                None, // No role specified
                if user_host_part.is_empty() {
                    "database".to_string()
                } else {
                    user_host_part.to_string()
                },
            )
        }
    };

    // Extract vault_db_name from the path part
    let vault_db_name = db_part.map(|s| s.to_string()).filter(|s| !s.is_empty());

    Some((role_name, mount_path, vault_db_name))
}

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("Vault address not set (explicit override or VAULT_ADDR environment variable)")]
    AddressError,
    #[error(
        "Vault token not found. Set VAULT_TOKEN environment variable or place token in ~/.vault-token"
    )]
    TokenError,
    #[error("Failed to read token file ~/.vault-token: {0}")]
    TokenFileError(String),
    #[error("Vault API error: {0}")]
    ApiError(String),
    #[error("Connection URL parsing error: {0}")]
    UrlParseError(#[from] url::ParseError),
    #[error("Database configuration '{0}' not found or not accessible at {1}/config/{0}")]
    DbConfigNotFound(String, String),
    #[error(
        "Role '{0}' not found or not allowed for database configuration '{1}' (path: {2}/creds/{0})"
    )]
    RoleNotFound(String, String, String),
    #[error("No roles available for database configuration: {0}")]
    NoRolesAvailable(String),
    #[error("Missing 'connection_url' in Vault database configuration for '{0}'")]
    MissingConnectionUrl(String),
    #[error("HTTP request error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
}

#[derive(Deserialize, Debug)]
pub struct VaultDbConfigConnectionDetails {
    pub connection_url: Option<String>,
}

fn deserialize_allowed_roles<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum AllowedRolesField {
        Roles(Vec<String>),
        Csv(String),
    }

    let parsed = Option::<AllowedRolesField>::deserialize(deserializer)?;

    let roles = match parsed {
        Some(AllowedRolesField::Roles(roles)) => roles,
        Some(AllowedRolesField::Csv(csv)) => csv
            .split(',')
            .map(str::trim)
            .filter(|role| !role.is_empty())
            .map(|role| role.to_string())
            .collect(),
        None => Vec::new(),
    };

    if roles.is_empty() {
        Ok(None)
    } else {
        Ok(Some(roles))
    }
}

#[derive(Deserialize, Debug)]
pub struct VaultDbConfigData {
    pub connection_details: VaultDbConfigConnectionDetails,
    #[serde(default, deserialize_with = "deserialize_allowed_roles")]
    pub allowed_roles: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct VaultDynamicCredentialsData {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, Debug)]
struct VaultListResponse {
    pub data: VaultListData,
}

#[derive(Deserialize, Debug)]
struct VaultListData {
    pub keys: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct VaultReadResponse<T> {
    pub data: T,
}

#[derive(Deserialize, Debug)]
pub struct VaultResultantAclData {
    pub exact_paths: std::collections::HashMap<String, VaultPathCapabilities>,
    pub glob_paths: std::collections::HashMap<String, VaultPathCapabilities>,
}

#[derive(Deserialize, Debug)]
pub struct VaultPathCapabilities {
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DetectedVaultAddr {
    pub addr: String,
    pub source: String,
}

pub fn detect_vault_addr() -> Option<DetectedVaultAddr> {
    env::var("VAULT_ADDR")
        .ok()
        .map(|vault_addr| vault_addr.trim().to_string())
        .filter(|vault_addr| !vault_addr.is_empty())
        .map(|addr| DetectedVaultAddr {
            addr,
            source: "process environment (VAULT_ADDR)".to_string(),
        })
}

pub fn get_vault_addr_with_override(
    vault_addr_override: Option<&str>,
) -> Result<String, VaultError> {
    if let Some(override_addr) = vault_addr_override.map(str::trim)
        && !override_addr.is_empty()
    {
        return Ok(override_addr.to_string());
    }

    detect_vault_addr()
        .map(|detected| detected.addr)
        .ok_or(VaultError::AddressError)
}

pub fn get_vault_addr() -> Result<String, VaultError> {
    get_vault_addr_with_override(None)
}

pub fn get_vault_token() -> Result<String, VaultError> {
    if let Ok(token) = env::var("VAULT_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }

    let token_path = dirs::home_dir()
        .ok_or_else(|| VaultError::TokenFileError("Home directory not found".to_string()))?
        .join(".vault-token");

    fs::read_to_string(token_path)
        .map(|s| s.trim().to_string())
        .map_err(|e| VaultError::TokenFileError(format!("{e}")))
}

// Create HTTP client with Vault headers
async fn create_vault_client_with_addr(
    vault_addr_override: Option<&str>,
) -> Result<(reqwest::Client, String), VaultError> {
    let vault_addr = get_vault_addr_with_override(vault_addr_override)?;
    let vault_token = get_vault_token()?;

    let mut headers = HeaderMap::new();
    let header_value = vault_token
        .parse()
        .map_err(|e| VaultError::ApiError(format!("Invalid token header value: {e}")))?;
    headers.insert("X-Vault-Token", header_value);

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    Ok((client, vault_addr))
}

async fn create_vault_client() -> Result<(reqwest::Client, String), VaultError> {
    create_vault_client_with_addr(None).await
}

pub async fn list_vault_databases(mount_path: &str) -> Result<Vec<String>, VaultError> {
    list_vault_databases_with_addr(mount_path, None).await
}

pub async fn list_vault_databases_with_addr(
    mount_path: &str,
    vault_addr_override: Option<&str>,
) -> Result<Vec<String>, VaultError> {
    let (client, vault_addr) = create_vault_client_with_addr(vault_addr_override).await?;
    let list_path = format!("{vault_addr}/v1/{mount_path}/config?list=true");

    let response = client.get(&list_path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;

        if status.as_u16() == 404 {
            return Err(VaultError::ApiError(format!(
                "Path {mount_path}/config not found (404). Ensure DB secrets engine is mounted at '{mount_path}' and that your token can list it."
            )));
        } else if status.as_u16() == 403 {
            return Err(VaultError::ApiError(format!(
                "Vault API error (403 Forbidden): {error_text}"
            )));
        }

        return Err(VaultError::ApiError(format!(
            "Vault API error ({status}): {error_text}"
        )));
    }

    let list_response: VaultListResponse = response.json().await?;
    Ok(list_response.data.keys)
}

async fn fetch_vault_database_config(
    client: &reqwest::Client,
    vault_addr: &str,
    mount_path: &str,
    db_config_name: &str,
) -> Result<VaultDbConfigData, VaultError> {
    let path = format!("{vault_addr}/v1/{mount_path}/config/{db_config_name}");

    let response = client.get(&path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(VaultError::DbConfigNotFound(
                db_config_name.to_string(),
                mount_path.to_string(),
            ));
        }
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Vault API error ({status}): {error_text}"
        )));
    }

    let config_response: VaultReadResponse<VaultDbConfigData> = response.json().await?;
    Ok(config_response.data)
}

fn role_creds_path(mount_path: &str, role_name: &str) -> String {
    format!("{mount_path}/creds/{role_name}")
}

fn parse_capabilities_value(value: &serde_json::Value) -> Vec<String> {
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .filter_map(|entry| entry.as_str().map(|entry| entry.to_string()))
            .collect();
    }

    if let Some(object) = value.as_object()
        && let Some(capabilities) = object.get("capabilities")
    {
        return parse_capabilities_value(capabilities);
    }

    Vec::new()
}

fn has_read_access(capabilities: &[String]) -> bool {
    capabilities
        .iter()
        .any(|capability| capability == "read" || capability == "root")
}

async fn get_capabilities_for_paths(
    client: &reqwest::Client,
    vault_addr: &str,
    paths: &[String],
) -> Result<HashMap<String, Vec<String>>, VaultError> {
    if paths.is_empty() {
        return Ok(HashMap::new());
    }

    let response = client
        .post(format!("{vault_addr}/v1/sys/capabilities-self"))
        .json(&serde_json::json!({ "paths": paths }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Failed to retrieve path capabilities ({status}): {error_text}"
        )));
    }

    let payload: serde_json::Value = response.json().await?;
    let data = payload
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut capabilities_by_path = HashMap::new();

    if paths.len() == 1 {
        if let Some(capabilities) = data.get("capabilities") {
            capabilities_by_path.insert(paths[0].clone(), parse_capabilities_value(capabilities));
            return Ok(capabilities_by_path);
        }
    }

    if let Some(object) = data.as_object() {
        for path in paths {
            let capabilities = object
                .get(path)
                .map(parse_capabilities_value)
                .unwrap_or_default();
            capabilities_by_path.insert(path.clone(), capabilities);
        }
    }

    Ok(capabilities_by_path)
}

pub async fn get_vault_database_config(
    mount_path: &str,
    db_config_name: &str,
) -> Result<VaultDbConfigData, VaultError> {
    get_vault_database_config_with_addr(mount_path, db_config_name, None).await
}

pub async fn get_vault_database_config_with_addr(
    mount_path: &str,
    db_config_name: &str,
    vault_addr_override: Option<&str>,
) -> Result<VaultDbConfigData, VaultError> {
    let (client, vault_addr) = create_vault_client_with_addr(vault_addr_override).await?;
    fetch_vault_database_config(&client, &vault_addr, mount_path, db_config_name).await
}

pub async fn get_dynamic_credentials(
    mount_path: &str,
    db_config_name: &str,
    role_name: &str,
) -> Result<VaultDynamicCredentialsData, VaultError> {
    let (client, vault_addr) = create_vault_client().await?;
    // The correct path format is just: {mount_path}/creds/{role_name}
    // The db_config_name is not part of the credentials path
    let path = format!("{vault_addr}/v1/{mount_path}/creds/{role_name}");

    let response = client.get(&path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(VaultError::RoleNotFound(
                role_name.to_string(),
                db_config_name.to_string(),
                mount_path.to_string(),
            ));
        }
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Vault API error ({status}): {error_text}"
        )));
    }

    let creds_response: VaultReadResponse<VaultDynamicCredentialsData> = response.json().await?;
    Ok(creds_response.data)
}

/// Get dynamic credentials with caching support
/// Returns (credentials, lease_info) where lease_info contains lease_id, duration, etc.
pub async fn get_dynamic_credentials_with_caching(
    mount_path: &str,
    db_config_name: &str,
    role_name: &str,
    config: &mut crate::config::Config,
) -> Result<(VaultDynamicCredentialsData, VaultLeaseInfo), VaultError> {
    get_dynamic_credentials_with_caching_with_addr(
        mount_path,
        db_config_name,
        role_name,
        config,
        None,
    )
    .await
}

pub async fn get_dynamic_credentials_with_caching_with_addr(
    mount_path: &str,
    db_config_name: &str,
    role_name: &str,
    config: &mut crate::config::Config,
    vault_addr_override: Option<&str>,
) -> Result<(VaultDynamicCredentialsData, VaultLeaseInfo), VaultError> {
    if let Some(cached_creds) =
        config.get_cached_vault_credentials(mount_path, db_config_name, role_name)
    {
        debug!(
            "Using cached vault credentials for {}/{}/{}",
            mount_path, db_config_name, role_name
        );

        let credentials = VaultDynamicCredentialsData {
            username: cached_creds.username.clone(),
            password: cached_creds.password.clone(),
        };

        let lease_info = VaultLeaseInfo {
            lease_id: cached_creds.lease_id.clone(),
            lease_duration: cached_creds.lease_duration,
            renewable: cached_creds.renewable,
        };

        return Ok((credentials, lease_info));
    }

    debug!(
        "Cache miss for vault credentials {}/{}/{}, fetching from Vault",
        mount_path, db_config_name, role_name
    );

    let (client, vault_addr) = create_vault_client_with_addr(vault_addr_override).await?;
    let path = format!("{vault_addr}/v1/{mount_path}/creds/{role_name}");

    let response = client.get(&path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(VaultError::RoleNotFound(
                role_name.to_string(),
                db_config_name.to_string(),
                mount_path.to_string(),
            ));
        }
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Vault API error ({status}): {error_text}"
        )));
    }

    let full_response: VaultReadResponse<VaultDynamicCredentialsData> = response.json().await?;

    let lease_duration = 3600;
    let lease_id = format!(
        "{}/creds/{}/{}",
        mount_path,
        role_name,
        chrono::Utc::now().timestamp()
    );

    let credentials = full_response.data;
    let lease_info = VaultLeaseInfo {
        lease_id: lease_id.clone(),
        lease_duration,
        renewable: true,
    };

    if config.vault_credential_cache_enabled {
        let now = chrono::Utc::now();
        let expire_time = now + chrono::Duration::seconds(lease_duration as i64);

        let cached_creds = crate::config::CachedVaultCredentials {
            username: credentials.username.clone(),
            password: credentials.password.clone(),
            lease_id: lease_id.clone(),
            lease_duration,
            issue_time: now,
            expire_time,
            renewable: lease_info.renewable,
            mount_path: mount_path.to_string(),
            database_name: db_config_name.to_string(),
            role_name: role_name.to_string(),
        };

        if let Err(e) =
            config.cache_vault_credentials(mount_path, db_config_name, role_name, cached_creds)
        {
            debug!("Failed to cache vault credentials: {e}");
        } else {
            debug!(
                "Cached vault credentials for {}/{}/{}",
                mount_path, db_config_name, role_name
            );
        }
    }

    Ok((credentials, lease_info))
}

/// Lease information returned with dynamic credentials
#[derive(Debug, Clone)]
pub struct VaultLeaseInfo {
    pub lease_id: String,
    pub lease_duration: u64,
    pub renewable: bool,
}

pub fn construct_postgres_url(
    template_url_str: &str,
    dynamic_user: &str,
    dynamic_pass: &str,
) -> Result<String, VaultError> {
    let mut parsed_url = Url::parse(template_url_str)?;
    parsed_url
        .set_username(dynamic_user)
        .map_err(|_| VaultError::UrlParseError(url::ParseError::InvalidDomainCharacter))?;
    parsed_url
        .set_password(Some(dynamic_pass))
        .map_err(|_| VaultError::UrlParseError(url::ParseError::InvalidDomainCharacter))?;
    Ok(parsed_url.to_string())
}

/// Get available roles for a specific database mount path and configuration name.
/// Only roles that the current token can read from <mount>/creds/<role> are returned.
pub async fn get_available_roles_for_user(
    mount_path: &str,
    db_config_name: &str,
) -> Result<Vec<String>, VaultError> {
    get_available_roles_for_user_with_addr(mount_path, db_config_name, None).await
}

pub async fn get_available_roles_for_user_with_addr(
    mount_path: &str,
    db_config_name: &str,
    vault_addr_override: Option<&str>,
) -> Result<Vec<String>, VaultError> {
    let (client, vault_addr) = create_vault_client_with_addr(vault_addr_override).await?;
    let db_config =
        fetch_vault_database_config(&client, &vault_addr, mount_path, db_config_name).await?;

    let Some(allowed_roles) = db_config.allowed_roles else {
        debug!("No roles defined for database {}", db_config_name);
        return Ok(Vec::new());
    };

    let role_paths: Vec<String> = allowed_roles
        .iter()
        .map(|role_name| role_creds_path(mount_path, role_name))
        .collect();
    let capabilities = get_capabilities_for_paths(&client, &vault_addr, &role_paths).await?;

    let available_roles: Vec<String> = allowed_roles
        .into_iter()
        .filter(|role_name| {
            let creds_path = role_creds_path(mount_path, role_name);
            capabilities
                .get(&creds_path)
                .map(|capabilities| has_read_access(capabilities))
                .unwrap_or(false)
        })
        .collect();

    debug!(
        "Roles available for database {} after capability checks: {:?}",
        db_config_name, available_roles
    );

    Ok(available_roles)
}

/// Filter a list of database configurations to only include those that expose
/// at least one readable role for the current token.
pub async fn filter_databases_with_available_roles(
    mount_path: &str,
    db_configs: Vec<String>,
) -> Result<Vec<String>, VaultError> {
    filter_databases_with_available_roles_with_addr(mount_path, db_configs, None).await
}

pub async fn filter_databases_with_available_roles_with_addr(
    mount_path: &str,
    db_configs: Vec<String>,
    vault_addr_override: Option<&str>,
) -> Result<Vec<String>, VaultError> {
    let (client, vault_addr) = create_vault_client_with_addr(vault_addr_override).await?;

    let mut tasks = FuturesUnordered::new();
    for db_name in db_configs {
        let client = client.clone();
        let vault_addr = vault_addr.clone();
        let mount_path = mount_path.to_string();
        tasks.push(async move {
            let result =
                fetch_vault_database_config(&client, &vault_addr, &mount_path, &db_name).await;
            (db_name, result)
        });
    }

    let mut roles_by_database: Vec<(String, Vec<String>)> = Vec::new();
    let mut role_paths = HashSet::new();

    while let Some((db_name, config_result)) = tasks.next().await {
        let db_config = match config_result {
            Ok(config) => config,
            Err(error) => {
                debug!(
                    "Skipping Vault database config {} because it could not be read: {}",
                    db_name, error
                );
                continue;
            }
        };

        let allowed_roles = db_config.allowed_roles.unwrap_or_default();
        if allowed_roles.is_empty() {
            debug!("Database {} has no allowed roles configured", db_name);
            continue;
        }

        for role_name in &allowed_roles {
            role_paths.insert(role_creds_path(mount_path, role_name));
        }
        roles_by_database.push((db_name, allowed_roles));
    }

    let role_paths: Vec<String> = role_paths.into_iter().collect();
    let capabilities = get_capabilities_for_paths(&client, &vault_addr, &role_paths).await?;

    let accessible_dbs: Vec<String> = roles_by_database
        .into_iter()
        .filter_map(|(db_name, allowed_roles)| {
            let readable_roles: Vec<String> = allowed_roles
                .into_iter()
                .filter(|role_name| {
                    let creds_path = role_creds_path(mount_path, role_name);
                    capabilities
                        .get(&creds_path)
                        .map(|capabilities| has_read_access(capabilities))
                        .unwrap_or(false)
                })
                .collect();

            if readable_roles.is_empty() {
                debug!(
                    "User does NOT have access to any allowed roles for database {}",
                    db_name
                );
                None
            } else {
                debug!(
                    "User has access to database {} via roles {:?}",
                    db_name, readable_roles
                );
                Some(db_name)
            }
        })
        .collect();

    debug!("Accessible databases after filtering: {:?}", accessible_dbs);
    Ok(accessible_dbs)
}

/// Checks if the user has the required capabilities for a specific path
pub fn has_path_permission(
    acl_data: &VaultResultantAclData,
    path: &str,
    required_capabilities: &[&str],
) -> bool {
    // First check exact paths
    if let Some(path_capabilities) = acl_data.exact_paths.get(path)
        && has_capabilities(&path_capabilities.capabilities, required_capabilities)
    {
        debug!("Found exact path match: {}", path);
        return true;
    }

    // Then check glob paths
    for (glob_path, capabilities) in &acl_data.glob_paths {
        if glob_matches(glob_path, path) {
            debug!("Found glob path match: {} for path {}", glob_path, path);
            if has_capabilities(&capabilities.capabilities, required_capabilities) {
                return true;
            }
        }
    }

    debug!("No permission found for path: {}", path);
    false
}

// Helper function to check if a Vault glob pattern matches a path.
// Vault uses:
// - * for a glob-style wildcard
// - + for a single path segment wildcard
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    if pattern == path {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix('/')
        && (path == prefix || path.starts_with(&format!("{prefix}/")))
    {
        return true;
    }

    let mut regex_pattern = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex_pattern.push_str(".*"),
            '+' => regex_pattern.push_str("[^/]+"),
            _ => regex_pattern.push_str(&regex::escape(&ch.to_string())),
        }
    }
    regex_pattern.push('$');

    Regex::new(&regex_pattern)
        .map(|regex| regex.is_match(path))
        .unwrap_or(false)
}

/// Helper function to check if the user has all required capabilities
pub fn has_capabilities(user_capabilities: &[String], required_capabilities: &[&str]) -> bool {
    // Must have ALL required capabilities
    required_capabilities
        .iter()
        .all(|required| user_capabilities.iter().any(|cap| cap == required))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_path_permission() {
        // Create a mock ACL data structure
        let acl_data = VaultResultantAclData {
            exact_paths: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "path/to/exact".to_string(),
                    VaultPathCapabilities {
                        capabilities: vec!["read".to_string(), "list".to_string()],
                    },
                );
                map
            },
            glob_paths: {
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "path/to/glob/*".to_string(),
                    VaultPathCapabilities {
                        capabilities: vec!["read".to_string(), "list".to_string()],
                    },
                );
                map
            },
        };

        // Test exact path match with required capabilities
        assert!(has_path_permission(&acl_data, "path/to/exact", &["read"]));

        // Test exact path match without required capabilities
        assert!(!has_path_permission(
            &acl_data,
            "path/to/exact",
            &["delete"]
        ));

        // Test glob path match with required capabilities
        assert!(has_path_permission(
            &acl_data,
            "path/to/glob/subpath",
            &["read"]
        ));

        // Test glob path match without required capabilities
        assert!(!has_path_permission(
            &acl_data,
            "path/to/glob/subpath",
            &["delete"]
        ));

        // Test path that doesn't match any permissions
        assert!(!has_path_permission(
            &acl_data,
            "non/existent/path",
            &["read"]
        ));

        // Test that ALL required capabilities must be present (not just any)
        assert!(has_path_permission(
            &acl_data,
            "path/to/exact",
            &["read", "list"]
        ));
        assert!(!has_path_permission(
            &acl_data,
            "path/to/exact",
            &["read", "delete"]
        ));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("path", "path"));
        assert!(glob_matches("path/", "path"));
        assert!(glob_matches("path/", "path/subpath"));
        assert!(glob_matches("path/*", "path/subpath"));
        assert!(glob_matches("path/+", "path/anything"));
        assert!(glob_matches(
            "database/creds/app-*",
            "database/creds/app-reader"
        ));
        assert!(!glob_matches("path", "non/existent/path"));
        assert!(!glob_matches("database/+", "database/creds/app-reader"));
        assert!(!glob_matches(
            "database/creds/+",
            "database/creds/app/reader"
        ));
    }

    #[test]
    fn test_has_capabilities() {
        let user_caps = vec!["read".to_string(), "list".to_string()];

        // User has all required capabilities
        assert!(has_capabilities(&user_caps, &["read"]));
        assert!(has_capabilities(&user_caps, &["list"]));
        assert!(has_capabilities(&user_caps, &["read", "list"]));

        // User lacks some required capabilities
        assert!(!has_capabilities(&user_caps, &["read", "create"]));
        assert!(!has_capabilities(&user_caps, &["write"]));
    }

    #[test]
    fn test_parse_vault_url() {
        // Test complete URL
        let result = parse_vault_url("vault://role@mount/database");
        assert_eq!(
            result,
            Some((
                Some("role".to_string()),
                "mount".to_string(),
                Some("database".to_string())
            ))
        );

        // Test URL with default mount path
        let result = parse_vault_url("vault:///database");
        assert_eq!(
            result,
            Some((None, "database".to_string(), Some("database".to_string())))
        );

        // Test URL without role
        let result = parse_vault_url("vault://mount/database");
        assert_eq!(
            result,
            Some((None, "mount".to_string(), Some("database".to_string())))
        );

        // Test URL with role but no database
        let result = parse_vault_url("vault://role@mount");
        assert_eq!(
            result,
            Some((Some("role".to_string()), "mount".to_string(), None))
        );

        // Test URL with empty components
        let result = parse_vault_url("vault://@/");
        assert_eq!(result, Some((None, "database".to_string(), None)));

        // Test invalid URL
        let result = parse_vault_url("postgres://user@host/db");
        assert_eq!(result, None);

        // Test minimal vault URL
        let result = parse_vault_url("vault://");
        assert_eq!(result, Some((None, "database".to_string(), None)));
    }
}
