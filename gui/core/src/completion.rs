//! SQL completion candidates for the query editor.
//!
//! Pure logic, no GPUI: the editor asks [`analyze`] what is being typed at
//! the caret (a table name? a column of an alias? a keyword?), looks up the
//! columns of the tables the statement references ([`Plan::needed_tables`]),
//! then asks [`suggest`] for ranked [`Candidate`]s.
//!
//! All offsets are UTF-8 byte offsets into the editor text.
//!
//! - **Scope**: only the statement under the caret is analysed (split with
//!   [`crate::statements`]), so tables of other statements never leak in.
//! - **Tokenizer**: comments, strings (`'..'`, dollar quoting), quoted
//!   identifiers and parameters are recognised; nothing is suggested inside
//!   a string or a comment.
//! - **Contexts**: after `FROM`/`JOIN`/`UPDATE`/`INTO`/`TABLE` → tables;
//!   after `x.` → columns of the table or alias `x` (or the tables of schema
//!   `x`); in the select list, `WHERE`, `ON`, … → columns of every table
//!   referenced anywhere in the statement, functions and keywords; after a
//!   table reference → clause keywords.
//! - **Ranking**: exact > prefix > word-boundary > subsequence match; ties
//!   are broken by what the context wants first (tables, then columns, then
//!   functions, then keywords) and finally by declaration order.

use crate::gui_state::StatementSeparators;
use crate::schema::{is_default_schema, is_system_object, quote_identifier, table_reference};
use crate::statements::{Boundary, SplitConfig, StatementIndex};
use crate::types::{TableEntry, TableKind};
use dbcrust::database::DatabaseType;
use std::collections::HashMap;
use std::ops::Range;

/// At most this many candidates are returned.
pub const MAX_CANDIDATES: usize = 200;
/// At most this many referenced tables get their columns resolved.
pub const MAX_TABLES_PER_STATEMENT: usize = 8;

// ---------------------------------------------------------------------------
// Dialects
// ---------------------------------------------------------------------------

/// Keyword and function sets differ per engine family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    PostgreSql,
    MySql,
    Sqlite,
    Generic,
}

impl Dialect {
    pub fn of(db_type: Option<&DatabaseType>) -> Self {
        match db_type {
            Some(DatabaseType::PostgreSQL | DatabaseType::DuckDB | DatabaseType::Redshift) => {
                Dialect::PostgreSql
            }
            Some(DatabaseType::MySQL) => Dialect::MySql,
            Some(DatabaseType::SQLite | DatabaseType::LibSql) => Dialect::Sqlite,
            _ => Dialect::Generic,
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Word,
    Quoted,
    String,
    Number,
    Comment,
    Param,
    Punct,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    /// Strings, quoted identifiers and block comments: the closing
    /// delimiter is part of the token.
    pub closed: bool,
}

impl Token {
    fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    fn is_punct(&self, src: &str, c: char) -> bool {
        self.kind == TokenKind::Punct && src[self.start..self.end].starts_with(c)
    }

    fn is_ident(&self) -> bool {
        matches!(self.kind, TokenKind::Word | TokenKind::Quoted)
    }
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Scan `src` into tokens. Whitespace is skipped; unterminated strings,
/// comments and quoted identifiers run to the end of the text.
pub(crate) fn tokenize(src: &str, backslash_escapes: bool) -> Vec<Token> {
    let bytes = src.as_bytes();
    let len = src.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < len {
        let Some(c) = src[i..].chars().next() else {
            break;
        };
        let width = c.len_utf8();
        if c.is_whitespace() {
            i += width;
            continue;
        }
        let start = i;
        let (kind, end, closed) = match c {
            '-' if bytes.get(i + 1) == Some(&b'-') => {
                let end = src[i..].find('\n').map_or(len, |n| i + n);
                (TokenKind::Comment, end, false)
            }
            '/' if bytes.get(i + 1) == Some(&b'*') => match src[i + 2..].find("*/") {
                Some(n) => (TokenKind::Comment, i + 2 + n + 2, true),
                None => (TokenKind::Comment, len, false),
            },
            '\'' => {
                let (end, closed) = scan_quoted(src, i, '\'', backslash_escapes);
                (TokenKind::String, end, closed)
            }
            '"' | '`' => {
                let (end, closed) = scan_quoted(src, i, c, false);
                (TokenKind::Quoted, end, closed)
            }
            '$' => scan_dollar(src, i),
            c if c.is_ascii_digit() => {
                let mut end = i;
                while end < len && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'.')
                    && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
                {
                    end += 1;
                    while end < len && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                        end += 1;
                    }
                }
                (TokenKind::Number, end, true)
            }
            c if is_word_start(c) => {
                let mut end = i + width;
                while let Some(next) = src[end..].chars().next() {
                    if !is_word_char(next) {
                        break;
                    }
                    end += next.len_utf8();
                }
                (TokenKind::Word, end, true)
            }
            _ => (TokenKind::Punct, i + width, true),
        };
        tokens.push(Token {
            kind,
            start,
            end,
            closed,
        });
        i = end.max(start + width);
    }
    tokens
}

/// End of the `quote`-delimited run starting at `start` (the opening quote),
/// with doubled quotes (and optionally backslashes) as escapes.
fn scan_quoted(src: &str, start: usize, quote: char, backslash: bool) -> (usize, bool) {
    let mut chars = src[start + quote.len_utf8()..].char_indices().peekable();
    let base = start + quote.len_utf8();
    while let Some((offset, c)) = chars.next() {
        if backslash && c == '\\' {
            chars.next();
            continue;
        }
        if c == quote {
            if matches!(chars.peek(), Some((_, next)) if *next == quote) {
                chars.next();
                continue;
            }
            return (base + offset + c.len_utf8(), true);
        }
    }
    (src.len(), false)
}

/// `$1` parameters, `$$ … $$` and `$tag$ … $tag$` dollar quoting.
fn scan_dollar(src: &str, start: usize) -> (TokenKind, usize, bool) {
    let rest = &src[start + 1..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        return (TokenKind::Param, start + 1 + digits, true);
    }
    let tag_len = rest
        .char_indices()
        .take_while(|(i, c)| {
            if *i == 0 {
                is_word_start(*c)
            } else {
                c.is_alphanumeric() || *c == '_'
            }
        })
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    if rest[tag_len..].starts_with('$') {
        let delimiter = &src[start..start + 1 + tag_len + 1];
        let body = start + delimiter.len();
        return match src[body..].find(delimiter) {
            Some(n) => (TokenKind::String, body + n + delimiter.len(), true),
            None => (TokenKind::String, src.len(), false),
        };
    }
    (TokenKind::Punct, start + 1, true)
}

/// The caret sits inside a string or comment token (nothing to complete).
fn caret_inside_literal(token: &Token, caret: usize) -> bool {
    token.start < caret
        && if token.closed {
            caret < token.end
        } else {
            caret <= token.end
        }
}

/// Is `caret` inside a comment or a string of the whole `text`?
fn caret_in_literal_of(text: &str, caret: usize, backslash_escapes: bool) -> bool {
    tokenize(text, backslash_escapes).iter().any(|t| {
        matches!(t.kind, TokenKind::Comment | TokenKind::String) && caret_inside_literal(t, caret)
    })
}

/// First SQL keyword of `sql`, upper-cased, skipping comments (`EXPLAIN`,
/// `SELECT`, …). `None` when the statement starts with something else.
pub fn leading_keyword(sql: &str) -> Option<String> {
    tokenize(sql, false)
        .into_iter()
        .find(|t| t.kind != TokenKind::Comment)
        .filter(|t| t.kind == TokenKind::Word)
        .map(|t| t.text(sql).to_ascii_uppercase())
}

/// Is typing `new_text` a reason to open the completion menu? A single
/// identifier character, `.`, or an opening quote.
pub fn is_trigger(new_text: &str) -> bool {
    let mut chars = new_text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => is_word_char(c) || matches!(c, '.' | '"' | '`'),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Keyword classes used by the statement state machine
// ---------------------------------------------------------------------------

/// Words that start or continue a clause: never a table name or an alias.
fn is_clause_word(up: &str) -> bool {
    matches!(
        up,
        "SELECT"
            | "FROM"
            | "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "OUTER"
            | "CROSS"
            | "NATURAL"
            | "ON"
            | "USING"
            | "WHERE"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "FETCH"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "MINUS"
            | "SET"
            | "VALUES"
            | "RETURNING"
            | "WITH"
            | "INSERT"
            | "REPLACE"
            | "UPDATE"
            | "DELETE"
            | "INTO"
            | "WINDOW"
            | "QUALIFY"
            | "FOR"
            | "STRAIGHT_JOIN"
            | "LATERAL"
            | "TABLESAMPLE"
            | "AS"
    )
}

/// Words that may sit between a clause keyword and the table name.
fn is_table_modifier(up: &str) -> bool {
    matches!(
        up,
        "IF" | "EXISTS" | "NOT" | "ONLY" | "LATERAL" | "TABLE" | "TEMPORARY" | "TEMP"
    )
}

// ---------------------------------------------------------------------------
// Statement scope
// ---------------------------------------------------------------------------

/// The byte range of the statement the caret edits. A caret right after a
/// `;` (or a blank line that ends a statement) starts a fresh, empty
/// statement at the caret: unlike when running, where whitespace after a
/// statement still belongs to it, completion must not offer the previous
/// query's tables and aliases to the next one.
pub fn statement_scope(text: &str, caret: usize, config: SplitConfig) -> Range<usize> {
    if config.unsplit {
        return 0..text.len();
    }
    scope_in(&StatementIndex::new(text, config), caret)
}

/// [`statement_scope`] in a document already split.
fn scope_in(index: &StatementIndex, caret: usize) -> Range<usize> {
    let text = index.text();
    let caret = floor_boundary(text, caret);
    if index.config().unsplit {
        return 0..text.len();
    }
    let previous = index
        .statements()
        .iter()
        .rev()
        .find(|span| span.range.start <= caret);
    match previous {
        Some(span) => {
            let range = &span.range;
            let gap = &text[range.end.min(caret)..caret];
            if caret <= range.end {
                range.clone()
            } else if gap.contains(';')
                || // An inferred boundary is a blank line right after the statement's
                // line: past that newline the caret is on it
                (span.after == Some(Boundary::Inferred) && gap.contains('\n'))
            {
                caret..caret
            } else {
                range.start..caret
            }
        }
        None => caret..caret,
    }
}

fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

// ---------------------------------------------------------------------------
// Analysis: what is being typed, and which tables the statement references
// ---------------------------------------------------------------------------

/// What kind of thing the caret is completing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// Empty statement: statement-starting keywords.
    StatementStart,
    /// A table name is expected (after `FROM`, `JOIN`, `UPDATE`, `INTO`, …).
    Tables,
    /// `schema.` in a table position: the tables of that schema.
    SchemaTables(String),
    /// `x.` in an expression: columns of table/alias `x`, or the tables of
    /// schema `x`. Holds the dotted path before the caret.
    Qualified(Vec<String>),
    /// An expression: select list, `WHERE`, `ON`, `SET`, function arguments…
    Columns,
    /// `INSERT INTO t (` — the columns of `t`.
    InsertColumns,
    /// After a table reference or a complete clause: continuation keywords.
    Clause,
}

/// A table named in the statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    /// `[table]` or `[schema, table]`; empty for a derived table (subquery,
    /// function) whose columns are unknown.
    pub path: Vec<String>,
    pub alias: Option<String>,
}

impl TableRef {
    /// How the statement refers to this table in `x.column`.
    fn handle(&self) -> Option<&str> {
        self.alias
            .as_deref()
            .or_else(|| self.path.last().map(String::as_str))
    }
}

