//! SQL input buffer analysis.
//!
//! Two consumers share the same lexer:
//! - [`SqlValidator`] tells reedline whether Enter should submit the buffer
//!   or insert a newline (unterminated strings / dollar-quotes / block
//!   comments keep the buffer open);
//! - [`split_statements`] splits a buffer into individual statements on
//!   top-level semicolons so multi-statement input (pasted scripts, `\i`
//!   files) executes statement-by-statement instead of failing in the
//!   driver's prepared-statement path.
//!
//! The lexer understands single-quoted strings (with `''` escapes), quoted
//! identifiers (with `""` escapes), `--` line comments, nested `/* */` block
//! comments, and PostgreSQL dollar-quoted blocks (`$tag$ … $tag$`).

use reedline::{ValidationResult, Validator};

/// Reedline validator: keep the buffer open while a statement is clearly
/// unterminated. Deliberately conservative — it never demands a trailing
/// `;` for plain statements, so existing "type a query, press Enter" muscle
/// memory keeps working.
pub struct SqlValidator;

impl Validator for SqlValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if is_buffer_complete(line) {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum LexState {
    Normal,
    SingleQuote,
    DoubleQuote,
    LineComment,
    BlockComment(u32),
    DollarQuote(String),
}

/// Scan `input`, invoking `on_statement_sep` with the byte index of every
/// top-level `;`. Returns the lexer state at end of input.
fn scan(input: &str, mut on_statement_sep: impl FnMut(usize)) -> LexState {
    let mut state = LexState::Normal;
    let mut chars = input.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        match &state {
            LexState::Normal => match c {
                '\'' => state = LexState::SingleQuote,
                '"' => state = LexState::DoubleQuote,
                '-' if matches!(chars.peek(), Some((_, '-'))) => {
                    chars.next();
                    state = LexState::LineComment;
                }
                '/' if matches!(chars.peek(), Some((_, '*'))) => {
                    chars.next();
                    state = LexState::BlockComment(1);
                }
                '$' => {
                    // Dollar quote opener: $tag$ where tag is empty or
                    // [A-Za-z_][A-Za-z0-9_]*. Anything else ($1 params,
                    // bare $) is left alone.
                    let look = chars.clone();
                    let mut tag = String::new();
                    let mut opened = false;
                    for (_, lc) in look {
                        if lc == '$' {
                            opened = true;
                            break;
                        }
                        let valid = if tag.is_empty() {
                            lc.is_ascii_alphabetic() || lc == '_'
                        } else {
                            lc.is_ascii_alphanumeric() || lc == '_'
                        };
                        if !valid {
                            break;
                        }
                        tag.push(lc);
                    }
                    if opened {
                        // Consume the tag and the closing '$' of the opener
                        for _ in 0..=tag.chars().count() {
                            chars.next();
                        }
                        state = LexState::DollarQuote(tag);
                    }
                }
                ';' => on_statement_sep(i),
                _ => {}
            },
            LexState::SingleQuote => {
                if c == '\'' {
                    if matches!(chars.peek(), Some((_, '\''))) {
                        chars.next(); // '' escape — stay in the string
                    } else {
                        state = LexState::Normal;
                    }
                }
            }
            LexState::DoubleQuote => {
                if c == '"' {
                    if matches!(chars.peek(), Some((_, '"'))) {
                        chars.next(); // "" escape — stay in the identifier
                    } else {
                        state = LexState::Normal;
                    }
                }
            }
            LexState::LineComment => {
                if c == '\n' {
                    state = LexState::Normal;
                }
            }
            LexState::BlockComment(depth) => {
                let depth = *depth;
                if c == '*' && matches!(chars.peek(), Some((_, '/'))) {
                    chars.next();
                    state = if depth <= 1 {
                        LexState::Normal
                    } else {
                        LexState::BlockComment(depth - 1)
                    };
                } else if c == '/' && matches!(chars.peek(), Some((_, '*'))) {
                    chars.next();
                    state = LexState::BlockComment(depth + 1);
                }
            }
            LexState::DollarQuote(tag) => {
                if c == '$' {
                    let closer_matches = {
                        let mut look = chars.clone();
                        let tag_matches = tag
                            .chars()
                            .all(|tc| matches!(look.next(), Some((_, lc)) if lc == tc));
                        tag_matches && matches!(look.next(), Some((_, '$')))
                    };
                    if closer_matches {
                        for _ in 0..=tag.chars().count() {
                            chars.next();
                        }
                        state = LexState::Normal;
                    }
                }
            }
        }
    }

    state
}

/// Whether the buffer is safe to submit: backslash commands, AI input, and
/// empty lines always are; SQL is complete unless a string, identifier,
/// dollar-quote, or block comment is left open.
pub fn is_buffer_complete(buffer: &str) -> bool {
    let trimmed = buffer.trim();
    if trimmed.is_empty() || trimmed.starts_with('\\') || trimmed.starts_with("??") {
        return true;
    }
    matches!(
        scan(buffer, |_| {}),
        // A line comment is terminated by end-of-input just as well as by \n
        LexState::Normal | LexState::LineComment
    )
}

