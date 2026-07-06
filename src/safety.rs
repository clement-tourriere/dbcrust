//! Read-only statement classification, shared by the CLI `--read-only` guard
//! and the AI agent's tool loop.
//!
//! Best-effort by design: classification is textual, so a SELECT calling a
//! user-defined side-effecting function still passes. For hard guarantees use
//! a read-only database role or a replica. Connect-time hardening
//! (`PRAGMA query_only` on SQLite, `default_transaction_read_only` on
//! PostgreSQL) backs this guard where the driver supports it.

use crate::database::DatabaseType;

/// Marker carried by every [`ReadOnlyViolation`] message. One-shot exit-code
/// mapping string-matches on it because violations cross `Box<dyn Error>`
/// boundaries (same pattern as the "Column selection aborted" marker).
pub const READ_ONLY_VIOLATION_MARKER: &str = "read-only mode:";

/// A statement rejected by `--read-only`.
#[derive(Debug, thiserror::Error)]
#[error("read-only mode: {reason}")]
pub struct ReadOnlyViolation {
    pub reason: String,
}

/// True when an error message (possibly wrapped by other error types)
/// originates from a [`ReadOnlyViolation`].
pub fn is_read_only_violation_message(message: &str) -> bool {
    message.contains(READ_ONLY_VIOLATION_MARKER)
}

/// Reject `query` unless it is a read-only statement for `db_type`.
pub fn check_read_only(query: &str, db_type: &DatabaseType) -> Result<(), ReadOnlyViolation> {
    let verdict = match db_type {
        DatabaseType::MongoDB => check_mongodb(query),
        DatabaseType::Elasticsearch => check_elasticsearch(query),
        _ => check_sql(query),
    };
    verdict.map_err(|reason| ReadOnlyViolation { reason })
}

/// True when nothing follows the first `;` — i.e. the string is a single
/// statement (a trailing semicolon is fine).
fn is_single_statement(upper: &str) -> bool {
    !upper.split(';').skip(1).any(|rest| !rest.trim().is_empty())
}

fn check_sql(query: &str) -> Result<(), String> {
    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();

    // PRAGMA gets a finer rule than the agent guard: the read forms
    // (`PRAGMA name`, `PRAGMA name(arg)`) are allowed, the write form
    // (`PRAGMA name = value`) is not.
    if upper.starts_with("PRAGMA") {
        if !is_single_statement(&upper) {
            return Err("multi-statement PRAGMA batches are not allowed".to_string());
        }
        return if trimmed.contains('=') {
            Err("PRAGMA assignments modify database state".to_string())
        } else {
            Ok(())
        };
    }

    if !is_read_only_sql(trimmed) {
        return Err("only read-only statements are allowed".to_string());
    }
    if let Some(reason) = side_effect_guard(trimmed) {
        return Err(format!("statement {reason}"));
    }
    Ok(())
}

/// MongoDB commands are not SQL — allowlist the read verbs that
/// `execute_query` dispatches (see database_mongodb.rs). Unknown commands are
/// rejected: new write verbs must never pass by default.
fn check_mongodb(query: &str) -> Result<(), String> {
    const READ_METHODS: [&str; 12] = [
        "find",
        "findOne",
        "aggregate",
        "count",
        "countDocuments",
        "estimatedDocumentCount",
        "distinct",
        "getIndexes",
        "stats",
        "explain",
        "getCollectionNames",
        "getCollectionInfos",
    ];

    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();

    // SQL-to-Mongo translation path
    if upper.starts_with("SELECT") || upper.starts_with("SHOW") {
        return check_sql(trimmed);
    }
    if trimmed == "dbStats" || trimmed == "db.stats()" {
        return Ok(());
    }

    if let Some(rest) = trimmed.strip_prefix("db.") {
        // `aggregate` pipelines can write via $out / $merge stages
        if upper.contains("$OUT") || upper.contains("$MERGE") {
            return Err("aggregate stages $out/$merge write to a collection".to_string());
        }
        match rest.split('(').next() {
            // Bare `db.<collection>` (no call) is a read shorthand. Note the
            // method is the LAST dot-segment — collection names may contain
            // dots (`db.my.coll.find(...)`).
            Some(head) if !rest.contains('(') => {
                let _ = head;
                return Ok(());
            }
            Some(before_paren) => {
                let method = before_paren
                    .rsplit('.')
                    .next()
                    .unwrap_or(before_paren)
                    .trim();
                if READ_METHODS.contains(&method) {
                    return Ok(());
                }
                return Err(format!("MongoDB method '{method}' is not read-only"));
            }
            None => {}
        }
    }

    Err("only find/aggregate/count/distinct/getIndexes/stats commands are allowed".to_string())
}

/// Elasticsearch speaks SQL here (see database_elasticsearch.rs); its SQL
/// interface is read-only by design, so the prefix check is the whole rule.
fn check_elasticsearch(query: &str) -> Result<(), String> {
    const ALLOWED_PREFIXES: [&str; 5] = ["SELECT", "SHOW", "DESCRIBE", "DESC", "EXPLAIN"];
    let upper = query.trim().to_uppercase();
    if ALLOWED_PREFIXES.iter().any(|p| upper.starts_with(p)) {
        Ok(())
    } else {
        Err("only SELECT/SHOW/DESCRIBE/EXPLAIN queries are allowed".to_string())
    }
}

