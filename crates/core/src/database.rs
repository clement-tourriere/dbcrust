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

/// Process-wide query timeout in seconds (0 = no timeout). Initialized from
/// `Config::query_timeout_seconds` at startup; database clients read it here
/// because they are constructed without access to the Config (same pattern as
/// the global complex-display config).
static QUERY_TIMEOUT_SECONDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(30);

pub fn set_query_timeout_seconds(seconds: u64) {
    QUERY_TIMEOUT_SECONDS.store(seconds, std::sync::atomic::Ordering::Relaxed);
}

/// The configured query timeout, or None when timeouts are disabled (0).
pub fn query_timeout() -> Option<std::time::Duration> {
    match QUERY_TIMEOUT_SECONDS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        seconds => Some(std::time::Duration::from_secs(seconds)),
    }
}

/// Process-wide one-shot mode flag (`-c`/`-f`/stdin scripts). Set at startup
/// before any connection is made; deep call sites read it here to keep stdout
/// free of status chatter so machine-readable output stays parseable.
static ONE_SHOT_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_one_shot_mode(enabled: bool) {
    ONE_SHOT_MODE.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn one_shot_mode() -> bool {
    ONE_SHOT_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Process-wide `--read-only` request. Set at startup BEFORE any connection so
/// backend clients can harden at connect-options level (SQLite
/// `PRAGMA query_only`, PostgreSQL `default_transaction_read_only`) — a
/// post-connect `SET`/`PRAGMA` would only reach one pooled connection.
/// `Database` instances also initialize their statement guard from it.
static READ_ONLY_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_read_only_requested(enabled: bool) {
    READ_ONLY_REQUESTED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn read_only_requested() -> bool {
    READ_ONLY_REQUESTED.load(std::sync::atomic::Ordering::Relaxed)
}

// The hardening option keys and their pure helpers live in `crate::options`
// (they are read by `config`, `password_sanitizer` and the drivers); re-exported
// so `crate::database::OPT_HARDEN_*` keeps resolving.
pub use crate::options::{
    OPT_HARDEN_READ_ONLY, OPT_HARDEN_TIMEOUT_SECS, connection_timeout_secs, is_hardened_connection,
};

/// Whether this connection must start every transaction read-only.
pub fn connection_read_only(options: &HashMap<String, String>) -> bool {
    read_only_requested()
        || options
            .get(OPT_HARDEN_READ_ONLY)
            .is_some_and(|value| value == "1")
}

thread_local! {
    static THREAD_INTERRUPT_FLAG: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install (or clear, with `None`) an interrupt flag scoped to the calling
/// thread. While set, [`interrupt_flag`] returns it instead of the
/// process-wide flag. The desktop GUI runs one current-thread runtime per
/// connection, so a per-thread flag cancels exactly that connection's
/// statement without touching queries running on other connections.
pub fn set_thread_interrupt_flag(flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) {
    THREAD_INTERRUPT_FLAG.with(|slot| *slot.borrow_mut() = flag);
}

/// Interrupt flag polled by database clients to cancel server-side. Returns
/// the calling thread's flag when one was installed with
/// [`set_thread_interrupt_flag`], otherwise the process-wide flag set by the
/// interactive Ctrl-C handler (one shared flag is correct for a CLI: there is
/// a single foreground query).
pub fn interrupt_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    if let Some(flag) = THREAD_INTERRUPT_FLAG.with(|slot| slot.borrow().clone()) {
        return flag;
    }
    static FLAG: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
        std::sync::OnceLock::new();
    FLAG.get_or_init(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .clone()
}

/// Escape a value for interpolation into a single-quoted SQL string literal.
/// Metadata queries interpolate user-controlled table/schema names; a name
/// containing `'` would otherwise break the statement (and is an injection
/// foothold).
pub fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// Quote an identifier with double quotes (SQLite / standard SQL), escaping
/// embedded quotes.
pub fn quote_sql_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Whether an Elasticsearch index name must be double-quoted to be used as a
/// SQL table name (special characters, or a leading digit).
///
/// Lives here rather than in the Elasticsearch driver so completion can ask
/// without depending on the driver.
pub fn elasticsearch_index_needs_quoting(name: &str) -> bool {
    name.contains('-')
        || name.contains('.')
        || name.contains(':')
        || name.contains(' ')
        || name.contains('@')
        || name.contains('#')
        || name.starts_with(char::is_numeric)
}

/// Supported database types
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, strum::EnumIter)]
pub enum DatabaseType {
    PostgreSQL,
    SQLite,
    MySQL,
    ClickHouse,
    MongoDB,
    Elasticsearch,
    WhiteDragon,
    /// Redis and wire-compatible servers (Valkey, KeyDB…): not SQL, queries
    /// are `redis-cli` style command lines.
    Redis,
    /// Microsoft SQL Server and Azure SQL (TDS): T-SQL over SQL authentication.
    SqlServer,
    /// Amazon Redshift: the PostgreSQL wire protocol (simple queries only)
    /// over a PostgreSQL 8-era SQL dialect and its own `SVV_*` catalog.
    Redshift,
    /// libSQL / Turso over the network (Hrana over HTTP): `sqld` and Turso
    /// Cloud. SQLite dialect; local files stay [`DatabaseType::SQLite`].
    LibSql,
    /// InfluxDB time-series servers: v1 (InfluxQL), v2 (Flux) and v3 (SQL)
    /// over HTTP, selected by the URL scheme or `?version=`.
    InfluxDB,
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

impl DatabaseType {
    pub fn supported_types() -> Vec<Self> {
        use strum::IntoEnumIterator;

        Self::iter().collect()
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
            DatabaseType::WhiteDragon => Some(7700),   // HTTP search API
            DatabaseType::Redis => Some(6379),         // RESP over TCP
            DatabaseType::SqlServer => Some(1433),     // TDS
            DatabaseType::Redshift => Some(5439),      // PostgreSQL protocol
            DatabaseType::LibSql => Some(443),         // Hrana over HTTPS (plain: 8080)
            DatabaseType::InfluxDB => Some(8086),      // HTTP API (v3 Core: 8181)
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
            DatabaseType::WhiteDragon => "White Dragon",
            DatabaseType::Redis => "Redis",
            DatabaseType::SqlServer => "SQL Server",
            DatabaseType::Redshift => "Redshift",
            DatabaseType::LibSql => "libSQL",
            DatabaseType::InfluxDB => "InfluxDB",
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
            | DatabaseType::Elasticsearch
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer
            | DatabaseType::Redshift
            | DatabaseType::InfluxDB => true,
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB => false, // File-based, no network connection
            DatabaseType::LibSql => false, // HTTP(S) to a public edge or a local sqld
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
            DatabaseType::WhiteDragon => &["white-dragon", "whitedragon", "wd"],
            DatabaseType::Redis => &["redis", "rediss", "valkey", "valkeys", "redis+unix", "unix"],
            DatabaseType::SqlServer => &["mssql", "sqlserver", "mssql+tls", "sqlserver+tls"],
            DatabaseType::Redshift => &["redshift", "redshift+tls"],
            DatabaseType::LibSql => &["libsql", "libsql+http", "turso"],
            DatabaseType::InfluxDB => &INFLUXDB_SCHEMES,
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
            | DatabaseType::Elasticsearch
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer
            | DatabaseType::Redshift
            | DatabaseType::LibSql
            | DatabaseType::InfluxDB => false,
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
            DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer // SHOWPLAN_XML only
            | DatabaseType::Redshift // text plan only
            | DatabaseType::LibSql // EXPLAIN QUERY PLAN rows
            | DatabaseType::InfluxDB => false, // v3 EXPLAIN text only
        }
    }

    fn requires_authentication(&self) -> bool {
        match self {
            DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MongoDB
            | DatabaseType::Elasticsearch
            | DatabaseType::SqlServer // SQL authentication
            | DatabaseType::Redshift // database user and password
            | DatabaseType::LibSql => true, // the auth token (the password slot)
            DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::InfluxDB => false, // Optional (v1 user/password, v2/v3 token)
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
            DatabaseType::WhiteDragon => "white-dragon",
            DatabaseType::Redis => "redis",
            DatabaseType::SqlServer => "mssql",
            DatabaseType::Redshift => "redshift",
            DatabaseType::LibSql => "libsql",
            DatabaseType::InfluxDB => "influxdb",
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
            DatabaseType::SQLite | DatabaseType::LibSql => &[
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
            DatabaseType::WhiteDragon => &["COUNT", "YEAR", "MONTH", "DAY"],
            // Redis has commands, not SQL functions (the driver lists them)
            DatabaseType::Redis => &[],
            // InfluxQL aggregates and selectors (Flux has its own, see the
            // driver's completion words)
            DatabaseType::InfluxDB => &[
                "COUNT",
                "DISTINCT",
                "INTEGRAL",
                "MEAN",
                "MEDIAN",
                "MODE",
                "SPREAD",
                "STDDEV",
                "SUM",
                "BOTTOM",
                "FIRST",
                "LAST",
                "MAX",
                "MIN",
                "PERCENTILE",
                "SAMPLE",
                "TOP",
                "ABS",
                "CEIL",
                "FLOOR",
                "ROUND",
                "DERIVATIVE",
                "DIFFERENCE",
                "ELAPSED",
                "MOVING_AVERAGE",
                "NON_NEGATIVE_DERIVATIVE",
                "CUMULATIVE_SUM",
                "NOW",
                "TIME",
                "FILL",
            ],
            DatabaseType::SqlServer => &[
                "COALESCE",
                "ISNULL",
                "NULLIF",
                "IIF",
                "GETDATE",
                "GETUTCDATE",
                "SYSDATETIME",
                "SYSDATETIMEOFFSET",
                "DATEADD",
                "DATEDIFF",
                "DATENAME",
                "DATEPART",
                "EOMONTH",
                "FORMAT",
                "CONVERT",
                "TRY_CONVERT",
                "CAST",
                "TRY_CAST",
                "LEN",
                "DATALENGTH",
                "CHARINDEX",
                "PATINDEX",
                "SUBSTRING",
                "STRING_AGG",
                "STRING_SPLIT",
                "CONCAT",
                "CONCAT_WS",
                "TRIM",
                "NEWID",
                "OPENJSON",
                "JSON_VALUE",
                "JSON_QUERY",
                "ROW_NUMBER",
                "DENSE_RANK",
                "OFFSET",
            ],
            DatabaseType::Redshift => &[
                "COALESCE",
                "NVL",
                "NVL2",
                "NULLIF",
                "GREATEST",
                "LEAST",
                "GETDATE",
                "SYSDATE",
                "CURRENT_DATE",
                "CURRENT_TIMESTAMP",
                "DATEADD",
                "DATEDIFF",
                "DATE_PART",
                "DATE_TRUNC",
                "EXTRACT",
                "TO_CHAR",
                "TO_DATE",
                "TO_TIMESTAMP",
                "CONVERT_TIMEZONE",
                "LISTAGG",
                "MEDIAN",
                "APPROXIMATE",
                "SPLIT_PART",
                "REGEXP_SUBSTR",
                "REGEXP_REPLACE",
                "JSON_EXTRACT_PATH_TEXT",
                "JSON_PARSE",
                "IS_VALID_JSON",
                "ROW_NUMBER",
                "RANK",
                "DENSE_RANK",
                "NTILE",
                "LAG",
                "LEAD",
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
            DatabaseType::PostgreSQL
            | DatabaseType::SQLite
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer
            | DatabaseType::Redshift
            | DatabaseType::LibSql
            | DatabaseType::InfluxDB => false,
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
            DatabaseType::WhiteDragon | DatabaseType::Redis => &[],
            DatabaseType::SqlServer => &[], // always `sa`
            DatabaseType::Redshift => &[],  // a managed service, never a container
            DatabaseType::LibSql => &[],    // no user: the credential is a token
            DatabaseType::InfluxDB => &["INFLUXDB_ADMIN_USER", "DOCKER_INFLUXDB_INIT_USERNAME"],
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
            DatabaseType::Redis => &["REDIS_PASSWORD"],
            DatabaseType::SqlServer => &["MSSQL_SA_PASSWORD", "SA_PASSWORD"],
            // v2 images are set up with an admin API token, v1 images with the
            // admin user's password (the v2 login password is not a credential
            // of the API, so it is not listed)
            DatabaseType::InfluxDB => &[
                "DOCKER_INFLUXDB_INIT_ADMIN_TOKEN",
                "INFLUXDB_ADMIN_PASSWORD",
            ],
            DatabaseType::WhiteDragon | DatabaseType::Redshift | DatabaseType::LibSql => &[],
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
            DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer
            | DatabaseType::Redshift
            | DatabaseType::LibSql => &[],
            DatabaseType::InfluxDB => &["INFLUXDB_DB", "DOCKER_INFLUXDB_INIT_BUCKET"],
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
            DatabaseType::WhiteDragon | DatabaseType::Redis | DatabaseType::InfluxDB => "",
            DatabaseType::SqlServer => "sa",
            DatabaseType::Redshift => "awsuser", // the console's default admin user
            DatabaseType::LibSql => "",
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
            "white-dragon" | "whitedragon" | "wd" => Ok(DatabaseType::WhiteDragon),
            "redis" | "rediss" | "valkey" | "valkeys" | "redis+unix" | "unix" => {
                Ok(DatabaseType::Redis)
            }
            "mssql" | "sqlserver" | "mssql+tls" | "sqlserver+tls" => Ok(DatabaseType::SqlServer),
            "redshift" | "redshift+tls" => Ok(DatabaseType::Redshift),
            "libsql" | "libsql+http" | "turso" => Ok(DatabaseType::LibSql),
            scheme if INFLUXDB_SCHEMES.contains(&scheme) => Ok(DatabaseType::InfluxDB),
            "parquet" => Ok(DatabaseType::Parquet),
            "csv" => Ok(DatabaseType::CSV),
            "json" | "ndjson" => Ok(DatabaseType::JSON),
            "duckdb" => Ok(DatabaseType::DuckDB),
            "docker" => Ok(DatabaseType::PostgreSQL), // Default to PostgreSQL for docker:// URLs
            scheme => Err(DatabaseError::UnsupportedScheme(scheme.to_string())),
        }
    }
}

/// Every URL scheme that opens an InfluxDB connection. The scheme selects the
/// API generation and the transport: `influxdb` is v1 (InfluxQL) over HTTP,
/// `influxdb2` v2 (Flux), `influxdb3` v3 (SQL); a trailing `s` or `+https` means
/// TLS.
pub const INFLUXDB_SCHEMES: [&str; 9] = [
    "influxdb",
    "influxdbs",
    "influxdb+https",
    "influxdb2",
    "influxdb2s",
    "influxdb2+https",
    "influxdb3",
    "influxdb3s",
    "influxdb3+https",
];

/// The API generation (1, 2 or 3) a scheme names — `None` for the generic
/// `influxdb`/`influxdbs` spellings, whose version comes from `?version=` (v1
/// by default) — and whether the scheme asks for TLS.
fn influxdb_scheme_parts(scheme: &str) -> Option<(Option<u8>, bool)> {
    Some(match scheme {
        "influxdb" => (None, false),
        "influxdbs" | "influxdb+https" => (None, true),
        "influxdb2" => (Some(2), false),
        "influxdb2s" | "influxdb2+https" => (Some(2), true),
        "influxdb3" => (Some(3), false),
        "influxdb3s" | "influxdb3+https" => (Some(3), true),
        _ => return None,
    })
}

/// The API generation of an InfluxDB connection (`1` when unspecified).
pub fn influxdb_version(options: &HashMap<String, String>) -> u8 {
    options
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("version"))
        .and_then(|(_, value)| value.trim().trim_start_matches(['v', 'V']).parse().ok())
        .filter(|version| (1..=3).contains(version))
        .unwrap_or(1)
}

/// The query language an InfluxDB connection sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfluxLanguage {
    /// InfluxQL: v1 `/query`, v2's 1.x-compatible `/query`, v3 `query_influxql`.
    InfluxQl,
    /// Flux: v2 `/api/v2/query`.
    Flux,
    /// SQL (DataFusion): v3 `query_sql`.
    Sql,
}

impl InfluxLanguage {
    /// The language of a (normalized) connection: the `language` option, else
    /// the generation's native one — InfluxQL for v1, Flux for v2, SQL for v3.
    pub fn from_options(options: &HashMap<String, String>) -> Self {
        let chosen = options
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("language"))
            .map(|(_, value)| value.trim().to_ascii_lowercase());
        match (chosen.as_deref(), influxdb_version(options)) {
            (Some("influxql"), _) => Self::InfluxQl,
            (Some("flux"), 2) => Self::Flux,
            (Some("sql"), 3) => Self::Sql,
            (_, 2) => Self::Flux,
            (_, 3) => Self::Sql,
            _ => Self::InfluxQl,
        }
    }

    /// Lowercase name, as the `language` option spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InfluxQl => "influxql",
            Self::Flux => "flux",
            Self::Sql => "sql",
        }
    }
}

