//! Test fixtures: a temp state directory, an in-memory secret store (never
//! the real keychain), a SQLite database with known rows and a gateway.

use super::admin::{Admin, EnrollOutcome, EnrollRequest};
use super::audit::AuditLog;
use super::gateway::{Gateway, RequestContext};
use super::registry::Registry;
use dbcrust_agent_protocol::{ClientMessage, ServerMessage};
use dbcrust_secrets::SecretStore;
use std::path::PathBuf;
use std::sync::Arc;

#[doc(hidden)]
pub struct Harness {
    pub dir: tempfile::TempDir,
    pub db_path: PathBuf,
    pub store: Arc<SecretStore>,
    pub admin: Admin,
    pub registry: Arc<Registry>,
    pub audit: Arc<AuditLog>,
    pub gateway: Arc<Gateway>,
}

pub fn ctx() -> RequestContext {
    RequestContext {
        client_label: "tests".to_string(),
        peer_uid: Some(4242),
        peer_pid: Some(4343),
    }
}

impl Harness {
    /// `items(id INTEGER PRIMARY KEY, name TEXT, note TEXT)` with 50 rows.
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data.db");
        {
            use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
            let options = SqliteConnectOptions::new()
                .filename(&db_path)
                .create_if_missing(true);
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL, note TEXT)",
            )
            .execute(&pool)
            .await
            .unwrap();
            for i in 1..=50 {
                let note: Option<String> = (i % 5 != 0).then(|| format!("note-{i}"));
                sqlx::query("INSERT INTO items (id, name, note) VALUES (?, ?, ?)")
                    .bind(i)
                    .bind(format!("item-{i}"))
                    .bind(note)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            pool.close().await;
        }
        let store = Arc::new(SecretStore::in_memory());
        let aliases = dir.path().join("state").join("aliases.toml");
        let admin = Admin::new(&aliases, store.clone());
        let registry = Arc::new(Registry::new(aliases, store.clone()));
        let audit = Arc::new(AuditLog::new(dir.path().join("state").join("audit.jsonl")));
        let gateway = Arc::new(Gateway::new(registry.clone(), audit.clone()));
        Self {
            dir,
            db_path,
            store,
            admin,
            registry,
            audit,
            gateway,
        }
    }

    pub fn sqlite_url(&self) -> String {
        format!("sqlite://{}", self.db_path.display())
    }

    pub async fn enroll(&self, request: EnrollRequest) -> EnrollOutcome {
        self.admin.enroll(&request, None).await.unwrap()
    }

    pub async fn enroll_sqlite(&self, alias: &str, allow_writes: bool) -> EnrollOutcome {
        self.enroll(EnrollRequest {
            alias: alias.to_string(),
            url: self.sqlite_url(),
            allow_writes,
            description: Some("test database".to_string()),
            ..Default::default()
        })
        .await
    }

    pub fn audit_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.audit.path())
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    pub async fn send(&self, message: ClientMessage) -> ServerMessage {
        self.gateway.handle(&ctx(), message).await
    }

    pub async fn query(&self, alias: &str, sql: &str) -> ServerMessage {
        self.query_with(alias, sql, None, None).await
    }

    pub async fn query_with(
        &self,
        alias: &str,
        sql: &str,
        max_rows: Option<u32>,
        timeout_secs: Option<u32>,
    ) -> ServerMessage {
        self.send(ClientMessage::Query {
            id: 7,
            alias: alias.to_string(),
            sql: sql.to_string(),
            max_rows,
            timeout_secs,
        })
        .await
    }

    /// Rows of an `items` count, via a direct (non-broker) connection.
    pub async fn count_items(&self) -> i64 {
        use sqlx::Row;
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let options = SqliteConnectOptions::new().filename(&self.db_path);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let row = sqlx::query("SELECT COUNT(*) AS n FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        let n: i64 = row.get("n");
        pool.close().await;
        n
    }
}

/// Unwrap a `query_result`.
pub fn rows_of(message: &ServerMessage) -> (Vec<String>, Vec<Vec<Option<String>>>, bool) {
    match message {
        ServerMessage::QueryResult {
            columns,
            rows,
            truncated,
            ..
        } => (columns.clone(), rows.clone(), *truncated),
        other => panic!("expected query_result, got {other:?}"),
    }
}

/// Unwrap an `error` into its code.
pub fn error_code(message: &ServerMessage) -> dbcrust_agent_protocol::ErrorCode {
    match message {
        ServerMessage::Error { code, .. } => *code,
        other => panic!("expected error, got {other:?}"),
    }
}
