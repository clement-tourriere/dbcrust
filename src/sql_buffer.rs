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
//! The lexer understands single-quoted strings (with `''` escapes; and, for
//! PostgreSQL `E'…'` escape strings, backslash escapes), quoted
//! identifiers (with `""` escapes), MySQL/ClickHouse backtick identifiers,
//! `--` line comments, nested `/* */` block comments, and PostgreSQL
//! dollar-quoted blocks (`$tag$ … $tag$`).
//!
//! [`split_statement_spans`] exposes the same split as byte ranges so editors
//! (the desktop GUI) can map a caret position to the statement under it.

use datafusion::sql::sqlparser::ast::{Query, SetExpr, Statement, Visit, Visitor};
use datafusion::sql::sqlparser::dialect::{
    ClickHouseDialect, Dialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};
use datafusion::sql::sqlparser::parser::Parser;
use datafusion::sql::sqlparser::tokenizer::Token;
use reedline::{ValidationResult, Validator};
use std::ops::ControlFlow;

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
    /// A PostgreSQL escape string, `E'…'`: a backslash escapes the next
    /// character (so `\'` does not close it).
    EscapeQuote,
    DoubleQuote,
    Backtick,
    LineComment,
    BlockComment(u32),
    DollarQuote(String),
}

/// Whether the `'` at byte `quote` opens an `E'…'` escape string: preceded
/// by `e`/`E` that starts a token (not the end of an identifier like `somE`).
fn opens_escape_string(input: &str, quote: usize) -> bool {
    let mut before = input[..quote].chars().rev();
    matches!(before.next(), Some('e' | 'E'))
        && !before
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// What the lexer reports besides the final state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanEvent {
    /// A top-level `;`.
    Semicolon,
    /// A character read in plain SQL (outside strings, quoted identifiers,
    /// comments and dollar-quoted bodies). A quote or comment opener counts:
    /// the line it is on is not blank.
    Code(char),
    /// A line ends outside any string or block comment (the `\n` that closes
    /// a `--` comment included).
    Newline,
    /// A MySQL/MariaDB executable comment opens: `/*!…*/`, `/*M!…*/`.
    ExecutableComment,
}

/// Scan `input`, invoking `on_statement_sep` with the byte index of every
/// top-level `;`. Returns the lexer state at end of input.
fn scan(input: &str, mut on_statement_sep: impl FnMut(usize)) -> LexState {
    scan_events(input, |at, event| {
        if event == ScanEvent::Semicolon {
            on_statement_sep(at);
        }
    })
}