/// Whether an InfluxDB connection skips certificate verification
/// (`verify_certs=false`, the spelling the Elasticsearch connector uses).
pub fn influxdb_skips_certificate_verification(options: &HashMap<String, String>) -> bool {
    options.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("verify_certs")
            && matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "false" | "no" | "off" | "0"
            )
    })
}

/// `redis+unix://` / `unix://`: a Redis server on a unix domain socket.
fn is_redis_unix_scheme(scheme: &str) -> bool {
    matches!(scheme, "redis+unix" | "unix")
}

/// `mssql+tls://` / `sqlserver+tls://`: SQL Server with mandatory encryption
/// spelled in the scheme (the default anyway; kept for symmetry with `rediss`).
fn is_sqlserver_tls_scheme(scheme: &str) -> bool {
    matches!(scheme, "mssql+tls" | "sqlserver+tls")
}

/// A SQL Server instance name or host separated by a backslash cannot ride in
/// a URL authority (`\` is a forbidden host character): percent-encode it in
/// the host part only, so `mssql://sa:pw@host\SQLEXPRESS/db` parses.
pub fn escape_sqlserver_instance_backslash(url_str: &str) -> std::borrow::Cow<'_, str> {
    let Some((scheme, rest)) = url_str.split_once("://") else {
        return url_str.into();
    };
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "mssql" | "sqlserver" | "mssql+tls" | "sqlserver+tls"
    ) {
        return url_str.into();
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    // Only what follows the last `@` is the host: a password may hold `\`
    let (userinfo, host) = match authority.rfind('@') {
        Some(at) => authority.split_at(at + 1),
        None => ("", authority),
    };
    if !host.contains('\\') {
        return url_str.into();
    }
    format!("{scheme}://{userinfo}{}{tail}", host.replace('\\', "%5C")).into()
}

/// How a SQL Server connection is encrypted, from the `encrypt` URL option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlServerEncryption {
    /// `encrypt=false`: only the login exchange is encrypted (the server may
    /// still force more).
    LoginOnly,
    /// `encrypt=true` (default): the whole session over TLS, certificate
    /// verified.
    Required,
    /// `encrypt=strict`: TLS before the TDS handshake (TDS 8.0).
    Strict,
}

impl SqlServerEncryption {
    /// Parse the `encrypt` option value (`None` is the default, `Required`).
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("true") | Some("yes") | Some("on") | Some("1")
            | Some("required") | Some("mandatory") => Ok(Self::Required),
            Some("false") | Some("no") | Some("off") | Some("0") | Some("optional") => {
                Ok(Self::LoginOnly)
            }
            Some("strict") => Ok(Self::Strict),
            Some(other) => Err(format!(
                "unknown encrypt value '{other}' (use true, false or strict)"
            )),
        }
    }

    /// The canonical option value stored in [`ConnectionInfo::options`].
    pub fn as_option(self) -> &'static str {
        match self {
            Self::LoginOnly => "false",
            Self::Required => "true",
            Self::Strict => "strict",
        }
    }
}

/// Option keys (lowercase) that mean "accept any server certificate".
const SQLSERVER_TRUST_CERT_KEYS: [&str; 3] = [
    "trustservercertificate",
    "trust_server_certificate",
    "trustcertificate",
];

/// Whether a SQL Server connection skips certificate verification.
pub fn sqlserver_trusts_any_certificate(options: &HashMap<String, String>) -> bool {
    options.iter().any(|(key, value)| {
        SQLSERVER_TRUST_CERT_KEYS.contains(&key.to_ascii_lowercase().as_str())
            && matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1"
            )
    })
}

/// Connection information parsed from database URL
#[derive(Clone)]
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
    pub use_tls: bool,                    // Use TLS/SSL for the connection
}

/// A neutral starting point for struct-update literals
/// (`ConnectionInfo { host: Some(..), ..Default::default() }`): PostgreSQL, the
/// engine every other default in DBCrust (port, scheme, Docker fallback)
/// assumes, with no host, credentials, path or options and TLS off.
///
/// The engine is a placeholder — callers always set `database_type` when it
/// matters. New fields get their default here instead of in every literal.
impl Default for ConnectionInfo {
    fn default() -> Self {
        Self {
            database_type: DatabaseType::PostgreSQL,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
            use_tls: false,
        }
    }
}

