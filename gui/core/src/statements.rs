//! Which SQL to run for "Run" / "Run all" / Explain / Format: the selection,
//! the statement under the caret, or every statement. Ranges are UTF-8 byte
//! offsets into the editor text, as used by GPUI Kit's editor.
//!
//! Everything that needs statement boundaries (the run commands, the toolbar
//! hint, the highlight of the statement under the caret, the provenance of
//! results, Format, completion scope) reads them from one [`StatementIndex`],
//! so they can never disagree. Build it once per document revision.
//!
//! # Who owns which bytes
//!
//! A statement runs its own range: its leading comments and body, without a
//! `;`, surrounding whitespace, or a comment trailing the previous statement's
//! `;` on that line. A caret *belongs* to a statement more broadly (see
//! [`StatementIndex::owner`]): from where the statement starts (its leading
//! standalone comment, else its first token) up to where the next one starts.
//! So whitespace between statements, the `;`, and same-line trailing comments
//! belong to the preceding statement; leading whitespace of the document
//! belongs to the first, and trailing comments of the document to the last.

use crate::gui_state::StatementSeparators;
use dbcrust::database::DatabaseType;
pub use dbcrust::sql_buffer::{Boundary, SplitMode, SqlDialect, StatementSpan};
use dbcrust::sql_buffer::{has_blank_line_separator, split_statement_spans_with};
use std::ops::Range;
use std::sync::Arc;

/// What part of the buffer an execution covers (shown in the editor's
/// hint bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// The user's selection.
    Selection,
    /// The `index`-th (0-based) of `count` statements, nearest the caret.
    Statement { index: usize, count: usize },
    /// The whole buffer as one unit (engines whose input is not split).
    Document,
}

/// A piece of the buffer to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub sql: String,
    /// Byte range of `sql` in the buffer (for highlighting).
    pub range: Range<usize>,
    pub kind: TargetKind,
}

/// How a buffer is split: the cache key of a [`StatementIndex`] besides the
/// document revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SplitConfig {
    pub mode: SplitMode,
    /// Engines (MongoDB) whose input is never split: the buffer is one unit.
    pub unsplit: bool,
}

impl SplitConfig {
    /// `;` only, splitting every engine's text.
    pub const SEMICOLONS: Self = Self {
        mode: SplitMode::Semicolons,
        unsplit: false,
    };

    /// The split for a connection of `db_type` under the user's preference.
    /// `unsplit` engines run the whole buffer whatever the preference says.
    pub fn new(
        separators: StatementSeparators,
        db_type: Option<&DatabaseType>,
        unsplit: bool,
    ) -> Self {
        let mode = match separators {
            StatementSeparators::SemicolonsOnly => SplitMode::Semicolons,
            StatementSeparators::SemicolonsAndBlankLines => SplitMode::Smart(dialect_of(db_type)),
        };
        Self { mode, unsplit }
    }
}

/// The SQL grammar to read statements of `db_type` with. Engines without a
/// dedicated grammar (files via DataFusion, Elasticsearch SQL, White Dragon)
/// use the generic one.
pub fn dialect_of(db_type: Option<&DatabaseType>) -> SqlDialect {
    match db_type {
        Some(DatabaseType::PostgreSQL | DatabaseType::Redshift) => SqlDialect::PostgreSql,
        Some(DatabaseType::MySQL) => SqlDialect::MySql,
        Some(DatabaseType::SQLite | DatabaseType::LibSql) => SqlDialect::Sqlite,
        Some(DatabaseType::ClickHouse) => SqlDialect::ClickHouse,
        _ => SqlDialect::Generic,
    }
}

/// The statements of one version of the buffer.
///
/// Parsing (only for [`SplitMode::Smart`], and only for regions between `;`
/// that hold a blank line) happens once, in [`StatementIndex::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementIndex {
    text: Arc<str>,
    config: SplitConfig,
    spans: Vec<StatementSpan>,
    /// The buffer is one statement that holds a blank line the splitter
    /// declined to cut at (worked out once, here, so that asking for the
    /// highlight never rescans the text).
    single_with_blank_line: bool,
}

