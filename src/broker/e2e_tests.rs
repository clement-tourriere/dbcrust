//! End-to-end tests: real SQLite databases, the real gateway and, for the
//! protocol tests, a real Unix socket in a temp directory.

use super::admin::{EnrollRequest, TightenRequest};
use super::audit::AuditLog;
use super::gateway::{Gateway, connection_info_for};
use super::paths::{BrokerMode, BrokerSettings};
use super::registry::{AliasFile, Registry};
use super::server::{Server, ServerConfig, bind_socket};
use super::testkit::{Harness, error_code, rows_of};
use dbcrust_agent_protocol::aio::{read_frame, read_message, write_frame, write_message};
use dbcrust_agent_protocol::{
    Access, ClientMessage, ErrorCode, MAX_FRAME_LEN, PROTOCOL_VERSION, ServerMessage,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UnixStream;

// ---- gateway behaviour (no socket) ------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_connections_exposes_policy_but_no_credentials() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;
    let message = h.send(ClientMessage::ListConnections { id: 1 }).await;
    let ServerMessage::Connections { connections, .. } = &message else {
        panic!("{message:?}");
    };
    assert_eq!(connections.len(), 1);
    let c = &connections[0];
    assert_eq!(c.alias, "scratch");
    assert_eq!(c.engine, "sqlite");
    assert_eq!(c.access, Access::ReadOnly);
    assert!(c.native_read_only);
    assert_eq!(c.max_rows, 500);
    assert_eq!(c.timeout_secs, 30);
    assert_eq!(c.description.as_deref(), Some("test database"));

    // No credential-looking key or value anywhere in the serialized frame
    let json = serde_json::to_string(&message).unwrap();
    for needle in [
        "password",
        "secret",
        "secret_options",
        "token",
        "host",
        "user",
    ] {
        assert!(!json.to_lowercase().contains(needle), "{needle} in {json}");
    }
    assert!(!json.contains(h.db_path.to_str().unwrap()), "{json}");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_tables_and_describe_table() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;

    let tables = h
        .send(ClientMessage::ListTables {
            id: 2,
            alias: "scratch".into(),
        })
        .await;
    let ServerMessage::Tables { tables, id } = tables else {
        panic!("expected tables");
    };
    assert_eq!(id, 2);
    assert!(tables.iter().any(|t| t.name == "items"), "{tables:?}");

    let described = h
        .send(ClientMessage::DescribeTable {
            id: 3,
            alias: "scratch".into(),
            table: "items".into(),
        })
        .await;
    let ServerMessage::TableDescription { columns, table, .. } = described else {
        panic!("expected table_description");
    };
    assert_eq!(table, "items");
    let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "name", "note"]);
    assert!(columns[0].primary_key);
    assert!(!columns[1].primary_key);
    assert_eq!(columns[1].nullable, Some(false));

    // Names the metadata layer would interpolate into SQL are refused
    for bad in ["items; DROP TABLE items", "items'--", "a.b.c.d", "\"x"] {
        let bad = h
            .send(ClientMessage::DescribeTable {
                id: 4,
                alias: "scratch".into(),
                table: bad.into(),
            })
            .await;
        assert_eq!(error_code(&bad), ErrorCode::QueryError);
    }
    assert_eq!(h.count_items().await, 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn query_returns_rows_with_nulls_and_truncates_by_rows() {
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "small".into(),
        url: h.sqlite_url(),
        max_rows: Some(5),
        ..Default::default()
    })
    .await;
    let result = h
        .query("small", "SELECT id, name, note FROM items ORDER BY id")
        .await;
    let (columns, rows, truncated) = rows_of(&result);
    assert_eq!(columns, vec!["id", "name", "note"]);
    assert_eq!(rows.len(), 5);
    assert!(truncated);
    assert_eq!(rows[0][0].as_deref(), Some("1"));
    assert_eq!(rows[0][2].as_deref(), Some("note-1"));

    // exactly the limit: not truncated; NULL stays NULL
    let result = h
        .query(
            "small",
            "SELECT id, note FROM items WHERE id BETWEEN 1 AND 5 ORDER BY id",
        )
        .await;
    let (_, rows, truncated) = rows_of(&result);
    assert_eq!(rows.len(), 5);
    assert!(!truncated);
    assert_eq!(rows[4][1], None);
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_limits_and_comments_cannot_defeat_the_row_cap() {
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "small".into(),
        url: h.sqlite_url(),
        max_rows: Some(5),
        ..Default::default()
    })
    .await;
    for sql in [
        "SELECT * FROM items LIMIT 1000000",
        "SELECT * FROM items -- trailing comment",
        "SELECT * FROM items /* block */",
        "SELECT * FROM items;",
        "SELECT * FROM items LIMIT 1000 OFFSET 2",
        "SELECT id FROM items UNION ALL SELECT id FROM items",
    ] {
        let (_, rows, truncated) = rows_of(&h.query("small", sql).await);
        assert_eq!(rows.len(), 5, "{sql}");
        assert!(truncated, "{sql}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn byte_budget_truncates_the_result() {
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "tiny".into(),
        url: h.sqlite_url(),
        max_bytes: Some(120),
        ..Default::default()
    })
    .await;
    let (_, rows, truncated) = rows_of(
        &h.query("tiny", "SELECT id, name, note FROM items ORDER BY id")
            .await,
    );
    assert!(truncated);
    assert!(!rows.is_empty() && rows.len() < 20, "{} rows", rows.len());
    // the audit line records the byte count and the truncation
    let lines = h.audit_lines();
    let last = lines.last().unwrap();
    assert_eq!(last["truncated"], true);
    assert!(last["bytes"].as_u64().unwrap() <= 120);
}

