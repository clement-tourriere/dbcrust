//! Rendering of result sets as CSV, TSV, JSON and Markdown, for the clipboard
//! and for files. Pure functions over [`ResultSet`]: no I/O, no GPUI.
//!
//! Every renderer takes an optional row order (indices into
//! [`ResultSet::rows`]) so that "what you see" — the grid sorted by a column,
//! or a selection of rows — is exactly what is copied or exported. Indices
//! out of range are skipped.
//!
//! Conventions, chosen to round-trip into spreadsheets and databases:
//! - **CSV** follows RFC 4180: CRLF line ends, fields containing `,`, `"`, CR
//!   or LF are quoted, quotes are doubled. Like PostgreSQL's `COPY … CSV`,
//!   SQL `NULL` is an empty unquoted field and an empty string is `""`, so
//!   the two stay distinguishable.
//! - **TSV** is what spreadsheets put on the clipboard: fields containing a
//!   tab, CR, LF or `"` are quoted the same way as CSV; `NULL` is empty.
//! - **JSON** is an array of objects with 2-space indentation. Cells are the
//!   engine's text rendering, so values are strings (`null` for `NULL`), never
//!   guessed numbers or booleans — `"007"` must not become `7`. Duplicate
//!   column names are disambiguated as `name (2)`, `name (3)`.
//! - **Markdown** is a GitHub-flavoured table; `|` is escaped, line breaks
//!   become `<br>`, and `NULL` is spelled out.

use crate::types::ResultSet;
use std::collections::HashSet;
use std::fmt::Write as _;

/// How SQL `NULL` is spelled when a single value is copied, and in Markdown.
pub const NULL_TEXT: &str = "NULL";

/// The export formats offered by the results panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    Markdown,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 4] = [
        ExportFormat::Csv,
        ExportFormat::Tsv,
        ExportFormat::Json,
        ExportFormat::Markdown,
    ];

    /// Name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Tsv => "TSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Markdown => "Markdown",
        }
    }

    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::Markdown => "md",
        }
    }

    /// Render `result` (all rows, or only `rows`, in that order).
    pub fn render(self, result: &ResultSet, rows: Option<&[usize]>) -> String {
        let table = Table::new(result, rows);
        match self {
            ExportFormat::Csv => table.csv(),
            ExportFormat::Tsv => table.tsv(),
            ExportFormat::Json => table.json(),
            ExportFormat::Markdown => table.markdown(),
        }
    }
}

/// All rows as CSV (header first).
pub fn to_csv(result: &ResultSet) -> String {
    ExportFormat::Csv.render(result, None)
}

/// All rows as TSV (header first).
pub fn to_tsv(result: &ResultSet) -> String {
    ExportFormat::Tsv.render(result, None)
}

/// All rows as a JSON array of objects.
pub fn to_json(result: &ResultSet) -> String {
    ExportFormat::Json.render(result, None)
}

/// All rows as a Markdown table.
pub fn to_markdown(result: &ResultSet) -> String {
    ExportFormat::Markdown.render(result, None)
}

/// One row as tab-separated values, without a header: what pastes into a
/// spreadsheet row.
pub fn row_to_tsv(result: &ResultSet, row: usize) -> Option<String> {
    let cells = result.rows.get(row)?;
    let mut out = String::new();
    for (ix, cell) in cells.iter().enumerate() {
        if ix > 0 {
            out.push('\t');
        }
        push_tsv_field(&mut out, cell.as_deref());
    }
    Some(out)
}

/// One row as a pretty-printed JSON object.
pub fn row_to_json(result: &ResultSet, row: usize) -> Option<String> {
    let cells = result.rows.get(row)?;
    let names = unique_column_names(&result.columns);
    let mut out = String::new();
    push_json_object(&mut out, &names, cells, 0);
    Some(out)
}

/// The values of one column, one per line (header excluded), for `rows` (or
/// all rows).
pub fn column_to_text(result: &ResultSet, col: usize, rows: Option<&[usize]>) -> String {
    let table = Table::new(result, rows);
    let mut out = String::new();
    for (ix, row) in table.rows.iter().enumerate() {
        if ix > 0 {
            out.push('\n');
        }
        push_tsv_field(&mut out, row.get(col).and_then(|c| c.as_deref()));
    }
    out
}

/// A single value as it is copied: `NULL` is spelled out.
pub fn value_to_text(value: Option<&str>) -> &str {
    value.unwrap_or(NULL_TEXT)
}

