//! Database abstraction layer for multi-database support
//! Supports PostgreSQL, SQLite, and MySQL/MariaDB
use async_trait::async_trait;
use percent_encoding;
use regex;
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;
use tracing::debug;
use url::Url;

/// Supported database types
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DatabaseType {
    PostgreSQL,
    SQLite,
    MySQL,
    ClickHouse,
    MongoDB,
    Elasticsearch,
    // File formats (via DataFusion)
    Parquet,
    CSV,
    JSON,
    DuckDB,
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Trait for database type specific behavior to eliminate match statements
pub trait DatabaseTypeExt {
    /// Get the default port for this database type
    fn default_port(&self) -> Option<u16>;

    /// Get the display name for this database type
    fn display_name(&self) -> &'static str;

    /// Check if this database type supports SSH tunneling
    fn supports_ssh_tunnel(&self) -> bool;

    /// Get URL schemes supported by this database type
    fn url_schemes(&self) -> &'static [&'static str];

    /// Check if this database type is file-based (no network connection)
    fn is_file_based(&self) -> bool;

    /// Check if this database type supports JSON EXPLAIN output
    fn supports_json_explain(&self) -> bool;

    /// Check if this database type requires authentication (password)
    fn requires_authentication(&self) -> bool;

    /// Get the URL scheme for building URLs
    fn url_scheme(&self) -> &'static str;

    /// Get function names for SQL completion
    fn sql_functions(&self) -> &'static [&'static str];

    /// Check if from_unixtime is available
    fn supports_from_unixtime(&self) -> bool;

    /// Get environment variable names for username lookup in Docker containers
    fn docker_username_env_vars(&self) -> &'static [&'static str];

    /// Get environment variable names for password lookup in Docker containers
    fn docker_password_env_vars(&self) -> &'static [&'static str];

    /// Get environment variable names for database name lookup in Docker containers
    fn docker_database_env_vars(&self) -> &'static [&'static str];

    /// Get default username for this database type
    fn default_username(&self) -> &'static str;
}

impl DatabaseTypeExt for DatabaseType {
    fn default_port(&self) -> Option<u16> {
        match self {
            DatabaseType::PostgreSQL => Some(5432),
            DatabaseType::MySQL => Some(3306),
            DatabaseType::SQLite => None,              // File-based
            DatabaseType::ClickHouse => Some(8123),    // HTTP interface
            DatabaseType::MongoDB => Some(27017),      // MongoDB default port
            DatabaseType::Elasticsearch => Some(9200), // HTTP REST API
            DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => None, // File-based
        }
    }

