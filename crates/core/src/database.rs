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
    /// Amazon S3 and S3-compatible stores (Cloudflare R2, MinIO…): not SQL,
    /// queries are the typed read-only commands of [`crate::s3_command`].
    S3,
    /// Amazon DynamoDB (and DynamoDB Local): PartiQL `SELECT` plus the typed
    /// read commands of [`crate::dynamodb_command`].
    DynamoDB,
    /// Amazon CloudWatch Logs: Logs Insights queries over a time window plus
    /// the typed read commands of [`crate::cloudwatch_command`].
    CloudWatchLogs,
    // File formats (via DataFusion)
    Parquet,
    CSV,
    JSON,
    /// DuckDB database files and `:memory:` (the embedded DuckDB engine, cargo
    /// feature `duckdb`; opt-in, it compiles a C++ engine).
    DuckDB,
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

impl DatabaseType {
    /// The AWS-SDK connectors: no host unless a custom endpoint is given, no
    /// port, credentials that are often ambient (profile, SSO, role), and a
    /// secret binding that includes the region and the transport.
    pub fn is_aws(&self) -> bool {
        matches!(
            self,
            DatabaseType::S3 | DatabaseType::DynamoDB | DatabaseType::CloudWatchLogs
        )
    }

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
            DatabaseType::S3 | DatabaseType::DynamoDB | DatabaseType::CloudWatchLogs => None, // AWS endpoint, or the one named by ?endpoint=
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
            DatabaseType::S3 => "Amazon S3",
            DatabaseType::DynamoDB => "Amazon DynamoDB",
            DatabaseType::CloudWatchLogs => "Amazon CloudWatch Logs",
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
            DatabaseType::S3 | DatabaseType::DynamoDB | DatabaseType::CloudWatchLogs => false, // the SDK talks HTTPS to the endpoint itself
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
            DatabaseType::S3 => &S3_SCHEMES,
            DatabaseType::DynamoDB => &DYNAMODB_SCHEMES,
            DatabaseType::CloudWatchLogs => &CLOUDWATCH_SCHEMES,
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
            | DatabaseType::S3
            | DatabaseType::DynamoDB
            | DatabaseType::CloudWatchLogs
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
            | DatabaseType::JSON => true, // DataFusion supports EXPLAIN
            DatabaseType::DuckDB => true, // EXPLAIN (FORMAT JSON)
            DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer // SHOWPLAN_XML only
            | DatabaseType::Redshift // text plan only
            | DatabaseType::LibSql // EXPLAIN QUERY PLAN rows
            | DatabaseType::S3 // no query planner
            | DatabaseType::DynamoDB // PartiQL has no EXPLAIN
            | DatabaseType::CloudWatchLogs // Logs Insights has no query plan
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
            | DatabaseType::S3 // ambient credentials (profile, SSO, role) or public buckets
            | DatabaseType::DynamoDB // ambient credentials (profile, SSO, role)
            | DatabaseType::CloudWatchLogs // ambient credentials (profile, SSO, role)
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
            DatabaseType::S3 => "s3",
            DatabaseType::DynamoDB => "dynamodb",
            DatabaseType::CloudWatchLogs => "cloudwatch",
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
            // S3 has commands, not SQL functions
            DatabaseType::S3 => &[],
            // PartiQL's functions (the driver lists them with its commands)
            DatabaseType::DynamoDB => &[
                "begins_with",
                "contains",
                "attribute_type",
                "size",
                "MISSING",
            ],
            // Logs Insights commands and functions (the driver lists the
            // commands of this client with them)
            DatabaseType::CloudWatchLogs => &[
                "fields",
                "filter",
                "stats",
                "sort",
                "limit",
                "parse",
                "display",
                "dedup",
                "pattern",
                "diff",
                "unnest",
                "count",
                "count_distinct",
                "sum",
                "avg",
                "min",
                "max",
                "pct",
                "bin",
                "strlen",
                "concat",
                "ispresent",
                "isempty",
                "datefloor",
            ],
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
            DatabaseType::DuckDB => DUCKDB_FUNCTIONS,
            // DataFusion SQL functions (for file formats)
            DatabaseType::Parquet | DatabaseType::CSV | DatabaseType::JSON => &[
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
            | DatabaseType::JSON => true, // DataFusion supports timestamp functions
            DatabaseType::DuckDB => false, // epoch_ms()/to_timestamp() instead
            DatabaseType::PostgreSQL
            | DatabaseType::SQLite
            | DatabaseType::WhiteDragon
            | DatabaseType::Redis
            | DatabaseType::SqlServer
            | DatabaseType::Redshift
            | DatabaseType::LibSql
            | DatabaseType::S3
            | DatabaseType::DynamoDB
            | DatabaseType::CloudWatchLogs
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
            DatabaseType::S3 => &["MINIO_ROOT_USER", "MINIO_ACCESS_KEY"],
            DatabaseType::DynamoDB | DatabaseType::CloudWatchLogs => &[],
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
            DatabaseType::S3 => &["MINIO_ROOT_PASSWORD", "MINIO_SECRET_KEY"],
            DatabaseType::DynamoDB | DatabaseType::CloudWatchLogs => &[],
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
            | DatabaseType::LibSql
            | DatabaseType::S3
            | DatabaseType::DynamoDB
            | DatabaseType::CloudWatchLogs => &[],
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
            DatabaseType::LibSql
            | DatabaseType::S3
            | DatabaseType::DynamoDB
            | DatabaseType::CloudWatchLogs => "",
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
            scheme if S3_SCHEMES.contains(&scheme) => Ok(DatabaseType::S3),
            scheme if DYNAMODB_SCHEMES.contains(&scheme) => Ok(DatabaseType::DynamoDB),
            scheme if CLOUDWATCH_SCHEMES.contains(&scheme) => Ok(DatabaseType::CloudWatchLogs),
            "parquet" => Ok(DatabaseType::Parquet),
            "csv" => Ok(DatabaseType::CSV),
            "json" | "ndjson" => Ok(DatabaseType::JSON),
            "duckdb" => Ok(DatabaseType::DuckDB),
            "docker" => Ok(DatabaseType::PostgreSQL), // Default to PostgreSQL for docker:// URLs
            scheme => Err(DatabaseError::UnsupportedScheme(scheme.to_string())),
        }
    }
}

/// URL schemes of the S3 connector. `s3` reaches AWS, or any S3-compatible
/// endpoint through `?endpoint=`; `r2` (Cloudflare R2) and `minio` (a MinIO
/// server) are conveniences that build that endpoint.
pub const S3_SCHEMES: [&str; 3] = ["s3", "r2", "minio"];
/// MinIO's default API port.
pub const MINIO_DEFAULT_PORT: u16 = 9000;

/// Query-string values: everything but unreserved characters and the few
/// reserved ones that read better as they are (`https://host:9000/a/b`).
const S3_QUERY_VALUE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b':')
    .remove(b'/');
/// Object-key prefixes in the path: `/` stays, the rest of the reserved set is encoded.
const S3_PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// Remove (case-insensitively) the first of `names` found in `options`.
fn take_option(options: &mut HashMap<String, String>, names: &[&str]) -> Option<String> {
    let key = options
        .keys()
        .find(|key| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
        .cloned()?;
    options.remove(&key)
}

fn parse_bool_option(name: &str, value: &str) -> Result<bool, DatabaseError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(DatabaseError::InvalidUrl(format!(
            "unknown {name} value '{other}' (use true or false)"
        ))),
    }
}

