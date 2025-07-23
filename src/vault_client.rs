use std::env;
use std::fs;

use reqwest::header::HeaderMap;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::debug_log;

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
                if role.is_empty() { None } else { Some(role.to_string()) },
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
    #[error("Vault address not set (VAULT_ADDR environment variable)")]
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

#[derive(Deserialize, Debug)]
pub struct VaultDbConfigData {
    pub connection_details: VaultDbConfigConnectionDetails,
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
struct VaultResultantAclResponse {
    pub data: VaultResultantAclData,
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

pub fn get_vault_addr() -> Result<String, VaultError> {
    env::var("VAULT_ADDR").map_err(|_| VaultError::AddressError)
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
async fn create_vault_client() -> Result<(reqwest::Client, String), VaultError> {
    let vault_addr = get_vault_addr()?;
    let vault_token = get_vault_token()?;

    let mut headers = HeaderMap::new();
    let header_value = vault_token.parse()
        .map_err(|e| VaultError::ApiError(format!("Invalid token header value: {e}")))?;
    headers.insert("X-Vault-Token", header_value);

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    Ok((client, vault_addr))
}

pub async fn list_vault_databases(mount_path: &str) -> Result<Vec<String>, VaultError> {
    let (client, vault_addr) = create_vault_client().await?;
    let list_path = format!("{vault_addr}/v1/{mount_path}/config?list=true");

    let response = client.get(&list_path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(VaultError::ApiError(format!(
                "Path {mount_path}/config not found (404). Ensure DB secrets engine is at '{mount_path}' and you have permissions."
            )));
        }
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Vault API error ({status}): {error_text}"
        )));
    }

    let list_response: VaultListResponse = response.json().await?;
    Ok(list_response.data.keys)
}

pub async fn get_vault_database_config(
    mount_path: &str,
    db_config_name: &str,
) -> Result<VaultDbConfigData, VaultError> {
    let (client, vault_addr) = create_vault_client().await?;
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
    // Check cache first if caching is enabled
    if let Some(cached_creds) = config.get_cached_vault_credentials(mount_path, db_config_name, role_name) {
        crate::debug_log!("Using cached vault credentials for {}/{}/{}", mount_path, db_config_name, role_name);
        
        // Return cached credentials with lease info reconstructed from cache
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

    // Cache miss or caching disabled - fetch fresh credentials from Vault
    crate::debug_log!("Cache miss for vault credentials {}/{}/{}, fetching from Vault", mount_path, db_config_name, role_name);
    
    let (client, vault_addr) = create_vault_client().await?;
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

    // Parse full response to get lease information
    let full_response: VaultReadResponse<VaultDynamicCredentialsData> = response.json().await?;
    
    // Extract lease information from response headers or use defaults
    // Note: In a real implementation, you'd extract this from the response
    // For now, we'll use reasonable defaults and let the user configure TTL
    let lease_duration = 3600; // 1 hour default
    let lease_id = format!("{}/creds/{}/{}", mount_path, role_name, chrono::Utc::now().timestamp());
    
    let credentials = full_response.data;
    let lease_info = VaultLeaseInfo {
        lease_id: lease_id.clone(),
        lease_duration,
        renewable: true, // Most dynamic credentials are renewable
    };

    // Cache the credentials if caching is enabled
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
        
        if let Err(e) = config.cache_vault_credentials(mount_path, db_config_name, role_name, cached_creds) {
            crate::debug_log!("Failed to cache vault credentials: {}", e);
            // Don't fail the whole operation if caching fails
        } else {
            crate::debug_log!("Cached vault credentials for {}/{}/{}", mount_path, db_config_name, role_name);
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
/// This simply returns all roles allowed by the database config without further filtering.
pub async fn get_available_roles_for_user(
    mount_path: &str,
    db_config_name: &str,
) -> Result<Vec<String>, VaultError> {
    // Get all allowed roles from the database config
    let db_config = get_vault_database_config(mount_path, db_config_name).await?;
    
    // Return all roles defined in the database config
    // The filtering has already been done at the database selection level
    match db_config.allowed_roles {
        Some(roles) => {
            debug_log!("Roles available for database {}: {:?}", db_config_name, roles);
            Ok(roles)
        },
        None => {
            debug_log!("No roles defined for database {}", db_config_name);
            Ok(Vec::new())
        }
    }
}

/// Filter a list of database configurations to only include those that the current user has access to.
/// Returns a filtered list of database names that are accessible.
pub async fn filter_databases_with_available_roles(
    mount_path: &str,
    db_configs: Vec<String>,
) -> Result<Vec<String>, VaultError> {
    // Get user's ACL permissions once for all databases
    let user_acl = get_user_acl_permissions().await?;
    
    // Debug log the full ACL structure
    debug_log!("User ACL exact paths: {:?}", user_acl.exact_paths.keys().collect::<Vec<_>>());
    debug_log!("User ACL glob paths: {:?}", user_acl.glob_paths.keys().collect::<Vec<_>>());
    
    let mut accessible_dbs = Vec::new();
    
    for db_name in db_configs {
        // Check for database access based on the two critical paths:
        // 1. database/creds/<database> - needs read/create
        // 2. database/<database> - needs read/create
        
        let creds_path = format!("{mount_path}/creds/{db_name}");
        let direct_path = format!("{mount_path}/{db_name}");
        
        let creds_access = has_path_permission(&user_acl, &creds_path, &["read", "create"]);
        let direct_access = has_path_permission(&user_acl, &direct_path, &["read", "create"]);
        
        if creds_access || direct_access {
            debug_log!("User has access to database: {} (creds_path: {}, direct_access: {})", 
                      db_name, creds_access, direct_access);
            accessible_dbs.push(db_name);
        } else {
            debug_log!("User does NOT have access to database: {}", db_name);
        }
    }
    
    debug_log!("Accessible databases after filtering: {:?}", accessible_dbs);
    Ok(accessible_dbs)
}

/// Retrieves the user's ACL permissions from the resultant-acl endpoint
async fn get_user_acl_permissions() -> Result<VaultResultantAclData, VaultError> {
    let (client, vault_addr) = create_vault_client().await?;
    let acl_path = format!("{vault_addr}/v1/sys/internal/ui/resultant-acl");

    let response = client.get(&acl_path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Failed to retrieve ACL permissions ({status}): {error_text}"
        )));
    }

    let response_text = response.text().await?;
    debug_log!("ACL Response: {}", response_text);
    
    let acl_response: VaultResultantAclResponse = serde_json::from_str(&response_text)
        .map_err(VaultError::JsonError)?;
        
    Ok(acl_response.data)
}

