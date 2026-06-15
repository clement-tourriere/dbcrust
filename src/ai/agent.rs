//! Agentic investigation loop for the `???` command.
//!
//! A tool-using agent: the model calls read-only tools (`list_tables`,
//! `describe_table`, `run_sql`, `explain`), observes the results, and iterates
//! until it produces a structured analysis. The loop core ([`run_agent`]) is
//! backend-agnostic and reused by both the interactive REPL (`handle_ai_agentic`
//! in `cli_core`) and the Python entry point (`run_ai_investigation` in `lib`).
//!
//! Safety: every query is gated by [`crate::ai::streaming::is_select_query`] plus
//! a [`side_effect_guard`], so the agent runs only read-only statements — no
//! INSERT/UPDATE/DELETE/DDL, `SELECT … INTO` / file writes, mutating `PRAGMA`,
//! sequence bumps, or share/advisory locks. This is best-effort SQL inspection,
//! NOT a hard sandbox (a SELECT can still call a user-defined side-effecting
//! function); for hard enforcement run it under a read-only database role or
//! against a replica.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use genai::chat::{ChatMessage, ChatRequest, Tool, ToolCall, ToolResponse};
use serde_json::json;

use crate::ai::config::AiConfig;
use crate::ai::{AgentStep, AiError, MessageRole, run_agent_step};
use crate::db::Database;

/// Result of one tool invocation. `content` is fed back to the model; `summary`
/// is a short human-readable line for the progress display.
pub struct ToolOutcome {
    pub content: String,
    pub summary: String,
}

/// Executes the agent's tool calls. Kept as a trait so the loop core stays
/// independent of how the database is owned/locked. `?Send` because the DB-backed
/// implementation holds a `std::sync::MutexGuard` across the query await (the loop
/// is awaited directly, never spawned across threads).
#[async_trait(?Send)]
pub trait AgentToolExecutor {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome;
}

/// Sink for the agent's progress narration (dim tool lines and interim text).
pub trait ProgressSink {
    fn note(&self, line: &str);
}

/// Discards progress — the default for programmatic callers (e.g. the Python
/// `ask_ai` API) so tool traces don't leak into stdout / server logs.
pub struct NoOpProgress;

impl ProgressSink for NoOpProgress {
    fn note(&self, _line: &str) {}
}

/// Prints dim progress to stdout — used by the REPL.
pub struct StdoutProgress;

impl ProgressSink for StdoutProgress {
    fn note(&self, line: &str) {
        println!("\x1b[2m{line}\x1b[0m");
    }
}

/// Appends each progress line (plain, no ANSI) to a file. Lets a Python caller
/// (e.g. the Django dashboard) tail the agent's progress while it runs in a
/// background thread — the GIL is released, so the file is the side channel.
pub struct FileProgress {
    path: std::path::PathBuf,
}

impl FileProgress {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl ProgressSink for FileProgress {
    fn note(&self, line: &str) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// The read-only tools exposed to the model. Schemas are plain JSON Schema; no
/// provider-specific configuration.
pub fn agent_tools() -> Vec<Tool> {
    vec![
        Tool::new("list_tables")
            .with_description("List tables and views in the database.")
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "schema": {
                        "type": "string",
                        "description": "Optional schema name; omit for the default search path."
                    }
                }
            })),
        Tool::new("describe_table")
            .with_description(
                "Show columns, indexes, primary/foreign keys and referencing tables for one table.",
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "table": {
                        "type": "string",
                        "description": "Table or view name; may be schema-qualified as schema.table."
                    },
                    "schema": {
                        "type": "string",
                        "description": "Optional schema name; omit for the default / search-path schema."
                    }
                },
                "required": ["table"]
            })),
        Tool::new("run_sql")
            .with_description(
                "Run ONE read-only SQL statement (SELECT/WITH/SHOW/EXPLAIN) and return the rows. \
                 Writes (INSERT/UPDATE/DELETE/DDL) are rejected.",
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "A single read-only SQL statement." }
                },
                "required": ["query"]
            })),
        Tool::new("explain")
            .with_description(
                "Return the query plan for a read-only statement. Set analyze=true to actually run \
                 it (read-only only) for real timings; otherwise it only plans.",
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "A single read-only SQL statement to plan." },
                    "analyze": { "type": "boolean", "description": "If true, use EXPLAIN ANALYZE (executes the query)." }
                },
                "required": ["query"]
            })),
    ]
}

