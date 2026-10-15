//! Pure logic of the results grid: column profiling, ordering, cell text and
//! number formatting. Nothing here touches GPUI, so it is unit-tested.

use dbcrust_gui_core::ResultSet;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::time::Duration;

/// Narrowest and widest an automatically sized column can be.
pub const MIN_COLUMN_WIDTH: f32 = 64.;
pub const MAX_COLUMN_WIDTH: f32 = 360.;

/// Advance of a 12px monospace glyph, with a little slack.
const CELL_CHAR_WIDTH: f32 = 7.4;
/// Average advance of prose in the UI font at the grid's size.
const PROSE_CHAR_WIDTH: f32 = 6.7;
/// Cell padding plus breathing room.
const CELL_PADDING: f32 = 20.;
/// Room for the sort indicator next to a header label.
const HEADER_EXTRA: f32 = 22.;
/// At most this many rows are looked at to size and classify a column.
const SAMPLE_ROWS: usize = 256;
/// Characters of a value that count towards the column width.
const SAMPLE_CHARS: usize = 80;
/// Longest text a grid cell hands to the text system.
pub const MAX_DISPLAY_CHARS: usize = 300;

/// How the values of a column are typeset: identifiers, numbers and
/// structured data in the monospace font, prose in the UI font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// Numbers: monospace, right-aligned.
    Numeric,
    /// Identifiers, codes, timestamps, JSON, e-mail addresses…: monospace.
    Code,
    /// Names and free text: the UI font.
    Prose,
}

/// What was learnt about a column when the result was shown. Computed once
/// from a bounded sample, never per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnProfile {
    /// Every sampled value parses as a number: right-align the column.
    pub numeric: bool,
    pub kind: ColumnKind,
    /// Initial width in logical pixels.
    pub width: f32,
}

/// True for values that read as data rather than words: structured text,
/// timestamps, `snake_case`/dotted/pathlike tokens, anything mixing letters
/// and digits, and short upper-case codes (`SE`, `USD`). Whitespace-separated
/// words are prose.
pub fn looks_like_code(value: &str) -> bool {
    let value = value.trim();
    let Some(&first) = value.as_bytes().first() else {
        return false;
    };
    if matches!(first, b'{' | b'[' | b'<') {
        return true;
    }
    if first.is_ascii_digit()
        && value.chars().all(|c| {
            c.is_ascii_digit() || matches!(c, '-' | ':' | '/' | '.' | ' ' | 'T' | 'Z' | '+' | ',')
        })
    {
        return true;
    }
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    if value.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    if value
        .chars()
        .any(|c| matches!(c, '_' | '.' | '-' | '/' | ':' | '@' | '#' | '=' | '\\'))
    {
        return true;
    }
    value.chars().all(|c| c.is_ascii_uppercase()) || matches!(value, "true" | "false")
}

pub fn profile_columns(result: &ResultSet) -> Vec<ColumnProfile> {
    let stride = result.rows.len().div_ceil(SAMPLE_ROWS).max(1);
    result
        .columns
        .iter()
        .enumerate()
        .map(|(col, name)| {
            let mut numeric = true;
            let mut seen = 0usize;
            let mut code = 0usize;
            let mut chars = 0usize;
            for row in result.rows.iter().step_by(stride) {
                let Some(Some(value)) = row.get(col) else {
                    chars = chars.max("NULL".len());
                    continue;
                };
                chars = chars.max(value.chars().take(SAMPLE_CHARS).count());
                if value.is_empty() {
                    continue;
                }
                seen += 1;
                numeric = numeric && looks_numeric(value);
                code += usize::from(looks_like_code(value));
            }
            let numeric = numeric && seen > 0;
            // A tie reads as data: a column of short tokens is not prose
            let kind = if numeric {
                ColumnKind::Numeric
            } else if seen > 0 && code * 2 >= seen {
                ColumnKind::Code
            } else {
                ColumnKind::Prose
            };
            let char_width = if kind == ColumnKind::Prose {
                PROSE_CHAR_WIDTH
            } else {
                CELL_CHAR_WIDTH
            };
            let header_chars = name.chars().take(SAMPLE_CHARS).count();
            let cell_width = chars as f32 * char_width + CELL_PADDING;
            // Headers are identifiers: always monospace
            let header_width = header_chars as f32 * CELL_CHAR_WIDTH + CELL_PADDING + HEADER_EXTRA;
            ColumnProfile {
                numeric,
                kind,
                width: cell_width
                    .max(header_width)
                    .clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
                    .round(),
            }
        })
        .collect()
}

