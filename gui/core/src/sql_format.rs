//! A conservative SQL formatter for the query editor's "Format SQL".
//!
//! It only changes whitespace and the case of reserved keywords, never
//! strings, quoted identifiers, comments or identifiers, so the statement
//! means the same afterwards. Supported statements: `SELECT`, `WITH`,
//! `INSERT`, `REPLACE`, `UPDATE`, `DELETE` and `VALUES`; anything else is
//! left alone (`None`).
//!
//! Layout: every top-level clause on its own line (`FROM`, `WHERE`,
//! `GROUP BY`, `ORDER BY`, `HAVING`, `LIMIT`, `UNION`, joins, `SET`,
//! `VALUES`), select and `SET` lists one item per line when they have more
//! than one, `AND`/`OR` of `WHERE`/`ON`/`HAVING` indented under their clause,
//! subqueries and CTE bodies indented inside their parentheses.

use crate::completion::{Token, TokenKind, tokenize};
use crate::statements::{Boundary, SplitConfig, StatementIndex};
use dbcrust::sql_buffer;

const INDENT: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Quoted,
    Literal,
    LineComment,
    BlockComment,
    /// A single punctuation character.
    Punct(char),
    /// A comparison operator (`=`, `<>`, `<=`, …), always spaced.
    Operator,
}

#[derive(Debug, Clone)]
struct Item {
    kind: Kind,
    text: String,
    /// Whitespace separated this token from the previous one.
    space_before: bool,
    /// ... and it contained a line break.
    newline_before: bool,
}

impl Item {
    fn upper(&self) -> String {
        if self.kind == Kind::Word {
            self.text.to_ascii_uppercase()
        } else {
            String::new()
        }
    }

    fn is_punct(&self, c: char) -> bool {
        self.kind == Kind::Punct(c)
    }

    fn is_comment(&self) -> bool {
        matches!(self.kind, Kind::LineComment | Kind::BlockComment)
    }
}

fn items(sql: &str, backslash_escapes: bool) -> Vec<Item> {
    let tokens: Vec<Token> = tokenize(sql, backslash_escapes);
    let mut items: Vec<Item> = Vec::with_capacity(tokens.len());
    let mut previous_end = 0;
    let mut index = 0;
    // Punctuation up to here belongs to a run already judged "not a comparison"
    let mut plain_until = 0;
    while index < tokens.len() {
        let token = tokens[index];
        let gap = &sql[previous_end..token.start];
        let space_before = !gap.is_empty();
        let newline_before = gap.contains('\n');
        let text = &sql[token.start..token.end];
        index += 1;
        let (kind, text, end) = match token.kind {
            TokenKind::Word => (Kind::Word, text.to_string(), token.end),
            TokenKind::Quoted => (Kind::Quoted, text.to_string(), token.end),
            TokenKind::String | TokenKind::Number | TokenKind::Param => {
                (Kind::Literal, text.to_string(), token.end)
            }
            TokenKind::Comment if text.starts_with("--") => {
                (Kind::LineComment, text.trim_end().to_string(), token.end)
            }
            TokenKind::Comment => (Kind::BlockComment, text.to_string(), token.end),
            TokenKind::Punct => {
                let c = text.chars().next().unwrap_or(' ');
                // A run of adjacent punctuation that is exactly a comparison
                // operator becomes one spaced operator; any other run keeps
                // its original spacing (`::`, `->>`, `||`, …)
                let mut run = String::from(c);
                let mut end = token.end;
                let mut next = index;
                while let Some(following) = tokens.get(next) {
                    let adjacent = following.kind == TokenKind::Punct && following.start == end;
                    let c = sql[following.start..following.end]
                        .chars()
                        .next()
                        .unwrap_or(' ');
                    if !adjacent || matches!(c, '(' | ')' | ',' | ';' | '.') {
                        break;
                    }
                    run.push(c);
                    end = following.end;
                    next += 1;
                }
                let separate = matches!(c, '(' | ')' | ',' | ';' | '.');
                let continues_run = token.start < plain_until;
                if !separate
                    && !continues_run
                    && matches!(
                        run.as_str(),
                        "=" | "<>" | "!=" | "<=" | ">=" | "<" | ">" | "=="
                    )
                {
                    index = next;
                    (Kind::Operator, run, end)
                } else {
                    if !separate && !continues_run {
                        plain_until = end;
                    }
                    (Kind::Punct(c), c.to_string(), token.end)
                }
            }
        };
        previous_end = end;
        items.push(Item {
            kind,
            text,
            space_before,
            newline_before,
        });
    }
    items
}