/// [`scan`] with every [`ScanEvent`] and the byte index it happened at.
fn scan_events(input: &str, mut on: impl FnMut(usize, ScanEvent)) -> LexState {
    let mut state = LexState::Normal;
    let mut chars = input.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if state == LexState::Normal {
            match c {
                '\n' => on(i, ScanEvent::Newline),
                ';' => on(i, ScanEvent::Semicolon),
                _ => on(i, ScanEvent::Code(c)),
            }
        }
        match &state {
            LexState::Normal => match c {
                '\'' => {
                    state = if opens_escape_string(input, i) {
                        LexState::EscapeQuote
                    } else {
                        LexState::SingleQuote
                    }
                }
                '"' => state = LexState::DoubleQuote,
                '`' => state = LexState::Backtick,
                '-' if matches!(chars.peek(), Some((_, '-'))) => {
                    chars.next();
                    state = LexState::LineComment;
                }
                '/' if matches!(chars.peek(), Some((_, '*'))) => {
                    chars.next();
                    let mut look = chars.clone();
                    if matches!(
                        (look.next(), look.next()),
                        (Some((_, '!')), _) | (Some((_, 'M')), Some((_, '!')))
                    ) {
                        on(i, ScanEvent::ExecutableComment);
                    }
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
            LexState::EscapeQuote => match c {
                '\\' => {
                    chars.next(); // the escaped character, a quote included
                }
                '\'' => {
                    if matches!(chars.peek(), Some((_, '\''))) {
                        chars.next(); // '' escape — stay in the string
                    } else {
                        state = LexState::Normal;
                    }
                }
                _ => {}
            },
            LexState::DoubleQuote => {
                if c == '"' {
                    if matches!(chars.peek(), Some((_, '"'))) {
                        chars.next(); // "" escape — stay in the identifier
                    } else {
                        state = LexState::Normal;
                    }
                }
            }
            LexState::Backtick => {
                if c == '`' {
                    if matches!(chars.peek(), Some((_, '`'))) {
                        chars.next(); // `` escape — stay in the identifier
                    } else {
                        state = LexState::Normal;
                    }
                }
            }
            LexState::LineComment => {
                if c == '\n' {
                    on(i, ScanEvent::Newline);
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
    split_statement_spans(buffer)
        .into_iter()
        .map(|span| buffer[span].to_string())
        .collect()
}

/// Byte ranges of the statements [`split_statements`] would return, in
/// order. Each range is trimmed of surrounding whitespace and excludes the
/// terminating `;`, so `&buffer[range]` equals the matching
/// `split_statements` entry.
pub fn split_statement_spans(buffer: &str) -> Vec<std::ops::Range<usize>> {
    fn trimmed(buffer: &str, start: usize, end: usize) -> std::ops::Range<usize> {
        let segment = &buffer[start..end];
        let leading = segment.len() - segment.trim_start().len();
        let trailing = segment.len() - segment.trim_end().len();
        (start + leading)..(end - trailing)
    }

    let mut spans = Vec::new();
    let mut start = 0usize;

    scan(buffer, |sep_idx| {
        if has_sql_content(&buffer[start..sep_idx]) {
            spans.push(trimmed(buffer, start, sep_idx));
        }
        start = sep_idx + 1; // ';' is one byte
    });

    if has_sql_content(&buffer[start..]) {
        spans.push(trimmed(buffer, start, buffer.len()));
    }

    spans
}

// ---------------------------------------------------------------------------
// Smart splitting (desktop app)
// ---------------------------------------------------------------------------

/// The SQL grammar a buffer is parsed with when deciding whether a blank line
/// separates two statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SqlDialect {
    #[default]
    Generic,
    PostgreSql,
    MySql,
    Sqlite,
    ClickHouse,
}

/// How [`split_statement_spans_with`] finds statement boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SplitMode {
    /// Top-level `;` only (what the CLI does).
    Semicolons,
    /// `;`, plus blank lines between plain `SELECT`-like queries that cannot
    /// be read as one statement (see [`split_statement_spans_with`]).
    Smart(SqlDialect),
}

/// What ends a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// A top-level `;`.
    Explicit,
    /// A blank line, inferred by [`SplitMode::Smart`].
    Inferred,
}

/// One statement of a buffer, as the desktop app sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementSpan {
    /// What to execute: the statement with its leading comments, without
    /// surrounding whitespace, a trailing `;`, or a comment that trails the
    /// previous statement on its line.
    pub range: std::ops::Range<usize>,
    /// How the statement ends; `None` at the end of the text.
    pub after: Option<Boundary>,
    /// Byte index of the `;` that ends it, for [`Boundary::Explicit`].
    pub terminator: Option<usize>,
}

/// Regions longer than this are never parsed to look for blank-line
/// boundaries: they stay whole, like in the CLI.
const SMART_REGION_LIMIT: usize = 256 * 1024;

/// The statements of `buffer` as byte ranges, in order.
///
/// [`SplitMode::Semicolons`] splits on top-level `;` like
/// [`split_statement_spans`], except that a comment trailing the previous
/// statement's `;` on the same line stays out of the next statement.
///
/// [`SplitMode::Smart`] also looks inside each region between two `;` that
/// holds a blank line (a line of whitespace only, outside strings, comments and
/// parentheses):
/// 1. the region parses as exactly one statement: it stays whole;
/// 2. else it is cut at every such blank line, but only if there are at least
///    two non-comment pieces and each one parses, alone, as exactly one plain
///    read query (a query whose every nested query is a plain read: no
///    `INTO`, no `FOR UPDATE/SHARE`, no data-modifying CTE);
/// 3. otherwise it stays whole.
///
/// A region holding a MySQL executable comment (`/*! … */`, `/*M! … */`) is
/// never cut. Comment-only pieces join the query that follows them (a
/// trailing one stays outside every range). PostgreSQL's bare `TABLE t` is not
/// understood by the parser's statement entry point, so a region holding one
/// stays whole.
pub fn split_statement_spans_with(buffer: &str, mode: SplitMode) -> Vec<StatementSpan> {
    // Regions between top-level `;`
    let mut regions: Vec<(usize, usize, Option<usize>)> = Vec::new();
    let mut start = 0usize;
    scan(buffer, |sep| {
        regions.push((start, sep, Some(sep)));
        start = sep + 1;
    });
    regions.push((start, buffer.len(), None));

    let mut spans = Vec::new();
    for (n, (region_start, end, terminator)) in regions.into_iter().enumerate() {
        let start = if n == 0 {
            region_start
        } else {
            skip_trailing_comment(buffer, region_start, end)
        };
        if !has_sql_content(&buffer[start..end]) {
            continue;
        }
        let range = trim_range(buffer, start, end);
        let pieces = match mode {
            SplitMode::Semicolons => None,
            SplitMode::Smart(dialect) => infer_pieces(buffer, &range, dialect),
        }
        .unwrap_or_else(|| vec![range]);
        let last = pieces.len() - 1;
        for (k, piece) in pieces.into_iter().enumerate() {
            let closes_region = k == last;
            spans.push(StatementSpan {
                range: piece,
                after: if !closes_region {
                    Some(Boundary::Inferred)
                } else {
                    terminator.map(|_| Boundary::Explicit)
                },
                terminator: if closes_region { terminator } else { None },
            });
        }
    }
    spans
}

fn trim_range(buffer: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let segment = &buffer[start..end];
    let leading = segment.len() - segment.trim_start().len();
    let trailing = segment.len() - segment.trim_end().len();
    (start + leading)..(end - trailing)
}

/// Where a statement really starts in `buffer[start..end]`, the text after a
/// `;`: past a comment that is the last thing on the `;` line (it trails the
/// previous statement), or `start` when there is none.
fn skip_trailing_comment(buffer: &str, start: usize, end: usize) -> usize {
    let mut pos = start;
    loop {
        let rest = &buffer[pos..end];
        let at = pos + rest.len() - rest.trim_start_matches([' ', '\t', '\r']).len();
        let rest = &buffer[at..end];
        if rest.is_empty() || rest.starts_with('\n') {
            return at;
        }
        if rest.starts_with("--") {
            return rest.find('\n').map_or(end, |eol| at + eol);
        }
        let executable = rest.starts_with("/*!") || rest.starts_with("/*M!");
        // A comment that nests (PostgreSQL nests `/* /* */ */`, and the
        // lexer follows), is not closed, or is executable is not understood
        // well enough to take apart, and stays with the statement
        match simple_block_comment_len(rest) {
            Some(len) if !executable && !rest[..len].contains('\n') => pos = at + len,
            _ => return start,
        }
    }
}

/// Length of the block comment `text` starts with, scanned the way a nesting
/// lexer reads it, when it is closed and nothing nests inside it. `None` for
/// anything else, including an opener that overlaps a closer (`/*/*/` opens
/// twice, whatever the first `*/` suggests).
fn simple_block_comment_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if !text.starts_with("/*") {
        return None;
    }
    let mut i = 2;
    while i + 1 < bytes.len() {
        match (bytes[i], bytes[i + 1]) {
            (b'/', b'*') => return None,
            (b'*', b'/') => return Some(i + 2),
            _ => i += 1,
        }
    }
    None
}

