use async_trait::async_trait;
use strum::{Display, EnumIter, IntoStaticStr};
use std::collections::HashMap;
use std::error::Error;
use std::str::FromStr;
use url::Url;

use crate::config::Config;
use crate::docker::DockerClient;

/// Error types for URL scheme operations
#[derive(Debug, thiserror::Error)]
pub enum UrlSchemeError {
    #[error("Unsupported URL scheme: {0}")]
    UnsupportedScheme(String),
    #[error("Invalid URL format: {0}")]
    InvalidUrl(String),
    #[error("URL parsing error: {0}")]
    ParseError(#[from] url::ParseError),
}

/// Result of URL scheme parsing
#[derive(Debug, Clone)]
pub struct ParsedUrl {
    pub scheme: UrlScheme,
    pub original_url: String,
    pub parsed_url: Option<Url>, // Some for standard schemes, None for special schemes
}

impl ParsedUrl {
    /// Create a new ParsedUrl
    pub fn new(scheme: UrlScheme, original_url: String, parsed_url: Option<Url>) -> Self {
        Self {
            scheme,
            original_url,
            parsed_url,
        }
    }

    /// Returns whether this is a special scheme (session://, recent://, etc.)
    pub fn is_special_scheme(&self) -> bool {
        matches!(
            self.scheme,
            UrlScheme::Session | UrlScheme::Recent | UrlScheme::Vault | UrlScheme::VaultDB
        )
    }

    /// Returns whether this is a standard database scheme
    pub fn is_database_scheme(&self) -> bool {
        matches!(
            self.scheme,
            UrlScheme::Postgres | UrlScheme::MySQL | UrlScheme::SQLite
        )
    }

    /// Returns whether this is a container scheme
    pub fn is_container_scheme(&self) -> bool {
        matches!(self.scheme, UrlScheme::Docker)
    }
}

/// Enum representing all supported URL schemes in DBCrust.
/// Using strum for automatic iteration and string conversion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, EnumIter, Display, IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum UrlScheme {
    #[strum(serialize = "postgres")]
    Postgres,
    #[strum(serialize = "mysql")]
    MySQL,
    #[strum(serialize = "sqlite")]
    SQLite,
    #[strum(serialize = "docker")]
    Docker,
    #[strum(serialize = "session")]
    Session,
    #[strum(serialize = "recent")]
    Recent,
    #[strum(serialize = "vault")]
    Vault,
    #[strum(serialize = "vaultdb")]
    VaultDB,
}

impl UrlScheme {
    /// Returns the URL prefix for this scheme (including ://)
    pub fn url_prefix(&self) -> String {
        format!("{}://", self)
    }

    /// Returns a description of what this scheme represents
    pub fn description(&self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL database connection",
            Self::MySQL => "MySQL database connection",
            Self::SQLite => "SQLite database file",
            Self::Docker => "Docker container database",
            Self::Session => "Saved session connection",
            Self::Recent => "Recent connection from history",
            Self::Vault | Self::VaultDB => "HashiCorp Vault dynamic credentials",
        }
    }

    /// Returns whether this scheme supports contextual completion
    pub fn supports_contextual_completion(&self) -> bool {
        matches!(self, Self::Docker | Self::Session | Self::SQLite)
    }

    /// Parse a URL string and return the scheme and parsed URL
    pub fn parse_url(url_string: &str) -> Result<ParsedUrl, UrlSchemeError> {
        // Handle URLs without scheme - default to postgres
        let full_url = if !url_string.contains("://") {
            format!("postgres://{}", url_string)
        } else {
            url_string.to_string()
        };

        // Extract scheme from URL
        let scheme_end = full_url.find("://").ok_or_else(|| {
            UrlSchemeError::InvalidUrl(format!("No scheme found in URL: {}", full_url))
        })?;
        let scheme_str = &full_url[..scheme_end];

        // Parse scheme using FromStr (handles both "postgresql" and "postgres" -> Postgres)
        let scheme = UrlScheme::from_str(scheme_str)
            .map_err(|_| UrlSchemeError::UnsupportedScheme(scheme_str.to_string()))?;

        // For special schemes, don't parse with url crate
        if matches!(
            scheme,
            Self::Session | Self::Recent | Self::Vault | Self::VaultDB | Self::Docker
        ) {
            return Ok(ParsedUrl::new(scheme, full_url, None));
        }

        // For standard database schemes, parse with url crate
        // Convert postgresql:// URLs to postgres:// for consistency
        let normalized_url = if scheme_str == "postgresql" {
            full_url.replace("postgresql://", "postgres://")
        } else {
            full_url.clone()
        };
        
        let parsed_url = Url::parse(&normalized_url)?;
        Ok(ParsedUrl::new(scheme, normalized_url, Some(parsed_url)))
    }

    /// Returns all schemes that match a given scheme string
    pub fn from_scheme_string(scheme: &str) -> Result<Self, UrlSchemeError> {
        match scheme.to_lowercase().as_str() {
            "postgresql" | "postgres" => Ok(Self::Postgres),
            "mysql" => Ok(Self::MySQL),
            "sqlite" => Ok(Self::SQLite),
            "docker" => Ok(Self::Docker),
            "session" => Ok(Self::Session),
            "recent" => Ok(Self::Recent),
            "vault" => Ok(Self::Vault),
            "vaultdb" => Ok(Self::VaultDB),
            _ => Err(UrlSchemeError::UnsupportedScheme(scheme.to_string())),
        }
    }

    /// Returns whether this scheme represents a PostgreSQL variant
    pub fn is_postgresql(&self) -> bool {
        matches!(self, Self::Postgres)
    }

    /// Returns the canonical database type for this scheme
    pub fn to_database_type(&self) -> Option<&'static str> {
        match self {
            Self::Postgres | Self::Docker => Some("PostgreSQL"),
            Self::MySQL => Some("MySQL"),
            Self::SQLite => Some("SQLite"),
            Self::Session | Self::Recent | Self::Vault | Self::VaultDB => None, // Resolved later
        }
    }

    /// Returns all scheme variants for a given database type
    pub fn for_database_type(db_type: &str) -> Vec<Self> {
        use strum::IntoEnumIterator;

        match db_type.to_lowercase().as_str() {
            "postgresql" | "postgres" => vec![Self::Postgres],
            "mysql" => vec![Self::MySQL],
            "sqlite" => vec![Self::SQLite],
            _ => UrlScheme::iter().collect(),
        }
    }
}