/// The result of analysing the caret position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub context: Context,
    /// Byte range of the editor text a chosen candidate replaces.
    pub replace: Range<usize>,
    /// The text of `replace` as it is now.
    pub existing: String,
    /// What the user typed of the word being completed (without an opening
    /// quote).
    pub prefix: String,
    /// The user opened an identifier quote: candidates are inserted quoted.
    pub quote: Option<char>,
    /// Tables referenced anywhere in the statement, in order of appearance.
    pub tables: Vec<TableRef>,
    /// Common table expressions defined by the statement.
    pub ctes: Vec<String>,
}

/// Analyse what is being typed at `caret`. `None` when nothing should be
/// suggested (inside a string, a comment or a number).
pub fn analyze(text: &str, caret: usize, db_type: Option<&DatabaseType>) -> Option<Plan> {
    analyze_with(text, caret, db_type, StatementSeparators::default())
}

/// [`analyze`] with the user's statement-separator preference.
pub fn analyze_with(
    text: &str,
    caret: usize,
    db_type: Option<&DatabaseType>,
    separators: StatementSeparators,
) -> Option<Plan> {
    let index = StatementIndex::new(text, completion_split(separators, db_type));
    analyze_indexed(&index, caret, db_type)
}

/// How completion splits the buffer of an engine: like running it (MongoDB
/// input is one unit).
pub fn completion_split(
    separators: StatementSeparators,
    db_type: Option<&DatabaseType>,
) -> SplitConfig {
    let unsplit = matches!(db_type, Some(DatabaseType::MongoDB));
    SplitConfig::new(separators, db_type, unsplit)
}

/// [`analyze_with`] over a document already split into statements (the
/// split must follow the engine: see [`completion_split`]).
pub fn analyze_indexed(
    index: &StatementIndex,
    caret: usize,
    db_type: Option<&DatabaseType>,
) -> Option<Plan> {
    let text = index.text();
    let caret = floor_boundary(text, caret);
    let dialect = Dialect::of(db_type);
    let scope = scope_in(index, caret);
    // A gap holds no statement (comment-only text, or text after a `;`): the
    // scope is empty and only the whole text shows that the caret is inside
    // a comment or a string
    if scope.is_empty() && caret_in_literal_of(text, caret, dialect == Dialect::MySql) {
        return None;
    }
    let src = &text[scope.clone()];
    let rel = caret.saturating_sub(scope.start).min(src.len());
    let tokens = tokenize(src, dialect == Dialect::MySql);

    // The token under the caret decides the word being completed
    let mut word: Option<Token> = None;
    let mut quoted: Option<Token> = None;
    for token in &tokens {
        if !(token.start < rel && rel <= token.end) {
            continue;
        }
        match token.kind {
            TokenKind::Comment | TokenKind::String => {
                if caret_inside_literal(token, rel) {
                    return None;
                }
            }
            TokenKind::Number | TokenKind::Param => return None,
            TokenKind::Word => word = Some(*token),
            TokenKind::Quoted => {
                if !(token.closed && rel == token.end) {
                    quoted = Some(*token);
                }
            }
            TokenKind::Punct => {}
        }
        break;
    }

    let (replace, prefix, quote) = if let Some(token) = word {
        (
            token.start..token.end,
            src[token.start..rel].to_string(),
            None,
        )
    } else if let Some(token) = quoted {
        let open = src[token.start..].chars().next().unwrap_or('"');
        let from = token.start + open.len_utf8();
        let to = if token.closed { token.end } else { rel };
        (
            token.start..to,
            src[from..rel.max(from)].to_string(),
            Some(open),
        )
    } else {
        (rel..rel, String::new(), None)
    };

    let significant: Vec<Token> = tokens
        .iter()
        .copied()
        .filter(|t| t.kind != TokenKind::Comment)
        .collect();
    let before: Vec<Token> = significant
        .iter()
        .copied()
        .filter(|t| t.end <= replace.start)
        .collect();

    // `a.b.` right before the word
    let mut qualifier: Vec<String> = Vec::new();
    let mut cut = before.len();
    while cut >= 2 && before[cut - 1].is_punct(src, '.') && before[cut - 2].is_ident() {
        qualifier.insert(0, ident_text(src, &before[cut - 2]));
        cut -= 2;
    }

    let machine = Machine::run(src, &before[..cut]);
    let frame = machine.frame();
    let context = if let Some(last) = qualifier.last() {
        if frame.expect_table {
            // `schema.|` (or `catalog.schema.|`) where a table is expected
            Context::SchemaTables(last.clone())
        } else {
            Context::Qualified(qualifier.clone())
        }
    } else if frame.expect_table {
        Context::Tables
    } else if frame.clause == Clause::InsertColumns {
        Context::InsertColumns
    } else if frame.after_table {
        Context::Clause
    } else if frame.clause == Clause::None {
        if machine.stack.len() > 1 {
            Context::Columns
        } else if machine.seen == 0 {
            Context::StatementStart
        } else {
            Context::Clause
        }
    } else {
        Context::Columns
    };

    let all = Machine::run(src, &significant);
    let existing = src[replace.clone()].to_string();
    Some(Plan {
        context,
        replace: (replace.start + scope.start)..(replace.end + scope.start),
        existing,
        prefix,
        quote,
        tables: all.refs,
        ctes: all.ctes,
    })
}

fn ident_text(src: &str, token: &Token) -> String {
    let text = token.text(src);
    if token.kind == TokenKind::Quoted {
        let mut chars = text.chars();
        let open = chars.next().unwrap_or('"');
        let inner = &text[open.len_utf8()..];
        let inner = if token.closed {
            inner.strip_suffix(open).unwrap_or(inner)
        } else {
            inner
        };
        let doubled = format!("{open}{open}");
        inner.replace(&doubled, &open.to_string())
    } else {
        text.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    None,
    With,
    Select,
    From,
    Join,
    On,
    Where,
    Group,
    Order,
    Having,
    Set,
    Update,
    Into,
    InsertColumns,
    Values,
    Limit,
    Using,
    Returning,
    Other,
}

#[derive(Debug, Clone)]
struct Frame {
    clause: Clause,
    /// The next identifier names a table.
    expect_table: bool,
    /// A table (or derived table) was just referenced: an alias or a
    /// continuation keyword comes next.
    after_table: bool,
    after_alias: bool,
    /// Opened by `EXTRACT(` / `TRIM(` …, where `FROM` is not a clause.
    func_from: bool,
    /// Opened where a table was expected: a derived table.
    derived: bool,
    last_ref: Option<usize>,
}

impl Frame {
    fn new() -> Self {
        Self {
            clause: Clause::None,
            expect_table: false,
            after_table: false,
            after_alias: false,
            func_from: false,
            derived: false,
            last_ref: None,
        }
    }

    fn begin(&mut self, clause: Clause, expect_table: bool) {
        self.clause = clause;
        self.expect_table = expect_table;
        self.after_table = false;
        self.after_alias = false;
    }
}

/// Walks the tokens of a statement keeping the clause stack. Run over the
/// tokens before the caret it tells what is expected next; run over the
/// whole statement it collects the table references.
struct Machine {
    stack: Vec<Frame>,
    refs: Vec<TableRef>,
    ctes: Vec<String>,
    /// Significant tokens consumed.
    seen: usize,
}

impl Machine {
    fn frame(&self) -> &Frame {
        // The stack always holds the root frame
        &self.stack[self.stack.len() - 1]
    }

    fn top(&mut self) -> &mut Frame {
        let last = self.stack.len() - 1;
        &mut self.stack[last]
    }

    fn run(src: &str, tokens: &[Token]) -> Machine {
        let mut m = Machine {
            stack: vec![Frame::new()],
            refs: Vec::new(),
            ctes: Vec::new(),
            seen: 0,
        };
        let mut prev = String::new();
        let mut i = 0;
        while i < tokens.len() {
            let token = tokens[i];
            m.seen += 1;
            let up = if token.kind == TokenKind::Word {
                token.text(src).to_ascii_uppercase()
            } else {
                String::new()
            };

            match token.kind {
                TokenKind::Word | TokenKind::Quoted => {
                    let clause_word = token.kind == TokenKind::Word && is_clause_word(&up);
                    let modifier = token.kind == TokenKind::Word && is_table_modifier(&up);
                    if m.frame().expect_table && !clause_word && !modifier {
                        // A (possibly qualified) table name
                        let mut path = vec![ident_text(src, &token)];
                        let mut j = i;
                        while j + 2 < tokens.len()
                            && tokens[j + 1].is_punct(src, '.')
                            && tokens[j + 2].is_ident()
                        {
                            path.push(ident_text(src, &tokens[j + 2]));
                            j += 2;
                        }
                        let index = m.refs.len();
                        m.refs.push(TableRef { path, alias: None });
                        let frame = m.top();
                        frame.expect_table = false;
                        frame.after_table = true;
                        frame.after_alias = false;
                        frame.last_ref = Some(index);
                        prev = String::from("<ident>");
                        i = j + 1;
                        continue;
                    }
                    if token.kind == TokenKind::Word && up == "AS" && !m.frame().expect_table {
                        // alias introducer: keep the state
                        prev = up;
                        i += 1;
                        continue;
                    }
                    let (after_table, after_alias, slot) = {
                        let frame = m.frame();
                        (frame.after_table, frame.after_alias, frame.last_ref)
                    };
                    if after_table && !after_alias && !clause_word {
                        let alias = ident_text(src, &token);
                        if let Some(reference) = slot.and_then(|ix| m.refs.get_mut(ix)) {
                            reference.alias = Some(alias);
                        }
                        m.top().after_alias = true;
                        prev = String::from("<ident>");
                        i += 1;
                        continue;
                    }
                    if token.kind == TokenKind::Word {
                        m.keyword(src, tokens, i, &up, &prev);
                    }
                    prev = if token.kind == TokenKind::Word {
                        up
                    } else {
                        String::from("<ident>")
                    };
                }
                TokenKind::Punct => {
                    let c = token.text(src).chars().next().unwrap_or(' ');
                    match c {
                        '(' => {
                            let parent = m.top();
                            let derived = parent.expect_table;
                            let insert_columns =
                                parent.clause == Clause::Into && parent.after_table;
                            if derived {
                                parent.expect_table = false;
                                parent.after_table = true;
                                parent.after_alias = false;
                                parent.last_ref = None;
                            }
                            let mut frame = Frame::new();
                            frame.derived = derived;
                            frame.func_from = matches!(
                                prev.as_str(),
                                "EXTRACT" | "TRIM" | "SUBSTRING" | "OVERLAY" | "POSITION"
                            );
                            if insert_columns {
                                frame.clause = Clause::InsertColumns;
                            }
                            m.stack.push(frame);
                        }
                        ')' => {
                            if m.stack.len() > 1 {
                                let closed = m.stack.pop();
                                if closed.is_some_and(|f| f.derived) {
                                    let index = m.refs.len();
                                    m.refs.push(TableRef {
                                        path: Vec::new(),
                                        alias: None,
                                    });
                                    m.top().last_ref = Some(index);
                                }
                            }
                        }
                        ',' => {
                            let frame = m.top();
                            let more_tables = matches!(frame.clause, Clause::From | Clause::Join)
                                && frame.after_table;
                            // `JOIN b ON a.id = b.id, c`: the condition ended
                            let after_condition =
                                matches!(frame.clause, Clause::On | Clause::Using);
                            if more_tables || after_condition {
                                frame.begin(Clause::From, true);
                            }
                        }
                        _ => {}
                    }
                    prev = c.to_string();
                }
                _ => {
                    prev = String::from("<literal>");
                }
            }
            i += 1;
        }
        m
    }

    fn keyword(&mut self, src: &str, tokens: &[Token], i: usize, up: &str, prev: &str) {
        let seen = self.seen;
        let is_root_with = self.stack.len() == 1 && self.frame().clause == Clause::With;
        // `WITH name AS (` defines a common table expression
        if is_root_with
            && !is_clause_word(up)
            && let Some(name) = cte_name(src, tokens, i)
        {
            self.ctes.push(name);
        }
        let frame = self.top();
        match up {
            "SELECT" => frame.begin(Clause::Select, false),
            "FROM" => {
                if prev == "DISTINCT" || frame.func_from {
                    return;
                }
                frame.begin(Clause::From, true);
            }
            "JOIN" | "STRAIGHT_JOIN" => frame.begin(Clause::Join, true),
            "ON" => frame.begin(Clause::On, false),
            "USING" => frame.begin(Clause::Using, false),
            "WHERE" => frame.begin(Clause::Where, false),
            "GROUP" => frame.begin(Clause::Group, false),
            "ORDER" => frame.begin(Clause::Order, false),
            "HAVING" | "QUALIFY" => frame.begin(Clause::Having, false),
            "LIMIT" | "OFFSET" | "FETCH" => frame.begin(Clause::Limit, false),
            "SET" => {
                // a leading `SET` is a session command, not `UPDATE … SET`
                if seen > 1 {
                    frame.begin(Clause::Set, false);
                }
            }
            "VALUES" => frame.begin(Clause::Values, false),
            "RETURNING" => frame.begin(Clause::Returning, false),
            "UPDATE" => {
                if !matches!(prev, "FOR" | "DO" | "ON") {
                    frame.begin(Clause::Update, true);
                }
            }
            "INTO" => frame.begin(Clause::Into, true),
            "INSERT" | "DELETE" | "REPLACE" => frame.begin(Clause::None, false),
            "WITH" => {
                if seen == 1 {
                    frame.begin(Clause::With, false);
                }
            }
            "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" => frame.begin(Clause::None, false),
            "TRUNCATE" | "DESCRIBE" | "COPY" | "REFERENCES" | "LOCK" => {
                frame.begin(Clause::Other, true);
            }
            "DESC" => {
                if seen == 1 {
                    frame.begin(Clause::Other, true);
                }
            }
            "TABLE" => {
                let creating = matches!(
                    prev,
                    "CREATE"
                        | "TEMP"
                        | "TEMPORARY"
                        | "UNLOGGED"
                        | "VIRTUAL"
                        | "EXTERNAL"
                        | "GLOBAL"
                        | "LOCAL"
                );
                frame.begin(Clause::Other, !creating);
            }
            _ => {}
        }
    }
}

/// `name AS (` / `name (cols) AS (` / `name AS [NOT] MATERIALIZED (`.
fn cte_name(src: &str, tokens: &[Token], i: usize) -> Option<String> {
    let name = &tokens[i];
    if !name.is_ident() {
        return None;
    }
    let mut j = i + 1;
    if tokens.get(j)?.is_punct(src, '(') {
        let mut depth = 0usize;
        while let Some(token) = tokens.get(j) {
            if token.is_punct(src, '(') {
                depth += 1;
            } else if token.is_punct(src, ')') {
                depth -= 1;
                if depth == 0 {
                    j += 1;
                    break;
                }
            }
            j += 1;
        }
    }
    let as_token = tokens.get(j)?;
    if as_token.kind != TokenKind::Word || !as_token.text(src).eq_ignore_ascii_case("AS") {
        return None;
    }
    j += 1;
    while let Some(token) = tokens.get(j) {
        let is_modifier = token.kind == TokenKind::Word
            && matches!(
                token.text(src).to_ascii_uppercase().as_str(),
                "NOT" | "MATERIALIZED"
            );
        if is_modifier {
            j += 1;
        } else {
            break;
        }
    }
    tokens
        .get(j)?
        .is_punct(src, '(')
        .then(|| ident_text(src, name))
}

// ---------------------------------------------------------------------------
// Resolving references against the schema
// ---------------------------------------------------------------------------

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
}

