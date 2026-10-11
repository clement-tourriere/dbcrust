//! Does an edit change what ran? Decides whether the results on screen still
//! belong to the editor ("Results from previous run" otherwise).
//!
//! Each piece of SQL that ran is remembered exactly as it was sent
//! ([`Executed`]) and looked for in the buffer as it is now:
//!
//! - a **whole statement** (the one under the caret, or every statement of
//!   the buffer): the buffer is split again, by the same [`StatementIndex`]
//!   the run used (`;` and, in smart mode, inferred blank-line boundaries),
//!   and the statement must still be one of the results, verbatim. `SELECT 1` → `SELECT 10`, or an
//!   appended `WHERE`, is a different statement; edits to the other
//!   statements of a script are not.
//! - a **slice** (a selection, or the statements of a selection): its text
//!   must still be in the buffer *and* still stand where it stood: text
//!   against it that was not there when it ran (`SELECT 20` → `SELECT 200`,
//!   an appended clause, a comment line above) has become part of it, while
//!   a new statement after a `;` (or, in smart mode, after a blank line), or
//!   whitespace, has not.
//!
//! Nothing here touches GPUI.

use dbcrust_gui_core::dbcrust::sql_buffer::{Boundary, SplitMode, split_statement_spans_with};
use dbcrust_gui_core::statements::StatementIndex;
use std::ops::Range;

/// A piece of SQL that ran, and how it sat in the buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Executed {
    /// What was sent. For a slice that ends in its own `;`, without it: the
    /// `;` ends the statement, it is not part of what the edit must preserve.
    pub sql: String,
    /// A whole statement of the buffer, as opposed to a slice of it.
    pub aligned: bool,
    /// For a slice: the nearest text before and after it (whitespace
    /// aside) when it ran. A slice that ends in `;` is closed on the right,
    /// whatever follows it.
    pub neighbours: (Option<char>, Option<char>),
}

impl Executed {
    /// `range` of the buffer `index` was built from was sent: a whole
    /// statement when `aligned`.
    pub(super) fn new(index: &StatementIndex, range: Range<usize>, aligned: bool) -> Self {
        let buffer = index.text();
        let (before, after) = (buffer.get(..range.start), buffer.get(range.end..));
        let sent = buffer.get(range.clone()).unwrap_or_default();
        let mut neighbours = (
            before.and_then(|text| neighbour_before(text, index)),
            after.and_then(|text| neighbour_after(text, range.end, index)),
        );
        let mut sql = sent;
        if !aligned && let Some(body) = terminated(sent, index.config().mode) {
            sql = body;
            neighbours.1 = Some(';');
        }
        Self {
            sql: sql.to_string(),
            aligned,
            neighbours,
        }
    }
}

/// The nearest thing before `text`'s end (whitespace aside). A boundary the
/// splitter *inferred* there (a blank line `index` accepted as the end of a
/// statement) stands between two statements just like a `;` does, so it is
/// reported as one. A blank line that did not split anything is only
/// whitespace: the text before it is still against what follows.
fn neighbour_before(text: &str, index: &StatementIndex) -> Option<char> {
    let trimmed = text.trim_end();
    let last = trimmed.chars().next_back()?;
    Some(if inferred_boundary_at(index, trimmed.len()) {
        ';'
    } else {
        last
    })
}

/// The nearest thing after `text`'s start, which sits at byte `at` of the
/// buffer `index` describes (see [`neighbour_before`]).
fn neighbour_after(text: &str, at: usize, index: &StatementIndex) -> Option<char> {
    let first = text.trim_start().chars().next()?;
    Some(if inferred_boundary_at(index, at) {
        ';'
    } else {
        first
    })
}

/// A statement of `index` ends at `end` because a blank line follows it.
fn inferred_boundary_at(index: &StatementIndex, end: usize) -> bool {
    let spans = index.statements();
    spans
        .binary_search_by_key(&end, |span| span.range.end)
        .is_ok_and(|found| spans[found].after == Some(Boundary::Inferred))
}

/// The text of a selection before its closing `;`, when it has one. The
/// statement splitter decides what is a `;` (not one inside a string, a
/// dollar-quoted body or a comment) and what follows it: nothing but
/// whitespace and comments, or it would have found another statement.
fn terminated(sql: &str, mode: SplitMode) -> Option<&str> {
    let last = split_statement_spans_with(sql, mode).pop()?.range;
    sql[last.end..]
        .trim_start()
        .starts_with(';')
        .then(|| &sql[..last.end])
}