/// Implement FromStr for UrlScheme to enable parsing from strings
impl FromStr for UrlScheme {
    type Err = UrlSchemeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_scheme_string(s)
    }
}

/// Trait for URL scheme completion providers
#[async_trait]
pub trait UrlSchemeCompleter: Send + Sync {
    /// Returns the URL scheme this completer handles
    fn scheme(&self) -> UrlScheme;

    /// Returns contextual completions for this scheme
    /// For example: docker containers for docker://, saved sessions for session://
    async fn get_completions(&self, partial: &str) -> Result<Vec<String>, Box<dyn Error>>;

    /// Returns whether this completer requires async operations
    fn is_async(&self) -> bool {
        true
    }
}

/// Docker scheme completer - provides docker container names
pub struct DockerSchemeCompleter;

#[async_trait]
impl UrlSchemeCompleter for DockerSchemeCompleter {
    fn scheme(&self) -> UrlScheme {
        UrlScheme::Docker
    }

    async fn get_completions(&self, partial: &str) -> Result<Vec<String>, Box<dyn Error>> {
        // Create docker client
        let docker_client = DockerClient::new().map_err(|e| {
            format!("Failed to connect to Docker: {}", e)
        })?;

        // Get database containers
        let containers = docker_client.list_database_containers().await?;

        // Filter and format completions
        let completions: Vec<String> = containers
            .into_iter()
            .filter(|c| c.status.contains("running") || c.status.contains("Up")) // Only show running containers
            .filter(|c| c.name.starts_with(partial))
            .map(|c| {
                // Return just the container name part after docker://
                c.name
            })
            .collect();

        Ok(completions)
    }
}

/// Session scheme completer - provides saved session names
pub struct SessionSchemeCompleter {
    config: Config,
}

impl SessionSchemeCompleter {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

#[async_trait]
impl UrlSchemeCompleter for SessionSchemeCompleter {
    fn scheme(&self) -> UrlScheme {
        UrlScheme::Session
    }

    async fn get_completions(&self, partial: &str) -> Result<Vec<String>, Box<dyn Error>> {
        // Get saved sessions from config
        let sessions = self.config.list_sessions();

        // Filter and format completions
        let completions: Vec<String> = sessions
            .into_iter()
            .filter(|(name, _)| name.starts_with(partial))
            .map(|(name, _)| name)
            .collect();

        Ok(completions)
    }

    fn is_async(&self) -> bool {
        false // Session listing doesn't require async
    }
}

/// SQLite scheme completer - provides file path completion
pub struct SQLiteSchemeCompleter;

#[async_trait]
impl UrlSchemeCompleter for SQLiteSchemeCompleter {
    fn scheme(&self) -> UrlScheme {
        UrlScheme::SQLite
    }

    async fn get_completions(&self, _partial: &str) -> Result<Vec<String>, Box<dyn Error>> {
        // For SQLite, we'll return an empty vec and let the shell handle file completion
        // This is because file completion is better handled by the shell itself
        Ok(vec![])
    }