/// Checks if the user has the required capabilities for a specific path
pub fn has_path_permission(
    acl_data: &VaultResultantAclData,
    path: &str,
    required_capabilities: &[&str],
) -> bool {
    // First check exact paths
    if let Some(path_capabilities) = acl_data.exact_paths.get(path) {
        if has_capabilities(&path_capabilities.capabilities, required_capabilities) {
            debug_log!("Found exact path match: {}", path);
            return true;
        }
    }

    // Then check glob paths
    for (glob_path, capabilities) in &acl_data.glob_paths {
        if glob_matches(glob_path, path) {
            debug_log!("Found glob path match: {} for path {}", glob_path, path);
            if has_capabilities(&capabilities.capabilities, required_capabilities) {
                return true;
            }
        }
    }

    debug_log!("No permission found for path: {}", path);
    false
}

// Helper function to check if a glob pattern matches a path
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    // Simple glob matching for various patterns
    
    // Case 1: Exact match
    if pattern == path {
        return true;
    }
    
    // Case 2: Trailing slash (directory match)
    // e.g., "path/" matches "path" and "path/subpath"
    if let Some(prefix) = pattern.strip_suffix('/') {
        if path == prefix || path.starts_with(&format!("{prefix}/")) {
            return true;
        }
    }
    
    // Case 3: Trailing asterisk (wildcard match)
    // e.g., "path/*" or "path*" matches anything with that prefix
    if pattern.ends_with('*') {
        let prefix = if let Some(p) = pattern.strip_suffix("/*") {
            p
        } else if let Some(p) = pattern.strip_suffix('*') {
            p
        } else {
            unreachable!()
        };
        
        if path.starts_with(prefix) {
            return true;
        }
    }
    
    // Case 4: Plus sign (+ wildcard)
    // e.g., "path/+" matches "path/anything"
    if pattern.contains('+') {
        let parts: Vec<&str> = pattern.split('+').collect();
        if parts.len() > 1 && path.starts_with(parts[0]) {
            return true;
        }
    }
    
    false
}

/// Helper function to check if the user has all required capabilities
pub fn has_capabilities(user_capabilities: &[String], required_capabilities: &[&str]) -> bool {
    // Must have ALL required capabilities
    required_capabilities.iter().all(|required| {
        user_capabilities.iter().any(|cap| cap == required)
    })
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
                map.insert("path/to/exact".to_string(), VaultPathCapabilities {
                    capabilities: vec!["read".to_string(), "list".to_string()]
                });
                map
            },
            glob_paths: {
                let mut map = std::collections::HashMap::new();
                map.insert("path/to/glob/*".to_string(), VaultPathCapabilities {
                    capabilities: vec!["read".to_string(), "list".to_string()]
                });
                map
            },
        };

        // Test exact path match with required capabilities
        assert!(has_path_permission(&acl_data, "path/to/exact", &["read"]));
        
        // Test exact path match without required capabilities
        assert!(!has_path_permission(&acl_data, "path/to/exact", &["delete"]));
        
        // Test glob path match with required capabilities
        assert!(has_path_permission(&acl_data, "path/to/glob/subpath", &["read"]));
        
        // Test glob path match without required capabilities
        assert!(!has_path_permission(&acl_data, "path/to/glob/subpath", &["delete"]));
        
        // Test path that doesn't match any permissions
        assert!(!has_path_permission(&acl_data, "non/existent/path", &["read"]));
        
        // Test that ALL required capabilities must be present (not just any)
        assert!(has_path_permission(&acl_data, "path/to/exact", &["read", "list"]));
        assert!(!has_path_permission(&acl_data, "path/to/exact", &["read", "delete"]));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("path", "path"));
        assert!(glob_matches("path/", "path"));
        assert!(glob_matches("path/", "path/subpath"));
        assert!(glob_matches("path/*", "path/subpath"));
        assert!(glob_matches("path/+", "path/anything"));
        assert!(!glob_matches("path", "non/existent/path"));
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
        assert_eq!(result, Some((Some("role".to_string()), "mount".to_string(), Some("database".to_string()))));

        // Test URL with default mount path
        let result = parse_vault_url("vault:///database");
        assert_eq!(result, Some((None, "database".to_string(), Some("database".to_string()))));

        // Test URL without role
        let result = parse_vault_url("vault://mount/database");
        assert_eq!(result, Some((None, "mount".to_string(), Some("database".to_string()))));

        // Test URL with role but no database
        let result = parse_vault_url("vault://role@mount");
        assert_eq!(result, Some((Some("role".to_string()), "mount".to_string(), None)));

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
