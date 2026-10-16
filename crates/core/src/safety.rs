//! Read-only statement classification, shared by the CLI `--read-only` guard
//! and the AI agent's tool loop.
//!
//! Best-effort by design: classification is textual, so a SELECT calling a
//! user-defined side-effecting function still passes. For hard guarantees use
//! a read-only database role or a replica. Connect-time hardening
//! (`PRAGMA query_only` on SQLite, `default_transaction_read_only` on
//! PostgreSQL) backs this guard where the driver supports it.

use crate::database::{DatabaseType, InfluxLanguage};

mod influxdb;

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
        DatabaseType::WhiteDragon => check_white_dragon(query),
        DatabaseType::Redis => check_redis(query),
        DatabaseType::SqlServer => check_sql_server(query),
        DatabaseType::Redshift => check_redshift(query),
        DatabaseType::LibSql => check_libsql(query),
        DatabaseType::DuckDB => check_duckdb(query),
        DatabaseType::InfluxDB => influxdb::check_influxdb(query),
        DatabaseType::S3 => check_s3(query),
        DatabaseType::DynamoDB => check_dynamodb(query),
        DatabaseType::CloudWatchLogs => check_cloudwatch(query),
        _ => check_sql(query),
    };
    verdict.map_err(|reason| ReadOnlyViolation { reason })
}

/// Reject `query` unless it is a read-only InfluxDB query in `language`
/// (InfluxQL, Flux or v3 SQL). [`check_read_only`] guesses the language;
/// drivers and the CLI that know the connection's language use this one.
pub fn check_influxdb_read_only(
    query: &str,
    language: InfluxLanguage,
) -> Result<(), ReadOnlyViolation> {
    influxdb::check_influxdb_as(query, language).map_err(|reason| ReadOnlyViolation { reason })
}