    fn display_name(&self) -> &'static str {
        match self {
            DatabaseType::PostgreSQL => "PostgreSQL",
            DatabaseType::MySQL => "MySQL",
            DatabaseType::SQLite => "SQLite",
            DatabaseType::ClickHouse => "ClickHouse",
            DatabaseType::MongoDB => "MongoDB",
            DatabaseType::Elasticsearch => "Elasticsearch",
            DatabaseType::Parquet => "Parquet",
            DatabaseType::CSV => "CSV",
            DatabaseType::JSON => "JSON",
            DatabaseType::DuckDB => "DuckDB",
        }
    }

    fn supports_ssh_tunnel(&self) -> bool {
        match self {
            DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MongoDB
            | DatabaseType::Elasticsearch => true,
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => false, // File-based, no network connection
        }
    }

    fn url_schemes(&self) -> &'static [&'static str] {
        match self {
            DatabaseType::PostgreSQL => &["postgresql", "postgres"],
            DatabaseType::MySQL => &["mysql"],
            DatabaseType::SQLite => &["sqlite"],
            DatabaseType::ClickHouse => &["clickhouse"],
            DatabaseType::MongoDB => &["mongodb", "mongodb+srv"],
            DatabaseType::Elasticsearch => &["elasticsearch", "elastic", "es"],
            DatabaseType::Parquet => &["parquet"],
            DatabaseType::CSV => &["csv"],
            DatabaseType::JSON => &["json", "ndjson"],
            DatabaseType::DuckDB => &["duckdb"],
        }
    }

    fn is_file_based(&self) -> bool {
        match self {
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => true,
            DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MongoDB
            | DatabaseType::Elasticsearch => false,
        }
    }

    fn supports_json_explain(&self) -> bool {
        match self {
            DatabaseType::PostgreSQL
            | DatabaseType::ClickHouse
            | DatabaseType::MongoDB
            | DatabaseType::Elasticsearch
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => true, // DataFusion supports EXPLAIN
            DatabaseType::MySQL | DatabaseType::SQLite => false,
        }
    }

    fn requires_authentication(&self) -> bool {
        match self {
            DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MongoDB
            | DatabaseType::Elasticsearch => true,
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => false, // File-based, no auth needed
        }
    }

    fn url_scheme(&self) -> &'static str {
        match self {
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "mysql",
            DatabaseType::SQLite => "sqlite",
            DatabaseType::ClickHouse => "clickhouse",
            DatabaseType::MongoDB => "mongodb",
            DatabaseType::Elasticsearch => "elasticsearch",
            DatabaseType::Parquet => "parquet",
            DatabaseType::CSV => "csv",
            DatabaseType::JSON => "json",
            DatabaseType::DuckDB => "duckdb",
        }
    }

    fn sql_functions(&self) -> &'static [&'static str] {
        match self {
            DatabaseType::PostgreSQL => &[
                "COALESCE",
                "NULLIF",
                "GREATEST",
                "LEAST",
                "NOW",
                "CURRENT_DATE",
                "CURRENT_TIME",
                "CURRENT_TIMESTAMP",
                "AGE",
                "EXTRACT",
                "DATE_PART",
                "TO_CHAR",
                "TO_DATE",
                "TO_TIMESTAMP",
                "ARRAY_AGG",
                "STRING_AGG",
                "JSON_BUILD_OBJECT",
                "JSON_AGG",
                "JSONB_BUILD_OBJECT",
            ],
            DatabaseType::MySQL => &[
                "COALESCE",
                "IFNULL",
                "NULLIF",
                "GREATEST",
                "LEAST",
                "NOW",
                "CURRENT_DATE",
                "CURRENT_TIME",
                "CURRENT_TIMESTAMP",
                "DATE_FORMAT",
                "STR_TO_DATE",
                "FROM_UNIXTIME",
                "UNIX_TIMESTAMP",
                "GROUP_CONCAT",
                "JSON_OBJECT",
                "JSON_ARRAY",
            ],
            DatabaseType::SQLite => &[
                "COALESCE",
                "IFNULL",
                "NULLIF",
                "MAX",
                "MIN",
                "DATE",
                "TIME",
                "DATETIME",
                "STRFTIME",
                "JULIANDAY",
                "GROUP_CONCAT",
                "JSON_OBJECT",
                "JSON_ARRAY",
            ],
            DatabaseType::ClickHouse => &[
                "COALESCE",
                "NULLIF",
                "GREATEST",
                "LEAST",
                "NOW",
                "TODAY",
                "YESTERDAY",
                "CURRENT_TIMESTAMP",
                "TO_DATE",
                "TO_DATETIME",
                "FORMAT_DATETIME",
                "PARSE_DATETIME",
                "FROM_UNIXTIME",
                "TO_UNIX_TIMESTAMP",
                "GROUP_ARRAY",
                "GROUP_CONCAT",
                "COUNT_DISTINCT",
                "UNIQ",
                "UNIQ_EXACT",
                "SUM",
                "AVG",
                "MIN",
                "MAX",
                "ANY",
                "ANY_LAST",
                "ARRAY_JOIN",
                "HAS",
                "LENGTH",
                "EMPTY",
                "NOT_EMPTY",
                "LOWER",
                "UPPER",
                "SUBSTRING",
                "POSITION",
                "EXTRACT",
                "JSON_EXTRACT",
                "JSON_EXTRACT_STRING",
            ],
            DatabaseType::MongoDB => &[
                // MongoDB aggregation operators
                "$sum",
                "$avg",
                "$min",
                "$max",
                "$count",
                "$match",
                "$group",
                "$sort",
                "$limit",
                "$skip",
                "$project",
                "$lookup",
                "$unwind",
                "$addFields",
                // Date operators
                "$dateToString",
                "$dateFromString",
                "$year",
                "$month",
                "$dayOfMonth",
                // String operators
                "$concat",
                "$substr",
                "$toLower",
                "$toUpper",
                "$split",
                // Array operators
                "$size",
                "$push",
                "$addToSet",
                "$first",
                "$last",
                // Conditional operators
                "$cond",
                "$ifNull",
                "$switch",
            ],
            DatabaseType::Elasticsearch => &[
                // Standard SQL functions supported by Elasticsearch
                "COUNT",
                "SUM",
                "AVG",
                "MIN",
                "MAX",
                "COALESCE",
                "NULLIF",
                "GREATEST",
                "LEAST",
                // Date/time functions
                "NOW",
                "CURRENT_DATE",
                "CURRENT_TIME",
                "CURRENT_TIMESTAMP",
                "EXTRACT",
                "DATE_FORMAT",
                "DATE_HISTOGRAM",
                "DATE_TRUNC",
                // String functions
                "CONCAT",
                "LENGTH",
                "SUBSTRING",
                "UPPER",
                "LOWER",
                "TRIM",
                "REPLACE",
                // Search-specific functions
                "MATCH",
                "QUERY",
                "SCORE",
                // Aggregation functions
                "TERMS",
                "STATS",
                "EXTENDED_STATS",
                "PERCENTILES",
                "CARDINALITY",
                "VALUE_COUNT",
                // Math functions
                "ABS",
                "CEIL",
                "FLOOR",
                "ROUND",
                "SQRT",
                "POWER",
                // Conditional functions
                "CASE",
                "WHEN",
                "THEN",
                "ELSE",
                "END",
            ],
            // DataFusion SQL functions (for file formats)
            DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => &[
                // Aggregate functions
                "COUNT",
                "SUM",
                "AVG",
                "MIN",
                "MAX",
                "STDDEV",
                "VAR",
                "MEDIAN",
                "APPROX_DISTINCT",
                "APPROX_PERCENTILE_CONT",
                "ARRAY_AGG",
                "FIRST_VALUE",
                "LAST_VALUE",
                // Scalar functions
                "ABS",
                "CEIL",
                "FLOOR",
                "ROUND",
                "TRUNC",
                "SQRT",
                "POW",
                "EXP",
                "LN",
                "LOG",
                "LOG2",
                "LOG10",
                // String functions
                "ASCII",
                "BIT_LENGTH",
                "BTRIM",
                "CHARACTER_LENGTH",
                "CHAR_LENGTH",
                "CONCAT",
                "CONCAT_WS",
                "INITCAP",
                "LEFT",
                "LENGTH",
                "LOWER",
                "LPAD",
                "LTRIM",
                "OCTET_LENGTH",
                "REPEAT",
                "REPLACE",
                "REVERSE",
                "RIGHT",
                "RPAD",
                "RTRIM",
                "SPLIT_PART",
                "STARTS_WITH",
                "STRPOS",
                "SUBSTR",
                "SUBSTRING",
                "TRANSLATE",
                "TRIM",
                "UPPER",
                // Date/time functions
                "NOW",
                "CURRENT_DATE",
                "CURRENT_TIME",
                "CURRENT_TIMESTAMP",
                "DATE_TRUNC",
                "DATE_PART",
                "EXTRACT",
                "TO_TIMESTAMP",
                "TO_TIMESTAMP_MILLIS",
                "TO_TIMESTAMP_MICROS",
                "TO_TIMESTAMP_SECONDS",
                // Conditional functions
                "COALESCE",
                "NULLIF",
                "CASE",
                "WHEN",
                "THEN",
                "ELSE",
                "END",
                // Type conversion
                "CAST",
                "TRY_CAST",
                // Array functions
                "ARRAY_LENGTH",
                "ARRAY_POSITION",
                "ARRAY_APPEND",
                "ARRAY_CONCAT",
                "ARRAY_CONTAINS",
                "ARRAY_DISTINCT",
                "ARRAY_ELEMENT",
                "ARRAY_REMOVE",
                "ARRAY_REPLACE",
                "CARDINALITY",
                // Window functions
                "ROW_NUMBER",
                "RANK",
                "DENSE_RANK",
                "PERCENT_RANK",
                "CUME_DIST",
                "NTILE",
                "LAG",
                "LEAD",
            ],
        }
    }

    fn supports_from_unixtime(&self) -> bool {
        match self {
            DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MongoDB
            | DatabaseType::Elasticsearch
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => true, // DataFusion supports timestamp functions
            DatabaseType::PostgreSQL | DatabaseType::SQLite => false,
        }
    }

    fn docker_username_env_vars(&self) -> &'static [&'static str] {
        match self {
            DatabaseType::PostgreSQL => &["POSTGRES_USER", "PGUSER"],
            DatabaseType::MySQL => &["MYSQL_USER"],
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => &[],
            DatabaseType::ClickHouse => &["CLICKHOUSE_USER"],
            DatabaseType::MongoDB => &["MONGO_INITDB_ROOT_USERNAME"],
            DatabaseType::Elasticsearch => &["ELASTIC_USERNAME", "ES_USERNAME"],
        }
    }

    fn docker_password_env_vars(&self) -> &'static [&'static str] {
        match self {
            DatabaseType::PostgreSQL => &["POSTGRES_PASSWORD", "PGPASSWORD"],
            DatabaseType::MySQL => &["MYSQL_PASSWORD", "MYSQL_ROOT_PASSWORD"],
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => &[],
            DatabaseType::ClickHouse => &["CLICKHOUSE_PASSWORD"],
            DatabaseType::MongoDB => &["MONGO_INITDB_ROOT_PASSWORD"],
            DatabaseType::Elasticsearch => &["ELASTIC_PASSWORD", "ES_PASSWORD"],
        }
    }

    fn docker_database_env_vars(&self) -> &'static [&'static str] {
        match self {
            DatabaseType::PostgreSQL => &["POSTGRES_DB", "PGDATABASE"],
            DatabaseType::MySQL => &["MYSQL_DATABASE"],
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => &[],
            DatabaseType::ClickHouse => &["CLICKHOUSE_DB"],
            DatabaseType::MongoDB => &["MONGO_INITDB_DATABASE"],
            DatabaseType::Elasticsearch => &["ELASTIC_INDEX", "ES_INDEX"],
        }
    }

    fn default_username(&self) -> &'static str {
        match self {
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "root",
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => "",
            DatabaseType::ClickHouse => "default",
            DatabaseType::MongoDB => "admin",
            DatabaseType::Elasticsearch => "elastic",
        }
    }
}

