//! RESP replies → `StructuredQueryResult` tables.
//!
//! Redis replies are not tabular, so the mapping is by *shape*:
//!
//! | reply | columns |
//! |---|---|
//! | scalar (string, integer, double, boolean, `OK`) | `value` |
//! | nil | `value` = NULL |
//! | array / set of scalars | `index`, `value` (set: `value`) |
//! | map, or a flat pair list of `HGETALL`/`CONFIG GET`/`XINFO STREAM` | `field`, `value` |
//! | `… WITHSCORES` | `member`, `score` |
//! | `SCAN` / `SSCAN` | `cursor`, `value` |
//! | `HSCAN` / `ZSCAN` | `cursor`, `field`, `value` / `cursor`, `member`, `score` |
//! | `XRANGE` / `XREVRANGE` | `id`, `field`, `value` |
//! | `XREAD` | `stream`, `id`, `field`, `value` |
//! | `XINFO GROUPS` / `CONSUMERS` | one column per field |
//! | `INFO` | `section`, `key`, `value` |
//! | anything nested | the nested part as JSON text in a `value` cell |
//!
//! Rows are capped at `max_rows`; the mapping reports whether it cut.

use dbcrust_core::database::StructuredQueryResult;
use redis::Value;
use serde_json::Value as Json;

/// How a command's reply is laid out as a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Generic,
    /// Flat `[field, value, …]` (RESP2) or a map (RESP3)
    Pairs,
    /// `[member, score, …]`
    ScoredMembers,
    /// `[cursor, [key, …]]`
    ScanKeys,
    /// `[cursor, [field, value, …]]`
    ScanFields,
    /// `[cursor, [member, score, …]]`
    ScanMembers,
    /// `[[id, [field, value, …]], …]`
    StreamEntries,
    /// `[[stream, [[id, [field, value, …]], …]], …]`
    StreamRead,
    /// `[[k, v, k, v…], …]`: one row per record
    Records,
    /// the `INFO` text
    Info,
}

/// The layout of the reply to the (already split) command `args`.
pub(crate) fn shape_for(args: &[Vec<u8>]) -> Shape {
    let command = dbcrust_core::redis_command::command_name(args);
    let sub = dbcrust_core::redis_command::subcommand_name(args);
    let has = |wanted: &str| {
        args.iter()
            .skip(1)
            .any(|arg| String::from_utf8_lossy(arg).eq_ignore_ascii_case(wanted))
    };
    match command.as_str() {
        "HGETALL" => Shape::Pairs,
        "HRANDFIELD" if has("WITHVALUES") => Shape::Pairs,
        "CONFIG" if sub.as_deref() == Some("GET") => Shape::Pairs,
        "XINFO" => match sub.as_deref() {
            Some("STREAM") => Shape::Pairs,
            Some("GROUPS" | "CONSUMERS") => Shape::Records,
            _ => Shape::Generic,
        },
        "ZRANGE" | "ZRANGEBYSCORE" | "ZREVRANGE" | "ZREVRANGEBYSCORE" | "ZRANDMEMBER"
        | "ZINTER" | "ZUNION" | "ZDIFF"
            if has("WITHSCORES") =>
        {
            Shape::ScoredMembers
        }
        "ZPOPMIN" | "ZPOPMAX" => Shape::ScoredMembers,
        "SCAN" | "SSCAN" => Shape::ScanKeys,
        "HSCAN" if has("NOVALUES") => Shape::ScanKeys,
        "HSCAN" => Shape::ScanFields,
        "ZSCAN" => Shape::ScanMembers,
        "XRANGE" | "XREVRANGE" => Shape::StreamEntries,
        "XREAD" => Shape::StreamRead,
        "INFO" => Shape::Info,
        _ => Shape::Generic,
    }
}

/// A mapped reply and whether `max_rows` cut it short.
#[derive(Debug, PartialEq)]
pub(crate) struct Mapped {
    pub result: StructuredQueryResult,
    pub truncated: bool,
}

type Row = Vec<Option<String>>;

/// Table builder enforcing the row cap.
struct Table {
    columns: Vec<String>,
    rows: Vec<Row>,
    max_rows: usize,
    truncated: bool,
}

impl Table {
    fn new(columns: &[&str], max_rows: usize) -> Self {
        Self {
            columns: columns.iter().map(|c| (*c).to_string()).collect(),
            rows: Vec::new(),
            max_rows,
            truncated: false,
        }
    }