/// The S3 command language is read-only by construction: `S3Command` has no
/// variant that writes (no put/delete/copy/multipart/ACL/presign), so a line is
/// accepted exactly when it parses and anything else is refused. The IAM policy
/// of the credentials stays the server-side limit; this guard only keeps the
/// client from offering a write.
fn check_s3(query: &str) -> Result<(), String> {
    crate::s3_command::parse(query)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// DynamoDB input is read-only by construction: a typed command (none writes)
/// or one PartiQL `SELECT`. `crate::dynamodb_command::parse` is the single
/// judge: it lexes through comments, quoted identifiers and string literals, so
/// `INSERT`/`UPDATE`/`DELETE`/`EXISTS`, a second statement after a `;` and a
/// write hidden behind a comment are all refused, and the driver executes
/// exactly what this accepts. The IAM policy of the credentials stays the
/// server-side limit.
fn check_dynamodb(query: &str) -> Result<(), String> {
    crate::dynamodb_command::parse(query)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// CloudWatch Logs input is read-only by construction: a typed command (none
/// writes) or a Logs Insights query, whose language has no write verb; the one
/// side effect of a query is the billable work of `StartQuery`, which the
/// driver bounds (a mandatory time window, a result limit) and stops on Ctrl-C.
/// `crate::cloudwatch_command::parse` is the single judge and the driver runs
/// exactly what it accepts, so `PutLogEvents`, `DeleteLogGroup`, retention and
/// export calls are unreachable.
fn check_cloudwatch(query: &str) -> Result<(), String> {
    crate::cloudwatch_command::parse(query)
        .map(|_| ())
        .map_err(|error| error.to_string())
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

/// Redshift: the PostgreSQL-style textual guard plus the Redshift statements
/// that write outside the cluster (`UNLOAD`/`COPY` to and from S3, stored
/// procedures, datashares). Those are refused by the "starts with SELECT/WITH/
/// EXPLAIN/SHOW" rule already, so the extra check only has to look inside
/// `EXPLAIN`, the one prefix that wraps another statement. A SELECT can still
/// call a Lambda UDF or read whatever the user can; use a read-only user
/// (`GRANT SELECT` only) for a hard limit.
fn check_redshift(query: &str) -> Result<(), String> {
    let upper = query.trim().to_uppercase();
    if upper.starts_with("PRAGMA") {
        return Err("PRAGMA is not a Redshift statement".to_string());
    }
    check_sql(query)?;
    if upper.starts_with("EXPLAIN") {
        const WRAPPED: [&str; 7] = [
            "UNLOAD", "COPY", "CALL", "VACUUM", "ANALYZE", "CANCEL", "ABORT",
        ];
        if let Some(word) = upper
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .find(|token| WRAPPED.contains(token))
        {
            return Err(format!("EXPLAIN of {word} is not allowed"));
        }
    }
    Ok(())
}

/// `PRAGMA`s a remote libSQL connection may run: introspection only. The
/// local SQLite rule (any `PRAGMA name` without `=`) lets calls such as
/// `PRAGMA wal_checkpoint` or `PRAGMA incremental_vacuum(10)` through, and on a
/// shared server they are not ours to run.
const LIBSQL_READ_PRAGMAS: &[&str] = &[
    "TABLE_INFO",
    "TABLE_XINFO",
    "TABLE_LIST",
    "INDEX_LIST",
    "INDEX_INFO",
    "INDEX_XINFO",
    "FOREIGN_KEY_LIST",
    "FOREIGN_KEY_CHECK",
    "DATABASE_LIST",
    "COLLATION_LIST",
    "FUNCTION_LIST",
    "MODULE_LIST",
    "PRAGMA_LIST",
    "COMPILE_OPTIONS",
    "ENCODING",
    "PAGE_COUNT",
    "PAGE_SIZE",
    "USER_VERSION",
    "SCHEMA_VERSION",
    "FOREIGN_KEYS",
    "JOURNAL_MODE",
    "QUERY_ONLY",
];

/// libSQL / Turso (Hrana over HTTP, SQLite dialect): the SQLite textual guard
/// with a stricter `PRAGMA` rule, and the SQLite statements that reach outside
/// the database file refused wherever they appear (`SELECT load_extension()`,
/// `EXPLAIN VACUUM INTO`, `ATTACH`/`DETACH`). The driver additionally asks the
/// server whether the statement is read-only (`describe.is_readonly`, SQLite's
/// own `sqlite3_stmt_readonly`) before running it. A read-only *token* is the
/// only hard limit.
fn check_libsql(query: &str) -> Result<(), String> {
    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();
    if let Some(pragma) = upper.strip_prefix("PRAGMA") {
        if !is_single_statement(&upper) {
            return Err("multi-statement PRAGMA batches are not allowed".to_string());
        }
        if trimmed.contains('=') {
            return Err("PRAGMA assignments modify database state".to_string());
        }
        let name = pragma
            .trim_start()
            .split(|c: char| c == '(' || c == ';' || c.is_whitespace())
            .next()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_string();
        return if LIBSQL_READ_PRAGMAS.contains(&name.as_str()) {
            Ok(())
        } else {
            Err(format!(
                "PRAGMA {} is not on the read-only allowlist",
                name.to_lowercase()
            ))
        };
    }
    check_sql(query)?;
    if let Some(word) = upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .find(|token| matches!(*token, "LOAD_EXTENSION" | "ATTACH" | "DETACH" | "VACUUM"))
    {
        return Err(format!("statement uses {word}"));
    }
    Ok(())
}

/// T-SQL statement keywords that start something other than a query. They
/// are refused wherever they appear (reserved words cannot be unquoted
/// identifiers, so a legitimate query never contains them): this is what
/// keeps `SELECT 1 DROP TABLE t` (no `;` needed in T-SQL) and the bodies of
/// subqueries honest, on top of the "starts with SELECT/WITH" rule.
const TSQL_DENIED_WORDS: &[&str] = &[
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "TRUNCATE",
    "CREATE",
    "ALTER",
    "DROP",
    "GRANT",
    "DENY",
    "REVOKE",
    "EXEC",
    "EXECUTE",
    "DBCC",
    "BACKUP",
    "RESTORE",
    "DUMP",
    "LOAD",
    "BULK",
    "WAITFOR",
    "SHUTDOWN",
    "KILL",
    "RECONFIGURE",
    "CHECKPOINT",
    "USE",
    "SET",
    "SETUSER",
    "REVERT",
    "DECLARE",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVE",
    "IF",
    "WHILE",
    "BREAK",
    "CONTINUE",
    "GOTO",
    "RETURN",
    "PRINT",
    "RAISERROR",
    "THROW",
    "OPEN",
    "CLOSE",
    "DEALLOCATE",
    "READTEXT",
    "WRITETEXT",
    "UPDATETEXT",
    "RECEIVE",
    // `SELECT … INTO` creates a table; the other INTOs belong to INSERT
    "INTO",
    // Locking hints that take update/exclusive locks
    "UPDLOCK",
    "XLOCK",
    "TABLOCKX",
];

/// Names refused as exact matches (functions and rowsets that reach outside
/// the database: linked servers, files, other servers' data).
const TSQL_DENIED_NAMES: &[&str] = &[
    "OPENROWSET",
    "OPENQUERY",
    "OPENDATASOURCE",
    "OPENXML",
    "FN_GET_AUDIT_FILE",
    "FN_DUMP_DBLOG",
];

/// Name prefixes refused: system and extended procedures (`xp_cmdshell`,
/// `sp_configure`, `sp_executesql`…), trace and extended-event file readers.
const TSQL_DENIED_NAME_PREFIXES: &[&str] = &["SP_", "XP_", "FN_TRACE_", "FN_XE_"];

/// Word pairs that are only a problem together (the first word alone is an
/// ordinary identifier or part of an ordinary query).
const TSQL_DENIED_PAIRS: &[(&str, &str)] = &[
    ("NEXT", "VALUE"), // NEXT VALUE FOR seq advances a sequence
    ("END", "CONVERSATION"),
    ("GET", "CONVERSATION"),
    ("MOVE", "CONVERSATION"),
    ("SEND", "ON"),
    ("ADD", "SIGNATURE"),
    ("ENABLE", "TRIGGER"),
    ("DISABLE", "TRIGGER"),
];

/// The text of a `[bracketed]` or `"quoted"` identifier token, unescaped and
/// uppercased; `None` for string literals and other tokens.
fn tsql_quoted_identifier(sql: &str, token: &Token) -> Option<String> {
    if token.kind != Tok::Quoted {
        return None;
    }
    let text = &sql[token.start..token.end];
    let (open, close) = match text.chars().next()? {
        '[' => ('[', ']'),
        '"' => ('"', '"'),
        _ => return None,
    };
    let inner = text.strip_prefix(open)?.strip_suffix(close)?;
    let doubled = format!("{close}{close}");
    Some(inner.replace(&doubled, &close.to_string()).to_uppercase())
}

fn tsql_name_denied(name: &str) -> bool {
    TSQL_DENIED_NAMES.contains(&name)
        || TSQL_DENIED_NAME_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Whether `token` is the first thing on its line and the rest of the line is
/// at most a repeat count — the `GO` batch separator that sqlcmd/SSMS split on.
fn is_go_batch_separator(sql: &str, token: &Token) -> bool {
    if !token.is_word("GO") {
        return false;
    }
    let line_start = sql[..token.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = sql[token.end..]
        .find('\n')
        .map_or(sql.len(), |i| token.end + i);
    sql[line_start..token.start].trim().is_empty()
        && sql[token.end..line_end]
            .trim()
            .chars()
            .all(|c| c.is_ascii_digit())
}

/// T-SQL (SQL Server / Azure SQL): default-deny. A statement is accepted only
/// when it starts with `SELECT`/`WITH … SELECT` (an optional leading `EXPLAIN`
/// is the driver's showplan shorthand) and contains none of the statement
/// keywords, extended procedures, linked-server and file rowsets listed above.
/// Best effort like the other textual guards: a SELECT can still call a CLR or
/// scalar function with side effects, and can read whatever the login can —
/// use a read-only login (`db_datareader` + `DENY` writes) for a hard limit.
fn check_sql_server(query: &str) -> Result<(), String> {
    let tokens = lex(query, Dialect::SqlServer).map_err(|refusal| refusal.reason)?;

    // Statements: split on top-level `;`, empty ones (`;WITH …`, `…;;`) ignored
    let mut statements = tokens
        .split(|t| t.is_punct(';'))
        .filter(|statement| !statement.is_empty());
    let Some(statement) = statements.next() else {
        return Err("the statement is empty".to_string());
    };
    if statements.next().is_some() {
        return Err("multi-statement batches are not allowed".to_string());
    }

    if tokens.iter().any(|t| is_go_batch_separator(query, t)) {
        return Err("the GO batch separator is not part of T-SQL and is not allowed".to_string());
    }

    // The statement verb: leading parentheses (`(SELECT 1) UNION …`), then
    // an optional EXPLAIN, then SELECT or WITH
    let mut body = statement;
    while body.first().is_some_and(|t| t.is_punct('(')) {
        body = &body[1..];
    }
    if body.first().is_some_and(|t| t.is_word("EXPLAIN")) {
        body = &body[1..];
        while body.first().is_some_and(|t| t.is_punct('(')) {
            body = &body[1..];
        }
    }
    match body.first().and_then(Token::word) {
        Some("SELECT") => {}
        Some("WITH") => {
            // The main statement follows the CTE list at the same depth
            let depth = body[0].depth;
            let main = body[1..]
                .iter()
                .find(|t| t.depth == depth && t.is_word("SELECT"));
            if main.is_none() {
                return Err("WITH must be followed by a SELECT".to_string());
            }
        }
        _ => {
            return Err(
                "only SELECT (or WITH … SELECT) statements are allowed on SQL Server".to_string(),
            );
        }
    }

    // Four-part names (`linked.db.schema.table`) run on another server. A
    // name part directly followed by a dot continues the chain; anything else
    // starts a new one (so `a.b JOIN c.d JOIN e.f` is three short chains)
    #[derive(PartialEq)]
    enum Last {
        Nothing,
        Name,
        Dot,
    }
    let (mut last, mut dots) = (Last::Nothing, 0usize);
    for token in statement {
        match &token.kind {
            Tok::Punct('.') => {
                dots = if last == Last::Nothing { 1 } else { dots + 1 };
                last = Last::Dot;
                if dots >= 3 {
                    return Err(
                        "four-part names (linked servers) are not allowed on SQL Server"
                            .to_string(),
                    );
                }
            }
            Tok::Word(_) | Tok::Quoted => {
                if last != Last::Dot {
                    dots = 0;
                }
                last = Last::Name;
            }
            _ => (last, dots) = (Last::Nothing, 0),
        }
    }

    for (i, token) in statement.iter().enumerate() {
        let next = statement.get(i + 1).and_then(Token::word);
        if let Some(word) = token.word() {
            // `MERGE JOIN` / `MERGE UNION` are join hints, not the statement
            let merge_hint = word == "MERGE" && matches!(next, Some("JOIN" | "UNION"));
            if TSQL_DENIED_WORDS.contains(&word) && !merge_hint {
                return Err(format!(
                    "{word} is not allowed on SQL Server in read-only mode"
                ));
            }
            if tsql_name_denied(word) {
                return Err(format!(
                    "{word} reaches outside the database and is not allowed"
                ));
            }
            if TSQL_DENIED_PAIRS
                .iter()
                .any(|(first, second)| word == *first && next == Some(*second))
            {
                return Err(format!(
                    "{word} {} is not allowed on SQL Server in read-only mode",
                    next.unwrap_or_default()
                ));
            }
        } else if let Some(name) = tsql_quoted_identifier(query, token)
            && tsql_name_denied(&name)
        {
            return Err(format!(
                "{name} reaches outside the database and is not allowed"
            ));
        }
    }
    Ok(())
}

// ----- DuckDB ---------------------------------------------------------------
//
// A DuckDB connection opened for `--read-only` or the broker is read-only at
// the engine (`access_mode=READ_ONLY`, external access off, configuration
// locked), so nothing below is what stops a write there. The textual check
// gives a clear message before the statement is sent, covers the in-memory
// database (which cannot be opened read-only) and is what an AI agent's tool
// loop calls. Default-deny: a statement passes only when it starts with a
// reading verb and none of the file, network and environment functions or
// replacement scans (`FROM 'x.csv'`) appear.

/// Statement verbs that only read.
const DUCKDB_READ_VERBS: &[&str] = &[
    "SELECT",
    "VALUES",
    "FROM",
    "SUMMARIZE",
    "DESCRIBE",
    "DESC",
    "SHOW",
    "PIVOT",
    "UNPIVOT",
];

/// `PRAGMA name` / `PRAGMA name(arg)` forms that only read.
const DUCKDB_READ_PRAGMAS: &[&str] = &[
    "database_size",
    "database_list",
    "table_info",
    "show_tables",
    "show_tables_expanded",
    "storage_info",
    "table_storage_info",
    "version",
    "platform",
    "functions",
    "collations",
];

/// Functions that read files, URLs, other databases or the environment, run
/// SQL from a string, or reveal secrets. Lowercase; a trailing `*` is a prefix.
const DUCKDB_DENIED_FUNCTIONS: &[&str] = &[
    "read_*",
    "glob",
    "parquet_*",
    "sniff_csv",
    "sqlite_*",
    "postgres_*",
    "mysql_*",
    "iceberg_*",
    "delta_*",
    "excel_*",
    "st_read*",
    "http_*",
    "getenv",
    "duckdb_secrets",
    "which_secret",
    "query",
    "query_table",
    "load_aws_credentials",
    "install_extension",
    "load_extension",
    "force_checkpoint",
    "checkpoint",
];

/// File extensions DuckDB opens by itself when they name a "table".
const DUCKDB_DATA_EXTENSIONS: &[&str] = &[
    ".csv", ".tsv", ".parquet", ".json", ".jsonl", ".ndjson", ".gz", ".zst", ".xlsx", ".db",
    ".duckdb", ".sqlite", ".arrow",
];

/// The text of a quoted token, quotes removed.
fn quoted_text<'a>(sql: &'a str, token: &Token) -> &'a str {
    let text = &sql[token.start..token.end];
    match text.chars().next() {
        Some('\'' | '"') if text.len() >= 2 => &text[1..text.len() - 1],
        _ => text,
    }
}

fn duckdb_path_like(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("://")
        || lower.starts_with('/')
        || lower.starts_with("~/")
        || lower.starts_with("./")
        || lower.starts_with("../")
        || lower.contains('\\')
        || DUCKDB_DATA_EXTENSIONS
            .iter()
            .any(|ext| lower.ends_with(ext))
}

/// What DuckDB reads when a string or a quoted name stands where a table is
/// expected (a "replacement scan"): `FROM 'x.csv'`, `JOIN "/etc/data.parquet"`,
/// `FROM a, 'b.csv'`.
fn duckdb_replacement_scan(sql: &str, tokens: &[Token]) -> Option<String> {
    for (i, token) in tokens.iter().enumerate() {
        if token.kind != Tok::Quoted {
            continue;
        }
        let text = &sql[token.start..token.end];
        let single = text.starts_with('\'');
        let after_from = i
            .checked_sub(1)
            .and_then(|p| tokens.get(p))
            .is_some_and(|prev| prev.is_word("FROM") || prev.is_word("JOIN"));
        let after_comma = i
            .checked_sub(1)
            .and_then(|p| tokens.get(p))
            .is_some_and(|prev| prev.is_punct(','));
        let inner = quoted_text(sql, token);
        // A string in table position is always a file; a quoted name only when
        // it looks like a path (a table may be called "my table")
        if (after_from && (single || duckdb_path_like(inner)))
            || (after_comma && duckdb_path_like(inner))
        {
            return Some(inner.to_string());
        }
    }
    None
}

/// A denied DuckDB function called anywhere in `tokens`.
fn duckdb_denied_call(sql: &str, tokens: &[Token]) -> Option<String> {
    for (i, token) in tokens.iter().enumerate() {
        if !tokens.get(i + 1).is_some_and(|next| next.is_punct('(')) {
            continue;
        }
        let name = match &token.kind {
            Tok::Word(word) => word.to_ascii_lowercase(),
            Tok::Quoted if sql[token.start..token.end].starts_with('"') => {
                quoted_text(sql, token).to_ascii_lowercase()
            }
            _ => continue,
        };
        if matches_denylist(&name, DUCKDB_DENIED_FUNCTIONS) {
            return Some(name);
        }
    }
    None
}

/// `PRAGMA [schema.]name[(args)]` with a read-only name and no assignment.
fn duckdb_pragma_allowed(tokens: &[Token]) -> Result<(), String> {
    let mut i = 0usize;
    let mut name = tokens
        .get(i)
        .and_then(Token::word)
        .map(str::to_ascii_lowercase);
    if tokens.get(i + 1).is_some_and(|t| t.is_punct('.')) {
        i += 2;
        name = tokens
            .get(i)
            .and_then(Token::word)
            .map(str::to_ascii_lowercase);
    }
    let Some(name) = name else {
        return Err("PRAGMA statement not recognised".to_string());
    };
    if tokens.get(i + 1).is_some_and(|t| t.is_punct('=')) {
        return Err("PRAGMA assignments modify configuration".to_string());
    }
    if DUCKDB_READ_PRAGMAS.contains(&name.as_str()) {
        Ok(())
    } else {
        Err(format!("PRAGMA {name} is not on the read-only allowlist"))
    }
}

/// One DuckDB statement (no `;`), default-deny.
fn check_duckdb_statement(sql: &str, statement: &[Token]) -> Result<(), String> {
    let mut body = statement;
    while body.first().is_some_and(|t| t.is_punct('(')) {
        body = &body[1..];
    }
    let Some(verb) = body.first().and_then(Token::word) else {
        return Err("only read-only statements are allowed on DuckDB".to_string());
    };
    match verb {
        v if DUCKDB_READ_VERBS.contains(&v) => {}
        "WITH" => {
            // The main statement follows the CTE list at the same depth
            let depth = body[0].depth;
            let main = body[1..]
                .iter()
                .filter(|t| t.depth == depth)
                .filter_map(Token::word)
                .find(|w| {
                    DUCKDB_READ_VERBS.contains(w)
                        || DML_VERBS.contains(w)
                        || matches!(*w, "CREATE" | "COPY" | "EXPORT" | "DROP")
                });
            match main {
                Some(w) if DUCKDB_READ_VERBS.contains(&w) => {}
                Some(w) => return Err(format!("WITH … {w} is not a read-only DuckDB statement")),
                None => return Err("WITH must be followed by a SELECT".to_string()),
            }
        }
        "EXPLAIN" => {
            let mut i = 1usize;
            loop {
                match body.get(i) {
                    Some(t) if matches!(t.word(), Some("ANALYZE" | "ANALYSE" | "VERBOSE")) => {
                        i += 1;
                    }
                    // `EXPLAIN (FORMAT JSON) SELECT …`: skip the option list
                    Some(t)
                        if t.is_punct('(')
                            && !body
                                .get(i + 1)
                                .and_then(Token::word)
                                .is_some_and(|w| DUCKDB_READ_VERBS.contains(&w) || w == "WITH") =>
                    {
                        let mut j = i + 1;
                        while j < body.len() && !(body[j].is_punct(')') && body[j].depth == t.depth)
                        {
                            j += 1;
                        }
                        i = j + 1;
                    }
                    _ => break,
                }
            }
            let inner = &body[i.min(body.len())..];
            if inner.is_empty() {
                return Err("EXPLAIN without a statement".to_string());
            }
            check_duckdb_statement(sql, inner)?;
        }
        "PRAGMA" => duckdb_pragma_allowed(&body[1..])?,
        // Switching the current database changes nothing stored
        "USE" => {}
        other => return Err(format!("{other} is not a read-only DuckDB statement")),
    }

    if let Some(name) = duckdb_denied_call(sql, statement) {
        return Err(format!(
            "function {name}() reads files, other databases or the environment and is not allowed"
        ));
    }
    if let Some(path) = duckdb_replacement_scan(sql, statement) {
        return Err(format!(
            "reading the file '{path}' by name is not allowed in read-only mode"
        ));
    }
    for (i, token) in statement.iter().enumerate() {
        let Some(word) = token.word() else { continue };
        if matches!(word, "ATTACH" | "DETACH" | "INSTALL") {
            return Err(format!("{word} is not allowed in read-only mode"));
        }
        // Writable subquery / CTE: a DML verb opening a parenthesised group
        let prev = i.checked_sub(1).and_then(|p| statement.get(p));
        let next = statement.get(i + 1);
        if DML_VERBS.contains(&word)
            && prev.is_some_and(|p| p.is_punct('('))
            && !next.is_some_and(|n| n.is_punct('('))
        {
            return Err(format!("{word} inside a query is not allowed"));
        }
    }
    Ok(())
}

/// DuckDB: every statement of the string must be a read.
fn check_duckdb(query: &str) -> Result<(), String> {
    let tokens = lex(query, Dialect::DuckDb).map_err(|refusal| refusal.reason)?;
    let mut statements = tokens
        .split(|t| t.is_punct(';'))
        .filter(|statement| !statement.is_empty())
        .peekable();
    if statements.peek().is_none() {
        return Err("the statement is empty".to_string());
    }
    for statement in statements {
        check_duckdb_statement(query, statement)?;
    }
    Ok(())
}

/// MongoDB commands are not SQL — allowlist the read verbs that
/// `execute_query` dispatches (see `crates/driver-mongodb`). Unknown commands are
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

/// White Dragon exposes only search operations. Both its SQL and qualifier
/// front-ends use the same read-only `/v1/search` endpoint, so qualifier input
/// must not be rejected merely because it does not start with `SELECT`.
fn check_white_dragon(query: &str) -> Result<(), String> {
    if query.trim().is_empty() {
        Err("an empty White Dragon search is not valid".to_string())
    } else {
        Ok(())
    }
}

/// Redis commands that read and have no side effect on the dataset, matched on
/// the upper-cased command name. Everything else — every write, every admin
/// command, anything new or module-provided — is refused (default-deny).
const REDIS_READ_COMMANDS: &[&str] = &[
    // strings / generic
    "GET",
    "MGET",
    "STRLEN",
    "GETRANGE",
    "SUBSTR",
    "LCS",
    "EXISTS",
    "TYPE",
    "TTL",
    "PTTL",
    "EXPIRETIME",
    "PEXPIRETIME",
    "DUMP",
    "RANDOMKEY",
    "DBSIZE",
    "SCAN",
    "SORT_RO",
    // bitmaps / hyperloglog / geo (the read forms; GEORADIUS, GEOSEARCHSTORE and
    // PFADD/PFMERGE write)
    "BITCOUNT",
    "BITPOS",
    "GETBIT",
    "BITFIELD_RO",
    "PFCOUNT",
    "GEOPOS",
    "GEODIST",
    "GEOHASH",
    "GEOSEARCH",
    "GEORADIUS_RO",
    "GEORADIUSBYMEMBER_RO",
    // hashes
    "HGET",
    "HMGET",
    "HGETALL",
    "HKEYS",
    "HVALS",
    "HLEN",
    "HEXISTS",
    "HSTRLEN",
    "HRANDFIELD",
    "HSCAN",
    // lists
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LPOS",
    // sets
    "SMEMBERS",
    "SCARD",
    "SISMEMBER",
    "SMISMEMBER",
    "SRANDMEMBER",
    "SINTER",
    "SUNION",
    "SDIFF",
    "SINTERCARD",
    "SSCAN",
    // sorted sets (the STORE variants are separate, refused commands)
    "ZRANGE",
    "ZRANGEBYSCORE",
    "ZRANGEBYLEX",
    "ZREVRANGE",
    "ZREVRANGEBYSCORE",
    "ZREVRANGEBYLEX",
    "ZCARD",
    "ZCOUNT",
    "ZLEXCOUNT",
    "ZSCORE",
    "ZMSCORE",
    "ZRANK",
    "ZREVRANK",
    "ZRANDMEMBER",
    "ZINTER",
    "ZUNION",
    "ZDIFF",
    "ZSCAN",
    // streams (consumer-group commands mutate state and are refused)
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XPENDING",
    // server
    "PING",
    "ECHO",
    "TIME",
    "INFO",
    "LASTSAVE",
    "ROLE",
];

/// Commands refused with a specific reason (they would be refused by default
/// anyway; the reason tells the user what to use instead).
const REDIS_DENIED_COMMANDS: &[(&str, &str)] = &[
    ("FLUSHALL", "wipes every database"),
    ("FLUSHDB", "wipes the whole database"),
    ("CONFIG", "reads or changes server configuration"),
    ("DEBUG", "is a server debugging command"),
    ("SHUTDOWN", "stops the server"),
    ("MODULE", "loads or lists server modules"),
    ("SCRIPT", "manages server-side scripts"),
    ("EVAL", "runs arbitrary server-side Lua"),
    ("EVALSHA", "runs arbitrary server-side Lua"),
    ("EVAL_RO", "runs arbitrary server-side Lua"),
    ("EVALSHA_RO", "runs arbitrary server-side Lua"),
    ("FCALL", "runs arbitrary server-side functions"),
    ("FCALL_RO", "runs arbitrary server-side functions"),
    ("FUNCTION", "manages server-side functions"),
    ("KEYS", "blocks the server on a large keyspace; use SCAN"),
    ("MONITOR", "streams every command of every client"),
    ("SUBSCRIBE", "blocks the connection waiting for messages"),
    ("PSUBSCRIBE", "blocks the connection waiting for messages"),
    ("SSUBSCRIBE", "blocks the connection waiting for messages"),
    ("PUBLISH", "sends a message to subscribers"),
    ("SYNC", "starts a replication stream"),
    ("PSYNC", "starts a replication stream"),
    ("BLPOP", "blocks and removes an element"),
    ("BRPOP", "blocks and removes an element"),
    ("BLMOVE", "blocks and moves an element"),
    ("BRPOPLPUSH", "blocks and moves an element"),
    ("BLMPOP", "blocks and removes elements"),
    ("BZPOPMIN", "blocks and removes a member"),
    ("BZPOPMAX", "blocks and removes a member"),
    ("BZMPOP", "blocks and removes members"),
    ("XREADGROUP", "changes consumer-group state"),
    ("WAIT", "blocks until replicas acknowledge"),
    ("WAITAOF", "blocks until the AOF is synced"),
    ("SORT", "can STORE its result; use SORT_RO"),
    ("ACL", "reads or changes users and permissions"),
    ("AUTH", "changes the connection's identity"),
    ("HELLO", "changes the connection's protocol or identity"),
    (
        "SELECT",
        "changes the connection's database; connect with the /N URL path or \\c N",
    ),
    ("MULTI", "opens a transaction"),
    ("EXEC", "runs a transaction"),
    ("DISCARD", "ends a transaction"),
    ("WATCH", "watches keys for a transaction"),
    ("SLOWLOG", "exposes other clients' commands"),
];

/// Redis is a command language, not SQL: allowlist the commands that cannot
/// change the dataset or the server, and refuse everything else — including
/// commands this list has never heard of (new write verbs and module commands
/// must never pass by default). `KEYS` is refused for load, not safety: it
/// blocks the whole server on a large keyspace.
///
/// Like the other textual guards this is best-effort. For hard enforcement
/// connect as an ACL user that has only read commands (see the Redis
/// reference).
fn check_redis(query: &str) -> Result<(), String> {
    use crate::redis_command::{command_name, split_command_line, subcommand_name};

    let args = split_command_line(query).map_err(|error| error.to_string())?;
    let command = command_name(&args);
    let sub = subcommand_name(&args);
    let has_arg = |wanted: &str| {
        args.iter()
            .skip(1)
            .any(|arg| String::from_utf8_lossy(arg).eq_ignore_ascii_case(wanted))
    };

    if let Some((_, reason)) = REDIS_DENIED_COMMANDS
        .iter()
        .find(|(denied, _)| *denied == command)
    {
        return Err(format!("Redis command {command} {reason}"));
    }
    if REDIS_READ_COMMANDS.contains(&command.as_str()) {
        return Ok(());
    }

    // Commands whose safety is decided by the subcommand or a flag
    let sub = sub.as_deref();
    let allowed = match command.as_str() {
        "XREAD" => {
            if has_arg("BLOCK") {
                return Err("Redis command XREAD BLOCK blocks the connection".to_string());
            }
            true
        }
        "OBJECT" => matches!(
            sub,
            Some("ENCODING" | "IDLETIME" | "REFCOUNT" | "FREQ" | "HELP")
        ),
        "MEMORY" => matches!(sub, Some("USAGE" | "STATS" | "DOCTOR" | "HELP")),
        "XINFO" => matches!(sub, Some("STREAM" | "GROUPS" | "CONSUMERS" | "HELP")),
        "CLIENT" => matches!(sub, Some("INFO" | "GETNAME" | "ID")),
        "COMMAND" => matches!(
            sub,
            None | Some(
                "COUNT" | "DOCS" | "INFO" | "LIST" | "GETKEYS" | "GETKEYSANDFLAGS" | "HELP"
            )
        ),
        "PUBSUB" => matches!(
            sub,
            Some("CHANNELS" | "NUMSUB" | "NUMPAT" | "SHARDCHANNELS" | "SHARDNUMSUB")
        ),
        _ => false,
    };
    if allowed {
        return Ok(());
    }
    match sub {
        Some(sub)
            if matches!(
                command.as_str(),
                "OBJECT" | "MEMORY" | "XINFO" | "CLIENT" | "COMMAND" | "PUBSUB"
            ) =>
        {
            Err(format!(
                "Redis command {command} {sub} is not on the read-only allowlist"
            ))
        }
        _ => Err(format!(
            "Redis command {command} is not on the read-only allowlist"
        )),
    }
}

/// Elasticsearch speaks SQL here (see `crates/driver-elasticsearch`); its SQL
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

// ---------------------------------------------------------------------------
// Credential-broker classifier
// ---------------------------------------------------------------------------
//
// The textual guard above is a convenience for humans (`--read-only`) and the
// AI tool loop. The credential broker executes SQL on behalf of *untrusted*
// agents, so it gets a real lexer: comments, quoted identifiers, string
// literals (with per-engine escape rules) and PostgreSQL dollar-quoting are
// understood, statements are split on top-level `;`, and the statement kind is
// taken from the first keyword. Everything the classifier does not positively
// recognise is refused.
//
// Residual risk (documented, not solvable by parsing): a SELECT may call a
// user-defined function with side effects, and any permitted SELECT can read
// whatever the database role can read. The DB role's privileges (and the
// native read-only session settings the gateway applies) are the real defence.

/// What an accepted statement does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementClass {
    /// SELECT / SHOW / DESCRIBE / EXPLAIN / read-only PRAGMA: no data change.
    Read,
    /// INSERT / UPDATE / DELETE / MERGE / REPLACE, writable CTEs, locking
    /// SELECTs: allowed only for aliases enrolled with writes.
    Write,
}

/// Why the classifier refused a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// Nothing but whitespace/comments/semicolons.
    Empty,
    /// More than one statement.
    MultiStatement,
    /// DDL, DCL, admin, session control, file/network access, or a statement
    /// kind the broker does not know.
    Ddl,
    /// A write statement for a read-only alias.
    ReadOnlyViolation,
    /// Lexically invalid (unterminated string/comment, unbalanced parens).
    Malformed,
}

/// A refused statement. `reason` never contains the SQL text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct Refusal {
    pub kind: RefusalKind,
    pub reason: String,
}