/// Parses plain decimal numbers (`42`, `-3.5`, `1e9`). `NaN`, `inf` and
/// text that merely starts with a digit are not numbers.
pub fn parse_number(value: &str) -> Option<f64> {
    let value = value.trim();
    let first = *value.as_bytes().first()?;
    if !(first.is_ascii_digit() || matches!(first, b'-' | b'+' | b'.')) {
        return None;
    }
    value.parse::<f64>().ok().filter(|n| n.is_finite())
}

pub fn looks_numeric(value: &str) -> bool {
    parse_number(value).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// A value as the sort sees it. Numbers order before text, which keeps the
/// comparison a total order even for columns that mix both.
enum SortKey {
    Number(f64),
    /// Lower-cased once per value. UTF-8 orders like the code points it
    /// encodes, so comparing the bytes compares the characters.
    Text(String),
}

fn compare_keys(a: &SortKey, b: &SortKey) -> Ordering {
    match (a, b) {
        (SortKey::Number(x), SortKey::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (SortKey::Number(_), SortKey::Text(_)) => Ordering::Less,
        (SortKey::Text(_), SortKey::Number(_)) => Ordering::Greater,
        (SortKey::Text(x), SortKey::Text(y)) => x.cmp(y),
    }
}

/// Source row indices ordered by column `col`: numeric-aware (`9` before
/// `10`), case-insensitive for text, stable, and with NULL always last — in
/// both directions.
pub fn sorted_order(result: &ResultSet, col: usize, direction: SortDirection) -> Vec<usize> {
    let mut keyed: Vec<(usize, SortKey)> = Vec::with_capacity(result.rows.len());
    let mut nulls: Vec<usize> = Vec::new();
    for (ix, row) in result.rows.iter().enumerate() {
        match row.get(col).and_then(|cell| cell.as_deref()) {
            Some(value) => {
                let key = match parse_number(value) {
                    Some(number) => SortKey::Number(number),
                    None => SortKey::Text(value.chars().flat_map(char::to_lowercase).collect()),
                };
                keyed.push((ix, key));
            }
            None => nulls.push(ix),
        }
    }
    match direction {
        SortDirection::Ascending => keyed.sort_by(|a, b| compare_keys(&a.1, &b.1)),
        SortDirection::Descending => keyed.sort_by(|a, b| compare_keys(&b.1, &a.1)),
    }
    keyed.into_iter().map(|(ix, _)| ix).chain(nulls).collect()
}

/// A value prepared for a single-line grid cell: line breaks become `↵`,
/// tabs a space, and anything past [`MAX_DISPLAY_CHARS`] is cut with `…`.
/// Borrows when the value is already fine (the common case).
pub fn display_text(value: &str) -> Cow<'_, str> {
    let mut truncate_at = None;
    let mut needs_change = false;
    for (count, (ix, ch)) in value.char_indices().enumerate() {
        if count == MAX_DISPLAY_CHARS {
            truncate_at = Some(ix);
            break;
        }
        needs_change |= matches!(ch, '\n' | '\r' | '\t');
    }
    if truncate_at.is_none() && !needs_change {
        return Cow::Borrowed(value);
    }
    let visible = &value[..truncate_at.unwrap_or(value.len())];
    let mut out = String::with_capacity(visible.len() + 3);
    let mut chars = visible.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' => out.push('↵'),
            '\t' => out.push(' '),
            ch => out.push(ch),
        }
    }
    if truncate_at.is_some() {
        out.push('…');
    }
    Cow::Owned(out)
}

/// Re-indents a JSON document (2 spaces) without reordering its keys, which
/// parsing into a map would. `None` unless the text is an object or array
/// that really is valid JSON.
pub fn pretty_json(value: &str) -> Option<String> {
    let text = value.trim();
    let bytes = text.as_bytes();
    if !matches!((bytes.first()?, bytes.last()?), (b'{', b'}') | (b'[', b']')) {
        return None;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(text).ok()?;

    fn newline(out: &mut String, depth: usize) {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    }

    let mut out = String::with_capacity(text.len() * 2);
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '{' | '[' => {
                out.push(ch);
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                let close = if ch == '{' { '}' } else { ']' };
                if chars.peek() == Some(&close) {
                    out.push(close);
                    chars.next();
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(ch);
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            ch if ch.is_whitespace() => {}
            ch => out.push(ch),
        }
    }
    Some(out)
}

/// The first `max_lines` non-blank lines of a statement, each cut at
/// `max_chars`; the flag says whether anything was left out.
pub fn statement_preview(sql: &str, max_lines: usize, max_chars: usize) -> (String, bool) {
    let mut lines = sql
        .lines()
        .map(str::trim_end)
        .skip_while(|l| l.trim().is_empty());
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    for line in lines.by_ref().take(max_lines) {
        if line.chars().count() > max_chars {
            out.push(line.chars().take(max_chars).collect::<String>() + "…");
            cut = true;
        } else {
            out.push(line.to_string());
        }
    }
    if lines.any(|l| !l.trim().is_empty()) {
        cut = true;
    }
    (out.join("\n"), cut)
}

/// `1,234` — thousands separators for counts.
pub fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, ch) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `1 row`, `24 rows`, `10,000 rows`.
pub fn row_count_label(rows: usize) -> String {
    if rows == 1 {
        "1 row".to_string()
    } else {
        format!("{} rows", group_thousands(rows))
    }
}