    /// Returns false (and marks the table truncated) once the cap is reached.
    fn push(&mut self, row: Row) -> bool {
        if self.rows.len() >= self.max_rows {
            self.truncated = true;
            return false;
        }
        self.rows.push(row);
        true
    }

    fn finish(self) -> Mapped {
        Mapped {
            result: StructuredQueryResult {
                columns: self.columns,
                rows: self.rows,
            },
            truncated: self.truncated,
        }
    }
}

/// Map `value` (the reply to a command of layout `shape`) to a table.
///
/// A top-level server error is the caller's `Err`; this only fails on that.
pub(crate) fn map_reply(shape: Shape, value: &Value, max_rows: usize) -> Result<Mapped, String> {
    let value = unwrap_attribute(value);
    if let Value::ServerError(error) = value {
        return Err(error.to_string());
    }
    Ok(match shape {
        Shape::Info => map_info(value, max_rows),
        Shape::Pairs => map_pairs(value, &["field", "value"], max_rows),
        Shape::ScoredMembers => map_pairs(value, &["member", "score"], max_rows),
        Shape::ScanKeys => map_scan(value, &["cursor", "value"], 1, max_rows),
        Shape::ScanFields => map_scan(value, &["cursor", "field", "value"], 2, max_rows),
        Shape::ScanMembers => map_scan(value, &["cursor", "member", "score"], 2, max_rows),
        Shape::StreamEntries => map_stream_entries(value, max_rows),
        Shape::StreamRead => map_stream_read(value, max_rows),
        Shape::Records => map_records(value, max_rows),
        Shape::Generic => map_generic(value, max_rows),
    })
}

fn unwrap_attribute(value: &Value) -> &Value {
    match value {
        Value::Attribute { data, .. } => unwrap_attribute(data),
        other => other,
    }
}

/// `Some(cell)` for a scalar (nil is `Some(None)`), `None` for containers.
fn scalar(value: &Value) -> Option<Option<String>> {
    Some(match unwrap_attribute(value) {
        Value::Nil => None,
        Value::Int(number) => Some(number.to_string()),
        Value::BulkString(bytes) => Some(render_bytes(bytes)),
        Value::SimpleString(text) => Some(text.clone()),
        Value::Okay => Some("OK".to_string()),
        Value::Double(number) => Some(render_double(*number)),
        Value::Boolean(flag) => Some(flag.to_string()),
        Value::VerbatimString { text, .. } => Some(text.clone()),
        Value::BigNumber(digits) => Some(render_bigint(digits)),
        _ => return None,
    })
}

/// Cell text of any value: scalars as text, containers as JSON.
fn cell(value: &Value) -> Option<String> {
    match scalar(value) {
        Some(text) => text,
        None => Some(to_json(value).to_string()),
    }
}

/// Binary-safe text: valid UTF-8 as is, otherwise `redis-cli` style `\xNN`
/// escapes for the bytes that are not printable ASCII.
pub(crate) fn render_bytes(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let mut out = String::with_capacity(bytes.len());
            for &byte in bytes {
                match byte {
                    b'\\' => out.push_str("\\\\"),
                    0x20..=0x7e => out.push(byte as char),
                    _ => out.push_str(&format!("\\x{byte:02x}")),
                }
            }
            out
        }
    }
}

fn render_double(number: f64) -> String {
    if number.is_infinite() {
        if number > 0.0 { "inf" } else { "-inf" }.to_string()
    } else if number.is_nan() {
        "nan".to_string()
    } else {
        number.to_string()
    }
}

/// `BigNumber` carries its decimal digits as bytes (the crate's `num-bigint`
/// feature, which this driver does not enable, would make it a `BigInt`).
fn render_bigint(digits: &[u8]) -> String {
    String::from_utf8_lossy(digits).into_owned()
}

