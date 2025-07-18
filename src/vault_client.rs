use std::env;
use std::fs;

use reqwest::header::HeaderMap;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

use crate::debug_log;

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
struct VaultResultantAclData {
    pub exact_paths: std::collections::HashMap<String, VaultPathCapabilities>,
    pub glob_paths: std::collections::HashMap<String, VaultPathCapabilities>,
}

#[derive(Deserialize, Debug)]
struct VaultPathCapabilities {
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
        .map_err(|e| VaultError::TokenFileError(format!("{}", e)))
}

// Create HTTP client with Vault headers
async fn create_vault_client() -> Result<(reqwest::Client, String), VaultError> {
    let vault_addr = get_vault_addr()?;
    let vault_token = get_vault_token()?;

    let mut headers = HeaderMap::new();
    headers.insert("X-Vault-Token", vault_token.parse().unwrap());

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()?;

    Ok((client, vault_addr))
}

pub async fn list_vault_databases(mount_path: &str) -> Result<Vec<String>, VaultError> {
    let (client, vault_addr) = create_vault_client().await?;
    let list_path = format!("{}/v1/{}/config?list=true", vault_addr, mount_path);

    let response = client.get(&list_path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        if status.as_u16() == 404 {
            return Err(VaultError::ApiError(format!(
                "Path {}/config not found (404). Ensure DB secrets engine is at '{}' and you have permissions.",
                mount_path, mount_path
            )));
        }
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Vault API error ({}): {}",
            status, error_text
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
    let path = format!("{}/v1/{}/config/{}", vault_addr, mount_path, db_config_name);

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
            "Vault API error ({}): {}",
            status, error_text
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
    let path = format!("{}/v1/{}/creds/{}", vault_addr, mount_path, role_name);

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
            "Vault API error ({}): {}",
            status, error_text
        )));
    }

    let creds_response: VaultReadResponse<VaultDynamicCredentialsData> = response.json().await?;
    Ok(creds_response.data)
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
        
        let creds_path = format!("{}/creds/{}", mount_path, db_name);
        let direct_path = format!("{}/{}", mount_path, db_name);
        
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
    let acl_path = format!("{}/v1/sys/internal/ui/resultant-acl", vault_addr);

    let response = client.get(&acl_path).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await?;
        return Err(VaultError::ApiError(format!(
            "Failed to retrieve ACL permissions ({}): {}",
            status, error_text
        )));
    }

    let response_text = response.text().await?;
    debug_log!("ACL Response: {}", response_text);
    
    let acl_response: VaultResultantAclResponse = serde_json::from_str(&response_text)
        .map_err(|e| VaultError::JsonError(e))?;
        
    Ok(acl_response.data)
}

/// Checks if the user has the required capabilities for a specific path
fn has_path_permission(
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
fn glob_matches(pattern: &str, path: &str) -> bool {
    // Simple glob matching for various patterns
    
    // Case 1: Exact match
    if pattern == path {
        return true;
    }
    
    // Case 2: Trailing slash (directory match)
    // e.g., "path/" matches "path" and "path/subpath"
    if pattern.ends_with('/') {
        let prefix = &pattern[..pattern.len() - 1];
        if path == prefix || path.starts_with(&format!("{}/", prefix)) {
            return true;
        }
    }
    
    // Case 3: Trailing asterisk (wildcard match)
    // e.g., "path/*" or "path*" matches anything with that prefix
    if pattern.ends_with('*') {
        let prefix = if pattern.ends_with("/*") {
            &pattern[..pattern.len() - 2]
        } else {
            &pattern[..pattern.len() - 1]
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
fn has_capabilities(user_capabilities: &[String], required_capabilities: &[&str]) -> bool {
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
}