impl Refusal {
    fn new(kind: RefusalKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Postgres,
    MySql,
    Sqlite,
    ClickHouse,
    /// T-SQL: `[bracketed]` identifiers, nested block comments, no
    /// backslash escapes. Only the textual `--read-only` guard lexes with it
    /// (the broker refuses SQL Server).
    SqlServer,
    /// DuckDB: PostgreSQL quoting (nested block comments, `E'…'`, dollar
    /// quotes) without backticks.
    DuckDb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Unquoted word, ASCII-uppercased.
    Word(String),
    /// Quoted identifier or string literal (contents irrelevant).
    Quoted,
    Number,
    Punct(char),
}

#[derive(Debug, Clone)]
struct Token {
    kind: Tok,
    start: usize,
    end: usize,
    /// Parenthesis depth *outside* the token (an opening paren has the depth
    /// of its surroundings, the matching closing paren too).
    depth: usize,
}

impl Token {
    fn word(&self) -> Option<&str> {
        match &self.kind {
            Tok::Word(w) => Some(w.as_str()),
            _ => None,
        }
    }

    fn is_word(&self, expected: &str) -> bool {
        self.word() == Some(expected)
    }

    fn is_punct(&self, expected: char) -> bool {
        self.kind == Tok::Punct(expected)
    }
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$' || !c.is_ascii()
}

fn lex(sql: &str, dialect: Dialect) -> Result<Vec<Token>, Refusal> {
    let malformed = |what: &str| Refusal::new(RefusalKind::Malformed, what.to_string());
    // Servers truncate at an embedded NUL: what they run could differ from
    // what was classified
    if sql.contains('\0') {
        return Err(malformed("NUL characters are not allowed"));
    }
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|(_, c)| *c);
    let pos = |i: usize| chars.get(i).map_or(sql.len(), |(p, _)| *p);
    let backslash_escapes = matches!(dialect, Dialect::MySql | Dialect::ClickHouse);

    let mut tokens: Vec<Token> = Vec::new();
    let mut depth = 0usize;
    let mut i = 0usize;

    // Scan a quoted run starting after the opening quote at `i`; `doubling`
    // lets the quote char be repeated, `escapes` honours backslashes.
    fn scan_quoted(
        chars: &[(usize, char)],
        mut i: usize,
        quote: char,
        doubling: bool,
        escapes: bool,
    ) -> Option<usize> {
        while i < chars.len() {
            let c = chars[i].1;
            if escapes && c == '\\' {
                i += 2;
                continue;
            }
            if c == quote {
                if doubling && chars.get(i + 1).map(|(_, n)| *n) == Some(quote) {
                    i += 2;
                    continue;
                }
                return Some(i + 1);
            }
            i += 1;
        }
        None
    }

    while i < chars.len() {
        let c = chars[i].1;
        let start = pos(i);

        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // `--` comments (MySQL needs whitespace after the dashes)
        if c == '-' && at(i + 1) == Some('-') {
            let is_comment = match dialect {
                Dialect::MySql => at(i + 2).is_none_or(|n| n.is_whitespace() || n.is_control()),
                _ => true,
            };
            if is_comment {
                while i < chars.len() && chars[i].1 != '\n' {
                    i += 1;
                }
                continue;
            }
        }
        // `#` comments (MySQL, ClickHouse); an operator character elsewhere
        if c == '#' && matches!(dialect, Dialect::MySql | Dialect::ClickHouse) {
            while i < chars.len() && chars[i].1 != '\n' {
                i += 1;
            }
            continue;
        }
        // Block comments
        if c == '/' && at(i + 1) == Some('*') {
            // MySQL/MariaDB execute `/*! ... */` and `/*M! ... */` bodies
            if matches!(dialect, Dialect::MySql | Dialect::ClickHouse)
                && (at(i + 2) == Some('!') || (at(i + 2) == Some('M') && at(i + 3) == Some('!')))
            {
                return Err(Refusal::new(
                    RefusalKind::Ddl,
                    "version-conditional comments (/*! ... */) are executed by MySQL and are not allowed",
                ));
            }
            let nests = matches!(
                dialect,
                Dialect::Postgres | Dialect::DuckDb | Dialect::SqlServer
            );
            let mut level = 1usize;
            i += 2;
            loop {
                match (at(i), at(i + 1)) {
                    (None, _) => return Err(malformed("unterminated block comment")),
                    (Some('*'), Some('/')) => {
                        level -= 1;
                        i += 2;
                        if level == 0 {
                            break;
                        }
                    }
                    (Some('/'), Some('*')) if nests => {
                        level += 1;
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            continue;
        }

        match c {
            '\'' => {
                // E'...' in PostgreSQL uses backslash escapes
                let e_string = matches!(dialect, Dialect::Postgres | Dialect::DuckDb)
                    && tokens
                        .last()
                        .is_some_and(|t| t.is_word("E") && t.end == start);
                let end = scan_quoted(&chars, i + 1, '\'', true, backslash_escapes || e_string)
                    .ok_or_else(|| malformed("unterminated string literal"))?;
                tokens.push(Token {
                    kind: Tok::Quoted,
                    start,
                    end: pos(end),
                    depth,
                });
                i = end;
            }
            '"' => {
                let end = scan_quoted(&chars, i + 1, '"', true, backslash_escapes)
                    .ok_or_else(|| malformed("unterminated quoted identifier"))?;
                tokens.push(Token {
                    kind: Tok::Quoted,
                    start,
                    end: pos(end),
                    depth,
                });
                i = end;
            }
            '`' if !matches!(
                dialect,
                Dialect::Postgres | Dialect::DuckDb | Dialect::SqlServer
            ) =>
            {
                let end = scan_quoted(&chars, i + 1, '`', true, false)
                    .ok_or_else(|| malformed("unterminated quoted identifier"))?;
                tokens.push(Token {
                    kind: Tok::Quoted,
                    start,
                    end: pos(end),
                    depth,
                });
                i = end;
            }
            '[' if dialect == Dialect::SqlServer => {
                // [identifier]: `]]` is an escaped closing bracket
                let mut j = i + 1;
                loop {
                    match (at(j), at(j + 1)) {
                        (None, _) => return Err(malformed("unterminated bracket identifier")),
                        (Some(']'), Some(']')) => j += 2,
                        (Some(']'), _) => break,
                        _ => j += 1,
                    }
                }
                tokens.push(Token {
                    kind: Tok::Quoted,
                    start,
                    end: pos(j + 1),
                    depth,
                });
                i = j + 1;
            }
            '[' if dialect == Dialect::Sqlite => {
                let mut j = i + 1;
                while j < chars.len() && chars[j].1 != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    return Err(malformed("unterminated bracket identifier"));
                }
                tokens.push(Token {
                    kind: Tok::Quoted,
                    start,
                    end: pos(j + 1),
                    depth,
                });
                i = j + 1;
            }
            '$' if matches!(dialect, Dialect::Postgres | Dialect::DuckDb)
                && !at(i + 1).is_some_and(|n| n.is_ascii_digit()) =>
            {
                // Dollar quoting: $tag$ ... $tag$ (tag may be empty)
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].1.is_alphanumeric() || chars[j].1 == '_' || !chars[j].1.is_ascii())
                {
                    j += 1;
                }
                if at(j) == Some('$') {
                    let tag: String = chars[i..=j].iter().map(|(_, ch)| *ch).collect();
                    let body_start = pos(j + 1);
                    match sql[body_start..].find(&tag) {
                        Some(offset) => {
                            let end_byte = body_start + offset + tag.len();
                            tokens.push(Token {
                                kind: Tok::Quoted,
                                start,
                                end: end_byte,
                                depth,
                            });
                            // Resume after the closing tag
                            i = chars
                                .iter()
                                .position(|(p, _)| *p >= end_byte)
                                .unwrap_or(chars.len());
                        }
                        None => return Err(malformed("unterminated dollar-quoted string")),
                    }
                } else {
                    tokens.push(Token {
                        kind: Tok::Punct('$'),
                        start,
                        end: pos(i + 1),
                        depth,
                    });
                    i += 1;
                }
            }
            c if c.is_ascii_digit() => {
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].1.is_alphanumeric() || matches!(chars[j].1, '_' | '.'))
                {
                    j += 1;
                }
                tokens.push(Token {
                    kind: Tok::Number,
                    start,
                    end: pos(j),
                    depth,
                });
                i = j;
            }
            c if is_word_start(c) => {
                let mut j = i + 1;
                while j < chars.len() && is_word_char(chars[j].1) {
                    j += 1;
                }
                let text = &sql[start..pos(j)];
                tokens.push(Token {
                    kind: Tok::Word(text.to_ascii_uppercase()),
                    start,
                    end: pos(j),
                    depth,
                });
                i = j;
            }
            '(' => {
                tokens.push(Token {
                    kind: Tok::Punct('('),
                    start,
                    end: pos(i + 1),
                    depth,
                });
                depth += 1;
                i += 1;
            }
            ')' => {
                if depth == 0 {
                    return Err(malformed("unbalanced parentheses"));
                }
                depth -= 1;
                tokens.push(Token {
                    kind: Tok::Punct(')'),
                    start,
                    end: pos(i + 1),
                    depth,
                });
                i += 1;
            }
            other => {
                tokens.push(Token {
                    kind: Tok::Punct(other),
                    start,
                    end: pos(i + 1),
                    depth,
                });
                i += 1;
            }
        }
    }
    if depth != 0 {
        return Err(malformed("unbalanced parentheses"));
    }
    Ok(tokens)
}

/// Functions refused for every alias, any access level: they reach the
/// server's filesystem, network or session settings, other backends, or run
/// arbitrary SQL from a string. `name` matches exactly or, for entries ending
/// in `*`, as a prefix. Lowercase.
const ALWAYS_DENIED_FUNCTIONS: &[&str] = &[
    // PostgreSQL
    "set_config",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_promote",
    "pg_notify",
    "pg_stat_file",
    "pg_stat_reset*",
    "pg_switch_*",
    "pg_create_*",
    "pg_drop_*",
    "pg_read_*",
    "pg_ls_*",
    "pg_advisory_*",
    "pg_try_advisory_*",
    "pg_logical_*",
    "pg_replication_*",
    "pg_wal_replay_*",
    "pg_backup_*",
    "pg_start_backup",
    "pg_stop_backup",
    "pg_import_*",
    "pg_log_backend_memory_contexts",
    "pg_file_*",
    "lo_*",
    "dblink*",
    "query_to_xml*",
    "table_to_xml*",
    "schema_to_xml*",
    "database_to_xml*",
    "cursor_to_xml",
    // MySQL / MariaDB
    "load_file",
    "sys_exec",
    "sys_eval",
    "get_lock",
    "release_lock",
    "release_all_locks",
    "benchmark",
    "master_pos_wait",
    "source_pos_wait",
    // SQLite
    "load_extension",
    "readfile",
    "writefile",
    "edit",
    "fts3_tokenizer",
];