/// What the lexer found in a region.
#[derive(Default)]
struct RegionFacts {
    /// Blank lines outside strings, comments and parentheses (region-relative).
    blank_lines: Vec<std::ops::Range<usize>>,
    /// A MySQL executable comment is present.
    executable_comment: bool,
}

fn region_facts(text: &str) -> RegionFacts {
    let mut facts = RegionFacts::default();
    let mut depth = 0usize;
    let mut line_start = 0usize;
    let mut line_blank = true;
    scan_events(text, |at, event| match event {
        ScanEvent::Code(c) => {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            line_blank &= c.is_whitespace();
        }
        ScanEvent::Newline => {
            if line_blank && depth == 0 {
                facts.blank_lines.push(line_start..at);
            }
            line_start = at + 1;
            line_blank = true;
        }
        ScanEvent::ExecutableComment => facts.executable_comment = true,
        ScanEvent::Semicolon => {}
    });
    facts
}

/// Whether `text` ends inside a `--` comment (anything appended on the same
/// line, a `;` included, would be part of it).
pub fn ends_in_line_comment(text: &str) -> bool {
    scan(text, |_| {}) == LexState::LineComment
}

/// Whether `text` has a blank line outside strings, comments and parentheses
/// between two pieces of SQL.
pub fn has_blank_line_separator(text: &str) -> bool {
    !region_facts(text.trim()).blank_lines.is_empty()
}