impl StatementIndex {
    pub fn new(text: &str, config: SplitConfig) -> Self {
        Self::from_shared(Arc::from(text), config)
    }

    /// [`Self::new`] over text the caller already owns (a snapshot of the
    /// editor taken off the UI thread): the text is not copied again.
    pub fn from_shared(text: Arc<str>, config: SplitConfig) -> Self {
        let spans = if config.unsplit {
            Vec::new()
        } else {
            split_statement_spans_with(&text, config.mode)
        };
        let single_with_blank_line = matches!(spans.as_slice(), [only] if has_blank_line_separator(&text[only.range.clone()]));
        Self {
            text,
            config,
            spans,
            single_with_blank_line,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn config(&self) -> SplitConfig {
        self.config
    }

    /// The statements in order (none for an unsplit engine).
    pub fn statements(&self) -> &[StatementSpan] {
        &self.spans
    }

    /// Index of the statement the caret belongs to. `None` for a document
    /// without statements.
    pub fn owner(&self, caret: usize) -> Option<usize> {
        let caret = floor_boundary(&self.text, caret);
        if self.spans.is_empty() {
            return None;
        }
        // The first statement owns everything before the second begins
        let after = self.spans[1..].partition_point(|span| span.range.start <= caret);
        Some(after)
    }

    fn execution(&self, index: usize) -> Execution {
        let (range, kind) = self.statement_target(index);
        self.execute(range, kind)
    }

    fn statement_target(&self, index: usize) -> (Range<usize>, TargetKind) {
        (
            self.spans[index].range.clone(),
            TargetKind::Statement {
                index,
                count: self.spans.len(),
            },
        )
    }

    fn document_target(&self, scope: Range<usize>) -> Option<(Range<usize>, TargetKind)> {
        let range = trim_range(&self.text, scope);
        (!range.is_empty()).then_some((range, TargetKind::Document))
    }

    fn document(&self, scope: Range<usize>) -> Option<Execution> {
        self.document_target(scope)
            .map(|(range, kind)| self.execute(range, kind))
    }

    fn execute(&self, range: Range<usize>, kind: TargetKind) -> Execution {
        Execution {
            sql: self.text[range.clone()].to_string(),
            range,
            kind,
        }
    }

    /// What "Run" executes for `selection` (a caret is an empty range): the
    /// selection as selected, or the statement the caret belongs to. `None`
    /// for a blank buffer or a blank selection.
    pub fn resolve(&self, selection: Range<usize>) -> Option<Execution> {
        self.target(selection)
            .map(|(range, kind)| self.execute(range, kind))
    }

    /// The range and kind [`Self::resolve`] would execute, without copying
    /// its text (the toolbar hint asks on every caret move).
    pub fn target(&self, selection: Range<usize>) -> Option<(Range<usize>, TargetKind)> {
        let selection = clamp(&self.text, selection);
        if !selection.is_empty() {
            let range = trim_range(&self.text, selection);
            return (!range.is_empty()).then_some((range, TargetKind::Selection));
        }
        if self.config.unsplit {
            return self.document_target(0..self.text.len());
        }
        self.owner(selection.start)
            .map(|index| self.statement_target(index))
    }

    /// The statement the caret will run, if the buffer has more than one:
    /// what the editor highlights.
    pub fn highlight(&self, selection: Range<usize>) -> Option<Range<usize>> {
        if self.config.unsplit {
            return None;
        }
        let selection = clamp(&self.text, selection);
        if !selection.is_empty() {
            return None;
        }
        let range = self.spans[self.owner(selection.start)?].range.clone();
        // With one statement there is nothing to tell apart, unless it
        // holds a blank line the splitter declined to cut at: the block that
        // will run is worth showing then (an unfinished second query, say)
        (self.spans.len() > 1 || self.single_with_blank_line).then_some(range)
    }

    /// Every statement of the selection (or of the whole buffer when the
    /// selection is empty), for "Run all". A selection is split on its own
    /// text: a partial selection never grows to the statements around it,
    /// and the ranges stay absolute offsets in the buffer.
    pub fn all(&self, selection: Range<usize>) -> Vec<Execution> {
        let selection = clamp(&self.text, selection);
        if selection.is_empty() {
            if self.config.unsplit {
                return self.document(0..self.text.len()).into_iter().collect();
            }
            return (0..self.spans.len()).map(|i| self.execution(i)).collect();
        }
        if self.config.unsplit {
            return self.document(selection).into_iter().collect();
        }
        let scoped = StatementIndex::new(&self.text[selection.clone()], self.config);
        let count = scoped.spans.len();
        scoped
            .spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                let range =
                    (span.range.start + selection.start)..(span.range.end + selection.start);
                Execution {
                    sql: self.text[range.clone()].to_string(),
                    range,
                    kind: TargetKind::Statement { index, count },
                }
            })
            .collect()
    }

