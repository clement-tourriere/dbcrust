//! One-shot output formats. Mirrors DBCrust's own one-shot contract
//! (`dbcrust agents`): `json` is a single-line envelope, `jsonl` one object
//! per row plus a trailing `_truncated` marker, `csv` has a header row,
//! `table` is plain aligned text.

use std::fmt::Write as _;

use dbcrust_agent_protocol::{Access, ColumnInfo, ConnectionSummary, TableSummary};
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::client::ResultSet;
use crate::error::AgentError;

/// Whether errors for this format are machine-readable JSON.
pub fn json_errors(format: OutputFormat) -> bool {
    matches!(format, OutputFormat::Json | OutputFormat::Jsonl)
}

/// Render a result set (always ends with a newline, except for an empty
/// `csv`/`jsonl` result).
pub fn render_result(result: &ResultSet, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => {
            let mut out = serde_json::to_string(result).unwrap_or_default();
            out.push('\n');
            out
        }
        OutputFormat::Jsonl => render_jsonl(result),
        OutputFormat::Csv => render_csv(result),
        OutputFormat::Table => render_table(result),
    }
}

fn render_jsonl(result: &ResultSet) -> String {
    // Built by hand: a serde_json map may sort its keys, and jsonl promises
    // column order
    let keys: Vec<String> = result
        .columns
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .collect();
    let mut out = String::new();
    for row in &result.rows {
        out.push('{');
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(key);
            out.push(':');
            match row.get(i).and_then(|cell| cell.as_deref()) {
                Some(value) => {
                    out.push_str(&serde_json::to_string(value).unwrap_or_default());
                }
                None => out.push_str("null"),
            }
        }
        out.push_str("}\n");
    }
    if result.truncated {
        let _ = writeln!(
            out,
            r#"{{"_truncated":true,"_row_count":{}}}"#,
            result.row_count
        );
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn render_csv(result: &ResultSet) -> String {
    if result.columns.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let header: Vec<String> = result.columns.iter().map(|c| csv_field(c)).collect();
    out.push_str(&header.join(","));
    out.push('\n');
    for row in &result.rows {
        let line: Vec<String> = row
            .iter()
            .map(|cell| cell.as_deref().map(csv_field).unwrap_or_default())
            .collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

fn cell_text(cell: &Option<String>) -> String {
    match cell {
        None => "NULL".to_string(),
        Some(value) => value
            .replace('\r', "")
            .replace('\n', "\\n")
            .replace('\t', " "),
    }
}

fn render_table(result: &ResultSet) -> String {
    let footer = if result.truncated {
        format!("({} rows, truncated)\n", result.row_count)
    } else if result.row_count == 1 {
        "(1 row)\n".to_string()
    } else {
        format!("({} rows)\n", result.row_count)
    };
    if result.columns.is_empty() {
        return footer;
    }
    let rows: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| {
            (0..result.columns.len())
                .map(|i| cell_text(row.get(i).unwrap_or(&None)))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = result.columns.iter().map(|c| c.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let pad = |text: &str, width: usize| {
        let missing = width.saturating_sub(text.chars().count());
        format!("{text}{}", " ".repeat(missing))
    };
    let line = |cells: &[String]| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| pad(c, widths[i]))
            .collect();
        format!(" {}\n", parts.join(" | ").trim_end())
    };
    let mut out = line(&result.columns);
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
    out.push_str(&rule.join("+"));
    out.push('\n');
    for row in &rows {
        out.push_str(&line(row));
    }
    out.push_str(&footer);
    out
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: ErrorBody<'a>,
}

/// The JSON error envelope (single line, no trailing newline).
pub fn error_json(code: &str, message: &str) -> String {
    serde_json::to_string(&ErrorEnvelope {
        error: ErrorBody { code, message },
    })
    .unwrap_or_default()
}

/// The stderr text for a failed one-shot command (ends with a newline).
pub fn render_error(error: &AgentError, format: OutputFormat) -> String {
    if json_errors(format) {
        format!("{}\n", error_json(error.code.as_str(), &error.message))
    } else {
        format!("error: {}\n", error.message)
    }
}

fn access_str(access: Access) -> &'static str {
    match access {
        Access::ReadOnly => "read_only",
        Access::ReadWrite => "read_write",
    }
}

fn some(value: &str) -> Option<String> {
    Some(value.to_string())
}

fn result(columns: &[&str], rows: Vec<Vec<Option<String>>>) -> ResultSet {
    ResultSet {
        columns: columns.iter().map(|c| c.to_string()).collect(),
        row_count: rows.len() as u64,
        rows,
        truncated: false,
    }
}

/// `list` as a result set.
pub fn connections_result(connections: &[ConnectionSummary]) -> ResultSet {
    result(
        &[
            "alias",
            "engine",
            "access",
            "native_read_only",
            "max_rows",
            "timeout_secs",
            "description",
        ],
        connections
            .iter()
            .map(|c| {
                vec![
                    some(&c.alias),
                    some(&c.engine),
                    some(access_str(c.access)),
                    Some(c.native_read_only.to_string()),
                    Some(c.max_rows.to_string()),
                    Some(c.timeout_secs.to_string()),
                    c.description.clone(),
                ]
            })
            .collect(),
    )
}

/// `tables` as a result set.
pub fn tables_result(tables: &[TableSummary]) -> ResultSet {
    result(
        &["schema", "name", "kind"],
        tables
            .iter()
            .map(|t| vec![t.schema.clone(), some(&t.name), t.kind.clone()])
            .collect(),
    )
}

/// `describe` as a result set.
pub fn columns_result(columns: &[ColumnInfo]) -> ResultSet {
    result(
        &["name", "data_type", "nullable", "default", "primary_key"],
        columns
            .iter()
            .map(|c| {
                vec![
                    some(&c.name),
                    some(&c.data_type),
                    c.nullable.map(|n| n.to_string()),
                    c.default.clone(),
                    Some(c.primary_key.to_string()),
                ]
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use dbcrust_agent_protocol::ErrorCode;
    use serde_json::{Value, json};

    fn sample(truncated: bool) -> ResultSet {
        ResultSet {
            columns: vec!["id".into(), "name".into(), "note".into()],
            rows: vec![
                vec![Some("1".into()), Some("Alice".into()), None],
                vec![
                    Some("2".into()),
                    Some("a,\"b\"".into()),
                    Some("x\ny".into()),
                ],
            ],
            row_count: 2,
            truncated,
        }
    }

    #[test]
    fn json_envelope_is_one_line_in_contract_order() {
        let out = render_result(&sample(false), OutputFormat::Json);
        assert_eq!(out.matches('\n').count(), 1);
        assert!(out.ends_with('\n'));
        assert!(out.starts_with(r#"{"columns":["id","name","note"],"rows":[["1","Alice",null]"#));
        let v: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["row_count"], 2);
        assert_eq!(v["truncated"], false);
        assert_eq!(v["rows"][0][2], Value::Null);
        assert_eq!(v["rows"][1][2], json!("x\ny"));
    }

    #[test]
    fn jsonl_rows_keep_column_order_and_nulls() {
        let out = render_result(&sample(false), OutputFormat::Jsonl);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], r#"{"id":"1","name":"Alice","note":null}"#);
        assert_eq!(lines[1], r#"{"id":"2","name":"a,\"b\"","note":"x\ny"}"#);
    }

    #[test]
    fn jsonl_truncation_marker() {
        let out = render_result(&sample(true), OutputFormat::Jsonl);
        assert_eq!(
            out.lines().last().unwrap(),
            r#"{"_truncated":true,"_row_count":2}"#
        );
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn csv_quotes_and_nulls() {
        let out = render_result(&sample(false), OutputFormat::Csv);
        assert_eq!(out, "id,name,note\n1,Alice,\n2,\"a,\"\"b\"\"\",\"x\ny\"\n");
    }

    #[test]
    fn table_is_aligned() {
        let out = render_result(&sample(false), OutputFormat::Table);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], " id | name  | note");
        assert_eq!(lines[1], "----+-------+------");
        assert_eq!(lines[2], " 1  | Alice | NULL");
        assert_eq!(lines[3], r#" 2  | a,"b" | x\ny"#);
        assert_eq!(lines[4], "(2 rows)");
    }

    #[test]
    fn table_footer_variants() {
        let mut one = sample(false);
        one.rows.truncate(1);
        one.row_count = 1;
        assert!(render_result(&one, OutputFormat::Table).ends_with("(1 row)\n"));
        assert!(
            render_result(&sample(true), OutputFormat::Table).ends_with("(2 rows, truncated)\n")
        );
    }

    #[test]
    fn empty_columns_do_not_panic() {
        let empty = ResultSet {
            columns: vec![],
            rows: vec![],
            row_count: 0,
            truncated: false,
        };
        assert_eq!(render_result(&empty, OutputFormat::Csv), "");
        assert_eq!(render_result(&empty, OutputFormat::Jsonl), "");
        assert_eq!(render_result(&empty, OutputFormat::Table), "(0 rows)\n");
        assert_eq!(
            render_result(&empty, OutputFormat::Json),
            "{\"columns\":[],\"rows\":[],\"row_count\":0,\"truncated\":false}\n"
        );
    }

    #[test]
    fn errors_are_json_for_json_formats_and_plain_otherwise() {
        let error = AgentError::new(ErrorCode::ReadOnlyViolation, "writes are refused");
        assert_eq!(error.code, Code::Remote(ErrorCode::ReadOnlyViolation));
        for format in [OutputFormat::Json, OutputFormat::Jsonl] {
            let text = render_error(&error, format);
            let v: Value = serde_json::from_str(text.trim()).unwrap();
            assert_eq!(v["error"]["code"], "read_only_violation");
            assert_eq!(v["error"]["message"], "writes are refused");
        }
        for format in [OutputFormat::Csv, OutputFormat::Table] {
            assert_eq!(render_error(&error, format), "error: writes are refused\n");
        }
    }

    #[test]
    fn listing_results() {
        let connections = connections_result(&[ConnectionSummary {
            alias: "prod".into(),
            engine: "postgresql".into(),
            access: Access::ReadOnly,
            native_read_only: true,
            description: None,
            max_rows: 500,
            timeout_secs: 30,
        }]);
        assert_eq!(connections.rows[0][2].as_deref(), Some("read_only"));
        assert_eq!(connections.rows[0][6], None);
        let tables = tables_result(&[TableSummary {
            schema: Some("public".into()),
            name: "users".into(),
            kind: None,
        }]);
        assert_eq!(tables.row_count, 1);
        let columns = columns_result(&[ColumnInfo {
            name: "id".into(),
            data_type: "int".into(),
            nullable: Some(false),
            default: None,
            primary_key: true,
        }]);
        assert_eq!(columns.rows[0][2].as_deref(), Some("false"));
        assert_eq!(columns.rows[0][4].as_deref(), Some("true"));
    }
}
