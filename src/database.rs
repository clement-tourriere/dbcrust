//! Database abstraction layer for multi-database support
//! Supports PostgreSQL, SQLite, and MySQL/MariaDB
use async_trait::async_trait;
use tracing::debug;
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;
use url::Url;
use percent_encoding;

/// Supported database types
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DatabaseType {
    PostgreSQL,
    SQLite,
    MySQL,
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DatabaseType::PostgreSQL => write!(f, "PostgreSQL"),
            DatabaseType::SQLite => write!(f, "SQLite"),
            DatabaseType::MySQL => write!(f, "MySQL"),
        }
    }
}

/// Connection information parsed from database URL
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub database_type: DatabaseType,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub file_path: Option<String>, // For SQLite
    pub options: HashMap<String, String>, // Query parameters
    pub docker_container: Option<String>, // For Docker containers
}

/// Errors that can occur during database operations
#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Unsupported database URL scheme: {0}")]
    UnsupportedScheme(String),
    
    #[error("Invalid database URL: {0}")]
    InvalidUrl(String),
    
    #[error("Docker error: {0}")]
    Docker(#[from] crate::docker::DockerError),
    
    #[error("Connection error: {0}")]
    ConnectionError(String),
    
    #[error("Query error: {0}")]
    QueryError(String),
    
    #[error("Metadata error: {0}")]
    MetadataError(String),
    
    #[error("Feature not supported for {database_type}: {feature}")]
    FeatureNotSupported {
        database_type: DatabaseType,
        feature: String,
    },
    
    #[error("SQLx error: {0}")]
    SqlxError(#[from] sqlx::Error),
}

/// Factory for creating database clients
pub async fn create_database_client(connection_info: ConnectionInfo) -> Result<Box<dyn DatabaseClient>, DatabaseError> {
    match connection_info.database_type {
        DatabaseType::PostgreSQL => {
            let client = crate::database_postgresql::PostgreSQLClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
        DatabaseType::SQLite => {
            let client = crate::database_sqlite::SqliteClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
        DatabaseType::MySQL => {
            let client = crate::database_mysql::MySqlClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
    }
}

impl ConnectionInfo {
    /// Parse a database URL into connection information
    pub fn parse_url(url_str: &str) -> Result<Self, DatabaseError> {
        debug!("[ConnectionInfo::parse_url] Parsing URL: {}", crate::password_sanitizer::sanitize_connection_url(url_str));
        
        let url = Url::parse(url_str)
            .map_err(|e| DatabaseError::InvalidUrl(format!("Failed to parse URL: {e}")))?;

        let database_type = match url.scheme() {
            "postgresql" | "postgres" => DatabaseType::PostgreSQL,
            "sqlite" => DatabaseType::SQLite,
            "mysql" => DatabaseType::MySQL,
            "docker" => {
                // Docker URL parsing will be handled separately
                // For now, return a placeholder - will be resolved later
                DatabaseType::PostgreSQL // Default to PostgreSQL for docker:// URLs
            },
            scheme => return Err(DatabaseError::UnsupportedScheme(scheme.to_string())),
        };

        let mut connection_info = ConnectionInfo {
            database_type: database_type.clone(),
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };

        // Handle Docker URLs first
        if url.scheme() == "docker" {
            // Parse Docker URL using our custom parser
            if let Some((username, password, container_name, database_name)) = crate::docker::DockerClient::parse_docker_url(url_str) {
                connection_info.docker_container = Some(container_name);
                connection_info.username = username;
                connection_info.password = password;
                connection_info.database = database_name;
                // Database type will be determined later after container inspection
            } else {
                return Err(DatabaseError::InvalidUrl("Invalid Docker URL format".to_string()));
            }
            return Ok(connection_info);
        }

        // Parse database-specific connection details
        match database_type {
            DatabaseType::SQLite => {
                // For SQLite: sqlite:///path/to/file.db or sqlite://./relative/path.db
                let path = url.path();
                
                // Handle different SQLite URL formats:
                // sqlite:///absolute/path -> /absolute/path (absolute)
                // sqlite://./relative/path -> ./relative/path (relative)
                // sqlite:///./relative/path -> ./relative/path (relative)
                // sqlite:///relative/path -> relative/path (relative if starts with single slash)
                
                let file_path = if path.starts_with("/./") {
                    // sqlite:///./relative/path -> ./relative/path
                    path[1..].to_string()
                } else if path.starts_with("./") {
                    // sqlite://./relative/path -> ./relative/path
                    path.to_string()
                } else if path.starts_with("//") {
                    // sqlite:////absolute/path -> /absolute/path (absolute)
                    path[1..].to_string()
                } else if path.starts_with("/") && path.len() > 1 {
                    // sqlite:///relative/path -> relative/path (treat as relative)
                    // Only make it absolute if it looks like a real absolute path
                    if path.starts_with("/home/") || path.starts_with("/Users/") || path.starts_with("/tmp/") || path.starts_with("/var/") {
                        path.to_string()
                    } else {
                        path[1..].to_string()
                    }
                } else {
                    // sqlite:///path -> path or empty path
                    path.to_string()
                };
                
                connection_info.file_path = Some(file_path);
            }
            DatabaseType::PostgreSQL | DatabaseType::MySQL => {
                // For network databases
                connection_info.host = url.host_str().map(|h| h.to_string());
                connection_info.port = url.port();
                connection_info.username = if url.username().is_empty() { 
                    None 
                } else { 
                    Some(url.username().to_string()) 
                };
                connection_info.password = url.password().map(|p| p.to_string());
                
                // Database name is the path without leading slash - URL decode it
                if let Some(mut segments) = url.path_segments() {
                    if let Some(db_name) = segments.next() {
                        if !db_name.is_empty() {
                            // URL-decode the database name to handle special characters like %3A (colon)
                            let decoded_db_name = percent_encoding::percent_decode_str(db_name)
                                .decode_utf8()
                                .map_err(|e| DatabaseError::InvalidUrl(format!("Failed to decode database name '{}': {}", db_name, e)))?
                                .to_string();
                            connection_info.database = Some(decoded_db_name);
                        }
                    }
                }
            }
        }

        // Parse query parameters
        for (key, value) in url.query_pairs() {
            connection_info.options.insert(key.to_string(), value.to_string());
        }

        debug!("[ConnectionInfo::parse_url] Parsed connection info for {}", database_type);
        Ok(connection_info)
    }

    /// Check if SSH tunneling is applicable for this database type
    pub fn supports_ssh_tunnel(&self) -> bool {
        match self.database_type {
            DatabaseType::PostgreSQL | DatabaseType::MySQL => true,
            DatabaseType::SQLite => false, // File-based, no network connection
        }
    }

    /// Check if this connection is for a Docker container
    pub fn is_docker_connection(&self) -> bool {
        self.docker_container.is_some()
    }

    /// Get the default port for this database type
    pub fn default_port(&self) -> Option<u16> {
        match self.database_type {
            DatabaseType::PostgreSQL => Some(5432),
            DatabaseType::MySQL => Some(3306),
            DatabaseType::SQLite => None, // File-based
        }
    }

    /// Check if this connection info represents the same logical connection as another
    /// (useful for connection caching and reuse)
    pub fn is_same_connection(&self, other: &ConnectionInfo) -> bool {
        if self.database_type != other.database_type {
            return false;
        }

        match self.database_type {
            DatabaseType::SQLite => self.file_path == other.file_path,
            DatabaseType::PostgreSQL | DatabaseType::MySQL => {
                self.host == other.host
                    && self.port == other.port
                    && self.username == other.username
                    && self.database == other.database
            }
        }
    }
    
    /// Build a complete connection URL from connection information
    /// This is useful for storing in connection history
    pub fn to_url(&self) -> String {
        match self.database_type {
            DatabaseType::SQLite => {
                if let Some(ref file_path) = self.file_path {
                    format!("sqlite://{file_path}")
                } else {
                    "sqlite://".to_string()
                }
            },
            DatabaseType::PostgreSQL => {
                let mut url = "postgres://".to_string();
                
                // Build standard PostgreSQL URL with resolved connection details
                if let Some(ref username) = self.username {
                    url.push_str(username);
                    url.push('@');
                }
                if let Some(ref host) = self.host {
                    url.push_str(host);
                    if let Some(port) = self.port {
                        url.push(':');
                        url.push_str(&port.to_string());
                    }
                }
                if let Some(ref database) = self.database {
                    url.push('/');
                    url.push_str(database);
                }
                
                // Add docker container info as a comment-like suffix if present
                if let Some(ref container) = self.docker_container {
                    url.push_str(&format!(" # Docker: {container}"));
                }
                url
            },
            DatabaseType::MySQL => {
                let mut url = "mysql://".to_string();
                
                // Build standard MySQL URL with resolved connection details
                if let Some(ref username) = self.username {
                    url.push_str(username);
                    url.push('@');
                }
                if let Some(ref host) = self.host {
                    url.push_str(host);
                    if let Some(port) = self.port {
                        url.push(':');
                        url.push_str(&port.to_string());
                    }
                }
                if let Some(ref database) = self.database {
                    url.push('/');
                    url.push_str(database);
                }
                
                // Add docker container info as a comment-like suffix if present
                if let Some(ref container) = self.docker_container {
                    url.push_str(&format!(" # Docker: {container}"));
                }
                url
            }
        }
    }
}

/// Trait for database-specific metadata operations
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    /// Get list of schemas (databases/namespaces)
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError>;

    /// Get list of tables and views in a schema
    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError>;

    /// Get list of columns for a table
    async fn get_columns(&self, table: &str, schema: Option<&str>) -> Result<Vec<String>, DatabaseError>;

    /// Get list of functions in a schema
    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError>;

    /// Get detailed table information (indexes, constraints, etc.)
    async fn get_table_details(&self, table: &str, schema: Option<&str>) -> Result<crate::db::TableDetails, DatabaseError>;

    /// Check if a query can be explained
    fn supports_explain(&self) -> bool;

    /// Get the default schema/database name for this database type
    fn default_schema(&self) -> Option<String>;
}

/// Trait for executing database queries and managing connections
#[async_trait]
pub trait DatabaseClient: Send + Sync {
    /// Execute a query and return results as Vec<Vec<String>>
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// Execute a query with EXPLAIN prefix
    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// Execute a query with EXPLAIN prefix and return raw output (unformatted)
    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// List available databases (where applicable)
    async fn list_databases(&self) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// Connect to a different database
    async fn connect_to_database(&mut self, database: &str) -> Result<(), DatabaseError>;

    /// Get the current database name
    fn get_current_database(&self) -> String;

    /// Get connection information
    fn get_connection_info(&self) -> &ConnectionInfo;

    /// Get metadata provider for this database
    fn get_metadata_provider(&self) -> &dyn MetadataProvider;

    /// Check if the connection is still active
    async fn is_connected(&self) -> bool;

    /// Close the connection
    async fn close(&mut self) -> Result<(), DatabaseError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn test_connection_info_to_url_postgresql() {
        // Test standard PostgreSQL connection
        let conn_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("localhost".to_string()),
            port: Some(5432),
            username: Some("user".to_string()),
            password: Some("password".to_string()),
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };
        
        let url = conn_info.to_url();
        assert_eq!(url, "postgres://user@localhost:5432/testdb");
    }

    #[rstest]
    fn test_connection_info_to_url_docker_postgresql() {
        // Test Docker PostgreSQL connection
        let conn_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("container.orb.local".to_string()),
            port: Some(5432),
            username: Some("postgres".to_string()),
            password: Some("password".to_string()),
            database: Some("myapp".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: Some("myapp-postgres".to_string()),
        };
        
        let url = conn_info.to_url();
        assert_eq!(url, "postgres://postgres@container.orb.local:5432/myapp # Docker: myapp-postgres");
    }

    #[rstest]
    fn test_connection_info_to_url_mysql() {
        // Test MySQL connection
        let conn_info = ConnectionInfo {
            database_type: DatabaseType::MySQL,
            host: Some("localhost".to_string()),
            port: Some(3306),
            username: Some("root".to_string()),
            password: Some("password".to_string()),
            database: Some("testdb".to_string()),
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
        };
        
        let url = conn_info.to_url();
        assert_eq!(url, "mysql://root@localhost:3306/testdb");
    }

    #[rstest]
    fn test_connection_info_to_url_sqlite() {
        // Test SQLite connection
        let conn_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: Some("/path/to/database.db".to_string()),
            options: HashMap::new(),
            docker_container: None,
        };
        
        let url = conn_info.to_url();
        assert_eq!(url, "sqlite:///path/to/database.db");
    }

    #[rstest]
    #[case("postgres://user:pass@localhost:5432/mydb", DatabaseType::PostgreSQL, Some("localhost"), Some(5432), Some("user"), Some("mydb"))]
    #[case("postgres://user@localhost/mydb", DatabaseType::PostgreSQL, Some("localhost"), None, Some("user"), Some("mydb"))]
    #[case("sqlite:///path/to/database.db", DatabaseType::SQLite, None, None, None, None)]
    #[case("mysql://user:pass@localhost:3306/mydb", DatabaseType::MySQL, Some("localhost"), Some(3306), Some("user"), Some("mydb"))]
    fn test_parse_database_url(
        #[case] url: &str,
        #[case] expected_type: DatabaseType,
        #[case] expected_host: Option<&str>,
        #[case] expected_port: Option<u16>,
        #[case] expected_user: Option<&str>,
        #[case] expected_db: Option<&str>,
    ) {
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        
        assert_eq!(conn_info.database_type, expected_type);
        assert_eq!(conn_info.host.as_deref(), expected_host);
        assert_eq!(conn_info.port, expected_port);
        assert_eq!(conn_info.username.as_deref(), expected_user);
        assert_eq!(conn_info.database.as_deref(), expected_db);
    }

    #[rstest]
    #[case("postgres://localhost/db", true)]
    #[case("mysql://localhost/db", true)]
    #[case("sqlite:///path/to/db", false)]
    fn test_ssh_tunnel_support(#[case] url: &str, #[case] expected: bool) {
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(conn_info.supports_ssh_tunnel(), expected);
    }

    #[rstest]
    #[case("postgres://localhost/db", Some(5432))]
    #[case("mysql://localhost/db", Some(3306))]
    #[case("sqlite:///path/to/db", None)]
    fn test_default_ports(#[case] url: &str, #[case] expected: Option<u16>) {
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(conn_info.default_port(), expected);
    }

    #[test]
    fn test_invalid_url() {
        let result = ConnectionInfo::parse_url("invalid-url");
        assert!(result.is_err());
    }

    #[test]
    fn test_unsupported_scheme() {
        let result = ConnectionInfo::parse_url("oracle://localhost/db");
        assert!(matches!(result, Err(DatabaseError::UnsupportedScheme(_))));
    }

    #[test]
    fn test_url_encoded_database_name() {
        // Test URL with encoded database name (as generated by Django)
        let url = "postgres://user:pass@host:5432/tt2%3Amain";
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        
        // Verify that %3A was decoded to :
        assert_eq!(conn_info.database.as_deref(), Some("tt2:main"));
        assert_eq!(conn_info.username.as_deref(), Some("user"));
    }

}