/// `https://host[:port]`, `http://host[:port]` or a bare `host[:port]` (TLS),
/// as (host, port, tls). A path, credentials or a query are refused.
fn parse_s3_endpoint(value: &str) -> Result<(String, Option<u16>, bool), DatabaseError> {
    let value = value.trim();
    let with_scheme = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let invalid = || {
        DatabaseError::InvalidUrl(format!(
            "'{}' is not a valid S3 endpoint (use https://host[:port] or http://host[:port])",
            crate::password_sanitizer::redact_text(value)
        ))
    };
    let url = Url::parse(&with_scheme).map_err(|_| invalid())?;
    let tls = match url.scheme() {
        "https" => true,
        "http" => false,
        _ => return Err(invalid()),
    };
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(invalid)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok((host.to_string(), url.port(), tls))
}

/// A bucket name that cannot be a path segment or smuggle a request.
fn valid_s3_bucket(bucket: &str) -> bool {
    !bucket.is_empty()
        && bucket.len() <= 255
        && !bucket.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '?' | '#' | '@')
        })
}

/// S3 specifics of a parsed URL (`s3://`, `r2://`, `minio://`). The bucket sits
/// where a host usually is, the key prefix in the path, and the custom endpoint
/// becomes the connection's host/port/TLS, so everything that keeps those
/// (sessions, the secret-store binding, redaction) works unchanged:
///
/// - `s3://[ACCESS:SECRET@]bucket[/prefix][?region=..&endpoint=..&profile=..]`
/// - `r2://[ACCESS:SECRET@]account/bucket[/prefix]` (endpoint
///   `https://<account>.r2.cloudflarestorage.com`, region `auto`)
/// - `minio://[ACCESS:SECRET@]host[:9000]/bucket[/prefix]` (path style,
///   region `us-east-1`, TLS unless `?tls=false`)
///
/// `ConnectionInfo`: `database` = bucket (None for account-level commands),
/// `file_path` = key prefix, `host`/`port`/`use_tls` = custom endpoint (host
/// None = AWS), `username`/`password` = access key id / secret access key.
/// The `tls` option records the transport so a stored secret is never offered
/// to a plain-HTTP endpoint.
fn parse_s3_url(url: &Url, mut info: ConnectionInfo) -> Result<ConnectionInfo, DatabaseError> {
    let decode = |text: &str| {
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    };
    info.username = (!url.username().is_empty()).then(|| decode(url.username()));
    info.password = url.password().map(decode);
    for (key, value) in url.query_pairs() {
        info.options.insert(key.to_string(), value.to_string());
    }
    let scheme = url.scheme();
    let host_part = url.host_str().map(decode).unwrap_or_default();
    let path = decode(url.path().trim_start_matches('/'));
    let endpoint_option = take_option(&mut info.options, &["endpoint", "endpoint_url"]);
    let tls_option = take_option(&mut info.options, &["tls", "ssl"])
        .map(|value| parse_bool_option("tls", &value))
        .transpose()?;

    // (bucket, prefix, endpoint (host, port, tls))
    let (bucket, prefix, endpoint): (String, String, Option<(String, Option<u16>, bool)>) =
        match scheme {
            "r2" => {
                if endpoint_option.is_some() {
                    return Err(DatabaseError::InvalidUrl(
                        "r2:// builds its own endpoint: use s3://bucket?endpoint=… for another one"
                            .to_string(),
                    ));
                }
                if tls_option == Some(false) {
                    return Err(DatabaseError::InvalidUrl(
                        "Cloudflare R2 only speaks HTTPS".to_string(),
                    ));
                }
                if host_part.is_empty() {
                    return Err(DatabaseError::InvalidUrl(
                        "an R2 URL needs the account id: r2://ACCOUNT_ID/bucket".to_string(),
                    ));
                }
                let jurisdiction = take_option(&mut info.options, &["jurisdiction"])
                    .map(|value| value.trim().to_ascii_lowercase());
                let jurisdiction = match jurisdiction.as_deref() {
                    None | Some("") | Some("default") => String::new(),
                    Some(j @ ("eu" | "fedramp")) => format!("{j}."),
                    Some(other) => {
                        return Err(DatabaseError::InvalidUrl(format!(
                            "unknown R2 jurisdiction '{other}' (eu or fedramp)"
                        )));
                    }
                };
                let (bucket, prefix) = path.split_once('/').unwrap_or((path.as_str(), ""));
                info.options
                    .entry("region".to_string())
                    .or_insert_with(|| "auto".to_string());
                (
                    bucket.to_string(),
                    prefix.to_string(),
                    Some((
                        format!("{host_part}.{jurisdiction}r2.cloudflarestorage.com"),
                        None,
                        true,
                    )),
                )
            }
            "minio" => {
                if endpoint_option.is_some() {
                    return Err(DatabaseError::InvalidUrl(
                        "minio:// is the endpoint already: use s3://bucket?endpoint=… for another one"
                            .to_string(),
                    ));
                }
                if host_part.is_empty() {
                    return Err(DatabaseError::InvalidUrl(
                        "a MinIO URL needs the server: minio://host:9000/bucket".to_string(),
                    ));
                }
                let (bucket, prefix) = path.split_once('/').unwrap_or((path.as_str(), ""));
                info.options
                    .entry("region".to_string())
                    .or_insert_with(|| "us-east-1".to_string());
                (
                    bucket.to_string(),
                    prefix.to_string(),
                    Some((
                        host_part.clone(),
                        Some(url.port().unwrap_or(MINIO_DEFAULT_PORT)),
                        tls_option.unwrap_or(true),
                    )),
                )
            }
            _ => {
                let endpoint = endpoint_option
                    .as_deref()
                    .map(parse_s3_endpoint)
                    .transpose()?;
                if let (Some((_, _, tls)), Some(wanted)) = (&endpoint, tls_option) {
                    if *tls != wanted {
                        return Err(DatabaseError::InvalidUrl(
                            "tls= contradicts the scheme of the endpoint".to_string(),
                        ));
                    }
                }
                if endpoint.is_none() && tls_option == Some(false) {
                    return Err(DatabaseError::InvalidUrl(
                        "AWS S3 only speaks HTTPS; tls=false needs an ?endpoint=http://…"
                            .to_string(),
                    ));
                }
                (host_part.clone(), path.clone(), endpoint)
            }
        };

    if !bucket.is_empty() {
        if !valid_s3_bucket(&bucket) {
            return Err(DatabaseError::InvalidUrl(format!(
                "'{bucket}' is not a valid bucket name"
            )));
        }
        info.database = Some(bucket);
    }
    info.file_path = (!prefix.is_empty()).then_some(prefix);
    match endpoint {
        Some((host, port, tls)) => {
            info.host = Some(host);
            info.port = port;
            info.use_tls = tls;
        }
        None => info.use_tls = true,
    }
    info.options
        .insert("tls".to_string(), info.use_tls.to_string());
    Ok(info)
}

/// The in-memory DuckDB database spelled as a path.
pub const DUCKDB_MEMORY_PATH: &str = ":memory:";

/// `duckdb://:memory:` is not a URL the `url` crate accepts (`:memory:` would
/// be the port); it is the same URL with the path spelled out.
pub fn escape_duckdb_memory_authority(url: &str) -> std::borrow::Cow<'_, str> {
    const PREFIX: &str = "duckdb://:memory:";
    match url.get(..PREFIX.len()) {
        Some(head) if head.eq_ignore_ascii_case(PREFIX) => {
            std::borrow::Cow::Owned(format!("duckdb:///:memory:{}", &url[PREFIX.len()..]))
        }
        _ => std::borrow::Cow::Borrowed(url),
    }
}