/// Column names with duplicates disambiguated: `id, id, id` becomes
/// `id, id (2), id (3)`. A generated name never collides with a column that
/// really has that name.
pub fn unique_column_names(columns: &[String]) -> Vec<String> {
    let originals: HashSet<&str> = columns.iter().map(String::as_str).collect();
    let mut used: HashSet<String> = HashSet::with_capacity(columns.len());
    let mut names = Vec::with_capacity(columns.len());
    for name in columns {
        let unique = if used.contains(name.as_str()) {
            (2..)
                .map(|n| format!("{name} ({n})"))
                .find(|candidate| {
                    !used.contains(candidate) && !originals.contains(candidate.as_str())
                })
                .unwrap_or_else(|| name.clone())
        } else {
            name.clone()
        };
        used.insert(unique.clone());
        names.push(unique);
    }
    names
}

// ---------------------------------------------------------------------------

/// The part of a result set being rendered.
struct Table<'a> {
    columns: &'a [String],
    rows: Vec<&'a [Option<String>]>,
}

impl<'a> Table<'a> {
    fn new(result: &'a ResultSet, order: Option<&[usize]>) -> Self {
        let rows = match order {
            None => result.rows.iter().map(Vec::as_slice).collect(),
            Some(order) => order
                .iter()
                .filter_map(|&ix| result.rows.get(ix).map(Vec::as_slice))
                .collect(),
        };
        Self {
            columns: &result.columns,
            rows,
        }
    }

    fn csv(&self) -> String {
        let mut out = String::new();
        for (ix, name) in self.columns.iter().enumerate() {
            if ix > 0 {
                out.push(',');
            }
            push_csv_field(&mut out, Some(name));
        }
        out.push_str("\r\n");
        for row in &self.rows {
            for (ix, cell) in row.iter().enumerate() {
                if ix > 0 {
                    out.push(',');
                }
                push_csv_field(&mut out, cell.as_deref());
            }
            out.push_str("\r\n");
        }
        out
    }

    fn tsv(&self) -> String {
        let mut out = String::new();
        for (ix, name) in self.columns.iter().enumerate() {
            if ix > 0 {
                out.push('\t');
            }
            push_tsv_field(&mut out, Some(name));
        }
        out.push('\n');
        for row in &self.rows {
            for (ix, cell) in row.iter().enumerate() {
                if ix > 0 {
                    out.push('\t');
                }
                push_tsv_field(&mut out, cell.as_deref());
            }
            out.push('\n');
        }
        out
    }

    fn json(&self) -> String {
        if self.rows.is_empty() {
            return "[]".to_string();
        }
        let names = unique_column_names(self.columns);
        let mut out = String::from("[\n");
        for (ix, row) in self.rows.iter().enumerate() {
            if ix > 0 {
                out.push_str(",\n");
            }
            out.push_str("  ");
            push_json_object(&mut out, &names, row, 1);
        }
        out.push_str("\n]");
        out
    }

    fn markdown(&self) -> String {
        if self.columns.is_empty() {
            return String::new();
        }
        let mut out = String::from("|");
        for name in self.columns {
            out.push(' ');
            push_markdown_cell(&mut out, name);
            out.push_str(" |");
        }
        out.push_str("\n|");
        for _ in self.columns {
            out.push_str(" --- |");
        }
        out.push('\n');
        for row in &self.rows {
            out.push('|');
            for cell in row.iter() {
                out.push(' ');
                push_markdown_cell(&mut out, cell.as_deref().unwrap_or(NULL_TEXT));
                out.push_str(" |");
            }
            out.push('\n');
        }
        out
    }
}

/// RFC 4180 field. `None` (NULL) is empty; `Some("")` is `""`.
fn push_csv_field(out: &mut String, value: Option<&str>) {
    match value {
        None => {}
        Some("") => out.push_str("\"\""),
        Some(value) if value.contains([',', '"', '\r', '\n']) => push_quoted(out, value),
        Some(value) => out.push_str(value),
    }
}

/// Spreadsheet-clipboard field: quoted only when it would break the grid.
fn push_tsv_field(out: &mut String, value: Option<&str>) {
    match value {
        None => {}
        Some(value) if value.contains(['\t', '"', '\r', '\n']) => push_quoted(out, value),
        Some(value) => out.push_str(value),
    }
}

fn push_quoted(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
}

fn push_markdown_cell(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '|' => out.push_str("\\|"),
            '\r' => {}
            '\n' => out.push_str("<br>"),
            ch => out.push(ch),
        }
    }
}

