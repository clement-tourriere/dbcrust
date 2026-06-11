//! Stream AI responses to terminal via mpsc channel

use crate::ai::{AiError, AiStreamEvent};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// Stream AI response to terminal, collecting the full text.
/// Uses dim ANSI coloring for AI output.
/// Respects interrupt flag (Ctrl-C cancels mid-stream).
pub async fn stream_to_terminal(
    mut rx: mpsc::Receiver<AiStreamEvent>,
    interrupt_flag: &Arc<AtomicBool>,
) -> Result<String, AiError> {
    let mut full_response = String::new();
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();

    // Start dim coloring
    let _ = write!(handle, "\x1b[2m");
    let _ = handle.flush();

    loop {
        // Check for interrupt. Returning Err (never the partial text!) is
        // essential: a truncated UPDATE/DELETE offered for execution would
        // be worse than no SQL at all.
        if interrupt_flag.load(Ordering::Relaxed) {
            let _ = writeln!(handle, "\x1b[0m");
            let _ = handle.flush();
            return Err(AiError::Cancelled);
        }

        // The periodic tick re-checks the flag even when the provider has
        // stalled and no events arrive
        let event = tokio::select! {
            event = rx.recv() => event,
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => continue,
        };

        match event {
            Some(AiStreamEvent::TextDelta(text)) => {
                full_response.push_str(&text);
                let _ = write!(handle, "{text}");
                let _ = handle.flush();
            }
            Some(AiStreamEvent::Done) => {
                break;
            }
            Some(AiStreamEvent::Error(msg)) => {
                let _ = write!(handle, "\x1b[0m");
                let _ = handle.flush();
                return Err(AiError::RequestFailed(msg));
            }
            None => {
                // Channel closed
                break;
            }
        }
    }

    // Reset coloring
    let _ = writeln!(handle, "\x1b[0m");
    let _ = handle.flush();

    Ok(full_response)
}

/// Extract SQL from AI response, stripping markdown fences if present
pub fn extract_sql(response: &str) -> String {
    let trimmed = response.trim();

    // Strip markdown code fences. Any fence language tag counts (```sql,
    // ```postgresql, ```mysql, …) — keeping the fence line guaranteed a
    // syntax error on execution.
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            let start = 1;
            let end = if lines.last().is_some_and(|l| l.trim() == "```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            return lines[start..end].join("\n").trim().to_string();
        }
    }

    trimmed.to_string()
}

/// Check if SQL is likely a read-only query, used to decide whether
/// AI-generated SQL may run without confirmation.
///
/// A prefix check alone is NOT enough: `WITH d AS (DELETE FROM t RETURNING *)
/// SELECT * FROM d` starts with WITH, and `SELECT 1; DROP TABLE t` starts
/// with SELECT. Conservative by design — false negatives only cost an extra
/// confirmation prompt.
pub fn is_select_query(sql: &str) -> bool {
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
    if upper.split(';').skip(1).any(|rest| !rest.trim().is_empty()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sql_plain() {
        let sql = "SELECT * FROM users LIMIT 100;";
        assert_eq!(extract_sql(sql), sql);
    }

    #[test]
    fn test_extract_sql_with_fences() {
        let response = "```sql\nSELECT * FROM users LIMIT 100;\n```";
        assert_eq!(extract_sql(response), "SELECT * FROM users LIMIT 100;");
    }

    #[test]
    fn test_extract_sql_with_plain_fences() {
        let response = "```\nSELECT * FROM users;\n```";
        assert_eq!(extract_sql(response), "SELECT * FROM users;");
    }

    #[test]
    fn test_is_select_query() {
        assert!(is_select_query("SELECT * FROM users"));
        assert!(is_select_query("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(is_select_query("EXPLAIN SELECT * FROM users"));
        assert!(is_select_query("SELECT * FROM users;"));
        assert!(!is_select_query("INSERT INTO users VALUES (1)"));
        assert!(!is_select_query("DELETE FROM users WHERE id = 1"));
        assert!(!is_select_query("UPDATE users SET name = 'test'"));
        assert!(!is_select_query("DROP TABLE users"));
    }

    #[test]
    fn test_is_select_query_rejects_disguised_writes() {
        // Data-modifying CTE starts with WITH but writes
        assert!(!is_select_query(
            "WITH d AS (DELETE FROM users RETURNING *) SELECT * FROM d"
        ));
        // Multi-statement smuggling
        assert!(!is_select_query("SELECT 1; DROP TABLE users"));
        // EXPLAIN ANALYZE executes the statement
        assert!(!is_select_query("EXPLAIN ANALYZE DELETE FROM users"));
        // Identifiers merely containing a keyword are fine
        assert!(is_select_query("SELECT updated_at FROM user_inserts"));
    }

    #[test]
    fn test_extract_sql_strips_any_fence_tag() {
        assert_eq!(extract_sql("```postgresql\nSELECT 1;\n```"), "SELECT 1;");
        assert_eq!(extract_sql("```mysql\nSELECT 1;\n```"), "SELECT 1;");
    }
}