/// DuckDB functions offered for completion before the catalog is read (the
/// driver completes from `duckdb_functions()` once connected).
const DUCKDB_FUNCTIONS: &[&str] = &[
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "ANY_VALUE",
    "ARG_MAX",
    "ARG_MIN",
    "BOOL_AND",
    "BOOL_OR",
    "FIRST",
    "LAST",
    "LIST",
    "ARRAY_AGG",
    "STRING_AGG",
    "MEDIAN",
    "MODE",
    "QUANTILE_CONT",
    "QUANTILE_DISC",
    "STDDEV",
    "VARIANCE",
    "HISTOGRAM",
    "APPROX_COUNT_DISTINCT",
    "ABS",
    "CEIL",
    "FLOOR",
    "ROUND",
    "SQRT",
    "POW",
    "LN",
    "LOG",
    "RANDOM",
    "COALESCE",
    "NULLIF",
    "IFNULL",
    "CAST",
    "TRY_CAST",
    "LENGTH",
    "LOWER",
    "UPPER",
    "TRIM",
    "SUBSTRING",
    "REPLACE",
    "CONCAT",
    "CONCAT_WS",
    "SPLIT_PART",
    "STRING_SPLIT",
    "REGEXP_MATCHES",
    "REGEXP_REPLACE",
    "REGEXP_EXTRACT",
    "LIST_VALUE",
    "LIST_AGGREGATE",
    "LIST_FILTER",
    "LIST_TRANSFORM",
    "LIST_SORT",
    "LIST_CONTAINS",
    "UNNEST",
    "GENERATE_SERIES",
    "RANGE",
    "STRUCT_PACK",
    "STRUCT_EXTRACT",
    "MAP",
    "MAP_KEYS",
    "MAP_VALUES",
    "JSON_EXTRACT",
    "JSON_EXTRACT_STRING",
    "TO_JSON",
    "NOW",
    "CURRENT_DATE",
    "DATE_TRUNC",
    "DATE_PART",
    "DATE_DIFF",
    "DATE_ADD",
    "STRFTIME",
    "STRPTIME",
    "EPOCH",
    "EPOCH_MS",
    "READ_CSV",
    "READ_PARQUET",
    "READ_JSON",
    "GLOB",
];

/// URL schemes of the DynamoDB connector: `dynamodb` (AWS, or any endpoint over
/// HTTPS), `dynamodb+http` (a plain-HTTP endpoint such as DynamoDB Local) and
/// the short alias `ddb`.
pub const DYNAMODB_SCHEMES: [&str; 3] = ["dynamodb", "dynamodb+http", "ddb"];

/// URL schemes of the CloudWatch Logs connector: `cloudwatch` (AWS, or any
/// endpoint over HTTPS), `cloudwatch+http` (a plain-HTTP endpoint) and the
/// short alias `cwlogs`.
pub const CLOUDWATCH_SCHEMES: [&str; 3] = ["cloudwatch", "cloudwatch+http", "cwlogs"];

/// `eu-west-1`, `us-gov-west-1`, `ap-southeast-2`: lowercase words joined by
/// dashes and ending in a number. What tells a region from an endpoint host in
/// a regional AWS URL (`dynamodb://eu-west-1/Orders` against
/// `dynamodb://localhost:8000`).
pub fn looks_like_aws_region(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    (3..=5).contains(&parts.len())
        && parts[..parts.len() - 1]
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase()))
        && parts
            .last()
            .is_some_and(|last| !last.is_empty() && last.chars().all(|c| c.is_ascii_digit()))
}

/// The host a regional AWS URL uses when it has credentials but neither a
/// region nor an endpoint host (`dynamodb://KEY:SECRET@_?region=local`): the
/// `url` crate refuses an empty host next to credentials. The parser reads it
/// as "no host".
const AWS_NO_HOST: &str = "_";

/// `dynamodb://KEY:SECRET@?endpoint=…` has credentials and an empty host, which
/// the `url` crate rejects: spell the placeholder host out.
pub fn escape_aws_empty_authority(url: &str) -> std::borrow::Cow<'_, str> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return std::borrow::Cow::Borrowed(url);
    };
    if !DYNAMODB_SCHEMES
        .iter()
        .chain(CLOUDWATCH_SCHEMES.iter())
        .any(|known| known.eq_ignore_ascii_case(scheme))
    {
        return std::borrow::Cow::Borrowed(url);
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.ends_with('@') {
        std::borrow::Cow::Owned(format!(
            "{scheme}://{authority}{AWS_NO_HOST}{}",
            &rest[authority_end..]
        ))
    } else {
        std::borrow::Cow::Borrowed(url)
    }
}

/// DynamoDB table names: 3 to 255 characters of `[A-Za-z0-9_.-]`.
fn valid_dynamodb_table(name: &str) -> bool {
    (3..=255).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The URL grammar of the regional AWS services (DynamoDB, CloudWatch Logs):
///
/// - `scheme://[ACCESS:SECRET@][region][/path][?region=..&endpoint=..&profile=..]`
/// - `scheme://[ACCESS:SECRET@]host[:port][/path]`: a host that is not a region
///   is a custom endpoint over HTTPS (VPC endpoint, LocalStack…)
/// - `scheme+http://host[:port][/path]`: a custom endpoint over plain HTTP
///   (DynamoDB Local)
///
/// `ConnectionInfo`: `host`/`port`/`use_tls` = the custom endpoint (no host =
/// AWS), `username`/`password` = access key id / secret access key, option
/// `region`, `database` = what `path` makes of the URL path (a table, a
/// log-group prefix), and the `tls` option records the transport so a stored
/// secret is never offered to a plain-HTTP endpoint. `plain_scheme` is the
/// `+http` spelling.
fn parse_aws_regional_url(
    url: &Url,
    mut info: ConnectionInfo,
    plain_scheme: &str,
    path: impl Fn(&str) -> Result<Option<String>, String>,
) -> Result<ConnectionInfo, DatabaseError> {
    let decode = |text: &str| {
        percent_encoding::percent_decode_str(text)
            .decode_utf8_lossy()
            .into_owned()
    };
    info.username = (!url.username().is_empty()).then(|| decode(url.username()));
    info.password = url.password().map(decode);
    for (key, value) in url.query_pairs() {
        info.options.insert(key.to_string(), value.to_string());
    }
    let host_part = url
        .host_str()
        .map(decode)
        .filter(|host| host != AWS_NO_HOST)
        .unwrap_or_default();
    let endpoint_option = take_option(&mut info.options, &["endpoint", "endpoint_url"]);
    let tls_option = take_option(&mut info.options, &["tls", "ssl"])
        .map(|value| parse_bool_option("tls", &value))
        .transpose()?;
    let plain = url.scheme() == plain_scheme;
    if plain && tls_option == Some(true) {
        return Err(DatabaseError::InvalidUrl(format!(
            "{plain_scheme}:// is plain HTTP: tls=true contradicts it"
        )));
    }

    let mut endpoint: Option<(String, Option<u16>, bool)> = None;
    if host_part.is_empty() {
        if plain {
            return Err(DatabaseError::InvalidUrl(format!(
                "{plain_scheme}:// needs the endpoint: {plain_scheme}://localhost:8000"
            )));
        }
    } else if looks_like_aws_region(&host_part) {
        if plain {
            return Err(DatabaseError::InvalidUrl(format!(
                "{plain_scheme}:// is for a plain-HTTP endpoint, not a region: \
                 use ?endpoint=http://host:port"
            )));
        }
        if url.port().is_some() {
            return Err(DatabaseError::InvalidUrl(format!(
                "'{host_part}' is a region and has no port"
            )));
        }
        if let Some(existing) = info.options.get("region") {
            if *existing != host_part {
                return Err(DatabaseError::InvalidUrl(format!(
                    "the URL names region {host_part} but ?region={existing} says otherwise"
                )));
            }
        }
        info.options.insert("region".to_string(), host_part);
    } else {
        if endpoint_option.is_some() {
            return Err(DatabaseError::InvalidUrl(
                "the endpoint is given twice (the host and ?endpoint=)".to_string(),
            ));
        }
        endpoint = Some((host_part, url.port(), !plain && tls_option != Some(false)));
    }
    if let Some(option) = endpoint_option.as_deref() {
        endpoint = Some(parse_s3_endpoint(option)?);
    }
    if let (Some((_, _, tls)), Some(wanted)) = (&endpoint, tls_option) {
        if *tls != wanted {
            return Err(DatabaseError::InvalidUrl(
                "tls= contradicts the scheme of the endpoint".to_string(),
            ));
        }
    }
    if endpoint.is_none() && tls_option == Some(false) {
        return Err(DatabaseError::InvalidUrl(
            "AWS only speaks HTTPS; tls=false needs an ?endpoint=http://…".to_string(),
        ));
    }

    info.database = path(&decode(url.path().strip_prefix('/').unwrap_or_default()))
        .map_err(DatabaseError::InvalidUrl)?;
    match endpoint {
        Some((host, port, tls)) => {
            info.host = Some(host);
            info.port = port;
            info.use_tls = tls;
        }
        None => info.use_tls = true,
    }
    info.options
        .insert("tls".to_string(), info.use_tls.to_string());
    Ok(info)
}

/// Log-group names: 1 to 512 characters of `[A-Za-z0-9_.\-/#]`.
fn valid_log_group(name: &str) -> bool {
    (1..=512).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '#'))
}

