//! Data crossing the worker → UI boundary. All types are owned and `Send`.

use crate::ids::ConnectionId;
use crate::store::RecentEntry;
use dbcrust::database::{DatabaseType, DatabaseTypeExt};
use std::time::Duration;

/// What to connect to.
#[derive(Clone)]
pub enum ConnectTarget {
    /// Any URL DBCrust understands: `postgres://`, `mysql://`, `sqlite://`,
    /// `docker://`, `vault://role@mount/db`, file paths, …
    Url(String),
    /// A saved session, by name.
    Session(String),
    /// A recent connection (its URL is rebuilt on the worker, which may look
    /// up a password in `.dbcrust`/pgpass/`.my.cnf` or run a command).
    Recent(Box<RecentEntry>),
    /// Fully resolved connection details (credentials and options included),
    /// e.g. another database of an open connection's server.
    Info(Box<dbcrust::database::ConnectionInfo>),
}

// A URL target can carry a password: `Debug` shows it redacted.
impl std::fmt::Debug for ConnectTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => f
                .debug_tuple("Url")
                .field(&dbcrust::password_sanitizer::redact_url(url))
                .finish(),
            Self::Session(name) => f.debug_tuple("Session").field(name).finish(),
            Self::Recent(entry) => f.debug_tuple("Recent").field(entry).finish(),
            Self::Info(info) => f.debug_tuple("Info").field(info).finish(),
        }
    }
}

/// A connection request from the UI.
#[derive(Debug, Clone)]
pub struct ConnectRequest {
    pub target: ConnectTarget,
    /// Open the connection read-only: hardens the connection where the
    /// engine supports it (PostgreSQL `default_transaction_read_only`,
    /// SQLite `query_only`) and enables DBCrust's write-statement guard.
    pub read_only: bool,
    /// Vault address override for `vault://` targets.
    pub vault_addr: Option<String>,
}

impl ConnectRequest {
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            target: ConnectTarget::Url(url.into()),
            read_only: false,
            vault_addr: None,
        }
    }

    pub fn session(name: impl Into<String>) -> Self {
        Self {
            target: ConnectTarget::Session(name.into()),
            read_only: false,
            vault_addr: None,
        }
    }

    pub fn info(info: dbcrust::database::ConnectionInfo) -> Self {
        Self {
            target: ConnectTarget::Info(Box::new(info)),
            read_only: false,
            vault_addr: None,
        }
    }

    pub fn recent(entry: RecentEntry) -> Self {
        Self {
            target: ConnectTarget::Recent(Box::new(entry)),
            read_only: false,
            vault_addr: None,
        }
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn vault_addr(mut self, vault_addr: Option<String>) -> Self {
        self.vault_addr = vault_addr;
        self
    }
}

/// Identity and display data of a live connection.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionSummary {
    pub id: ConnectionId,
    pub db_type: DatabaseType,
    /// Current database (or file name for file-based engines).
    pub database: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Connection URL with the password redacted.
    pub display_url: String,
    /// Short human label: the session name, or `database` (`host`).
    pub label: String,
    /// The saved session this connection was opened from.
    pub session_name: Option<String>,
    /// Opened read-only (hardened + guarded).
    pub read_only: bool,
}

impl ConnectionSummary {
    pub fn type_name(&self) -> &'static str {
        self.db_type.display_name()
    }

    /// File-backed engines have no host/port/user.
    pub fn is_file_based(&self) -> bool {
        matches!(
            self.db_type,
            DatabaseType::SQLite
                | DatabaseType::Parquet
                | DatabaseType::CSV
                | DatabaseType::JSON
                | DatabaseType::DuckDB
        )
    }

    /// Where this connection points, for subtitles and the status bar:
    /// `user@host:port`, a file path, `docker://…`, or `vault://…`.
    pub fn target(&self) -> String {
        if self.display_url.starts_with("vault://") || self.display_url.starts_with("docker://") {
            return self.display_url.clone();
        }
        if self.is_file_based() {
            return strip_scheme(&self.display_url).to_string();
        }
        let user = if self.user.is_empty() {
            String::new()
        } else {
            format!("{}@", self.user)
        };
        if self.port == 0 {
            format!("{user}{}", self.host)
        } else {
            format!("{user}{}:{}", self.host, self.port)
        }
    }

    /// The engine cancels a running statement server-side. Other engines can
    /// only stop waiting for it.
    pub fn supports_server_cancel(&self) -> bool {
        matches!(self.db_type, DatabaseType::PostgreSQL)
    }

    /// `EXPLAIN ANALYZE` with a structured (JSON) plan.
    pub fn supports_explain_analyze(&self) -> bool {
        matches!(self.db_type, DatabaseType::PostgreSQL)
    }

    /// Input is not SQL split on `;` (MongoDB shell syntax): run the
    /// selection or the whole buffer as one unit.
    pub fn runs_unsplit(&self) -> bool {
        matches!(self.db_type, DatabaseType::MongoDB)
    }

    /// Engines with more than one database the user can switch to.
    pub fn supports_database_switch(&self) -> bool {
        matches!(
            self.db_type,
            DatabaseType::PostgreSQL | DatabaseType::MySQL | DatabaseType::ClickHouse
        )
    }
}