    /// How many statements `selection` holds when run as a batch.
    pub fn count_in(&self, selection: Range<usize>) -> usize {
        self.all(selection).len()
    }
}

/// `sql` without its leading whitespace and comments (`-- …` lines and
/// `/* … */` blocks), for callers that must both classify and EXECUTE the
/// same text (EXPLAIN ANALYZE): DBCrust's read-only classifier looks at the
/// first keyword, so a commented statement would otherwise be refused.
///
/// Conservative: stops (keeps the rest verbatim) at anything engines read
/// differently — MySQL executable comments `/*! … */`, block comments
/// containing `/*` (nested in PostgreSQL only), and unterminated comments.
pub fn strip_leading_comments(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            match after.split_once('\n') {
                Some((_, tail)) => rest = tail.trim_start(),
                None => return "",
            }
        } else if rest.starts_with("/*!") {
            return rest;
        } else if let Some(after) = rest.strip_prefix("/*") {
            match after.split_once("*/") {
                Some((body, tail)) if !body.contains("/*") => rest = tail.trim_start(),
                _ => return rest,
            }
        } else {
            return rest;
        }
    }
}

fn floor_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn clamp(text: &str, range: Range<usize>) -> Range<usize> {
    let (start, end) = (
        floor_boundary(text, range.start),
        floor_boundary(text, range.end),
    );
    start.min(end)..start.max(end)
}