/// True if `segment` contains anything besides whitespace and comments.
fn has_sql_content(segment: &str) -> bool {
    let mut content = false;
    let mut state = LexState::Normal;
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        match &state {
            LexState::Normal => match c {
                '-' if matches!(chars.peek(), Some('-')) => {
                    chars.next();
                    state = LexState::LineComment;
                }
                '/' if matches!(chars.peek(), Some('*')) => {
                    chars.next();
                    state = LexState::BlockComment(1);
                }
                c if c.is_whitespace() => {}
                _ => {
                    content = true;
                    break;
                }
            },
            LexState::LineComment => {
                if c == '\n' {
                    state = LexState::Normal;
                }
            }
            LexState::BlockComment(depth) => {
                let depth = *depth;
                if c == '*' && matches!(chars.peek(), Some('/')) {
                    chars.next();
                    state = if depth <= 1 {
                        LexState::Normal
                    } else {
                        LexState::BlockComment(depth - 1)
                    };
                } else if c == '/' && matches!(chars.peek(), Some('*')) {
                    chars.next();
                    state = LexState::BlockComment(depth + 1);
                }
            }
            // Strings only appear after real content set `content = true`
            _ => break,
        }
    }
    content
}

/// Split a buffer into statements on top-level semicolons. Trailing
/// semicolons are dropped; whitespace/comment-only segments are skipped.
/// A buffer with no top-level `;` comes back as a single statement.
pub fn split_statements(buffer: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut start = 0usize;

    scan(buffer, |sep_idx| {
        let segment = &buffer[start..sep_idx];
        if has_sql_content(segment) {
            statements.push(segment.trim().to_string());
        }
        start = sep_idx + 1; // ';' is one byte
    });

    let tail = &buffer[start..];
    if has_sql_content(tail) {
        statements.push(tail.trim().to_string());
    }

    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_simple_statements() {
        assert!(is_buffer_complete("SELECT 1"));
        assert!(is_buffer_complete("SELECT 1;"));
        assert!(is_buffer_complete("SELECT 'it''s fine'"));
        assert!(is_buffer_complete("SELECT \"col\"\"name\" FROM t"));
        assert!(is_buffer_complete("SELECT 1 -- trailing comment"));
        assert!(is_buffer_complete("SELECT 1 /* done */"));
        assert!(is_buffer_complete("SELECT $$body$$"));
        assert!(is_buffer_complete("SELECT $1, $2")); // params, not dollar quotes
    }

    #[test]
    fn backslash_and_ai_input_always_complete() {
        assert!(is_buffer_complete("\\dt"));
        assert!(is_buffer_complete("?? show me all users"));
        assert!(is_buffer_complete(""));
        assert!(is_buffer_complete("   "));
    }

    #[test]
    fn incomplete_constructs_keep_buffer_open() {
        assert!(!is_buffer_complete("SELECT 'unterminated"));
        assert!(!is_buffer_complete("SELECT \"unterminated"));
        assert!(!is_buffer_complete("SELECT 1 /* open comment"));
        assert!(!is_buffer_complete(
            "SELECT 1 /* outer /* nested */ still open"
        ));
        assert!(!is_buffer_complete("CREATE FUNCTION f() AS $body$ BEGIN"));
        assert!(!is_buffer_complete("SELECT $tag$text"));
    }

    #[test]
    fn split_basic() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;"),
            vec!["SELECT 1", "SELECT 2"]
        );
        assert_eq!(split_statements("SELECT 1"), vec!["SELECT 1"]);
        assert_eq!(
            split_statements("SELECT 1;\nINSERT INTO t VALUES (2)"),
            vec!["SELECT 1", "INSERT INTO t VALUES (2)"]
        );
    }

    #[test]
    fn split_respects_strings_comments_and_dollar_quotes() {
        assert_eq!(
            split_statements("SELECT 'a;b'; SELECT 2"),
            vec!["SELECT 'a;b'", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT 1 -- comment; not a separator\n; SELECT 2"),
            vec!["SELECT 1 -- comment; not a separator", "SELECT 2"]
        );
        assert_eq!(
            split_statements(
                "CREATE FUNCTION f() RETURNS void AS $$ BEGIN PERFORM 1; END $$ LANGUAGE plpgsql; SELECT 2"
            ),
            vec![
                "CREATE FUNCTION f() RETURNS void AS $$ BEGIN PERFORM 1; END $$ LANGUAGE plpgsql",
                "SELECT 2"
            ]
        );
        assert_eq!(
            split_statements("SELECT \"weird;name\" FROM t; SELECT 2"),
            vec!["SELECT \"weird;name\" FROM t", "SELECT 2"]
        );
    }

    #[test]
    fn split_drops_empty_and_comment_only_segments() {
        assert_eq!(split_statements("SELECT 1;;;"), vec!["SELECT 1"]);
        assert_eq!(
            split_statements("SELECT 1; -- just a comment\n; SELECT 2"),
            vec!["SELECT 1", "SELECT 2"]
        );
        assert!(split_statements("/* only a comment */").is_empty());
        assert!(split_statements("   ").is_empty());
    }

    #[test]
    fn validator_matches_buffer_completeness() {
        let validator = SqlValidator;
        assert!(matches!(
            validator.validate("SELECT 1"),
            ValidationResult::Complete
        ));
        assert!(matches!(
            validator.validate("SELECT 'open"),
            ValidationResult::Incomplete
        ));
    }
}