// `Debug` never shows the password or secret-looking option values
impl std::fmt::Debug for ConnectionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionInfo")
            .field("database_type", &self.database_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field(
                "password",
                &crate::password_sanitizer::mask_secret(&self.password),
            )
            .field("database", &self.database)
            .field("file_path", &self.file_path)
            .field(
                "options",
                &crate::password_sanitizer::redact_options(&self.options),
            )
            .field("docker_container", &self.docker_container)
            .field("use_tls", &self.use_tls)
            .finish()
    }
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
        info.supports_roles = info.version_major.is_some_and(|major| major >= 8);
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
///
/// Driver SDK errors (sqlx, DataFusion, …) and the Docker error are carried
/// boxed, so this type does not name any driver crate: each driver converts
/// its own errors with a small `db_err` helper (see [`DatabaseError::driver`]).
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum DatabaseError {
    #[error("Unsupported database URL scheme: {}", crate::password_sanitizer::redact_text(.0))]
    UnsupportedScheme(String),

    #[error("Invalid database URL: {}", crate::password_sanitizer::redact_text(.0))]
    InvalidUrl(String),

    /// A Docker discovery/route failure (the boxed value is the Docker
    /// layer's own error type, which implements `From` for this enum).
    #[error("Docker error: {0}")]
    Docker(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Connection error: {}", crate::password_sanitizer::redact_text(.0))]
    ConnectionError(String),

    #[error("Query error: {}", crate::password_sanitizer::redact_text(.0))]
    QueryError(String),

    #[error("Metadata error: {}", crate::password_sanitizer::redact_text(.0))]
    MetadataError(String),

    #[error("Feature not supported for {database_type}: {feature}")]
    FeatureNotSupported {
        database_type: DatabaseType,
        feature: String,
    },

    /// An error from a driver's own SDK (sqlx, DataFusion, …), boxed.
    ///
    /// The message prefix and redaction are the ones the SDK-specific variants
    /// used to have: sqlx-backed engines render `SQLx error: …` with secrets
    /// redacted, the DataFusion-backed file engines `DataFusion error: …`.
    #[error("{}", driver_error_text(.engine, .source.as_ref()))]
    Driver {
        engine: DatabaseType,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The engine exists in [`DatabaseType`] but this build was compiled
    /// without its connector. `feature` is the cargo feature that provides it.
    #[error(
        "{engine} support is not compiled into this build (enable the `{feature}` cargo feature, or install a full build)"
    )]
    ConnectorNotBuilt {
        engine: DatabaseType,
        feature: &'static str,
    },
}

impl DatabaseError {
    /// Wrap an SDK error raised by the driver for `engine`.
    pub fn driver(
        engine: DatabaseType,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Driver {
            engine,
            source: Box::new(source),
        }
    }
}

/// The user-visible text of [`DatabaseError::Driver`].
fn driver_error_text(engine: &DatabaseType, source: &(dyn std::error::Error + 'static)) -> String {
    match engine {
        DatabaseType::Parquet | DatabaseType::CSV | DatabaseType::JSON | DatabaseType::DuckDB => {
            format!("DataFusion error: {source}")
        }
        _ => format!(
            "SQLx error: {}",
            crate::password_sanitizer::redact_text(&source.to_string())
        ),
    }
}

/// `redshift+tls://`: Redshift with the TLS requirement spelled in the scheme
/// (the default anyway; kept for symmetry with `mssql+tls`).
fn is_redshift_tls_scheme(scheme: &str) -> bool {
    scheme == "redshift+tls"
}

/// The `sslmode` values Redshift accepts (the libpq set), lower-cased.
pub fn redshift_sslmode(mode: &str) -> Option<&'static str> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "disable" => Some("disable"),
        "allow" => Some("allow"),
        "prefer" => Some("prefer"),
        "require" => Some("require"),
        "verify-ca" => Some("verify-ca"),
        "verify-full" => Some("verify-full"),
        _ => None,
    }
}

/// Redshift specifics of a parsed URL: TLS is required unless `sslmode` says
/// otherwise (the generic PostgreSQL default, `prefer`, would send the
/// password in clear to a cluster that does not enforce `require_ssl`), and
/// the canonical `sslmode` is always recorded so sessions and the secret-store
/// binding keep it.
fn normalize_redshift_connection(
    info: &mut ConnectionInfo,
    scheme: &str,
) -> Result<(), DatabaseError> {
    let key = info
        .options
        .keys()
        .find(|key| key.eq_ignore_ascii_case("sslmode"))
        .cloned();
    let mode = match key {
        Some(key) => {
            let raw = info.options.remove(&key).unwrap_or_default();
            redshift_sslmode(&raw).ok_or_else(|| {
                DatabaseError::InvalidUrl(format!(
                    "unknown sslmode '{raw}' (use disable, allow, prefer, require, verify-ca or verify-full)"
                ))
            })?
        }
        None => "require",
    };
    if is_redshift_tls_scheme(scheme) && matches!(mode, "disable" | "allow" | "prefer") {
        return Err(DatabaseError::InvalidUrl(format!(
            "redshift+tls:// requires TLS: sslmode={mode} contradicts it"
        )));
    }
    info.use_tls = matches!(mode, "require" | "verify-ca" | "verify-full");
    info.options.insert("sslmode".to_string(), mode.to_string());
    Ok(())
}

/// Default port of a libSQL endpoint over TLS (Turso Cloud, any reverse proxy).
pub const LIBSQL_TLS_PORT: u16 = 443;
/// Default port of a plain-HTTP `sqld`.
pub const LIBSQL_PLAIN_PORT: u16 = 8080;

/// Whether `host` is this machine (plain HTTP to it never crosses a network).
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// libSQL / Turso specifics of a parsed URL. The auth token is a secret and
/// lives in the password slot, so the secret store, the target binding and the
/// redaction all treat it like any other password: it can come as the URL's
/// password (`libsql://:TOKEN@host`) or as `?authToken=` and is moved there.
/// TLS is the default; plain HTTP (`libsql+http://` or `?tls=false`) is for a
/// local `sqld`, and for another host only with an explicit `tls=false`.
fn normalize_libsql_connection(
    info: &mut ConnectionInfo,
    scheme: &str,
) -> Result<(), DatabaseError> {
    let Some(host) = info.host.clone() else {
        return Err(DatabaseError::InvalidUrl(
            "a libSQL URL needs a host (libsql://my-db-org.turso.io)".to_string(),
        ));
    };

    // The token: userinfo password or ?authToken= (never both, never kept as an option)
    let token_keys: Vec<String> = info
        .options
        .keys()
        .filter(|key| {
            let key: String = key
                .chars()
                .filter(|c| !matches!(c, '_' | '-'))
                .flat_map(char::to_lowercase)
                .collect();
            key == "authtoken"
        })
        .cloned()
        .collect();
    for key in token_keys {
        let token = info.options.remove(&key).unwrap_or_default();
        if token.is_empty() {
            continue;
        }
        match &info.password {
            Some(existing) if existing != &token => {
                return Err(DatabaseError::InvalidUrl(
                    "the libSQL auth token is given twice with different values".to_string(),
                ));
            }
            _ => info.password = Some(token),
        }
    }
    if info.password.as_deref() == Some("") {
        info.password = None;
    }

    // Namespace (sqld multi-tenant): the path, or ?namespace=
    if let Some(key) = info
        .options
        .keys()
        .find(|key| key.eq_ignore_ascii_case("namespace"))
        .cloned()
    {
        let namespace = info.options.remove(&key).unwrap_or_default();
        match (&info.database, namespace.is_empty()) {
            (_, true) => {}
            (Some(path), false) if path != &namespace => {
                return Err(DatabaseError::InvalidUrl(
                    "the libSQL namespace is given twice with different names".to_string(),
                ));
            }
            _ => info.database = Some(namespace),
        }
    }

    // Transport: TLS unless the scheme or an explicit tls=false says plain
    let tls_key = info
        .options
        .keys()
        .find(|key| key.eq_ignore_ascii_case("tls"))
        .cloned();
    let tls_option = match tls_key {
        Some(key) => {
            let value = info.options.remove(&key).unwrap_or_default();
            match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                other => {
                    return Err(DatabaseError::InvalidUrl(format!(
                        "unknown tls value '{other}' (use true or false)"
                    )));
                }
            }
        }
        None => None,
    };
    let plain_scheme = scheme == "libsql+http";
    if plain_scheme && tls_option == Some(true) {
        return Err(DatabaseError::InvalidUrl(
            "libsql+http:// is plain HTTP: it contradicts tls=true (use libsql://)".to_string(),
        ));
    }
    let plain = plain_scheme || tls_option == Some(false);
    if plain && !is_loopback_host(&host) && tls_option != Some(false) {
        return Err(DatabaseError::InvalidUrl(format!(
            "plain HTTP would send the auth token and every query to {host} in clear text: use libsql:// (TLS), or add ?tls=false to confirm"
        )));
    }
    info.use_tls = !plain;
    info.options.insert("tls".to_string(), (!plain).to_string());
    if info.port.is_none() {
        info.port = Some(if plain {
            LIBSQL_PLAIN_PORT
        } else {
            LIBSQL_TLS_PORT
        });
    }
    Ok(())
}

/// SQL Server specifics of a parsed URL: the `host\instance` split, the
/// canonical `encrypt` option (which also decides `use_tls`) and the
/// `trustServerCertificate` opt-in.
fn normalize_sqlserver_connection(
    info: &mut ConnectionInfo,
    scheme: &str,
) -> Result<(), DatabaseError> {
    // Named instance: `host%5CINSTANCE` in the authority or `?instance=`
    if let Some(host) = info.host.take() {
        let host = percent_encoding::percent_decode_str(&host)
            .decode_utf8_lossy()
            .into_owned();
        match host.split_once('\\') {
            Some((name, instance)) if !name.is_empty() && !instance.is_empty() => {
                info.options
                    .insert("instance".to_string(), instance.to_string());
                info.host = Some(name.to_string());
            }
            Some(_) => {
                return Err(DatabaseError::InvalidUrl(
                    "a SQL Server instance needs a host and a name (host\\INSTANCE)".to_string(),
                ));
            }
            None => info.host = Some(host),
        }
    }
    let instance_key = info
        .options
        .keys()
        .find(|key| {
            key.eq_ignore_ascii_case("instance") || key.eq_ignore_ascii_case("instancename")
        })
        .cloned();
    if let Some(key) = instance_key {
        let instance = info.options.remove(&key).unwrap_or_default();
        if instance.trim().is_empty() {
            return Err(DatabaseError::InvalidUrl(
                "the SQL Server instance name is empty".to_string(),
            ));
        }
        if let Some(previous) = info
            .options
            .insert("instance".to_string(), instance.clone())
            && previous != instance
        {
            return Err(DatabaseError::InvalidUrl(
                "the SQL Server instance is given twice with different names".to_string(),
            ));
        }
    }
    if info.options.contains_key("instance") && info.port.is_some() {
        return Err(DatabaseError::InvalidUrl(
            "a SQL Server named instance is resolved by SQL Browser (UDP 1434): give the instance or the port, not both"
                .to_string(),
        ));
    }

    // Encryption: the scheme, then `encrypt`, then the default (on)
    let encrypt_key = info
        .options
        .keys()
        .find(|key| key.eq_ignore_ascii_case("encrypt"))
        .cloned();
    let encrypt_value = encrypt_key.and_then(|key| info.options.remove(&key));
    let encryption =
        SqlServerEncryption::parse(encrypt_value.as_deref()).map_err(DatabaseError::InvalidUrl)?;
    if is_sqlserver_tls_scheme(scheme) && encryption == SqlServerEncryption::LoginOnly {
        return Err(DatabaseError::InvalidUrl(
            "encrypt=false contradicts the +tls scheme".to_string(),
        ));
    }
    info.options
        .insert("encrypt".to_string(), encryption.as_option().to_string());
    info.use_tls = encryption != SqlServerEncryption::LoginOnly;

    // The opt-in to skip certificate verification, under one canonical key
    let trust = sqlserver_trusts_any_certificate(&info.options);
    info.options
        .retain(|key, _| !SQLSERVER_TRUST_CERT_KEYS.contains(&key.to_ascii_lowercase().as_str()));
    if trust {
        info.options
            .insert("trustServerCertificate".to_string(), "true".to_string());
    }
    Ok(())
}