/// JSON rendering of nested replies.
pub(crate) fn to_json(value: &Value) -> Json {
    match value {
        Value::Nil => Json::Null,
        Value::Int(number) => Json::from(*number),
        Value::BulkString(bytes) => Json::String(render_bytes(bytes)),
        Value::SimpleString(text) => Json::String(text.clone()),
        Value::Okay => Json::String("OK".to_string()),
        Value::Double(number) => serde_json::Number::from_f64(*number)
            .map(Json::Number)
            .unwrap_or_else(|| Json::String(render_double(*number))),
        Value::Boolean(flag) => Json::Bool(*flag),
        Value::VerbatimString { text, .. } => Json::String(text.clone()),
        Value::BigNumber(digits) => Json::String(render_bigint(digits)),
        Value::Array(items) | Value::Set(items) => Json::Array(items.iter().map(to_json).collect()),
        Value::Map(pairs) => Json::Object(
            pairs
                .iter()
                .map(|(key, value)| {
                    let key = match scalar(key) {
                        Some(Some(text)) => text,
                        Some(None) => "null".to_string(),
                        None => to_json(key).to_string(),
                    };
                    (key, to_json(value))
                })
                .collect(),
        ),
        Value::Attribute { data, .. } => to_json(data),
        Value::Push { kind, data } => {
            let mut object = serde_json::Map::new();
            object.insert("push".to_string(), Json::String(format!("{kind:?}")));
            object.insert(
                "data".to_string(),
                Json::Array(data.iter().map(to_json).collect()),
            );
            Json::Object(object)
        }
        Value::ServerError(error) => Json::String(error.to_string()),
        // `Value` is #[non_exhaustive]
        other => Json::String(format!("{other:?}")),
    }
}

fn map_generic(value: &Value, max_rows: usize) -> Mapped {
    match value {
        Value::Array(items) => {
            let mut table = Table::new(&["index", "value"], max_rows);
            for (index, item) in items.iter().enumerate() {
                if !table.push(vec![Some(index.to_string()), cell(item)]) {
                    break;
                }
            }
            table.finish()
        }
        Value::Set(items) => {
            let mut table = Table::new(&["value"], max_rows);
            for item in items {
                if !table.push(vec![cell(item)]) {
                    break;
                }
            }
            table.finish()
        }
        Value::Map(pairs) => {
            let mut table = Table::new(&["field", "value"], max_rows);
            for (key, value) in pairs {
                if !table.push(vec![cell(key), cell(value)]) {
                    break;
                }
            }
            table.finish()
        }
        Value::Push { .. } => single_value(Some(to_json(value).to_string())),
        other => single_value(cell(other)),
    }
}

fn single_value(text: Option<String>) -> Mapped {
    Mapped {
        result: StructuredQueryResult {
            columns: vec!["value".to_string()],
            rows: vec![vec![text]],
        },
        truncated: false,
    }
}

/// Pairs out of a flat RESP2 array, a nested `[[a, b], …]` array or a RESP3
/// map. A trailing odd element is kept with a NULL second cell.
fn pairs_of(value: &Value) -> Option<Vec<(&Value, Option<&Value>)>> {
    match unwrap_attribute(value) {
        Value::Map(pairs) => Some(pairs.iter().map(|(k, v)| (k, Some(v))).collect()),
        Value::Array(items) | Value::Set(items) => {
            let nested = !items.is_empty()
                && items
                    .iter()
                    .all(|item| matches!(item, Value::Array(inner) if inner.len() == 2));
            if nested {
                Some(
                    items
                        .iter()
                        .map(|item| match item {
                            Value::Array(inner) => (&inner[0], Some(&inner[1])),
                            _ => unreachable!("checked above"),
                        })
                        .collect(),
                )
            } else {
                Some(
                    items
                        .chunks(2)
                        .map(|chunk| (&chunk[0], chunk.get(1)))
                        .collect(),
                )
            }
        }
        _ => None,
    }
}

fn map_pairs(value: &Value, columns: &[&str], max_rows: usize) -> Mapped {
    let Some(pairs) = pairs_of(value) else {
        return map_generic(value, max_rows);
    };
    let mut table = Table::new(columns, max_rows);
    for (first, second) in pairs {
        if !table.push(vec![cell(first), second.and_then(cell)]) {
            break;
        }
    }
    table.finish()
}

/// `[cursor, [items…]]`; `width` items make one row (1 key, 2 field+value).
fn map_scan(value: &Value, columns: &[&str], width: usize, max_rows: usize) -> Mapped {
    let (cursor, items) = match value {
        Value::Array(parts) if parts.len() == 2 => match (&parts[0], &parts[1]) {
            (cursor, Value::Array(items)) => (cell(cursor), items),
            _ => return map_generic(value, max_rows),
        },
        _ => return map_generic(value, max_rows),
    };
    let mut table = Table::new(columns, max_rows);
    if items.is_empty() {
        // An empty page still carries the cursor to continue from (a non-zero
        // cursor with no keys is normal under MATCH)
        if cursor.as_deref() != Some("0") {
            let mut row = vec![cursor];
            row.extend(std::iter::repeat_n(None, width));
            table.push(row);
        }
        return table.finish();
    }
    for chunk in items.chunks(width) {
        let mut row = vec![cursor.clone()];
        for index in 0..width {
            row.push(chunk.get(index).and_then(cell));
        }
        if !table.push(row) {
            break;
        }
    }
    table.finish()
}