/// Reserved words that are upper-cased. Deliberately short: words that can
/// double as identifiers in some engine (`name`, `date`, `count`…) are left
/// as typed.
fn is_keyword(up: &str) -> bool {
    matches!(
        up,
        "SELECT"
            | "FROM"
            | "WHERE"
            | "GROUP"
            | "ORDER"
            | "BY"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "ALL"
            | "DISTINCT"
            | "AS"
            | "AND"
            | "OR"
            | "NOT"
            | "IN"
            | "IS"
            | "NULL"
            | "LIKE"
            | "ILIKE"
            | "BETWEEN"
            | "EXISTS"
            | "CASE"
            | "WHEN"
            | "THEN"
            | "ELSE"
            | "END"
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
            | "INSERT"
            | "INTO"
            | "VALUES"
            | "UPDATE"
            | "SET"
            | "DELETE"
            | "WITH"
            | "RECURSIVE"
            | "ASC"
            | "DESC"
            | "OVER"
            | "PARTITION"
            | "RETURNING"
            | "DEFAULT"
            | "TRUE"
            | "FALSE"
            | "REPLACE"
    )
}

fn is_join_modifier(up: &str) -> bool {
    matches!(
        up,
        "INNER" | "LEFT" | "RIGHT" | "FULL" | "OUTER" | "CROSS" | "NATURAL"
    )
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
    Having,
    Set,
    Other,
}

#[derive(Debug, Clone)]
struct Frame {
    /// Indent level of the clause keywords of this frame.
    base: usize,
    /// Opened by `( SELECT …`: its content sits on its own lines.
    block: bool,
    clause: Clause,
    /// Items of the current select/`SET` list go on separate lines.
    list: bool,
    /// The next token starts the (broken-out) list.
    list_pending: bool,
    between: bool,
    case_depth: usize,
    /// `EXTRACT(x FROM y)` and friends: `FROM` is not a clause there.
    inline_from: bool,
}

impl Frame {
    fn new(base: usize, block: bool) -> Self {
        Self {
            base,
            block,
            clause: Clause::None,
            list: false,
            list_pending: false,
            between: false,
            case_depth: 0,
            inline_from: false,
        }
    }

    /// Clause keywords break lines only at the top level and inside
    /// subquery blocks, not inside function calls or `OVER (…)`.
    fn breaks(&self, root: bool) -> bool {
        root || self.block
    }
}

struct Formatter<'a> {
    items: &'a [Item],
    out: String,
    frames: Vec<Frame>,
    at_line_start: bool,
    break_before: Option<usize>,
    /// What was written last, for spacing decisions.
    last: Option<Item>,
    last_up: String,
    prev_up: String,
}

impl<'a> Formatter<'a> {
    fn new(items: &'a [Item]) -> Self {
        Self {
            items,
            out: String::new(),
            frames: vec![Frame::new(0, false)],
            at_line_start: true,
            break_before: None,
            last: None,
            last_up: String::new(),
            prev_up: String::new(),
        }
    }

    fn frame(&mut self) -> &mut Frame {
        let last = self.frames.len() - 1;
        &mut self.frames[last]
    }

    fn is_root(&self) -> bool {
        self.frames.len() == 1
    }

    /// Start a new line at `level` (re-indenting an empty line instead of
    /// adding a blank one).
    fn newline(&mut self, level: usize) {
        if self.out.is_empty() {
            return;
        }
        let trimmed = self.out.trim_end_matches(' ').len();
        self.out.truncate(trimmed);
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        for _ in 0..level {
            self.out.push_str(INDENT);
        }
        self.at_line_start = true;
    }

    /// Level of continuation lines inside the current clause.
    fn continuation(&mut self) -> usize {
        let frame = self.frame();
        match frame.clause {
            Clause::Select | Clause::Set if frame.list => frame.base + 1,
            Clause::Where | Clause::On | Clause::Having => frame.base + 1,
            _ => frame.base,
        }
    }