/// Find the schema object named by `path` (`[table]` or `[schema, table]`).
/// Without a schema, the default schema wins over other schemas, and an
/// exact-case match over a case-insensitive one.
pub fn resolve_table<'a>(
    tables: &'a [TableEntry],
    db_type: Option<&DatabaseType>,
    path: &[String],
) -> Option<&'a TableEntry> {
    let (schema, name) = match path {
        [] => return None,
        [name] => (None, name),
        [.., schema, name] => (Some(schema), name),
    };
    tables
        .iter()
        .filter(|t| eq_ci(&t.name, name))
        .filter(|t| match schema {
            Some(schema) => t.schema.as_deref().is_some_and(|s| eq_ci(s, schema)),
            None => true,
        })
        .min_by_key(|t| {
            let default = t.schema.as_deref().is_none_or(|s| match db_type {
                Some(db_type) => is_default_schema(db_type, s),
                None => matches!(s, "public" | "main"),
            });
            (t.name != *name, !default)
        })
}

/// What `x.` refers to.
enum QualifiedTarget<'a> {
    Columns(&'a TableEntry),
    Schema(String),
    Unknown,
}

fn resolve_qualified<'a>(
    plan: &Plan,
    tables: &'a [TableEntry],
    db_type: Option<&DatabaseType>,
    path: &[String],
) -> QualifiedTarget<'a> {
    if let [handle] = path {
        // An alias, or a table named in the statement
        let by_alias = plan
            .tables
            .iter()
            .find(|r| r.alias.as_deref().is_some_and(|a| eq_ci(a, handle)));
        let by_name = || {
            plan.tables
                .iter()
                .find(|r| r.alias.is_none() && r.path.last().is_some_and(|n| eq_ci(n, handle)))
        };
        if let Some(reference) = by_alias.or_else(by_name) {
            return match resolve_table(tables, db_type, &reference.path) {
                Some(entry) => QualifiedTarget::Columns(entry),
                None => QualifiedTarget::Unknown,
            };
        }
    }
    if let Some(entry) = resolve_table(tables, db_type, path) {
        return QualifiedTarget::Columns(entry);
    }
    if let [schema] = path
        && tables
            .iter()
            .any(|t| t.schema.as_deref().is_some_and(|s| eq_ci(s, schema)))
    {
        return QualifiedTarget::Schema(schema.clone());
    }
    QualifiedTarget::Unknown
}

impl Plan {
    /// The schema objects whose columns [`suggest`] needs (describe them and
    /// pass the result as `columns`, keyed by `TableEntry::qualified_name`).
    pub fn needed_tables<'a>(
        &self,
        tables: &'a [TableEntry],
        db_type: Option<&DatabaseType>,
    ) -> Vec<&'a TableEntry> {
        let mut needed: Vec<&TableEntry> = Vec::new();
        let mut add = |entry: Option<&'a TableEntry>| {
            if let Some(entry) = entry
                && needed.len() < MAX_TABLES_PER_STATEMENT
                && !needed.contains(&entry)
            {
                needed.push(entry);
            }
        };
        match &self.context {
            Context::Qualified(path) => {
                if let QualifiedTarget::Columns(entry) =
                    resolve_qualified(self, tables, db_type, path)
                {
                    add(Some(entry));
                }
            }
            Context::Columns => {
                for reference in &self.tables {
                    add(resolve_table(tables, db_type, &reference.path));
                }
            }
            Context::InsertColumns => {
                if let Some(reference) = self.tables.first() {
                    add(resolve_table(tables, db_type, &reference.path));
                }
            }
            _ => {}
        }
        needed
    }
}

// ---------------------------------------------------------------------------
// Candidates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateKind {
    Table,
    View,
    Schema,
    Cte,
    Column,
    Alias,
    Function,
    Keyword,
}

/// A column offered for completion (from a described table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnCandidate {
    pub name: String,
    pub data_type: String,
}

impl ColumnCandidate {
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
        }
    }
}

/// One entry of the completion menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What the menu shows (and what the typed text is matched against).
    pub label: String,
    /// What replaces `Plan::replace` when chosen (quoted where needed).
    pub insert: String,
    pub kind: CandidateKind,
    /// Short right-aligned hint: the column type, `table`, `view`, …
    pub detail: String,
    /// Bytes of `label` the typed prefix matched from the start (0 when the
    /// match is not a prefix match). Always on a char boundary.
    pub matched: usize,
}

/// The schema as completion sees it.
pub struct SchemaView<'a> {
    pub db_type: Option<&'a DatabaseType>,
    pub tables: &'a [TableEntry],
    /// Described columns by `TableEntry::qualified_name()`.
    pub columns: &'a HashMap<String, Vec<ColumnCandidate>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Subsequence,
    Boundary,
    Prefix,
    Exact,
}

/// How well `prefix` matches `label` (case-insensitive); `None` = no match.
/// The second value orders matches within a tier (higher is better).
#[cfg(test)]
fn match_label(prefix: &str, label: &str) -> Option<(Tier, i32)> {
    if prefix.is_empty() {
        return Some((Tier::Prefix, 0));
    }
    let p: Vec<char> = prefix.chars().flat_map(char::to_lowercase).collect();
    match_prepared(&p, &PreparedName::new(label))
}

/// A label as [`match_prepared`] reads it: case-folded, and as written (for
/// word boundaries).
#[derive(Debug)]
struct PreparedName {
    lower: Vec<char>,
    original: Vec<char>,
}

impl PreparedName {
    fn new(label: &str) -> Self {
        Self {
            lower: label.chars().flat_map(char::to_lowercase).collect(),
            original: label.chars().collect(),
        }
    }
}

/// The table labels of one schema snapshot, prepared once for matching.
/// Completion requests of the same snapshot share it, so a keystroke does
/// not case-fold thousands of names again. Labels not found here (columns,
/// keywords, CTEs) are prepared on the fly: results are identical either way.
#[derive(Debug, Default)]
pub struct CompletionNames(HashMap<String, PreparedName>);

impl CompletionNames {
    /// Prepare the bare and the schema-qualified label of every table.
    pub fn new(tables: &[TableEntry]) -> Self {
        let mut names = HashMap::with_capacity(tables.len() * 2);
        for table in tables {
            for name in [table.name.clone(), table.qualified_name()] {
                if let std::collections::hash_map::Entry::Vacant(slot) = names.entry(name) {
                    let prepared = PreparedName::new(slot.key());
                    slot.insert(prepared);
                }
            }
        }
        Self(names)
    }
}

/// [`match_label`] over a case-folded prefix and a prepared label.
fn match_prepared(p: &[char], name: &PreparedName) -> Option<(Tier, i32)> {
    if p.is_empty() {
        return Some((Tier::Prefix, 0));
    }
    let l = &name.lower;
    if l.as_slice() == p {
        return Some((Tier::Exact, 0));
    }
    if l.starts_with(p) {
        return Some((Tier::Prefix, -((l.len() - p.len()) as i32)));
    }
    // Word-boundary: the prefix starts a word inside the label
    let original = &name.original;
    if original.len() == l.len() {
        for ix in 1..original.len() {
            let before = original[ix - 1];
            let here = original[ix];
            let boundary = !before.is_alphanumeric()
                || (before.is_lowercase() && here.is_uppercase())
                || (before.is_alphabetic() != here.is_alphabetic());
            if boundary && l[ix..].starts_with(p) {
                return Some((
                    Tier::Boundary,
                    -(ix as i32) * 4 - (l.len() - p.len()) as i32,
                ));
            }
        }
    }
    // Subsequence: every typed char appears in order
    let mut wanted = p.iter().peekable();
    let mut gaps = 0i32;
    let mut run_started = false;
    for c in l {
        match wanted.peek() {
            Some(next) if *next == c => {
                wanted.next();
                run_started = true;
            }
            Some(_) if run_started => gaps += 1,
            _ => {}
        }
    }
    wanted
        .peek()
        .is_none()
        .then_some((Tier::Subsequence, -gaps))
}