/// Duration of a finished statement: `<1 ms`, `12 ms`, `1.24 s`, `2 min 05 s`.
pub fn format_elapsed(elapsed: Duration) -> String {
    let millis = elapsed.as_secs_f64() * 1000.;
    if millis < 1. {
        "<1 ms".to_string()
    } else if millis < 1000. {
        format!("{} ms", millis.round() as u64)
    } else if elapsed.as_secs() < 60 {
        format!("{:.2} s", elapsed.as_secs_f64())
    } else {
        format!(
            "{} min {:02} s",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60
        )
    }
}

/// The running timer shows tenths of a second up to here, whole seconds
/// after (it is repainted for every change it shows: see [`running_tick`]).
const RUNNING_TENTHS_UNTIL: Duration = Duration::from_secs(2);

/// Live timer of a running statement: `0.4 s`, `12 s`, `2 min 05 s`.
pub fn format_running(elapsed: Duration) -> String {
    if elapsed < RUNNING_TENTHS_UNTIL {
        format!("{:.1} s", elapsed.as_secs_f64())
    } else if elapsed.as_secs() < 60 {
        format!("{} s", elapsed.as_secs())
    } else {
        format!(
            "{} min {:02} s",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60
        )
    }
}

/// How long until [`format_running`] shows something else after `elapsed`:
/// a tenth of a second at first, then just past the next whole second.
pub fn running_tick(elapsed: Duration) -> Duration {
    if elapsed < RUNNING_TENTHS_UNTIL {
        return Duration::from_millis(100);
    }
    let into_second = Duration::from_nanos(u64::from(elapsed.subsec_nanos()));
    Duration::from_secs(1) - into_second + Duration::from_millis(5)
}

/// The result set shown first for a multi-statement run: the last one that
/// returned columns (the final SELECT of a script), else the first.
pub fn default_tab(sets: &[std::sync::Arc<ResultSet>]) -> usize {
    sets.iter()
        .rposition(|s| !s.columns.is_empty())
        .unwrap_or(0)
}