/// `[[id, [field, value, …]], …]`, one row per field.
fn push_stream_entries(
    table: &mut Table,
    entries: &[Value],
    stream: Option<&Option<String>>,
) -> bool {
    for entry in entries {
        let Value::Array(parts) = entry else {
            continue;
        };
        let id = parts.first().and_then(cell);
        let fields: Vec<(Option<String>, Option<String>)> = match parts.get(1) {
            Some(Value::Array(flat)) => flat
                .chunks(2)
                .map(|chunk| (cell(&chunk[0]), chunk.get(1).and_then(cell)))
                .collect(),
            Some(Value::Map(pairs)) => pairs.iter().map(|(k, v)| (cell(k), cell(v))).collect(),
            _ => Vec::new(),
        };
        let fields = if fields.is_empty() {
            vec![(None, None)]
        } else {
            fields
        };
        for (field, value) in fields {
            let mut row = Vec::new();
            if let Some(stream) = stream {
                row.push(stream.clone());
            }
            row.extend([id.clone(), field, value]);
            if !table.push(row) {
                return false;
            }
        }
    }
    true
}

fn map_stream_entries(value: &Value, max_rows: usize) -> Mapped {
    let Value::Array(entries) = value else {
        return map_generic(value, max_rows);
    };
    let mut table = Table::new(&["id", "field", "value"], max_rows);
    push_stream_entries(&mut table, entries, None);
    table.finish()
}

fn map_stream_read(value: &Value, max_rows: usize) -> Mapped {
    // XREAD times out as nil; RESP3 answers a map of stream → entries
    let streams: Vec<(&Value, &Value)> = match value {
        Value::Nil => Vec::new(),
        Value::Map(pairs) => pairs.iter().map(|(k, v)| (k, v)).collect(),
        Value::Array(items) => {
            let mut streams = Vec::new();
            for item in items {
                match item {
                    Value::Array(parts) if parts.len() == 2 => streams.push((&parts[0], &parts[1])),
                    _ => return map_generic(value, max_rows),
                }
            }
            streams
        }
        _ => return map_generic(value, max_rows),
    };
    let mut table = Table::new(&["stream", "id", "field", "value"], max_rows);
    for (name, entries) in streams {
        if let Value::Array(entries) = entries {
            if !push_stream_entries(&mut table, entries, Some(&cell(name))) {
                break;
            }
        }
    }
    table.finish()
}

fn map_records(value: &Value, max_rows: usize) -> Mapped {
    let Value::Array(records) = value else {
        return map_generic(value, max_rows);
    };
    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<(String, Option<String>)>> = Vec::new();
    for record in records {
        let Some(pairs) = pairs_of(record) else {
            return map_generic(value, max_rows);
        };
        let mut row = Vec::new();
        for (key, value) in pairs {
            let key = cell(key).unwrap_or_default();
            if !columns.contains(&key) {
                columns.push(key.clone());
            }
            row.push((key, value.and_then(cell)));
        }
        rows.push(row);
    }
    let mut truncated = false;
    let mut table_rows: Vec<Row> = Vec::new();
    for row in rows {
        if table_rows.len() >= max_rows {
            truncated = true;
            break;
        }
        table_rows.push(
            columns
                .iter()
                .map(|column| {
                    row.iter()
                        .find(|(key, _)| key == column)
                        .and_then(|(_, value)| value.clone())
                })
                .collect(),
        );
    }
    Mapped {
        result: StructuredQueryResult {
            columns,
            rows: table_rows,
        },
        truncated,
    }
}