impl DatabaseType {
    /// Create DatabaseType from URL scheme
    pub fn from_scheme(scheme: &str) -> Result<Self, DatabaseError> {
        match scheme {
            "postgresql" | "postgres" => Ok(DatabaseType::PostgreSQL),
            "sqlite" => Ok(DatabaseType::SQLite),
            "mysql" => Ok(DatabaseType::MySQL),
            "clickhouse" => Ok(DatabaseType::ClickHouse),
            "mongodb" | "mongodb+srv" => Ok(DatabaseType::MongoDB),
            "elasticsearch" | "elastic" | "es" => Ok(DatabaseType::Elasticsearch),
            "parquet" => Ok(DatabaseType::Parquet),
            "csv" => Ok(DatabaseType::CSV),
            "json" | "ndjson" => Ok(DatabaseType::JSON),
            "duckdb" => Ok(DatabaseType::DuckDB),
            "docker" => Ok(DatabaseType::PostgreSQL), // Default to PostgreSQL for docker:// URLs
            scheme => Err(DatabaseError::UnsupportedScheme(scheme.to_string())),
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
    pub file_path: Option<String>,        // For SQLite
    pub options: HashMap<String, String>, // Query parameters
    pub docker_container: Option<String>, // For Docker containers
}

/// Server information returned by database connections
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub server_type: String,         // e.g., "PostgreSQL", "MySQL", "SQLite"
    pub server_version: String,      // e.g., "17.5 (Debian 17.5-1.pgdg120+1)"
    pub version_major: Option<u16>,  // e.g., 17 for PostgreSQL 17.x
    pub version_minor: Option<u16>,  // e.g., 5 for PostgreSQL 17.5
    pub version_patch: Option<u16>,  // e.g., 0 for PostgreSQL 17.5.0
    pub client_version: String,      // DBCrust version
    pub supports_transactions: bool, // Whether the database supports transactions
    pub supports_roles: bool,        // Whether the database supports role-based auth
    pub additional_info: HashMap<String, String>, // Any additional database-specific info
}

impl ServerInfo {
    /// Create a new ServerInfo with minimal required fields
    pub fn new(server_type: String, server_version: String) -> Self {
        Self {
            server_type,
            server_version,
            version_major: None,
            version_minor: None,
            version_patch: None,
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            supports_transactions: true,
            supports_roles: false,
            additional_info: HashMap::new(),
        }
    }