    fn space_needed(&self, item: &Item) -> bool {
        let Some(last) = &self.last else {
            return false;
        };
        if last.is_punct('(') || item.is_punct(')') || item.is_punct(',') || item.is_punct(';') {
            return false;
        }
        if last.is_punct('.') || item.is_punct('.') {
            return false;
        }
        if last.is_punct(',') {
            return true;
        }
        if item.is_punct('(') && is_keyword(&self.last_up) {
            return true;
        }
        if last.kind == Kind::Operator || item.kind == Kind::Operator {
            return true;
        }
        item.space_before
    }

    fn write(&mut self, item: &Item, text: &str) {
        if let Some(level) = self.break_before.take() {
            self.newline(level);
        }
        if !self.at_line_start && self.space_needed(item) {
            self.out.push(' ');
        }
        self.out.push_str(text);
        self.at_line_start = false;
        self.last = Some(item.clone());
        self.last_up = item.upper();
    }

    fn peek_word(&self, from: usize) -> Option<(usize, String)> {
        self.items[from..]
            .iter()
            .enumerate()
            .find(|(_, item)| !item.is_comment())
            .map(|(offset, item)| (from + offset, item.upper()))
    }

    /// Does the list of the clause starting at `from` have a top-level comma?
    fn list_has_comma(&self, from: usize, stops: &[&str]) -> bool {
        let mut depth = 0usize;
        for item in &self.items[from + 1..] {
            match item.kind {
                Kind::Punct('(') => depth += 1,
                Kind::Punct(')') => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                Kind::Punct(',') if depth == 0 => return true,
                Kind::Punct(';') if depth == 0 => return false,
                Kind::Word if depth == 0 && stops.contains(&item.upper().as_str()) => return false,
                _ => {}
            }
        }
        false
    }

    /// Is the word at `from` the start of `[LEFT|RIGHT|…] [OUTER] JOIN`?
    fn starts_join(&self, from: usize) -> bool {
        let mut at = from;
        while let Some(item) = self.items.get(at) {
            let up = item.upper();
            if is_join_modifier(&up) {
                at += 1;
            } else {
                return up == "JOIN" && at != from;
            }
        }
        false
    }

    fn comment(&mut self, item: &Item) {
        // A comment on the line of the previous token stays there, even when
        // a list break was queued after that token's comma
        let pending = self.break_before.take();
        let level = pending.unwrap_or_else(|| self.continuation());
        if item.newline_before && !self.at_line_start {
            self.break_before = Some(level);
        }
        self.write(item, &item.text);
        self.break_before = if item.kind == Kind::LineComment {
            Some(level)
        } else {
            pending
        };
    }

    /// Queued before the first token of a select/`SET` list.
    fn flush_list_break(&mut self) {
        if self.frame().list_pending {
            let level = self.frame().base + 1;
            self.frame().list_pending = false;
            self.break_before = Some(level);
        }
    }