/// `INFO` text: `# Section` headers and `key:value` lines.
fn map_info(value: &Value, max_rows: usize) -> Mapped {
    let Some(Some(text)) = scalar(value) else {
        return map_generic(value, max_rows);
    };
    let mut table = Table::new(&["section", "key", "value"], max_rows);
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('#') {
            section = name.trim().to_string();
            continue;
        }
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        if !table.push(vec![
            Some(section.clone()),
            Some(key.to_string()),
            Some(value.to_string()),
        ]) {
            break;
        }
    }
    table.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn bulk(text: &str) -> Value {
        Value::BulkString(text.as_bytes().to_vec())
    }

    fn map(shape: Shape, value: Value) -> Mapped {
        map_reply(shape, &value, 1000).unwrap()
    }

    fn row(cells: &[Option<&str>]) -> Row {
        cells.iter().map(|c| c.map(str::to_string)).collect()
    }

    fn args(line: &str) -> Vec<Vec<u8>> {
        dbcrust_core::redis_command::split_command_line(line).unwrap()
    }

    #[test]
    fn bulk_string_is_one_value_cell() {
        let mapped = map(Shape::Generic, bulk("hello"));
        assert_eq!(mapped.result.columns, ["value"]);
        assert_eq!(mapped.result.rows, [row(&[Some("hello")])]);
        assert!(!mapped.truncated);
    }

    #[rstest]
    #[case(Value::SimpleString("PONG".into()), "PONG")]
    #[case(Value::Okay, "OK")]
    #[case(Value::Int(-42), "-42")]
    #[case(Value::Double(1.5), "1.5")]
    #[case(Value::Double(f64::INFINITY), "inf")]
    #[case(Value::Boolean(true), "true")]
    #[case(Value::BigNumber(b"123456789012345678901234567890".to_vec()), "123456789012345678901234567890")]
    #[case(Value::VerbatimString { format: redis::VerbatimFormat::Text, text: "doc".into() }, "doc")]
    fn scalars_map_to_text(#[case] value: Value, #[case] expected: &str) {
        let mapped = map(Shape::Generic, value);
        assert_eq!(mapped.result.rows, [row(&[Some(expected)])]);
    }

    #[test]
    fn nil_is_null() {
        let mapped = map(Shape::Generic, Value::Nil);
        assert_eq!(mapped.result.columns, ["value"]);
        assert_eq!(mapped.result.rows, [row(&[None])]);
    }

    #[test]
    fn array_of_scalars_has_index_and_value() {
        let mapped = map(
            Shape::Generic,
            Value::Array(vec![bulk("a"), Value::Nil, Value::Int(3)]),
        );
        assert_eq!(mapped.result.columns, ["index", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("0"), Some("a")]),
                row(&[Some("1"), None]),
                row(&[Some("2"), Some("3")]),
            ]
        );
    }

    #[test]
    fn empty_array_has_columns_and_no_rows() {
        let mapped = map(Shape::Generic, Value::Array(vec![]));
        assert_eq!(mapped.result.columns, ["index", "value"]);
        assert!(mapped.result.rows.is_empty());
    }

    #[test]
    fn set_has_a_single_value_column() {
        let mapped = map(Shape::Generic, Value::Set(vec![bulk("x"), bulk("y")]));
        assert_eq!(mapped.result.columns, ["value"]);
        assert_eq!(mapped.result.rows, [row(&[Some("x")]), row(&[Some("y")])]);
    }

    #[test]
    fn resp3_map_has_field_and_value() {
        let mapped = map(
            Shape::Generic,
            Value::Map(vec![
                (bulk("name"), bulk("ada")),
                (bulk("age"), Value::Int(36)),
            ]),
        );
        assert_eq!(mapped.result.columns, ["field", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("name"), Some("ada")]),
                row(&[Some("age"), Some("36")])
            ]
        );
    }

    #[test]
    fn nested_arrays_render_as_json_text() {
        let nested = Value::Array(vec![
            bulk("a"),
            Value::Array(vec![Value::Int(1), bulk("b")]),
        ]);
        let mapped = map(Shape::Generic, Value::Array(vec![nested]));
        assert_eq!(
            mapped.result.rows,
            [row(&[Some("0"), Some(r#"["a",[1,"b"]]"#)])]
        );
    }

    #[test]
    fn nested_maps_render_as_json_objects() {
        let value = Value::Array(vec![Value::Map(vec![(bulk("k"), Value::Boolean(false))])]);
        let mapped = map(Shape::Generic, value);
        assert_eq!(
            mapped.result.rows,
            [row(&[Some("0"), Some(r#"{"k":false}"#)])]
        );
    }

    #[test]
    fn attributes_are_dropped_in_favour_of_the_data() {
        let value = Value::Attribute {
            data: Box::new(bulk("v")),
            attributes: vec![(bulk("ttl"), Value::Int(1))],
        };
        assert_eq!(map(Shape::Generic, value).result.rows, [row(&[Some("v")])]);
    }

    #[test]
    fn push_messages_render_as_json() {
        let value = Value::Push {
            kind: redis::PushKind::Message,
            data: vec![bulk("ch"), bulk("hi")],
        };
        let mapped = map(Shape::Generic, value);
        let text = mapped.result.rows[0][0].clone().unwrap();
        assert!(
            text.contains("\"push\"") && text.contains("\"hi\""),
            "{text}"
        );
    }

    #[test]
    fn a_top_level_server_error_is_an_error() {
        let value = redis::parse_redis_value(b"-READONLY You can't write\r\n").unwrap();
        assert!(matches!(value, Value::ServerError(_)));
        let message = map_reply(Shape::Generic, &value, 10).unwrap_err();
        assert!(message.contains("write"), "{message}");
    }

    #[test]
    fn binary_values_are_escaped_not_lost() {
        let mapped = map(
            Shape::Generic,
            Value::BulkString(vec![b'a', 0x00, 0xff, b'\\']),
        );
        assert_eq!(mapped.result.rows, [row(&[Some("a\\x00\\xff\\\\")])]);
    }

    #[test]
    fn hgetall_flat_pairs() {
        let mapped = map(
            shape_for(&args("HGETALL user:1")),
            Value::Array(vec![bulk("name"), bulk("ada"), bulk("age"), bulk("36")]),
        );
        assert_eq!(mapped.result.columns, ["field", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("name"), Some("ada")]),
                row(&[Some("age"), Some("36")])
            ]
        );
    }

    #[test]
    fn zrange_withscores_flat_and_nested() {
        let shape = shape_for(&args("ZRANGE z 0 -1 WITHSCORES"));
        assert_eq!(shape, Shape::ScoredMembers);
        let flat = map(
            shape,
            Value::Array(vec![bulk("a"), bulk("1"), bulk("b"), bulk("2.5")]),
        );
        assert_eq!(flat.result.columns, ["member", "score"]);
        assert_eq!(
            flat.result.rows,
            [row(&[Some("a"), Some("1")]), row(&[Some("b"), Some("2.5")])]
        );
        let nested = map(
            shape,
            Value::Array(vec![
                Value::Array(vec![bulk("a"), Value::Double(1.0)]),
                Value::Array(vec![bulk("b"), Value::Double(2.5)]),
            ]),
        );
        // RESP3 doubles render as numbers, RESP2 scores are the server's text
        assert_eq!(
            nested.result.rows,
            [row(&[Some("a"), Some("1")]), row(&[Some("b"), Some("2.5")])]
        );
    }

    #[test]
    fn zrange_without_scores_is_a_plain_list() {
        assert_eq!(shape_for(&args("ZRANGE z 0 -1")), Shape::Generic);
    }

    #[test]
    fn scan_has_cursor_and_value() {
        let mapped = map(
            shape_for(&args("SCAN 0 MATCH user:*")),
            Value::Array(vec![
                bulk("17"),
                Value::Array(vec![bulk("user:1"), bulk("user:2")]),
            ]),
        );
        assert_eq!(mapped.result.columns, ["cursor", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("17"), Some("user:1")]),
                row(&[Some("17"), Some("user:2")])
            ]
        );
    }

    #[test]
    fn scan_finished_with_no_keys_has_no_rows() {
        let mapped = map(
            Shape::ScanKeys,
            Value::Array(vec![bulk("0"), Value::Array(vec![])]),
        );
        assert!(mapped.result.rows.is_empty());
    }

    #[test]
    fn scan_page_without_keys_keeps_the_cursor() {
        let mapped = map(
            Shape::ScanKeys,
            Value::Array(vec![bulk("42"), Value::Array(vec![])]),
        );
        assert_eq!(mapped.result.rows, [row(&[Some("42"), None])]);
    }

    #[test]
    fn hscan_has_cursor_field_value() {
        let mapped = map(
            shape_for(&args("HSCAN h 0")),
            Value::Array(vec![
                bulk("0"),
                Value::Array(vec![bulk("f1"), bulk("v1"), bulk("f2"), bulk("v2")]),
            ]),
        );
        assert_eq!(mapped.result.columns, ["cursor", "field", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("0"), Some("f1"), Some("v1")]),
                row(&[Some("0"), Some("f2"), Some("v2")]),
            ]
        );
        assert_eq!(shape_for(&args("HSCAN h 0 NOVALUES")), Shape::ScanKeys);
    }

    #[test]
    fn zscan_has_cursor_member_score() {
        let mapped = map(
            shape_for(&args("ZSCAN z 0")),
            Value::Array(vec![bulk("0"), Value::Array(vec![bulk("m"), bulk("3")])]),
        );
        assert_eq!(mapped.result.columns, ["cursor", "member", "score"]);
        assert_eq!(
            mapped.result.rows,
            [row(&[Some("0"), Some("m"), Some("3")])]
        );
    }

    #[test]
    fn xrange_has_one_row_per_field() {
        let entries = Value::Array(vec![
            Value::Array(vec![
                bulk("1-0"),
                Value::Array(vec![bulk("a"), bulk("1"), bulk("b"), bulk("2")]),
            ]),
            Value::Array(vec![bulk("2-0"), Value::Array(vec![bulk("c"), bulk("3")])]),
        ]);
        let mapped = map(shape_for(&args("XRANGE s - +")), entries);
        assert_eq!(mapped.result.columns, ["id", "field", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("1-0"), Some("a"), Some("1")]),
                row(&[Some("1-0"), Some("b"), Some("2")]),
                row(&[Some("2-0"), Some("c"), Some("3")]),
            ]
        );
    }

    #[test]
    fn xread_has_stream_id_field_value() {
        let reply = Value::Array(vec![Value::Array(vec![
            bulk("mystream"),
            Value::Array(vec![Value::Array(vec![
                bulk("1-0"),
                Value::Array(vec![bulk("f"), bulk("v")]),
            ])]),
        ])]);
        let mapped = map(shape_for(&args("XREAD STREAMS mystream 0")), reply);
        assert_eq!(mapped.result.columns, ["stream", "id", "field", "value"]);
        assert_eq!(
            mapped.result.rows,
            [row(&[Some("mystream"), Some("1-0"), Some("f"), Some("v")])]
        );
        let empty = map(Shape::StreamRead, Value::Nil);
        assert!(empty.result.rows.is_empty());
    }

    #[test]
    fn xinfo_groups_is_a_table_of_records() {
        let reply = Value::Array(vec![
            Value::Array(vec![
                bulk("name"),
                bulk("g1"),
                bulk("pending"),
                Value::Int(2),
            ]),
            Value::Array(vec![
                bulk("name"),
                bulk("g2"),
                bulk("pending"),
                Value::Int(0),
            ]),
        ]);
        let mapped = map(shape_for(&args("XINFO GROUPS s")), reply);
        assert_eq!(mapped.result.columns, ["name", "pending"]);
        assert_eq!(
            mapped.result.rows,
            [row(&[Some("g1"), Some("2")]), row(&[Some("g2"), Some("0")])]
        );
    }

    #[test]
    fn info_is_split_into_sections_and_keys() {
        let text = "# Server\r\nredis_version:7.2.4\r\nos:Linux\r\n\r\n# Keyspace\r\ndb0:keys=3,expires=0\r\n";
        let mapped = map(shape_for(&args("INFO")), bulk(text));
        assert_eq!(mapped.result.columns, ["section", "key", "value"]);
        assert_eq!(
            mapped.result.rows,
            [
                row(&[Some("Server"), Some("redis_version"), Some("7.2.4")]),
                row(&[Some("Server"), Some("os"), Some("Linux")]),
                row(&[Some("Keyspace"), Some("db0"), Some("keys=3,expires=0")]),
            ]
        );
    }

    #[test]
    fn rows_are_capped_and_flagged() {
        let items: Vec<Value> = (0..10).map(Value::Int).collect();
        let mapped = map_reply(Shape::Generic, &Value::Array(items), 4).unwrap();
        assert_eq!(mapped.result.rows.len(), 4);
        assert!(mapped.truncated);
        let exact = map_reply(
            Shape::Generic,
            &Value::Array((0..4).map(Value::Int).collect()),
            4,
        )
        .unwrap();
        assert!(!exact.truncated);
    }

    #[test]
    fn row_cap_applies_to_every_shape() {
        let pairs = Value::Array((0..20).map(|n| bulk(&n.to_string())).collect());
        let mapped = map_reply(Shape::Pairs, &pairs, 3).unwrap();
        assert_eq!(mapped.result.rows.len(), 3);
        assert!(mapped.truncated);
    }
}