/// Ranking weight of a kind in a context (higher first).
fn kind_priority(context: &Context, kind: CandidateKind, system: bool) -> i32 {
    use CandidateKind::*;
    let base = match context {
        Context::Tables | Context::SchemaTables(_) => match kind {
            Cte => 6,
            Table | View => 5,
            Schema => 3,
            _ => 1,
        },
        Context::Qualified(_) => match kind {
            Column => 6,
            Table | View => 5,
            _ => 1,
        },
        Context::Columns | Context::InsertColumns => match kind {
            Column => 6,
            Alias => 5,
            Function => 4,
            Keyword => 3,
            Table | View | Cte | Schema => 1,
        },
        Context::Clause | Context::StatementStart => match kind {
            Keyword => 5,
            _ => 1,
        },
    };
    if system { base - 1 } else { base }
}

struct Entry {
    candidate: Candidate,
    tier: Tier,
    fine: i32,
    priority: i32,
    order: usize,
}

struct Collector<'a> {
    prefix: &'a str,
    folded_prefix: Vec<char>,
    names: Option<&'a CompletionNames>,
    context: &'a Context,
    existing: &'a str,
    /// The word already typed is itself a candidate.
    complete: bool,
    /// Added to the ranking weight of what is pushed next.
    bonus: i32,
    entries: Vec<Entry>,
}

impl Collector<'_> {
    fn push(
        &mut self,
        label: String,
        insert: String,
        kind: CandidateKind,
        detail: String,
        system: bool,
    ) {
        // The word is already complete: offering it again only swallows Enter
        if insert == self.existing {
            self.complete = true;
            return;
        }
        let prepared;
        let name = match self.names.and_then(|names| names.0.get(&label)) {
            Some(name) => name,
            None => {
                prepared = PreparedName::new(&label);
                &prepared
            }
        };
        let Some((tier, fine)) = match_prepared(&self.folded_prefix, name) else {
            return;
        };
        // Keywords keep their curated order; names favour the shorter match
        let fine = if kind == CandidateKind::Keyword {
            0
        } else {
            fine
        };
        let matched = if tier >= Tier::Prefix {
            prefix_len_in(&label, self.prefix)
        } else {
            0
        };
        self.entries.push(Entry {
            candidate: Candidate {
                label,
                insert,
                kind,
                detail,
                matched,
            },
            tier,
            fine,
            priority: kind_priority(self.context, kind, system) + self.bonus,
            order: self.entries.len(),
        });
    }

    fn finish(mut self) -> Vec<Candidate> {
        if self.complete {
            // `orders` typed in full: only longer names that extend it stay
            self.entries.retain(|entry| entry.tier >= Tier::Prefix);
        }
        self.entries.sort_by(|a, b| {
            b.tier
                .cmp(&a.tier)
                .then(b.priority.cmp(&a.priority))
                .then(b.fine.cmp(&a.fine))
                .then(a.order.cmp(&b.order))
        });
        let mut seen: Vec<(String, CandidateKind)> = Vec::new();
        let mut result = Vec::new();
        for entry in self.entries {
            let key = (entry.candidate.insert.clone(), entry.candidate.kind);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            result.push(entry.candidate);
            if result.len() >= MAX_CANDIDATES {
                break;
            }
        }
        result
    }
}

/// Byte length of the part of `label` that a case-insensitive `prefix`
/// covers; 0 when `label` does not start with it.
fn prefix_len_in(label: &str, prefix: &str) -> usize {
    let mut covered = 0;
    let mut typed = prefix.chars().flat_map(char::to_lowercase);
    for (ix, c) in label.char_indices() {
        let mut lowered = c.to_lowercase();
        let ok = lowered.all(|l| typed.next() == Some(l));
        if !ok {
            return 0;
        }
        covered = ix + c.len_utf8();
        if typed.clone().next().is_none() {
            return covered;
        }
    }
    if typed.next().is_none() { covered } else { 0 }
}

/// Keywords follow the case the user types in: lower-case when the typed
/// prefix is non-empty and has no capitals, upper-case otherwise.
fn keyword_case(word: &str, prefix: &str) -> String {
    if !prefix.is_empty() && !prefix.chars().any(char::is_uppercase) {
        word.to_lowercase()
    } else {
        word.to_string()
    }
}

fn quote_always(quote: char, ident: &str) -> String {
    let doubled = format!("{quote}{quote}");
    format!("{quote}{}{quote}", ident.replace(quote, &doubled))
}

/// Build the ranked candidates for `plan`.
pub fn suggest(plan: &Plan, schema: &SchemaView<'_>) -> Vec<Candidate> {
    suggest_prepared(plan, schema, None)
}

/// [`suggest`] with the table labels of `schema` prepared beforehand (see
/// [`CompletionNames`]); `names` must come from the same table list.
pub fn suggest_prepared(
    plan: &Plan,
    schema: &SchemaView<'_>,
    names: Option<&CompletionNames>,
) -> Vec<Candidate> {
    let dialect = Dialect::of(schema.db_type);
    let mut out = Collector {
        prefix: &plan.prefix,
        folded_prefix: plan.prefix.chars().flat_map(char::to_lowercase).collect(),
        names,
        context: &plan.context,
        existing: &plan.existing,
        complete: false,
        bonus: 0,
        entries: Vec::new(),
    };
    let ident = |name: &str| match (plan.quote, schema.db_type) {
        (Some(quote), _) => quote_always(quote, name),
        (None, Some(db_type)) => quote_identifier(db_type, name),
        (None, None) => name.to_string(),
    };

    match &plan.context {
        Context::StatementStart => {
            add_keywords(&mut out, statement_start_keywords(dialect), &plan.prefix);
        }
        Context::Clause => {
            add_keywords(&mut out, clause_keywords(dialect), &plan.prefix);
            add_keywords(&mut out, general_keywords(dialect), &plan.prefix);
        }
        Context::Tables => {
            for cte in &plan.ctes {
                out.push(
                    cte.clone(),
                    ident(cte),
                    CandidateKind::Cte,
                    "CTE".into(),
                    false,
                );
            }
            add_tables(&mut out, schema, plan, None);
        }
        Context::SchemaTables(name) => {
            let mongo_collections =
                matches!(schema.db_type, Some(DatabaseType::MongoDB)) && name == "db";
            add_tables(
                &mut out,
                schema,
                plan,
                (!mongo_collections).then_some(name.as_str()),
            );
        }
        Context::Qualified(path)
            if matches!(schema.db_type, Some(DatabaseType::MongoDB))
                && path.len() == 1
                && path[0] == "db" =>
        {
            // `db.` in the Mongo shell syntax lists collections
            add_tables(&mut out, schema, plan, None);
        }
        Context::Qualified(path) => {
            match resolve_qualified(plan, schema.tables, schema.db_type, path) {
                QualifiedTarget::Columns(entry) => {
                    if plan.prefix.is_empty() && plan.quote.is_none() {
                        out.push(
                            "*".into(),
                            "*".into(),
                            CandidateKind::Column,
                            "all columns".into(),
                            false,
                        );
                    }
                    if let Some(columns) = schema.columns.get(&entry.qualified_name()) {
                        for column in columns {
                            out.push(
                                column.name.clone(),
                                ident(&column.name),
                                CandidateKind::Column,
                                column.data_type.clone(),
                                false,
                            );
                        }
                    }
                }
                QualifiedTarget::Schema(name) => add_tables(&mut out, schema, plan, Some(&name)),
                QualifiedTarget::Unknown => {}
            }
        }
        Context::InsertColumns => {
            if let Some(entry) = plan
                .tables
                .first()
                .and_then(|r| resolve_table(schema.tables, schema.db_type, &r.path))
                && let Some(columns) = schema.columns.get(&entry.qualified_name())
            {
                for column in columns {
                    out.push(
                        column.name.clone(),
                        ident(&column.name),
                        CandidateKind::Column,
                        column.data_type.clone(),
                        false,
                    );
                }
            }
        }
        Context::Columns => {
            add_expression_candidates(&mut out, schema, plan, dialect, &ident);
        }
    }
    out.finish()
}

fn add_expression_candidates(
    out: &mut Collector<'_>,
    schema: &SchemaView<'_>,
    plan: &Plan,
    dialect: Dialect,
    ident: &dyn Fn(&str) -> String,
) {
    // Resolve each reference once, in statement order
    let mut resolved: Vec<(&TableRef, &TableEntry)> = Vec::new();
    for reference in &plan.tables {
        if let Some(entry) = resolve_table(schema.tables, schema.db_type, &reference.path)
            && !resolved
                .iter()
                .any(|(r, e)| *e == entry && r.alias == reference.alias)
        {
            resolved.push((reference, entry));
        }
    }
    // A column present in several referenced tables is inserted qualified
    let mut seen_names: HashMap<String, usize> = HashMap::new();
    for (_, entry) in &resolved {
        if let Some(columns) = schema.columns.get(&entry.qualified_name()) {
            for column in columns {
                *seen_names.entry(column.name.to_lowercase()).or_default() += 1;
            }
        }
    }
    for (reference, entry) in &resolved {
        let Some(columns) = schema.columns.get(&entry.qualified_name()) else {
            continue;
        };
        let handle = reference.handle().unwrap_or(&entry.name);
        for column in columns {
            let ambiguous = seen_names
                .get(&column.name.to_lowercase())
                .copied()
                .unwrap_or(0)
                > 1;
            let insert = if ambiguous {
                format!("{}.{}", ident(handle), ident(&column.name))
            } else {
                ident(&column.name)
            };
            out.push(
                column.name.clone(),
                insert,
                CandidateKind::Column,
                format!("{} · {}", column.data_type, handle),
                false,
            );
        }
    }
    // Table handles: `o` → `o.`
    for reference in &plan.tables {
        if let Some(handle) = reference.handle() {
            let detail = match (&reference.alias, reference.path.last()) {
                (Some(_), Some(table)) => table.clone(),
                _ => "table".to_string(),
            };
            out.push(
                handle.to_string(),
                ident(handle),
                CandidateKind::Alias,
                detail,
                false,
            );
        }
    }
    // `GROUP BY`, `ORDER BY`, `LIMIT`… continue the statement: once the user
    // typed something they come ahead of functions
    out.bonus = if plan.prefix.is_empty() { 0 } else { 2 };
    add_keywords(out, clause_keywords(dialect), &plan.prefix);
    out.bonus = 0;
    for (name, detail) in functions(dialect) {
        out.push(
            (*name).to_string(),
            keyword_case(name, &plan.prefix),
            CandidateKind::Function,
            (*detail).to_string(),
            false,
        );
    }
    add_keywords(out, general_keywords(dialect), &plan.prefix);
}

fn add_keywords(out: &mut Collector<'_>, words: &[&str], prefix: &str) {
    for word in words {
        out.push(
            (*word).to_string(),
            keyword_case(word, prefix),
            CandidateKind::Keyword,
            String::new(),
            false,
        );
    }
}