/// Run the agentic investigation loop. Returns the final analysis text.
///
/// Tool rounds are non-streaming (the structured response is needed to read tool
/// calls). When the iteration budget is exhausted, a final pass is issued with the
/// tools removed so the model is forced to answer in prose — the caller always
/// gets a conclusion, never a silent dead-end.
pub async fn run_agent(
    ai_config: &AiConfig,
    system_prompt: &str,
    messages: &[(MessageRole, String)],
    max_iters: usize,
    executor: &dyn AgentToolExecutor,
    progress: &dyn ProgressSink,
    interrupt: &Arc<AtomicBool>,
) -> Result<String, AiError> {
    let max_iters = max_iters.max(1);
    // The caller threads any prior-investigation history here (the current
    // question is the last user message); the REPL keeps a dedicated agentic
    // history separate from the `??` text-to-SQL history.
    let initial: Vec<ChatMessage> = messages
        .iter()
        .map(|(role, content)| match role {
            MessageRole::System => ChatMessage::system(content.clone()),
            MessageRole::User => ChatMessage::user(content.clone()),
            MessageRole::Assistant => ChatMessage::assistant(content.clone()),
        })
        .collect();
    let mut req = ChatRequest::new(initial)
        .with_system(system_prompt)
        .with_tools(agent_tools());

    for _ in 0..max_iters {
        if interrupt.load(Ordering::Relaxed) {
            return Err(AiError::Cancelled);
        }

        let step = race_step(ai_config, &req, interrupt).await?;

        if step.tool_calls.is_empty() {
            // No tool calls → this is the final answer.
            let answer = step.text.unwrap_or_default();
            if answer.trim().is_empty() {
                return Err(AiError::RequestFailed(
                    "AI returned no analysis".to_string(),
                ));
            }
            return Ok(answer);
        }

        // The model may narrate before/with its tool calls — surface it dimly.
        if let Some(text) = step.text.as_ref().filter(|t| !t.trim().is_empty()) {
            progress.note(text);
        }

        // Re-append the ORIGINAL tool calls (preserves Gemini thought signatures).
        req = req.append_message(step.tool_calls.clone());

        for call in &step.tool_calls {
            if interrupt.load(Ordering::Relaxed) {
                return Err(AiError::Cancelled);
            }
            progress.note(&format!("🔧 {}", tool_call_label(call)));
            let outcome = executor.execute(call).await;
            progress.note(&format!("   {}", outcome.summary));
            req = req.append_message(ToolResponse::new(call.call_id.clone(), outcome.content));
        }
    }

    // Budget exhausted — force a prose answer by re-issuing without tools.
    progress.note("⏳ reached the investigation limit — summarizing findings…");
    let mut forced = req.clone();
    forced.tools = None;
    let step = race_step(ai_config, &forced, interrupt).await?;
    Ok(step
        .text
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            "The investigation reached its iteration limit without a definitive conclusion."
                .to_string()
        }))
}

/// One agent round-trip, raced against Ctrl-C. Dropping the genai future on
/// cancellation aborts the underlying request (mirrors `handle_ai_text_to_sql`).
async fn race_step(
    ai_config: &AiConfig,
    req: &ChatRequest,
    interrupt: &Arc<AtomicBool>,
) -> Result<AgentStep, AiError> {
    let fut = run_agent_step(ai_config, req);
    tokio::pin!(fut);
    loop {
        tokio::select! {
            res = &mut fut => return res,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if interrupt.load(Ordering::Relaxed) {
                    return Err(AiError::Cancelled);
                }
            }
        }
    }
}