    fn is_async(&self) -> bool {
        false
    }
}

/// Manager for URL scheme completion
pub struct UrlSchemeManager {
    completers: HashMap<UrlScheme, Box<dyn UrlSchemeCompleter>>,
}

impl UrlSchemeManager {
    /// Creates a new URL scheme manager with default completers
    pub fn new(config: Config) -> Self {
        let mut completers: HashMap<UrlScheme, Box<dyn UrlSchemeCompleter>> = HashMap::new();

        // Register completers for schemes that support contextual completion
        completers.insert(UrlScheme::Docker, Box::new(DockerSchemeCompleter));
        completers.insert(
            UrlScheme::Session,
            Box::new(SessionSchemeCompleter::new(config)),
        );
        completers.insert(UrlScheme::SQLite, Box::new(SQLiteSchemeCompleter));

        Self { completers }
    }

    /// Returns all available URL schemes with their prefixes
    pub fn get_all_schemes() -> Vec<(String, &'static str)> {
        use strum::IntoEnumIterator;

        UrlScheme::iter()
            .map(|scheme| (scheme.url_prefix(), scheme.description()))
            .collect()
    }

    /// Returns URL scheme suggestions based on partial input
    pub fn get_scheme_suggestions(partial: &str) -> Vec<String> {
        use strum::IntoEnumIterator;

        UrlScheme::iter()
            .map(|scheme| scheme.url_prefix())
            .filter(|prefix| prefix.starts_with(partial))
            .collect()
    }

    /// Returns contextual completions for a specific scheme
    pub async fn get_contextual_completions(
        &self,
        scheme: &UrlScheme,
        partial: &str,
    ) -> Result<Vec<String>, Box<dyn Error>> {
        if let Some(completer) = self.completers.get(scheme) {
            completer.get_completions(partial).await
        } else {
            // No contextual completion for this scheme
            Ok(vec![])
        }
    }

    /// Parses a partial URL and returns appropriate completions
    pub async fn get_url_completions(&self, partial: &str) -> Result<Vec<String>, Box<dyn Error>> {
        // If partial doesn't contain "://", suggest schemes
        if !partial.contains("://") {
            return Ok(Self::get_scheme_suggestions(partial));
        }

        // Parse the scheme from the partial URL
        if let Some(separator_pos) = partial.find("://") {
            let scheme_str = &partial[..separator_pos];
            let after_scheme = &partial[separator_pos + 3..];

            // Try to parse the scheme
            use std::str::FromStr;
            if let Ok(scheme) = UrlScheme::from_str(scheme_str) {
                // Get contextual completions for this scheme
                let completions = self
                    .get_contextual_completions(&scheme, after_scheme)
                    .await?;

                // Format completions with the scheme prefix
                Ok(completions
                    .into_iter()
                    .map(|completion| format!("{}://{}", scheme_str, completion))
                    .collect())
            } else {
                // Unknown scheme, no completions
                Ok(vec![])
            }
        } else {
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn test_url_scheme_iteration() {
        let schemes: Vec<_> = UrlScheme::iter().collect();
        assert_eq!(schemes.len(), 8); // All 8 schemes (removed PostgreSQL duplicate)

        // Verify all schemes have proper string representation
        for scheme in schemes {
            assert!(!scheme.to_string().is_empty());
            assert!(!scheme.url_prefix().is_empty());
            assert!(scheme.url_prefix().ends_with("://"));
        }
    }

    #[test]
    fn test_scheme_suggestions() {
        // Test partial matching
        let suggestions = UrlSchemeManager::get_scheme_suggestions("post");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0], "postgres://");

        let suggestions = UrlSchemeManager::get_scheme_suggestions("doc");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0], "docker://");