/// Tables and views; `only_schema` restricts to one schema (and shows bare
/// names), otherwise non-default schemas are shown as `schema.name`.
fn add_tables(
    out: &mut Collector<'_>,
    schema: &SchemaView<'_>,
    plan: &Plan,
    only_schema: Option<&str>,
) {
    for entry in schema.tables {
        if let Some(only) = only_schema
            && !entry.schema.as_deref().is_some_and(|s| eq_ci(s, only))
        {
            continue;
        }
        let system = schema
            .db_type
            .is_some_and(|db_type| is_system_object(db_type, entry));
        let (label, insert) = match (only_schema, schema.db_type) {
            (Some(_), db_type) => (
                entry.name.clone(),
                match (plan.quote, db_type) {
                    (Some(quote), _) => quote_always(quote, &entry.name),
                    (None, Some(db_type)) => quote_identifier(db_type, &entry.name),
                    (None, None) => entry.name.clone(),
                },
            ),
            (None, Some(db_type)) => {
                let reference = match plan.quote {
                    Some(quote) => match entry.schema.as_deref() {
                        Some(s) if !is_default_schema(db_type, s) => {
                            format!(
                                "{}.{}",
                                quote_always(quote, s),
                                quote_always(quote, &entry.name)
                            )
                        }
                        _ => quote_always(quote, &entry.name),
                    },
                    None => table_reference(db_type, entry),
                };
                let label = match entry.schema.as_deref() {
                    Some(s) if !is_default_schema(db_type, s) => format!("{s}.{}", entry.name),
                    _ => entry.name.clone(),
                };
                (label, reference)
            }
            (None, None) => (entry.qualified_name(), entry.qualified_name()),
        };
        let kind = match entry.kind {
            TableKind::View | TableKind::MaterializedView => CandidateKind::View,
            _ => CandidateKind::Table,
        };
        out.push(label, insert, kind, entry.kind.label().to_string(), system);
    }
}

// ---------------------------------------------------------------------------
// Keyword and function sets
// ---------------------------------------------------------------------------

fn statement_start_keywords(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        Dialect::PostgreSql => &[
            "SELECT",
            "WITH",
            "INSERT INTO",
            "UPDATE",
            "DELETE FROM",
            "CREATE TABLE",
            "CREATE INDEX",
            "CREATE VIEW",
            "ALTER TABLE",
            "DROP TABLE",
            "DROP INDEX",
            "DROP VIEW",
            "TRUNCATE",
            "EXPLAIN",
            "EXPLAIN ANALYZE",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "SHOW",
            "SET",
            "ANALYZE",
            "VACUUM",
            "COPY",
            "GRANT",
            "REVOKE",
            "COMMENT ON",
        ],
        Dialect::MySql => &[
            "SELECT",
            "WITH",
            "INSERT INTO",
            "REPLACE INTO",
            "UPDATE",
            "DELETE FROM",
            "CREATE TABLE",
            "CREATE INDEX",
            "CREATE VIEW",
            "ALTER TABLE",
            "DROP TABLE",
            "DROP INDEX",
            "DROP VIEW",
            "TRUNCATE",
            "EXPLAIN",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "SHOW TABLES",
            "SHOW DATABASES",
            "DESCRIBE",
            "USE",
            "SET",
            "OPTIMIZE TABLE",
            "GRANT",
            "REVOKE",
        ],
        Dialect::Sqlite => &[
            "SELECT",
            "WITH",
            "INSERT INTO",
            "REPLACE INTO",
            "UPDATE",
            "DELETE FROM",
            "CREATE TABLE",
            "CREATE INDEX",
            "CREATE VIEW",
            "ALTER TABLE",
            "DROP TABLE",
            "DROP INDEX",
            "DROP VIEW",
            "EXPLAIN",
            "EXPLAIN QUERY PLAN",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "PRAGMA",
            "VACUUM",
            "ATTACH",
            "DETACH",
            "REINDEX",
            "ANALYZE",
        ],
        Dialect::Generic => &[
            "SELECT",
            "WITH",
            "INSERT INTO",
            "UPDATE",
            "DELETE FROM",
            "CREATE TABLE",
            "CREATE VIEW",
            "ALTER TABLE",
            "DROP TABLE",
            "DROP VIEW",
            "TRUNCATE",
            "EXPLAIN",
            "DESCRIBE",
            "SHOW",
            "SET",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
        ],
    }
}

fn clause_keywords(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        Dialect::PostgreSql => &[
            "JOIN",
            "INNER JOIN",
            "LEFT JOIN",
            "LEFT OUTER JOIN",
            "RIGHT JOIN",
            "FULL JOIN",
            "CROSS JOIN",
            "NATURAL JOIN",
            "ON",
            "USING",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "AS",
            "SET",
            "VALUES",
            "SELECT",
            "RETURNING",
            "FOR UPDATE",
            "WINDOW",
            "FETCH FIRST",
            "ON CONFLICT",
            "LATERAL",
        ],
        Dialect::MySql => &[
            "JOIN",
            "INNER JOIN",
            "LEFT JOIN",
            "LEFT OUTER JOIN",
            "RIGHT JOIN",
            "CROSS JOIN",
            "NATURAL JOIN",
            "STRAIGHT_JOIN",
            "ON",
            "USING",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "AS",
            "SET",
            "VALUES",
            "SELECT",
            "FOR UPDATE",
            "ON DUPLICATE KEY UPDATE",
        ],
        Dialect::Sqlite => &[
            "JOIN",
            "INNER JOIN",
            "LEFT JOIN",
            "LEFT OUTER JOIN",
            "CROSS JOIN",
            "NATURAL JOIN",
            "ON",
            "USING",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "AS",
            "SET",
            "VALUES",
            "SELECT",
            "RETURNING",
            "INDEXED BY",
            "NOT INDEXED",
            "ON CONFLICT",
        ],
        Dialect::Generic => &[
            "JOIN",
            "INNER JOIN",
            "LEFT JOIN",
            "LEFT OUTER JOIN",
            "RIGHT JOIN",
            "FULL JOIN",
            "CROSS JOIN",
            "ON",
            "USING",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "AS",
            "SET",
            "VALUES",
            "SELECT",
        ],
    }
}

fn general_keywords(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        Dialect::PostgreSql => &[
            "SELECT",
            "FROM",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "DISTINCT ON",
            "AS",
            "AND",
            "OR",
            "NOT",
            "IN",
            "IS",
            "NULL",
            "IS NULL",
            "IS NOT NULL",
            "LIKE",
            "ILIKE",
            "SIMILAR TO",
            "BETWEEN",
            "EXISTS",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "JOIN",
            "LEFT JOIN",
            "RIGHT JOIN",
            "INNER JOIN",
            "FULL JOIN",
            "CROSS JOIN",
            "ON",
            "USING",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "ALL",
            "ANY",
            "SOME",
            "ASC",
            "DESC",
            "NULLS FIRST",
            "NULLS LAST",
            "TRUE",
            "FALSE",
            "DEFAULT",
            "WITH",
            "RECURSIVE",
            "OVER",
            "PARTITION BY",
            "FILTER",
            "WITHIN GROUP",
            "LATERAL",
            "RETURNING",
            "CAST",
            "INTERVAL",
            "ARRAY",
            "FOR UPDATE",
            "ON CONFLICT",
            "DO NOTHING",
            "DO UPDATE SET",
            "VALUES",
            "SET",
        ],
        Dialect::MySql => &[
            "SELECT",
            "FROM",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "AS",
            "AND",
            "OR",
            "NOT",
            "IN",
            "IS",
            "NULL",
            "IS NULL",
            "IS NOT NULL",
            "LIKE",
            "REGEXP",
            "RLIKE",
            "BETWEEN",
            "EXISTS",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "JOIN",
            "LEFT JOIN",
            "RIGHT JOIN",
            "INNER JOIN",
            "CROSS JOIN",
            "STRAIGHT_JOIN",
            "ON",
            "USING",
            "UNION",
            "UNION ALL",
            "ALL",
            "ANY",
            "ASC",
            "DESC",
            "TRUE",
            "FALSE",
            "DEFAULT",
            "WITH",
            "RECURSIVE",
            "OVER",
            "PARTITION BY",
            "INTERVAL",
            "FOR UPDATE",
            "ON DUPLICATE KEY UPDATE",
            "VALUES",
            "SET",
            "UNSIGNED",
            "AUTO_INCREMENT",
        ],
        Dialect::Sqlite => &[
            "SELECT",
            "FROM",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "AS",
            "AND",
            "OR",
            "NOT",
            "IN",
            "IS",
            "NULL",
            "IS NULL",
            "IS NOT NULL",
            "LIKE",
            "GLOB",
            "BETWEEN",
            "EXISTS",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "JOIN",
            "LEFT JOIN",
            "INNER JOIN",
            "CROSS JOIN",
            "NATURAL JOIN",
            "ON",
            "USING",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "ALL",
            "ASC",
            "DESC",
            "NULLS FIRST",
            "NULLS LAST",
            "TRUE",
            "FALSE",
            "DEFAULT",
            "WITH",
            "RECURSIVE",
            "OVER",
            "PARTITION BY",
            "FILTER",
            "RETURNING",
            "CAST",
            "COLLATE",
            "ON CONFLICT",
            "DO NOTHING",
            "DO UPDATE SET",
            "VALUES",
            "SET",
            "AUTOINCREMENT",
            "WITHOUT ROWID",
        ],
        Dialect::Generic => &[
            "SELECT",
            "FROM",
            "WHERE",
            "GROUP BY",
            "HAVING",
            "ORDER BY",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "AS",
            "AND",
            "OR",
            "NOT",
            "IN",
            "IS",
            "NULL",
            "IS NULL",
            "IS NOT NULL",
            "LIKE",
            "BETWEEN",
            "EXISTS",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "JOIN",
            "LEFT JOIN",
            "RIGHT JOIN",
            "INNER JOIN",
            "FULL JOIN",
            "CROSS JOIN",
            "ON",
            "USING",
            "UNION",
            "UNION ALL",
            "INTERSECT",
            "EXCEPT",
            "ALL",
            "ANY",
            "ASC",
            "DESC",
            "TRUE",
            "FALSE",
            "DEFAULT",
            "WITH",
            "OVER",
            "PARTITION BY",
            "CAST",
            "VALUES",
            "SET",
        ],
    }
}

const COMMON_FUNCTIONS: &[(&str, &str)] = &[
    ("COUNT", "aggregate"),
    ("SUM", "aggregate"),
    ("AVG", "aggregate"),
    ("MIN", "aggregate"),
    ("MAX", "aggregate"),
    ("COALESCE", "first non-null"),
    ("NULLIF", "null if equal"),
    ("CAST", "convert type"),
    ("LOWER", "string"),
    ("UPPER", "string"),
    ("LENGTH", "string"),
    ("TRIM", "string"),
    ("LTRIM", "string"),
    ("RTRIM", "string"),
    ("SUBSTRING", "string"),
    ("REPLACE", "string"),
    ("CONCAT", "string"),
    ("ROUND", "number"),
    ("CEIL", "number"),
    ("FLOOR", "number"),
    ("ABS", "number"),
    ("MOD", "number"),
    ("POWER", "number"),
    ("SQRT", "number"),
    ("NOW", "date/time"),
    ("CURRENT_DATE", "date/time"),
    ("CURRENT_TIMESTAMP", "date/time"),
    ("EXTRACT", "date/time"),
    ("ROW_NUMBER", "window"),
    ("RANK", "window"),
    ("DENSE_RANK", "window"),
    ("LAG", "window"),
    ("LEAD", "window"),
    ("FIRST_VALUE", "window"),
    ("LAST_VALUE", "window"),
    ("NTILE", "window"),
];

