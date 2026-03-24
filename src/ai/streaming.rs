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
        // Check for interrupt
        if interrupt_flag.load(Ordering::Relaxed) {
            let _ = writeln!(handle, "\x1b[0m");
            let _ = handle.flush();
            return Ok(full_response);
        }

        match rx.recv().await {
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

    // Strip markdown code fences
    if trimmed.starts_with("```") {
        let lines: Vec<&str> = trimmed.lines().collect();
        if lines.len() >= 2 {
            let start = if lines[0].starts_with("```sql")
                || lines[0].starts_with("```SQL")
                || lines[0] == "```"
            {
                1
            } else {
                0
            };
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

/// Check if SQL is likely a read-only SELECT query
pub fn is_select_query(sql: &str) -> bool {
    let upper = sql.trim().to_uppercase();
    upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("SHOW")
        || upper.starts_with("DESCRIBE")
        || upper.starts_with("PRAGMA")
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
        assert!(is_select_query("WITH cte AS (...) SELECT ..."));
        assert!(is_select_query("EXPLAIN SELECT * FROM users"));
        assert!(!is_select_query("INSERT INTO users VALUES (1)"));
        assert!(!is_select_query("DELETE FROM users WHERE id = 1"));
        assert!(!is_select_query("UPDATE users SET name = 'test'"));
        assert!(!is_select_query("DROP TABLE users"));
    }
}