fn dialect_of(dialect: SqlDialect) -> Box<dyn Dialect> {
    match dialect {
        SqlDialect::Generic => Box::new(GenericDialect {}),
        SqlDialect::PostgreSql => Box::new(PostgreSqlDialect {}),
        SqlDialect::MySql => Box::new(MySqlDialect {}),
        SqlDialect::Sqlite => Box::new(SQLiteDialect {}),
        SqlDialect::ClickHouse => Box::new(ClickHouseDialect {}),
    }
}

/// `sql` as exactly one statement: one statement parses and the parser then
/// reaches the end of the text (it would otherwise stop at, say, an `END`).
fn parse_single(sql: &str, dialect: SqlDialect) -> Option<Statement> {
    let dialect = dialect_of(dialect);
    let mut parser = Parser::new(dialect.as_ref()).try_with_sql(sql).ok()?;
    let statement = parser.parse_statement().ok()?;
    (parser.peek_token().token == Token::EOF).then_some(statement)
}

/// `sql` parses as exactly one statement in `dialect`.
pub fn parses_as_single_statement(sql: &str, dialect: SqlDialect) -> bool {
    parse_single(sql, dialect).is_some()
}

/// A statement that only reads: a query whose every nested query is a plain
/// `SELECT`/`VALUES`/`TABLE` (or a set operation of those), with no `INTO`
/// and no locking clause, and no data-modifying CTE.
struct ReadOnlyCheck {
    plain: bool,
}

impl ReadOnlyCheck {
    fn set_expr(expr: &SetExpr) -> bool {
        match expr {
            SetExpr::Select(select) => select.into.is_none(),
            // A nested query is visited on its own
            SetExpr::Query(_) | SetExpr::Values(_) | SetExpr::Table(_) => true,
            SetExpr::SetOperation { left, right, .. } => {
                Self::set_expr(left) && Self::set_expr(right)
            }
            SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
                false
            }
        }
    }
}