#[tokio::test(flavor = "multi_thread")]
async fn client_limits_can_only_tighten() {
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "cap".into(),
        url: h.sqlite_url(),
        max_rows: Some(10),
        ..Default::default()
    })
    .await;
    // asking for more than the ceiling changes nothing
    let (_, rows, truncated) = rows_of(
        &h.query_with("cap", "SELECT id FROM items", Some(10_000), Some(600))
            .await,
    );
    assert_eq!(rows.len(), 10);
    assert!(truncated);
    // asking for less is honoured
    let (_, rows, truncated) = rows_of(
        &h.query_with("cap", "SELECT id FROM items", Some(3), None)
            .await,
    );
    assert_eq!(rows.len(), 3);
    assert!(truncated);
    // zero cannot mean "unlimited"
    let (_, rows, _) = rows_of(
        &h.query_with("cap", "SELECT id FROM items", Some(0), None)
            .await,
    );
    assert_eq!(rows.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_alias_refuses_writes_and_data_is_untouched() {
    let h = Harness::new().await;
    h.enroll_sqlite("ro", false).await;
    for sql in [
        "INSERT INTO items (id, name) VALUES (1000, 'x')",
        "UPDATE items SET name = 'x'",
        "DELETE FROM items",
        "REPLACE INTO items (id, name) VALUES (1, 'x')",
        "WITH d AS (SELECT 1) DELETE FROM items WHERE id IN (SELECT * FROM d)",
    ] {
        let response = h.query("ro", sql).await;
        assert_eq!(error_code(&response), ErrorCode::ReadOnlyViolation, "{sql}");
    }
    assert_eq!(h.count_items().await, 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn write_enabled_alias_allows_dml_but_never_ddl() {
    let h = Harness::new().await;
    h.enroll_sqlite("rw", true).await;
    let list = h.send(ClientMessage::ListConnections { id: 1 }).await;
    let ServerMessage::Connections { connections, .. } = list else {
        panic!()
    };
    assert_eq!(connections[0].access, Access::ReadWrite);
    assert!(!connections[0].native_read_only);

    let inserted = h
        .query(
            "rw",
            "INSERT INTO items (id, name) VALUES (1000, 'written')",
        )
        .await;
    assert!(
        matches!(inserted, ServerMessage::QueryResult { .. }),
        "{inserted:?}"
    );
    assert_eq!(h.count_items().await, 51);

    for sql in [
        "DROP TABLE items",
        "CREATE TABLE evil (a int)",
        "ALTER TABLE items ADD COLUMN x int",
        "ATTACH DATABASE '/tmp/other.db' AS other",
        "PRAGMA journal_mode = DELETE",
        "VACUUM",
    ] {
        let response = h.query("rw", sql).await;
        assert_eq!(error_code(&response), ErrorCode::DdlRefused, "{sql}");
    }
    assert_eq!(h.count_items().await, 51);
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_statement_and_malformed_input_is_refused() {
    let h = Harness::new().await;
    h.enroll_sqlite("rw", true).await;
    for sql in [
        "SELECT 1; DELETE FROM items",
        "SELECT 1;SELECT 2",
        "SELECT 'x'; -- \nDROP TABLE items",
    ] {
        let response = h.query("rw", sql).await;
        assert_eq!(
            error_code(&response),
            ErrorCode::MultiStatementRefused,
            "{sql}"
        );
    }
    // a semicolon inside a literal or comment is not a second statement
    for sql in [
        "SELECT 'a;b' AS v",
        "SELECT 1 /* ; */",
        "SELECT 1 -- ;",
        "SELECT 1;",
    ] {
        assert!(
            matches!(h.query("rw", sql).await, ServerMessage::QueryResult { .. }),
            "{sql}"
        );
    }
    assert_eq!(error_code(&h.query("rw", "").await), ErrorCode::QueryError);
    assert_eq!(
        error_code(&h.query("rw", "SELECT 'unterminated").await),
        ErrorCode::QueryError
    );
    assert_eq!(h.count_items().await, 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_invalid_aliases_are_unknown() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;
    for alias in ["nope", "../scratch", "SCRATCH", "scratch ", ""] {
        let response = h.query(alias, "SELECT 1").await;
        assert_eq!(error_code(&response), ErrorCode::UnknownAlias, "{alias:?}");
    }
    let response = h
        .send(ClientMessage::ListTables {
            id: 1,
            alias: "nope".into(),
        })
        .await;
    assert_eq!(error_code(&response), ErrorCode::UnknownAlias);
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_statement_times_out_and_the_connection_recovers() {
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "short".into(),
        url: h.sqlite_url(),
        timeout_secs: Some(1),
        ..Default::default()
    })
    .await;
    let started = Instant::now();
    let response = h
        .query(
            "short",
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) \
             SELECT count(*) FROM c",
        )
        .await;
    assert_eq!(error_code(&response), ErrorCode::Timeout, "{response:?}");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "took {:?}",
        started.elapsed()
    );
    // The next statement runs on a fresh connection
    let (_, rows, _) = rows_of(&h.query("short", "SELECT COUNT(*) FROM items").await);
    assert_eq!(rows[0][0].as_deref(), Some("50"));
}

#[tokio::test(flavor = "multi_thread")]
async fn explain_never_analyzes_and_follows_the_same_rules() {
    let h = Harness::new().await;
    h.enroll_sqlite("ro", false).await;
    let plan = h
        .send(ClientMessage::Explain {
            id: 1,
            alias: "ro".into(),
            sql: "SELECT * FROM items WHERE id = 1".into(),
        })
        .await;
    let (_, rows, _) = rows_of(&plan);
    assert!(!rows.is_empty());

    // DML under a read-only alias, DDL, multi-statement, EXPLAIN of EXPLAIN
    for (sql, code) in [
        ("DELETE FROM items", ErrorCode::ReadOnlyViolation),
        ("DROP TABLE items", ErrorCode::DdlRefused),
        ("SELECT 1; SELECT 2", ErrorCode::MultiStatementRefused),
        ("EXPLAIN SELECT 1", ErrorCode::QueryError),
    ] {
        let response = h
            .send(ClientMessage::Explain {
                id: 2,
                alias: "ro".into(),
                sql: sql.into(),
            })
            .await;
        assert_eq!(error_code(&response), code, "{sql}");
    }
    assert_eq!(h.count_items().await, 50);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_and_tightening_take_effect_without_a_restart() {
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "scratch".into(),
        url: h.sqlite_url(),
        allow_writes: true,
        ..Default::default()
    })
    .await;
    assert!(matches!(
        h.query("scratch", "SELECT 1").await,
        ServerMessage::QueryResult { .. }
    ));

    // tighten: read-only and 3 rows
    let policy = h
        .admin
        .tighten(
            "scratch",
            TightenRequest {
                max_rows: Some(3),
                read_only: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(policy.max_rows, 3);
    let response = h.query("scratch", "DELETE FROM items").await;
    assert_eq!(error_code(&response), ErrorCode::ReadOnlyViolation);
    let (_, rows, _) = rows_of(&h.query("scratch", "SELECT id FROM items").await);
    assert_eq!(rows.len(), 3);

    // loosening is refused: it needs a re-enrollment
    for request in [
        TightenRequest {
            max_rows: Some(100),
            ..Default::default()
        },
        TightenRequest {
            timeout_secs: Some(31),
            ..Default::default()
        },
        TightenRequest {
            max_bytes: Some(u64::MAX),
            ..Default::default()
        },
    ] {
        let error = h.admin.tighten("scratch", request).unwrap_err();
        assert!(error.to_string().contains("re-enroll"), "{error}");
    }

    // revoke
    assert!(h.admin.revoke("scratch").unwrap());
    let response = h.query("scratch", "SELECT 1").await;
    assert_eq!(error_code(&response), ErrorCode::UnknownAlias);
    assert!(!h.admin.revoke("scratch").unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn re_enrolling_bumps_the_revision_and_replaces_the_policy() {
    let h = Harness::new().await;
    let first = h.enroll_sqlite("scratch", false).await;
    assert_eq!(first.revision, 1);
    assert!(!first.replaced);
    let second = h.enroll_sqlite("scratch", true).await;
    assert_eq!(second.revision, 2);
    assert!(second.replaced);
    let inserted = h
        .query("scratch", "INSERT INTO items (id, name) VALUES (2000, 'x')")
        .await;
    assert!(
        matches!(inserted, ServerMessage::QueryResult { .. }),
        "{inserted:?}"
    );
}

// ---- binding and tampering ---------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn aliases_toml_cannot_change_the_target_or_the_policy() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;

    // Another database the descriptor tries to redirect to
    let other = h.dir.path().join("other.db");
    std::fs::write(&other, b"").unwrap();

    let path = h.admin.aliases_path.clone();
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace(
        "engine =",
        &format!(
            "host = \"evil.example\"\nport = 5432\ndatabase = \"{}\"\nfile_path = \"{}\"\naccess = \"read_write\"\nmax_rows = 100000\nengine =",
            other.display(),
            other.display()
        ),
    );
    assert_ne!(text, tampered);
    std::fs::write(&path, tampered).unwrap();

    // Still reads the enrolled database, still read-only, still capped
    let (_, rows, _) = rows_of(&h.query("scratch", "SELECT COUNT(*) FROM items").await);
    assert_eq!(rows[0][0].as_deref(), Some("50"));
    assert_eq!(
        error_code(&h.query("scratch", "DELETE FROM items").await),
        ErrorCode::ReadOnlyViolation
    );
    let resolved = h.registry.resolve("scratch").unwrap();
    let info = connection_info_for(&resolved, true, 30).unwrap();
    assert_eq!(
        info.file_path.as_deref(),
        Some(std::fs::canonicalize(&h.db_path).unwrap().to_str().unwrap())
    );
    assert_eq!(info.host, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_descriptor_engine_is_denied() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;
    let path = h.admin.aliases_path.clone();
    let mut file = AliasFile::load(&path).unwrap();
    let mut entry = file.get("scratch").unwrap().clone();
    entry.engine = "postgresql".to_string();
    file.upsert(entry);
    file.save(&path).unwrap();
    let response = h.query("scratch", "SELECT 1").await;
    assert_eq!(error_code(&response), ErrorCode::Denied);
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_is_built_from_the_record_only() {
    use crate::secrets::{AgentPolicy, SecretRecord, TargetBinding};
    use std::collections::BTreeMap;
    use zeroize::Zeroizing;

    let mut options = BTreeMap::new();
    options.insert(
        "sslpassword".to_string(),
        Zeroizing::new("keypass".to_string()),
    );
    // A record whose options try to smuggle internal switches in
    options.insert(
        crate::database::OPT_HARDEN_READ_ONLY.to_string(),
        Zeroizing::new("0".to_string()),
    );
    options.insert(
        "password_command".to_string(),
        Zeroizing::new("echo hacked".to_string()),
    );
    let mut record = SecretRecord::new(
        "s3cret",
        options,
        TargetBinding::new(
            "postgres",
            "db.good",
            Some(6432),
            "app",
            "agent",
            Some("verify-full"),
        ),
    );
    record.agent_policy = Some(AgentPolicy::default());
    let resolved = super::registry::ResolvedAlias {
        alias: "prod".into(),
        entry: super::registry::AliasEntry {
            alias: "prod".into(),
            record_id: "broker:prod".into(),
            engine: "mysql".into(), // descriptor lies; the record wins
            description: None,
            enrolled_at: String::new(),
            revision: 1,
        },
        policy: record.agent_policy.unwrap(),
        record,
    };
    let info = connection_info_for(&resolved, true, 12).unwrap();
    assert_eq!(info.host.as_deref(), Some("db.good"));
    assert_eq!(info.port, Some(6432));
    assert_eq!(info.database.as_deref(), Some("app"));
    assert_eq!(info.username.as_deref(), Some("agent"));
    assert_eq!(info.password.as_deref(), Some("s3cret"));
    assert_eq!(
        info.options.get("sslmode").map(String::as_str),
        Some("verify-full")
    );
    assert!(info.use_tls);
    assert_eq!(
        info.options
            .get(crate::database::OPT_HARDEN_READ_ONLY)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        info.options
            .get(crate::database::OPT_HARDEN_TIMEOUT_SECS)
            .map(String::as_str),
        Some("12")
    );
    assert!(!info.options.contains_key("password_command"));
    assert_eq!(
        info.database_type,
        crate::database::DatabaseType::PostgreSQL
    );
}

// ---- audit ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn audit_records_every_decision_without_sql_text() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;
    let secret_literal = "canary-literal-8472";
    h.query("scratch", &format!("SELECT '{secret_literal}' AS v"))
        .await;
    h.query("scratch", "DROP TABLE items").await;
    h.query("scratch", "SELECT nope FROM nowhere").await;
    h.query("missing", "SELECT 1").await;

    let text = std::fs::read_to_string(h.audit.path()).unwrap();
    assert!(!text.contains(secret_literal));
    assert!(!text.contains("DROP TABLE"));
    let lines = h.audit_lines();

    let allowed_start = lines
        .iter()
        .find(|l| l["phase"] == "start" && l["op"] == "query")
        .unwrap();
    assert_eq!(allowed_start["client_label"], "tests");
    assert_eq!(allowed_start["peer_uid"], 4242);
    assert_eq!(allowed_start["peer_pid"], 4343);
    assert_eq!(allowed_start["alias"], "scratch");
    assert_eq!(allowed_start["decision"], "allowed");
    assert_eq!(
        allowed_start["sql_len"],
        format!("SELECT '{secret_literal}' AS v").len()
    );
    assert_eq!(allowed_start["sql_sha256"].as_str().unwrap().len(), 64);

    let end = lines
        .iter()
        .find(|l| l["phase"] == "end" && l["seq"] == allowed_start["seq"])
        .unwrap();
    assert_eq!(end["decision"], "allowed");
    assert_eq!(end["rows"], 1);
    assert_eq!(end["truncated"], false);
    assert!(end["duration_ms"].is_u64());

    let ddl = lines.iter().find(|l| l["code"] == "ddl_refused").unwrap();
    assert_eq!(ddl["decision"], "refused");
    assert_eq!(ddl["phase"], "end");
    // refused before execution: no start line shares its seq
    assert!(
        !lines
            .iter()
            .any(|l| l["phase"] == "start" && l["seq"] == ddl["seq"])
    );

    assert!(lines.iter().any(|l| l["code"] == "unknown_alias"));
    assert!(
        lines
            .iter()
            .any(|l| l["code"] == "query_error" && l["decision"] == "error")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unwritable_audit_log_fails_closed() {
    let h = Harness::new().await;
    h.enroll_sqlite("rw", true).await;

    // A gateway whose audit file lives under a regular file
    let blocker = h.dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let broken_audit = Arc::new(AuditLog::new(blocker.join("audit.jsonl")));
    let gateway = Gateway::new(h.registry.clone(), broken_audit);

    let response = gateway
        .handle(
            &super::testkit::ctx(),
            ClientMessage::Query {
                id: 9,
                alias: "rw".into(),
                sql: "INSERT INTO items (id, name) VALUES (3000, 'sneaky')".into(),
                max_rows: None,
                timeout_secs: None,
            },
        )
        .await;
    assert_eq!(error_code(&response), ErrorCode::Internal);
    // Nothing ran
    assert_eq!(h.count_items().await, 50);

    // reads, listings and metadata are refused as well
    for message in [
        ClientMessage::ListConnections { id: 1 },
        ClientMessage::ListTables {
            id: 2,
            alias: "rw".into(),
        },
    ] {
        let response = gateway.handle(&super::testkit::ctx(), message).await;
        assert_eq!(error_code(&response), ErrorCode::Internal);
    }
}

// ---- queueing --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn requests_beyond_the_per_alias_queue_get_limit_exceeded() {
    let h = Arc::new(Harness::new().await);
    h.enroll(EnrollRequest {
        alias: "busy".into(),
        url: h.sqlite_url(),
        timeout_secs: Some(2),
        ..Default::default()
    })
    .await;
    // Warm the connection
    h.query("busy", "SELECT 1").await;

    let slow =
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let h = h.clone();
        tasks.push(tokio::spawn(async move { h.query("busy", slow).await }));
    }
    let mut limited = 0;
    for task in tasks {
        if let ServerMessage::Error {
            code: ErrorCode::LimitExceeded,
            ..
        } = task.await.unwrap()
        {
            limited += 1;
        }
    }
    assert!(limited >= 1, "expected some requests to be shed");
}

// ---- the socket ------------------------------------------------------------------

struct RunningBroker {
    path: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    harness: Harness,
}

impl RunningBroker {
    async fn start(harness: Harness, max_connections: usize) -> Self {
        let path = harness.dir.path().join("broker.sock");
        let listener = bind_socket(&path, BrokerMode::User, &BrokerSettings::default()).unwrap();
        let server = Server::new(
            harness.gateway.clone(),
            ServerConfig {
                mode: BrokerMode::User,
                // SAFETY: geteuid cannot fail
                own_uid: unsafe { libc::geteuid() },
                settings: BrokerSettings::default(),
                max_connections,
            },
        );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            server
                .serve(listener, async {
                    let _ = stopped.await;
                })
                .await;
        });
        Self {
            path,
            stop: Some(stop),
            task,
            harness,
        }
    }

    async fn connect(&self) -> UnixStream {
        UnixStream::connect(&self.path).await.unwrap()
    }

    async fn handshake(&self, client: &str) -> UnixStream {
        let mut stream = self.connect().await;
        write_message(
            &mut stream,
            &ClientMessage::Hello {
                protocol: PROTOCOL_VERSION,
                client: client.to_string(),
            },
        )
        .await
        .unwrap();
        let welcome: ServerMessage = read_message(&mut stream).await.unwrap();
        assert!(matches!(
            welcome,
            ServerMessage::Welcome { protocol: 1, .. }
        ));
        stream
    }

    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = self.task.await;
        self.harness.gateway.close_all().await;
    }
}

async fn request(stream: &mut UnixStream, message: &ClientMessage) -> ServerMessage {
    write_message(stream, message).await.unwrap();
    read_message(stream).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_conversation_end_to_end() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;
    let broker = RunningBroker::start(h, 8).await;

    let mut stream = broker.handshake("e2e-test").await;

    let listed = request(&mut stream, &ClientMessage::ListConnections { id: 1 }).await;
    let ServerMessage::Connections { id, connections } = listed else {
        panic!();
    };
    assert_eq!(id, 1);
    assert_eq!(connections[0].alias, "scratch");
    assert!(connections[0].native_read_only);

    let result = request(
        &mut stream,
        &ClientMessage::Query {
            id: 2,
            alias: "scratch".into(),
            sql: "SELECT id FROM items ORDER BY id".into(),
            max_rows: Some(4),
            timeout_secs: None,
        },
    )
    .await;
    assert_eq!(result.id(), Some(2));
    let (_, rows, truncated) = rows_of(&result);
    assert_eq!(rows.len(), 4);
    assert!(truncated);

    let refused = request(
        &mut stream,
        &ClientMessage::Query {
            id: 3,
            alias: "scratch".into(),
            sql: "INSERT INTO items (id, name) VALUES (9000, 'x')".into(),
            max_rows: None,
            timeout_secs: None,
        },
    )
    .await;
    assert_eq!(refused.id(), Some(3));
    assert_eq!(error_code(&refused), ErrorCode::ReadOnlyViolation);

    let unknown = request(
        &mut stream,
        &ClientMessage::Query {
            id: 4,
            alias: "nope".into(),
            sql: "SELECT 1".into(),
            max_rows: None,
            timeout_secs: None,
        },
    )
    .await;
    assert_eq!(unknown.id(), Some(4));
    assert_eq!(error_code(&unknown), ErrorCode::UnknownAlias);

    // The audit line carries the client label from the hello
    let labels: Vec<String> = broker
        .harness
        .audit_lines()
        .iter()
        .map(|l| l["client_label"].as_str().unwrap().to_string())
        .collect();
    assert!(labels.iter().all(|l| l == "e2e-test"), "{labels:?}");
    let uid = unsafe { libc::geteuid() };
    assert!(
        broker
            .harness
            .audit_lines()
            .iter()
            .all(|l| l["peer_uid"] == uid)
    );

    drop(stream);
    broker.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn version_mismatch_is_refused_and_closed() {
    let broker = RunningBroker::start(Harness::new().await, 8).await;
    let mut stream = broker.connect().await;
    write_message(
        &mut stream,
        &ClientMessage::Hello {
            protocol: 99,
            client: "future".into(),
        },
    )
    .await
    .unwrap();
    let response: ServerMessage = read_message(&mut stream).await.unwrap();
    assert!(matches!(
        response,
        ServerMessage::Error {
            id: None,
            code: ErrorCode::UnsupportedVersion,
            ..
        }
    ));
    // closed afterwards
    assert!(read_frame(&mut stream).await.is_err());
    broker.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_message_must_be_hello() {
    let broker = RunningBroker::start(Harness::new().await, 8).await;
    let mut stream = broker.connect().await;
    write_message(&mut stream, &ClientMessage::ListConnections { id: 1 })
        .await
        .unwrap();
    let response: ServerMessage = read_message(&mut stream).await.unwrap();
    assert!(matches!(
        response,
        ServerMessage::Error {
            id: None,
            code: ErrorCode::ProtocolError,
            ..
        }
    ));
    broker.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn oversize_frame_is_rejected_before_its_body_is_read() {
    let broker = RunningBroker::start(Harness::new().await, 8).await;
    let mut stream = broker.handshake("big").await;
    use tokio::io::AsyncWriteExt;
    // Announce a body one byte over the limit and send none of it
    let header = ((MAX_FRAME_LEN as u32) + 1).to_be_bytes();
    stream.write_all(&header).await.unwrap();
    let response: ServerMessage = read_message(&mut stream).await.unwrap();
    assert!(matches!(
        response,
        ServerMessage::Error {
            code: ErrorCode::ProtocolError,
            ..
        }
    ));
    assert!(
        read_frame(&mut stream).await.is_err(),
        "connection must close"
    );
    broker.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_requests_get_protocol_errors_with_the_id_when_known() {
    let broker = RunningBroker::start(Harness::new().await, 8).await;
    let mut stream = broker.handshake("fuzz").await;

    write_frame(&mut stream, br#"{"type":"drop_database","id":41}"#)
        .await
        .unwrap();
    let response: ServerMessage = read_message(&mut stream).await.unwrap();
    assert!(matches!(
        response,
        ServerMessage::Error {
            id: Some(41),
            code: ErrorCode::ProtocolError,
            ..
        }
    ));

    write_frame(&mut stream, b"not json at all").await.unwrap();
    let response: ServerMessage = read_message(&mut stream).await.unwrap();
    assert!(matches!(
        response,
        ServerMessage::Error {
            id: None,
            code: ErrorCode::ProtocolError,
            ..
        }
    ));

    // A second hello is a protocol error, and the conversation continues
    write_message(
        &mut stream,
        &ClientMessage::Hello {
            protocol: 1,
            client: "again".into(),
        },
    )
    .await
    .unwrap();
    let response: ServerMessage = read_message(&mut stream).await.unwrap();
    assert_eq!(error_code(&response), ErrorCode::ProtocolError);
    let listed = request(&mut stream, &ClientMessage::ListConnections { id: 5 }).await;
    assert!(matches!(listed, ServerMessage::Connections { id: 5, .. }));
    broker.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_cap_sheds_extra_clients() {
    let broker = RunningBroker::start(Harness::new().await, 1).await;
    let _first = broker.handshake("first").await;
    let mut second = broker.connect().await;
    let response: ServerMessage = read_message(&mut second).await.unwrap();
    assert!(matches!(
        response,
        ServerMessage::Error {
            code: ErrorCode::LimitExceeded,
            ..
        }
    ));
    broker.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hostile_client_label_is_sanitized_in_the_audit_log() {
    let h = Harness::new().await;
    h.enroll_sqlite("scratch", false).await;
    let broker = RunningBroker::start(h, 8).await;
    let mut stream = broker
        .handshake("evil\n{\"forged\":\"line\"}\u{1b}[31m")
        .await;
    request(&mut stream, &ClientMessage::ListConnections { id: 1 }).await;
    let text = std::fs::read_to_string(broker.harness.audit.path()).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}"); // start + end
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(!value["client_label"].as_str().unwrap().contains('\n'));
        assert!(value.get("forged").is_none());
    }
    drop(stream);
    broker.stop().await;
}

// ---- service-mode storage ---------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn service_store_persists_records_in_an_encrypted_file() {
    use super::cli::open_store;
    use super::paths::BrokerPaths;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("dbcrust-broker");
    crate::secrets::fsutil::create_private_dir(&state).unwrap();
    let paths = BrokerPaths {
        mode: BrokerMode::Service,
        state_dir: state.clone(),
        socket: state.join("broker.sock"),
    };

    // A SQLite file to enroll
    let db = dir.path().join("svc.db");
    {
        use sqlx::sqlite::SqliteConnectOptions;
        let pool = sqlx::SqlitePool::connect_with(
            SqliteConnectOptions::new()
                .filename(&db)
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::query("CREATE TABLE t (a INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (1), (2)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    let admin = super::admin::Admin::new(paths.aliases_file(), open_store(&paths, true));
    admin
        .enroll(
            &EnrollRequest {
                alias: "svc".into(),
                url: format!("sqlite://{}", db.display()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

    // Nothing is world/group accessible and the state passes the check
    for name in ["aliases.toml", "secrets.enc"] {
        let mode = std::fs::metadata(state.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{name}");
    }
    assert!(
        super::paths::service_permission_violations(&paths, super::paths::effective_uid())
            .unwrap()
            .is_empty()
    );
    // The descriptor file names no path or credential
    let aliases = std::fs::read_to_string(state.join("aliases.toml")).unwrap();
    assert!(!aliases.contains("svc.db"));

    // A fresh store/registry/gateway (as after a restart) serves the alias
    let store = open_store(&paths, false);
    let registry = Arc::new(Registry::new(paths.aliases_file(), store));
    let audit = Arc::new(AuditLog::new(paths.audit_file()));
    let gateway = Gateway::new(registry, audit);
    let response = gateway
        .handle(
            &super::testkit::ctx(),
            ClientMessage::Query {
                id: 1,
                alias: "svc".into(),
                sql: "SELECT SUM(a) FROM t".into(),
                max_rows: None,
                timeout_secs: None,
            },
        )
        .await;
    let (_, rows, _) = rows_of(&response);
    assert_eq!(rows[0][0].as_deref(), Some("3"));
    let audit_mode = std::fs::metadata(paths.audit_file())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(audit_mode, 0o600);
}

// ---- CLI ------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn enroll_refuses_to_run_without_a_terminal() {
    use crate::cli::BrokerAction;
    let dir = tempfile::tempdir().unwrap();
    let action = BrokerAction::Enroll {
        alias: "prod".into(),
        url: "postgres://agent@db.example.com/app".into(),
        allow_writes: false,
        max_rows: None,
        timeout: None,
        max_bytes: None,
        description: None,
        mode: None,
        state_dir: Some(dir.path().join("state").display().to_string()),
        allow_untrusted_binary: false,
    };
    // interactive = false: no password may come from anywhere else
    assert_eq!(super::cli::run(Some(&action), None, false).await, 2);
    assert!(!dir.path().join("state").join("aliases.toml").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn enroll_rejects_forbidden_urls_before_prompting() {
    use crate::cli::BrokerAction;
    let dir = tempfile::tempdir().unwrap();
    for url in [
        "postgres://alice:pw@db.example.com/app",
        "postgres://alice@db.example.com/app?password_command=echo+x",
        "docker://pg/app",
        "vault://role@db/app",
        "session://prod",
        "mongodb://alice@db/app",
    ] {
        let action = BrokerAction::Enroll {
            alias: "prod".into(),
            url: url.into(),
            allow_writes: false,
            max_rows: None,
            timeout: None,
            max_bytes: None,
            description: None,
            mode: None,
            state_dir: Some(dir.path().join("state").display().to_string()),
            allow_untrusted_binary: false,
        };
        // `interactive = true` must not lead to a prompt: validation fails first
        assert_eq!(super::cli::run(Some(&action), None, true).await, 2, "{url}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_cli_refuses_allow_writes() {
    use crate::cli::BrokerAction;
    let action = BrokerAction::Policy {
        alias: "prod".into(),
        max_rows: None,
        timeout: None,
        max_bytes: None,
        read_only: false,
        allow_writes: true,
        mode: None,
        state_dir: None,
        allow_untrusted_binary: false,
    };
    assert_eq!(super::cli::run(Some(&action), None, false).await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn sqlite_enrollment_through_the_cli_needs_no_terminal() {
    use crate::cli::BrokerAction;
    let h = Harness::new().await;
    let state = h.dir.path().join("cli-state");
    let make = |alias: &str| BrokerAction::Enroll {
        alias: alias.into(),
        url: h.sqlite_url(),
        allow_writes: false,
        max_rows: Some(25),
        timeout: Some(5),
        max_bytes: None,
        description: Some("via cli".into()),
        mode: None,
        state_dir: Some(state.display().to_string()),
        allow_untrusted_binary: false,
    };
    assert_eq!(super::cli::run(Some(&make("cli-db")), None, false).await, 0);
    assert!(state.join("aliases.toml").exists());

    // Re-enrolling the same alias needs a terminal (the owner proof)
    assert_eq!(super::cli::run(Some(&make("cli-db")), None, false).await, 2);

    let list = BrokerAction::List {
        format: None,
        mode: None,
        state_dir: Some(state.display().to_string()),
        allow_untrusted_binary: false,
    };
    assert_eq!(super::cli::run(Some(&list), None, false).await, 0);
    let revoke = BrokerAction::Revoke {
        alias: "cli-db".into(),
        mode: None,
        state_dir: Some(state.display().to_string()),
        allow_untrusted_binary: false,
    };
    assert_eq!(super::cli::run(Some(&revoke), None, false).await, 0);
    assert_eq!(super::cli::run(Some(&revoke), None, false).await, 1);
}

// ---- PostgreSQL (gated on DATABASE_URL) -------------------------------------------

/// Native read-only behaviour and statement timeouts against a real server.
/// Skips silently without `DATABASE_URL` (same convention as the other
/// PostgreSQL integration tests); the role needs CREATE on its database.
#[tokio::test(flavor = "multi_thread")]
async fn postgres_native_read_only_and_statement_timeout() {
    use crate::secrets::{AgentAccess, AgentPolicy, RecordId, SecretRecord, TargetBinding};
    use std::collections::BTreeMap;

    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL is not set");
        return;
    };
    let info = crate::database::ConnectionInfo::parse_url(&database_url).unwrap();
    if info.database_type != crate::database::DatabaseType::PostgreSQL {
        eprintln!("skipping: DATABASE_URL is not PostgreSQL");
        return;
    }

    // Setup through a direct connection: a table and a SELECT-able function
    // that writes (the classifier cannot see inside it; only the native
    // read-only session can stop it)
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    for statement in [
        "DROP FUNCTION IF EXISTS dbcrust_broker_test_write()",
        "DROP TABLE IF EXISTS dbcrust_broker_test",
        "CREATE TABLE dbcrust_broker_test (a int)",
        "CREATE FUNCTION dbcrust_broker_test_write() RETURNS int LANGUAGE sql \
         AS 'INSERT INTO dbcrust_broker_test VALUES (1) RETURNING a'",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }

    let h = Harness::new().await;
    let binding = TargetBinding::new(
        "postgresql",
        info.host.as_deref().unwrap_or("localhost"),
        info.port,
        info.database.as_deref().unwrap_or("postgres"),
        info.username.as_deref().unwrap_or("postgres"),
        Some(info.options.get("sslmode").map_or("prefer", String::as_str)),
    );
    for (alias, access) in [
        ("pg-ro", AgentAccess::ReadOnly),
        ("pg-rw", AgentAccess::ReadWrite),
    ] {
        let mut record = SecretRecord::new(
            info.password.as_deref().unwrap_or_default(),
            BTreeMap::new(),
            binding.clone(),
        );
        record.agent_policy = Some(AgentPolicy {
            access,
            timeout_secs: 2,
            ..AgentPolicy::default()
        });
        h.store
            .put_record(&RecordId::broker(alias), &record)
            .unwrap();
        let mut file = AliasFile::load(&h.admin.aliases_path).unwrap();
        file.upsert(super::registry::AliasEntry {
            alias: alias.to_string(),
            record_id: RecordId::broker(alias),
            engine: "postgresql".to_string(),
            description: None,
            enrolled_at: "2026-01-01T00:00:00Z".to_string(),
            revision: 1,
        });
        file.save(&h.admin.aliases_path).unwrap();
    }

    // plain read works
    let (_, rows, _) = rows_of(&h.query("pg-ro", "SELECT 1 AS one").await);
    assert_eq!(rows[0][0].as_deref(), Some("1"));

    // the session starts read-only
    let (_, rows, _) = rows_of(&h.query("pg-ro", "SHOW default_transaction_read_only").await);
    assert_eq!(rows[0][0].as_deref(), Some("on"));

    // a statement the classifier allows but that writes is stopped natively
    let response = h.query("pg-ro", "SELECT dbcrust_broker_test_write()").await;
    assert_eq!(
        error_code(&response),
        ErrorCode::ReadOnlyViolation,
        "{response:?}"
    );

    // the write-enabled alias may run it
    let response = h.query("pg-rw", "SELECT dbcrust_broker_test_write()").await;
    assert!(
        matches!(response, ServerMessage::QueryResult { .. }),
        "{response:?}"
    );
    let written: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dbcrust_broker_test")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(written, 1);

    // statement_timeout is enforced server-side
    let started = Instant::now();
    let response = h
        .query_with("pg-ro", "SELECT pg_sleep(30)", None, Some(1))
        .await;
    assert_eq!(error_code(&response), ErrorCode::Timeout, "{response:?}");
    assert!(started.elapsed() < Duration::from_secs(8));

    // and the connection works again afterwards
    let (_, rows, _) = rows_of(&h.query("pg-ro", "SELECT 2").await);
    assert_eq!(rows[0][0].as_deref(), Some("2"));

    for statement in [
        "DROP FUNCTION IF EXISTS dbcrust_broker_test_write()",
        "DROP TABLE IF EXISTS dbcrust_broker_test",
    ] {
        let _ = sqlx::query(statement).execute(&pool).await;
    }
    pool.close().await;
    h.gateway.close_all().await;
}

// ---- the real thin client -------------------------------------------------------

/// `dbcrust-agent` lives in another package, so cargo cannot hand its path to
/// this crate's tests; use the sibling binary in the target directory when it
/// has been built (the smoke test script builds it) and skip otherwise.
fn agent_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let candidate = dir.join("dbcrust-agent");
    candidate.is_file().then_some(candidate)
}

#[tokio::test(flavor = "multi_thread")]
async fn real_agent_binary_against_the_broker() {
    let Some(agent) = agent_binary() else {
        eprintln!("skipping: target/debug/dbcrust-agent has not been built");
        return;
    };
    let h = Harness::new().await;
    h.enroll(EnrollRequest {
        alias: "scratch".into(),
        url: h.sqlite_url(),
        max_rows: Some(3),
        ..Default::default()
    })
    .await;
    let broker = RunningBroker::start(h, 8).await;
    let socket = broker.path.clone();

    let run = |args: Vec<String>| {
        let agent = agent.clone();
        let socket = socket.clone();
        async move {
            let output = tokio::process::Command::new(agent)
                .arg("--socket")
                .arg(&socket)
                .args(&args)
                .env_clear()
                .output()
                .await
                .unwrap();
            (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).into_owned(),
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
        }
    };

    let (code, stdout, stderr) = run(vec!["list".into()]).await;
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("scratch"), "{stdout}");

    let (code, stdout, stderr) = run(vec![
        "query".into(),
        "scratch".into(),
        "-c".into(),
        "SELECT id FROM items ORDER BY id".into(),
        "-o".into(),
        "json".into(),
    ])
    .await;
    assert_eq!(code, Some(0), "{stderr}");
    let envelope: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(envelope["row_count"], 3);
    assert_eq!(envelope["truncated"], true);

    let (code, _, stderr) = run(vec![
        "query".into(),
        "scratch".into(),
        "-c".into(),
        "DELETE FROM items".into(),
    ])
    .await;
    assert_ne!(code, Some(0));
    assert!(
        stderr.contains("read_only") || stderr.contains("read-only"),
        "{stderr}"
    );
    assert_eq!(broker.harness.count_items().await, 50);
    broker.stop().await;
}

/// Enrollment's test-connect against a real PostgreSQL (gated on DATABASE_URL;
/// the URL's password, when present, is supplied the way the prompt would).
#[tokio::test(flavor = "multi_thread")]
async fn postgres_enrollment_test_connects_and_stores_the_record() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL is not set");
        return;
    };
    let mut url = url::Url::parse(&database_url).unwrap();
    if url.scheme() != "postgres" && url.scheme() != "postgresql" {
        eprintln!("skipping: DATABASE_URL is not PostgreSQL");
        return;
    }
    let password = url.password().unwrap_or("x").to_string();
    url.set_password(None).unwrap();

    let h = Harness::new().await;
    let outcome = h
        .admin
        .enroll(
            &EnrollRequest {
                alias: "pg".into(),
                url: url.to_string(),
                ..Default::default()
            },
            Some(zeroize::Zeroizing::new(password)),
        )
        .await
        .unwrap();
    assert_eq!(outcome.engine, "postgresql");
    let (_, rows, _) = rows_of(&h.query("pg", "SELECT 41 + 1").await);
    assert_eq!(rows[0][0].as_deref(), Some("42"));
    h.gateway.close_all().await;
}
