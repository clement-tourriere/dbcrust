use async_trait::async_trait;
use std::collections::HashMap;
use std::error::Error;
use std::str::FromStr;
use strum::{Display, EnumIter, IntoStaticStr};
use url::Url;

use crate::config::Config;
#[cfg(feature = "docker")]
use crate::docker::DockerClient;

/// Error types for URL scheme operations
#[derive(Debug, thiserror::Error)]
pub enum UrlSchemeError {
    #[error("Unsupported URL scheme: {}", crate::password_sanitizer::redact_text(.0))]
    UnsupportedScheme(String),
    #[error("Invalid URL format: {}", crate::password_sanitizer::redact_text(.0))]
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
            UrlScheme::File | UrlScheme::Session | UrlScheme::Recent | UrlScheme::Vault
        )
    }

    /// Returns whether this is a standard database scheme
    pub fn is_database_scheme(&self) -> bool {
        matches!(
            self.scheme,
            UrlScheme::Postgres
                | UrlScheme::MySQL
                | UrlScheme::SQLite
                | UrlScheme::WhiteDragon
                | UrlScheme::Redis
                | UrlScheme::SqlServer
                | UrlScheme::Redshift
                | UrlScheme::LibSql
                | UrlScheme::InfluxDb
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
    #[strum(serialize = "clickhouse")]
    ClickHouse,
    #[strum(serialize = "mongodb")]
    MongoDB,
    #[strum(serialize = "elasticsearch")]
    Elasticsearch,
    #[strum(serialize = "white-dragon")]
    WhiteDragon,
    #[strum(serialize = "redis")]
    Redis,
    #[strum(serialize = "mssql")]
    SqlServer,
    #[strum(serialize = "redshift")]
    Redshift,
    #[strum(serialize = "libsql")]
    LibSql,
    #[strum(serialize = "influxdb")]
    InfluxDb,
    #[strum(serialize = "parquet")]
    Parquet,
    #[strum(serialize = "csv")]
    Csv,
    #[strum(serialize = "json")]
    Json,
    #[strum(serialize = "duckdb")]
    DuckDB,
    #[strum(serialize = "docker")]
    Docker,
    #[strum(serialize = "file")]
    File,
    #[strum(serialize = "session")]
    Session,
    #[strum(serialize = "recent")]
    Recent,
    #[strum(serialize = "vault")]
    Vault,
}

impl UrlScheme {
    /// The engine this scheme connects to, for the schemes that name one.
    fn engine(&self) -> Option<crate::database::DatabaseType> {
        use crate::database::DatabaseType;
        Some(match self {
            Self::Postgres => DatabaseType::PostgreSQL,
            Self::MySQL => DatabaseType::MySQL,
            Self::SQLite => DatabaseType::SQLite,
            Self::ClickHouse => DatabaseType::ClickHouse,
            Self::MongoDB => DatabaseType::MongoDB,
            Self::Elasticsearch => DatabaseType::Elasticsearch,
            Self::WhiteDragon => DatabaseType::WhiteDragon,
            Self::Redis => DatabaseType::Redis,
            Self::SqlServer => DatabaseType::SqlServer,
            Self::Redshift => DatabaseType::Redshift,
            Self::LibSql => DatabaseType::LibSql,
            Self::InfluxDb => DatabaseType::InfluxDB,
            Self::Parquet => DatabaseType::Parquet,
            Self::Csv => DatabaseType::CSV,
            Self::Json => DatabaseType::JSON,
            Self::DuckDB => DatabaseType::DuckDB,
            Self::Docker | Self::File | Self::Session | Self::Recent | Self::Vault => return None,
        })
    }

    /// Whether this build can serve the scheme: its connector (or, for
    /// `docker://`, the `docker` feature) is compiled in. Completion and help
    /// only offer available schemes; parsing still accepts every scheme so a
    /// disabled one fails with the "not compiled in" message and not with
    /// "unknown scheme".
    pub fn is_available(&self) -> bool {
        match self {
            Self::Docker => cfg!(feature = "docker"),
            // The picker lists SQLite databases and the DataFusion file formats
            Self::File => {
                crate::drivers::is_compiled(&crate::database::DatabaseType::SQLite)
                    || crate::drivers::is_compiled(&crate::database::DatabaseType::CSV)
            }
            scheme => scheme
                .engine()
                .is_none_or(|engine| crate::drivers::is_compiled(&engine)),
        }
    }

    /// Returns the URL prefix for this scheme (including ://)
    pub fn url_prefix(&self) -> String {
        format!("{self}://")
    }

    /// Returns a description of what this scheme represents
    pub fn description(&self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL database connection",
            Self::MySQL => "MySQL database connection",
            Self::SQLite => "SQLite database file",
            Self::ClickHouse => "ClickHouse database connection",
            Self::MongoDB => "MongoDB database connection",
            Self::Elasticsearch => "Elasticsearch cluster connection",
            Self::WhiteDragon => "White Dragon schema-first search endpoint",
            Self::Redis => "Redis (or Valkey) server; rediss:// for TLS",
            Self::SqlServer => {
                "SQL Server or Azure SQL (SQL authentication; encrypt=true by default)"
            }
            Self::Redshift => "Amazon Redshift (PostgreSQL protocol, TLS required by default)",
            Self::LibSql => {
                "libSQL / Turso over HTTP (sqld or Turso Cloud; libsql+http:// for a local sqld)"
            }
            Self::InfluxDb => {
                "InfluxDB v1 (InfluxQL); influxdb2:// is v2 (Flux), influxdb3:// v3 (SQL), a trailing s is TLS"
            }
            Self::Parquet => "Parquet file or glob (DataFusion)",
            Self::Csv => "CSV/TSV file or glob (DataFusion)",
            Self::Json => "JSON/NDJSON file or glob (DataFusion)",
            Self::DuckDB => "DuckDB database file",
            Self::Docker => "Docker container database",
            Self::File => "Interactive local file picker",
            Self::Session => "Saved session connection",
            Self::Recent => "Recent connection from history",
            Self::Vault => "HashiCorp Vault dynamic credentials",
        }
    }

    /// Returns whether this scheme supports contextual completion
    pub fn supports_contextual_completion(&self) -> bool {
        matches!(
            self,
            Self::Docker | Self::File | Self::Session | Self::SQLite
        )
    }

    /// Parse a URL string and return the scheme and parsed URL
    pub fn parse_url(url_string: &str) -> Result<ParsedUrl, UrlSchemeError> {
        // Handle URLs without scheme - default to postgres
        let full_url = if !url_string.contains("://") {
            format!("postgres://{url_string}")
        } else {
            url_string.to_string()
        };

        // Extract scheme from URL
        let scheme_end = full_url.find("://").ok_or_else(|| {
            UrlSchemeError::InvalidUrl(format!(
                "No scheme found in URL: {}",
                crate::password_sanitizer::redact_url(&full_url)
            ))
        })?;
        let scheme_str = &full_url[..scheme_end];

        // Parse scheme using FromStr (handles both "postgresql" and "postgres" -> Postgres)
        let scheme = UrlScheme::from_str(scheme_str)
            .map_err(|_| UrlSchemeError::UnsupportedScheme(scheme_str.to_string()))?;

        // For special schemes, don't parse with url crate
        if matches!(
            scheme,
            Self::File | Self::Session | Self::Recent | Self::Vault | Self::Docker
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

        // `host\instance` (SQL Server) is not a valid URL host as typed
        let normalized_url =
            crate::database::escape_sqlserver_instance_backslash(&normalized_url).into_owned();
        let parsed_url = Url::parse(&normalized_url)?;
        Ok(ParsedUrl::new(scheme, normalized_url, Some(parsed_url)))
    }

    /// Returns all schemes that match a given scheme string
    pub fn from_scheme_string(scheme: &str) -> Result<Self, UrlSchemeError> {
        match scheme.to_lowercase().as_str() {
            "postgresql" | "postgres" => Ok(Self::Postgres),
            "mysql" => Ok(Self::MySQL),
            "sqlite" => Ok(Self::SQLite),
            "clickhouse" => Ok(Self::ClickHouse),
            "mongodb" | "mongodb+srv" => Ok(Self::MongoDB),
            "elasticsearch" | "elastic" | "es" => Ok(Self::Elasticsearch),
            "white-dragon" | "whitedragon" | "wd" => Ok(Self::WhiteDragon),
            "redis" | "rediss" | "valkey" | "valkeys" | "redis+unix" | "unix" => Ok(Self::Redis),
            "mssql" | "sqlserver" | "mssql+tls" | "sqlserver+tls" => Ok(Self::SqlServer),
            "redshift" | "redshift+tls" => Ok(Self::Redshift),
            "libsql" | "libsql+http" | "turso" => Ok(Self::LibSql),
            scheme if crate::database::INFLUXDB_SCHEMES.contains(&scheme) => Ok(Self::InfluxDb),
            "parquet" => Ok(Self::Parquet),
            "csv" => Ok(Self::Csv),
            "json" | "ndjson" => Ok(Self::Json),
            "duckdb" => Ok(Self::DuckDB),
            "docker" => Ok(Self::Docker),
            "file" => Ok(Self::File),
            "session" => Ok(Self::Session),
            "recent" => Ok(Self::Recent),
            "vault" => Ok(Self::Vault),
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
            Self::ClickHouse => Some("ClickHouse"),
            Self::MongoDB => Some("MongoDB"),
            Self::Elasticsearch => Some("Elasticsearch"),
            Self::WhiteDragon => Some("White Dragon"),
            Self::Redis => Some("Redis"),
            Self::SqlServer => Some("SQL Server"),
            Self::Redshift => Some("Redshift"),
            Self::LibSql => Some("libSQL"),
            Self::InfluxDb => Some("InfluxDB"),
            Self::Parquet => Some("Parquet"),
            Self::Csv => Some("CSV"),
            Self::Json => Some("JSON"),
            Self::DuckDB => Some("DuckDB"),
            Self::File | Self::Session | Self::Recent | Self::Vault => None, // Resolved later
        }
    }

    /// Returns all scheme variants for a given database type
    pub fn for_database_type(db_type: &str) -> Vec<Self> {
        use strum::IntoEnumIterator;

        match db_type.to_lowercase().as_str() {
            "postgresql" | "postgres" => vec![Self::Postgres],
            "mysql" => vec![Self::MySQL],
            "sqlite" => vec![Self::SQLite],
            "white dragon" | "white-dragon" | "whitedragon" | "wd" => {
                vec![Self::WhiteDragon]
            }
            "redis" | "valkey" => vec![Self::Redis],
            "sql server" | "sqlserver" | "mssql" | "azure sql" => vec![Self::SqlServer],
            "redshift" | "aws redshift" => vec![Self::Redshift],
            "libsql" | "turso" | "sqld" => vec![Self::LibSql],
            "influxdb" | "influx" => vec![Self::InfluxDb],
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
#[cfg(feature = "docker")]
pub struct DockerSchemeCompleter;

#[cfg(feature = "docker")]
#[async_trait]
impl UrlSchemeCompleter for DockerSchemeCompleter {
    fn scheme(&self) -> UrlScheme {
        UrlScheme::Docker
    }

    async fn get_completions(&self, partial: &str) -> Result<Vec<String>, Box<dyn Error>> {
        // Create docker client
        let docker_client =
            DockerClient::new().map_err(|e| format!("Failed to connect to Docker: {e}"))?;

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

/// File scheme completer - shell file completion is the best UX here.
pub struct FileSchemeCompleter;

#[async_trait]
impl UrlSchemeCompleter for FileSchemeCompleter {
    fn scheme(&self) -> UrlScheme {
        UrlScheme::File
    }

    async fn get_completions(&self, _partial: &str) -> Result<Vec<String>, Box<dyn Error>> {
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
        #[cfg(feature = "docker")]
        completers.insert(UrlScheme::Docker, Box::new(DockerSchemeCompleter));
        completers.insert(
            UrlScheme::Session,
            Box::new(SessionSchemeCompleter::new(config)),
        );
        completers.insert(UrlScheme::SQLite, Box::new(SQLiteSchemeCompleter));
        completers.insert(UrlScheme::File, Box::new(FileSchemeCompleter));

        Self { completers }
    }

    /// Returns all available URL schemes with their prefixes
    pub fn get_all_schemes() -> Vec<(String, &'static str)> {
        use strum::IntoEnumIterator;

        UrlScheme::iter()
            .filter(UrlScheme::is_available)
            .map(|scheme| (scheme.url_prefix(), scheme.description()))
            .collect()
    }

    /// Returns URL scheme suggestions based on partial input
    pub fn get_scheme_suggestions(partial: &str) -> Vec<String> {
        use strum::IntoEnumIterator;

        UrlScheme::iter()
            .filter(UrlScheme::is_available)
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
                    .map(|completion| format!("{scheme_str}://{completion}"))
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
        assert_eq!(schemes.len(), 21); // All schemes

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

        // `docker://` is only offered by builds that can serve it
        let suggestions = UrlSchemeManager::get_scheme_suggestions("doc");
        if cfg!(feature = "docker") {
            assert_eq!(suggestions, ["docker://"]);
        } else {
            assert!(suggestions.is_empty(), "{suggestions:?}");
        }

        let suggestions = UrlSchemeManager::get_scheme_suggestions("va");
        assert_eq!(suggestions.len(), 1);
        assert!(suggestions.contains(&"vault://".to_string()));
    }

    #[test]
    fn test_contextual_completion_support() {
        assert!(UrlScheme::Docker.supports_contextual_completion());
        assert!(UrlScheme::File.supports_contextual_completion());
        assert!(UrlScheme::Session.supports_contextual_completion());
        assert!(UrlScheme::SQLite.supports_contextual_completion());

        assert!(!UrlScheme::Postgres.supports_contextual_completion());
        assert!(!UrlScheme::MySQL.supports_contextual_completion());
        assert!(!UrlScheme::Recent.supports_contextual_completion());
        assert!(!UrlScheme::WhiteDragon.supports_contextual_completion());
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

        let parsed = UrlScheme::parse_url("white-dragon://localhost:7700").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::WhiteDragon);
        assert!(parsed.is_database_scheme());

        // Test special scheme (session)
        let parsed = UrlScheme::parse_url("session://my_session").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Session);
        assert!(parsed.is_special_scheme());
        assert!(parsed.parsed_url.is_none()); // Special schemes don't use url crate parsing

        // Test file picker scheme
        let parsed = UrlScheme::parse_url("file://").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::File);
        assert!(parsed.is_special_scheme());
        assert!(parsed.parsed_url.is_none());

        // Test docker scheme
        let parsed = UrlScheme::parse_url("docker://my-container/db").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Docker);
        assert!(parsed.is_container_scheme());
        assert!(parsed.parsed_url.is_none());

        // Test vault scheme
        let parsed = UrlScheme::parse_url("vault://role@mount/database").unwrap();
        assert_eq!(parsed.scheme, UrlScheme::Vault);
        assert!(parsed.is_special_scheme());

        // Redis and its TLS / Valkey / unix-socket spellings
        for url in [
            "redis://localhost:6379/0",
            "rediss://user:pw@cache:6380/1",
            "valkey://localhost",
            "valkeys://localhost",
            "redis+unix:///run/redis.sock?db=1",
        ] {
            let parsed = UrlScheme::parse_url(url).unwrap();
            assert_eq!(parsed.scheme, UrlScheme::Redis, "{url}");
            assert!(parsed.is_database_scheme());
        }
        assert_eq!(UrlScheme::Redis.url_prefix(), "redis://");
        assert_eq!(UrlScheme::Redis.to_database_type(), Some("Redis"));
        assert_eq!(
            UrlScheme::for_database_type("valkey"),
            vec![UrlScheme::Redis]
        );

        // SQL Server and its spellings
        for url in [
            "mssql://sa:pw@db:1433/Sales",
            "sqlserver://sa@db/Sales?encrypt=strict",
            "mssql+tls://sa@db",
            "sqlserver+tls://sa@db",
        ] {
            let parsed = UrlScheme::parse_url(url).unwrap();
            assert_eq!(parsed.scheme, UrlScheme::SqlServer, "{url}");
            assert!(parsed.is_database_scheme());
        }
        // Redshift and its TLS spelling
        for url in [
            "redshift://awsuser:pw@c.abc.eu-west-1.redshift.amazonaws.com:5439/dev",
            "redshift://awsuser@c.example/dev?sslmode=verify-full",
            "redshift+tls://awsuser@c.example/dev",
        ] {
            let parsed = UrlScheme::parse_url(url).unwrap();
            assert_eq!(parsed.scheme, UrlScheme::Redshift, "{url}");
            assert!(parsed.is_database_scheme());
        }
        // libSQL / Turso and its spellings
        for url in [
            "libsql://my-db-org.turso.io",
            "libsql://my-db-org.turso.io?authToken=abc",
            "turso://my-db-org.turso.io",
            "libsql+http://localhost:8080",
        ] {
            let parsed = UrlScheme::parse_url(url).unwrap();
            assert_eq!(parsed.scheme, UrlScheme::LibSql, "{url}");
            assert!(parsed.is_database_scheme());
        }
        assert_eq!(UrlScheme::LibSql.url_prefix(), "libsql://");
        assert_eq!(UrlScheme::LibSql.to_database_type(), Some("libSQL"));
        assert_eq!(
            UrlScheme::for_database_type("turso"),
            vec![UrlScheme::LibSql]
        );

        assert_eq!(UrlScheme::Redshift.url_prefix(), "redshift://");
        assert_eq!(UrlScheme::Redshift.to_database_type(), Some("Redshift"));
        assert_eq!(
            UrlScheme::for_database_type("redshift"),
            vec![UrlScheme::Redshift]
        );

        assert_eq!(UrlScheme::SqlServer.url_prefix(), "mssql://");
        assert_eq!(UrlScheme::SqlServer.to_database_type(), Some("SQL Server"));
        assert_eq!(
            UrlScheme::for_database_type("sql server"),
            vec![UrlScheme::SqlServer]
        );

        // InfluxDB: every generation and TLS spelling
        for url in [
            "influxdb://localhost:8086/telemetry",
            "influxdbs://db.example/telemetry",
            "influxdb+https://db.example/telemetry",
            "influxdb2://:token@localhost/bucket?org=acme",
            "influxdb2s://db.example:443/bucket?org=acme",
            "influxdb3://localhost/mydb",
            "influxdb3+https://db.example/mydb",
        ] {
            let parsed = UrlScheme::parse_url(url).unwrap();
            assert_eq!(parsed.scheme, UrlScheme::InfluxDb, "{url}");
            assert!(parsed.is_database_scheme());
        }
        assert_eq!(UrlScheme::InfluxDb.url_prefix(), "influxdb://");
        assert_eq!(UrlScheme::InfluxDb.to_database_type(), Some("InfluxDB"));
        assert_eq!(
            UrlScheme::for_database_type("influx"),
            vec![UrlScheme::InfluxDb]
        );

        // Test unsupported scheme
        let result = UrlScheme::parse_url("unsupported://test");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            UrlSchemeError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn test_scheme_from_string() {
        assert_eq!(
            UrlScheme::from_str("postgresql").unwrap(),
            UrlScheme::Postgres
        );
        assert_eq!(
            UrlScheme::from_str("postgres").unwrap(),
            UrlScheme::Postgres
        );
        assert_eq!(UrlScheme::from_str("mysql").unwrap(), UrlScheme::MySQL);
        assert_eq!(UrlScheme::from_str("docker").unwrap(), UrlScheme::Docker);
        assert_eq!(UrlScheme::from_str("file").unwrap(), UrlScheme::File);
        assert_eq!(UrlScheme::from_str("session").unwrap(), UrlScheme::Session);

        // Case insensitive
        assert_eq!(
            UrlScheme::from_str("POSTGRESQL").unwrap(),
            UrlScheme::Postgres
        );
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
        assert_eq!(UrlScheme::File.to_database_type(), None);
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

    #[test]
    fn test_availability_follows_the_compiled_connectors() {
        assert_eq!(UrlScheme::Docker.is_available(), cfg!(feature = "docker"));
        assert_eq!(UrlScheme::MongoDB.is_available(), cfg!(feature = "mongodb"));
        assert_eq!(
            UrlScheme::Postgres.is_available(),
            cfg!(feature = "postgres")
        );
        // Not engines: always there
        for scheme in [UrlScheme::Session, UrlScheme::Recent, UrlScheme::Vault] {
            assert!(scheme.is_available(), "{scheme}");
        }
        // Disabled schemes are neither listed nor suggested …
        let listed: Vec<String> = UrlSchemeManager::get_all_schemes()
            .into_iter()
            .map(|(prefix, _)| prefix)
            .collect();
        assert_eq!(
            listed.contains(&"mongodb://".to_string()),
            cfg!(feature = "mongodb")
        );
        // … but still parse, so the connect error can say what is missing
        assert!(UrlScheme::parse_url("mongodb://db/app").is_ok());
    }
}