/// InfluxDB specifics of a parsed URL: the API generation and transport from
/// the scheme or `?version=`, the bucket/database (path, `?bucket=`, `?db=`),
/// and the API token — moved into the one secret slot (`password`) so the
/// session, recent-connection and secret-store plumbing treats it exactly like
/// a password and no option ever holds it.
fn normalize_influxdb_connection(
    info: &mut ConnectionInfo,
    scheme: &str,
) -> Result<(), DatabaseError> {
    let (scheme_version, scheme_tls) = influxdb_scheme_parts(scheme)
        .ok_or_else(|| DatabaseError::UnsupportedScheme(scheme.to_string()))?;

    let take_option = |info: &mut ConnectionInfo, names: &[&str]| -> Vec<(String, String)> {
        let keys: Vec<String> = info
            .options
            .keys()
            .filter(|key| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| info.options.remove(&key).map(|value| (key, value)))
            .collect()
    };

    // API generation: a versioned scheme is explicit, `?version=` may refine
    // only the generic one
    let given = take_option(info, &["version"]);
    let mut option_version = None;
    for (_, value) in &given {
        let parsed = value
            .trim()
            .trim_start_matches(['v', 'V'])
            .trim_end_matches(".x")
            .parse::<u8>()
            .ok()
            .filter(|version| (1..=3).contains(version))
            .ok_or_else(|| {
                DatabaseError::InvalidUrl(format!(
                    "unknown InfluxDB version '{value}' (use 1, 2 or 3)"
                ))
            })?;
        option_version = Some(parsed);
    }
    let version = match (scheme_version, option_version) {
        (Some(from_scheme), Some(from_option)) if from_scheme != from_option => {
            return Err(DatabaseError::InvalidUrl(format!(
                "the {scheme}:// scheme means InfluxDB v{from_scheme} but version={from_option} was given"
            )));
        }
        (Some(version), _) | (None, Some(version)) => version,
        (None, None) => 1,
    };
    info.options
        .insert("version".to_string(), version.to_string());

    // v3 listens on 8181 where v1/v2 use 8086: make it explicit, so sessions
    // and the secret-store binding (which fall back to the engine's default
    // port) name the port actually used
    if version == 3 && info.port.is_none() {
        info.port = Some(8181);
    }

    // Transport: the scheme (`s`, `+https`), then the generic ssl/tls options
    // the parser already folded into `use_tls`
    if scheme_tls {
        let contradicts = info.options.iter().any(|(key, value)| {
            matches!(key.to_ascii_lowercase().as_str(), "ssl" | "tls" | "secure")
                && matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "false" | "no" | "off" | "0"
                )
        });
        if contradicts {
            return Err(DatabaseError::InvalidUrl(format!(
                "ssl=false contradicts the {scheme}:// scheme"
            )));
        }
        info.use_tls = true;
    }
    for key in ["ssl", "secure", "sslmode"] {
        let keys: Vec<String> = info
            .options
            .keys()
            .filter(|name| name.eq_ignore_ascii_case(key))
            .cloned()
            .collect();
        for name in keys {
            info.options.remove(&name);
        }
    }
    let _ = take_option(info, &["tls"]);
    if info.use_tls {
        info.options.insert("tls".to_string(), "true".to_string());
    }

    // Bucket / database: the path, or an option, not both
    let named = take_option(info, &["bucket", "db", "database"]);
    for (key, value) in named {
        match &info.database {
            Some(path) if *path != value => {
                return Err(DatabaseError::InvalidUrl(format!(
                    "the InfluxDB database is given twice with different names (path and {key}=)"
                )));
            }
            _ => info.database = Some(value),
        }
    }

    // The API token is a secret: it lives in the password slot, never in options
    let tokens = take_option(info, &["token", "influx_token"]);
    for (_, token) in tokens {
        if token.is_empty() {
            continue;
        }
        match &info.password {
            Some(existing) if *existing != token => {
                return Err(DatabaseError::InvalidUrl(
                    "the InfluxDB token is given twice with different values".to_string(),
                ));
            }
            _ => info.password = Some(token),
        }
    }

    // Query language: the generation's native one unless asked otherwise
    let languages = take_option(info, &["language", "lang", "dialect"]);
    if let Some((_, value)) = languages.into_iter().next_back() {
        let language = value.trim().to_ascii_lowercase();
        let allowed: &[&str] = match version {
            1 => &["influxql"],
            2 => &["flux", "influxql"],
            _ => &["sql", "influxql"],
        };
        if !allowed.contains(&language.as_str()) {
            return Err(DatabaseError::InvalidUrl(format!(
                "InfluxDB v{version} cannot query in '{language}' (use {})",
                allowed.join(" or ")
            )));
        }
        info.options.insert("language".to_string(), language);
    }

    // One canonical spelling of the organization
    if let Some((_, org)) = take_option(info, &["org"]).into_iter().next_back() {
        info.options.insert("org".to_string(), org);
    }
    Ok(())
}