/// Label of a result tab: `1 · 24 rows`, `2 · ok`.
pub fn tab_label(index: usize, result: &ResultSet) -> String {
    if result.columns.is_empty() {
        format!("{} · ok", index + 1)
    } else {
        format!("{} · {}", index + 1, row_count_label(result.row_count()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn result(columns: &[&str], rows: &[&[Option<&str>]]) -> ResultSet {
        ResultSet {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|c| c.map(str::to_string)).collect())
                .collect(),
            elapsed: Duration::ZERO,
            auto_limit: None,
            sql: String::new(),
        }
    }

    fn column(values: &[Option<&str>]) -> ResultSet {
        let rows: Vec<Vec<Option<&str>>> = values.iter().map(|v| vec![*v]).collect();
        let rows: Vec<&[Option<&str>]> = rows.iter().map(Vec::as_slice).collect();
        result(&["v"], &rows)
    }

    fn sorted_values(values: &[Option<&str>], direction: SortDirection) -> Vec<Option<String>> {
        let r = column(values);
        sorted_order(&r, 0, direction)
            .into_iter()
            .map(|ix| r.rows[ix][0].clone())
            .collect()
    }

    fn some(values: &[&str]) -> Vec<Option<String>> {
        values.iter().map(|v| Some(v.to_string())).collect()
    }

    #[rstest]
    #[case("42", Some(42.))]
    #[case("-3.5", Some(-3.5))]
    #[case("+7", Some(7.))]
    #[case(".5", Some(0.5))]
    #[case("1e3", Some(1000.))]
    #[case(" 12 ", Some(12.))]
    #[case("", None)]
    #[case("abc", None)]
    #[case("12abc", None)]
    #[case("1,000", None)]
    #[case("NaN", None)]
    #[case("inf", None)]
    #[case("-inf", None)]
    #[case("0x10", None)]
    fn numbers(#[case] input: &str, #[case] expected: Option<f64>) {
        assert_eq!(parse_number(input), expected);
    }

    #[test]
    fn numeric_sort_is_numeric_not_lexicographic() {
        assert_eq!(
            sorted_values(
                &[Some("10"), Some("9"), Some("100"), Some("-2.5")],
                SortDirection::Ascending
            ),
            some(&["-2.5", "9", "10", "100"])
        );
        assert_eq!(
            sorted_values(
                &[Some("10"), Some("9"), Some("100")],
                SortDirection::Descending
            ),
            some(&["100", "10", "9"])
        );
    }

    #[test]
    fn null_is_last_in_both_directions() {
        let values = [Some("b"), None, Some("a"), None];
        assert_eq!(
            sorted_values(&values, SortDirection::Ascending),
            vec![Some("a".into()), Some("b".into()), None, None]
        );
        assert_eq!(
            sorted_values(&values, SortDirection::Descending),
            vec![Some("b".into()), Some("a".into()), None, None]
        );
    }

    #[test]
    fn text_sort_ignores_case() {
        assert_eq!(
            sorted_values(
                &[Some("banana"), Some("Apple"), Some("cherry"), Some("apple")],
                SortDirection::Ascending
            ),
            some(&["Apple", "apple", "banana", "cherry"])
        );
    }

    #[test]
    fn sort_is_stable_in_both_directions() {
        let r = result(
            &["k", "id"],
            &[
                &[Some("1"), Some("a")],
                &[Some("1"), Some("b")],
                &[Some("0"), Some("c")],
                &[Some("1"), Some("d")],
            ],
        );
        assert_eq!(
            sorted_order(&r, 0, SortDirection::Ascending),
            vec![2, 0, 1, 3]
        );
        assert_eq!(
            sorted_order(&r, 0, SortDirection::Descending),
            vec![0, 1, 3, 2]
        );
    }

    #[test]
    fn mixed_columns_order_numbers_before_text() {
        assert_eq!(
            sorted_values(
                &[Some("b"), Some("10"), Some("a"), Some("9")],
                SortDirection::Ascending
            ),
            some(&["9", "10", "a", "b"])
        );
    }

    #[test]
    fn text_keys_order_like_comparing_lowercased_characters() {
        let values = [
            "Zebra",
            "zèbre",
            "ÉCOLE",
            "école",
            "ecole",
            "İstanbul",
            "istanbul",
            "ΣΟΦΙΑ",
            "σοφια",
            "straße",
            "STRASSE",
            "a\u{0}b",
            "a",
            "",
            "ǅungla",
            "ǆ",
            "日本",
            "z",
            "Ab",
            "aB",
            "10a",
            "9b",
        ];
        let reference = |direction: SortDirection| {
            let mut order: Vec<usize> = (0..values.len()).collect();
            let lower = |ix: &usize| values[*ix].chars().flat_map(char::to_lowercase);
            order.sort_by(|a, b| match direction {
                SortDirection::Ascending => lower(a).cmp(lower(b)),
                SortDirection::Descending => lower(b).cmp(lower(a)),
            });
            order
        };
        let r = column(&values.map(Some));
        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            assert_eq!(sorted_order(&r, 0, direction), reference(direction));
        }
    }

    #[test]
    fn sorting_an_empty_or_missing_column_is_harmless() {
        let empty = result(&["a"], &[]);
        assert!(sorted_order(&empty, 0, SortDirection::Ascending).is_empty());
        let r = column(&[Some("1"), Some("2")]);
        assert_eq!(sorted_order(&r, 5, SortDirection::Ascending), vec![0, 1]);
    }

    #[test]
    fn numeric_columns_are_detected_and_sized() {
        let r = result(
            &["id", "name", "mixed", "all_null", "empty"],
            &[
                &[Some("1"), Some("Ada Lovelace"), Some("1"), None, Some("")],
                &[Some("22"), Some("Grace"), Some("x"), None, Some("")],
                &[Some("-3.5"), None, Some("3"), None, Some("")],
            ],
        );
        let profiles = profile_columns(&r);
        assert!(profiles[0].numeric);
        assert!(!profiles[1].numeric);
        assert!(!profiles[2].numeric, "one non-number makes the column text");
        assert!(!profiles[3].numeric, "no values, nothing to align");
        assert!(!profiles[4].numeric);
        for p in &profiles {
            assert!((MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(&p.width));
        }
        assert!(
            profiles[1].width > profiles[0].width,
            "wider content gets a wider column"
        );
    }

    #[rstest]
    #[case(r#"{"a": 1}"#, true)]
    #[case("[1, 2]", true)]
    #[case("2025-12-10 11:16:00", true)]
    #[case("2025-12-10T11:16:00Z", true)]
    #[case("550e8400-e29b-41d4-a716-446655440000", true)]
    #[case("customer_id", true)]
    #[case("ada@example.com", true)]
    #[case("SE", true)]
    #[case("USD", true)]
    #[case("true", true)]
    #[case("abc123", true)]
    #[case("Bjarne Stroustrup", false)]
    #[case("Ada", false)]
    #[case("paid", false)]
    #[case("A longer free-form comment, with 3 words", false)]
    #[case("東京都", false)]
    #[case("", false)]
    fn identifiers_are_code_and_words_are_prose(#[case] value: &str, #[case] code: bool) {
        assert_eq!(looks_like_code(value), code, "{value:?}");
    }

    #[test]
    fn columns_are_typeset_by_what_they_hold() {
        let r = result(
            &["id", "name", "country", "created", "note", "all_null"],
            &[
                &[
                    Some("1"),
                    Some("Ada Lovelace"),
                    Some("GB"),
                    Some("2025-12-10 11:16:00"),
                    Some("first customer"),
                    None,
                ],
                &[
                    Some("2"),
                    Some("Alan Turing"),
                    Some("GB"),
                    Some("2025-12-11 09:00:00"),
                    None,
                    None,
                ],
            ],
        );
        let kinds: Vec<ColumnKind> = profile_columns(&r).iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                ColumnKind::Numeric,
                ColumnKind::Prose,
                ColumnKind::Code,
                ColumnKind::Code,
                ColumnKind::Prose,
                ColumnKind::Prose,
            ]
        );
    }

    #[test]
    fn column_width_is_clamped() {
        let long = "x".repeat(2000);
        let r = result(
            &[
                "a_rather_long_column_name_that_keeps_going_and_going_and_going_forever",
                "b",
            ],
            &[&[Some(&long), Some("1")]],
        );
        let profiles = profile_columns(&r);
        assert_eq!(profiles[0].width, MAX_COLUMN_WIDTH);
        assert_eq!(profiles[1].width, MIN_COLUMN_WIDTH);
    }

    #[test]
    fn display_text_borrows_plain_values() {
        assert!(matches!(display_text("hello"), Cow::Borrowed("hello")));
        assert!(matches!(display_text(""), Cow::Borrowed("")));
    }

    #[rstest]
    #[case("a\nb", "a↵b")]
    #[case("a\r\nb", "a↵b")]
    #[case("a\rb", "a↵b")]
    #[case("a\tb", "a b")]
    #[case("héllo\n世界", "héllo↵世界")]
    fn display_text_flattens_whitespace(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(display_text(input), expected);
    }

    #[test]
    fn display_text_truncates_on_char_boundaries() {
        let long = "é".repeat(MAX_DISPLAY_CHARS + 50);
        let shown = display_text(&long);
        assert_eq!(shown.chars().count(), MAX_DISPLAY_CHARS + 1);
        assert!(shown.ends_with('…'));
        let exact = "x".repeat(MAX_DISPLAY_CHARS);
        assert!(matches!(display_text(&exact), Cow::Borrowed(_)));
    }

    #[rstest]
    #[case(
        r#"{"b":1,"a":[1,2,{"c":null}],"s":"x,y:{z}"}"#,
        "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2,\n    {\n      \"c\": null\n    }\n  ],\n  \"s\": \"x,y:{z}\"\n}"
    )]
    #[case("[]", "[]")]
    #[case("{ }", "{}")]
    #[case(r#"[ 1 , "a\"b" ]"#, "[\n  1,\n  \"a\\\"b\"\n]")]
    fn json_is_reindented_in_place(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(pretty_json(input).as_deref(), Some(expected));
    }

    #[rstest]
    #[case("plain text")]
    #[case("{not json}")]
    #[case("[1, 2")]
    #[case("42")]
    #[case("\"quoted\"")]
    #[case("")]
    fn non_json_is_left_alone(#[case] input: &str) {
        assert_eq!(pretty_json(input), None);
    }

    #[test]
    fn json_key_order_is_preserved() {
        let pretty = pretty_json(r#"{"z":1,"a":2,"m":3}"#).expect("json");
        let z = pretty.find("\"z\"").expect("z");
        let a = pretty.find("\"a\"").expect("a");
        let m = pretty.find("\"m\"").expect("m");
        assert!(z < a && a < m);
    }

    #[test]
    fn statement_preview_limits_lines_and_width() {
        let sql = "\n\nSELECT a,\n       b\nFROM t\nWHERE a > 1\n";
        assert_eq!(
            statement_preview(sql, 10, 80),
            (
                "SELECT a,\n       b\nFROM t\nWHERE a > 1".to_string(),
                false
            )
        );
        let (preview, cut) = statement_preview(sql, 2, 80);
        assert_eq!(preview, "SELECT a,\n       b");
        assert!(cut);
        let (preview, cut) = statement_preview("SELECT 123456789", 5, 6);
        assert_eq!(preview, "SELECT…");
        assert!(cut);
        assert_eq!(statement_preview("", 3, 10), (String::new(), false));
    }

    #[rstest]
    #[case(0, "0")]
    #[case(999, "999")]
    #[case(1000, "1,000")]
    #[case(1234567, "1,234,567")]
    fn thousands(#[case] n: usize, #[case] expected: &str) {
        assert_eq!(group_thousands(n), expected);
    }

    #[rstest]
    #[case(0, "0 rows")]
    #[case(1, "1 row")]
    #[case(24, "24 rows")]
    #[case(10_000, "10,000 rows")]
    fn row_counts(#[case] n: usize, #[case] expected: &str) {
        assert_eq!(row_count_label(n), expected);
    }

    #[rstest]
    #[case(Duration::ZERO, "<1 ms")]
    #[case(Duration::from_micros(400), "<1 ms")]
    #[case(Duration::from_millis(12), "12 ms")]
    #[case(Duration::from_millis(1240), "1.24 s")]
    #[case(Duration::from_secs(125), "2 min 05 s")]
    fn elapsed(#[case] d: Duration, #[case] expected: &str) {
        assert_eq!(format_elapsed(d), expected);
    }

    #[rstest]
    #[case(Duration::from_millis(40), "0.0 s")]
    #[case(Duration::from_millis(1960), "2.0 s")]
    #[case(Duration::from_millis(2000), "2 s")]
    #[case(Duration::from_millis(3400), "3 s")]
    #[case(Duration::from_millis(59_999), "59 s")]
    #[case(Duration::from_secs(61), "1 min 01 s")]
    fn running_timer(#[case] d: Duration, #[case] expected: &str) {
        assert_eq!(format_running(d), expected);
    }

    #[test]
    fn the_running_timer_repaints_when_its_text_changes() {
        assert_eq!(running_tick(Duration::ZERO), Duration::from_millis(100));
        assert_eq!(
            running_tick(Duration::from_millis(1950)),
            Duration::from_millis(100)
        );
        // Whole seconds: wake just past the next one, whatever the drift
        for elapsed in [2_000, 2_400, 7_999, 61_250, 3_600_010] {
            let elapsed = Duration::from_millis(elapsed);
            let next = elapsed + running_tick(elapsed);
            assert_eq!(next.as_secs(), elapsed.as_secs() + 1, "{elapsed:?}");
            assert!(next.subsec_millis() <= 5, "{elapsed:?}");
            assert_ne!(format_running(next), format_running(elapsed));
        }
    }

    #[test]
    fn scripts_open_on_their_last_result_with_columns() {
        use std::sync::Arc;
        let ddl = Arc::new(result(&[], &[]));
        let select = Arc::new(result(&["a"], &[]));
        assert_eq!(default_tab(&[ddl.clone(), select.clone(), ddl.clone()]), 1);
        assert_eq!(
            default_tab(&[ddl.clone(), select.clone(), select.clone()]),
            2
        );
        assert_eq!(default_tab(&[ddl.clone(), ddl.clone()]), 0);
        assert_eq!(default_tab(&[]), 0);
    }

    #[test]
    fn tab_labels() {
        let rows = result(&["a"], &[&[Some("1")], &[Some("2")]]);
        let ddl = result(&[], &[]);
        assert_eq!(tab_label(0, &rows), "1 · 2 rows");
        assert_eq!(tab_label(1, &ddl), "2 · ok");
        assert_eq!(tab_label(2, &result(&["a"], &[])), "3 · 0 rows");
    }
}