    /// Parse version numbers from version string
    pub fn parse_version_numbers(&mut self) {
        let version_regex = regex::Regex::new(r"(\d+)\.?(\d+)?\.?(\d+)?").unwrap();
        if let Some(captures) = version_regex.captures(&self.server_version) {
            self.version_major = captures.get(1).and_then(|m| m.as_str().parse().ok());
            self.version_minor = captures.get(2).and_then(|m| m.as_str().parse().ok());
            self.version_patch = captures.get(3).and_then(|m| m.as_str().parse().ok());
        }
    }

    /// Create ServerInfo for PostgreSQL
    pub fn postgresql(server_version: String) -> Self {
        let mut info = Self::new("PostgreSQL".to_string(), server_version);
        info.supports_transactions = true;
        info.supports_roles = true;
        info.parse_version_numbers();
        info
    }

    /// Create ServerInfo for MySQL
    pub fn mysql(server_version: String) -> Self {
        let mut info = Self::new("MySQL".to_string(), server_version);
        info.supports_transactions = true;
        info.supports_roles = info.version_major.map_or(false, |major| major >= 8);
        info.parse_version_numbers();
        info
    }

    /// Create ServerInfo for SQLite
    pub fn sqlite(server_version: String) -> Self {
        let mut info = Self::new("SQLite".to_string(), server_version);
        info.supports_transactions = true;
        info.supports_roles = false;
        info.parse_version_numbers();
        info
    }
}

impl fmt::Display for ServerInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Server: {} {}", self.server_type, self.server_version)
    }
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

    #[error("DataFusion error: {0}")]
    DataFusionError(#[from] datafusion::error::DataFusionError),
}