    fn word(&mut self, index: usize, item: &Item) {
        let up = item.upper();
        let root = self.is_root();
        let frame_base = self.frame().base;
        let breaks = self.frame().breaks(root);
        let prev = self.prev_up.clone();
        let started = !self.out.is_empty();
        let text = if is_keyword(&up) {
            up.clone()
        } else {
            item.text.clone()
        };

        // A pending list break goes before the first item (after DISTINCT/ALL)
        if self.frame().list_pending {
            let keeps = matches!(up.as_str(), "DISTINCT" | "ALL")
                || (up == "ON" && self.last_up == "DISTINCT");
            if !keeps {
                let level = frame_base + 1;
                self.frame().list_pending = false;
                self.break_before = Some(level);
            }
        }

        let break_to = |this: &mut Self, level: usize| {
            if started {
                this.break_before = Some(level);
            }
        };

        match up.as_str() {
            "SELECT" => {
                if breaks {
                    break_to(self, frame_base);
                }
                let list = self.list_has_comma(
                    index,
                    &[
                        "FROM",
                        "WHERE",
                        "GROUP",
                        "ORDER",
                        "HAVING",
                        "LIMIT",
                        "UNION",
                        "INTERSECT",
                        "EXCEPT",
                        "INTO",
                    ],
                );
                let frame = self.frame();
                frame.clause = Clause::Select;
                frame.list = list;
                frame.list_pending = list;
            }
            "FROM" => {
                let inline = prev == "DELETE" || self.frame().inline_from || !breaks;
                if !inline {
                    break_to(self, frame_base);
                    self.frame().clause = Clause::From;
                    self.frame().list = false;
                }
            }
            "WHERE" if breaks => {
                break_to(self, frame_base);
                let frame = self.frame();
                frame.clause = Clause::Where;
                frame.list = false;
            }
            "GROUP" | "ORDER" if breaks => {
                break_to(self, frame_base);
                let frame = self.frame();
                frame.clause = Clause::Other;
                frame.list = false;
            }
            "HAVING" if breaks => {
                break_to(self, frame_base);
                let frame = self.frame();
                frame.clause = Clause::Having;
                frame.list = false;
            }
            "LIMIT" | "OFFSET" | "FETCH" | "RETURNING" if breaks => {
                break_to(self, frame_base);
                let frame = self.frame();
                frame.clause = Clause::Other;
                frame.list = false;
            }
            "UNION" | "INTERSECT" | "EXCEPT" if breaks => {
                break_to(self, frame_base);
                let frame = self.frame();
                frame.clause = Clause::None;
                frame.list = false;
            }
            "VALUES" if breaks => {
                break_to(self, frame_base);
                self.frame().clause = Clause::Other;
            }
            "SET" if breaks && prev != "UPDATE" => {
                break_to(self, frame_base);
                let list = self.list_has_comma(index, &["WHERE", "FROM", "RETURNING"]);
                let frame = self.frame();
                frame.clause = Clause::Set;
                frame.list = list;
                frame.list_pending = list;
            }
            "JOIN" if breaks => {
                if !is_join_modifier(&prev) {
                    break_to(self, frame_base);
                }
                self.frame().clause = Clause::Join;
                self.frame().list = false;
            }
            "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL" | "OUTER"
                if breaks && self.starts_join(index) =>
            {
                if !is_join_modifier(&prev) {
                    break_to(self, frame_base);
                }
            }
            "ON" => {
                if self.frame().clause == Clause::Join {
                    self.frame().clause = Clause::On;
                }
            }
            "AND" | "OR" => {
                let frame = self.frame();
                let logical = matches!(frame.clause, Clause::Where | Clause::On | Clause::Having)
                    && frame.case_depth == 0;
                if up == "AND" && frame.between {
                    frame.between = false;
                } else if logical {
                    let level = frame.base + 1;
                    break_to(self, level);
                }
            }
            "BETWEEN" => self.frame().between = true,
            "CASE" => self.frame().case_depth += 1,
            "END" => {
                let frame = self.frame();
                frame.case_depth = frame.case_depth.saturating_sub(1);
            }
            "WITH" if !started => self.frame().clause = Clause::With,
            "INSERT" | "DELETE" | "REPLACE" if !started => self.frame().clause = Clause::None,
            "UPDATE" if !started => self.frame().clause = Clause::Other,
            _ => {}
        }
        self.write(item, &text);
        self.prev_up = up;
    }

    fn open_paren(&mut self, index: usize, item: &Item) {
        let subquery = self
            .peek_word(index + 1)
            .is_some_and(|(_, up)| matches!(up.as_str(), "SELECT" | "WITH" | "VALUES"));
        let inline_from = matches!(
            self.prev_up.as_str(),
            "EXTRACT" | "TRIM" | "SUBSTRING" | "OVERLAY" | "POSITION"
        );
        self.flush_list_break();
        self.write(item, "(");
        let base = self.frame().base;
        if subquery {
            let mut frame = Frame::new(base + 1, true);
            frame.clause = Clause::None;
            self.frames.push(frame);
            self.break_before = Some(base + 1);
        } else {
            let mut frame = Frame::new(base, false);
            frame.inline_from = inline_from;
            self.frames.push(frame);
        }
        self.prev_up = "(".to_string();
    }

    fn close_paren(&mut self, item: &Item) {
        if self.frames.len() > 1 {
            let closed = self.frames.pop();
            if closed.is_some_and(|frame| frame.block) {
                let level = self.frame().base;
                self.break_before = None;
                self.newline(level);
            }
        }
        self.write(item, ")");
        self.prev_up = ")".to_string();
    }