fn trim_range(text: &str, range: Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    let leading = slice.len() - slice.trim_start().len();
    let trailing = slice.len() - slice.trim_end().len();
    let start = range.start + leading;
    let end = (range.end - trailing).max(start);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const SCRIPT: &str = "SELECT 1;\n\nSELECT 'é';\n-- tail\nSELECT 3";

    fn semicolons(text: &str) -> StatementIndex {
        StatementIndex::new(text, SplitConfig::SEMICOLONS)
    }

    fn smart(text: &str) -> StatementIndex {
        StatementIndex::new(
            text,
            SplitConfig::new(
                StatementSeparators::SemicolonsAndBlankLines,
                Some(&DatabaseType::PostgreSQL),
                false,
            ),
        )
    }

    fn texts(executions: &[Execution]) -> Vec<&str> {
        executions.iter().map(|e| e.sql.as_str()).collect()
    }

    #[rstest]
    #[case(0, "SELECT 1", 0)]
    #[case(9, "SELECT 1", 0)] // right after `;`
    #[case(10, "SELECT 1", 0)] // the blank line between belongs to the one before
    #[case(11, "SELECT 'é'", 1)]
    #[case(SCRIPT.len(), "-- tail\nSELECT 3", 2)]
    fn caret_picks_the_owning_statement(
        #[case] caret: usize,
        #[case] expected: &str,
        #[case] index: usize,
    ) {
        let execution = semicolons(SCRIPT).resolve(caret..caret).unwrap();
        assert_eq!(execution.sql, expected);
        assert_eq!(execution.kind, TargetKind::Statement { index, count: 3 });
        assert_eq!(&SCRIPT[execution.range], expected);
    }

    #[test]
    fn caret_at_the_start_of_the_next_statement_runs_that_statement() {
        let index = semicolons("SELECT 1;SELECT 2");
        assert_eq!(index.resolve(9..9).unwrap().sql, "SELECT 2");
        // …while right after a `;` followed by whitespace it is the previous one
        let index = semicolons("DELETE FROM t; \nSELECT 2");
        assert_eq!(index.resolve(14..14).unwrap().sql, "DELETE FROM t");
    }

    #[test]
    fn a_gap_belongs_to_the_statement_before_it_however_wide() {
        // Old behaviour picked the nearest statement; the gap is now the
        // preceding statement's all the way to the next one
        let text = "SELECT 1;\n\n\n\n\n\nSELECT 2";
        let index = semicolons(text);
        for caret in 9..text.len() - "SELECT 2".len() {
            assert_eq!(
                index.resolve(caret..caret).unwrap().sql,
                "SELECT 1",
                "{caret}"
            );
        }
        assert_eq!(
            index.resolve(text.len() - 8..text.len() - 8).unwrap().sql,
            "SELECT 2"
        );
    }

    #[rstest]
    // Caret on each line of two blank-line separated queries
    #[case(0, 0)]
    #[case(10, 0)]
    #[case(42, 0)] // end of the first line
    #[case(43, 0)] // the blank line
    #[case(44, 1)] // start of the second query
    #[case(60, 1)]
    fn a_blank_line_separates_two_queries_without_semicolons(
        #[case] caret: usize,
        #[case] expected: usize,
    ) {
        let text = "select * from orders where status = 'open'\n\nselect count(*) from customers";
        let index = smart(text);
        assert_eq!(index.statements().len(), 2);
        let execution = index.resolve(caret..caret).unwrap();
        assert_eq!(
            execution.kind,
            TargetKind::Statement {
                index: expected,
                count: 2
            }
        );
        assert_eq!(
            execution.sql,
            [
                "select * from orders where status = 'open'",
                "select count(*) from customers"
            ][expected]
        );
    }

    #[test]
    fn semicolons_only_runs_the_whole_buffer() {
        let text = "select 1\n\nselect 2";
        let execution = semicolons(text).resolve(15..15).unwrap();
        assert_eq!(execution.sql, text);
        assert_eq!(execution.kind, TargetKind::Statement { index: 0, count: 1 });
    }

    #[test]
    fn leading_comments_belong_to_the_next_statement() {
        let text = "select 1\n\n-- about two\nselect 2";
        let index = smart(text);
        let in_comment = text.find("about").unwrap();
        let execution = index.resolve(in_comment..in_comment).unwrap();
        assert_eq!(execution.sql, "-- about two\nselect 2");
        assert_eq!(execution.kind, TargetKind::Statement { index: 1, count: 2 });
        // The gap before the comment is still the first query's
        assert_eq!(index.resolve(9..9).unwrap().sql, "select 1");
    }

    #[test]
    fn a_comment_trailing_a_semicolon_stays_with_its_statement() {
        let text = "SELECT 1; -- first\n\n-- second\nSELECT 2;";
        let index = semicolons(text);
        let first = text.find("first").unwrap();
        let second = text.find("second").unwrap();
        assert_eq!(index.resolve(first..first).unwrap().sql, "SELECT 1");
        assert_eq!(
            index.resolve(second..second).unwrap().sql,
            "-- second\nSELECT 2"
        );
        let gap = text.find("\n\n-- second").unwrap() + 1;
        assert_eq!(index.resolve(gap..gap).unwrap().sql, "SELECT 1");
    }

    #[test]
    fn document_edges_belong_to_the_first_and_last_statement() {
        let text = "\n\n  SELECT 1;\nSELECT 2;\n-- the end\n";
        let index = semicolons(text);
        assert_eq!(index.resolve(0..0).unwrap().sql, "SELECT 1");
        assert_eq!(
            index.resolve(text.len()..text.len()).unwrap().sql,
            "SELECT 2"
        );
        assert!(semicolons("-- only a comment\n").resolve(3..3).is_none());
        assert!(semicolons("   \n").resolve(0..0).is_none());
    }

    #[rstest]
    #[case("-- Top customers\nSELECT 1", "SELECT 1")]
    #[case("  /* a */ -- b\n  SELECT 1", "SELECT 1")]
    #[case(
        "/*!50000 DELETE FROM t */ SELECT 1",
        "/*!50000 DELETE FROM t */ SELECT 1"
    )]
    #[case(
        "/* a /* b */ PRAGMA */ DELETE FROM t",
        "/* a /* b */ PRAGMA */ DELETE FROM t"
    )]
    #[case("/* unterminated SELECT 1", "/* unterminated SELECT 1")]
    #[case("-- only a comment", "")]
    // A nested comment is never taken apart at its first `*/`
    #[case(
        "/* outer /* inner */ -- */ EXPLAIN\nDELETE FROM users",
        "/* outer /* inner */ -- */ EXPLAIN\nDELETE FROM users"
    )]
    #[case("SELECT 1 -- trailing", "SELECT 1 -- trailing")]
    fn leading_comments_are_stripped_conservatively(#[case] sql: &str, #[case] expected: &str) {
        assert_eq!(strip_leading_comments(sql), expected);
    }

    #[test]
    fn selection_wins_and_is_trimmed() {
        let text = "  SELECT 1;  SELECT 2  ";
        let index = semicolons(text);
        let execution = index.resolve(0..text.len()).unwrap();
        assert_eq!(execution.sql, "SELECT 1;  SELECT 2");
        assert_eq!(execution.kind, TargetKind::Selection);
        assert!(index.resolve(0..2).is_none());
    }

    #[test]
    fn target_is_what_resolve_executes() {
        let unsplit = StatementIndex::new(
            SCRIPT,
            SplitConfig {
                mode: SplitMode::Semicolons,
                unsplit: true,
            },
        );
        for index in [semicolons(SCRIPT), smart(SCRIPT), unsplit, semicolons("  ")] {
            let len = index.text().len();
            let selections = (0..=len)
                .map(|caret| caret..caret)
                .chain((0..=len).map(|end| 0..end))
                .chain((0..=len).map(|start| start..len));
            for selection in selections {
                let resolved = index.resolve(selection.clone());
                assert_eq!(
                    index.target(selection.clone()),
                    resolved.as_ref().map(|e| (e.range.clone(), e.kind)),
                    "{selection:?}"
                );
                if let Some(execution) = resolved {
                    assert_eq!(execution.sql, &index.text()[execution.range.clone()]);
                }
            }
        }
    }

    #[test]
    fn unsplit_engines_run_the_document() {
        let text = "db.users.find({a: ';'})\n";
        let index = StatementIndex::new(
            text,
            SplitConfig::new(
                StatementSeparators::SemicolonsAndBlankLines,
                Some(&DatabaseType::MongoDB),
                true,
            ),
        );
        let execution = index.resolve(3..3).unwrap();
        assert_eq!(execution.sql, "db.users.find({a: ';'})");
        assert_eq!(execution.kind, TargetKind::Document);
        assert_eq!(index.highlight(3..3), None);
    }

    #[test]
    fn blank_buffer_has_nothing_to_run() {
        assert!(semicolons("   \n").resolve(0..0).is_none());
        assert!(semicolons("").all(0..0).is_empty());
    }

    #[test]
    fn all_statements_of_selection_or_buffer() {
        let index = semicolons(SCRIPT);
        assert_eq!(index.all(0..0).len(), 3);
        let selected = index.all(11..SCRIPT.len());
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].sql, "SELECT 'é'");
        assert_eq!(&SCRIPT[selected[1].range.clone()], selected[1].sql);
    }

    #[test]
    fn a_selection_splits_on_blank_lines_with_absolute_offsets() {
        let text = "select 1\n\nselect 2\n\nselect 3\n\nselect 4";
        let index = smart(text);
        assert_eq!(index.statements().len(), 4);
        // Select the middle two, in whole
        let from = text.find("select 2").unwrap();
        let to = text.find("select 4").unwrap() - 2;
        let batch = index.all(from..to);
        assert_eq!(texts(&batch), vec!["select 2", "select 3"]);
        for execution in &batch {
            assert_eq!(&text[execution.range.clone()], execution.sql);
        }
        assert_eq!(batch[1].kind, TargetKind::Statement { index: 1, count: 2 });
        assert_eq!(index.count_in(from..to), 2);
    }

    #[test]
    fn a_partial_selection_is_not_widened_to_the_statements_around_it() {
        let text = "select 1\n\nselect 2\n\nselect 3";
        let index = smart(text);
        assert_eq!(index.statements().len(), 3);
        // `lect 1 … sel` cuts statements in half: it is one piece, as selected
        let from = 2;
        let to = text.rfind("select 3").unwrap() + 3;
        let batch = index.all(from..to);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].sql, &text[from..to]);
        assert_eq!(batch[0].range, from..to);
    }

    #[test]
    fn a_selection_of_one_statement_is_a_single_execution() {
        let text = "select 1\n\nselect 2";
        let index = smart(text);
        assert_eq!(index.all(0..8).len(), 1);
        assert_eq!(index.all(0..text.len()).len(), 2);
    }

    #[test]
    fn offsets_inside_multibyte_chars_are_clamped() {
        let text = "SELECT 'é'";
        let inside = text.find('é').unwrap() + 1;
        assert!(semicolons(text).resolve(inside..inside).is_some());
    }

    #[test]
    fn unicode_before_an_inferred_boundary_keeps_byte_offsets() {
        let text = "select 'héllo wörld' as s\n\nselect 'ü'";
        let index = smart(text);
        let spans = index.statements();
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].range.clone()], "select 'héllo wörld' as s");
        assert_eq!(&text[spans[1].range.clone()], "select 'ü'");
        assert_eq!(
            index.highlight(spans[1].range.start..spans[1].range.start),
            Some(spans[1].range.clone())
        );
    }

    #[test]
    fn the_highlight_follows_the_caret_and_hides_for_one_statement() {
        let text = "select 1\n\nselect 2";
        let index = smart(text);
        assert_eq!(index.highlight(2..2), Some(0..8));
        assert_eq!(index.highlight(12..12), Some(10..18));
        // A selection hides it
        assert_eq!(index.highlight(0..5), None);
        // Nothing to tell apart in a single statement
        assert_eq!(smart("select 1").highlight(2..2), None);
        // An incomplete second query merges the region: the highlight shows
        // the whole block that would run
        let typing = "select 1\n\nselect * from";
        assert_eq!(smart(typing).highlight(2..2), Some(0..typing.len()));
        assert_eq!(smart(typing).statements().len(), 1);
    }

    #[test]
    fn dialects_follow_the_engine() {
        for (db, expected) in [
            (DatabaseType::PostgreSQL, SqlDialect::PostgreSql),
            (DatabaseType::MySQL, SqlDialect::MySql),
            (DatabaseType::SQLite, SqlDialect::Sqlite),
            (DatabaseType::ClickHouse, SqlDialect::ClickHouse),
            (DatabaseType::CSV, SqlDialect::Generic),
            (DatabaseType::WhiteDragon, SqlDialect::Generic),
            (DatabaseType::Redis, SqlDialect::Generic),
            (DatabaseType::SqlServer, SqlDialect::Generic),
            (DatabaseType::Redshift, SqlDialect::PostgreSql),
            (DatabaseType::LibSql, SqlDialect::Sqlite),
            (DatabaseType::InfluxDB, SqlDialect::Generic),
        ] {
            assert_eq!(dialect_of(Some(&db)), expected);
        }
        assert_eq!(dialect_of(None), SqlDialect::Generic);
    }
}