fn strip_scheme(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // `sqlite:///abs/path` keeps its leading slash
    rest
}

/// Rows returned by one statement. Cells are the engine's text rendering;
/// `None` is SQL NULL (distinct from an empty string).
#[derive(Debug, Clone, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub elapsed: Duration,
    /// `Some(n)` when DBCrust appended `LIMIT n` to the statement
    /// (`default_limit`), i.e. more rows may exist.
    pub auto_limit: Option<usize>,
    /// The statement as submitted (before any automatic LIMIT).
    pub sql: String,
}

impl ResultSet {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The limit was applied and actually reached: the result is likely
    /// truncated.
    pub fn is_truncated(&self) -> bool {
        self.auto_limit
            .is_some_and(|limit| self.rows.len() >= limit)
    }
}

/// Raw EXPLAIN output, as returned by the engine (header row first).
#[derive(Debug, Clone, PartialEq)]
pub struct ExplainRaw {
    pub db_type: DatabaseType,
    pub sql: String,
    /// `rows[0]` is the header row.
    pub rows: Vec<Vec<String>>,
    /// Produced with `EXPLAIN ANALYZE` (actual times/rows present).
    pub analyzed: bool,
    pub elapsed: Duration,
}

/// Kind of schema object in the explorer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
    Collection,
    Index,
    Other(String),
}

impl TableKind {
    pub fn from_label(label: &str) -> Self {
        let lower = label.trim().to_lowercase();
        match lower.as_str() {
            "table" | "base table" | "base_table" | "r" | "" => TableKind::Table,
            "view" | "v" | "system view" => TableKind::View,
            "materialized view" | "materialized_view" | "m" => TableKind::MaterializedView,
            "foreign table" | "f" => TableKind::ForeignTable,
            "collection" => TableKind::Collection,
            "index" => TableKind::Index,
            _ => TableKind::Other(label.trim().to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            TableKind::Table => "table",
            TableKind::View => "view",
            TableKind::MaterializedView => "materialized view",
            TableKind::ForeignTable => "foreign table",
            TableKind::Collection => "collection",
            TableKind::Index => "index",
            TableKind::Other(label) => label,
        }
    }
}

/// One object in the schema explorer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableEntry {
    /// `None` for engines without schemas (SQLite, MongoDB, files, …).
    pub schema: Option<String>,
    pub name: String,
    pub kind: TableKind,
    pub owner: Option<String>,
}

impl TableEntry {
    /// `schema.name`, or `name` when there is no schema.
    pub fn qualified_name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Full description of a table, as returned by DBCrust core.
pub type TableDetails = dbcrust::db::TableDetails;

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn summary(
        db_type: DatabaseType,
        url: &str,
        host: &str,
        port: u16,
        user: &str,
    ) -> ConnectionSummary {
        ConnectionSummary {
            id: ConnectionId::next(),
            db_type,
            database: "db".into(),
            host: host.into(),
            port,
            user: user.into(),
            display_url: url.into(),
            label: "db".into(),
            session_name: None,
            read_only: false,
        }
    }

    #[rstest]
    #[case(
        DatabaseType::PostgreSQL,
        "postgres://u:[REDACTED]@h:5432/db",
        "h",
        5432,
        "u",
        "u@h:5432"
    )]
    #[case(
        DatabaseType::SQLite,
        "sqlite:///tmp/app.db",
        "unknown",
        0,
        "",
        "/tmp/app.db"
    )]
    #[case(
        DatabaseType::PostgreSQL,
        "docker://pg/app",
        "localhost",
        5432,
        "u",
        "docker://pg/app"
    )]
    #[case(
        DatabaseType::PostgreSQL,
        "vault://r@database/app",
        "h",
        5432,
        "v",
        "vault://r@database/app"
    )]
    fn target_formats(
        #[case] db_type: DatabaseType,
        #[case] url: &str,
        #[case] host: &str,
        #[case] port: u16,
        #[case] user: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(summary(db_type, url, host, port, user).target(), expected);
    }

    #[test]
    fn truncation_requires_reaching_the_limit() {
        let mut result = ResultSet {
            columns: vec!["a".into()],
            rows: vec![vec![Some("1".into())]; 3],
            elapsed: Duration::ZERO,
            auto_limit: Some(3),
            sql: "select a from t".into(),
        };
        assert!(result.is_truncated());
        result.auto_limit = Some(10);
        assert!(!result.is_truncated());
        result.auto_limit = None;
        assert!(!result.is_truncated());
    }

    #[test]
    fn table_kind_labels_round_trip() {
        assert_eq!(TableKind::from_label("BASE TABLE"), TableKind::Table);
        assert_eq!(TableKind::from_label("view"), TableKind::View);
        assert_eq!(
            TableKind::from_label("sequence"),
            TableKind::Other("sequence".into())
        );
    }
}