/// Check if SQL is likely a read-only query.
///
/// A prefix check alone is NOT enough: `WITH d AS (DELETE FROM t RETURNING *)
/// SELECT * FROM d` starts with WITH, and `SELECT 1; DROP TABLE t` starts
/// with SELECT. Conservative by design — false negatives only cost an extra
/// confirmation prompt (AI flow) or an explicit `--read-only=false` run.
pub fn is_read_only_sql(sql: &str) -> bool {
    let upper = sql.trim().to_uppercase();

    let read_only_prefix = upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("SHOW")
        || upper.starts_with("DESCRIBE")
        || upper.starts_with("PRAGMA");
    if !read_only_prefix {
        return false;
    }

    // Reject multi-statement strings: anything after a ';' could be DML
    if !is_single_statement(&upper) {
        return false;
    }

    // Reject if any write keyword appears as a word anywhere (data-modifying
    // CTEs, EXPLAIN ANALYZE on writes, …)
    const WRITE_KEYWORDS: [&str; 10] = [
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "TRUNCATE", "CREATE", "GRANT", "REVOKE",
        "MERGE",
    ];
    !upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|token| WRITE_KEYWORDS.contains(&token))
}

/// Best-effort secondary guard: flags read-only statements that can still mutate
/// or cause side effects. Returns a short reason when the statement should be
/// rejected.
///
/// [`is_read_only_sql`] already blocks DML/DDL and `SELECT … FOR UPDATE` (UPDATE
/// is a write keyword); this closes the remaining SELECT-side holes it misses:
/// `SELECT … INTO new_table` (PostgreSQL table creation), `INTO OUTFILE/DUMPFILE`
/// (MySQL file write), mutating `PRAGMA` (SQLite), sequence bumps, locks, and
/// known side-effecting functions. It is NOT a complete guarantee — a SELECT can
/// still call a user-defined side-effecting function — so for hard enforcement run
/// under a read-only database role or a replica.
pub fn side_effect_guard(sql: &str) -> Option<&'static str> {
    let upper = sql.to_uppercase();

    // SQLite PRAGMA can mutate (`PRAGMA user_version = 1`, `journal_mode = WAL`, …).
    // The AI agent rejects all PRAGMA; the CLI read-only path allows the read
    // forms via its own finer rule before calling this guard.
    if upper.trim_start().starts_with("PRAGMA") {
        return Some("uses PRAGMA (schema introspection is available via describe/\\d instead)");
    }

    // Whole-word keyword tokens (so identifiers like `into_count` don't match):
    // `SELECT … INTO <table>` creates a table on PostgreSQL; `INTO OUTFILE` /
    // `INTO DUMPFILE` writes a server-side file on MySQL.
    let has_token = |kw: &str| {
        upper
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|t| t == kw)
    };
    if has_token("INTO") {
        return Some("uses INTO (can create a table or write a file)");
    }

    // Side-effecting function / locking patterns (distinctive substrings).
    const FLAGGED: &[(&str, &str)] = &[
        ("NEXTVAL", "advances a sequence"),
        ("SETVAL", "resets a sequence"),
        ("PG_ADVISORY", "acquires an advisory lock"),
        ("PG_NOTIFY", "sends a notification"),
        ("GET_LOCK", "acquires a named lock"),
        ("FOR SHARE", "acquires row-share locks"),
        ("FOR KEY SHARE", "acquires row-share locks"),
        ("DBLINK", "can issue writes via dblink"),
        ("PG_TERMINATE_BACKEND", "terminates a backend"),
        ("PG_CANCEL_BACKEND", "cancels a backend"),
        ("LO_IMPORT", "writes a large object"),
        ("LO_EXPORT", "writes a server-side file"),
    ];
    FLAGGED
        .iter()
        .find(|(needle, _)| upper.contains(needle))
        .map(|(_, why)| *why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn test_is_read_only_sql() {
        assert!(is_read_only_sql("SELECT * FROM users"));
        assert!(is_read_only_sql("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(is_read_only_sql("EXPLAIN SELECT * FROM users"));
        assert!(is_read_only_sql("SELECT * FROM users;"));
        assert!(!is_read_only_sql("INSERT INTO users VALUES (1)"));
        assert!(!is_read_only_sql("DELETE FROM users WHERE id = 1"));
        assert!(!is_read_only_sql("UPDATE users SET name = 'test'"));
        assert!(!is_read_only_sql("DROP TABLE users"));
    }

    #[test]
    fn test_is_read_only_sql_rejects_disguised_writes() {
        // Data-modifying CTE
        assert!(!is_read_only_sql(
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"
        ));
        // Multi-statement
        assert!(!is_read_only_sql("SELECT 1; DROP TABLE users"));
        // EXPLAIN ANALYZE executes the statement
        assert!(!is_read_only_sql("EXPLAIN ANALYZE DELETE FROM users"));
        // Write keywords inside identifiers must NOT trigger
        assert!(is_read_only_sql("SELECT updated_at FROM user_inserts"));
    }

    #[test]
    fn test_side_effect_guard_flags_known_cases() {
        assert!(side_effect_guard("SELECT nextval('s')").is_some());
        assert!(side_effect_guard("select SETVAL('s', 1)").is_some());
        assert!(side_effect_guard("SELECT pg_advisory_lock(1)").is_some());
        assert!(side_effect_guard("SELECT pg_notify('ch', 'm')").is_some());
        assert!(side_effect_guard("SELECT GET_LOCK('x', 10)").is_some());
        assert!(side_effect_guard("SELECT * FROM t FOR SHARE").is_some());
        assert!(side_effect_guard("SELECT * FROM t FOR KEY SHARE").is_some());
        assert!(side_effect_guard("SELECT * INTO new_t FROM old_t").is_some());
        assert!(side_effect_guard("SELECT a INTO OUTFILE '/tmp/x' FROM t").is_some());
        assert!(side_effect_guard("PRAGMA user_version = 1").is_some());
        assert!(side_effect_guard("  pragma journal_mode = WAL").is_some());

        assert!(side_effect_guard("SELECT count(*) FROM orders").is_none());
        assert!(side_effect_guard("SELECT id, created_at FROM users LIMIT 10").is_none());
        assert!(side_effect_guard("SELECT into_count FROM stats").is_none());
    }

    #[rstest]
    #[case::select("SELECT * FROM t", true)]
    #[case::cte("WITH x AS (SELECT 1) SELECT * FROM x", true)]
    #[case::explain("EXPLAIN SELECT 1", true)]
    #[case::show("SHOW search_path", true)]
    #[case::pragma_read("PRAGMA table_info(users)", true)]
    #[case::pragma_read_bare("PRAGMA journal_mode", true)]
    #[case::pragma_read_semicolon("PRAGMA table_info(users);", true)]
    #[case::pragma_write("PRAGMA journal_mode = WAL", false)]
    #[case::pragma_multi("PRAGMA foo; DROP TABLE t", false)]
    #[case::insert("INSERT INTO t VALUES (1)", false)]
    #[case::update("UPDATE t SET a = 1", false)]
    #[case::ddl("CREATE TABLE t (a int)", false)]
    #[case::write_cte("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", false)]
    #[case::multi_statement("SELECT 1; DELETE FROM t", false)]
    #[case::select_into("SELECT * INTO t2 FROM t", false)]
    #[case::sequence("SELECT nextval('seq')", false)]
    fn test_check_read_only_sql_backends(#[case] sql: &str, #[case] allowed: bool) {
        for db_type in [
            DatabaseType::PostgreSQL,
            DatabaseType::MySQL,
            DatabaseType::SQLite,
            DatabaseType::ClickHouse,
        ] {
            assert_eq!(
                check_read_only(sql, &db_type).is_ok(),
                allowed,
                "{sql} on {db_type:?}"
            );
        }
    }

    #[rstest]
    #[case::find("db.users.find({})", true)]
    #[case::find_one("db.users.findOne({})", true)]
    #[case::dotted_collection("db.my.coll.find({})", true)]
    #[case::aggregate("db.orders.aggregate([{ $match: {} }])", true)]
    #[case::aggregate_out("db.orders.aggregate([{ $out: 'dest' }])", false)]
    #[case::aggregate_merge("db.orders.aggregate([{ $merge: { into: 'x' } }])", false)]
    #[case::count("db.users.countDocuments({})", true)]
    #[case::indexes("db.users.getIndexes()", true)]
    #[case::bare_collection("db.users", true)]
    #[case::db_stats("dbStats", true)]
    #[case::insert_one("db.users.insertOne({a: 1})", false)]
    #[case::update_many("db.users.updateMany({}, {$set: {a: 1}})", false)]
    #[case::delete_many("db.users.deleteMany({})", false)]
    #[case::drop("db.users.drop()", false)]
    #[case::create_index("db.users.createIndex({a: 1})", false)]
    #[case::sql_select("SELECT * FROM users", true)]
    #[case::unknown("resetErrorLog()", false)]
    fn test_check_read_only_mongodb(#[case] cmd: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(cmd, &DatabaseType::MongoDB).is_ok(),
            allowed,
            "{cmd}"
        );
    }

    #[rstest]
    #[case::select("SELECT * FROM logs", true)]
    #[case::show("SHOW TABLES", true)]
    #[case::describe("DESCRIBE logs", true)]
    #[case::explain("EXPLAIN SELECT * FROM logs", true)]
    #[case::delete("DELETE FROM logs", false)]
    #[case::raw_json("{\"query\": {\"match_all\": {}}}", false)]
    fn test_check_read_only_elasticsearch(#[case] q: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(q, &DatabaseType::Elasticsearch).is_ok(),
            allowed,
            "{q}"
        );
    }

    #[test]
    fn test_violation_marker() {
        let err = check_read_only("DROP TABLE t", &DatabaseType::PostgreSQL).unwrap_err();
        assert!(is_read_only_violation_message(&err.to_string()));
        assert!(!is_read_only_violation_message("some other error"));
    }
}