    fn comma(&mut self, item: &Item) {
        let frame = self.frame().clone();
        self.write(item, ",");
        match frame.clause {
            Clause::Select | Clause::Set if frame.list => self.break_before = Some(frame.base + 1),
            Clause::With => self.break_before = Some(frame.base),
            _ => {}
        }
        self.prev_up = ",".to_string();
    }

    fn run(&mut self) {
        let items = self.items;
        for (index, item) in items.iter().enumerate() {
            match item.kind {
                Kind::LineComment | Kind::BlockComment => self.comment(item),
                Kind::Word => self.word(index, item),
                Kind::Punct('(') => self.open_paren(index, item),
                Kind::Punct(')') => self.close_paren(item),
                Kind::Punct(',') => self.comma(item),
                _ => {
                    self.flush_list_break();
                    self.write(item, &item.text);
                    self.prev_up = String::from("<token>");
                }
            }
        }
    }
}

/// Format one statement. `None` when the statement is not one this
/// formatter understands (the caller should leave the text untouched).
pub fn format_sql(sql: &str, backslash_escapes: bool) -> Option<String> {
    let items = items(sql, backslash_escapes);
    let first = items.iter().find(|item| !item.is_comment())?;
    if !matches!(
        first.upper().as_str(),
        "SELECT" | "WITH" | "INSERT" | "REPLACE" | "UPDATE" | "DELETE" | "VALUES"
    ) {
        return None;
    }
    let mut formatter = Formatter::new(&items);
    formatter.run();
    let mut out = formatter.out;
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    Some(out)
}