/// Factory for creating database clients
pub async fn create_database_client(
    connection_info: ConnectionInfo,
) -> Result<Box<dyn DatabaseClient>, DatabaseError> {
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
        DatabaseType::ClickHouse => {
            let client = crate::database_clickhouse::ClickHouseClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
        DatabaseType::MongoDB => {
            let client = crate::database_mongodb::MongoDBClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
        DatabaseType::Elasticsearch => {
            let client =
                crate::database_elasticsearch::ElasticsearchClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
        // File formats via DataFusion
        DatabaseType::Parquet | DatabaseType::CSV | DatabaseType::JSON | DatabaseType::DuckDB => {
            let client = crate::database_datafusion::DataFusionClient::new(connection_info).await?;
            Ok(Box::new(client))
        }
    }
}

impl ConnectionInfo {
    /// Parse a database URL into connection information
    pub fn parse_url(url_str: &str) -> Result<Self, DatabaseError> {
        debug!(
            "[ConnectionInfo::parse_url] Parsing URL: {}",
            crate::password_sanitizer::sanitize_connection_url(url_str)
        );

        let url = Url::parse(url_str)
            .map_err(|e| DatabaseError::InvalidUrl(format!("Failed to parse URL: {e}")))?;

        let database_type = DatabaseType::from_scheme(url.scheme())?;

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
            if let Some((username, password, container_name, database_name)) =
                crate::docker::DockerClient::parse_docker_url(url_str)
            {
                connection_info.docker_container = Some(container_name);
                connection_info.username = username;
                connection_info.password = password;
                connection_info.database = database_name;
                // Database type will be determined later after container inspection
            } else {
                return Err(DatabaseError::InvalidUrl(
                    "Invalid Docker URL format".to_string(),
                ));
            }
            return Ok(connection_info);
        }

        // Parse database-specific connection details
        if database_type.is_file_based() {
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
                if path.starts_with("/home/")
                    || path.starts_with("/Users/")
                    || path.starts_with("/tmp/")
                    || path.starts_with("/var/")
                {
                    path.to_string()
                } else {
                    path[1..].to_string()
                }
            } else {
                // sqlite:///path -> path or empty path
                path.to_string()
            };

            connection_info.file_path = Some(file_path);
        } else {
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
                            .map_err(|e| {
                                DatabaseError::InvalidUrl(format!(
                                    "Failed to decode database name '{}': {}",
                                    db_name, e
                                ))
                            })?
                            .to_string();
                        connection_info.database = Some(decoded_db_name);
                    }
                }
            }
        }

        // Parse query parameters
        for (key, value) in url.query_pairs() {
            connection_info
                .options
                .insert(key.to_string(), value.to_string());
        }

        debug!(
            "[ConnectionInfo::parse_url] Parsed connection info for {}",
            database_type
        );
        Ok(connection_info)
    }

    /// Check if SSH tunneling is applicable for this database type
    pub fn supports_ssh_tunnel(&self) -> bool {
        self.database_type.supports_ssh_tunnel()
    }

    /// Check if this connection is for a Docker container
    pub fn is_docker_connection(&self) -> bool {
        self.docker_container.is_some()
    }

    /// Get the default port for this database type
    pub fn default_port(&self) -> Option<u16> {
        self.database_type.default_port()
    }

    /// Check if this connection info represents the same logical connection as another
    /// (useful for connection caching and reuse)
    pub fn is_same_connection(&self, other: &ConnectionInfo) -> bool {
        if self.database_type != other.database_type {
            return false;
        }

        if self.database_type.is_file_based() {
            self.file_path == other.file_path
        } else {
            self.host == other.host
                && self.port == other.port
                && self.username == other.username
                && self.database == other.database
        }
    }

    /// Build a complete connection URL from connection information
    /// This is useful for storing in connection history
    pub fn to_url(&self) -> String {
        if self.database_type.is_file_based() {
            if let Some(ref file_path) = self.file_path {
                format!("{}://{}", self.database_type.url_scheme(), file_path)
            } else {
                format!("{}://", self.database_type.url_scheme())
            }
        } else {
            let mut url = format!("{}://", self.database_type.url_scheme());

            // Build standard network database URL with resolved connection details
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

/// Trait for database-specific metadata operations
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    /// Get list of schemas (databases/namespaces)
    async fn get_schemas(&self) -> Result<Vec<String>, DatabaseError>;

    /// Get list of tables and views in a schema
    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError>;

    /// Get list of columns for a table
    async fn get_columns(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, DatabaseError>;

    /// Get list of functions in a schema
    async fn get_functions(&self, schema: Option<&str>) -> Result<Vec<String>, DatabaseError>;

    /// Get detailed table information (indexes, constraints, etc.)
    async fn get_table_details(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<crate::db::TableDetails, DatabaseError>;

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

    /// Test query execution without side effects (for validation)
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError>;

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

    /// Get server information including version details
    async fn get_server_info(&self) -> Result<ServerInfo, DatabaseError>;
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
        assert_eq!(
            url,
            "postgres://postgres@container.orb.local:5432/myapp # Docker: myapp-postgres"
        );
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
    #[case(
        "postgres://user:pass@localhost:5432/mydb",
        DatabaseType::PostgreSQL,
        Some("localhost"),
        Some(5432),
        Some("user"),
        Some("mydb")
    )]
    #[case(
        "postgres://user@localhost/mydb",
        DatabaseType::PostgreSQL,
        Some("localhost"),
        None,
        Some("user"),
        Some("mydb")
    )]
    #[case(
        "sqlite:///path/to/database.db",
        DatabaseType::SQLite,
        None,
        None,
        None,
        None
    )]
    #[case(
        "mysql://user:pass@localhost:3306/mydb",
        DatabaseType::MySQL,
        Some("localhost"),
        Some(3306),
        Some("user"),
        Some("mydb")
    )]
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