/// ClickHouse table functions that read files, the network or other servers.
const CLICKHOUSE_DENIED_FUNCTIONS: &[&str] = &[
    "file",
    "filecluster",
    "url",
    "urlcluster",
    "s3",
    "s3cluster",
    "hdfs",
    "hdfscluster",
    "remote",
    "remotesecure",
    "cluster",
    "clusterallreplicas",
    "mysql",
    "postgresql",
    "mongodb",
    "odbc",
    "jdbc",
    "sqlite",
    "redis",
    "executable",
    "input",
    "azureblobstorage",
    "azureblobstoragecluster",
    "gcs",
    "deltalake",
    "iceberg",
    "hudi",
];

/// State-changing functions that are acceptable for write-enabled aliases.
const WRITE_FUNCTIONS: &[&str] = &["nextval", "setval"];

fn matches_denylist(name: &str, list: &[&str]) -> bool {
    list.iter().any(|entry| match entry.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *entry,
    })
}

/// Keywords after which `INTO` is part of an INSERT/MERGE/REPLACE.
const INTO_OWNERS: &[&str] = &[
    "INSERT",
    "REPLACE",
    "MERGE",
    "IGNORE",
    "LOW_PRIORITY",
    "HIGH_PRIORITY",
    "DELAYED",
    "OVERWRITE",
];

const DML_VERBS: &[&str] = &["INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT"];

/// Words that may sit between EXPLAIN and the explained statement.
const EXPLAIN_MODIFIERS: &[&str] = &[
    "ANALYZE",
    "ANALYSE",
    "VERBOSE",
    "QUERY",
    "PLAN",
    "AST",
    "SYNTAX",
    "PIPELINE",
    "ESTIMATE",
    "EXTENDED",
    "PARTITIONS",
    "FORMAT",
    "TREE",
    "JSON",
    "XML",
    "YAML",
    "TEXT",
    "COSTS",
    "BUFFERS",
    "TIMING",
    "SUMMARY",
    "WAL",
    "SETTINGS",
    "ACTIONS",
    "HEADER",
    "DESCRIPTION",
    "GRAPH",
    "COMPACT",
    "INDEXES",
    "TRUE",
    "FALSE",
    "ON",
    "OFF",
    "FOR",
    "CONNECTION",
];

/// SQLite PRAGMAs that only read, usable without arguments.
const SQLITE_READ_PRAGMAS: &[&str] = &[
    "database_list",
    "collation_list",
    "compile_options",
    "function_list",
    "module_list",
    "pragma_list",
    "table_list",
    "foreign_keys",
    "schema_version",
    "user_version",
    "application_id",
    "page_count",
    "page_size",
    "freelist_count",
    "encoding",
    "journal_mode",
    "data_version",
    "integrity_check",
    "quick_check",
    "foreign_key_check",
];

/// SQLite PRAGMAs that read and take a table (or count) argument.
const SQLITE_ARG_PRAGMAS: &[&str] = &[
    "table_info",
    "table_xinfo",
    "table_list",
    "index_list",
    "index_info",
    "index_xinfo",
    "foreign_key_list",
    "foreign_key_check",
    "integrity_check",
    "quick_check",
];

/// A statement the broker accepted.
#[derive(Debug, Clone)]
pub struct BrokerStatement {
    pub class: StatementClass,
    /// True for SELECT/WITH/VALUES/TABLE statements that return rows and
    /// accept an appended LIMIT.
    pub is_select: bool,
    /// The statement's leading keyword, ASCII-uppercased (`SELECT`, `WITH`,
    /// `EXPLAIN`, `INSERT`, …).
    pub verb: String,
    /// The single statement, without surrounding comments or `;`. This is the
    /// text to execute: exactly the tokens that were classified.
    pub sql: String,
    tokens: Vec<Token>,
    /// Offset of `sql` in the original text (to rebase token spans).
    base: usize,
}

impl BrokerStatement {
    /// The statement with a row limit of at most `rows` applied: an existing
    /// literal top-level LIMIT is lowered, a missing one is appended, and
    /// anything more exotic (FETCH FIRST, LIMIT BY, locking clauses…) is
    /// wrapped in a subquery. Non-SELECT statements are returned unchanged.
    pub fn with_row_limit(&self, rows: u32) -> String {
        if !self.is_select {
            return self.sql.clone();
        }
        let top: Vec<&Token> = self.tokens.iter().filter(|t| t.depth == 0).collect();
        let blocker = top.iter().any(|t| {
            matches!(
                t.word(),
                Some("FETCH" | "SETTINGS" | "FORMAT" | "FOR" | "LOCK")
            )
        });
        let wrap = || {
            format!(
                "SELECT * FROM (\n{}\n) AS dbcrust_limited\nLIMIT {rows}",
                self.sql
            )
        };
        if blocker {
            return wrap();
        }
        let Some(limit_pos) = self
            .tokens
            .iter()
            .rposition(|t| t.depth == 0 && t.is_word("LIMIT"))
        else {
            return format!("{}\nLIMIT {rows}", self.sql);
        };
        let tail = &self.tokens[limit_pos + 1..];
        let number_at = |i: usize| -> Option<(u64, usize, usize)> {
            let t = tail.get(i)?;
            if t.kind != Tok::Number {
                return None;
            }
            let text = &self.sql[t.start - self.base..t.end - self.base];
            Some((text.parse::<u64>().ok()?, t.start, t.end))
        };
        // LIMIT n | LIMIT n OFFSET m | LIMIT m, n  (count is the last LIMIT arg)
        let count = match tail.len() {
            1 => number_at(0),
            3 if tail[1].is_word("OFFSET") => number_at(0).filter(|_| number_at(2).is_some()),
            3 if tail[1].is_punct(',') => number_at(0).and(number_at(2)),
            _ => None,
        };
        match count {
            Some((n, start, end)) if n > u64::from(rows) => {
                let mut limited = self.sql.clone();
                limited.replace_range(start - self.base..end - self.base, &rows.to_string());
                limited
            }
            Some(_) => self.sql.clone(),
            None => wrap(),
        }
    }
}

fn dialect_for(engine: &DatabaseType) -> Result<Dialect, Refusal> {
    match engine {
        DatabaseType::PostgreSQL => Ok(Dialect::Postgres),
        DatabaseType::MySQL => Ok(Dialect::MySql),
        DatabaseType::SQLite => Ok(Dialect::Sqlite),
        DatabaseType::ClickHouse => Ok(Dialect::ClickHouse),
        DatabaseType::DuckDB => Ok(Dialect::DuckDb),
        other => Err(Refusal::new(
            RefusalKind::Ddl,
            format!("{other} is not supported by the broker"),
        )),
    }
}

/// Classify one statement for the credential broker.
///
/// Refuses (never executes) multi-statement input, anything that is not a
/// plain query or DML, DDL/DCL/admin statements, file/network/session
/// side-effect functions, `SELECT … INTO`, and anything lexically malformed.
/// DML is *classified* as [`StatementClass::Write`]; whether the alias may
/// write is the caller's decision (see [`BrokerStatement::class`]).
pub fn classify_for_broker(sql: &str, engine: &DatabaseType) -> Result<BrokerStatement, Refusal> {
    let dialect = dialect_for(engine)?;
    let all = lex(sql, dialect)?;

    // Split on top-level `;`: trailing empty statements are fine
    let first_semicolon = all.iter().position(|t| t.is_punct(';'));
    let (body, rest): (&[Token], &[Token]) = match first_semicolon {
        Some(i) => (&all[..i], &all[i + 1..]),
        None => (&all[..], &[]),
    };
    if rest.iter().any(|t| !t.is_punct(';')) {
        // A refused first statement (e.g. CREATE TRIGGER … BEGIN …; END) is
        // reported as what it is rather than as a batch
        if !body.is_empty() {
            classify_body(body, dialect)?;
        }
        return Err(Refusal::new(
            RefusalKind::MultiStatement,
            "only one statement per request is allowed",
        ));
    }
    if body.is_empty() {
        return Err(Refusal::new(RefusalKind::Empty, "the statement is empty"));
    }

    let (class, is_select) = classify_body(body, dialect)?;
    if dialect == Dialect::DuckDb {
        // Quoted function names (`"read_csv"(…)`) and files named where a
        // table belongs: the word-level checks of `classify_body` miss both
        if let Some(name) = duckdb_denied_call(sql, body) {
            return Err(Refusal::new(
                RefusalKind::Ddl,
                format!("function {name}() reaches outside the database and is not allowed"),
            ));
        }
        if duckdb_replacement_scan(sql, body).is_some() {
            return Err(Refusal::new(
                RefusalKind::Ddl,
                "reading a file by name (FROM 'file.csv') is not allowed through the broker",
            ));
        }
    }
    let base = body[0].start;
    let end = body[body.len() - 1].end;
    let verb = body
        .iter()
        .find_map(Token::word)
        .unwrap_or_default()
        .to_string();
    Ok(BrokerStatement {
        class,
        is_select,
        verb,
        sql: sql[base..end].to_string(),
        tokens: body.to_vec(),
        base,
    })
}

fn refuse_kind(word: &str) -> Refusal {
    let what = match word {
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "RENAME" | "COMMENT" | "REINDEX" | "CLUSTER"
        | "OPTIMIZE" | "VACUUM" | "ANALYZE" | "ANALYSE" | "REFRESH" => {
            "schema/maintenance statements"
        }
        "GRANT" | "REVOKE" | "SECURITY" => "privilege statements",
        "COPY" | "LOAD" | "INSTALL" | "IMPORT" | "ATTACH" | "DETACH" | "SYSTEM" => {
            "file/extension/server statements"
        }
        "SET" | "RESET" | "USE" | "DISCARD" => "session-changing statements",
        "BEGIN" | "START" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "ABORT" => {
            "transaction control"
        }
        "DO" | "CALL" | "EXECUTE" | "PREPARE" | "DEALLOCATE" | "EXEC" => "procedural statements",
        "LISTEN" | "NOTIFY" | "UNLISTEN" | "LOCK" | "KILL" | "FLUSH" | "HANDLER" | "CHECKPOINT" => {
            "administrative statements"
        }
        "PRAGMA" => "PRAGMA statements",
        _ => "this kind of statement",
    };
    Refusal::new(
        RefusalKind::Ddl,
        format!("{what} ({word}) are not allowed through the broker"),
    )
}

fn classify_body(tokens: &[Token], dialect: Dialect) -> Result<(StatementClass, bool), Refusal> {
    // Leading parentheses: `(SELECT 1) UNION (SELECT 2)`
    let first = tokens
        .iter()
        .position(|t| !t.is_punct('('))
        .ok_or_else(|| Refusal::new(RefusalKind::Malformed, "the statement is empty"))?;
    let Some(verb) = tokens[first].word() else {
        return Err(Refusal::new(
            RefusalKind::Ddl,
            "the statement does not start with a recognised keyword",
        ));
    };

    let mut class;
    let mut is_select = false;
    match verb {
        "SELECT" | "VALUES" => {
            class = StatementClass::Read;
            is_select = true;
        }
        "TABLE" if matches!(dialect, Dialect::Postgres | Dialect::MySql) => {
            class = StatementClass::Read;
            is_select = true;
        }
        "WITH" => {
            // The main statement follows the CTE list at depth 0
            let main = tokens[first + 1..]
                .iter()
                .find(|t| {
                    t.depth == tokens[first].depth
                        && t.word().is_some_and(|w| {
                            matches!(w, "SELECT" | "VALUES" | "TABLE") || DML_VERBS.contains(&w)
                        })
                })
                .and_then(Token::word)
                .ok_or_else(|| {
                    Refusal::new(RefusalKind::Ddl, "WITH without a SELECT or DML statement")
                })?;
            if DML_VERBS.contains(&main) {
                class = StatementClass::Write;
            } else {
                class = StatementClass::Read;
                is_select = true;
            }
        }
        "SHOW" | "DESCRIBE" | "DESC" if dialect != Dialect::Sqlite => {
            class = StatementClass::Read;
        }
        // DuckDB's FROM-first query and its table-shaped utility statements
        "FROM" if dialect == Dialect::DuckDb => {
            class = StatementClass::Read;
            is_select = true;
        }
        "SUMMARIZE" | "PIVOT" | "UNPIVOT" if dialect == Dialect::DuckDb => {
            class = StatementClass::Read;
        }
        "EXISTS" if dialect == Dialect::ClickHouse => {
            class = StatementClass::Read;
        }
        "EXPLAIN" => {
            class = classify_explain(&tokens[first + 1..], dialect)?;
        }
        "PRAGMA" if dialect == Dialect::Sqlite => {
            classify_pragma(&tokens[first + 1..])?;
            class = StatementClass::Read;
        }
        "PRAGMA" if dialect == Dialect::DuckDb => {
            duckdb_pragma_allowed(&tokens[first + 1..])
                .map_err(|reason| Refusal::new(RefusalKind::Ddl, reason))?;
            class = StatementClass::Read;
        }
        v if DML_VERBS.contains(&v) => {
            class = StatementClass::Write;
        }
        other => return Err(refuse_kind(other)),
    }

    // Body-wide checks: apply to every nested token, including subqueries
    // and the inner statement of EXPLAIN
    for (i, tok) in tokens.iter().enumerate() {
        let Some(word) = tok.word() else { continue };
        let prev = i.checked_sub(1).and_then(|p| tokens.get(p));
        let next = tokens.get(i + 1);

        // SELECT ... INTO / INTO OUTFILE / INTO @var
        if word == "INTO" {
            let owned = prev
                .and_then(Token::word)
                .is_some_and(|w| INTO_OWNERS.contains(&w));
            if !owned {
                return Err(Refusal::new(
                    RefusalKind::Ddl,
                    "SELECT ... INTO creates tables or files and is not allowed",
                ));
            }
        }

        // Function calls: the word is directly followed by `(`
        if next.is_some_and(|n| n.is_punct('(')) {
            let name = word.to_ascii_lowercase();
            if matches_denylist(&name, ALWAYS_DENIED_FUNCTIONS)
                || (dialect == Dialect::ClickHouse
                    && matches_denylist(&name, CLICKHOUSE_DENIED_FUNCTIONS))
                || (dialect == Dialect::DuckDb && matches_denylist(&name, DUCKDB_DENIED_FUNCTIONS))
            {
                return Err(Refusal::new(
                    RefusalKind::Ddl,
                    format!("function {name}() reaches outside the database and is not allowed"),
                ));
            }
            if WRITE_FUNCTIONS.contains(&name.as_str()) {
                class = StatementClass::Write;
            }
        }

        // Writable CTE / subquery: a DML verb opening a parenthesised group
        if DML_VERBS.contains(&word)
            && prev.is_some_and(|p| p.is_punct('('))
            && !next.is_some_and(|n| n.is_punct('('))
        {
            class = StatementClass::Write;
        }

        // Locking reads take row locks: treat as writes
        if word == "FOR"
            && next
                .and_then(Token::word)
                .is_some_and(|w| matches!(w, "UPDATE" | "SHARE" | "NO" | "KEY"))
        {
            class = StatementClass::Write;
        }
        if word == "LOCK" && next.is_some_and(|n| n.is_word("IN")) {
            class = StatementClass::Write;
        }

        // ClickHouse per-statement settings and output formats
        if dialect == Dialect::ClickHouse
            && tok.depth == 0
            && matches!(word, "SETTINGS" | "FORMAT")
            && verb != "EXPLAIN"
            && verb != "SHOW"
        {
            return Err(Refusal::new(
                RefusalKind::Ddl,
                "SETTINGS/FORMAT clauses are not allowed through the broker",
            ));
        }
    }

    Ok((class, is_select))
}