/// Format every statement of `script` (a selection holding several
/// statements), keeping the statements the formatter does not understand as
/// they are. Statements are split like Run splits them (`config`). The text
/// around the statements stays: comments are never moved or dropped, a
/// statement that ends in a `--` comment keeps the separator it had, and
/// where only whitespace and `;` stood between two statements they end up
/// separated by `;` and a blank line (by a blank line alone when a blank
/// line was the boundary). `None` when nothing could be formatted.
pub fn format_script(script: &str, backslash_escapes: bool, config: SplitConfig) -> Option<String> {
    let index = StatementIndex::new(script, config);
    let spans = index.statements();
    let first = spans.first()?;
    let mut any = false;
    let mut out = String::with_capacity(script.len());
    out.push_str(&script[..first.range.start]);
    for (n, span) in spans.iter().enumerate() {
        let original = &script[span.range.clone()];
        let text = match format_sql(original, backslash_escapes) {
            Some(text) => {
                any = true;
                text
            }
            None => original.to_string(),
        };
        let gap_end = spans.get(n + 1).map(|next| next.range.start);
        out.push_str(&text);
        let Some(gap_end) = gap_end else {
            out.push_str(&script[span.range.end..]);
            break;
        };
        let gap = &script[span.range.end..gap_end];
        let plain = gap.chars().all(|c| c.is_whitespace() || c == ';');
        if plain && !sql_buffer::ends_in_line_comment(&text) {
            out.push_str(match span.after {
                Some(Boundary::Inferred) => "\n\n",
                _ => ";\n\n",
            });
        } else {
            out.push_str(gap);
        }
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statements::{SplitMode, SqlDialect};
    use rstest::rstest;

    const SEMICOLONS: SplitConfig = SplitConfig::SEMICOLONS;
    const SMART: SplitConfig = SplitConfig {
        mode: SplitMode::Smart(SqlDialect::PostgreSql),
        unsplit: false,
    };

    #[test]
    fn scripts_are_formatted_statement_by_statement() {
        assert_eq!(
            format_script(
                "select 1;select a,b from t; create table x (a int);",
                false,
                SEMICOLONS
            )
            .unwrap(),
            "SELECT 1;\n\nSELECT\n  a,\n  b\nFROM t;\n\ncreate table x (a int);"
        );
        assert_eq!(
            format_script("select 1", false, SEMICOLONS).as_deref(),
            Some("SELECT 1")
        );
        assert_eq!(
            format_script("create table x (a int);", false, SEMICOLONS),
            None
        );
        assert_eq!(format_script("  ", false, SEMICOLONS), None);
    }

    #[test]
    fn blank_line_boundaries_stay_blank_line_boundaries() {
        assert_eq!(
            format_script("select 1\n\n\nselect a,b from t", false, SMART).unwrap(),
            "SELECT 1\n\nSELECT\n  a,\n  b\nFROM t"
        );
    }

    #[test]
    fn comments_between_statements_are_kept_in_place() {
        let script = "select 1; -- first\n\n-- second\nselect 2;\n-- the end";
        assert_eq!(
            format_script(script, false, SEMICOLONS).unwrap(),
            "SELECT 1; -- first\n\n-- second\nSELECT 2;\n-- the end"
        );
    }

    #[test]
    fn a_separator_never_lands_inside_a_trailing_comment() {
        let script = "select 1 -- one\n;select 2";
        let formatted = format_script(script, false, SEMICOLONS).unwrap();
        assert_eq!(formatted, "SELECT 1 -- one\n;SELECT 2");
        let script = "select 1 -- one\n\nselect 2";
        let formatted = format_script(script, false, SMART).unwrap();
        assert!(formatted.starts_with("SELECT 1 -- one\n"), "{formatted:?}");
        assert!(formatted.ends_with("SELECT 2"), "{formatted:?}");
        assert!(!formatted.contains("-- one;"), "{formatted:?}");
    }

    fn fmt(sql: &str) -> String {
        format_sql(sql, false).expect("a supported statement")
    }

    #[test]
    fn formats_the_showcase_query() {
        let sql = "-- Top customers by lifetime value\nSELECT c.name AS customer,\n       c.country,\n       COUNT(DISTINCT o.id) AS orders,\n       ROUND(SUM(oi.quantity * oi.unit_price), 2) AS lifetime_value\nFROM customers c\nJOIN orders o ON o.customer_id = c.id\nWHERE o.status <> 'cancelled'\nGROUP BY c.id\nORDER BY lifetime_value DESC\nLIMIT 50";
        let expected = "-- Top customers by lifetime value\nSELECT\n  c.name AS customer,\n  c.country,\n  COUNT(DISTINCT o.id) AS orders,\n  ROUND(SUM(oi.quantity * oi.unit_price), 2) AS lifetime_value\nFROM customers c\nJOIN orders o ON o.customer_id = c.id\nWHERE o.status <> 'cancelled'\nGROUP BY c.id\nORDER BY lifetime_value DESC\nLIMIT 50";
        assert_eq!(fmt(sql), expected);
    }

    #[rstest]
    #[case("select * from t where a=1", "SELECT *\nFROM t\nWHERE a = 1")]
    #[case("SELECT a,b FROM t", "SELECT\n  a,\n  b\nFROM t")]
    #[case("select distinct a,b from t", "SELECT DISTINCT\n  a,\n  b\nFROM t")]
    #[case("select 1", "SELECT 1")]
    #[case(
        "select * from t where a=1 and b between 1 and 2 or c=3",
        "SELECT *\nFROM t\nWHERE a = 1\n  AND b BETWEEN 1 AND 2\n  OR c = 3"
    )]
    #[case(
        "select * from a left outer join b on a.id=b.id and a.x=b.x inner join c using (id)",
        "SELECT *\nFROM a\nLEFT OUTER JOIN b ON a.id = b.id\n  AND a.x = b.x\nINNER JOIN c USING (id)"
    )]
    #[case(
        "select a from t where (a=1 and b=2) or c=3",
        "SELECT a\nFROM t\nWHERE (a = 1 AND b = 2)\n  OR c = 3"
    )]
    #[case(
        "select a from t group by a having count(*)>1 order by a desc limit 5",
        "SELECT a\nFROM t\nGROUP BY a\nHAVING count(*) > 1\nORDER BY a DESC\nLIMIT 5"
    )]
    #[case("select 1 union all select 2", "SELECT 1\nUNION ALL\nSELECT 2")]
    fn selects(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(fmt(input), expected);
    }

    #[rstest]
    #[case(
        "update t set a=1,b=2 where id=3",
        "UPDATE t\nSET\n  a = 1,\n  b = 2\nWHERE id = 3"
    )]
    #[case("update t set a=1 where id=3", "UPDATE t\nSET a = 1\nWHERE id = 3")]
    #[case("delete from t where id=3", "DELETE FROM t\nWHERE id = 3")]
    #[case(
        "insert into t (a,b) values (1,2),(3,4)",
        "INSERT INTO t (a, b)\nVALUES (1, 2), (3, 4)"
    )]
    fn writes(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(fmt(input), expected);
    }

    #[test]
    fn subqueries_and_ctes_are_indented() {
        assert_eq!(
            fmt("select * from (select a from t where x in (select 1)) s where a=1"),
            "SELECT *\nFROM (\n  SELECT a\n  FROM t\n  WHERE x IN (\n    SELECT 1\n  )\n) s\nWHERE a = 1"
        );
        assert_eq!(
            fmt("with x as (select 1 a), y as (select 2) select * from x join y on true"),
            "WITH x AS (\n  SELECT 1 a\n),\ny AS (\n  SELECT 2\n)\nSELECT *\nFROM x\nJOIN y ON TRUE"
        );
    }

    #[test]
    fn strings_quoted_identifiers_and_operators_are_untouched() {
        let sql = "select \"Weird Name\", 'a  select  b', data->>'x', a::int, a||b from t where x='it''s'";
        let out = fmt(sql);
        assert!(out.contains("\"Weird Name\""), "{out}");
        assert!(out.contains("'a  select  b'"), "{out}");
        assert!(out.contains("data->>'x'"), "{out}");
        assert!(out.contains("a::int"), "{out}");
        assert!(out.contains("a||b"), "{out}");
        assert!(out.contains("x = 'it''s'"), "{out}");
    }

    #[test]
    fn identifiers_that_look_like_keywords_keep_their_case() {
        let out = fmt("select name, date, count from t");
        assert!(out.contains("name"), "{out}");
        assert!(out.contains("date"), "{out}");
        assert!(out.contains("count"), "{out}");
    }

    #[test]
    fn comments_stay_where_they_were() {
        let out = fmt("select a, -- first\n b /* second */ from t -- tail");
        assert_eq!(
            out,
            "SELECT\n  a, -- first\n  b /* second */\nFROM t -- tail"
        );
        let out = fmt("-- lead\nselect 1");
        assert_eq!(out, "-- lead\nSELECT 1");
    }

    #[test]
    fn extract_from_is_not_a_clause() {
        assert_eq!(
            fmt("select extract(year from d) from t"),
            "SELECT extract(year FROM d)\nFROM t"
        );
    }

    #[test]
    fn window_clauses_stay_inline() {
        assert_eq!(
            fmt("select row_number() over (partition by a order by b) from t"),
            "SELECT row_number() OVER (PARTITION BY a ORDER BY b)\nFROM t"
        );
    }

    #[test]
    fn case_expressions_stay_inline() {
        assert_eq!(
            fmt("select case when a and b then 1 else 2 end from t"),
            "SELECT CASE WHEN a AND b THEN 1 ELSE 2 END\nFROM t"
        );
    }

    #[test]
    fn unsupported_statements_are_left_alone() {
        assert_eq!(format_sql("CREATE TABLE t (a int)", false), None);
        assert_eq!(format_sql("db.users.find({})", false), None);
        assert_eq!(format_sql("   ", false), None);
        assert_eq!(format_sql("-- only a comment", false), None);
    }

    #[rstest]
    #[case("select a,b from t where x=1 and y in (select 1 from u) order by a")]
    #[case("with x as (select 1) select * from x")]
    #[case("update t set a=1,b=2 where id=3")]
    #[case("insert into t (a,b) values (1,2)")]
    #[case("-- c\nselect  a ,b\n from t\n\n where  a=1  and b=2")]
    fn formatting_is_idempotent(#[case] input: &str) {
        let once = fmt(input);
        assert_eq!(fmt(&once), once);
    }

    #[test]
    fn only_whitespace_and_keyword_case_change() {
        let sql = "select a , b from t join u on t.id=u.id where x like 'q%' and y is not null";
        let out = fmt(sql);
        let words = |s: &str| {
            s.split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')'))
                .filter(|w| !w.is_empty())
                .map(|w| w.to_ascii_lowercase())
                .collect::<Vec<_>>()
        };
        // `t.id=u.id` gains spaces around `=`, which splits one word into three
        let normalised = |s: &str| words(s).join(" ").replace("t.id=u.id", "t.id = u.id");
        assert_eq!(normalised(&out), normalised(sql));
    }
}