impl ConnectionInfo {
    /// Parse a database URL into connection information
    pub fn parse_url(url_str: &str) -> Result<Self, DatabaseError> {
        debug!(
            "[ConnectionInfo::parse_url] Parsing URL: {}",
            crate::password_sanitizer::sanitize_connection_url(url_str)
        );

        let url_str = escape_sqlserver_instance_backslash(url_str);
        let url_str = url_str.as_ref();
        let url = Url::parse(url_str)
            .map_err(|e| DatabaseError::InvalidUrl(format!("Failed to parse URL: {e}")))?;

        let database_type = DatabaseType::from_scheme(url.scheme())?;

        let mut connection_info = ConnectionInfo {
            database_type: database_type.clone(),
            ..Default::default()
        };

        // Handle Docker URLs first
        if url.scheme() == "docker" {
            // Parse Docker URL using our custom parser
            if let Some(target) = crate::docker_target::DockerTarget::parse(url_str) {
                connection_info.docker_container = Some(target.container);
                connection_info.username = target.user;
                connection_info.password = target.password;
                connection_info.database = target.database;
                // Database type will be determined later after container inspection
            } else {
                return Err(DatabaseError::InvalidUrl(
                    "Invalid Docker URL format".to_string(),
                ));
            }
            return Ok(connection_info);
        }

        // Parse database-specific connection details
        if is_redis_unix_scheme(url.scheme()) {
            // redis+unix:///run/redis.sock?db=2&user=app&pass=secret — the
            // authority is empty (credentials cannot ride in the userinfo), the
            // path is the socket, the logical database is `?db=`
            let socket = percent_encoding::percent_decode_str(url.path())
                .decode_utf8_lossy()
                .into_owned();
            if socket.is_empty() || socket == "/" {
                return Err(DatabaseError::InvalidUrl(
                    "a Redis unix socket URL needs the socket path (redis+unix:///path/to/redis.sock)"
                        .to_string(),
                ));
            }
            connection_info.file_path = Some(socket);
            for (key, value) in url.query_pairs() {
                match key.to_ascii_lowercase().as_str() {
                    "db" | "database" => connection_info.database = Some(value.into_owned()),
                    "user" | "username" => connection_info.username = Some(value.into_owned()),
                    "pass" | "password" => connection_info.password = Some(value.into_owned()),
                    _ => {
                        connection_info
                            .options
                            .insert(key.to_string(), value.to_string());
                    }
                }
            }
            return Ok(connection_info);
        } else if database_type.is_file_based() {
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
            } else {
                // sqlite:///absolute/path — a single leading slash is an
                // absolute path (URI semantics, like file://), regardless of
                // which directory it points into. Use sqlite://./path for
                // relative paths.
                path.to_string()
            };

            // The url crate percent-encodes the path (e.g. spaces become %20)
            let file_path = percent_encoding::percent_decode_str(&file_path)
                .decode_utf8_lossy()
                .into_owned();

            connection_info.file_path = Some(file_path);
        } else {
            // For network databases
            connection_info.host = url.host_str().map(|h| h.to_string());
            connection_info.port = url.port();
            // url's accessors return the percent-encoded serialization, so a
            // password typed as p%40ss (for p@ss) would otherwise be sent
            // verbatim to the server — decode like the database name below.
            connection_info.username = if url.username().is_empty() {
                None
            } else {
                Some(
                    percent_encoding::percent_decode_str(url.username())
                        .decode_utf8_lossy()
                        .into_owned(),
                )
            };
            connection_info.password = url.password().map(|p| {
                percent_encoding::percent_decode_str(p)
                    .decode_utf8_lossy()
                    .into_owned()
            });

            // Database name is the path without leading slash - URL decode it
            if let Some(mut segments) = url.path_segments() {
                if let Some(db_name) = segments.next() {
                    if !db_name.is_empty() {
                        // URL-decode the database name to handle special characters like %3A (colon)
                        let decoded_db_name = percent_encoding::percent_decode_str(db_name)
                            .decode_utf8()
                            .map_err(|e| {
                                DatabaseError::InvalidUrl(format!(
                                    "Failed to decode database name '{db_name}': {e}"
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

        // rediss:// and valkeys:// carry the TLS request in the scheme
        // (recorded as the `tls` option too, so everything that keeps options
        // — sessions, recent connections, the secret-store binding — keeps TLS)
        if matches!(url.scheme(), "rediss" | "valkeys") {
            connection_info.use_tls = true;
            connection_info
                .options
                .entry("tls".to_string())
                .or_insert_with(|| "true".to_string());
        }

        // Detect TLS from query parameters (ssl, sslmode, tls, secure)
        let mut tls_option_seen = false;
        let mut tls_requested_by_option = false;
        for (key, value) in &connection_info.options {
            let key = key.to_ascii_lowercase();
            let value = value.to_ascii_lowercase();

            match key.as_str() {
                "ssl" | "tls" | "secure" => {
                    tls_option_seen = true;
                    if matches!(value.as_str(), "true" | "1" | "yes" | "on") {
                        tls_requested_by_option = true;
                    }
                }
                "sslmode" => {
                    tls_option_seen = true;
                    if matches!(value.as_str(), "require" | "verify-ca" | "verify-full") {
                        tls_requested_by_option = true;
                    }
                }
                _ => {}
            }
        }

        if tls_requested_by_option {
            connection_info.use_tls = true;
        }

        if database_type == DatabaseType::SqlServer {
            normalize_sqlserver_connection(&mut connection_info, url.scheme())?;
        }
        if database_type == DatabaseType::Redshift {
            normalize_redshift_connection(&mut connection_info, url.scheme())?;
        }
        if database_type == DatabaseType::LibSql {
            normalize_libsql_connection(&mut connection_info, url.scheme())?;
        }
        if database_type == DatabaseType::InfluxDB {
            normalize_influxdb_connection(&mut connection_info, url.scheme())?;
        }

        // Auto-enable TLS for ClickHouse on the HTTPS HTTP interface port unless
        // the URL explicitly configured TLS itself (e.g. ssl=false).
        // Note: 9440 is ClickHouse's native TLS port for clickhouse-client, not HTTPS.
        if connection_info.database_type == DatabaseType::ClickHouse
            && connection_info.port == Some(8443)
            && !tls_option_seen
        {
            connection_info.use_tls = true;
        }

        debug!(
            "[ConnectionInfo::parse_url] Parsed connection info for {}",
            database_type
        );
        Ok(connection_info)
    }

    /// `to_url` for Redis: the scheme carries TLS (`rediss`), the path the
    /// logical database index, and a unix socket keeps its `?db=`.
    fn redis_url(&self) -> String {
        if let Some(socket) = &self.file_path {
            let mut url = format!("redis+unix://{socket}");
            if let Some(db) = &self.database {
                url.push_str(&format!("?db={db}"));
            }
            return url;
        }
        let mut url = String::from(if self.use_tls {
            "rediss://"
        } else {
            "redis://"
        });
        if let Some(username) = &self.username {
            url.push_str(username);
            url.push('@');
        }
        if let Some(host) = &self.host {
            // IPv6 literals need brackets back
            if host.contains(':') && !host.starts_with('[') {
                url.push_str(&format!("[{host}]"));
            } else {
                url.push_str(host);
            }
            if let Some(port) = self.port {
                url.push_str(&format!(":{port}"));
            }
        }
        if let Some(database) = &self.database {
            url.push('/');
            url.push_str(database);
        }
        if let Some(container) = &self.docker_container {
            url.push_str(&format!(" # Docker: {container}"));
        }
        url
    }

    /// `to_url` for SQL Server: the non-default parts of the security
    /// posture (`encrypt`, `trustServerCertificate`) and the named instance
    /// stay visible; the password never appears.
    fn sqlserver_url(&self) -> String {
        let mut url = String::from("mssql://");
        if let Some(username) = &self.username {
            url.push_str(username);
            url.push('@');
        }
        if let Some(host) = &self.host {
            url.push_str(host);
            if let Some(port) = self.port {
                url.push_str(&format!(":{port}"));
            }
        }
        if let Some(database) = &self.database {
            url.push('/');
            url.push_str(database);
        }
        let mut query: Vec<String> = Vec::new();
        if let Some(instance) = self.options.get("instance") {
            query.push(format!("instance={instance}"));
        }
        match self.options.get("encrypt").map(String::as_str) {
            Some(mode @ ("false" | "strict")) => query.push(format!("encrypt={mode}")),
            _ if !self.use_tls => query.push("encrypt=false".to_string()),
            _ => {}
        }
        if sqlserver_trusts_any_certificate(&self.options) {
            query.push("trustServerCertificate=true".to_string());
        }
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }
        if let Some(container) = &self.docker_container {
            url.push_str(&format!(" # Docker: {container}"));
        }
        url
    }

    /// `to_url` for Redshift: the TLS posture (`sslmode`) stays visible when it
    /// is not the default `require`; the password never appears.
    fn redshift_url(&self) -> String {
        let mut url = String::from("redshift://");
        if let Some(username) = &self.username {
            url.push_str(username);
            url.push('@');
        }
        if let Some(host) = &self.host {
            url.push_str(host);
            if let Some(port) = self.port {
                url.push_str(&format!(":{port}"));
            }
        }
        if let Some(database) = &self.database {
            url.push('/');
            url.push_str(database);
        }
        match self
            .options
            .get("sslmode")
            .and_then(|mode| redshift_sslmode(mode))
        {
            Some("require") => {}
            Some(mode) => url.push_str(&format!("?sslmode={mode}")),
            None if !self.use_tls => url.push_str("?sslmode=disable"),
            None => {}
        }
        if let Some(container) = &self.docker_container {
            url.push_str(&format!(" # Docker: {container}"));
        }
        url
    }

    /// `to_url` for libSQL: the transport (`libsql+http` for plain) and the
    /// namespace stay visible, the auth token never does.
    fn libsql_url(&self) -> String {
        let host = self.host.as_deref().unwrap_or_default();
        let mut url = String::from(if self.use_tls {
            "libsql://"
        } else {
            "libsql+http://"
        });
        if host.contains(':') && !host.starts_with('[') {
            url.push_str(&format!("[{host}]"));
        } else {
            url.push_str(host);
        }
        let default_port = if self.use_tls {
            LIBSQL_TLS_PORT
        } else {
            LIBSQL_PLAIN_PORT
        };
        if let Some(port) = self.port.filter(|port| *port != default_port) {
            url.push_str(&format!(":{port}"));
        }
        if let Some(namespace) = &self.database {
            url.push('/');
            url.push_str(namespace);
        }
        // Plain HTTP to another host needs its explicit confirmation to parse back
        if !self.use_tls && !is_loopback_host(host) {
            url.push_str("?tls=false");
        }
        if let Some(container) = &self.docker_container {
            url.push_str(&format!(" # Docker: {container}"));
        }
        url
    }

    /// `to_url` for InfluxDB: the scheme names the API generation and TLS, the
    /// path the bucket/database; `org`, a non-default language and a disabled
    /// certificate check stay visible. Neither the password nor the token
    /// ever appears.
    fn influxdb_url(&self) -> String {
        let version = influxdb_version(&self.options);
        let scheme = match (version, self.use_tls) {
            (2, false) => "influxdb2",
            (2, true) => "influxdb2s",
            (3, false) => "influxdb3",
            (3, true) => "influxdb3s",
            (_, false) => "influxdb",
            (_, true) => "influxdbs",
        };
        let mut url = format!("{scheme}://");
        if let Some(username) = &self.username {
            url.push_str(username);
            url.push('@');
        }
        if let Some(host) = &self.host {
            // IPv6 literals need brackets back
            if host.contains(':') && !host.starts_with('[') {
                url.push_str(&format!("[{host}]"));
            } else {
                url.push_str(host);
            }
            if let Some(port) = self.port {
                url.push_str(&format!(":{port}"));
            }
        }
        if let Some(database) = &self.database {
            url.push('/');
            url.push_str(
                &percent_encoding::utf8_percent_encode(
                    database,
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string(),
            );
        }
        let mut query: Vec<String> = Vec::new();
        for key in ["org", "rp"] {
            if let Some(value) = self.options.get(key) {
                query.push(format!(
                    "{key}={}",
                    percent_encoding::utf8_percent_encode(
                        value,
                        percent_encoding::NON_ALPHANUMERIC
                    )
                ));
            }
        }
        if let Some(language) = self.options.get("language") {
            query.push(format!("language={language}"));
        }
        if influxdb_skips_certificate_verification(&self.options) {
            query.push("verify_certs=false".to_string());
        }
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }
        if let Some(container) = &self.docker_container {
            url.push_str(&format!(" # Docker: {container}"));
        }
        url
    }

    /// Check if SSH tunneling is applicable for this database type
    pub fn supports_ssh_tunnel(&self) -> bool {
        // A Redis unix socket is local: nothing to tunnel
        self.database_type.supports_ssh_tunnel() && self.file_path.is_none()
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
                && self.use_tls == other.use_tls
        }
    }

    /// Build a complete connection URL from connection information
    /// This is useful for storing in connection history
    pub fn to_url(&self) -> String {
        if self.database_type == DatabaseType::Redis {
            return self.redis_url();
        }
        if self.database_type == DatabaseType::SqlServer {
            return self.sqlserver_url();
        }
        if self.database_type == DatabaseType::Redshift {
            return self.redshift_url();
        }
        if self.database_type == DatabaseType::LibSql {
            return self.libsql_url();
        }
        if self.database_type == DatabaseType::InfluxDB {
            return self.influxdb_url();
        }
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

            // Add TLS query parameter if enabled
            if self.use_tls {
                url.push_str("?ssl=true");
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
    ) -> Result<crate::model::TableDetails, DatabaseError>;

    /// Backend-native search qualifier names, when the backend has a query
    /// language in addition to SQL. SQL-only providers use the empty default.
    async fn get_search_qualifiers(&self) -> Result<Vec<String>, DatabaseError> {
        Ok(Vec::new())
    }

    /// What kind of object each of `names` (as returned by
    /// [`MetadataProvider::get_tables`]) is, for engines whose "tables" are
    /// not all alike (Redis key types). Names missing from the answer are
    /// plain tables; the default describes none.
    async fn get_table_kinds(
        &self,
        _names: &[String],
    ) -> Result<HashMap<String, String>, DatabaseError> {
        Ok(HashMap::new())
    }

    /// Check if a query can be explained
    fn supports_explain(&self) -> bool;

    /// Get the default schema/database name for this database type
    fn default_schema(&self) -> Option<String>;
}

/// Query results with per-cell SQL NULL-ness preserved.
///
/// `rows[i][j] == None` means the cell is NULL, as opposed to `Some("")`
/// (an actual empty string). The legacy `Vec<Vec<String>>` shape flattens
/// both to the backend's historical sentinel and cannot tell them apart.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StructuredQueryResult {
    /// Column names (the legacy shape's header row).
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl StructuredQueryResult {
    /// Wrap legacy display rows (header first) with no NULL information.
    pub fn from_display_rows(display_rows: Vec<Vec<String>>) -> Self {
        let mut rows = display_rows.into_iter();
        let columns = rows.next().unwrap_or_default();
        Self {
            columns,
            rows: rows
                .map(|row| row.into_iter().map(Some).collect())
                .collect(),
        }
    }

    /// Legacy display rows: header first, NULL cells rendered as `null_text`
    /// (each backend's historical encoding, so CLI output stays identical).
    pub fn into_display_rows(self, null_text: &str) -> Vec<Vec<String>> {
        let mut display = Vec::with_capacity(self.rows.len() + 1);
        display.push(self.columns);
        display.extend(self.rows.into_iter().map(|row| {
            row.into_iter()
                .map(|cell| cell.unwrap_or_else(|| null_text.to_string()))
                .collect()
        }));
        display
    }
}

/// Trait for executing database queries and managing connections
#[async_trait]
pub trait DatabaseClient: Send + Sync {
    /// Execute a query and return results as Vec<Vec<String>>
    async fn execute_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// Execute a query preserving per-cell NULL-ness.
    ///
    /// The default falls back to [`DatabaseClient::execute_query`] with no
    /// NULL information (every cell `Some`); backends override it so NULL
    /// and empty string stay distinguishable for structured consumers (GUI).
    async fn execute_query_structured(
        &self,
        sql: &str,
    ) -> Result<StructuredQueryResult, DatabaseError> {
        Ok(StructuredQueryResult::from_display_rows(
            self.execute_query(sql).await?,
        ))
    }

    /// Test query execution without side effects (for validation)
    async fn test_query(&self, sql: &str) -> Result<(), DatabaseError>;

    /// Execute a query with EXPLAIN prefix
    async fn explain_query(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// Execute a query with EXPLAIN prefix and return raw output (unformatted)
    async fn explain_query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>, DatabaseError>;

    /// `EXPLAIN ANALYZE` with raw, machine-readable output (PostgreSQL: the
    /// JSON plan with actual timings and buffers). This EXECUTES the
    /// statement: callers must only pass statements they consider safe to
    /// run. Unsupported by default.
    async fn explain_analyze_query_raw(
        &self,
        _sql: &str,
    ) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(DatabaseError::FeatureNotSupported {
            database_type: self.get_connection_info().database_type.clone(),
            feature: "EXPLAIN ANALYZE".to_string(),
        })
    }

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

    #[test]
    fn test_connector_not_built_names_the_engine() {
        let error = DatabaseError::ConnectorNotBuilt {
            engine: DatabaseType::MongoDB,
            feature: "mongodb",
        };
        assert_eq!(
            error.to_string(),
            "MongoDB support is not compiled into this build (enable the `mongodb` cargo feature, or install a full build)"
        );
    }

    #[test]
    fn test_connection_info_default_is_neutral() {
        let info = ConnectionInfo::default();
        assert_eq!(info.database_type, DatabaseType::PostgreSQL);
        assert!(info.host.is_none() && info.port.is_none());
        assert!(info.username.is_none() && info.password.is_none());
        assert!(info.database.is_none() && info.file_path.is_none());
        assert!(info.options.is_empty());
        assert!(info.docker_container.is_none());
        assert!(!info.use_tls);
    }

    #[test]
    fn test_thread_interrupt_flag_is_scoped_to_its_thread() {
        let global = interrupt_flag();
        let scoped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_flag = scoped.clone();
        std::thread::spawn(move || {
            set_thread_interrupt_flag(Some(worker_flag.clone()));
            assert!(std::sync::Arc::ptr_eq(&interrupt_flag(), &worker_flag));
            set_thread_interrupt_flag(None);
            assert!(!std::sync::Arc::ptr_eq(&interrupt_flag(), &worker_flag));
        })
        .join()
        .unwrap();
        // Other threads keep seeing the process-wide flag
        assert!(std::sync::Arc::ptr_eq(&interrupt_flag(), &global));
        assert!(!std::sync::Arc::ptr_eq(&interrupt_flag(), &scoped));
    }

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
            ..Default::default()
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
            docker_container: Some("myapp-postgres".to_string()),
            ..Default::default()
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
            ..Default::default()
        };

        let url = conn_info.to_url();
        assert_eq!(url, "mysql://root@localhost:3306/testdb");
    }

    #[rstest]
    fn test_connection_info_to_url_sqlite() {
        // Test SQLite connection
        let conn_info = ConnectionInfo {
            database_type: DatabaseType::SQLite,
            file_path: Some("/path/to/database.db".to_string()),
            ..Default::default()
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
    #[case(
        "white-dragon://search.example:7700",
        DatabaseType::WhiteDragon,
        Some("search.example"),
        Some(7700),
        None,
        None
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
    #[case("white-dragon://localhost", true)]
    fn test_ssh_tunnel_support(#[case] url: &str, #[case] expected: bool) {
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(conn_info.supports_ssh_tunnel(), expected);
    }

    #[rstest]
    #[case("postgres://localhost/db", Some(5432))]
    #[case("mysql://localhost/db", Some(3306))]
    #[case("sqlite:///path/to/db", None)]
    #[case("white-dragon://localhost", Some(7700))]
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

    #[rstest]
    #[case("clickhouse://user@host:8123/mydb", false)]
    #[case("clickhouse://user@host:8443/mydb", true)] // Auto TLS on HTTPS HTTP port
    #[case("clickhouse://user@host:8443/mydb?ssl=false", false)] // Explicit override
    #[case("clickhouse://user@host:9440/mydb", false)] // Native TLS port, not HTTPS
    #[case("clickhouse://user@host:9440/mydb?ssl=true", true)] // Explicit custom HTTPS attempt
    #[case("clickhouse://user@host:8123/mydb?ssl=true", true)]
    #[case("clickhouse://user@host:8123/mydb?sslmode=require", true)]
    #[case("clickhouse://user@host:8123/mydb?tls=true", true)]
    #[case("clickhouse://user@host:8123/mydb?secure=true", true)]
    fn test_clickhouse_tls_detection(#[case] url: &str, #[case] expected_tls: bool) {
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(conn_info.database_type, DatabaseType::ClickHouse);
        assert_eq!(conn_info.use_tls, expected_tls);
    }

    #[rstest]
    #[case("white-dragon://localhost:7700", false)]
    #[case("whitedragon://localhost:7700?tls=true", true)]
    #[case("wd://localhost:443?sslmode=require", true)]
    fn test_white_dragon_url_aliases_and_tls(#[case] url: &str, #[case] expected_tls: bool) {
        let conn_info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(conn_info.database_type, DatabaseType::WhiteDragon);
        assert_eq!(conn_info.use_tls, expected_tls);
    }

    #[rstest]
    #[case("redis://localhost", "localhost", None, None, None, None, false)]
    #[case(
        "redis://localhost:6380/3",
        "localhost",
        Some(6380),
        None,
        None,
        Some("3"),
        false
    )]
    #[case(
        "redis://:s3cret@cache.example/0",
        "cache.example",
        None,
        None,
        Some("s3cret"),
        Some("0"),
        false
    )]
    #[case(
        "rediss://app:p%40ss@cache.example:6390/2",
        "cache.example",
        Some(6390),
        Some("app"),
        Some("p@ss"),
        Some("2"),
        true
    )]
    #[case("valkey://localhost", "localhost", None, None, None, None, false)]
    #[case("valkeys://localhost", "localhost", None, None, None, None, true)]
    #[case(
        "redis://localhost?tls=true",
        "localhost",
        None,
        None,
        None,
        None,
        true
    )]
    #[case(
        "redis://[::1]:6379/1",
        "[::1]",
        Some(6379),
        None,
        None,
        Some("1"),
        false
    )]
    fn test_redis_url_parsing(
        #[case] url: &str,
        #[case] host: &str,
        #[case] port: Option<u16>,
        #[case] user: Option<&str>,
        #[case] password: Option<&str>,
        #[case] database: Option<&str>,
        #[case] tls: bool,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::Redis);
        assert_eq!(info.host.as_deref(), Some(host));
        assert_eq!(info.port, port);
        assert_eq!(info.username.as_deref(), user);
        assert_eq!(info.password.as_deref(), password);
        assert_eq!(info.database.as_deref(), database);
        assert_eq!(info.use_tls, tls);
        assert_eq!(info.default_port(), Some(6379));
        assert!(info.file_path.is_none());
        assert!(info.supports_ssh_tunnel());
    }

    #[rstest]
    #[case("redis+unix:///run/redis.sock", "/run/redis.sock", None, None)]
    #[case(
        "unix:///tmp/r.sock?db=4&user=app&pass=pw",
        "/tmp/r.sock",
        Some("4"),
        Some("app")
    )]
    fn test_redis_unix_socket_url(
        #[case] url: &str,
        #[case] socket: &str,
        #[case] database: Option<&str>,
        #[case] user: Option<&str>,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::Redis);
        assert_eq!(info.file_path.as_deref(), Some(socket));
        assert_eq!(info.database.as_deref(), database);
        assert_eq!(info.username.as_deref(), user);
        assert!(info.host.is_none());
        assert!(!info.supports_ssh_tunnel());
        // `db`, `user` and `pass` are connection fields, not leftover options
        assert!(info.options.is_empty());
    }

    #[test]
    fn test_redis_unix_socket_url_needs_a_path() {
        assert!(matches!(
            ConnectionInfo::parse_url("redis+unix://"),
            Err(DatabaseError::InvalidUrl(_))
        ));
    }

    #[rstest]
    #[case("redis://localhost:6379/2", "redis://localhost:6379/2")]
    #[case("rediss://app@cache:6390/1", "rediss://app@cache:6390/1")]
    #[case("valkeys://cache", "rediss://cache")]
    #[case("redis://[::1]:6379", "redis://[::1]:6379")]
    #[case(
        "redis+unix:///run/redis.sock?db=2",
        "redis+unix:///run/redis.sock?db=2"
    )]
    fn test_redis_to_url_round_trips_without_the_password(
        #[case] url: &str,
        #[case] expected: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert!(info.is_same_connection(&again));
    }

    #[test]
    fn test_redis_password_never_reaches_debug_or_to_url() {
        let info = ConnectionInfo::parse_url("rediss://app:Cnry-7f3a9@cache.example/1").unwrap();
        assert!(!format!("{info:?}").contains("Cnry-7f3a9"));
        assert!(!info.to_url().contains("Cnry-7f3a9"));
    }

    #[rstest]
    #[case(
        "mssql://sa@localhost",
        "localhost",
        None,
        None,
        None,
        "true",
        true,
        None
    )]
    #[case(
        "sqlserver://sa:p%40ss@db.example:1444/Sales",
        "db.example",
        Some(1444),
        Some("Sales"),
        Some("p@ss"),
        "true",
        true,
        None
    )]
    #[case(
        "mssql://sa@db.example/Sales?encrypt=false",
        "db.example",
        None,
        Some("Sales"),
        None,
        "false",
        false,
        None
    )]
    #[case(
        "mssql://sa@db.example?Encrypt=STRICT",
        "db.example",
        None,
        None,
        None,
        "strict",
        true,
        None
    )]
    #[case(
        "mssql+tls://sa@db.example",
        "db.example",
        None,
        None,
        None,
        "true",
        true,
        None
    )]
    #[case(
        "sqlserver+tls://sa@db.example",
        "db.example",
        None,
        None,
        None,
        "true",
        true,
        None
    )]
    #[case(
        "mssql://sa@[::1]:1433/master",
        "[::1]",
        Some(1433),
        Some("master"),
        None,
        "true",
        true,
        None
    )]
    #[case(
        "mssql://sa@srv\\SQLEXPRESS/Sales",
        "srv",
        None,
        Some("Sales"),
        None,
        "true",
        true,
        Some("SQLEXPRESS")
    )]
    #[case(
        "mssql://sa@srv%5CSQLEXPRESS/Sales",
        "srv",
        None,
        Some("Sales"),
        None,
        "true",
        true,
        Some("SQLEXPRESS")
    )]
    #[case(
        "mssql://sa@srv/Sales?instance=DEV",
        "srv",
        None,
        Some("Sales"),
        None,
        "true",
        true,
        Some("DEV")
    )]
    fn test_sqlserver_url_parsing(
        #[case] url: &str,
        #[case] host: &str,
        #[case] port: Option<u16>,
        #[case] database: Option<&str>,
        #[case] password: Option<&str>,
        #[case] encrypt: &str,
        #[case] tls: bool,
        #[case] instance: Option<&str>,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::SqlServer);
        assert_eq!(info.host.as_deref(), Some(host));
        assert_eq!(info.port, port);
        assert_eq!(info.database.as_deref(), database);
        assert_eq!(info.password.as_deref(), password);
        assert_eq!(info.username.as_deref(), Some("sa"));
        assert_eq!(info.use_tls, tls);
        assert_eq!(
            info.options.get("encrypt").map(String::as_str),
            Some(encrypt)
        );
        assert_eq!(info.options.get("instance").map(String::as_str), instance);
        assert_eq!(info.default_port(), Some(1433));
        assert!(info.file_path.is_none());
        assert!(info.supports_ssh_tunnel());
        assert!(!sqlserver_trusts_any_certificate(&info.options));
    }

    #[test]
    fn test_sqlserver_backslash_in_the_password_is_not_an_instance() {
        let info = ConnectionInfo::parse_url("mssql://sa:pa%5Css@srv/db").unwrap();
        assert_eq!(info.password.as_deref(), Some("pa\\ss"));
        assert_eq!(info.host.as_deref(), Some("srv"));
        assert!(!info.options.contains_key("instance"));
        // an unescaped backslash in the userinfo stays in the userinfo
        let info = ConnectionInfo::parse_url("mssql://DOM\\sa:pw@srv\\INST/db").unwrap();
        assert_eq!(info.host.as_deref(), Some("srv"));
        assert_eq!(
            info.options.get("instance").map(String::as_str),
            Some("INST")
        );
        assert_eq!(info.username.as_deref(), Some("DOM\\sa"));
    }

    #[rstest]
    #[case("mssql://sa@h?trustServerCertificate=true")]
    #[case("mssql://sa@h?TrustServerCertificate=TRUE")]
    #[case("mssql://sa@h?trust_server_certificate=1")]
    fn test_sqlserver_certificate_trust_is_an_explicit_canonical_opt_in(#[case] url: &str) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert!(sqlserver_trusts_any_certificate(&info.options));
        assert_eq!(
            info.options
                .get("trustServerCertificate")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(info.options.len(), 2, "{:?}", info.options); // encrypt + trust
        assert!(info.to_url().contains("trustServerCertificate=true"));
    }

    #[test]
    fn test_sqlserver_trust_defaults_to_verification() {
        let info = ConnectionInfo::parse_url("mssql://sa@h?trustServerCertificate=false").unwrap();
        assert!(!sqlserver_trusts_any_certificate(&info.options));
        assert!(!info.options.contains_key("trustServerCertificate"));
    }

    #[rstest]
    #[case("mssql://sa@h?encrypt=maybe")]
    #[case("mssql+tls://sa@h?encrypt=false")]
    #[case("mssql://sa@h:1433?instance=DEV")]
    #[case("mssql://sa@h?instance=")]
    #[case("mssql://sa@h\\")]
    fn test_sqlserver_invalid_urls_are_rejected(#[case] url: &str) {
        assert!(
            matches!(
                ConnectionInfo::parse_url(url),
                Err(DatabaseError::InvalidUrl(_))
            ),
            "{url}"
        );
    }

    #[rstest]
    #[case("mssql://sa@db:1433/Sales", "mssql://sa@db:1433/Sales")]
    #[case(
        "sqlserver://sa@db/Sales?encrypt=false",
        "mssql://sa@db/Sales?encrypt=false"
    )]
    #[case("mssql://sa@db?encrypt=strict", "mssql://sa@db?encrypt=strict")]
    #[case("mssql+tls://sa@db", "mssql://sa@db")]
    #[case("mssql://sa@srv\\DEV/Sales", "mssql://sa@srv/Sales?instance=DEV")]
    #[case(
        "mssql://sa@db?trustServerCertificate=true",
        "mssql://sa@db?trustServerCertificate=true"
    )]
    fn test_sqlserver_to_url_round_trips_without_the_password(
        #[case] url: &str,
        #[case] expected: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.options, info.options);
        assert_eq!(again.use_tls, info.use_tls);
        assert!(info.is_same_connection(&again));
    }

    #[test]
    fn test_sqlserver_password_never_reaches_debug_or_to_url() {
        let info = ConnectionInfo::parse_url("mssql://sa:Cnry-7f3a9@db.example/Sales").unwrap();
        assert!(!format!("{info:?}").contains("Cnry-7f3a9"));
        assert!(!info.to_url().contains("Cnry-7f3a9"));
        let err = ConnectionInfo::parse_url("mssql://sa:Cnry-7f3a9@db.example/Sales?encrypt=maybe")
            .unwrap_err();
        assert!(!err.to_string().contains("Cnry-7f3a9"), "{err}");
    }

    #[rstest]
    #[case(
        "redshift://awsuser:pw@c.abc.eu-west-1.redshift.amazonaws.com:5439/dev",
        Some("awsuser"),
        "c.abc.eu-west-1.redshift.amazonaws.com",
        Some(5439),
        Some("dev"),
        "require",
        true
    )]
    #[case(
        "redshift://awsuser@c.example",
        Some("awsuser"),
        "c.example",
        None,
        None,
        "require",
        true
    )]
    #[case(
        "redshift+tls://awsuser@c.example/dev",
        Some("awsuser"),
        "c.example",
        None,
        Some("dev"),
        "require",
        true
    )]
    #[case(
        "redshift://u@c.example/dev?sslmode=verify-full&sslrootcert=/etc/amazon.pem",
        Some("u"),
        "c.example",
        None,
        Some("dev"),
        "verify-full",
        true
    )]
    #[case(
        "redshift://u@c.example/dev?SSLMode=VERIFY-CA",
        Some("u"),
        "c.example",
        None,
        Some("dev"),
        "verify-ca",
        true
    )]
    #[case(
        "redshift://u@127.0.0.1:15439/dev?sslmode=disable",
        Some("u"),
        "127.0.0.1",
        Some(15439),
        Some("dev"),
        "disable",
        false
    )]
    #[case(
        "redshift://u@c.example/dev?sslmode=prefer",
        Some("u"),
        "c.example",
        None,
        Some("dev"),
        "prefer",
        false
    )]
    fn test_redshift_url_parsing(
        #[case] url: &str,
        #[case] user: Option<&str>,
        #[case] host: &str,
        #[case] port: Option<u16>,
        #[case] database: Option<&str>,
        #[case] sslmode: &str,
        #[case] use_tls: bool,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::Redshift);
        assert_eq!(info.username.as_deref(), user);
        assert_eq!(info.host.as_deref(), Some(host));
        assert_eq!(info.port, port);
        assert_eq!(info.database.as_deref(), database);
        assert_eq!(info.default_port(), Some(5439));
        assert_eq!(
            info.options.get("sslmode").map(String::as_str),
            Some(sslmode)
        );
        assert_eq!(info.use_tls, use_tls);
    }

    #[rstest]
    #[case("redshift://u@c.example/dev?sslmode=maybe")]
    #[case("redshift+tls://u@c.example/dev?sslmode=disable")]
    #[case("redshift+tls://u@c.example/dev?sslmode=prefer")]
    fn test_redshift_invalid_urls_are_rejected(#[case] url: &str) {
        let err = ConnectionInfo::parse_url(url).unwrap_err();
        assert!(matches!(err, DatabaseError::InvalidUrl(_)), "{url}: {err}");
    }

    #[rstest]
    #[case("redshift://u@c.example:5439/dev", "redshift://u@c.example:5439/dev")]
    #[case("redshift+tls://u@c.example/dev", "redshift://u@c.example/dev")]
    #[case(
        "redshift://u@c.example/dev?sslmode=verify-full",
        "redshift://u@c.example/dev?sslmode=verify-full"
    )]
    #[case(
        "redshift://u@c.example/dev?sslmode=disable",
        "redshift://u@c.example/dev?sslmode=disable"
    )]
    fn test_redshift_to_url_keeps_the_tls_posture_but_not_the_password(
        #[case] url: &str,
        #[case] expected: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.options, info.options);
        assert_eq!(again.use_tls, info.use_tls);
        assert!(info.is_same_connection(&again));
    }

    #[test]
    fn test_redshift_password_never_reaches_debug_or_to_url() {
        let url = "redshift://awsuser:Cnry-7f3a9@c.example/dev";
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert!(!format!("{info:?}").contains("Cnry-7f3a9"));
        assert!(!info.to_url().contains("Cnry-7f3a9"));
        let err =
            ConnectionInfo::parse_url("redshift://awsuser:Cnry-7f3a9@c.example/dev?sslmode=maybe")
                .unwrap_err();
        assert!(!err.to_string().contains("Cnry-7f3a9"), "{err}");
    }

    #[rstest]
    #[case("libsql://my-db-org.turso.io", "my-db-org.turso.io", 443, true, None)]
    #[case("turso://my-db-org.turso.io", "my-db-org.turso.io", 443, true, None)]
    #[case(
        "libsql://db.example:8443/tenant1",
        "db.example",
        8443,
        true,
        Some("tenant1")
    )]
    #[case(
        "libsql://db.example?namespace=tenant2",
        "db.example",
        443,
        true,
        Some("tenant2")
    )]
    #[case("libsql+http://localhost", "localhost", 8080, false, None)]
    #[case(
        "libsql+http://127.0.0.1:9000/ns",
        "127.0.0.1",
        9000,
        false,
        Some("ns")
    )]
    #[case("libsql+http://[::1]:8080", "[::1]", 8080, false, None)]
    #[case(
        "libsql+http://sqld.internal:8080?tls=false",
        "sqld.internal",
        8080,
        false,
        None
    )]
    fn test_libsql_url_parsing(
        #[case] url: &str,
        #[case] host: &str,
        #[case] port: u16,
        #[case] use_tls: bool,
        #[case] namespace: Option<&str>,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::LibSql);
        assert_eq!(info.host.as_deref(), Some(host));
        assert_eq!(info.port, Some(port));
        assert_eq!(info.use_tls, use_tls);
        assert_eq!(info.database.as_deref(), namespace);
        assert_eq!(
            info.options.get("tls").map(String::as_str),
            Some(if use_tls { "true" } else { "false" })
        );
        assert_eq!(info.password, None);
    }

    #[rstest]
    #[case("libsql://db.example?authToken=TOK-1")]
    #[case("libsql://db.example?authtoken=TOK-1")]
    #[case("libsql://db.example?auth_token=TOK-1")]
    #[case("libsql://:TOK-1@db.example")]
    #[case("libsql://user:TOK-1@db.example")]
    #[case("libsql://db.example?authToken=TOK-1&namespace=x")]
    fn test_libsql_auth_token_lands_in_the_password_slot_only(#[case] url: &str) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.password.as_deref(), Some("TOK-1"), "{url}");
        assert!(
            !info
                .options
                .keys()
                .any(|key| key.to_lowercase().contains("token")),
            "{:?}",
            info.options
        );
        assert!(!format!("{info:?}").contains("TOK-1"));
        assert!(!info.to_url().contains("TOK-1"));
        let sanitized = crate::password_sanitizer::sanitize_connection_url(url);
        assert!(!sanitized.contains("TOK-1"), "{sanitized}");
    }

    #[rstest]
    #[case("libsql://:AAA-secret@db.example?authToken=BBB-secret")]
    #[case("libsql://db.example/one?namespace=two")]
    #[case("libsql+http://localhost?tls=true")]
    #[case("libsql://db.example?tls=maybe")]
    #[case("libsql+http://sqld.internal:8080")]
    #[case("libsql+http://10.0.0.5:8080?authToken=BBB-secret")]
    fn test_libsql_invalid_urls_are_rejected_without_leaking_the_token(#[case] url: &str) {
        let err = ConnectionInfo::parse_url(url).unwrap_err();
        assert!(matches!(err, DatabaseError::InvalidUrl(_)), "{url}: {err}");
        assert!(!err.to_string().contains("-secret"), "{err}");
    }

    #[rstest]
    #[case("libsql://db.example", "libsql://db.example")]
    #[case(
        "turso://db.example:8443/ns?authToken=T",
        "libsql://db.example:8443/ns"
    )]
    #[case("libsql+http://localhost:8080", "libsql+http://localhost")]
    #[case("libsql+http://127.0.0.1:9000", "libsql+http://127.0.0.1:9000")]
    #[case(
        "libsql+http://sqld.internal?tls=false",
        "libsql+http://sqld.internal?tls=false"
    )]
    fn test_libsql_to_url_round_trips_without_the_token(#[case] url: &str, #[case] expected: &str) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.options, info.options);
        assert_eq!(again.use_tls, info.use_tls);
        assert_eq!(again.port, info.port);
        assert!(info.is_same_connection(&again));
    }

    #[rstest]
    #[case("localhost", true)]
    #[case("db.localhost", true)]
    #[case("127.0.0.1", true)]
    #[case("127.4.5.6", true)]
    #[case("::1", true)]
    #[case("[::1]", true)]
    #[case("192.168.1.10", false)]
    #[case("sqld.internal", false)]
    #[case("localhost.evil.example", false)]
    fn test_is_loopback_host(#[case] host: &str, #[case] expected: bool) {
        assert_eq!(is_loopback_host(host), expected);
    }

    #[rstest]
    #[case(None, SqlServerEncryption::Required)]
    #[case(Some("TRUE"), SqlServerEncryption::Required)]
    #[case(Some("required"), SqlServerEncryption::Required)]
    #[case(Some("no"), SqlServerEncryption::LoginOnly)]
    #[case(Some("strict"), SqlServerEncryption::Strict)]
    fn test_sqlserver_encryption_levels(
        #[case] value: Option<&str>,
        #[case] expected: SqlServerEncryption,
    ) {
        assert_eq!(SqlServerEncryption::parse(value).unwrap(), expected);
    }

    #[rstest]
    #[case("influxdb://db.example/telemetry", 1, false, None, "telemetry", None)]
    #[case(
        "influxdb://db.example:8087/telemetry",
        1,
        false,
        Some(8087),
        "telemetry",
        None
    )]
    #[case("influxdbs://db.example/telemetry", 1, true, None, "telemetry", None)]
    #[case(
        "influxdb+https://db.example/telemetry",
        1,
        true,
        None,
        "telemetry",
        None
    )]
    #[case(
        "influxdb2://db.example/bkt?org=acme",
        2,
        false,
        None,
        "bkt",
        Some("acme")
    )]
    #[case(
        "influxdb2s://db.example:443/bkt?org=acme",
        2,
        true,
        Some(443),
        "bkt",
        Some("acme")
    )]
    #[case(
        "influxdb2+https://db.example/bkt?ORG=acme",
        2,
        true,
        None,
        "bkt",
        Some("acme")
    )]
    #[case("influxdb3://db.example/mydb", 3, false, Some(8181), "mydb", None)]
    #[case("influxdb3s://db.example:9000/mydb", 3, true, Some(9000), "mydb", None)]
    #[case(
        "influxdb://db.example/?version=2&org=acme&bucket=bkt",
        2,
        false,
        None,
        "bkt",
        Some("acme")
    )]
    #[case(
        "influxdb://db.example/?version=v3&db=mydb",
        3,
        false,
        Some(8181),
        "mydb",
        None
    )]
    #[case("influxdb://db.example/db%20x?ssl=true", 1, true, None, "db x", None)]
    fn test_influxdb_url_parsing(
        #[case] url: &str,
        #[case] version: u8,
        #[case] tls: bool,
        #[case] port: Option<u16>,
        #[case] database: &str,
        #[case] org: Option<&str>,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::InfluxDB);
        assert_eq!(info.host.as_deref(), Some("db.example"));
        assert_eq!(info.port, port);
        assert_eq!(info.use_tls, tls, "{url}");
        assert_eq!(info.database.as_deref(), Some(database));
        assert_eq!(influxdb_version(&info.options), version);
        assert_eq!(info.options.get("org").map(String::as_str), org);
        assert_eq!(info.options.contains_key("tls"), tls);
        assert_eq!(
            info.options.get("version").map(String::as_str),
            Some(version.to_string().as_str())
        );
    }

    #[rstest]
    #[case("influxdb2://h/b?version=1", "means InfluxDB v2")]
    #[case("influxdb3://h/b?version=2", "means InfluxDB v3")]
    #[case("influxdb://h/b?version=9", "unknown InfluxDB version")]
    #[case("influxdbs://h/b?ssl=false", "contradicts")]
    #[case("influxdb://h/b?db=other", "given twice")]
    #[case("influxdb://h/b?language=flux", "cannot query in 'flux'")]
    #[case("influxdb2://h/b?language=sql", "cannot query in 'sql'")]
    #[case("influxdb://:a@h/b?token=b", "token is given twice")]
    fn test_influxdb_invalid_urls_are_rejected(#[case] url: &str, #[case] reason: &str) {
        let error = ConnectionInfo::parse_url(url).unwrap_err();
        assert!(error.to_string().contains(reason), "{url}: {error}");
    }

    #[test]
    fn test_influxdb_token_moves_into_the_password_slot() {
        let from_query =
            ConnectionInfo::parse_url("influxdb2://h/b?org=o&token=Cnry-7f3a9&ssl=true").unwrap();
        assert_eq!(from_query.password.as_deref(), Some("Cnry-7f3a9"));
        assert!(!from_query.options.keys().any(|k| k.contains("token")));
        let from_userinfo = ConnectionInfo::parse_url("influxdb2://:Cnry-7f3a9@h/b?org=o").unwrap();
        assert_eq!(from_userinfo.password.as_deref(), Some("Cnry-7f3a9"));
        assert_eq!(from_userinfo.username, None);
        let same = ConnectionInfo::parse_url("influxdb2://:Cnry-7f3a9@h/b?org=o&token=Cnry-7f3a9")
            .unwrap();
        assert_eq!(same.password.as_deref(), Some("Cnry-7f3a9"));
    }

    #[test]
    fn test_influxdb_secrets_never_reach_debug_to_url_or_errors() {
        let info = ConnectionInfo::parse_url("influxdb2s://admin:Cnry-7f3a9@h/b?org=o").unwrap();
        assert!(!format!("{info:?}").contains("Cnry-7f3a9"));
        assert!(!info.to_url().contains("Cnry-7f3a9"));
        let info = ConnectionInfo::parse_url("influxdb2://h/b?org=o&token=Cnry-7f3a9").unwrap();
        assert!(!format!("{info:?}").contains("Cnry-7f3a9"));
        assert!(!info.to_url().contains("Cnry-7f3a9"));
        let error = ConnectionInfo::parse_url("influxdb://u:Cnry-7f3a9@h/b?version=9").unwrap_err();
        assert!(!error.to_string().contains("Cnry-7f3a9"), "{error}");
    }

    #[rstest]
    #[case("influxdb://alice@h:8086/db", "influxdb://alice@h:8086/db")]
    #[case("influxdbs://h/db", "influxdbs://h/db")]
    #[case("influxdb+https://h/db", "influxdbs://h/db")]
    #[case("influxdb2://h/bkt?org=acme", "influxdb2://h/bkt?org=acme")]
    #[case(
        "influxdb2s://h:443/b%2Fx?org=a%20b",
        "influxdb2s://h:443/b%2Fx?org=a%20b"
    )]
    #[case("influxdb3://h/mydb", "influxdb3://h:8181/mydb")]
    #[case(
        "influxdb://h/db?rp=autogen&verify_certs=false&ssl=true",
        "influxdbs://h/db?rp=autogen&verify_certs=false"
    )]
    #[case("influxdb://[::1]:8086/db", "influxdb://[::1]:8086/db")]
    fn test_influxdb_to_url_round_trips(#[case] url: &str, #[case] expected: &str) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        // The rendered URL is itself a valid InfluxDB URL for the same target
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.host, info.host);
        assert_eq!(again.port, info.port);
        assert_eq!(again.use_tls, info.use_tls);
        assert_eq!(again.database, info.database);
        assert_eq!(again.options, info.options);
    }

    #[rstest]
    #[case("influxdb://h/db", InfluxLanguage::InfluxQl)]
    #[case("influxdb2://h/b?org=o", InfluxLanguage::Flux)]
    #[case("influxdb2://h/b?org=o&language=influxql", InfluxLanguage::InfluxQl)]
    #[case("influxdb3://h/db", InfluxLanguage::Sql)]
    #[case("influxdb3://h/db?language=influxql", InfluxLanguage::InfluxQl)]
    fn test_influxdb_language_follows_the_generation(
        #[case] url: &str,
        #[case] expected: InfluxLanguage,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(InfluxLanguage::from_options(&info.options), expected);
    }

    #[test]
    fn test_influxdb_engine_facts() {
        assert_eq!(DatabaseType::InfluxDB.default_port(), Some(8086));
        assert_eq!(DatabaseType::InfluxDB.display_name(), "InfluxDB");
        assert_eq!(DatabaseType::InfluxDB.url_scheme(), "influxdb");
        assert!(DatabaseType::InfluxDB.supports_ssh_tunnel());
        assert!(!DatabaseType::InfluxDB.is_file_based());
        for scheme in INFLUXDB_SCHEMES {
            assert_eq!(
                DatabaseType::from_scheme(scheme).unwrap(),
                DatabaseType::InfluxDB,
                "{scheme}"
            );
        }
        assert_eq!(
            DatabaseType::InfluxDB.url_schemes().len(),
            INFLUXDB_SCHEMES.len()
        );
        assert!(DatabaseType::supported_types().contains(&DatabaseType::InfluxDB));
    }

    #[test]
    fn test_connection_info_to_url_with_tls() {
        let conn_info = ConnectionInfo {
            database_type: DatabaseType::ClickHouse,
            host: Some("host".to_string()),
            port: Some(8443),
            username: Some("user".to_string()),
            database: Some("mydb".to_string()),
            use_tls: true,
            ..Default::default()
        };

        let url = conn_info.to_url();
        assert_eq!(url, "clickhouse://user@host:8443/mydb?ssl=true");
    }
}
