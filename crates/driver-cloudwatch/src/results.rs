//! Logs Insights results → a table.

use aws_sdk_cloudwatchlogs::types::ResultField;
use dbcrust_core::database::StructuredQueryResult;
use std::collections::HashMap;

/// The field Insights adds to every row to fetch the whole record (`GetLogRecord`):
/// an opaque pointer, not data.
pub(crate) const POINTER_FIELD: &str = "@ptr";

/// Rows of `(field, value)` pairs → columns in first-seen order. `@ptr` is
/// hidden. A field a row does not have is a SQL NULL. A name that appears twice
/// in one row (`stats count(*), count(*)` style outputs) keeps both: the later
/// ones become `name_2`, `name_3`.
pub(crate) fn map_results(rows: &[Vec<ResultField>]) -> StructuredQueryResult {
    let mut columns: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut mapped: Vec<Vec<(usize, Option<String>)>> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        let mut cells = Vec::with_capacity(row.len());
        for field in row {
            let Some(name) = field.field() else { continue };
            if name == POINTER_FIELD {
                continue;
            }
            let occurrence = seen.entry(name).or_insert(0);
            *occurrence += 1;
            let column = if *occurrence == 1 {
                name.to_string()
            } else {
                format!("{name}_{occurrence}")
            };
            let position = *index.entry(column.clone()).or_insert_with(|| {
                columns.push(column);
                columns.len() - 1
            });
            cells.push((position, field.value().map(str::to_string)));
        }
        mapped.push(cells);
    }
    let width = columns.len();
    let rows = mapped
        .into_iter()
        .map(|cells| {
            let mut row: Vec<Option<String>> = vec![None; width];
            for (position, value) in cells {
                row[position] = value;
            }
            row
        })
        .collect();
    StructuredQueryResult { columns, rows }
}

/// `12345678` → `11.8 MiB`.
pub(crate) fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", value as u64)
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Epoch milliseconds as `2026-10-16T08:00:00.123Z`.
pub(crate) fn timestamp_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn field(name: &str, value: Option<&str>) -> ResultField {
        let mut builder = ResultField::builder().field(name);
        if let Some(value) = value {
            builder = builder.value(value);
        }
        builder.build()
    }

    #[test]
    fn columns_follow_the_first_row_and_the_pointer_is_hidden() {
        let rows = vec![
            vec![
                field("@timestamp", Some("2026-10-16 08:00:00.000")),
                field("@message", Some("hello")),
                field("@ptr", Some("CmUKJQo")),
            ],
            vec![
                field("@timestamp", Some("2026-10-16 08:00:01.000")),
                field("@message", Some("world")),
                field("@ptr", Some("CmUKJQp")),
            ],
        ];
        let result = map_results(&rows);
        assert_eq!(result.columns, ["@timestamp", "@message"]);
        assert_eq!(result.rows[1][1].as_deref(), Some("world"));
    }

    #[test]
    fn ragged_rows_become_nulls_and_new_fields_append() {
        let rows = vec![
            vec![field("a", Some("1"))],
            vec![field("b", Some("2")), field("a", Some("3"))],
            vec![],
        ];
        let result = map_results(&rows);
        assert_eq!(result.columns, ["a", "b"]);
        assert_eq!(result.rows[0], [Some("1".into()), None]);
        assert_eq!(result.rows[1], [Some("3".into()), Some("2".into())]);
        assert_eq!(result.rows[2], [None, None]);
    }

    #[test]
    fn a_name_twice_in_a_row_keeps_both() {
        let rows = vec![vec![
            field("count(*)", Some("5")),
            field("count(*)", Some("7")),
            field("count(*)", Some("9")),
        ]];
        let result = map_results(&rows);
        assert_eq!(result.columns, ["count(*)", "count(*)_2", "count(*)_3"]);
        assert_eq!(result.rows[0][2].as_deref(), Some("9"));
    }

    #[test]
    fn empty_values_and_missing_values_differ() {
        let rows = vec![vec![field("a", Some("")), field("b", None)]];
        let result = map_results(&rows);
        assert_eq!(result.rows[0], [Some(String::new()), None]);
    }

    #[test]
    fn no_rows_no_columns() {
        let result = map_results(&[]);
        assert!(result.columns.is_empty() && result.rows.is_empty());
    }

    #[test]
    fn huge_cells_pass_through() {
        let big = "x".repeat(300_000);
        let result = map_results(&[vec![field("@message", Some(&big))]]);
        assert_eq!(result.rows[0][0].as_deref().map(str::len), Some(300_000));
    }

    #[rstest]
    #[case(0.0, "0 B")]
    #[case(1023.0, "1023 B")]
    #[case(1536.0, "1.50 KiB")]
    #[case(5.0 * 1024.0 * 1024.0, "5.00 MiB")]
    #[case(3.0 * 1024.0 * 1024.0 * 1024.0, "3.00 GiB")]
    fn sizes(#[case] bytes: f64, #[case] expected: &str) {
        assert_eq!(human_bytes(bytes), expected);
    }

    #[rstest]
    #[case(0, "1970-01-01T00:00:00.000Z")]
    #[case(1_792_137_600_123, "2026-10-16T08:00:00.123Z")]
    fn timestamps(#[case] ms: i64, #[case] expected: &str) {
        assert_eq!(timestamp_ms(ms), expected);
    }
}