impl Visitor for ReadOnlyCheck {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        if !query.locks.is_empty() || !Self::set_expr(&query.body) {
            self.plain = false;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        if !matches!(statement, Statement::Query(_)) {
            self.plain = false;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

fn is_plain_read_query(statement: &Statement) -> bool {
    let mut check = ReadOnlyCheck { plain: true };
    let _ = statement.visit(&mut check);
    check.plain
}

/// The pieces `range` (a region of `buffer` holding a blank line) is cut
/// into, or `None` when it stays whole. See [`split_statement_spans_with`].
fn infer_pieces(
    buffer: &str,
    range: &std::ops::Range<usize>,
    dialect: SqlDialect,
) -> Option<Vec<std::ops::Range<usize>>> {
    if range.len() > SMART_REGION_LIMIT {
        return None;
    }
    let text = &buffer[range.clone()];
    let facts = region_facts(text);
    if facts.blank_lines.is_empty() || facts.executable_comment {
        return None;
    }
    if parse_single(text, dialect).is_some() {
        return None;
    }

    // Cut at every blank line at once
    let mut cuts = Vec::new();
    let mut pos = 0usize;
    for blank in &facts.blank_lines {
        cuts.push(pos..blank.start);
        pos = blank.end;
    }
    cuts.push(pos..text.len());

    let mut pieces: Vec<std::ops::Range<usize>> = Vec::new();
    // Where the comment-only pieces waiting for their query start
    let mut comments_from: Option<usize> = None;
    for cut in cuts {
        let piece = trim_range(text, cut.start, cut.end);
        if piece.is_empty() {
            continue;
        }
        if !has_sql_content(&text[piece.clone()]) {
            comments_from.get_or_insert(piece.start);
            continue;
        }
        let statement = parse_single(&text[piece.clone()], dialect)?;
        if !is_plain_read_query(&statement) {
            return None;
        }
        let from = comments_from.take().unwrap_or(piece.start);
        pieces.push((range.start + from)..(range.start + piece.end));
    }
    (pieces.len() >= 2).then_some(pieces)
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
    fn split_respects_backtick_identifiers() {
        assert_eq!(
            split_statements("SELECT `a;b` FROM t; SELECT `x``;y`"),
            vec!["SELECT `a;b` FROM t", "SELECT `x``;y`"]
        );
        assert!(!is_buffer_complete("SELECT `unterminated"));
    }

    #[test]
    fn spans_index_into_the_buffer() {
        let buffer = "  SELECT 1;\n\n-- note\nSELECT 'é;';  \n";
        let spans = split_statement_spans(buffer);
        assert_eq!(spans.len(), 2);
        assert_eq!(&buffer[spans[0].clone()], "SELECT 1");
        assert_eq!(&buffer[spans[1].clone()], "-- note\nSELECT 'é;'");
        let texts: Vec<_> = spans.iter().map(|s| &buffer[s.clone()]).collect();
        assert_eq!(texts, split_statements(buffer));
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

    /// The statements `buffer` splits into, by text.
    fn spans(buffer: &str) -> Vec<&str> {
        split_statement_spans(buffer)
            .into_iter()
            .map(|span| &buffer[span])
            .collect()
    }

    #[rstest::rstest]
    // In E'…' a backslash escapes the next character: `\'` does not close it
    #[case(r"SELECT E'a\';-- b'; SELECT 2", &[r"SELECT E'a\';-- b'", "SELECT 2"])]
    #[case(r"SELECT e'a\';-- b'; SELECT 2", &[r"SELECT e'a\';-- b'", "SELECT 2"])]
    // …an escaped backslash does not escape the quote after it
    #[case(r"SELECT E'a\\'; SELECT 2", &[r"SELECT E'a\\'", "SELECT 2"])]
    // `''` still doubles inside it
    #[case("SELECT E'it''s;'; SELECT 2", &["SELECT E'it''s;'", "SELECT 2"])]
    #[case(r"SELECT (E'x\n;'), 1; SELECT 2", &[r"SELECT (E'x\n;'), 1", "SELECT 2"])]
    // Ordinary strings take no backslash escapes: the quote after `\` closes
    #[case(r"SELECT 'a\'; SELECT 2", &[r"SELECT 'a\'", "SELECT 2"])]
    // The `E` must start a token, not end an identifier
    #[case(r"SELECT somE'a\'; SELECT 2", &[r"SELECT somE'a\'", "SELECT 2"])]
    #[case(r"SELECT a_E'a\'; SELECT 2", &[r"SELECT a_E'a\'", "SELECT 2"])]
    #[case(r"SELECT 1E'a\'; SELECT 2", &[r"SELECT 1E'a\'", "SELECT 2"])]
    fn escape_strings_are_split_like_postgresql_reads_them(
        #[case] buffer: &str,
        #[case] expected: &[&str],
    ) {
        assert_eq!(spans(buffer), expected);
    }

    #[test]
    fn an_escape_string_left_open_keeps_the_buffer_open() {
        assert!(!is_buffer_complete(r"SELECT E'a\'"));
        assert!(is_buffer_complete(r"SELECT E'a\''"));
        assert!(is_buffer_complete(r"SELECT 'a\'"));
    }

    // ----- Smart splitting -----

    /// The statements `buffer` splits into under `Smart(dialect)`.
    fn smart(buffer: &str, dialect: SqlDialect) -> Vec<&str> {
        split_statement_spans_with(buffer, SplitMode::Smart(dialect))
            .into_iter()
            .map(|span| &buffer[span.range])
            .collect()
    }

    #[rstest::rstest]
    #[case("EXPLAIN (FORMAT JSON)\n\nDELETE FROM users;", SqlDialect::PostgreSql)]
    #[case("VACUUM FULL\n\nANALYZE users;", SqlDialect::PostgreSql)]
    #[case(
        "CREATE TRIGGER cleanup AFTER INSERT ON events FOR EACH ROW\n\nDELETE FROM expired_sessions;",
        SqlDialect::MySql
    )]
    #[case(
        "CREATE TABLE archive (id INT)\n\nSELECT id FROM orders;",
        SqlDialect::MySql
    )]
    #[case("DELETE FROM users\n\nWHERE id = 5", SqlDialect::PostgreSql)]
    #[case("UPDATE t SET a = 1\n\nWHERE id = 2", SqlDialect::Generic)]
    #[case("INSERT INTO t (a)\n\nSELECT 1", SqlDialect::PostgreSql)]
    #[case("WITH x AS (SELECT 1)\n\nSELECT * FROM x", SqlDialect::PostgreSql)]
    #[case("SELECT 1\n\nUNION ALL\n\nSELECT 2", SqlDialect::PostgreSql)]
    #[case("SELECT a\n\nFROM t", SqlDialect::Sqlite)]
    #[case(
        "WITH d AS (DELETE FROM t RETURNING *)\n\nSELECT 1\n\nSELECT 2",
        SqlDialect::PostgreSql
    )]
    #[case("SELECT * INTO new_t FROM t\n\nSELECT 1", SqlDialect::PostgreSql)]
    #[case("SELECT * FROM t FOR UPDATE\n\nSELECT 1", SqlDialect::PostgreSql)]
    #[case("SELECT * FROM t FOR SHARE\n\nSELECT 1", SqlDialect::PostgreSql)]
    #[case(
        "SELECT * FROM (SELECT * FROM users FOR UPDATE) AS locked\n\nSELECT 2",
        SqlDialect::PostgreSql
    )]
    #[case(
        "SELECT * FROM t WHERE id IN (SELECT id FROM u FOR UPDATE)\n\nSELECT 2",
        SqlDialect::PostgreSql
    )]
    #[case("SELECT * FROM t INTO OUTFILE '/tmp/x'\n\nSELECT 2", SqlDialect::MySql)]
    #[case("INSERT INTO t FORMAT CSV\n\n1,2\n3,4", SqlDialect::ClickHouse)]
    #[case("SET x = 1\n\nSELECT 1", SqlDialect::PostgreSql)]
    #[case("SELECT 1\n\n/*! UNION ALL */ SELECT 2", SqlDialect::MySql)]
    #[case("SELECT 1 /*! INTO OUTFILE '/tmp/x' */\n\nSELECT 2", SqlDialect::MySql)]
    #[case("SELECT 1\n\n/*M! UNION ALL */ SELECT 2", SqlDialect::MySql)]
    #[case("SELECT 1\n\n/*!50100 UNION ALL */ SELECT 2", SqlDialect::MySql)]
    // The parser stops at END: not one complete statement
    #[case("SELECT 1 LIMIT 1 END\n\nSELECT 2", SqlDialect::Generic)]
    // Blank lines inside parentheses, strings and comments are not boundaries
    #[case("SELECT (1 +\n\n2)", SqlDialect::Generic)]
    #[case("SELECT 'a\n\nb'", SqlDialect::Generic)]
    #[case("SELECT 1 /* a\n\nb */", SqlDialect::Generic)]
    // A single query next to comments only
    #[case("SELECT 1\n\n-- done", SqlDialect::Generic)]
    #[case("-- note\n\nSELECT 1", SqlDialect::Generic)]
    // An incomplete second query keeps the region whole
    #[case("SELECT 1\n\nSELECT * FROM", SqlDialect::PostgreSql)]
    #[case("SELECT 1\n\nSEL", SqlDialect::PostgreSql)]
    // PostgreSQL's bare `TABLE t` is not read by the statement parser
    #[case("TABLE t\n\nSELECT 1", SqlDialect::PostgreSql)]
    // Not a plain query
    #[case("SELECT 1\n\nEXPLAIN SELECT 2", SqlDialect::PostgreSql)]
    #[case("SELECT 1\n\nDELETE FROM t", SqlDialect::PostgreSql)]
    fn blank_lines_do_not_split_what_is_not_plain_queries(
        #[case] buffer: &str,
        #[case] dialect: SqlDialect,
    ) {
        let whole = buffer.trim_end_matches(';');
        assert_eq!(smart(buffer, dialect), vec![whole], "{buffer:?}");
    }

    #[rstest::rstest]
    #[case(
        "select * from orders where status = 'open'\n\nselect count(*) from customers",
        &["select * from orders where status = 'open'", "select count(*) from customers"]
    )]
    #[case(
        "select * from orders where status = 'open'\r\n\r\nselect count(*) from customers",
        &["select * from orders where status = 'open'", "select count(*) from customers"]
    )]
    // Blank lines may hold spaces and tabs
    #[case("SELECT 1\n \t \nSELECT 2", &["SELECT 1", "SELECT 2"])]
    #[case("SELECT 1\r\n  \r\nSELECT 2", &["SELECT 1", "SELECT 2"])]
    #[case("SELECT 1;\n\nSELECT 2\n\nSELECT 3", &["SELECT 1", "SELECT 2", "SELECT 3"])]
    #[case(
        "SELECT 1\n\n-- the second\nSELECT 2",
        &["SELECT 1", "-- the second\nSELECT 2"]
    )]
    #[case(
        "SELECT 1\n\n-- the second\n\nSELECT 2",
        &["SELECT 1", "-- the second\n\nSELECT 2"]
    )]
    #[case("SELECT 'é;ü'\n\nSELECT 'ß'", &["SELECT 'é;ü'", "SELECT 'ß'"])]
    #[case("SELECT 1\n\n\n\nSELECT 2", &["SELECT 1", "SELECT 2"])]
    #[case("SELECT 1 -- one\n\nSELECT 2 -- two", &["SELECT 1 -- one", "SELECT 2 -- two"])]
    #[case("(SELECT 1)\n\nVALUES (2)", &["(SELECT 1)", "VALUES (2)"])]
    #[case(
        "SELECT 1\n\nWITH x AS (SELECT 2) SELECT * FROM x",
        &["SELECT 1", "WITH x AS (SELECT 2) SELECT * FROM x"]
    )]
    #[case("SELECT 1\n\nSELECT 2 UNION SELECT 3", &["SELECT 1", "SELECT 2 UNION SELECT 3"])]
    fn blank_lines_split_plain_queries(#[case] buffer: &str, #[case] expected: &[&str]) {
        assert_eq!(
            smart(buffer, SqlDialect::PostgreSql),
            expected,
            "{buffer:?}"
        );
    }

    #[test]
    fn semicolons_mode_never_splits_on_blank_lines() {
        let buffer = "select 1\n\nselect 2";
        let spans = split_statement_spans_with(buffer, SplitMode::Semicolons);
        assert_eq!(spans.len(), 1);
        assert_eq!(&buffer[spans[0].range.clone()], buffer);
    }

    #[test]
    fn the_cli_splitter_is_unchanged_by_blank_lines() {
        let buffer = "select 1\n\nselect 2; select 3";
        assert_eq!(
            split_statements(buffer),
            vec!["select 1\n\nselect 2", "select 3"]
        );
    }

    #[test]
    fn boundaries_say_how_a_statement_ends() {
        let buffer = "SELECT 1;\n\nSELECT 2\n\nSELECT 3";
        let spans = split_statement_spans_with(buffer, SplitMode::Smart(SqlDialect::Generic));
        let after: Vec<_> = spans.iter().map(|s| s.after).collect();
        assert_eq!(
            after,
            vec![Some(Boundary::Explicit), Some(Boundary::Inferred), None]
        );
        assert_eq!(spans[0].terminator, Some(8));
        assert_eq!(spans[1].terminator, None);
    }

    #[test]
    fn a_comment_after_the_semicolon_stays_with_its_statement() {
        let buffer = "SELECT 1; -- first\n\n-- second\nSELECT 2;";
        for mode in [SplitMode::Semicolons, SplitMode::Smart(SqlDialect::Generic)] {
            let spans = split_statement_spans_with(buffer, mode);
            let texts: Vec<_> = spans.iter().map(|s| &buffer[s.range.clone()]).collect();
            assert_eq!(texts, vec!["SELECT 1", "-- second\nSELECT 2"], "{mode:?}");
        }
        // …but a comment that leads the next statement on the same line is its own
        let buffer = "SELECT 1; /* a */ SELECT 2";
        let spans = split_statement_spans_with(buffer, SplitMode::Semicolons);
        assert_eq!(&buffer[spans[1].range.clone()], "/* a */ SELECT 2");
        let buffer = "SELECT 1; /* a */\nSELECT 2";
        let spans = split_statement_spans_with(buffer, SplitMode::Semicolons);
        assert_eq!(&buffer[spans[1].range.clone()], "SELECT 2");
    }

    #[test]
    fn a_nested_comment_after_a_semicolon_is_never_taken_apart() {
        // The first `*/` closes the inner comment only: `-- */ EXPLAIN` is
        // still inside the outer one, so `DELETE` is not a statement of its own
        // `/*/*/` opens twice: its `*/` overlaps the second opener
        for (buffer, second) in [
            (
                "SELECT 1; /* outer /* inner */ -- */ EXPLAIN\nDELETE FROM users;",
                "/* outer /* inner */ -- */ EXPLAIN\nDELETE FROM users",
            ),
            (
                "SELECT 1; /*/*/ -- */ */ EXPLAIN\nDELETE FROM users;",
                "/*/*/ -- */ */ EXPLAIN\nDELETE FROM users",
            ),
        ] {
            for mode in [
                SplitMode::Semicolons,
                SplitMode::Smart(SqlDialect::PostgreSql),
            ] {
                let spans = split_statement_spans_with(buffer, mode);
                let texts: Vec<_> = spans.iter().map(|s| &buffer[s.range.clone()]).collect();
                assert_eq!(texts, vec!["SELECT 1", second], "{mode:?}");
            }
        }
        // Executable, or nested at a deeper level: also left with the statement
        for buffer in [
            "SELECT 1; /*! DELETE */\nSELECT 2",
            "SELECT 1; /* a /* b */ c */\nSELECT 2",
        ] {
            let spans = split_statement_spans_with(buffer, SplitMode::Semicolons);
            assert!(
                buffer[spans[1].range.clone()].starts_with("/*"),
                "{buffer:?}"
            );
        }
        // A plain one-line block comment still trails its statement
        let buffer = "SELECT 1; /* done */\nSELECT 2";
        let spans = split_statement_spans_with(buffer, SplitMode::Semicolons);
        assert_eq!(&buffer[spans[1].range.clone()], "SELECT 2");
    }

    #[rstest::rstest]
    #[case("/* done */ x", Some(10))]
    #[case("/**/", Some(4))]
    #[case("/* a /* b */ c */", None)]
    #[case("/*/*/ -- */ */", None)]
    #[case("/* never closed", None)]
    #[case("/*/", None)]
    #[case("-- not a block", None)]
    fn only_simple_block_comments_are_measured(#[case] text: &str, #[case] len: Option<usize>) {
        assert_eq!(simple_block_comment_len(text), len, "{text:?}");
    }

    #[test]
    fn dialect_decides_what_parses() {
        // `LIMIT 1, 2` is MySQL only
        let buffer = "SELECT a FROM t LIMIT 1, 2\n\nSELECT 2";
        assert_eq!(smart(buffer, SqlDialect::MySql).len(), 2);
        // A statement the dialect cannot read stays whole, never half-split
        let buffer = "SELECT a FROM t FOR UPDATE\n\nSELECT 2";
        assert_eq!(smart(buffer, SqlDialect::MySql).len(), 1);
    }

    #[rstest::rstest]
    #[case("SELECT 1\n\nSELECT 2", true)]
    #[case("SELECT 1\r\n\r\nSELECT 2", true)]
    #[case("SELECT 1\nSELECT 2", false)]
    #[case("SELECT '1\n\n2'", false)]
    #[case("SELECT (1\n\n)", false)]
    #[case("SELECT 1 /*\n\n*/", false)]
    #[case("SELECT 1 -- x\n\nSELECT 2", true)]
    fn blank_line_separators(#[case] text: &str, #[case] expected: bool) {
        assert_eq!(has_blank_line_separator(text), expected, "{text:?}");
    }

    #[test]
    fn single_statement_parse_needs_the_end_of_the_text() {
        assert!(parses_as_single_statement(
            "SELECT a\n\nFROM t",
            SqlDialect::Generic
        ));
        assert!(!parses_as_single_statement(
            "SELECT 1\n\nSELECT 2",
            SqlDialect::Generic
        ));
        assert!(!parses_as_single_statement(
            "SELECT 1 LIMIT 1 END",
            SqlDialect::Generic
        ));
    }
}