/// Collapse whitespace and cap to one short line for the progress display.
fn one_line(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(100).collect();
    if truncated.len() < collapsed.len() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

fn tool_call_label(call: &ToolCall) -> String {
    let arg = |k: &str| {
        call.fn_arguments
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
    };
    match call.fn_name.as_str() {
        "list_tables" => {
            let schema = arg("schema");
            if schema.is_empty() {
                "list_tables".to_string()
            } else {
                format!("list_tables: {schema}")
            }
        }
        "describe_table" => format!("describe_table: {}", arg("table")),
        "run_sql" => format!("run_sql: {}", one_line(arg("query"))),
        "explain" => format!("explain: {}", one_line(arg("query"))),
        other => other.to_string(),
    }
}

// ==================== Serialization helpers ====================

/// Render a table/view name list compactly, capped.
pub fn serialize_table_list(names: &[String]) -> String {
    const CAP: usize = 200;
    let mut out = format!("{} tables/views:\n", names.len());
    for name in names.iter().take(CAP) {
        out.push_str(name);
        out.push('\n');
    }
    if names.len() > CAP {
        out.push_str(&format!("… ({} more)\n", names.len() - CAP));
    }
    out
}

/// Render query result rows (row 0 = header) as compact pipe-delimited text,
/// capped to `max_rows` data rows, wide cells truncated, total payload bounded.
pub fn serialize_rows(rows: &[Vec<String>], max_rows: usize) -> String {
    if rows.is_empty() {
        return "(no rows)".to_string();
    }
    const MAX_CELL: usize = 120;
    const MAX_TOTAL: usize = 8000;
    let max_rows = max_rows.max(1);

    let truncate_cell = |s: &str| -> String {
        let t: String = s.chars().take(MAX_CELL).collect();
        if t.len() < s.len() {
            format!("{t}…")
        } else {
            t
        }
    };

    let total_data_rows = rows.len().saturating_sub(1);
    let header = &rows[0];
    let mut out = format!("rows={} cols={}\n", total_data_rows, header.len());
    out.push_str(
        &header
            .iter()
            .map(|c| truncate_cell(c))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    out.push('\n');

    for row in rows.iter().skip(1).take(max_rows) {
        out.push_str(
            &row.iter()
                .map(|c| truncate_cell(c))
                .collect::<Vec<_>>()
                .join(" | "),
        );
        out.push('\n');
        if out.len() > MAX_TOTAL {
            out.push_str("… (truncated)\n");
            return out;
        }
    }

    let shown = total_data_rows.min(max_rows);
    if total_data_rows > shown {
        // The fetch is itself capped (see `cap_query`), so the true total is
        // unknown — report the limit rather than a misleading exact count.
        out.push_str(&format!("… (output limited to {max_rows} rows)\n"));
    }
    out
}

/// Bound how many rows a data query can materialize. `agentic_max_rows_per_tool`
/// only caps what's SENT to the model; without this a `SELECT` over a huge table
/// (or one the model gave a giant LIMIT) would still load every row into memory
/// first. SELECT/WITH are wrapped in a capped subquery (one extra row so
/// truncation is detectable); SHOW/EXPLAIN results are already bounded and aren't
/// subquery-able, so they pass through unchanged.
fn cap_query(sql: &str, max_rows: usize) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let upper = trimmed.to_uppercase();
    if upper.starts_with("SELECT") || upper.starts_with("WITH") {
        // Newline before `)` so a trailing line comment in the inner query can't
        // comment out the wrapper's closing paren.
        format!(
            "SELECT * FROM (\n{trimmed}\n) AS _dbcrust_agent LIMIT {}",
            max_rows.saturating_add(1)
        )
    } else {
        trimmed.to_string()
    }
}

fn err_outcome(msg: &str) -> ToolOutcome {
    ToolOutcome {
        content: format!("ERROR: {msg}"),
        summary: format!("⚠ {msg}"),
    }
}

fn rejected_outcome(detail: &str) -> ToolOutcome {
    ToolOutcome {
        content: format!(
            "REJECTED: only read-only queries are allowed in agentic mode. {detail} Rephrase it \
             as a plain SELECT / EXPLAIN / SHOW."
        ),
        summary: "⛔ rejected (read-only only)".to_string(),
    }
}

/// Best-effort secondary guard: flags read-only statements that can still mutate
/// or cause side effects. Returns a short reason when the statement should be
/// rejected.
///
/// `is_select_query` already blocks DML/DDL and `SELECT … FOR UPDATE` (UPDATE is a
/// write keyword); this closes the remaining SELECT-side holes it misses:
/// `SELECT … INTO new_table` (PostgreSQL table creation), `INTO OUTFILE/DUMPFILE`
/// (MySQL file write), mutating `PRAGMA` (SQLite), sequence bumps, locks, and
/// known side-effecting functions. It is NOT a complete guarantee — a SELECT can
/// still call a user-defined side-effecting function — so for hard enforcement run
/// under a read-only database role or a replica.
fn side_effect_guard(sql: &str) -> Option<&'static str> {
    let upper = sql.to_uppercase();

    // SQLite PRAGMA can mutate (`PRAGMA user_version = 1`, `journal_mode = WAL`, …).
    // The agent doesn't need it — describe_table covers schema introspection.
    if upper.trim_start().starts_with("PRAGMA") {
        return Some("uses PRAGMA (use describe_table for schema instead)");
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

// ==================== Database-backed tool executor ====================

/// Tool executor backed by a shared [`Database`]. Locks the mutex only for the
/// duration of each DB call; the agent's LLM round-trips happen outside, with no
/// lock held. Reused by the REPL and the Python entry point.
pub struct DbToolExecutor {
    db: Arc<std::sync::Mutex<Database>>,
    interrupt: Arc<AtomicBool>,
    max_rows: usize,
}

impl DbToolExecutor {
    pub fn new(
        db: Arc<std::sync::Mutex<Database>>,
        interrupt: Arc<AtomicBool>,
        max_rows: usize,
    ) -> Self {
        Self {
            db,
            interrupt,
            max_rows: max_rows.max(1),
        }
    }

    /// Run a read-only query, neutralizing the user's `\x` explain-mode so the
    /// agent gets ACTUAL ROWS rather than a silently-rewritten EXPLAIN plan
    /// (`Database::execute_query_with_interrupt_and_info` rewrites to EXPLAIN when
    /// `explain_mode` is on). The query is wrapped in a hard row cap first (see
    /// [`cap_query`]) so a huge result can't be materialized into memory.
    #[allow(clippy::await_holding_lock)]
    async fn run_readonly(&self, sql: &str) -> ToolOutcome {
        let capped_sql = cap_query(sql, self.max_rows);
        let result = {
            let mut db = self.db.lock().unwrap();
            let was_explain = db.is_explain_mode();
            if was_explain {
                db.toggle_explain_mode();
            }
            // No-column-selection variant: the agent must never raise an
            // interactive `inquire` prompt on a wide result (hangs a background
            // thread; wrong mid-investigation in the REPL).
            let r = db
                .execute_query_with_interrupt_no_column_selection(&capped_sql, &self.interrupt)
                .await;
            if was_explain {
                db.toggle_explain_mode();
            }
            r
        };
        match result {
            Ok(rows) => {
                let data_rows = rows.len().saturating_sub(1);
                let cols = rows.first().map(|r| r.len()).unwrap_or(0);
                // cap_query fetches one extra row past the cap so truncation is
                // detectable; show a `+` when the result was capped.
                let capped = data_rows > self.max_rows;
                let shown = data_rows.min(self.max_rows);
                let plus = if capped { "+" } else { "" };
                ToolOutcome {
                    content: serialize_rows(&rows, self.max_rows),
                    summary: format!("📊 {shown}{plus} rows × {cols} cols"),
                }
            }
            Err(e) => err_outcome(&format!("query failed: {e}")),
        }
    }
}

#[async_trait(?Send)]
impl AgentToolExecutor for DbToolExecutor {
    #[allow(clippy::await_holding_lock)]
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let arg_str = |k: &str| {
            call.fn_arguments
                .get(k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        match call.fn_name.as_str() {
            "list_tables" => {
                let schema = arg_str("schema");
                let result = {
                    let mut db = self.db.lock().unwrap();
                    db.get_tables_and_views(schema.as_deref()).await
                };
                match result {
                    Ok(names) => ToolOutcome {
                        summary: format!("📋 {} tables", names.len()),
                        content: serialize_table_list(&names),
                    },
                    Err(e) => err_outcome(&format!("list_tables failed: {e}")),
                }
            }
            "describe_table" => {
                let Some(raw_table) = arg_str("table") else {
                    return err_outcome("describe_table requires a 'table' argument");
                };
                // Resolve the schema: an explicit `schema` arg wins; otherwise
                // accept a dotted `schema.table` form so non-public tables
                // (e.g. analytics.orders) describe correctly instead of silently
                // falling back to public.
                let (schema, table) = match arg_str("schema") {
                    Some(s) => (Some(s), raw_table),
                    None => match raw_table.split_once('.') {
                        Some((s, t)) => (Some(s.to_string()), t.to_string()),
                        None => (None, raw_table),
                    },
                };
                let label = match &schema {
                    Some(s) => format!("{s}.{table}"),
                    None => table.clone(),
                };
                let (db_type, details) = {
                    let mut db = self.db.lock().unwrap();
                    let dt = db.get_database_type();
                    (
                        dt,
                        db.get_table_details_in_schema(&table, schema.as_deref())
                            .await,
                    )
                };
                match details {
                    Ok(d) => ToolOutcome {
                        summary: format!("🗂  {label}: {} cols", d.columns.len()),
                        content: crate::ai::schema_context::format_table_ddl(&d, &db_type),
                    },
                    Err(e) => err_outcome(&format!("describe_table({label}) failed: {e}")),
                }
            }
            "run_sql" => {
                let Some(raw) = arg_str("query") else {
                    return err_outcome("run_sql requires a 'query' argument");
                };
                let sql = crate::ai::streaming::extract_sql(&raw);
                if !crate::ai::streaming::is_select_query(&sql) {
                    return rejected_outcome("It appears to modify data or is multi-statement.");
                }
                if let Some(reason) = side_effect_guard(&sql) {
                    return rejected_outcome(&format!("It {reason}, which has side effects."));
                }
                self.run_readonly(&sql).await
            }
            "explain" => {
                let Some(raw) = arg_str("query") else {
                    return err_outcome("explain requires a 'query' argument");
                };
                let inner = crate::ai::streaming::extract_sql(&raw);
                if !crate::ai::streaming::is_select_query(&inner) {
                    return rejected_outcome("It appears to modify data or is multi-statement.");
                }
                if let Some(reason) = side_effect_guard(&inner) {
                    return rejected_outcome(&format!("It {reason}, which has side effects."));
                }
                let analyze = call
                    .fn_arguments
                    .get("analyze")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let prefix = if analyze {
                    "EXPLAIN ANALYZE "
                } else {
                    "EXPLAIN "
                };
                self.run_readonly(&format!("{prefix}{inner}")).await
            }
            other => err_outcome(&format!("unknown tool: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_rows_caps_rows() {
        let mut rows = vec![vec!["id".to_string(), "name".to_string()]];
        for i in 0..100 {
            rows.push(vec![i.to_string(), format!("name{i}")]);
        }
        let out = serialize_rows(&rows, 10);
        assert!(out.contains("rows=100 cols=2"));
        assert!(out.contains("output limited to 10 rows"));
        // header + count line + 10 data rows + trailing limit line
        assert!(out.lines().count() <= 14);
    }

    #[test]
    fn test_serialize_rows_empty() {
        assert_eq!(serialize_rows(&[], 10), "(no rows)");
    }

    #[test]
    fn test_serialize_table_list_caps() {
        let names: Vec<String> = (0..250).map(|i| format!("t{i}")).collect();
        let out = serialize_table_list(&names);
        assert!(out.starts_with("250 tables/views:"));
        assert!(out.contains("… (50 more)"));
    }

    #[test]
    fn test_agent_tools_shape() {
        let tools = agent_tools();
        assert_eq!(tools.len(), 4);
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
        // SELECT INTO (PostgreSQL table creation) and INTO OUTFILE (MySQL file write).
        assert!(side_effect_guard("SELECT * INTO new_t FROM old_t").is_some());
        assert!(side_effect_guard("SELECT a INTO OUTFILE '/tmp/x' FROM t").is_some());
        // Mutating PRAGMA (SQLite), including leading whitespace.
        assert!(side_effect_guard("PRAGMA user_version = 1").is_some());
        assert!(side_effect_guard("  pragma journal_mode = WAL").is_some());
        // Plain reads are not flagged — including identifiers that contain "into".
        assert!(side_effect_guard("SELECT count(*) FROM orders").is_none());
        assert!(side_effect_guard("SELECT id, created_at FROM users LIMIT 10").is_none());
        assert!(side_effect_guard("SELECT into_count FROM stats").is_none());
    }

    #[test]
    fn test_cap_query_wraps_data_queries_only() {
        let wrapped = cap_query("SELECT * FROM big", 50);
        assert!(wrapped.starts_with("SELECT * FROM ("));
        assert!(wrapped.ends_with("LIMIT 51"));
        // WITH is wrapped too (one extra row past the cap).
        assert!(cap_query("WITH x AS (SELECT 1) SELECT * FROM x", 10).contains("LIMIT 11"));
        // SHOW / EXPLAIN are not subquery-able — passed through unchanged.
        assert_eq!(cap_query("SHOW TABLES", 50), "SHOW TABLES");
        assert!(cap_query("EXPLAIN SELECT 1", 50).starts_with("EXPLAIN"));
        // A trailing semicolon is stripped before wrapping (no `;` inside the subquery).
        assert!(!cap_query("SELECT 1;", 5).contains(";\n)"));
    }
}