const POSTGRES_FUNCTIONS: &[(&str, &str)] = &[
    ("DATE_TRUNC", "date/time"),
    ("TO_CHAR", "format"),
    ("TO_DATE", "parse"),
    ("TO_TIMESTAMP", "parse"),
    ("AGE", "date/time"),
    ("STRING_AGG", "aggregate"),
    ("ARRAY_AGG", "aggregate"),
    ("JSON_AGG", "aggregate"),
    ("JSONB_AGG", "aggregate"),
    ("JSONB_BUILD_OBJECT", "json"),
    ("JSON_BUILD_OBJECT", "json"),
    ("JSONB_EXTRACT_PATH_TEXT", "json"),
    ("GENERATE_SERIES", "set-returning"),
    ("UNNEST", "set-returning"),
    ("POSITION", "string"),
    ("SPLIT_PART", "string"),
    ("REGEXP_REPLACE", "string"),
    ("REGEXP_MATCHES", "string"),
    ("GREATEST", "number"),
    ("LEAST", "number"),
    ("RANDOM", "number"),
    ("PG_SIZE_PRETTY", "size"),
    ("PG_TOTAL_RELATION_SIZE", "size"),
];

const MYSQL_FUNCTIONS: &[(&str, &str)] = &[
    ("IFNULL", "first non-null"),
    ("IF", "conditional"),
    ("DATE_FORMAT", "format"),
    ("STR_TO_DATE", "parse"),
    ("DATEDIFF", "date/time"),
    ("DATE_ADD", "date/time"),
    ("DATE_SUB", "date/time"),
    ("TIMESTAMPDIFF", "date/time"),
    ("GROUP_CONCAT", "aggregate"),
    ("CONCAT_WS", "string"),
    ("JSON_EXTRACT", "json"),
    ("JSON_OBJECT", "json"),
    ("GREATEST", "number"),
    ("LEAST", "number"),
    ("RAND", "number"),
    ("LAST_INSERT_ID", "session"),
];

const SQLITE_FUNCTIONS: &[(&str, &str)] = &[
    ("IFNULL", "first non-null"),
    ("IIF", "conditional"),
    ("STRFTIME", "format"),
    ("DATE", "date/time"),
    ("TIME", "date/time"),
    ("DATETIME", "date/time"),
    ("JULIANDAY", "date/time"),
    ("GROUP_CONCAT", "aggregate"),
    ("SUBSTR", "string"),
    ("INSTR", "string"),
    ("TYPEOF", "type"),
    ("RANDOM", "number"),
    ("JSON_EXTRACT", "json"),
    ("JSON_GROUP_ARRAY", "aggregate"),
    ("LAST_INSERT_ROWID", "session"),
];