fn classify_explain(tokens: &[Token], dialect: Dialect) -> Result<StatementClass, Refusal> {
    let mut analyze = false;
    let mut i = 0usize;
    while i < tokens.len() {
        let t = &tokens[i];
        if t.is_punct('(') {
            // Option list: find the matching close
            let mut j = i + 1;
            while j < tokens.len() && !(tokens[j].is_punct(')') && tokens[j].depth == t.depth) {
                if matches!(tokens[j].word(), Some("ANALYZE" | "ANALYSE")) {
                    analyze = true;
                }
                j += 1;
            }
            if i == 0 || tokens[i - 1].word().is_some() || j >= tokens.len() {
                // `EXPLAIN (SELECT ...)`: not an option list when it contains
                // a statement keyword — fall through to statement parsing
                let looks_like_statement = tokens[i + 1..j.min(tokens.len())]
                    .first()
                    .and_then(Token::word)
                    .is_some_and(|w| matches!(w, "SELECT" | "WITH" | "VALUES"));
                if looks_like_statement {
                    break;
                }
            }
            i = j + 1;
            continue;
        }
        match &t.kind {
            Tok::Word(w) if EXPLAIN_MODIFIERS.contains(&w.as_str()) => {
                if w == "ANALYZE" || w == "ANALYSE" {
                    analyze = true;
                }
                i += 1;
            }
            Tok::Punct('=') | Tok::Punct(',') | Tok::Number => i += 1,
            _ => break,
        }
    }
    let inner = &tokens[i.min(tokens.len())..];
    if inner.is_empty() {
        return Err(Refusal::new(
            RefusalKind::Malformed,
            "EXPLAIN without a statement",
        ));
    }
    // MySQL: `EXPLAIN tablename` is DESCRIBE
    let starts_statement = inner.first().and_then(Token::word).is_some_and(|w| {
        DML_VERBS.contains(&w)
            || matches!(
                w,
                "SELECT"
                    | "WITH"
                    | "VALUES"
                    | "TABLE"
                    | "CREATE"
                    | "ALTER"
                    | "DROP"
                    | "TRUNCATE"
                    | "GRANT"
                    | "REVOKE"
                    | "SET"
                    | "CALL"
                    | "DO"
                    | "LOAD"
                    | "COPY"
                    | "USE"
                    | "KILL"
                    | "FLUSH"
                    | "EXPLAIN"
            )
    });
    let is_bare_name = dialect == Dialect::MySql
        && !starts_statement
        && inner.len() <= 5
        && inner
            .iter()
            .all(|t| matches!(t.kind, Tok::Word(_) | Tok::Quoted | Tok::Punct('.')));
    if is_bare_name {
        return Ok(StatementClass::Read);
    }
    let (inner_class, _) = classify_body(inner, dialect)?;
    // EXPLAIN without ANALYZE plans but never executes the statement
    Ok(if analyze {
        inner_class
    } else {
        StatementClass::Read
    })
}