/// True when an edit changed any of what `ran`, so its results no longer
/// match the buffer `now` (split the way the run split it). `unsplit`
/// engines run the whole buffer as one statement.
pub(super) fn is_stale(ran: &[Executed], now: &StatementIndex) -> bool {
    let text = now.text();
    let statements: Vec<&str> = if ran.iter().any(|executed| executed.aligned) {
        if now.config().unsplit {
            vec![text.trim()]
        } else {
            now.statements()
                .iter()
                .map(|span| &text[span.range.clone()])
                .collect()
        }
    } else {
        Vec::new()
    };
    ran.iter().any(|executed| {
        if executed.aligned {
            !statements.contains(&executed.sql.as_str())
        } else {
            !slice_stands(executed, now)
        }
    })
}

/// A slice still stands when its text is in `now` with the same things
/// against it: a `;` (or a blank line `now` splits at) or the edge of the
/// buffer stays one, anything else must be the same text it was.
fn slice_stands(executed: &Executed, index: &StatementIndex) -> bool {
    if executed.sql.is_empty() {
        return true;
    }
    let now = index.text();
    now.match_indices(&executed.sql).any(|(start, found)| {
        let end = start + found.len();
        let before = neighbour_before(&now[..start], index);
        let after = neighbour_after(&now[end..], end, index);
        keeps(executed.neighbours.0, before) && keeps(executed.neighbours.1, after)
    })
}