/// CloudWatch Logs specifics of a parsed URL: the path is the log group, or a
/// prefix that selects several. AWS-managed groups start with `/aws/`, so the
/// URL `cloudwatch://eu-west-1/aws/lambda/` is understood as `/aws/lambda/`
/// (and `cloudwatch://eu-west-1//my-app` is the group `/my-app`).
fn parse_cloudwatch_url(url: &Url, info: ConnectionInfo) -> Result<ConnectionInfo, DatabaseError> {
    parse_aws_regional_url(url, info, "cloudwatch+http", |path| {
        if path.is_empty() {
            return Ok(None);
        }
        let group = if path.starts_with("aws/") {
            format!("/{path}")
        } else {
            path.to_string()
        };
        if valid_log_group(&group) {
            Ok(Some(group))
        } else {
            Err(format!(
                "'{group}' is not a log group name or prefix (letters, digits and _ . - / #)"
            ))
        }
    })
}

/// DynamoDB specifics of a parsed URL: the path is the default table.
fn parse_dynamodb_url(url: &Url, info: ConnectionInfo) -> Result<ConnectionInfo, DatabaseError> {
    parse_aws_regional_url(url, info, "dynamodb+http", |path| {
        let table = path.trim_end_matches('/');
        if table.is_empty() {
            Ok(None)
        } else if valid_dynamodb_table(table) {
            Ok(Some(table.to_string()))
        } else {
            Err(format!(
                "'{table}' is not a DynamoDB table name (3-255 letters, digits, _ - .)"
            ))
        }
    })
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

    /// An error from a driver's own SDK (sqlx, DataFusion, DuckDB…), boxed.
    ///
    /// The message prefix and redaction are the ones the SDK-specific variants
    /// used to have: sqlx-backed engines render `SQLx error: …` with secrets
    /// redacted, the DataFusion-backed file engines `DataFusion error: …`,
    /// DuckDB `DuckDB error: …`.
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
        DatabaseType::Parquet | DatabaseType::CSV | DatabaseType::JSON => {
            format!("DataFusion error: {source}")
        }
        DatabaseType::DuckDB => format!(
            "DuckDB error: {}",
            crate::password_sanitizer::redact_text(&source.to_string())
        ),
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
        let url_str = escape_duckdb_memory_authority(url_str.as_ref());
        let url_str = escape_aws_empty_authority(url_str.as_ref());
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

        // S3: the bucket is the URL's host, the key prefix its path
        if database_type == DatabaseType::S3 {
            return parse_s3_url(&url, connection_info);
        }
        // DynamoDB: the host is a region or an endpoint, the path the table
        if database_type == DatabaseType::DynamoDB {
            return parse_dynamodb_url(&url, connection_info);
        }
        // CloudWatch Logs: the same, the path a log group or a prefix of them
        if database_type == DatabaseType::CloudWatchLogs {
            return parse_cloudwatch_url(&url, connection_info);
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

            // `duckdb://:memory:` and `duckdb://` are the in-memory database
            let file_path = if database_type == DatabaseType::DuckDB
                && (file_path.is_empty() || file_path == "/:memory:")
            {
                DUCKDB_MEMORY_PATH.to_string()
            } else {
                file_path
            };

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

    /// `to_url` for the regional AWS services (DynamoDB, CloudWatch Logs):
    /// `scheme://[access-key-id@][region][/path]` and the custom endpoint with
    /// its transport plus the non-secret options as the query. Never the secret.
    fn aws_regional_url(&self, scheme: &str) -> String {
        let mut url = format!("{scheme}://");
        if let Some(username) = &self.username {
            url.push_str(
                &percent_encoding::utf8_percent_encode(
                    username,
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string(),
            );
            url.push('@');
        }
        let region = self
            .options
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("region"))
            .map(|(_, value)| value.as_str());
        // Only a region-shaped one can stand in the host position; any other
        // (`local`, `auto`) stays a query parameter
        let region_in_host = region.filter(|region| looks_like_aws_region(region));
        match region_in_host {
            Some(region) => url.push_str(region),
            None if self.username.is_some() => url.push_str(AWS_NO_HOST),
            None => {}
        }
        if let Some(database) = &self.database {
            // `/aws/lambda/x` is written `aws/lambda/x` (the parser puts the
            // slash back); any other name keeps its own leading slash
            let shown = database
                .strip_prefix('/')
                .filter(|rest| rest.starts_with("aws/"))
                .unwrap_or(database);
            url.push('/');
            url.push_str(
                &percent_encoding::utf8_percent_encode(shown, S3_PATH_SEGMENT).to_string(),
            );
        }
        let mut query: Vec<String> = Vec::new();
        if let Some(host) = &self.host {
            let scheme = if self.use_tls { "https" } else { "http" };
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host.clone()
            };
            let endpoint = match self.port {
                Some(port) => format!("{scheme}://{host}:{port}"),
                None => format!("{scheme}://{host}"),
            };
            query.push(format!(
                "endpoint={}",
                percent_encoding::utf8_percent_encode(&endpoint, S3_QUERY_VALUE)
            ));
        }
        let mut options: Vec<(&String, &String)> = self
            .options
            .iter()
            .filter(|(key, value)| {
                let in_host =
                    key.eq_ignore_ascii_case("region") && region_in_host == Some(value.as_str());
                let hidden = key.eq_ignore_ascii_case("tls")
                    || key.eq_ignore_ascii_case("endpoint")
                    || crate::password_sanitizer::is_secret_key(key);
                !(in_host || hidden)
            })
            .collect();
        options.sort();
        for (key, value) in options {
            query.push(format!(
                "{key}={}",
                percent_encoding::utf8_percent_encode(value, S3_QUERY_VALUE)
            ));
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

    /// `to_url` for S3, always in the canonical `s3://` spelling (the `r2://`
    /// and `minio://` conveniences expand to their endpoint): the access key id
    /// (never the secret), the bucket and prefix, the custom endpoint with its
    /// transport, and the non-secret options (`region`, `profile`, …).
    fn s3_url(&self) -> String {
        let mut url = String::from("s3://");
        if let Some(username) = &self.username {
            url.push_str(
                &percent_encoding::utf8_percent_encode(
                    username,
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string(),
            );
            url.push('@');
        }
        if let Some(bucket) = &self.database {
            url.push_str(bucket);
        }
        if let Some(prefix) = &self.file_path {
            url.push('/');
            url.push_str(
                &percent_encoding::utf8_percent_encode(prefix, S3_PATH_SEGMENT).to_string(),
            );
        }
        let mut query: Vec<String> = Vec::new();
        if let Some(host) = &self.host {
            let scheme = if self.use_tls { "https" } else { "http" };
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host.clone()
            };
            let endpoint = match self.port {
                Some(port) => format!("{scheme}://{host}:{port}"),
                None => format!("{scheme}://{host}"),
            };
            query.push(format!(
                "endpoint={}",
                percent_encoding::utf8_percent_encode(&endpoint, S3_QUERY_VALUE)
            ));
        }
        let mut options: Vec<(&String, &String)> = self
            .options
            .iter()
            .filter(|(key, _)| {
                !key.eq_ignore_ascii_case("tls")
                    && !key.eq_ignore_ascii_case("endpoint")
                    && !crate::password_sanitizer::is_secret_key(key)
            })
            .collect();
        options.sort();
        for (key, value) in options {
            query.push(format!(
                "{key}={}",
                percent_encoding::utf8_percent_encode(value, S3_QUERY_VALUE)
            ));
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
        if self.database_type == DatabaseType::S3 {
            return self.s3_url();
        }
        if self.database_type == DatabaseType::DynamoDB {
            return self.aws_regional_url("dynamodb");
        }
        if self.database_type == DatabaseType::CloudWatchLogs {
            return self.aws_regional_url("cloudwatch");
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
    // s3://: the bucket is the host, the prefix the path; no endpoint = AWS
    #[case("s3://my-bucket", Some("my-bucket"), None, None, None, None, true)]
    #[case("s3://my-bucket/", Some("my-bucket"), None, None, None, None, true)]
    #[case(
        "s3://my-bucket/logs/2026/",
        Some("my-bucket"),
        Some("logs/2026/"),
        None,
        None,
        None,
        true
    )]
    #[case("s3://", None, None, None, None, None, true)]
    #[case("s3://?region=eu-west-1", None, None, None, None, None, true)]
    #[case(
        "s3://b/a%20b/c?endpoint=https://s3.example.com",
        Some("b"),
        Some("a b/c"),
        Some("s3.example.com"),
        None,
        None,
        true
    )]
    #[case(
        "s3://b?endpoint=http://localhost:9000",
        Some("b"),
        None,
        Some("localhost"),
        Some(9000),
        None,
        false
    )]
    #[case(
        "s3://b?endpoint=minio.internal:9000",
        Some("b"),
        None,
        Some("minio.internal"),
        Some(9000),
        None,
        true
    )]
    #[case(
        "s3://b?endpoint=http://[::1]:9000&tls=false",
        Some("b"),
        None,
        Some("[::1]"),
        Some(9000),
        None,
        false
    )]
    // minio://: server + bucket in the path, path style, TLS unless tls=false
    #[case(
        "minio://minio.local:9001/data/raw/",
        Some("data"),
        Some("raw/"),
        Some("minio.local"),
        Some(9001),
        Some("us-east-1"),
        true
    )]
    #[case(
        "minio://localhost/data?tls=false",
        Some("data"),
        None,
        Some("localhost"),
        Some(9000),
        Some("us-east-1"),
        false
    )]
    // r2://: the account builds the endpoint, region auto
    #[case(
        "r2://abc123/media/img/",
        Some("media"),
        Some("img/"),
        Some("abc123.r2.cloudflarestorage.com"),
        None,
        Some("auto"),
        true
    )]
    #[case(
        "r2://abc123/media?jurisdiction=eu",
        Some("media"),
        None,
        Some("abc123.eu.r2.cloudflarestorage.com"),
        None,
        Some("auto"),
        true
    )]
    fn test_s3_url_parsing(
        #[case] url: &str,
        #[case] bucket: Option<&str>,
        #[case] prefix: Option<&str>,
        #[case] host: Option<&str>,
        #[case] port: Option<u16>,
        #[case] region: Option<&str>,
        #[case] use_tls: bool,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::S3, "{url}");
        assert_eq!(info.database.as_deref(), bucket, "{url}");
        assert_eq!(info.file_path.as_deref(), prefix, "{url}");
        assert_eq!(info.host.as_deref(), host, "{url}");
        assert_eq!(info.port, port, "{url}");
        assert_eq!(info.use_tls, use_tls, "{url}");
        assert_eq!(
            info.options.get("tls").map(String::as_str),
            Some(if use_tls { "true" } else { "false" })
        );
        if let Some(region) = region {
            assert_eq!(info.options.get("region").map(String::as_str), Some(region));
        }
        // the endpoint is the connection's host, never an option
        assert!(!info.options.contains_key("endpoint"), "{url}");
    }

    #[rstest]
    #[case("s3://AKIAEXAMPLE:wJalr%2FXUtn@b/p", "AKIAEXAMPLE", "wJalr/XUtn")]
    #[case("r2://AKID:sec%40ret@acc/bkt", "AKID", "sec@ret")]
    #[case("minio://root:minio-pass@h:9000/b", "root", "minio-pass")]
    fn test_s3_credentials_land_in_the_user_and_password_slots(
        #[case] url: &str,
        #[case] user: &str,
        #[case] secret: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.username.as_deref(), Some(user));
        assert_eq!(info.password.as_deref(), Some(secret));
        assert!(!format!("{info:?}").contains(secret), "{info:?}");
        assert!(!info.to_url().contains(secret), "{}", info.to_url());
        let sanitized = crate::password_sanitizer::sanitize_connection_url(url);
        assert!(!sanitized.contains(secret), "{sanitized}");
    }

    #[test]
    fn test_s3_session_token_option_is_a_secret() {
        let url =
            "s3://AKIAEXAMPLE:topsecret@b?session_token=FwoGZXIvYXdzSESSIONCANARY&region=eu-west-1";
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert!(!format!("{info:?}").contains("SESSIONCANARY"), "{info:?}");
        assert!(
            !info.to_url().contains("SESSIONCANARY"),
            "{}",
            info.to_url()
        );
        assert!(!crate::password_sanitizer::sanitize_connection_url(url).contains("SESSIONCANARY"));
        assert!(info.to_url().contains("region=eu-west-1"));
    }

    #[rstest]
    #[case("s3://b?endpoint=ftp://h")]
    #[case("s3://b?endpoint=https://h/path")]
    #[case("s3://b?endpoint=https://u:p@h")]
    #[case("s3://b?endpoint=")]
    #[case("s3://b?tls=false")]
    #[case("s3://b?endpoint=http://h&tls=true")]
    #[case("s3://b?tls=maybe")]
    #[case("r2:///bucket")]
    #[case("r2://acc/b?tls=false")]
    #[case("r2://acc/b?endpoint=https://h")]
    #[case("r2://acc/b?jurisdiction=mars")]
    #[case("minio:///bucket")]
    #[case("minio://h/b?endpoint=https://x")]
    fn test_s3_invalid_urls_are_rejected(#[case] url: &str) {
        let err = ConnectionInfo::parse_url(url).unwrap_err();
        assert!(matches!(err, DatabaseError::InvalidUrl(_)), "{url}: {err}");
    }

    #[rstest]
    #[case("s3://b", "s3://b")]
    #[case("s3://b/logs/2026/", "s3://b/logs/2026/")]
    #[case("s3://AKIA:secret@b?region=eu-west-1", "s3://AKIA@b?region=eu-west-1")]
    #[case("s3://b/a%20b", "s3://b/a%20b")]
    #[case(
        "minio://localhost:9000/data/raw?tls=false",
        "s3://data/raw?endpoint=http://localhost:9000&region=us-east-1"
    )]
    #[case(
        "r2://abc/media",
        "s3://media?endpoint=https://abc.r2.cloudflarestorage.com&region=auto"
    )]
    #[case(
        "s3://b?endpoint=https://s3.example.com:8443&path_style=true&profile=prod-ro&anonymous=false",
        "s3://b?endpoint=https://s3.example.com:8443&anonymous=false&path_style=true&profile=prod-ro"
    )]
    fn test_s3_to_url_is_canonical_and_round_trips_without_the_secret(
        #[case] url: &str,
        #[case] expected: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.to_url(), expected);
        assert_eq!(again.host, info.host);
        assert_eq!(again.port, info.port);
        assert_eq!(again.use_tls, info.use_tls);
        assert_eq!(again.database, info.database);
        assert_eq!(again.file_path, info.file_path);
        assert_eq!(again.options, info.options);
    }

    #[test]
    fn test_s3_database_type_metadata() {
        assert_eq!(DatabaseType::S3.display_name(), "Amazon S3");
        assert_eq!(DatabaseType::S3.url_scheme(), "s3");
        assert_eq!(DatabaseType::S3.default_port(), None);
        assert!(!DatabaseType::S3.is_file_based());
        assert!(!DatabaseType::S3.supports_ssh_tunnel());
        assert!(!DatabaseType::S3.requires_authentication());
        assert_eq!(DatabaseType::S3.url_schemes(), ["s3", "r2", "minio"]);
        for scheme in DatabaseType::S3.url_schemes() {
            assert_eq!(DatabaseType::from_scheme(scheme).unwrap(), DatabaseType::S3);
        }
        assert!(DatabaseType::supported_types().contains(&DatabaseType::S3));
    }

    #[rstest]
    // dynamodb://: a region-shaped host is the region, the path the default table
    #[case("dynamodb://eu-west-1", None, None, None, Some("eu-west-1"), true)]
    #[case("dynamodb://eu-west-1/", None, None, None, Some("eu-west-1"), true)]
    #[case(
        "dynamodb://eu-west-1/Orders",
        Some("Orders"),
        None,
        None,
        Some("eu-west-1"),
        true
    )]
    #[case(
        "dynamodb://us-gov-west-1/my.table-1",
        Some("my.table-1"),
        None,
        None,
        Some("us-gov-west-1"),
        true
    )]
    #[case(
        "ddb://ap-southeast-2/T_1",
        Some("T_1"),
        None,
        None,
        Some("ap-southeast-2"),
        true
    )]
    #[case("dynamodb://", None, None, None, None, true)]
    #[case(
        "dynamodb://?region=eu-west-3&profile=ro",
        None,
        None,
        None,
        Some("eu-west-3"),
        true
    )]
    #[case(
        "dynamodb:///Orders?region=eu-west-3",
        Some("Orders"),
        None,
        None,
        Some("eu-west-3"),
        true
    )]
    // any other host is a custom endpoint (HTTPS), dynamodb+http a plain one
    #[case(
        "dynamodb://vpce.example.com:8443/Orders",
        Some("Orders"),
        Some("vpce.example.com"),
        Some(8443),
        None,
        true
    )]
    #[case(
        "dynamodb+http://localhost:8000",
        None,
        Some("localhost"),
        Some(8000),
        None,
        false
    )]
    #[case(
        "dynamodb+http://localhost:8000/Orders?region=local",
        Some("Orders"),
        Some("localhost"),
        Some(8000),
        Some("local"),
        false
    )]
    #[case(
        "dynamodb://us-east-1?endpoint=http://localhost:8000",
        None,
        Some("localhost"),
        Some(8000),
        Some("us-east-1"),
        false
    )]
    #[case(
        "dynamodb://localhost:8000?tls=false",
        None,
        Some("localhost"),
        Some(8000),
        None,
        false
    )]
    #[case(
        "dynamodb://eu-west-1/Orders?endpoint=https://dynamodb.internal",
        Some("Orders"),
        Some("dynamodb.internal"),
        None,
        Some("eu-west-1"),
        true
    )]
    fn test_dynamodb_url_parsing(
        #[case] url: &str,
        #[case] table: Option<&str>,
        #[case] host: Option<&str>,
        #[case] port: Option<u16>,
        #[case] region: Option<&str>,
        #[case] use_tls: bool,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::DynamoDB, "{url}");
        assert_eq!(info.database.as_deref(), table, "{url}");
        assert_eq!(info.host.as_deref(), host, "{url}");
        assert_eq!(info.port, port, "{url}");
        assert_eq!(info.use_tls, use_tls, "{url}");
        assert_eq!(
            info.options.get("region").map(String::as_str),
            region,
            "{url}"
        );
        assert_eq!(
            info.options.get("tls").map(String::as_str),
            Some(if use_tls { "true" } else { "false" })
        );
        assert!(!info.options.contains_key("endpoint"), "{url}");
        assert_eq!(info.file_path, None);
    }

    #[rstest]
    #[case(
        "dynamodb://AKIAEXAMPLE:wJalr%2FXUtn@eu-west-1/Orders",
        "AKIAEXAMPLE",
        "wJalr/XUtn"
    )]
    #[case("ddb://AKID:sec%40ret@us-east-1", "AKID", "sec@ret")]
    #[case(
        "dynamodb+http://local:pass-word@localhost:8000/Orders",
        "local",
        "pass-word"
    )]
    fn test_dynamodb_credentials_land_in_the_user_and_password_slots(
        #[case] url: &str,
        #[case] user: &str,
        #[case] secret: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.username.as_deref(), Some(user));
        assert_eq!(info.password.as_deref(), Some(secret));
        assert!(!format!("{info:?}").contains(secret), "{info:?}");
        assert!(!info.to_url().contains(secret), "{}", info.to_url());
        let sanitized = crate::password_sanitizer::sanitize_connection_url(url);
        assert!(!sanitized.contains(secret), "{sanitized}");
    }

    #[test]
    fn test_dynamodb_session_token_option_is_a_secret() {
        let url = "dynamodb://AKIAEXAMPLE:topsecret@eu-west-1/Orders?session_token=FwoGZXIvYXdzSESSIONCANARY";
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert!(!format!("{info:?}").contains("SESSIONCANARY"), "{info:?}");
        assert!(
            !info.to_url().contains("SESSIONCANARY"),
            "{}",
            info.to_url()
        );
        assert!(!crate::password_sanitizer::sanitize_connection_url(url).contains("SESSIONCANARY"));
    }

    #[rstest]
    #[case("dynamodb://eu-west-1:8000")]
    #[case("dynamodb://eu-west-1?region=us-east-1")]
    #[case("dynamodb://host.example/Tbl1?endpoint=https://other")]
    #[case("dynamodb://eu-west-1?tls=false")]
    #[case("dynamodb://eu-west-1?endpoint=http://h&tls=true")]
    #[case("dynamodb://eu-west-1?endpoint=ftp://h")]
    #[case("dynamodb://eu-west-1?endpoint=https://h/path")]
    #[case("dynamodb://eu-west-1?tls=maybe")]
    #[case("dynamodb://eu-west-1/ab")]
    #[case("dynamodb://eu-west-1/has%20space")]
    #[case("dynamodb://eu-west-1/a/b")]
    #[case("dynamodb+http://")]
    #[case("dynamodb+http://eu-west-1/Orders")]
    #[case("dynamodb+http://localhost:8000?tls=true")]
    #[case("dynamodb://localhost:8000?tls=true&endpoint=http://x")]
    fn test_dynamodb_invalid_urls_are_rejected(#[case] url: &str) {
        let err = ConnectionInfo::parse_url(url).unwrap_err();
        assert!(matches!(err, DatabaseError::InvalidUrl(_)), "{url}: {err}");
    }

    #[rstest]
    #[case("dynamodb://eu-west-1", "dynamodb://eu-west-1")]
    #[case("ddb://eu-west-1/Orders", "dynamodb://eu-west-1/Orders")]
    #[case(
        "dynamodb://AKIA:secret@eu-west-1/Orders?profile=x",
        "dynamodb://AKIA@eu-west-1/Orders?profile=x"
    )]
    #[case(
        "dynamodb+http://localhost:8000/Tbl1?region=us-east-1",
        "dynamodb://us-east-1/Tbl1?endpoint=http://localhost:8000"
    )]
    #[case(
        "dynamodb+http://localhost:8000?region=local",
        "dynamodb://?endpoint=http://localhost:8000&region=local"
    )]
    #[case(
        "dynamodb://vpce.example.com:8443/Tbl1?region=eu-west-1",
        "dynamodb://eu-west-1/Tbl1?endpoint=https://vpce.example.com:8443"
    )]
    #[case(
        "dynamodb://?region=eu-west-3&profile=ro&role_arn=arn:aws:iam::1:role/r",
        "dynamodb://eu-west-3?profile=ro&role_arn=arn:aws:iam::1:role/r"
    )]
    fn test_dynamodb_to_url_is_canonical_and_round_trips_without_the_secret(
        #[case] url: &str,
        #[case] expected: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.to_url(), expected);
        assert_eq!(again.host, info.host);
        assert_eq!(again.port, info.port);
        assert_eq!(again.use_tls, info.use_tls);
        assert_eq!(again.database, info.database);
        assert_eq!(again.username, info.username);
        assert_eq!(again.options, info.options);
    }

    #[test]
    fn test_dynamodb_access_key_without_a_region_round_trips() {
        // static keys with the region left to the environment or the endpoint
        for url in [
            "dynamodb://AKIAEXAMPLE:secret@?endpoint=http://localhost:8000",
            "dynamodb+http://AKIAEXAMPLE:secret@localhost:8000/Orders",
        ] {
            let info = ConnectionInfo::parse_url(url).unwrap();
            assert_eq!(info.username.as_deref(), Some("AKIAEXAMPLE"), "{url}");
            let rendered = info.to_url();
            assert!(!rendered.contains("secret"), "{rendered}");
            let again = ConnectionInfo::parse_url(&rendered)
                .unwrap_or_else(|e| panic!("{rendered} does not parse back: {e}"));
            assert_eq!(again.username, info.username, "{rendered}");
            assert_eq!(again.host, info.host, "{rendered}");
            assert_eq!(again.port, info.port, "{rendered}");
            assert_eq!(again.use_tls, info.use_tls, "{rendered}");
            assert_eq!(again.database, info.database, "{rendered}");
        }
    }

    #[rstest]
    #[case("cloudwatch://eu-west-1", None, None, None, Some("eu-west-1"), true)]
    #[case("cloudwatch://eu-west-1/", None, None, None, Some("eu-west-1"), true)]
    // AWS-managed groups start with /aws/: the slash is optional in the URL
    #[case(
        "cloudwatch://eu-west-1/aws/lambda/orders",
        Some("/aws/lambda/orders"),
        None,
        None,
        Some("eu-west-1"),
        true
    )]
    #[case(
        "cloudwatch://eu-west-1//aws/lambda/",
        Some("/aws/lambda/"),
        None,
        None,
        Some("eu-west-1"),
        true
    )]
    #[case(
        "cwlogs://us-east-1/my-app",
        Some("my-app"),
        None,
        None,
        Some("us-east-1"),
        true
    )]
    #[case(
        "cloudwatch://us-east-1//my-app",
        Some("/my-app"),
        None,
        None,
        Some("us-east-1"),
        true
    )]
    #[case(
        "cloudwatch://?region=eu-west-3&profile=ro",
        None,
        None,
        None,
        Some("eu-west-3"),
        true
    )]
    #[case(
        "cloudwatch+http://localhost:4566/aws/x?region=us-east-1",
        Some("/aws/x"),
        Some("localhost"),
        Some(4566),
        Some("us-east-1"),
        false
    )]
    #[case(
        "cloudwatch://logs.internal:8443",
        None,
        Some("logs.internal"),
        Some(8443),
        None,
        true
    )]
    fn test_cloudwatch_url_parsing(
        #[case] url: &str,
        #[case] group: Option<&str>,
        #[case] host: Option<&str>,
        #[case] port: Option<u16>,
        #[case] region: Option<&str>,
        #[case] use_tls: bool,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::CloudWatchLogs, "{url}");
        assert_eq!(info.database.as_deref(), group, "{url}");
        assert_eq!(info.host.as_deref(), host, "{url}");
        assert_eq!(info.port, port, "{url}");
        assert_eq!(info.use_tls, use_tls, "{url}");
        assert_eq!(
            info.options.get("region").map(String::as_str),
            region,
            "{url}"
        );
        assert!(!info.options.contains_key("endpoint"), "{url}");
    }

    #[rstest]
    #[case("cloudwatch://eu-west-1/has%20space")]
    #[case("cloudwatch://eu-west-1/a%3Fb")]
    #[case("cloudwatch://eu-west-1:443")]
    #[case("cloudwatch://eu-west-1?tls=false")]
    #[case("cloudwatch+http://eu-west-1/x")]
    #[case("cloudwatch+http://")]
    fn test_cloudwatch_invalid_urls_are_rejected(#[case] url: &str) {
        let err = ConnectionInfo::parse_url(url).unwrap_err();
        assert!(matches!(err, DatabaseError::InvalidUrl(_)), "{url}: {err}");
    }

    #[rstest]
    #[case("cloudwatch://eu-west-1", "cloudwatch://eu-west-1")]
    #[case(
        "cwlogs://eu-west-1/aws/lambda/orders",
        "cloudwatch://eu-west-1/aws/lambda/orders"
    )]
    #[case("cloudwatch://eu-west-1//my-app", "cloudwatch://eu-west-1//my-app")]
    #[case(
        "cloudwatch://AKIA:secret@eu-west-1/aws/x?since=30m",
        "cloudwatch://AKIA@eu-west-1/aws/x?since=30m"
    )]
    #[case(
        "cloudwatch+http://localhost:4566?region=us-east-1",
        "cloudwatch://us-east-1?endpoint=http://localhost:4566"
    )]
    fn test_cloudwatch_to_url_is_canonical_and_round_trips_without_the_secret(
        #[case] url: &str,
        #[case] expected: &str,
    ) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), expected);
        assert!(!info.to_url().contains("secret"));
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.to_url(), expected);
        assert_eq!(again.host, info.host);
        assert_eq!(again.port, info.port);
        assert_eq!(again.use_tls, info.use_tls);
        assert_eq!(again.database, info.database);
        assert_eq!(again.username, info.username);
        assert_eq!(again.options, info.options);
    }

    #[test]
    fn test_cloudwatch_credentials_land_in_the_user_and_password_slots() {
        let url =
            "cloudwatch://AKIAEXAMPLE:wJalr%2FXUtn@eu-west-1/aws/x?session_token=SESSIONCANARY";
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.username.as_deref(), Some("AKIAEXAMPLE"));
        assert_eq!(info.password.as_deref(), Some("wJalr/XUtn"));
        let shown = format!("{info:?} {}", info.to_url());
        assert!(
            !shown.contains("wJalr") && !shown.contains("SESSIONCANARY"),
            "{shown}"
        );
        // credentials with no region and no host
        let bare = ConnectionInfo::parse_url(
            "cloudwatch://AKIA:secret@?region=local&endpoint=http://h:4566",
        )
        .unwrap();
        let again = ConnectionInfo::parse_url(&bare.to_url()).unwrap();
        assert_eq!(again.username.as_deref(), Some("AKIA"));
        assert_eq!(again.host.as_deref(), Some("h"));
    }

    #[test]
    fn test_cloudwatch_database_type_metadata() {
        assert_eq!(
            DatabaseType::CloudWatchLogs.display_name(),
            "Amazon CloudWatch Logs"
        );
        assert_eq!(DatabaseType::CloudWatchLogs.url_scheme(), "cloudwatch");
        assert_eq!(DatabaseType::CloudWatchLogs.default_port(), None);
        assert!(!DatabaseType::CloudWatchLogs.is_file_based());
        assert!(!DatabaseType::CloudWatchLogs.requires_authentication());
        assert!(DatabaseType::CloudWatchLogs.is_aws());
        assert_eq!(
            DatabaseType::CloudWatchLogs.url_schemes(),
            ["cloudwatch", "cloudwatch+http", "cwlogs"]
        );
        for scheme in DatabaseType::CloudWatchLogs.url_schemes() {
            assert_eq!(
                DatabaseType::from_scheme(scheme).unwrap(),
                DatabaseType::CloudWatchLogs
            );
        }
    }

    #[test]
    fn test_dynamodb_database_type_metadata() {
        assert_eq!(DatabaseType::DynamoDB.display_name(), "Amazon DynamoDB");
        assert_eq!(DatabaseType::DynamoDB.url_scheme(), "dynamodb");
        assert_eq!(DatabaseType::DynamoDB.default_port(), None);
        assert!(!DatabaseType::DynamoDB.is_file_based());
        assert!(!DatabaseType::DynamoDB.supports_ssh_tunnel());
        assert!(!DatabaseType::DynamoDB.requires_authentication());
        assert_eq!(
            DatabaseType::DynamoDB.url_schemes(),
            ["dynamodb", "dynamodb+http", "ddb"]
        );
        for scheme in DatabaseType::DynamoDB.url_schemes() {
            assert_eq!(
                DatabaseType::from_scheme(scheme).unwrap(),
                DatabaseType::DynamoDB
            );
        }
        assert!(DatabaseType::supported_types().contains(&DatabaseType::DynamoDB));
    }

    #[rstest]
    #[case("eu-west-1", true)]
    #[case("us-gov-west-1", true)]
    #[case("ap-southeast-12", true)]
    #[case("localhost", false)]
    #[case("local", false)]
    #[case("us-east", false)]
    #[case("US-EAST-1", false)]
    #[case("eu-west-x", false)]
    #[case("a-b-c-d-e-1", false)]
    #[case("vpce-123-abc.example.com", false)]
    fn test_looks_like_aws_region(#[case] text: &str, #[case] expected: bool) {
        assert_eq!(looks_like_aws_region(text), expected, "{text}");
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

    #[rstest]
    #[case::absolute("duckdb:///tmp/warehouse.duckdb", "/tmp/warehouse.duckdb")]
    #[case::memory("duckdb://:memory:", ":memory:")]
    #[case::memory_upper("DUCKDB://:MEMORY:", ":memory:")]
    #[case::memory_without_authority("duckdb://", ":memory:")]
    #[case::memory_as_path("duckdb:///:memory:", ":memory:")]
    #[case::spaces("duckdb:///tmp/my%20data.ddb", "/tmp/my data.ddb")]
    fn test_duckdb_url_parsing(#[case] url: &str, #[case] file_path: &str) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.database_type, DatabaseType::DuckDB);
        assert_eq!(info.file_path.as_deref(), Some(file_path));
        assert!(info.host.is_none() && info.password.is_none());
    }

    #[test]
    fn test_duckdb_url_options_and_memory_with_options() {
        let info = ConnectionInfo::parse_url(
            "duckdb:///tmp/x.duckdb?access_mode=read_only&threads=2&memory_limit=1GB",
        )
        .unwrap();
        assert_eq!(
            info.options.get("access_mode").map(String::as_str),
            Some("read_only")
        );
        assert_eq!(info.options.get("threads").map(String::as_str), Some("2"));
        assert_eq!(
            info.options.get("memory_limit").map(String::as_str),
            Some("1GB")
        );
        // the DuckLake form: an in-memory session with a catalog attached
        let lake = ConnectionInfo::parse_url(
            "duckdb://:memory:?ducklake=sqlite:/srv/lake/meta.sqlite&data_path=/srv/lake/data/",
        )
        .unwrap();
        assert_eq!(lake.file_path.as_deref(), Some(":memory:"));
        assert_eq!(
            lake.options.get("ducklake").map(String::as_str),
            Some("sqlite:/srv/lake/meta.sqlite")
        );
        assert_eq!(
            lake.options.get("data_path").map(String::as_str),
            Some("/srv/lake/data/")
        );
    }

    #[rstest]
    #[case("duckdb:///tmp/warehouse.duckdb")]
    #[case("duckdb://:memory:")]
    fn test_duckdb_to_url_round_trips(#[case] url: &str) {
        let info = ConnectionInfo::parse_url(url).unwrap();
        assert_eq!(info.to_url(), url);
        let again = ConnectionInfo::parse_url(&info.to_url()).unwrap();
        assert_eq!(again.file_path, info.file_path);
    }

    #[test]
    fn test_duckdb_engine_facts() {
        let engine = DatabaseType::DuckDB;
        assert!(engine.is_file_based());
        assert!(!engine.supports_ssh_tunnel());
        assert!(!engine.requires_authentication());
        assert_eq!(engine.default_port(), None);
        assert_eq!(engine.url_scheme(), "duckdb");
        assert_eq!(engine.display_name(), "DuckDB");
        assert!(engine.sql_functions().contains(&"READ_PARQUET"));
        assert_eq!(DatabaseType::from_scheme("duckdb").unwrap(), engine);
        assert_eq!(
            metadata_table_parts_probe("main.people"),
            (Some("main".into()), "people".into())
        );
    }

    fn metadata_table_parts_probe(name: &str) -> (Option<String>, String) {
        crate::model::metadata_table_parts(name, &DatabaseType::DuckDB)
    }

    #[test]
    fn test_duckdb_errors_say_duckdb_and_redact_secrets() {
        let error = DatabaseError::driver(
            DatabaseType::DuckDB,
            std::io::Error::other("catalog postgres://user:hunter2@db/x unreachable"),
        );
        let text = error.to_string();
        assert!(text.starts_with("DuckDB error: "), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        // the file engines keep their DataFusion prefix
        let files =
            DatabaseError::driver(DatabaseType::Parquet, std::io::Error::other("bad footer"));
        assert_eq!(files.to_string(), "DataFusion error: bad footer");
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