fn classify_pragma(tokens: &[Token]) -> Result<(), Refusal> {
    let refuse = |why: &str| Err(Refusal::new(RefusalKind::Ddl, why.to_string()));
    // [schema.]name
    let mut i = 0usize;
    let mut name = tokens
        .get(i)
        .and_then(Token::word)
        .map(str::to_ascii_lowercase);
    if tokens.get(i + 1).is_some_and(|t| t.is_punct('.')) {
        i += 2;
        name = tokens
            .get(i)
            .and_then(Token::word)
            .map(str::to_ascii_lowercase);
    }
    let Some(name) = name else {
        return refuse("PRAGMA statement not recognised");
    };
    let rest = &tokens[i + 1..];
    match rest.first() {
        None => {
            if SQLITE_READ_PRAGMAS.contains(&name.as_str())
                || SQLITE_ARG_PRAGMAS.contains(&name.as_str())
            {
                Ok(())
            } else {
                refuse("only read-only PRAGMAs are allowed")
            }
        }
        Some(t) if t.is_punct('=') => refuse("PRAGMA assignments modify database state"),
        Some(t) if t.is_punct('(') => {
            if SQLITE_ARG_PRAGMAS.contains(&name.as_str()) {
                Ok(())
            } else {
                refuse("this PRAGMA is not allowed with an argument")
            }
        }
        Some(_) => refuse("PRAGMA statement not recognised"),
    }
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

    #[rstest]
    #[case::qualifier("patch.code:memcpy repo:github.com/postgres/postgres", true)]
    #[case::regex("/EXPORT_SYMBOL(_GPL)?\\(/ tags:C", true)]
    #[case::sql("SELECT COUNT(*) FROM documents WHERE patch.code MATCH 'panic!'", true)]
    #[case::empty("", false)]
    fn test_check_read_only_white_dragon(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::WhiteDragon).is_ok(),
            allowed,
            "{query}"
        );
    }

    #[rstest]
    // reads
    #[case::get("GET user:1", true)]
    #[case::lowercase("get user:1", true)]
    #[case::quoted_key("GET \"user 1\"", true)]
    #[case::terminated("GET user:1;", true)]
    #[case::mget("MGET a b c", true)]
    #[case::strlen("STRLEN k", true)]
    #[case::exists("EXISTS a b", true)]
    #[case::type_("TYPE k", true)]
    #[case::ttl("TTL k", true)]
    #[case::pttl("PTTL k", true)]
    #[case::scan("SCAN 0 MATCH user:* COUNT 100", true)]
    #[case::hscan("HSCAN h 0", true)]
    #[case::sscan("SSCAN s 0", true)]
    #[case::zscan("ZSCAN z 0", true)]
    #[case::hgetall("HGETALL h", true)]
    #[case::hget("HGET h f", true)]
    #[case::hlen("HLEN h", true)]
    #[case::lrange("LRANGE l 0 10", true)]
    #[case::llen("LLEN l", true)]
    #[case::smembers("SMEMBERS s", true)]
    #[case::scard("SCARD s", true)]
    #[case::zrange("ZRANGE z 0 -1 WITHSCORES", true)]
    #[case::zrangebyscore("ZRANGEBYSCORE z 0 10", true)]
    #[case::zscore("ZSCORE z m", true)]
    #[case::xrange("XRANGE st - + COUNT 10", true)]
    #[case::xlen("XLEN st", true)]
    #[case::xread("XREAD COUNT 5 STREAMS st 0", true)]
    #[case::xinfo_stream("XINFO STREAM st", true)]
    #[case::info("INFO", true)]
    #[case::info_section("INFO keyspace", true)]
    #[case::dbsize("DBSIZE", true)]
    #[case::ping("PING", true)]
    #[case::memory_usage("MEMORY USAGE k", true)]
    #[case::object_encoding("OBJECT ENCODING k", true)]
    #[case::client_info("CLIENT INFO", true)]
    #[case::command_count("COMMAND COUNT", true)]
    #[case::sort_ro("SORT_RO l ALPHA", true)]
    #[case::geosearch("GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 5 km", true)]
    // writes and admin: default-deny
    #[case::set("SET k v", false)]
    #[case::del("DEL k", false)]
    #[case::expire("EXPIRE k 10", false)]
    #[case::incr("INCR n", false)]
    #[case::hset("HSET h f v", false)]
    #[case::lpush("LPUSH l v", false)]
    #[case::sadd("SADD s m", false)]
    #[case::zadd("ZADD z 1 m", false)]
    #[case::xadd("XADD st * f v", false)]
    #[case::rename("RENAME a b", false)]
    #[case::getdel("GETDEL k", false)]
    #[case::sort("SORT l STORE out", false)]
    #[case::zunionstore("ZUNIONSTORE out 1 z", false)]
    #[case::georadius("GEORADIUS g 0 0 5 km STORE out", false)]
    #[case::pfadd("PFADD h a", false)]
    #[case::unknown("MYNEWCOMMAND k", false)]
    #[case::module_command("JSON.SET k $ 1", false)]
    #[case::object_other("OBJECT FOO k", false)]
    #[case::memory_purge("MEMORY PURGE", false)]
    #[case::client_kill("CLIENT KILL ID 3", false)]
    #[case::client_bare("CLIENT", false)]
    #[case::command_other("COMMAND FOO", false)]
    #[case::xinfo_other("XINFO SOMETHING st", false)]
    // always refused
    #[case::flushall("FLUSHALL", false)]
    #[case::flushall_lower("flushall async", false)]
    #[case::flushdb("FLUSHDB", false)]
    #[case::config_get("CONFIG GET maxmemory", false)]
    #[case::config_set("CONFIG SET maxmemory 0", false)]
    #[case::debug("DEBUG SLEEP 0", false)]
    #[case::shutdown("SHUTDOWN NOSAVE", false)]
    #[case::module("MODULE LOAD /x.so", false)]
    #[case::script("SCRIPT FLUSH", false)]
    #[case::eval("EVAL \"return 1\" 0", false)]
    #[case::evalsha("EVALSHA abc 0", false)]
    #[case::fcall("FCALL f 0", false)]
    #[case::keys("KEYS *", false)]
    #[case::monitor("MONITOR", false)]
    #[case::subscribe("SUBSCRIBE ch", false)]
    #[case::blpop("BLPOP l 0", false)]
    #[case::brpop("BRPOP l 0", false)]
    #[case::xread_block("XREAD BLOCK 0 STREAMS st $", false)]
    #[case::xread_block_lower("xread block 100 streams st $", false)]
    #[case::xreadgroup("XREADGROUP GROUP g c STREAMS st >", false)]
    #[case::acl("ACL LIST", false)]
    #[case::select("SELECT 1", false)]
    #[case::multi("MULTI", false)]
    #[case::slowlog("SLOWLOG GET", false)]
    // malformed lines never pass
    #[case::empty("", false)]
    #[case::unbalanced("GET \"k", false)]
    #[case::sql_looking("SELECT * FROM t", false)]
    fn test_check_read_only_redis(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::Redis).is_ok(),
            allowed,
            "{query}"
        );
    }

    #[test]
    fn test_redis_refusal_names_the_command_and_the_alternative() {
        let err = check_read_only("KEYS *", &DatabaseType::Redis).unwrap_err();
        assert!(
            err.reason.contains("KEYS") && err.reason.contains("SCAN"),
            "{err}"
        );
        let err = check_read_only("FOO", &DatabaseType::Redis).unwrap_err();
        assert!(err.reason.contains("allowlist"), "{err}");
        let err = check_read_only("OBJECT FOO k", &DatabaseType::Redis).unwrap_err();
        assert!(err.reason.contains("OBJECT FOO"), "{err}");
    }

    #[rstest]
    #[case::ls("ls", true)]
    #[case::ls_prefix("ls logs/2026/ -r -n 10", true)]
    #[case::head("head a.txt", true)]
    #[case::get("get a.txt bytes=0-99", true)]
    #[case::buckets("buckets", true)]
    #[case::cd("cd ..", true)]
    #[case::help("help", true)]
    #[case::put("put a.txt", false)]
    #[case::rm("rm a.txt", false)]
    #[case::cp("cp a b", false)]
    #[case::presign("presign a.txt", false)]
    #[case::sql("SELECT * FROM t", false)]
    #[case::drop_table("DROP TABLE t", false)]
    #[case::empty("", false)]
    #[case::unbalanced("get 'a", false)]
    fn test_check_read_only_s3(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::S3).is_ok(),
            allowed,
            "{query}"
        );
    }

    #[test]
    fn test_s3_refusal_says_the_client_only_reads() {
        let err = check_read_only("rm a.txt", &DatabaseType::S3).unwrap_err();
        assert!(is_read_only_violation_message(&err.to_string()));
        assert!(err.reason.contains("only reads"), "{err}");
    }

    #[rstest]
    #[case::tables("tables", true)]
    #[case::describe("describe Orders", true)]
    #[case::scan("scan Orders --limit 10", true)]
    #[case::get("get Orders id=1 sk=2", true)]
    #[case::query("query Orders id=1 --index by_date", true)]
    #[case::help("help", true)]
    #[case::select("SELECT * FROM \"Orders\" WHERE id = '1'", true)]
    #[case::select_lower_semicolon("select * from \"Orders\";", true)]
    #[case::select_comment("-- note\nSELECT * FROM \"Orders\"", true)]
    #[case::select_keyword_in_literal("SELECT * FROM \"Orders\" WHERE n = 'DELETE; DROP'", true)]
    #[case::select_keyword_in_identifier("SELECT * FROM \"DELETE\"", true)]
    #[case::insert("INSERT INTO \"Orders\" VALUE {'id': '1'}", false)]
    #[case::update("UPDATE \"Orders\" SET a = 1 WHERE id = '1'", false)]
    #[case::delete("DELETE FROM \"Orders\" WHERE id = '1'", false)]
    #[case::exists("EXISTS(SELECT * FROM \"Orders\" WHERE id = '1')", false)]
    #[case::select_then_delete("SELECT * FROM \"A\"; DELETE FROM \"A\" WHERE id = '1'", false)]
    #[case::comment_hides_write("/* SELECT */ DELETE FROM \"A\" WHERE id = '1'", false)]
    #[case::put("put Orders id=1", false)]
    #[case::sql_drop("DROP TABLE Orders", false)]
    #[case::empty("", false)]
    #[case::unterminated("SELECT * FROM \"A", false)]
    fn test_check_read_only_dynamodb(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::DynamoDB).is_ok(),
            allowed,
            "{query}"
        );
    }

    #[test]
    fn test_dynamodb_refusal_says_the_client_only_reads() {
        let err = check_read_only("DELETE FROM \"Orders\"", &DatabaseType::DynamoDB).unwrap_err();
        assert!(is_read_only_violation_message(&err.to_string()));
        assert!(err.reason.contains("only reads"), "{err}");
    }

    #[rstest]
    #[case::groups("groups /aws/lambda/", true)]
    #[case::streams("streams /aws/lambda/a -n 5", true)]
    #[case::tail("tail /app --since 5m --filter ERROR", true)]
    #[case::use_groups("use /a,/b", true)]
    #[case::help("help", true)]
    #[case::fields("fields @timestamp, @message | sort @timestamp desc | limit 20", true)]
    #[case::stats("stats count(*) by bin(5m)", true)]
    #[case::since("since 30m filter @message like /error/", true)]
    #[case::between("between 2026-10-16 08:00 and now fields @message", true)]
    #[case::group_flag("--group /a fields @message", true)]
    #[case::put("put /app hello", false)]
    #[case::delete("delete /app", false)]
    #[case::create("create log group x", false)]
    #[case::retention("retention /app 7", false)]
    #[case::sql("DROP TABLE x", false)]
    #[case::window_only("since 5m", false)]
    #[case::empty("", false)]
    fn test_check_read_only_cloudwatch(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::CloudWatchLogs).is_ok(),
            allowed,
            "{query}"
        );
    }

    #[test]
    fn test_cloudwatch_refusal_says_the_client_only_reads() {
        let err = check_read_only("delete /app", &DatabaseType::CloudWatchLogs).unwrap_err();
        assert!(is_read_only_violation_message(&err.to_string()));
        assert!(err.reason.contains("only reads"), "{err}");
    }

    #[test]
    fn test_violation_marker() {
        let err = check_read_only("DROP TABLE t", &DatabaseType::PostgreSQL).unwrap_err();
        assert!(is_read_only_violation_message(&err.to_string()));
        assert!(!is_read_only_violation_message("some other error"));
    }

    // ---- credential-broker classifier --------------------------------------

    fn engine(name: &str) -> DatabaseType {
        match name {
            "pg" => DatabaseType::PostgreSQL,
            "mysql" => DatabaseType::MySQL,
            "sqlite" => DatabaseType::SQLite,
            "clickhouse" => DatabaseType::ClickHouse,
            "duckdb" => DatabaseType::DuckDB,
            other => panic!("unknown engine {other}"),
        }
    }

    #[rstest]
    #[case::pg_select("pg", "SELECT * FROM users", "read")]
    #[case::pg_select_semicolon("pg", "SELECT 1;", "read")]
    #[case::pg_select_semicolons("pg", "SELECT 1;;  ;", "read")]
    #[case::pg_select_leading_comment("pg", "/* hi */ -- there\nSELECT 1", "read")]
    #[case::pg_select_trailing_comment("pg", "SELECT 1 -- trailing", "read")]
    #[case::pg_with_select("pg", "WITH a AS (SELECT 1) SELECT * FROM a", "read")]
    #[case::pg_with_recursive(
        "pg",
        "WITH RECURSIVE t(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM t WHERE n < 5) SELECT * FROM t",
        "read"
    )]
    #[case::pg_values("pg", "VALUES (1),(2)", "read")]
    #[case::pg_table("pg", "TABLE users", "read")]
    #[case::pg_show("pg", "SHOW search_path", "read")]
    #[case::pg_explain("pg", "EXPLAIN SELECT * FROM users", "read")]
    #[case::pg_explain_options("pg", "EXPLAIN (FORMAT JSON, VERBOSE) SELECT * FROM users", "read")]
    #[case::pg_explain_analyze_select("pg", "EXPLAIN ANALYZE SELECT * FROM users", "read")]
    #[case::pg_explain_analyze_paren("pg", "EXPLAIN (ANALYZE, BUFFERS) SELECT 1", "read")]
    #[case::pg_explain_delete_plain("pg", "EXPLAIN DELETE FROM users", "read")]
    #[case::pg_explain_analyze_delete("pg", "EXPLAIN ANALYZE DELETE FROM users", "write")]
    #[case::pg_explain_paren_analyze_update(
        "pg",
        "EXPLAIN (ANALYZE true) UPDATE users SET a = 1",
        "write"
    )]
    #[case::pg_explain_create_table("pg", "EXPLAIN CREATE TABLE x (a int)", "ddl")]
    #[case::pg_semicolon_in_string("pg", "SELECT 'a;b'", "read")]
    #[case::pg_semicolon_in_ident("pg", "SELECT \"a;b\" FROM t", "read")]
    #[case::pg_semicolon_in_dollar("pg", "SELECT $$a; DROP TABLE t$$", "read")]
    #[case::pg_semicolon_in_tagged_dollar("pg", "SELECT $q$a; DROP TABLE t$q$", "read")]
    #[case::pg_semicolon_in_block_comment("pg", "SELECT 1 /* ; DROP TABLE t */", "read")]
    #[case::pg_semicolon_in_line_comment("pg", "SELECT 1 -- ; DROP TABLE t", "read")]
    #[case::pg_nested_block_comment("pg", "SELECT 1 /* a /* ; */ b */", "read")]
    #[case::pg_escape_string_quote("pg", "SELECT E'it\\'s; fine'", "read")]
    #[case::pg_standard_string_backslash("pg", "SELECT 'a\\'; DROP TABLE t; --'", "multi")]
    #[case::pg_positional_param("pg", "SELECT * FROM t WHERE a = $1", "read")]
    #[case::pg_multi_statement("pg", "SELECT 1; DROP TABLE t", "multi")]
    #[case::pg_multi_select("pg", "SELECT 1; SELECT 2", "multi")]
    #[case::pg_multi_after_comment("pg", "SELECT 1; /* x */ SELECT 2", "multi")]
    #[case::pg_multi_hidden_by_comment_tail_ok("pg", "SELECT 1; -- SELECT 2", "read")]
    #[case::pg_unterminated_string("pg", "SELECT 'abc", "malformed")]
    #[case::pg_unterminated_comment("pg", "SELECT 1 /* abc", "malformed")]
    #[case::pg_unbalanced_paren("pg", "SELECT (1", "malformed")]
    #[case::pg_extra_close_paren("pg", "SELECT 1)", "malformed")]
    #[case::pg_empty("pg", "", "empty")]
    #[case::pg_nul_byte("pg", "SELECT 1\u{0}; DROP TABLE t", "malformed")]
    #[case::my_table_stmt("mysql", "TABLE users", "read")]
    #[case::pg_only_comment("pg", "-- nothing", "empty")]
    #[case::pg_insert("pg", "INSERT INTO t VALUES (1)", "write")]
    #[case::pg_update("pg", "UPDATE t SET a = 1", "write")]
    #[case::pg_delete("pg", "DELETE FROM t", "write")]
    #[case::pg_merge(
        "pg",
        "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
        "write"
    )]
    #[case::pg_writable_cte("pg", "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", "write")]
    #[case::pg_writable_cte_insert(
        "pg",
        "WITH i AS (INSERT INTO t VALUES (1) RETURNING id) SELECT * FROM i",
        "write"
    )]
    #[case::pg_writable_cte_update(
        "pg",
        "WITH u AS (UPDATE t SET a = 1 RETURNING *) SELECT * FROM u",
        "write"
    )]
    #[case::pg_cte_main_insert("pg", "WITH s AS (SELECT 1) INSERT INTO t SELECT * FROM s", "write")]
    #[case::pg_select_for_update("pg", "SELECT * FROM t FOR UPDATE", "write")]
    #[case::pg_select_for_share("pg", "SELECT * FROM t FOR SHARE", "write")]
    #[case::pg_select_into("pg", "SELECT * INTO newt FROM t", "ddl")]
    #[case::pg_select_into_temp("pg", "SELECT 1 INTO TEMP x", "ddl")]
    #[case::pg_create_table_as("pg", "CREATE TABLE x AS SELECT 1", "ddl")]
    #[case::pg_create("pg", "CREATE TABLE x (a int)", "ddl")]
    #[case::pg_alter("pg", "ALTER TABLE t ADD COLUMN a int", "ddl")]
    #[case::pg_drop("pg", "DROP TABLE t", "ddl")]
    #[case::pg_drop_lower("pg", "drop table t", "ddl")]
    #[case::pg_truncate("pg", "TRUNCATE t", "ddl")]
    #[case::pg_grant("pg", "GRANT ALL ON t TO u", "ddl")]
    #[case::pg_revoke("pg", "REVOKE ALL ON t FROM u", "ddl")]
    #[case::pg_comment_on("pg", "COMMENT ON TABLE t IS 'x'", "ddl")]
    #[case::pg_vacuum("pg", "VACUUM t", "ddl")]
    #[case::pg_analyze("pg", "ANALYZE t", "ddl")]
    #[case::pg_reindex("pg", "REINDEX TABLE t", "ddl")]
    #[case::pg_cluster("pg", "CLUSTER t", "ddl")]
    #[case::pg_copy_program("pg", "COPY t TO PROGRAM 'rm -rf /'", "ddl")]
    #[case::pg_copy_stdout("pg", "COPY t TO STDOUT", "ddl")]
    #[case::pg_set_role("pg", "SET ROLE postgres", "ddl")]
    #[case::pg_set_session_auth("pg", "SET SESSION AUTHORIZATION postgres", "ddl")]
    #[case::pg_set_readonly_off("pg", "SET default_transaction_read_only = off", "ddl")]
    #[case::pg_reset("pg", "RESET ALL", "ddl")]
    #[case::pg_do("pg", "DO $$ BEGIN PERFORM 1; END $$", "ddl")]
    #[case::pg_call("pg", "CALL p()", "ddl")]
    #[case::pg_execute("pg", "EXECUTE stmt", "ddl")]
    #[case::pg_prepare("pg", "PREPARE s AS SELECT 1", "ddl")]
    #[case::pg_listen("pg", "LISTEN ch", "ddl")]
    #[case::pg_notify_stmt("pg", "NOTIFY ch", "ddl")]
    #[case::pg_lock("pg", "LOCK TABLE t", "ddl")]
    #[case::pg_begin("pg", "BEGIN READ WRITE", "ddl")]
    #[case::pg_commit("pg", "COMMIT", "ddl")]
    #[case::pg_fn_terminate("pg", "SELECT pg_terminate_backend(123)", "ddl")]
    #[case::pg_fn_cancel("pg", "SELECT pg_catalog.pg_cancel_backend(123)", "ddl")]
    #[case::pg_fn_read_file("pg", "SELECT pg_read_file('/etc/passwd')", "ddl")]
    #[case::pg_fn_ls_dir("pg", "SELECT pg_ls_dir('/')", "ddl")]
    #[case::pg_fn_lo_import("pg", "SELECT lo_import('/etc/passwd')", "ddl")]
    #[case::pg_fn_dblink(
        "pg",
        "SELECT * FROM dblink('host=evil', 'select 1') AS t(a int)",
        "ddl"
    )]
    #[case::pg_fn_set_config(
        "pg",
        "SELECT set_config('default_transaction_read_only', 'off', false)",
        "ddl"
    )]
    #[case::pg_fn_advisory("pg", "SELECT pg_advisory_lock(1)", "ddl")]
    #[case::pg_fn_query_to_xml(
        "pg",
        "SELECT query_to_xml('delete from t', false, false, '')",
        "ddl"
    )]
    #[case::pg_fn_quoted_not_call("pg", "SELECT \"pg_read_file\" FROM t", "read")]
    #[case::pg_fn_name_as_column_ok("pg", "SELECT lo_count FROM t", "read")]
    #[case::pg_nextval("pg", "SELECT nextval('s')", "write")]
    #[case::pg_setval("pg", "SELECT setval('s', 1)", "write")]
    #[case::pg_sleep_ok("pg", "SELECT pg_sleep(1)", "read")]
    #[case::pg_replace_function_ok(
        "pg",
        "SELECT replace(a, 'x', 'y') FROM t WHERE b IN (replace(c, 'a', 'b'))",
        "read"
    )]
    #[case::pg_column_named_update("pg", "SELECT a, update FROM t", "read")]
    #[case::pg_keyword_in_identifier("pg", "SELECT \"delete\" FROM \"drop\"", "read")]
    #[case::pg_keyword_in_string("pg", "SELECT 'DROP TABLE t; DELETE FROM t'", "read")]
    #[case::pg_drop_after_semicolon_in_cte_string("pg", "SELECT ';' AS a; DELETE FROM t", "multi")]
    #[case::my_select("mysql", "SELECT * FROM `users` LIMIT 10", "read")]
    #[case::my_show_tables("mysql", "SHOW TABLES", "read")]
    #[case::my_describe("mysql", "DESCRIBE users", "read")]
    #[case::my_desc("mysql", "DESC users", "read")]
    #[case::my_explain_table("mysql", "EXPLAIN users", "read")]
    #[case::my_explain_format("mysql", "EXPLAIN FORMAT=JSON SELECT * FROM users", "read")]
    #[case::my_explain_analyze_delete("mysql", "EXPLAIN ANALYZE DELETE FROM users", "write")]
    #[case::my_insert("mysql", "INSERT INTO t (a) VALUES (1)", "write")]
    #[case::my_insert_ignore("mysql", "INSERT IGNORE INTO t (a) VALUES (1)", "write")]
    #[case::my_replace("mysql", "REPLACE INTO t VALUES (1)", "write")]
    #[case::my_replace_function("mysql", "SELECT REPLACE(a, 'x', 'y') FROM t", "read")]
    #[case::my_insert_function("mysql", "SELECT INSERT('Quadratic', 3, 4, 'What')", "read")]
    #[case::my_conditional_comment("mysql", "SELECT 1 /*!50000 ; DROP TABLE t */", "ddl")]
    #[case::my_conditional_comment_stmt("mysql", "/*!50000 DROP TABLE t */", "ddl")]
    #[case::my_hash_comment("mysql", "SELECT 1 # ; DROP TABLE t", "read")]
    #[case::my_dash_comment_space("mysql", "SELECT 1 -- ; DROP TABLE t", "read")]
    #[case::my_dash_no_space_is_not_comment("mysql", "SELECT 1 --; DROP TABLE t", "multi")]
    #[case::my_backslash_escape("mysql", "SELECT 'a\\'; DROP TABLE t; --'", "read")]
    #[case::my_double_quoted_string("mysql", "SELECT \"a;b\"", "read")]
    #[case::my_into_outfile("mysql", "SELECT * FROM t INTO OUTFILE '/tmp/x'", "ddl")]
    #[case::my_into_dumpfile("mysql", "SELECT * FROM t INTO DUMPFILE '/tmp/x'", "ddl")]
    #[case::my_into_var("mysql", "SELECT a INTO @x FROM t", "ddl")]
    #[case::my_load_data("mysql", "LOAD DATA INFILE '/etc/passwd' INTO TABLE t", "ddl")]
    #[case::my_use("mysql", "USE mysql", "ddl")]
    #[case::my_set("mysql", "SET GLOBAL read_only = 0", "ddl")]
    #[case::my_set_transaction("mysql", "SET SESSION TRANSACTION READ WRITE", "ddl")]
    #[case::my_flush("mysql", "FLUSH PRIVILEGES", "ddl")]
    #[case::my_kill("mysql", "KILL 12", "ddl")]
    #[case::my_lock_in_share("mysql", "SELECT * FROM t LOCK IN SHARE MODE", "write")]
    #[case::my_load_file("mysql", "SELECT LOAD_FILE('/etc/passwd')", "ddl")]
    #[case::my_get_lock("mysql", "SELECT GET_LOCK('x', 10)", "ddl")]
    #[case::my_benchmark("mysql", "SELECT BENCHMARK(100000000, MD5('a'))", "ddl")]
    #[case::my_call("mysql", "CALL p()", "ddl")]
    #[case::my_handler("mysql", "HANDLER t OPEN", "ddl")]
    #[case::my_grant("mysql", "GRANT ALL ON *.* TO 'a'@'%'", "ddl")]
    #[case::my_rename("mysql", "RENAME TABLE a TO b", "ddl")]
    #[case::sl_select("sqlite", "SELECT * FROM sqlite_master", "read")]
    #[case::sl_with("sqlite", "WITH x AS (SELECT 1) SELECT * FROM x", "read")]
    #[case::sl_explain_query_plan("sqlite", "EXPLAIN QUERY PLAN SELECT * FROM t", "read")]
    #[case::sl_pragma_table_info("sqlite", "PRAGMA table_info(users)", "read")]
    #[case::sl_pragma_schema_table_info("sqlite", "PRAGMA main.table_info('users')", "read")]
    #[case::sl_pragma_index_list("sqlite", "PRAGMA index_list(users)", "read")]
    #[case::sl_pragma_user_version_read("sqlite", "PRAGMA user_version", "read")]
    #[case::sl_pragma_assign("sqlite", "PRAGMA user_version = 5", "ddl")]
    #[case::sl_pragma_query_only_off("sqlite", "PRAGMA query_only = OFF", "ddl")]
    #[case::sl_pragma_journal_mode_call("sqlite", "PRAGMA journal_mode(WAL)", "ddl")]
    #[case::sl_pragma_wal_checkpoint("sqlite", "PRAGMA wal_checkpoint(TRUNCATE)", "ddl")]
    #[case::sl_pragma_writable_schema("sqlite", "PRAGMA writable_schema", "ddl")]
    #[case::sl_pragma_unknown_bare("sqlite", "PRAGMA optimize", "ddl")]
    #[case::sl_attach("sqlite", "ATTACH DATABASE '/tmp/x.db' AS x", "ddl")]
    #[case::sl_detach("sqlite", "DETACH x", "ddl")]
    #[case::sl_vacuum_into("sqlite", "VACUUM INTO '/tmp/x.db'", "ddl")]
    #[case::sl_load_extension("sqlite", "SELECT load_extension('/tmp/x.so')", "ddl")]
    #[case::sl_writefile("sqlite", "SELECT writefile('/tmp/x', 'abc')", "ddl")]
    #[case::sl_insert("sqlite", "INSERT INTO t VALUES (1)", "write")]
    #[case::sl_insert_or_replace("sqlite", "INSERT OR REPLACE INTO t VALUES (1)", "write")]
    #[case::sl_replace("sqlite", "REPLACE INTO t VALUES (1)", "write")]
    #[case::sl_update_returning("sqlite", "UPDATE t SET a = 1 RETURNING *", "write")]
    #[case::sl_bracket_ident("sqlite", "SELECT [a;b] FROM t", "read")]
    #[case::sl_backslash_not_escape("sqlite", "SELECT 'a\\'; DROP TABLE t; --'", "multi")]
    #[case::sl_drop("sqlite", "DROP TABLE t", "ddl")]
    #[case::sl_create_trigger(
        "sqlite",
        "CREATE TRIGGER x AFTER INSERT ON t BEGIN DELETE FROM t; END",
        "ddl"
    )]
    #[case::sl_begin("sqlite", "BEGIN", "ddl")]
    #[case::sl_show_not_sqlite("sqlite", "SHOW TABLES", "ddl")]
    #[case::ch_select("clickhouse", "SELECT * FROM system.tables", "read")]
    #[case::ch_show("clickhouse", "SHOW TABLES", "read")]
    #[case::ch_describe("clickhouse", "DESCRIBE TABLE t", "read")]
    #[case::ch_exists("clickhouse", "EXISTS TABLE t", "read")]
    #[case::ch_explain_ast("clickhouse", "EXPLAIN AST SELECT 1", "read")]
    #[case::ch_explain_plan_actions("clickhouse", "EXPLAIN PLAN actions = 1 SELECT 1", "read")]
    #[case::ch_insert("clickhouse", "INSERT INTO t VALUES (1)", "write")]
    #[case::ch_alter_delete("clickhouse", "ALTER TABLE t DELETE WHERE 1", "ddl")]
    #[case::ch_optimize("clickhouse", "OPTIMIZE TABLE t", "ddl")]
    #[case::ch_system("clickhouse", "SYSTEM RELOAD CONFIG", "ddl")]
    #[case::ch_kill("clickhouse", "KILL QUERY WHERE 1", "ddl")]
    #[case::ch_set("clickhouse", "SET readonly = 0", "ddl")]
    #[case::ch_settings_clause("clickhouse", "SELECT 1 SETTINGS readonly = 0", "ddl")]
    #[case::ch_format_clause("clickhouse", "SELECT 1 FORMAT JSON", "ddl")]
    #[case::ch_fn_file("clickhouse", "SELECT * FROM file('/etc/passwd', 'CSV')", "ddl")]
    #[case::ch_fn_url("clickhouse", "SELECT * FROM url('http://evil/x', 'CSV')", "ddl")]
    #[case::ch_fn_remote("clickhouse", "SELECT * FROM remote('evil:9000', system.one)", "ddl")]
    #[case::ch_fn_s3("clickhouse", "SELECT * FROM s3('https://b/k', 'CSV')", "ddl")]
    #[case::ch_insert_into_function(
        "clickhouse",
        "INSERT INTO FUNCTION file('x') VALUES (1)",
        "ddl"
    )]
    #[case::ch_into_outfile("clickhouse", "SELECT 1 INTO OUTFILE 'x'", "ddl")]
    #[case::ch_hash_comment("clickhouse", "SELECT 1 # ; DROP TABLE t", "read")]
    #[case::ch_file_as_column("clickhouse", "SELECT file FROM uploads", "read")]
    fn broker_classifier_corpus(
        #[case] engine_name: &str,
        #[case] sql: &str,
        #[case] expected: &str,
    ) {
        let verdict = classify_for_broker(sql, &engine(engine_name));
        let actual = match &verdict {
            Ok(statement) => match statement.class {
                StatementClass::Read => "read",
                StatementClass::Write => "write",
            },
            Err(refusal) => match refusal.kind {
                RefusalKind::Empty => "empty",
                RefusalKind::MultiStatement => "multi",
                RefusalKind::Ddl => "ddl",
                RefusalKind::ReadOnlyViolation => "readonly",
                RefusalKind::Malformed => "malformed",
            },
        };
        assert_eq!(actual, expected, "{engine_name}: {sql:?} -> {verdict:?}");
    }

    #[test]
    fn broker_statement_text_drops_surrounding_comments_and_semicolon() {
        let statement = classify_for_broker(
            "/* lead */ SELECT 1 /* mid */ + 1 ; -- tail",
            &DatabaseType::PostgreSQL,
        )
        .unwrap();
        assert_eq!(statement.sql, "SELECT 1 /* mid */ + 1");
    }

    #[test]
    fn broker_refusal_never_echoes_the_statement() {
        let refusal = classify_for_broker(
            "SELECT 'topsecret-canary' INTO x",
            &DatabaseType::PostgreSQL,
        )
        .unwrap_err();
        assert!(!refusal.reason.contains("canary"), "{}", refusal.reason);
    }

    #[test]
    fn broker_rejects_engines_it_does_not_serve() {
        assert!(classify_for_broker("SELECT 1", &DatabaseType::MongoDB).is_err());
        assert!(classify_for_broker("SELECT 1", &DatabaseType::CSV).is_err());
    }

    #[rstest]
    #[case::append("SELECT * FROM t", 11, "SELECT * FROM t\nLIMIT 11")]
    #[case::keeps_smaller("SELECT * FROM t LIMIT 5", 11, "SELECT * FROM t LIMIT 5")]
    #[case::lowers_larger("SELECT * FROM t LIMIT 100000", 11, "SELECT * FROM t LIMIT 11")]
    #[case::lowers_with_offset(
        "SELECT * FROM t ORDER BY a LIMIT 500 OFFSET 20",
        11,
        "SELECT * FROM t ORDER BY a LIMIT 11 OFFSET 20"
    )]
    #[case::mysql_offset_comma("SELECT * FROM t LIMIT 20, 500", 11, "SELECT * FROM t LIMIT 20, 11")]
    #[case::ignores_subquery_limit(
        "SELECT * FROM (SELECT * FROM t LIMIT 3) x",
        11,
        "SELECT * FROM (SELECT * FROM t LIMIT 3) x\nLIMIT 11"
    )]
    #[case::trailing_comment_cannot_swallow_limit(
        "SELECT * FROM t -- note",
        11,
        "SELECT * FROM t\nLIMIT 11"
    )]
    #[case::fetch_first_is_wrapped(
        "SELECT * FROM t FETCH FIRST 5 ROWS ONLY",
        11,
        "SELECT * FROM (\nSELECT * FROM t FETCH FIRST 5 ROWS ONLY\n) AS dbcrust_limited\nLIMIT 11"
    )]
    #[case::limit_all_is_wrapped(
        "SELECT * FROM t LIMIT ALL",
        11,
        "SELECT * FROM (\nSELECT * FROM t LIMIT ALL\n) AS dbcrust_limited\nLIMIT 11"
    )]
    #[case::show_unchanged("SHOW TABLES", 11, "SHOW TABLES")]
    #[case::semicolon_removed("SELECT * FROM t;", 11, "SELECT * FROM t\nLIMIT 11")]
    fn broker_row_limit_rewrites(#[case] sql: &str, #[case] rows: u32, #[case] expected: &str) {
        let statement = classify_for_broker(sql, &DatabaseType::PostgreSQL).unwrap();
        assert_eq!(statement.with_row_limit(rows), expected);
    }

    // ---- T-SQL (SQL Server) ------------------------------------------------

    #[rstest]
    // plain reads
    #[case::select("SELECT * FROM dbo.Orders", true)]
    #[case::select_semicolon("SELECT TOP (10) * FROM [dbo].[Orders];", true)]
    #[case::leading_semicolon(";WITH c AS (SELECT 1 AS n) SELECT n FROM c", true)]
    #[case::cte("WITH c AS (SELECT 1 AS n) SELECT n FROM c", true)]
    #[case::recursive_cte(
        "WITH r AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM r WHERE n < 5) SELECT * FROM r OPTION (MAXRECURSION 10)",
        true
    )]
    #[case::xmlnamespaces("WITH XMLNAMESPACES ('u' AS x) SELECT 1 AS a FOR XML PATH", true)]
    #[case::parenthesised("(SELECT 1) UNION (SELECT 2)", true)]
    #[case::offset_fetch(
        "SELECT * FROM t ORDER BY id OFFSET 5 ROWS FETCH NEXT 10 ROWS ONLY",
        true
    )]
    #[case::case_end("SELECT CASE WHEN a = 1 THEN 'x' ELSE 'y' END FROM t", true)]
    #[case::for_json("SELECT a, b FROM t FOR JSON PATH, ROOT('rows')", true)]
    #[case::table_hint("SELECT * FROM t WITH (NOLOCK) WHERE id = 1", true)]
    #[case::merge_join_hint("SELECT * FROM a INNER MERGE JOIN b ON a.id = b.id", true)]
    #[case::merge_query_hint("SELECT * FROM a JOIN b ON a.id = b.id OPTION (MERGE JOIN)", true)]
    #[case::three_part_name("SELECT * FROM Sales.dbo.Orders", true)]
    #[case::empty_schema("SELECT * FROM Sales..Orders", true)]
    #[case::many_short_chains("SELECT * FROM a.b JOIN c.d ON 1 = 1 JOIN e.f ON 1 = 1", true)]
    #[case::string_with_keywords("SELECT 'DROP TABLE t; EXEC xp_cmdshell' AS s", true)]
    #[case::unicode_string("SELECT N'it''s; DELETE FROM t' AS s", true)]
    #[case::comment_with_keywords("SELECT 1 /* DROP TABLE t */ -- EXEC sp_who\n", true)]
    #[case::nested_comment("SELECT 1 /* a /* DROP TABLE t */ still a comment */", true)]
    #[case::bracketed_keyword_identifiers(
        "SELECT [update], [delete], [Drop Table] FROM [exec]",
        true
    )]
    #[case::quoted_identifier("SELECT \"insert\" FROM \"t\"", true)]
    #[case::bracket_with_escaped_bracket("SELECT [a]]b] FROM t", true)]
    #[case::variables("SELECT @@VERSION, @@SPID", true)]
    #[case::functions("SELECT GETDATE(), NEWID(), DB_NAME(), SUSER_SNAME()", true)]
    #[case::sys_views("SELECT name FROM sys.objects WHERE type = 'U'", true)]
    #[case::explain_shorthand("EXPLAIN SELECT * FROM t", true)]
    // writes and DDL
    #[case::insert("INSERT INTO t VALUES (1)", false)]
    #[case::update("UPDATE t SET a = 1", false)]
    #[case::delete("DELETE FROM t", false)]
    #[case::merge("MERGE t USING s ON t.id = s.id WHEN MATCHED THEN DELETE;", false)]
    #[case::truncate("TRUNCATE TABLE t", false)]
    #[case::drop_table("DROP TABLE t", false)]
    #[case::create("CREATE TABLE t (a int)", false)]
    #[case::alter("ALTER TABLE t ADD b int", false)]
    #[case::grant("GRANT SELECT ON t TO u", false)]
    #[case::deny("DENY SELECT ON t TO u", false)]
    #[case::writable_cte("WITH d AS (SELECT 1 AS a) DELETE FROM t", false)]
    #[case::cte_without_select("WITH d AS (SELECT 1 AS a) INSERT INTO t SELECT a FROM d", false)]
    #[case::select_into("SELECT * INTO copy FROM t", false)]
    #[case::select_into_temp("SELECT * INTO #copy FROM t", false)]
    #[case::second_statement_after_semicolon("SELECT 1; DROP TABLE t", false)]
    #[case::second_statement_without_semicolon("SELECT 1 DROP TABLE t", false)]
    #[case::two_selects("SELECT 1; SELECT 2", false)]
    #[case::declare("DECLARE @x int = 1 SELECT @x", false)]
    #[case::set_option("SET NOCOUNT OFF", false)]
    #[case::use_db("USE master", false)]
    #[case::begin_tran("BEGIN TRAN", false)]
    #[case::if_statement("IF 1 = 1 SELECT 1", false)]
    #[case::while_loop("SELECT 1 WHILE 1 = 1 SELECT 2", false)]
    // procedures
    #[case::exec("EXEC sp_who", false)]
    #[case::execute("EXECUTE ('SELECT 1')", false)]
    #[case::exec_after_select("SELECT 1 EXEC xp_cmdshell 'dir'", false)]
    #[case::exec_lowercase("select 1 exec sp_configure", false)]
    #[case::sp_reference("SELECT * FROM OPENQUERY(srv, 'x')", false)]
    #[case::xp_function("SELECT master.dbo.xp_cmdshell('dir')", false)]
    #[case::sp_prefix_function("SELECT sys.sp_executesql", false)]
    #[case::bracketed_xp("SELECT * FROM master.dbo.[xp_dirtree]", false)]
    #[case::quoted_openrowset("SELECT * FROM \"OPENROWSET\"('x')", false)]
    #[case::openrowset("SELECT * FROM OPENROWSET('SQLNCLI', 'Server=x;', 'SELECT 1')", false)]
    #[case::openrowset_bulk("SELECT * FROM OPENROWSET(BULK 'C:\\f.txt', SINGLE_CLOB) AS f", false)]
    #[case::opendatasource("SELECT * FROM OPENDATASOURCE('x', 'y').db.dbo.t", false)]
    #[case::openxml("SELECT * FROM OPENXML(@h, '/r')", false)]
    #[case::bulk_insert("BULK INSERT t FROM 'C:\\f.csv'", false)]
    #[case::audit_file_reader("SELECT * FROM sys.fn_get_audit_file('x', NULL, NULL)", false)]
    #[case::trace_reader("SELECT * FROM fn_trace_gettable('x', 1)", false)]
    #[case::xe_file_reader(
        "SELECT * FROM sys.fn_xe_file_target_read_file('x', NULL, NULL, NULL)",
        false
    )]
    #[case::next_value_for("SELECT NEXT VALUE FOR dbo.seq", false)]
    #[case::update_lock("SELECT * FROM t WITH (UPDLOCK)", false)]
    #[case::exclusive_lock("SELECT * FROM t WITH (XLOCK, ROWLOCK)", false)]
    // admin / session / files
    #[case::waitfor("WAITFOR DELAY '00:00:10'", false)]
    #[case::waitfor_after_select("SELECT 1 WAITFOR DELAY '00:00:10'", false)]
    #[case::shutdown("SHUTDOWN", false)]
    #[case::dbcc("DBCC CHECKDB", false)]
    #[case::backup("BACKUP DATABASE d TO DISK = 'x'", false)]
    #[case::restore("RESTORE DATABASE d FROM DISK = 'x'", false)]
    #[case::kill("KILL 52", false)]
    #[case::reconfigure("RECONFIGURE", false)]
    #[case::checkpoint("CHECKPOINT", false)]
    #[case::enable_trigger("ENABLE TRIGGER ALL ON t", false)]
    #[case::receive("RECEIVE TOP (1) * FROM q", false)]
    #[case::end_conversation("SELECT 1 END CONVERSATION @h", false)]
    #[case::execute_as("EXECUTE AS USER = 'x'", false)]
    #[case::linked_server("SELECT * FROM srv.db.dbo.t", false)]
    #[case::linked_server_empty_parts("SELECT * FROM srv...t", false)]
    #[case::linked_server_bracketed("SELECT * FROM [srv].[db].[dbo].[t]", false)]
    #[case::go_separator("SELECT 1\nGO\nSELECT 2", false)]
    #[case::go_with_count("SELECT 1\n  go 5\n", false)]
    #[case::go_after_select_on_same_line_is_an_alias("SELECT 1 go", true)]
    // lexical problems never pass
    #[case::empty("", false)]
    #[case::only_semicolons(" ; ; ", false)]
    #[case::only_comment("-- nothing", false)]
    #[case::unterminated_string("SELECT 'abc", false)]
    #[case::unterminated_bracket("SELECT [abc", false)]
    #[case::unterminated_comment("SELECT 1 /* never closed", false)]
    #[case::unbalanced_paren("SELECT (1", false)]
    #[case::nul_byte("SELECT 1\0 DROP TABLE t", false)]
    #[case::not_a_query("hello world", false)]
    fn test_check_read_only_sql_server(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::SqlServer).is_ok(),
            allowed,
            "{query:?}: {:?}",
            check_read_only(query, &DatabaseType::SqlServer)
        );
    }

    #[test]
    fn test_sql_server_refusals_name_the_problem_without_the_query() {
        let err = check_read_only(
            "SELECT 1 EXEC xp_cmdshell 'whoami /all'",
            &DatabaseType::SqlServer,
        )
        .unwrap_err();
        assert!(err.to_string().starts_with(READ_ONLY_VIOLATION_MARKER));
        assert!(err.reason.contains("EXEC"), "{err}");
        assert!(!err.to_string().contains("whoami"), "{err}");
    }

    #[rstest]
    #[case::select("SELECT * FROM sales.orders LIMIT 10", true)]
    #[case::with_cte("WITH t AS (SELECT 1) SELECT * FROM t", true)]
    #[case::show("SHOW search_path", true)]
    #[case::explain("EXPLAIN SELECT * FROM sales.orders", true)]
    #[case::system_view("SELECT * FROM svv_table_info", true)]
    #[case::trailing_semicolon("SELECT 1;", true)]
    #[case::unload("UNLOAD ('SELECT * FROM t') TO 's3://b/p' IAM_ROLE 'arn'", false)]
    #[case::copy("COPY t FROM 's3://b/p' IAM_ROLE 'arn'", false)]
    #[case::call("CALL refresh_all()", false)]
    #[case::vacuum("VACUUM FULL t", false)]
    #[case::analyze("ANALYZE t", false)]
    #[case::create_external("CREATE EXTERNAL SCHEMA s FROM DATA CATALOG DATABASE 'd'", false)]
    #[case::grant("GRANT SELECT ON t TO u", false)]
    #[case::alter_datashare("ALTER DATASHARE ds ADD SCHEMA s", false)]
    #[case::cancel("CANCEL 1234", false)]
    #[case::abort("ABORT", false)]
    #[case::set_auth("SET SESSION AUTHORIZATION 'admin'", false)]
    #[case::prepare("PREPARE p AS SELECT 1", false)]
    #[case::execute("EXECUTE p", false)]
    #[case::begin("BEGIN", false)]
    #[case::delete("DELETE FROM t", false)]
    #[case::select_into("SELECT * INTO new_t FROM t", false)]
    #[case::cte_insert("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x", false)]
    #[case::stacked("SELECT 1; DROP TABLE t", false)]
    #[case::terminate("SELECT pg_terminate_backend(123)", false)]
    #[case::cancel_backend("SELECT pg_cancel_backend(123)", false)]
    #[case::explain_unload("EXPLAIN UNLOAD ('SELECT 1') TO 's3://b/p'", false)]
    #[case::explain_call("explain call p()", false)]
    #[case::pragma("PRAGMA table_info(t)", false)]
    fn test_check_read_only_redshift(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::Redshift).is_ok(),
            allowed,
            "{query:?}: {:?}",
            check_read_only(query, &DatabaseType::Redshift)
        );
    }

    #[rstest]
    #[case::select("SELECT * FROM users LIMIT 5", true)]
    #[case::with_cte("WITH t AS (SELECT 1) SELECT * FROM t", true)]
    #[case::explain("EXPLAIN QUERY PLAN SELECT * FROM users", true)]
    #[case::catalog("SELECT name FROM sqlite_schema WHERE type = 'table'", true)]
    #[case::table_valued_pragma("SELECT * FROM pragma_table_info('users')", true)]
    #[case::pragma_table_info("PRAGMA table_info(users)", true)]
    #[case::pragma_schema_qualified("PRAGMA main.index_list('users')", true)]
    #[case::pragma_lowercase("pragma foreign_key_list(users)", true)]
    #[case::pragma_read("PRAGMA user_version", true)]
    #[case::pragma_assignment("PRAGMA user_version = 7", false)]
    #[case::pragma_checkpoint("PRAGMA wal_checkpoint(TRUNCATE)", false)]
    #[case::pragma_vacuum("PRAGMA incremental_vacuum(10)", false)]
    #[case::pragma_optimize("PRAGMA optimize", false)]
    #[case::pragma_batch("PRAGMA table_info(t); DROP TABLE t", false)]
    #[case::insert("INSERT INTO t VALUES (1)", false)]
    #[case::delete("DELETE FROM t", false)]
    #[case::drop("DROP TABLE t", false)]
    #[case::replace_into("REPLACE INTO t VALUES (1)", false)]
    #[case::attach("ATTACH DATABASE 'x.db' AS x", false)]
    #[case::vacuum_into("VACUUM INTO 'copy.db'", false)]
    #[case::explain_vacuum("EXPLAIN VACUUM INTO 'copy.db'", false)]
    #[case::load_extension("SELECT load_extension('/tmp/evil.so')", false)]
    #[case::stacked("SELECT 1; DROP TABLE t", false)]
    #[case::cte_insert("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x", false)]
    #[case::select_into("SELECT * INTO new_t FROM t", false)]
    fn test_check_read_only_libsql(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::LibSql).is_ok(),
            allowed,
            "{query:?}: {:?}",
            check_read_only(query, &DatabaseType::LibSql)
        );
    }

    #[rstest]
    #[case::select("SELECT * FROM users LIMIT 5", true)]
    #[case::from_first("FROM users SELECT name", true)]
    #[case::from_only("FROM users", true)]
    #[case::with_cte("WITH t AS (SELECT 1) SELECT * FROM t", true)]
    #[case::values("VALUES (1)", true)]
    #[case::describe("DESCRIBE users", true)]
    #[case::show_tables("SHOW TABLES", true)]
    #[case::summarize("SUMMARIZE users", true)]
    #[case::pivot("PIVOT users ON year USING sum(n)", true)]
    #[case::explain("EXPLAIN SELECT * FROM users", true)]
    #[case::explain_options("EXPLAIN (FORMAT JSON) SELECT * FROM users", true)]
    #[case::explain_analyze("EXPLAIN ANALYZE SELECT * FROM users", true)]
    #[case::pragma_table_info("PRAGMA table_info('users')", true)]
    #[case::pragma_database_size("PRAGMA database_size", true)]
    #[case::pragma_show_tables("PRAGMA show_tables", true)]
    #[case::use_catalog("USE other", true)]
    #[case::two_reads("SELECT 1; SELECT 2", true)]
    #[case::trailing_semicolon("SELECT 1;", true)]
    #[case::information_schema("SELECT * FROM information_schema.tables", true)]
    #[case::duckdb_tables("SELECT * FROM duckdb_tables()", true)]
    #[case::json_extract_is_pure("SELECT json_extract(j, '$.a') FROM t", true)]
    #[case::column_named_like_a_verb("SELECT \"update\", \"copy\" FROM t", true)]
    #[case::quoted_table_name("SELECT * FROM \"my table\"", true)]
    #[case::dollar_quoted_text("SELECT $$; DROP TABLE t$$", true)]
    #[case::insert("INSERT INTO t VALUES (1)", false)]
    #[case::update("UPDATE t SET a = 1", false)]
    #[case::delete("DELETE FROM t", false)]
    #[case::create_table("CREATE TABLE t(i INT)", false)]
    #[case::create_as("CREATE TABLE t AS SELECT 1", false)]
    #[case::drop("DROP TABLE t", false)]
    #[case::alter("ALTER TABLE t ADD COLUMN x INT", false)]
    #[case::truncate("TRUNCATE t", false)]
    #[case::copy_to("COPY t TO '/tmp/x.csv'", false)]
    #[case::copy_query_to("COPY (SELECT 1) TO '/tmp/x.parquet' (FORMAT parquet)", false)]
    #[case::copy_from("COPY t FROM '/tmp/x.csv'", false)]
    #[case::export("EXPORT DATABASE '/tmp/x'", false)]
    #[case::import("IMPORT DATABASE '/tmp/x'", false)]
    #[case::attach("ATTACH '/tmp/x.duckdb' AS x", false)]
    #[case::attach_ducklake("ATTACH 'ducklake:meta.db' AS lake", false)]
    #[case::detach("DETACH x", false)]
    #[case::install("INSTALL httpfs", false)]
    #[case::force_install("FORCE INSTALL httpfs", false)]
    #[case::load("LOAD httpfs", false)]
    #[case::set("SET threads = 2", false)]
    #[case::set_external("SET enable_external_access = true", false)]
    #[case::reset("RESET threads", false)]
    #[case::pragma_assignment("PRAGMA threads = 1", false)]
    #[case::pragma_unknown("PRAGMA enable_profiling", false)]
    #[case::call("CALL duckdb_secrets()", false)]
    #[case::checkpoint("CHECKPOINT", false)]
    #[case::vacuum("VACUUM", false)]
    #[case::create_secret("CREATE SECRET s (TYPE s3, KEY_ID 'a')", false)]
    #[case::begin("BEGIN", false)]
    #[case::explain_delete("EXPLAIN ANALYZE DELETE FROM t", false)]
    #[case::explain_copy("EXPLAIN COPY t TO '/tmp/x.csv'", false)]
    #[case::cte_insert("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x", false)]
    #[case::cte_in_parens_insert(
        "WITH x AS (INSERT INTO t VALUES (1) RETURNING *) SELECT * FROM x",
        false
    )]
    #[case::stacked("SELECT 1; DROP TABLE t", false)]
    #[case::stacked_copy("SELECT 1; COPY t TO '/tmp/x.csv'", false)]
    #[case::read_csv("SELECT * FROM read_csv('/etc/passwd')", false)]
    #[case::read_csv_auto("SELECT * FROM read_csv_auto('x.csv')", false)]
    #[case::read_parquet("SELECT * FROM read_parquet('x.parquet')", false)]
    #[case::read_json("SELECT * FROM read_json('x.json')", false)]
    #[case::read_blob("SELECT * FROM read_blob('x.bin')", false)]
    #[case::glob("SELECT * FROM glob('*')", false)]
    #[case::parquet_scan("SELECT * FROM parquet_scan('x.parquet')", false)]
    #[case::getenv("SELECT getenv('AWS_SECRET_ACCESS_KEY')", false)]
    #[case::secrets("SELECT * FROM duckdb_secrets()", false)]
    #[case::query_function("SELECT * FROM query('SELECT 1')", false)]
    #[case::sqlite_scan("SELECT * FROM sqlite_scan('x.db', 't')", false)]
    #[case::mixed_case_function("SELECT * FROM Read_Csv('x.csv')", false)]
    #[case::quoted_function("SELECT * FROM \"read_csv\"('x.csv')", false)]
    #[case::function_in_cte("WITH x AS (SELECT * FROM read_csv('x.csv')) SELECT * FROM x", false)]
    #[case::string_as_table("SELECT * FROM 'data.csv'", false)]
    #[case::string_as_table_parquet("SELECT * FROM '/data/x.parquet'", false)]
    #[case::string_as_join("SELECT * FROM t JOIN 'x.csv' USING (id)", false)]
    #[case::quoted_path_as_table("SELECT * FROM \"/data/x.parquet\"", false)]
    #[case::url_as_table("SELECT * FROM 'https://example.com/x.parquet'", false)]
    #[case::empty("", false)]
    #[case::only_semicolon(";", false)]
    #[case::unterminated("SELECT 'abc", false)]
    #[case::unbalanced("SELECT (1", false)]
    fn test_check_read_only_duckdb(#[case] query: &str, #[case] allowed: bool) {
        assert_eq!(
            check_read_only(query, &DatabaseType::DuckDB).is_ok(),
            allowed,
            "{query:?}: {:?}",
            check_read_only(query, &DatabaseType::DuckDB)
        );
    }

    #[test]
    fn test_duckdb_refusals_name_the_problem_without_the_query() {
        let reason = check_read_only(
            "SELECT * FROM read_csv('/etc/topsecret-canary')",
            &DatabaseType::DuckDB,
        )
        .unwrap_err()
        .to_string();
        assert!(reason.contains("read_csv"), "{reason}");
        assert!(!reason.contains("canary"), "{reason}");
        let reason = check_read_only("COPY t TO '/tmp/x'", &DatabaseType::DuckDB)
            .unwrap_err()
            .to_string();
        assert!(reason.contains("COPY"), "{reason}");
    }

    #[test]
    fn test_duckdb_row_limits_use_from_first_and_wrap_correctly() {
        let limited = classify_for_broker("FROM people", &DatabaseType::DuckDB)
            .unwrap()
            .with_row_limit(11);
        assert_eq!(limited, "FROM people\nLIMIT 11");
        let limited = classify_for_broker("SELECT * FROM people LIMIT 500", &DatabaseType::DuckDB)
            .unwrap()
            .with_row_limit(11);
        assert_eq!(limited, "SELECT * FROM people LIMIT 11");
        let describe = classify_for_broker("DESCRIBE people", &DatabaseType::DuckDB).unwrap();
        assert!(!describe.is_select);
        assert_eq!(describe.with_row_limit(11), "DESCRIBE people");
    }

    #[test]
    fn test_libsql_is_not_accepted_by_the_broker() {
        // Enrollment is refused before classification; the classifier agrees
        assert!(classify_for_broker("SELECT 1", &DatabaseType::LibSql).is_err());
    }

    #[test]
    fn test_redshift_is_not_accepted_by_the_broker() {
        // Enrollment is refused before classification; the classifier agrees
        assert!(classify_for_broker("SELECT 1", &DatabaseType::Redshift).is_err());
    }

    #[test]
    fn test_sql_server_is_not_accepted_by_the_broker_yet() {
        // Enrollment is refused before classification; the classifier agrees
        assert!(classify_for_broker("SELECT 1", &DatabaseType::SqlServer).is_err());
    }
}