/// A JSON object whose closing brace is indented for nesting `depth`; the
/// opening brace is emitted at the current position.
fn push_json_object(out: &mut String, names: &[String], cells: &[Option<String>], depth: usize) {
    if names.is_empty() {
        out.push_str("{}");
        return;
    }
    let inner = "  ".repeat(depth + 1);
    let outer = "  ".repeat(depth);
    out.push_str("{\n");
    for (ix, name) in names.iter().enumerate() {
        if ix > 0 {
            out.push_str(",\n");
        }
        out.push_str(&inner);
        push_json_string(out, name);
        out.push_str(": ");
        match cells.get(ix).and_then(|cell| cell.as_deref()) {
            Some(value) => push_json_string(out, value),
            None => out.push_str("null"),
        }
    }
    out.push('\n');
    out.push_str(&outer);
    out.push('}');
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => {
                // Writing to a String cannot fail.
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::time::Duration;

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

    fn sample() -> ResultSet {
        result(
            &["id", "name", "note"],
            &[
                &[Some("1"), Some("Ada, Countess"), None],
                &[Some("2"), Some("Say \"hi\""), Some("")],
                &[Some("3"), Some("line1\nline2"), Some("tab\there")],
            ],
        )
    }

    #[rstest]
    #[case(None, "")]
    #[case(Some(""), "\"\"")]
    #[case(Some("plain"), "plain")]
    #[case(Some("a,b"), "\"a,b\"")]
    #[case(Some("say \"hi\""), "\"say \"\"hi\"\"\"")]
    #[case(Some("a\nb"), "\"a\nb\"")]
    #[case(Some("a\r\nb"), "\"a\r\nb\"")]
    #[case(Some(" padded "), " padded ")]
    #[case(Some("héllo → 世界"), "héllo → 世界")]
    fn csv_fields(#[case] value: Option<&str>, #[case] expected: &str) {
        let mut out = String::new();
        push_csv_field(&mut out, value);
        assert_eq!(out, expected);
    }

    #[rstest]
    #[case(None, "")]
    #[case(Some(""), "")]
    #[case(Some("plain, with comma"), "plain, with comma")]
    #[case(Some("a\tb"), "\"a\tb\"")]
    #[case(Some("a\nb"), "\"a\nb\"")]
    #[case(Some("5\" pipe"), "\"5\"\" pipe\"")]
    fn tsv_fields(#[case] value: Option<&str>, #[case] expected: &str) {
        let mut out = String::new();
        push_tsv_field(&mut out, value);
        assert_eq!(out, expected);
    }

    #[test]
    fn csv_has_crlf_header_and_distinguishes_null_from_empty() {
        // NULL is an empty field, the empty string is `""`
        assert_eq!(
            to_csv(&sample()),
            "id,name,note\r\n\
             1,\"Ada, Countess\",\r\n\
             2,\"Say \"\"hi\"\"\",\"\"\r\n\
             3,\"line1\nline2\",tab\there\r\n"
        );
    }

    #[test]
    fn tsv_quotes_only_what_breaks_the_grid() {
        assert_eq!(
            to_tsv(&sample()),
            "id\tname\tnote\n\
             1\tAda, Countess\t\n\
             2\t\"Say \"\"hi\"\"\"\t\n\
             3\t\"line1\nline2\"\t\"tab\there\"\n"
        );
    }

    #[test]
    fn json_is_an_array_of_objects_with_nulls_and_string_values() {
        let json = to_json(&sample());
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(
            parsed,
            serde_json::json!([
                {"id": "1", "name": "Ada, Countess", "note": null},
                {"id": "2", "name": "Say \"hi\"", "note": ""},
                {"id": "3", "name": "line1\nline2", "note": "tab\there"},
            ])
        );
        // keys keep the column order, which serde_json's Value cannot show
        assert!(json.starts_with(
            "[\n  {\n    \"id\": \"1\",\n    \"name\": \"Ada, Countess\",\n    \"note\": null\n  },"
        ));
        assert!(json.ends_with("\n  }\n]"));
    }

    #[test]
    fn json_disambiguates_duplicate_columns() {
        let dup = result(
            &["id", "id", "id", "id (2)"],
            &[&[Some("a"), Some("b"), None, Some("d")]],
        );
        let json = to_json(&dup);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(
            parsed,
            serde_json::json!([{"id": "a", "id (3)": "b", "id (4)": null, "id (2)": "d"}])
        );
    }

    #[rstest]
    #[case(&["a", "b"], &["a", "b"])]
    #[case(&["id", "id"], &["id", "id (2)"])]
    #[case(&["id", "id", "id"], &["id", "id (2)", "id (3)"])]
    #[case(&["a", "a", "a (2)"], &["a", "a (3)", "a (2)"])]
    #[case(&[], &[])]
    fn unique_names(#[case] input: &[&str], #[case] expected: &[&str]) {
        let input: Vec<String> = input.iter().map(|s| s.to_string()).collect();
        assert_eq!(unique_column_names(&input), expected);
    }

    #[rstest]
    #[case("plain")]
    #[case("quote \" and backslash \\")]
    #[case("newline\n tab\t cr\r")]
    #[case("control \u{1} \u{1f} \u{8} \u{c}")]
    #[case("unicode é 世界 🚀 \u{2028}")]
    #[case("")]
    fn json_strings_match_serde_json(#[case] value: &str) {
        let mut out = String::new();
        push_json_string(&mut out, value);
        let back: String = serde_json::from_str(&out).expect("valid json string");
        assert_eq!(back, value);
    }

    #[test]
    fn json_of_empty_result_is_an_empty_array() {
        assert_eq!(to_json(&result(&["a"], &[])), "[]");
        assert_eq!(to_json(&result(&[], &[])), "[]");
    }

    #[test]
    fn json_of_rows_without_columns_is_empty_objects() {
        let r = ResultSet {
            rows: vec![vec![], vec![]],
            ..result(&[], &[])
        };
        assert_eq!(to_json(&r), "[\n  {},\n  {}\n]");
    }

    #[test]
    fn markdown_escapes_pipes_and_line_breaks_and_spells_out_null() {
        let r = result(
            &["a|b", "c"],
            &[&[Some("x | y"), None], &[Some("l1\nl2"), Some("")]],
        );
        assert_eq!(
            to_markdown(&r),
            "| a\\|b | c |\n| --- | --- |\n| x \\| y | NULL |\n| l1<br>l2 |  |\n"
        );
    }

    #[test]
    fn markdown_without_columns_is_empty() {
        assert_eq!(to_markdown(&result(&[], &[])), "");
    }

    #[test]
    fn header_only_results_still_render_the_header() {
        let r = result(&["a", "b"], &[]);
        assert_eq!(to_csv(&r), "a,b\r\n");
        assert_eq!(to_tsv(&r), "a\tb\n");
        assert_eq!(to_markdown(&r), "| a | b |\n| --- | --- |\n");
    }

    #[test]
    fn row_order_is_respected_and_out_of_range_is_skipped() {
        let r = result(&["n"], &[&[Some("a")], &[Some("b")], &[Some("c")]]);
        assert_eq!(ExportFormat::Tsv.render(&r, Some(&[2, 0, 9])), "n\nc\na\n");
        assert_eq!(ExportFormat::Csv.render(&r, Some(&[])), "n\r\n");
    }

    #[test]
    fn single_row_variants() {
        let r = sample();
        assert_eq!(row_to_tsv(&r, 0).as_deref(), Some("1\tAda, Countess\t"));
        assert_eq!(
            row_to_tsv(&r, 2).as_deref(),
            Some("3\t\"line1\nline2\"\t\"tab\there\"")
        );
        assert_eq!(row_to_tsv(&r, 3), None);

        let json = row_to_json(&r, 1).expect("row exists");
        assert_eq!(
            json,
            "{\n  \"id\": \"2\",\n  \"name\": \"Say \\\"hi\\\"\",\n  \"note\": \"\"\n}"
        );
        assert_eq!(row_to_json(&r, 3), None);
    }

    #[test]
    fn column_values_one_per_line() {
        let r = sample();
        assert_eq!(column_to_text(&r, 0, None), "1\n2\n3");
        assert_eq!(column_to_text(&r, 2, Some(&[2, 0])), "\"tab\there\"\n");
        assert_eq!(column_to_text(&r, 0, Some(&[2, 0])), "3\n1");
    }

    #[test]
    fn copied_values_spell_out_null() {
        assert_eq!(value_to_text(None), "NULL");
        assert_eq!(value_to_text(Some("")), "");
        assert_eq!(value_to_text(Some("x")), "x");
    }

    #[rstest]
    #[case(ExportFormat::Csv, "CSV", "csv")]
    #[case(ExportFormat::Tsv, "TSV", "tsv")]
    #[case(ExportFormat::Json, "JSON", "json")]
    #[case(ExportFormat::Markdown, "Markdown", "md")]
    fn format_names(#[case] format: ExportFormat, #[case] label: &str, #[case] extension: &str) {
        assert_eq!(format.label(), label);
        assert_eq!(format.extension(), extension);
        assert!(ExportFormat::ALL.contains(&format));
    }
}