/// A side that stood apart (`;` or the buffer's edge) still does; one that
/// had text against it has the same text.
fn keeps(was: Option<char>, is: Option<char>) -> bool {
    let apart = |neighbour: Option<char>| matches!(neighbour, None | Some(';'));
    if apart(was) { apart(is) } else { was == is }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro could shadow the built-in `#[test]` in neighbours
    use std::prelude::v1::test;

    use super::*;
    use dbcrust_gui_core::statements::{SplitConfig, SqlDialect};
    use rstest::rstest;

    const MODE: SplitMode = SplitMode::Semicolons;
    const SMART: SplitMode = SplitMode::Smart(SqlDialect::PostgreSql);

    fn index(buffer: &str, mode: SplitMode) -> StatementIndex {
        StatementIndex::new(
            buffer,
            SplitConfig {
                mode,
                unsplit: false,
            },
        )
    }

    /// [`super::is_stale`] on `now` split on `;`.
    fn is_stale(ran: &[Executed], now: &str, unsplit: bool) -> bool {
        smart_stale(
            ran,
            now,
            SplitConfig {
                mode: MODE,
                unsplit,
            },
        )
    }

    fn smart_stale(ran: &[Executed], now: &str, config: SplitConfig) -> bool {
        super::is_stale(ran, &StatementIndex::new(now, config))
    }

    /// `sql`, found in `buffer`, as a whole statement that ran.
    fn statement(buffer: &str, sql: &str) -> Executed {
        let start = buffer.find(sql).expect("the statement is in the buffer");
        Executed::new(&index(buffer, MODE), start..start + sql.len(), true)
    }

    /// `selected`, found in `buffer`, as a selection that ran.
    fn selection(buffer: &str, selected: &str) -> Executed {
        let start = buffer
            .find(selected)
            .expect("the selection is in the buffer");
        Executed::new(&index(buffer, MODE), start..start + selected.len(), false)
    }

    #[rstest]
    // The statement itself
    #[case("SELECT 1", "SELECT 10", true)]
    #[case("SELECT 1", "SELECT", true)]
    #[case("SELECT 1", "SELECT 1 WHERE true", true)]
    #[case("SELECT 1;", "SELECT 1 WHERE true;", true)]
    #[case("SELECT * FROM t", "SELECT * FROM t WHERE id = 1", true)]
    #[case("SELECT 1", "select 1", true)]
    #[case("SELECT 1", "", true)]
    // Nothing that changes it
    #[case("SELECT 1", "SELECT 1", false)]
    #[case("SELECT 1", "  SELECT 1\n", false)]
    #[case("SELECT 1;", "SELECT 1; SELECT 2;", false)]
    #[case("SELECT 1", "SELECT 1; SELECT 2", false)]
    #[case("SELECT 1", "-- note\n; SELECT 1", false)]
    fn a_statement_that_ran(#[case] before: &str, #[case] now: &str, #[case] stale: bool) {
        let ran = [statement(before, before.trim_end_matches(';'))];
        assert_eq!(is_stale(&ran, now, false), stale, "{now:?}");
    }

    #[test]
    fn editing_another_statement_of_the_script_leaves_the_rest_fresh() {
        let before = "SELECT 1 AS a;\nSELECT 2 AS b;\nSELECT 3 AS c";
        let ran = [statement(before, "SELECT 2 AS b")];
        // The statements around it change, or move
        for now in [
            "SELECT 9 AS a;\nSELECT 2 AS b;\nSELECT 3 AS c",
            "SELECT 1 AS a;\nSELECT 2 AS b;\nSELECT 9 AS c",
            "SELECT 0;\nSELECT 1 AS a;\nSELECT 2 AS b;\nSELECT 3 AS c",
            "SELECT 2 AS b",
        ] {
            assert!(!is_stale(&ran, now, false), "{now:?}");
        }
        // Its own text, however slightly, makes it stale
        for now in [
            "SELECT 1 AS a;\nSELECT 2 AS bb;\nSELECT 3 AS c",
            "SELECT 1 AS a;\nSELECT 20 AS b;\nSELECT 3 AS c",
            "SELECT 1 AS a;\nSELECT 2 AS b WHERE x;\nSELECT 3 AS c",
            // The `;` between two statements is gone: they became one
            "SELECT 1 AS a;\nSELECT 2 AS b\nSELECT 3 AS c",
        ] {
            assert!(is_stale(&ran, now, false), "{now:?}");
        }
    }

    #[test]
    fn a_batch_is_stale_as_soon_as_one_of_its_statements_is() {
        let before = "SELECT 1; SELECT 2; SELECT 3";
        let ran = [
            statement(before, "SELECT 1"),
            statement(before, "SELECT 2"),
            statement(before, "SELECT 3"),
        ];
        assert!(!is_stale(&ran, "SELECT 1;\nSELECT 2;\nSELECT 3;", false));
        assert!(is_stale(&ran, "SELECT 1; SELECT 2; SELECT 33", false));
        // …but a script that only partly ran (it stopped at an error) is
        // compared on what ran
        assert!(!is_stale(&ran[..1], "SELECT 1; oops", false));
    }

    #[test]
    fn engines_that_run_the_whole_buffer_compare_the_whole_buffer() {
        let before = "{ \"find\": \"users\" }";
        let ran = [statement(before, before)];
        assert!(!is_stale(&ran, "\n  { \"find\": \"users\" }  \n", true));
        assert!(is_stale(
            &ran,
            "{ \"find\": \"users\", \"limit\": 5 }",
            true
        ));
    }

    #[rstest]
    // Inside the selection, or against it
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 200;", true)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 2;", true)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 20 WHERE x;", true)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSEL 20;", true)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\n;", true)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\n-- hi\nSELECT 20;", true)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nEXPLAIN SELECT 20;", true)]
    // Elsewhere, or only apart from it
    #[case("SELECT 1;\nSELECT 20;", "SELECT 9;\nSELECT 20;", false)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 20;\nSELECT 3;", false)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 3;\nSELECT 20;", false)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\n\n  SELECT 20;", false)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 20; -- done", false)]
    #[case("SELECT 1;\nSELECT 20;", "SELECT 1;\nSELECT 20;", false)]
    fn a_selection_that_ran(#[case] before: &str, #[case] now: &str, #[case] stale: bool) {
        let ran = [selection(before, "SELECT 20")];
        assert_eq!(is_stale(&ran, now, false), stale, "{now:?}");
    }

    #[rstest]
    // The selection ends in its own `;`: what follows it does not matter…
    #[case("SELECT 1;", "SELECT 1;SELECT 2;", false)]
    #[case("SELECT 1;", "SELECT 1;\nSELECT 2;", false)]
    #[case("SELECT 1;", "SELECT 1; -- done", false)]
    #[case("SELECT 1;", "SELECT 0;\nSELECT 1;", false)]
    // …and dropping only that `;` is not a change of the statement
    #[case("SELECT 1;", "SELECT 1", false)]
    #[case("SELECT 1;", "SELECT 1\n", false)]
    #[case("SELECT 1;  \n-- end\n", "SELECT 1", false)]
    // But it still must not grow, or merge with what follows
    #[case("SELECT 1;", "SELECT 10;", true)]
    #[case("SELECT 1;", "SELECT 1 WHERE x;", true)]
    #[case("SELECT 1;", "SELECT 1\nSELECT 2", true)]
    #[case("SELECT 1;", "SELECT 2;", true)]
    fn a_selection_that_ends_in_a_semicolon(
        #[case] before: &str,
        #[case] now: &str,
        #[case] stale: bool,
    ) {
        let ran = [selection(before, before.trim_end())];
        assert_eq!(is_stale(&ran, now, false), stale, "{now:?}");
    }

    #[rstest]
    // A `;` and a `--` inside a string are text, not a terminator or a comment
    #[case("SELECT 'a;\n-- b';", "SELECT 'a;\n-- c';", true)]
    #[case("SELECT 'a;\n-- b';", "SELECT 'a;\n-- b';\nSELECT 2;", false)]
    #[case("SELECT 'a;\n-- b';", "SELECT 'a;\n-- b'", false)]
    #[case("SELECT 'a;\n-- b'", "SELECT 'a;\n-- b'\nSELECT 2", true)]
    // …and in a PostgreSQL escape string, where `\'` does not end it
    #[case("SELECT E'a\\';-- b';", "SELECT E'a\\';-- c';", true)]
    #[case("SELECT E'a\\';-- b';", "SELECT E'a\\';-- b';\nSELECT 2;", false)]
    #[case("SELECT E'a\\';-- b';", "SELECT E'a\\';-- b'", false)]
    // Likewise in a dollar-quoted body
    #[case("SELECT $$a;\n-- b$$;", "SELECT $$a;\n-- c$$;", true)]
    #[case("SELECT $$a;\n-- b$$;", "SELECT $$a;\n-- b$$; SELECT 2;", false)]
    #[case("SELECT $q$a;\n-- b$q$", "SELECT $q$a;\n-- b$q$\nSELECT 2", true)]
    // A `;` in a comment is no terminator either
    #[case("SELECT 1 /* ; */;", "SELECT 1 /* : */;", true)]
    #[case("SELECT 1 /* ; */;", "SELECT 1 /* ; */;\nSELECT 2;", false)]
    #[case("SELECT 1 /* ; */", "SELECT 1 /* ; */\nSELECT 2", true)]
    #[case("SELECT 1 -- ;", "SELECT 1 -- ;\nSELECT 2", true)]
    // Real comments after the terminator are not part of the statement
    #[case("SELECT 1; /* done */", "SELECT 1; /* done */\nSELECT 2;", false)]
    fn a_terminator_is_found_by_the_statement_splitter(
        #[case] before: &str,
        #[case] now: &str,
        #[case] stale: bool,
    ) {
        let ran = [selection(before, before.trim_end())];
        assert_eq!(is_stale(&ran, now, false), stale, "{now:?}");
    }

    #[test]
    fn a_selection_that_starts_after_a_semicolon_stays_closed_on_the_left() {
        let before = "SELECT 1;\nSELECT 2;";
        let ran = [selection(before, "SELECT 2;")];
        assert!(!is_stale(&ran, "SELECT 0;\nSELECT 1;\nSELECT 2;", false));
        assert!(!is_stale(&ran, "SELECT 1;\n\nSELECT 2", false));
        // The `;` before it is gone: the text before it is part of it now
        assert!(is_stale(&ran, "SELECT 1\nSELECT 2;", false));
    }

    #[test]
    fn a_whole_buffer_selection_that_gains_a_clause_is_stale() {
        let before = "SELECT * FROM t";
        let ran = [selection(before, before)];
        assert!(is_stale(&ran, "SELECT * FROM t\nWHERE id = 1", false));
        assert!(is_stale(&ran, "SELECT * FROM t2", false));
        // A new statement before or after it is not part of it
        assert!(!is_stale(&ran, "SELECT 1; SELECT * FROM t", false));
        assert!(!is_stale(&ran, "SELECT * FROM t;\nSELECT 2", false));
        assert!(!is_stale(&ran, "SELECT * FROM t\n", false));
    }

    #[test]
    fn typing_right_at_the_edge_of_a_selection_is_outside_it_when_it_is_only_space() {
        let before = "SELECT 1; SELECT 2";
        let ran = [selection(before, "SELECT 2")];
        assert!(!is_stale(&ran, "SELECT 1;  SELECT 2", false));
        assert!(!is_stale(&ran, "SELECT 1; SELECT 2 ", false));
        // One character in is inside
        assert!(is_stale(&ran, "SELECT 1; SELECT  2", false));
    }

    #[test]
    fn a_selection_in_the_middle_of_a_statement_keeps_what_surrounds_it() {
        let before = "SELECT a FROM t WHERE x = 1 ORDER BY a";
        let ran = [selection(before, "WHERE x = 1")];
        assert!(!is_stale(
            &ran,
            "SELECT a, b FROM t WHERE x = 1 ORDER BY a",
            false
        ));
        assert!(is_stale(
            &ran,
            "SELECT a FROM t WHERE x = 1 AND y ORDER BY a",
            false
        ));
        assert!(is_stale(
            &ran,
            "SELECT a FROM t JOIN u WHERE x = 1 ORDER BY a",
            false
        ));
    }

    // ----- Inferred (blank-line) boundaries -----

    const SMART_CONFIG: SplitConfig = SplitConfig {
        mode: SMART,
        unsplit: false,
    };

    fn smart_statement(buffer: &str, sql: &str) -> Executed {
        let start = buffer.find(sql).expect("the statement is in the buffer");
        Executed::new(&index(buffer, SMART), start..start + sql.len(), true)
    }

    #[test]
    fn a_statement_cut_at_a_blank_line_follows_the_same_boundaries_as_the_run() {
        let before = "select 1\n\nselect 2\n\nselect 3";
        let ran = [smart_statement(before, "select 2")];
        // Edits to its neighbours leave it alone
        for now in [
            "select 9\n\nselect 2\n\nselect 3",
            "select 1\n\nselect 2\n\nselect 9 from t",
            "select 0\n\nselect 1\n\nselect 2\n\nselect 3",
        ] {
            assert!(!smart_stale(&ran, now, SMART_CONFIG), "{now:?}");
        }
        // Its own text, or the blank line going away (they merge: nothing
        // parses as two statements any more), makes it stale
        for now in [
            "select 1\n\nselect 22\n\nselect 3",
            "select 1\n\nselect 2 where x\n\nselect 3",
            "select 1\nselect 2\n\nselect 3",
        ] {
            assert!(smart_stale(&ran, now, SMART_CONFIG), "{now:?}");
        }
        // Without the smart split the same statement never existed on its own
        let semicolons = SplitConfig {
            mode: MODE,
            unsplit: false,
        };
        assert!(smart_stale(
            &ran,
            "select 1\n\nselect 2\n\nselect 3",
            semicolons
        ));
    }

    #[test]
    fn typing_an_incomplete_query_merges_the_region_and_ages_the_results() {
        let before = "select 1\n\nselect 2";
        let ran = [smart_statement(before, "select 1")];
        assert!(!smart_stale(&ran, "select 1\n\nselect 2", SMART_CONFIG));
        // `select 2 from` does not parse: the region is one statement now
        assert!(smart_stale(&ran, "select 1\n\nselect 2 from", SMART_CONFIG));
    }

    #[test]
    fn a_blank_line_keeps_a_selected_slice_closed_like_a_semicolon() {
        let before = "select 1\n\nselect 20\n\nselect 3";
        let start = before.find("select 20").unwrap();
        let ran = [Executed::new(
            &index(before, SMART),
            start..start + 9,
            false,
        )];
        assert_eq!(ran[0].neighbours, (Some(';'), Some(';')));
        let now = |text: &str| smart_stale(&ran, text, SMART_CONFIG);
        assert!(!now("select 1\n\nselect 20\n\nselect 3"));
        assert!(!now(
            "select 0\n\nselect 1\n\n\nselect 20\n\nselect 3 from t"
        ));
        // Text that joins it now is against it
        assert!(now("select 1\nselect 20\n\nselect 3"));
        assert!(now("select 1\n\nselect 200\n\nselect 3"));
        // The same buffer with blank lines does not close it when only `;` count
        let plain = Executed::new(&index(before, MODE), start..start + 9, false);
        assert_eq!(plain.neighbours, (Some('1'), Some('s')));
    }

    #[test]
    fn a_blank_line_that_does_not_split_is_not_a_boundary() {
        // `select * from users` alone, then a `WHERE` below a blank line: the
        // index reads one statement, so the selection grew a clause
        let before = "SELECT * FROM users";
        let ran = [Executed::new(&index(before, SMART), 0..before.len(), false)];
        assert_eq!(ran[0].neighbours, (None, None));
        for now in [
            "SELECT * FROM users\n\nWHERE enabled = true",
            "SELECT * FROM users\n\n\nWHERE enabled = true",
        ] {
            assert!(smart_stale(&ran, now, SMART_CONFIG), "{now:?}");
        }
        // The same slice with a real second query below it stays fresh
        assert!(!smart_stale(
            &ran,
            "SELECT * FROM users\n\nSELECT 2",
            SMART_CONFIG
        ));
        // …and the same when what follows only parses as a query on its own
        // in the other mode: there the blank line never separates
        let semicolons = SplitConfig {
            mode: MODE,
            unsplit: false,
        };
        assert!(smart_stale(
            &ran,
            "SELECT * FROM users\n\nSELECT 2",
            semicolons
        ));
    }
}