        let suggestions = UrlSchemeManager::get_scheme_suggestions("va");
        assert_eq!(suggestions.len(), 2);
        assert!(suggestions.contains(&"vault://".to_string()));
        assert!(suggestions.contains(&"vaultdb://".to_string()));
    }

    #[test]
    fn test_contextual_completion_support() {
        assert!(UrlScheme::Docker.supports_contextual_completion());
        assert!(UrlScheme::Session.supports_contextual_completion());
        assert!(UrlScheme::SQLite.supports_contextual_completion());

        assert!(!UrlScheme::Postgres.supports_contextual_completion());
        assert!(!UrlScheme::MySQL.supports_contextual_completion());
        assert!(!UrlScheme::Recent.supports_contextual_completion());
    }

    #[tokio::test]
    async fn test_url_completions_without_scheme() {
        let config = Config::default();
        let manager = UrlSchemeManager::new(config);

        let completions = manager.get_url_completions("rec").await.unwrap();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0], "recent://");
    }

    #[tokio::test]
    async fn test_url_completions_with_scheme() {
        let config = Config::default();
        let manager = UrlSchemeManager::new(config);

        // Test docker:// with partial container name
        // This will fail if Docker isn't running, so we just verify it doesn't panic
        let _ = manager.get_url_completions("docker://my").await;

        // Test session:// with no sessions (default config)
        let completions = manager.get_url_completions("session://").await.unwrap();
        assert_eq!(completions.len(), 0); // No sessions in default config
    }

    #[test]
    fn test_url_parsing() {
        // Test URL without scheme (defaults to postgres)
        let parsed = UrlScheme::parse_url("localhost:5432/mydb").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Postgres);
        assert_eq!(parsed.original_url, "postgres://localhost:5432/mydb");
        assert!(parsed.parsed_url.is_some());

        // Test standard postgres URL
        let parsed = UrlScheme::parse_url("postgres://user@localhost:5432/db").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Postgres);
        assert!(parsed.is_database_scheme());
        assert!(!parsed.is_special_scheme());

        // Test postgresql URL (should be mapped to Postgres scheme)
        let parsed = UrlScheme::parse_url("postgresql://user@localhost:5432/db").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Postgres);
        assert!(parsed.is_database_scheme());
        assert_eq!(parsed.original_url, "postgres://user@localhost:5432/db"); // Normalized

        // Test MySQL URL
        let parsed = UrlScheme::parse_url("mysql://user:pass@host:3306/db").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::MySQL);
        assert!(parsed.is_database_scheme());

        // Test special scheme (session)
        let parsed = UrlScheme::parse_url("session://my_session").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Session);
        assert!(parsed.is_special_scheme());
        assert!(parsed.parsed_url.is_none()); // Special schemes don't use url crate parsing

        // Test docker scheme
        let parsed = UrlScheme::parse_url("docker://my-container/db").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Docker);
        assert!(parsed.is_container_scheme());
        assert!(parsed.parsed_url.is_none());

        // Test vault scheme
        let parsed = UrlScheme::parse_url("vault://role@mount/database").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Vault);
        assert!(parsed.is_special_scheme());

        // Test unsupported scheme
        let result = UrlScheme::parse_url("unsupported://test");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), UrlSchemeError::UnsupportedScheme(_)));
    }

    #[test]
    fn test_scheme_from_string() {
        assert_eq!(UrlScheme::from_str("postgresql").unwrap(), UrlScheme::Postgres);
        assert_eq!(UrlScheme::from_str("postgres").unwrap(), UrlScheme::Postgres);
        assert_eq!(UrlScheme::from_str("mysql").unwrap(), UrlScheme::MySQL);
        assert_eq!(UrlScheme::from_str("docker").unwrap(), UrlScheme::Docker);
        assert_eq!(UrlScheme::from_str("session").unwrap(), UrlScheme::Session);
        
        // Case insensitive
        assert_eq!(UrlScheme::from_str("POSTGRESQL").unwrap(), UrlScheme::Postgres);
        assert_eq!(UrlScheme::from_str("MySQL").unwrap(), UrlScheme::MySQL);

        // Invalid scheme
        assert!(UrlScheme::from_str("invalid").is_err());
    }

    #[test]
    fn test_database_type_mapping() {
        assert_eq!(UrlScheme::Postgres.to_database_type(), Some("PostgreSQL"));
        assert_eq!(UrlScheme::MySQL.to_database_type(), Some("MySQL"));
        assert_eq!(UrlScheme::SQLite.to_database_type(), Some("SQLite"));
        assert_eq!(UrlScheme::Docker.to_database_type(), Some("PostgreSQL")); // Default for docker
        
        // Special schemes return None (resolved later)
        assert_eq!(UrlScheme::Session.to_database_type(), None);
        assert_eq!(UrlScheme::Recent.to_database_type(), None);
        assert_eq!(UrlScheme::Vault.to_database_type(), None);
    }

    #[test]
    fn test_postgresql_variants() {
        assert!(UrlScheme::Postgres.is_postgresql());
        assert!(!UrlScheme::MySQL.is_postgresql());
        assert!(!UrlScheme::SQLite.is_postgresql());
        assert!(!UrlScheme::Docker.is_postgresql());
    }

    #[test]
    fn test_schemes_for_database_type() {
        let pg_schemes = UrlScheme::for_database_type("postgresql");
        assert_eq!(pg_schemes.len(), 1);
        assert!(pg_schemes.contains(&UrlScheme::Postgres));

        let mysql_schemes = UrlScheme::for_database_type("mysql");
        assert_eq!(mysql_schemes.len(), 1);
        assert!(mysql_schemes.contains(&UrlScheme::MySQL));

        let sqlite_schemes = UrlScheme::for_database_type("sqlite");
        assert_eq!(sqlite_schemes.len(), 1);
        assert!(sqlite_schemes.contains(&UrlScheme::SQLite));
    }
}