fn functions(dialect: Dialect) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let extra: &'static [(&'static str, &'static str)] = match dialect {
        Dialect::PostgreSql => POSTGRES_FUNCTIONS,
        Dialect::MySql => MYSQL_FUNCTIONS,
        Dialect::Sqlite => SQLITE_FUNCTIONS,
        Dialect::Generic => &[],
    };
    COMMON_FUNCTIONS.iter().chain(extra.iter())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ---- helpers ----

    /// Split `text` at the `|` marker into (text, caret).
    fn at_caret(marked: &str) -> (String, usize) {
        let caret = marked.find('|').expect("marker");
        (marked.replacen('|', "", 1), caret)
    }

    fn entry(schema: Option<&str>, name: &str, kind: TableKind) -> TableEntry {
        TableEntry {
            schema: schema.map(str::to_string),
            name: name.into(),
            kind,
            owner: None,
        }
    }

    fn shop() -> Vec<TableEntry> {
        vec![
            entry(None, "customers", TableKind::Table),
            entry(None, "order_items", TableKind::Table),
            entry(None, "orders", TableKind::Table),
            entry(None, "products", TableKind::Table),
            entry(None, "revenue_by_month", TableKind::View),
            entry(None, "sqlite_sequence", TableKind::Table),
        ]
    }

    fn shop_columns() -> HashMap<String, Vec<ColumnCandidate>> {
        let mut columns = HashMap::new();
        columns.insert(
            "orders".to_string(),
            vec![
                ColumnCandidate::new("id", "INTEGER"),
                ColumnCandidate::new("customer_id", "INTEGER"),
                ColumnCandidate::new("status", "TEXT"),
                ColumnCandidate::new("total", "REAL"),
            ],
        );
        columns.insert(
            "customers".to_string(),
            vec![
                ColumnCandidate::new("id", "INTEGER"),
                ColumnCandidate::new("name", "TEXT"),
                ColumnCandidate::new("email", "TEXT"),
            ],
        );
        columns
    }

    fn plan(marked: &str) -> Plan {
        let (text, caret) = at_caret(marked);
        analyze(&text, caret, Some(&DatabaseType::SQLite)).expect("a plan")
    }

    fn labels(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.label.as_str()).collect()
    }

    fn suggestions(marked: &str) -> Vec<Candidate> {
        let plan = plan(marked);
        let tables = shop();
        let columns = shop_columns();
        suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::SQLite),
                tables: &tables,
                columns: &columns,
            },
        )
    }

    // ---- prepared requests ----

    fn mixed_schema() -> Vec<TableEntry> {
        let mut tables = shop();
        tables.extend([
            entry(Some("main"), "Orders_Archive", TableKind::Table),
            entry(Some("audit"), "orders", TableKind::Table),
            entry(Some("audit"), "OrderEvents", TableKind::View),
            entry(
                Some("reporting"),
                "café_ventes",
                TableKind::MaterializedView,
            ),
            entry(Some("reporting"), "IStatus", TableKind::Table),
        ]);
        tables
    }

    const REQUESTS: &[&str] = &[
        "SELECT * FROM |",
        "SELECT * FROM or|",
        "SELECT * FROM OR|",
        "SELECT * FROM oe|",
        "SELECT * FROM audit.|",
        "SELECT * FROM audit.or|",
        "SELECT * FROM caf|",
        "SELECT * FROM st|",
        "SELECT * FROM orders|",
        "SELECT * FROM \"or|",
        "SELECT o.| FROM orders o",
        "SELECT cu| FROM orders o JOIN customers c ON c.id = o.customer_id",
        "SELECT 1;\n\nSEL|",
        "sel|",
        "SELECT * FROM orders WHERE |",
        "INSERT INTO orders (|",
    ];

    #[rstest]
    #[case(DatabaseType::SQLite)]
    #[case(DatabaseType::PostgreSQL)]
    #[case(DatabaseType::MySQL)]
    fn prepared_names_rank_exactly_like_matching_on_the_fly(#[case] db_type: DatabaseType) {
        let tables = mixed_schema();
        let columns = shop_columns();
        let names = CompletionNames::new(&tables);
        let schema = SchemaView {
            db_type: Some(&db_type),
            tables: &tables,
            columns: &columns,
        };
        for marked in REQUESTS {
            let (text, caret) = at_caret(marked);
            let Some(plan) = analyze(&text, caret, Some(&db_type)) else {
                continue;
            };
            assert_eq!(
                suggest_prepared(&plan, &schema, Some(&names)),
                suggest(&plan, &schema),
                "{marked}"
            );
        }
    }

    #[rstest]
    #[case(StatementSeparators::SemicolonsOnly, Some(DatabaseType::PostgreSQL))]
    #[case(
        StatementSeparators::SemicolonsAndBlankLines,
        Some(DatabaseType::PostgreSQL)
    )]
    #[case(
        StatementSeparators::SemicolonsAndBlankLines,
        Some(DatabaseType::MongoDB)
    )]
    #[case(StatementSeparators::SemicolonsAndBlankLines, None)]
    fn an_index_built_once_analyses_like_the_text(
        #[case] separators: StatementSeparators,
        #[case] db_type: Option<DatabaseType>,
    ) {
        let text = "SELECT 1;\n\nSELECT * FROM orders o\nWHERE o.\n\n-- note\nselect c. from customers c; 'é'";
        let index = StatementIndex::new(text, completion_split(separators, db_type.as_ref()));
        for caret in 0..=text.len() + 1 {
            assert_eq!(
                analyze_indexed(&index, caret, db_type.as_ref()),
                analyze_with(text, caret, db_type.as_ref(), separators),
                "caret {caret}"
            );
        }
    }

    /// A large schema: one completion request (what runs off the UI thread
    /// on a keystroke) stays well within a frame budget in release builds;
    /// the bound here is generous for unoptimized test builds.
    #[test]
    fn a_large_schema_completes_quickly() {
        let tables: Vec<TableEntry> = (0..5_000)
            .map(|i| {
                entry(
                    Some(["public", "sales", "audit"][i % 3]),
                    &format!("table_{i:05}_events"),
                    TableKind::Table,
                )
            })
            .collect();
        let columns = HashMap::new();
        let schema = SchemaView {
            db_type: Some(&DatabaseType::PostgreSQL),
            tables: &tables,
            columns: &columns,
        };
        let started = std::time::Instant::now();
        let names = CompletionNames::new(&tables);
        let prepared_in = started.elapsed();
        let text = "SELECT 1;\n".repeat(2_000) + "SELECT * FROM tab";
        let started = std::time::Instant::now();
        let plan = analyze(&text, text.len(), Some(&DatabaseType::PostgreSQL)).expect("a plan");
        let candidates = suggest_prepared(&plan, &schema, Some(&names));
        let requested_in = started.elapsed();
        assert_eq!(candidates.len(), MAX_CANDIDATES);
        assert!(
            prepared_in < std::time::Duration::from_secs(2)
                && requested_in < std::time::Duration::from_secs(2),
            "preparing took {prepared_in:?}, a request {requested_in:?}"
        );
    }

    // ---- tokenizer ----

    #[test]
    fn tokenizer_recognises_literals_and_comments() {
        let src = "SELECT 'a;b', \"Q\"\"x\", 12.5 -- tail\n/* b */ $1 $$x;y$$";
        let kinds: Vec<_> = tokenize(src, false).iter().map(|t| t.kind).collect();
        use TokenKind::*;
        assert_eq!(
            kinds,
            vec![
                Word, String, Punct, Quoted, Punct, Number, Comment, Comment, Param, String
            ]
        );
    }

    #[rstest]
    #[case("'abc", false)]
    #[case("'abc'", true)]
    #[case("\"abc", false)]
    #[case("/* open", false)]
    #[case("/* done */", true)]
    #[case("$tag$ body", false)]
    #[case("$tag$ body $tag$", true)]
    fn tokenizer_tracks_termination(#[case] src: &str, #[case] closed: bool) {
        let tokens = tokenize(src, false);
        assert_eq!(tokens.len(), 1, "{tokens:?}");
        assert_eq!(tokens[0].closed, closed);
        assert_eq!(tokens[0].end, src.len());
    }

    #[test]
    fn mysql_backslash_escapes_only_when_asked() {
        let src = r"'it\'s' x";
        assert_eq!(tokenize(src, true).len(), 2);
        assert_eq!(tokenize(src, false).len(), 3);
    }

    #[test]
    fn leading_keyword_skips_comments() {
        assert_eq!(
            leading_keyword("  -- c\n explain select 1").as_deref(),
            Some("EXPLAIN")
        );
        assert_eq!(
            leading_keyword("/* x */ select 1").as_deref(),
            Some("SELECT")
        );
        assert_eq!(leading_keyword("(select 1)"), None);
        assert_eq!(leading_keyword(""), None);
    }

    #[rstest]
    #[case("a", true)]
    #[case("_", true)]
    #[case("7", true)]
    #[case(".", true)]
    #[case("\"", true)]
    #[case(" ", false)]
    #[case("(", false)]
    #[case("ab", false)]
    #[case("", false)]
    fn trigger_characters(#[case] typed: &str, #[case] expected: bool) {
        assert_eq!(is_trigger(typed), expected);
    }

    // ---- scope ----

    #[test]
    fn scope_is_the_statement_under_the_caret() {
        let text = "SELECT 1;\nSELECT * FROM orders;\nSELECT 3";
        let caret = text.find("orders").unwrap() + 2;
        assert_eq!(
            &text[statement_scope(text, caret, SplitConfig::SEMICOLONS)],
            "SELECT * FROM orders"
        );
        // right after a `;` a fresh statement starts
        let after = text.find(';').unwrap() + 1;
        assert_eq!(
            statement_scope(text, after, SplitConfig::SEMICOLONS),
            after..after
        );
        // trailing whitespace stays in the statement being typed
        let open = "SELECT * FROM ";
        assert_eq!(
            &open[statement_scope(open, open.len(), SplitConfig::SEMICOLONS)],
            open
        );
    }

    // ---- contexts ----

    #[rstest]
    #[case("|", Context::StatementStart)]
    #[case("  \n-- c\n|", Context::StatementStart)]
    #[case("SELECT 1; |", Context::StatementStart)]
    #[case("SELECT * FROM |", Context::Tables)]
    #[case("select * from ord|", Context::Tables)]
    #[case("SELECT * FROM orders o JOIN |", Context::Tables)]
    #[case("SELECT * FROM orders, |", Context::Tables)]
    #[case("UPDATE |", Context::Tables)]
    #[case("INSERT INTO |", Context::Tables)]
    #[case("DELETE FROM |", Context::Tables)]
    #[case("DROP TABLE IF EXISTS |", Context::Tables)]
    #[case("SELECT * FROM public.|", Context::SchemaTables("public".into()))]
    #[case("SELECT |", Context::Columns)]
    #[case("SELECT id, na| FROM customers", Context::Columns)]
    #[case("SELECT * FROM orders WHERE |", Context::Columns)]
    #[case("SELECT * FROM orders o JOIN customers c ON |", Context::Columns)]
    #[case("SELECT * FROM orders GROUP BY |", Context::Columns)]
    #[case("SELECT * FROM orders ORDER BY |", Context::Columns)]
    #[case("UPDATE orders SET |", Context::Columns)]
    #[case("SELECT count(|", Context::Columns)]
    #[case("SELECT * FROM (SELECT | FROM orders)", Context::Columns)]
    #[case("SELECT o.| FROM orders o", Context::Qualified(vec!["o".into()]))]
    #[case("SELECT * FROM orders o WHERE o.st|", Context::Qualified(vec!["o".into()]))]
    #[case("SELECT * FROM a JOIN b ON a.id = b.|", Context::Qualified(vec!["b".into()]))]
    #[case("SELECT public.orders.| FROM orders", Context::Qualified(vec!["public".into(), "orders".into()]))]
    #[case("INSERT INTO orders (|", Context::InsertColumns)]
    #[case("INSERT INTO orders (id, |", Context::InsertColumns)]
    #[case("SELECT * FROM orders |", Context::Clause)]
    #[case("SELECT * FROM orders o |", Context::Clause)]
    #[case("SELECT * FROM orders AS o |", Context::Clause)]
    #[case("SELECT * FROM orders LEFT |", Context::Clause)]
    #[case("EXPLAIN |", Context::Clause)]
    #[case("SELECT 1 UNION |", Context::Clause)]
    fn contexts(#[case] marked: &str, #[case] expected: Context) {
        assert_eq!(plan(marked).context, expected, "{marked}");
    }

    #[rstest]
    #[case("SELECT 'abc|")]
    #[case("SELECT 'abc|def'")]
    #[case("SELECT 1 -- note|")]
    #[case("SELECT /* x|")]
    #[case("SELECT * FROM t WHERE a = 12|")]
    #[case("SELECT $1|")]
    #[case("SELECT $$ body|")]
    // Gaps hold no statement: the scope is empty but the caret is still in a literal
    #[case("-- note|")]
    #[case("/* x|")]
    #[case("SELECT 1;\n-- note|")]
    #[case("SELECT 1;\n/* x| */")]
    #[case("SELECT 1; 'abc|")]
    fn nothing_inside_literals_and_comments(#[case] marked: &str) {
        let (text, caret) = at_caret(marked);
        assert_eq!(
            analyze(&text, caret, Some(&DatabaseType::SQLite)),
            None,
            "{marked}"
        );
    }

    #[test]
    fn after_a_closed_string_completion_resumes() {
        let plan = plan("SELECT 'a' |");
        assert_eq!(plan.context, Context::Columns);
    }

    #[test]
    fn prefix_and_replace_range_cover_the_word() {
        let (text, caret) = at_caret("SELECT cust| FROM customers");
        let plan = analyze(&text, caret, None).unwrap();
        assert_eq!(plan.prefix, "cust");
        assert_eq!(&text[plan.replace.clone()], "cust");
        assert_eq!(plan.existing, "cust");

        // caret inside a word: the whole word is replaced, the prefix is what precedes the caret
        let (text, caret) = at_caret("SELECT cu|st FROM customers");
        let plan = analyze(&text, caret, None).unwrap();
        assert_eq!(plan.prefix, "cu");
        assert_eq!(&text[plan.replace.clone()], "cust");
    }

    #[test]
    fn replace_range_is_in_buffer_coordinates_with_multibyte_text() {
        let (text, caret) = at_caret("SELECT 'é';\nSELECT ord|");
        let plan = analyze(&text, caret, None).unwrap();
        assert_eq!(&text[plan.replace.clone()], "ord");
        assert_eq!(plan.replace.end, caret);
    }

    #[test]
    fn typed_quote_is_replaced_with_the_quoted_name() {
        let (text, caret) = at_caret("SELECT * FROM \"ord|\"");
        let plan = analyze(&text, caret, Some(&DatabaseType::PostgreSQL)).unwrap();
        assert_eq!(plan.context, Context::Tables);
        assert_eq!(plan.quote, Some('"'));
        assert_eq!(plan.prefix, "ord");
        assert_eq!(&text[plan.replace.clone()], "\"ord\"");
    }

    #[test]
    fn unterminated_quote_replaces_up_to_the_caret() {
        let (text, caret) = at_caret("SELECT * FROM \"ord|");
        let plan = analyze(&text, caret, Some(&DatabaseType::PostgreSQL)).unwrap();
        assert_eq!(&text[plan.replace.clone()], "\"ord");
    }

    // ---- references ----

    #[test]
    fn references_and_aliases_are_collected_from_the_whole_statement() {
        let plan = plan(
            "SELECT o.|, c.name FROM orders o JOIN customers AS c ON c.id = o.customer_id, products",
        );
        let refs: Vec<_> = plan
            .tables
            .iter()
            .map(|r| (r.path.join("."), r.alias.clone()))
            .collect();
        assert_eq!(
            refs,
            vec![
                ("orders".into(), Some("o".into())),
                ("customers".into(), Some("c".into())),
                ("products".into(), None),
            ]
        );
    }

    #[test]
    fn schema_qualified_references() {
        let plan = plan("SELECT | FROM sales.orders o");
        assert_eq!(plan.tables[0].path, vec!["sales", "orders"]);
        assert_eq!(plan.tables[0].alias.as_deref(), Some("o"));
    }

    #[test]
    fn other_statements_are_ignored() {
        let plan = plan("SELECT * FROM customers;\nSELECT | FROM orders");
        assert_eq!(plan.tables.len(), 1);
        assert_eq!(plan.tables[0].path, vec!["orders"]);
    }

    #[test]
    fn derived_tables_and_ctes() {
        let plan = plan(
            "WITH recent AS (SELECT * FROM orders), big(n) AS MATERIALIZED (SELECT 1) SELECT | FROM recent r JOIN (SELECT 1) sub ON true",
        );
        assert_eq!(plan.ctes, vec!["recent", "big"]);
        let handles: Vec<_> = plan.tables.iter().map(|r| r.alias.clone()).collect();
        assert!(handles.contains(&Some("r".to_string())), "{handles:?}");
        assert!(handles.contains(&Some("sub".to_string())), "{handles:?}");
        let derived = plan
            .tables
            .iter()
            .find(|r| r.alias.as_deref() == Some("sub"))
            .unwrap();
        assert!(derived.path.is_empty());
    }

    #[test]
    fn extract_from_is_not_a_table_position() {
        let plan = plan("SELECT extract(year FROM |) FROM orders");
        assert_eq!(plan.context, Context::Columns);
    }

    #[test]
    fn resolve_prefers_default_schema_and_exact_case() {
        let tables = vec![
            entry(Some("archive"), "orders", TableKind::Table),
            entry(Some("public"), "orders", TableKind::Table),
            entry(Some("public"), "Orders", TableKind::Table),
        ];
        let pg = DatabaseType::PostgreSQL;
        let found = resolve_table(&tables, Some(&pg), &["orders".into()]).unwrap();
        assert_eq!(found.qualified_name(), "public.orders");
        let found = resolve_table(&tables, Some(&pg), &["ORDERS".into()]).unwrap();
        assert!(found.name.eq_ignore_ascii_case("orders"));
        let found =
            resolve_table(&tables, Some(&pg), &["archive".into(), "orders".into()]).unwrap();
        assert_eq!(found.qualified_name(), "archive.orders");
        assert!(resolve_table(&tables, Some(&pg), &["missing".into()]).is_none());
    }

    #[test]
    fn needed_tables_follow_the_context() {
        let tables = shop();
        let plan = plan("SELECT o.| FROM orders o JOIN customers c ON true");
        let needed = plan.needed_tables(&tables, Some(&DatabaseType::SQLite));
        assert_eq!(
            needed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["orders"]
        );

        let plan = self::plan("SELECT | FROM orders o JOIN customers c ON true");
        let needed = plan.needed_tables(&tables, Some(&DatabaseType::SQLite));
        assert_eq!(
            needed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["orders", "customers"]
        );

        let plan = self::plan("SELECT * FROM |");
        assert!(
            plan.needed_tables(&tables, Some(&DatabaseType::SQLite))
                .is_empty()
        );
    }

    // ---- ranking ----

    #[rstest]
    #[case("ord", "orders", Tier::Prefix)]
    #[case("orders", "orders", Tier::Exact)]
    #[case("ORD", "orders", Tier::Prefix)]
    #[case("items", "order_items", Tier::Boundary)]
    #[case("oi", "order_items", Tier::Subsequence)]
    #[case("cid", "customer_id", Tier::Subsequence)]
    #[case("Item", "orderItems", Tier::Boundary)]
    fn match_tiers(#[case] prefix: &str, #[case] label: &str, #[case] tier: Tier) {
        assert_eq!(match_label(prefix, label).map(|m| m.0), Some(tier));
    }

    #[test]
    fn non_matches_are_dropped() {
        assert_eq!(match_label("zzz", "orders"), None);
        assert_eq!(match_label("sro", "orders"), None);
    }

    #[test]
    fn tables_rank_prefix_then_boundary_then_subsequence() {
        let tables = vec![
            entry(None, "stock_items", TableKind::Table), // word boundary for "item"
            entry(None, "it_emails", TableKind::Table),   // subsequence for "item"
            entry(None, "items", TableKind::Table),       // prefix
            entry(None, "items_archive", TableKind::Table), // prefix, longer
        ];
        let (text, caret) = at_caret("SELECT * FROM item|");
        let plan = analyze(&text, caret, Some(&DatabaseType::SQLite)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::SQLite),
                tables: &tables,
                columns: &columns,
            },
        );
        assert_eq!(
            labels(&found),
            vec!["items", "items_archive", "stock_items", "it_emails"]
        );
    }

    #[test]
    fn tables_context_offers_tables_views_and_system_last() {
        let found = suggestions("SELECT * FROM |");
        assert_eq!(
            labels(&found),
            vec![
                "customers",
                "order_items",
                "orders",
                "products",
                "revenue_by_month",
                "sqlite_sequence"
            ]
        );
        assert_eq!(found[4].kind, CandidateKind::View);
        assert_eq!(found[4].detail, "view");
    }

    #[test]
    fn tables_context_includes_ctes_first() {
        let found = suggestions("WITH recent AS (SELECT 1) SELECT * FROM |");
        assert_eq!(found[0].label, "recent");
        assert_eq!(found[0].kind, CandidateKind::Cte);
    }

    #[test]
    fn alias_resolves_to_the_table_columns() {
        let found = suggestions("SELECT * FROM orders o JOIN customers c ON c.id = o.|");
        assert_eq!(
            labels(&found),
            vec!["*", "id", "customer_id", "status", "total"]
        );
        assert_eq!(found[1].detail, "INTEGER");
        let found = suggestions("SELECT c.na| FROM orders o JOIN customers c ON true");
        assert_eq!(labels(&found), vec!["name"]);
    }

    #[test]
    fn unqualified_columns_come_from_every_referenced_table() {
        let found = suggestions("SELECT | FROM orders o JOIN customers c ON true");
        let columns: Vec<_> = found
            .iter()
            .filter(|c| c.kind == CandidateKind::Column)
            .map(|c| (c.label.as_str(), c.insert.as_str()))
            .collect();
        // `id` exists in both tables: inserted qualified; the rest plain
        assert!(columns.contains(&("id", "o.id")), "{columns:?}");
        assert!(columns.contains(&("id", "c.id")), "{columns:?}");
        assert!(columns.contains(&("email", "email")), "{columns:?}");
        // columns first, then aliases, then functions, then keywords
        let kinds: Vec<_> = found.iter().map(|c| c.kind).collect();
        let first_alias = kinds
            .iter()
            .position(|k| *k == CandidateKind::Alias)
            .unwrap();
        let last_column = kinds
            .iter()
            .rposition(|k| *k == CandidateKind::Column)
            .unwrap();
        let first_function = kinds
            .iter()
            .position(|k| *k == CandidateKind::Function)
            .unwrap();
        let first_keyword = kinds
            .iter()
            .position(|k| *k == CandidateKind::Keyword)
            .unwrap();
        assert!(
            last_column < first_alias
                && first_alias < first_function
                && first_function < first_keyword
        );
    }

    #[test]
    fn typed_prefix_beats_kind_priority() {
        // `se` matches the keyword SELECT by prefix, the column `status` only by prefix too; with
        // "sel" only the keyword matches
        let found = suggestions("SELECT * FROM orders WHERE sel|");
        assert_eq!(found[0].label, "SELECT");
        // lower-case typing → lower-case keyword
        assert_eq!(found[0].insert, "select");
        let found = suggestions("SELECT * FROM orders WHERE st|");
        assert_eq!(found[0].label, "status");
    }

    #[test]
    fn keyword_case_follows_the_typed_prefix() {
        let found = suggestions("SELECT * FROM orders WHERE x = 1 GRO|");
        assert_eq!(found[0].insert, "GROUP BY");
        let found = suggestions("SELECT * FROM orders WHERE x = 1 gro|");
        assert_eq!(found[0].insert, "group by");
    }

    #[test]
    fn a_fully_typed_word_is_not_offered_again() {
        // only `orders` itself matches "orders": nothing to offer, so the menu closes
        let found = suggestions("SELECT * FROM orders|");
        assert!(found.is_empty(), "{found:?}");
        // "order" still offers the longer names
        let found = suggestions("SELECT * FROM order|");
        assert_eq!(labels(&found), vec!["orders", "order_items"]);
    }

    #[test]
    fn clause_context_offers_continuations() {
        let found = suggestions("SELECT * FROM orders |");
        assert!(
            labels(&found).starts_with(&["JOIN", "INNER JOIN"]),
            "{:?}",
            labels(&found)
        );
        let found = suggestions("SELECT * FROM orders wh|");
        assert_eq!(found[0].label, "WHERE");
    }

    #[test]
    fn statement_start_offers_statement_keywords() {
        let found = suggestions("sel|");
        assert_eq!(found[0].insert, "select");
        assert!(found.iter().all(|c| c.kind == CandidateKind::Keyword));
        let found = suggestions("|");
        assert_eq!(found[0].label, "SELECT");
        assert!(labels(&found).contains(&"PRAGMA"));
    }

    #[test]
    fn insert_columns_come_from_the_target_table() {
        let found = suggestions("INSERT INTO orders (|");
        assert_eq!(labels(&found), vec!["id", "customer_id", "status", "total"]);
    }

    #[test]
    fn unknown_alias_offers_nothing() {
        assert!(suggestions("SELECT x.| FROM orders o").is_empty());
    }

    // ---- quoting ----

    #[test]
    fn insert_text_is_quoted_where_needed() {
        let tables = vec![
            entry(Some("public"), "Orders", TableKind::Table),
            entry(Some("sales"), "events", TableKind::Table),
            entry(Some("public"), "users", TableKind::Table),
        ];
        let pg = DatabaseType::PostgreSQL;
        let (text, caret) = at_caret("SELECT * FROM |");
        let plan = analyze(&text, caret, Some(&pg)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&pg),
                tables: &tables,
                columns: &columns,
            },
        );
        let pairs: Vec<_> = found
            .iter()
            .map(|c| (c.label.as_str(), c.insert.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("Orders", "\"Orders\""),
                ("sales.events", "sales.events"),
                ("users", "users"),
            ]
        );
    }

    #[test]
    fn schema_qualifier_lists_that_schemas_tables() {
        let tables = vec![
            entry(Some("public"), "users", TableKind::Table),
            entry(Some("sales"), "Events", TableKind::Table),
            entry(Some("sales"), "orders", TableKind::Table),
        ];
        let pg = DatabaseType::PostgreSQL;
        let (text, caret) = at_caret("SELECT * FROM sales.|");
        let plan = analyze(&text, caret, Some(&pg)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&pg),
                tables: &tables,
                columns: &columns,
            },
        );
        let pairs: Vec<_> = found
            .iter()
            .map(|c| (c.label.as_str(), c.insert.as_str()))
            .collect();
        assert_eq!(pairs, vec![("Events", "\"Events\""), ("orders", "orders")]);

        // in an expression, `sales.` also lists the schema's tables
        let (text, caret) = at_caret("SELECT sales.| FROM sales.orders");
        let plan = analyze(&text, caret, Some(&pg)).unwrap();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&pg),
                tables: &tables,
                columns: &columns,
            },
        );
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn typed_quote_keeps_the_quote_style() {
        let tables = vec![entry(None, "order_items", TableKind::Table)];
        let (text, caret) = at_caret("SELECT * FROM \"ord|");
        let plan = analyze(&text, caret, Some(&DatabaseType::PostgreSQL)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::PostgreSQL),
                tables: &tables,
                columns: &columns,
            },
        );
        assert_eq!(found[0].insert, "\"order_items\"");
        assert_eq!(&text[plan.replace.clone()], "\"ord");
    }

    #[test]
    fn mysql_quotes_with_backticks() {
        let tables = vec![entry(None, "order items", TableKind::Table)];
        let (text, caret) = at_caret("SELECT * FROM ord|");
        let plan = analyze(&text, caret, Some(&DatabaseType::MySQL)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::MySQL),
                tables: &tables,
                columns: &columns,
            },
        );
        assert_eq!(found[0].insert, "`order items`");
    }

    #[test]
    fn matched_prefix_length_is_on_a_char_boundary() {
        let tables = vec![
            entry(None, "élèves", TableKind::Table),
            entry(None, "x_élèves", TableKind::Table),
        ];
        let (text, caret) = at_caret("SELECT * FROM él|");
        let plan = analyze(&text, caret, Some(&DatabaseType::SQLite)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::SQLite),
                tables: &tables,
                columns: &columns,
            },
        );
        for candidate in &found {
            assert!(
                candidate.label.is_char_boundary(candidate.matched),
                "{candidate:?}"
            );
        }
        assert_eq!(found[0].label, "élèves");
        assert_eq!(found[0].matched, "él".len());
        // the word-boundary match highlights nothing
        assert_eq!(found[1].matched, 0);
    }

    #[test]
    fn results_are_capped() {
        let tables: Vec<_> = (0..500)
            .map(|n| entry(None, &format!("table_{n:03}"), TableKind::Table))
            .collect();
        let (text, caret) = at_caret("SELECT * FROM |");
        let plan = analyze(&text, caret, Some(&DatabaseType::SQLite)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::SQLite),
                tables: &tables,
                columns: &columns,
            },
        );
        assert_eq!(found.len(), MAX_CANDIDATES);
        assert_eq!(found[0].label, "table_000");
    }

    #[test]
    fn dialects_pick_their_function_sets() {
        let pg = Dialect::of(Some(&DatabaseType::PostgreSQL));
        assert!(functions(pg).any(|(name, _)| *name == "DATE_TRUNC"));
        let sqlite = Dialect::of(Some(&DatabaseType::SQLite));
        assert!(functions(sqlite).any(|(name, _)| *name == "STRFTIME"));
        assert!(!functions(sqlite).any(|(name, _)| *name == "DATE_TRUNC"));
        assert_eq!(
            Dialect::of(Some(&DatabaseType::ClickHouse)),
            Dialect::Generic
        );
        assert_eq!(Dialect::of(None), Dialect::Generic);
    }

    #[test]
    fn mongo_db_dot_lists_collections() {
        let tables = vec![TableEntry {
            schema: None,
            name: "users".into(),
            kind: TableKind::Collection,
            owner: None,
        }];
        let (text, caret) = at_caret("db.|");
        let plan = analyze(&text, caret, Some(&DatabaseType::MongoDB)).unwrap();
        let columns = HashMap::new();
        let found = suggest(
            &plan,
            &SchemaView {
                db_type: Some(&DatabaseType::MongoDB),
                tables: &tables,
                columns: &columns,
            },
        );
        assert_eq!(labels(&found), vec!["users"]);
    }

    // ----- Blank-line boundaries -----

    fn smart_scope(text: &str, caret: usize) -> &str {
        let config = SplitConfig::new(
            StatementSeparators::SemicolonsAndBlankLines,
            Some(&DatabaseType::PostgreSQL),
            false,
        );
        &text[statement_scope(text, caret, config)]
    }

    #[test]
    fn a_blank_line_between_queries_scopes_completion_to_the_query_at_the_caret() {
        let text = "select * from orders o\n\nselect * from customers c where c.id = 1";
        // In the second query: its own text only
        assert_eq!(
            smart_scope(text, text.len()),
            "select * from customers c where c.id = 1"
        );
        // In the first
        assert_eq!(smart_scope(text, 10), "select * from orders o");
        // On the blank line after the first query a fresh statement starts:
        // its alias `o` is not offered to what is typed next
        let blank = text.find("\n\n").unwrap() + 1;
        assert_eq!(smart_scope(text, blank), "");
        assert_eq!(smart_scope(text, blank - 1), "select * from orders o");
    }

    #[test]
    fn an_unfinished_second_query_leaves_the_region_whole() {
        // While the second query does not parse, nothing is cut: completion
        // sees the block, like Run does
        let text = "select 1\n\nselect * from";
        assert_eq!(smart_scope(text, text.len()), text);
        let text = "select * from orders o\n\nselect * from customers c where c.";
        assert_eq!(smart_scope(text, text.len()), text);
    }
}
