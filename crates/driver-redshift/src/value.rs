//! Result cells.
//!
//! Statements run through the simple query protocol, so the server sends every
//! value in its text form and sqlx never has to know the column's type: a
//! `SUPER` (JSON text), `VARBYTE` (hex), `GEOMETRY`/`GEOGRAPHY` (hex EWKB),
//! `HLLSKETCH` or `TIMETZ` column renders exactly as the server printed it,
//! where decoding through sqlx's type tables would need a `pg_type` lookup
//! Redshift cannot answer. The one translation is the boolean, which the wire
//! sends as `t`/`f`.

use dbcrust_core::database::StructuredQueryResult;
use sqlx::postgres::PgRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// The text of `bytes` as a column of `type_name` prints it.
pub(crate) fn render(type_name: &str, bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if type_name.eq_ignore_ascii_case("BOOL") => match text {
            "t" => "true".to_string(),
            "f" => "false".to_string(),
            other => other.to_string(),
        },
        Ok(text) => text.to_string(),
        // Not text: show the bytes the way `bytea` does rather than lose them
        Err(_) => {
            let mut hex = String::with_capacity(2 + bytes.len() * 2);
            hex.push_str("\\x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02x}"));
            }
            hex
        }
    }
}

/// Cell `index` of `row`: `None` exactly when the value is SQL NULL.
pub(crate) fn cell(row: &PgRow, index: usize) -> Option<String> {
    let raw = row.try_get_raw(index).ok()?;
    if raw.is_null() {
        return None;
    }
    let bytes = raw.as_bytes().ok()?;
    Some(render(row.column(index).type_info().name(), bytes))
}

/// A result set as a table (no rows at all is the empty default, like the
/// other SQL drivers).
pub(crate) fn rows_to_result(rows: &[PgRow]) -> StructuredQueryResult {
    let Some(first) = rows.first() else {
        return StructuredQueryResult::default();
    };
    let columns = (0..first.len())
        .map(|i| first.column(i).name().to_string())
        .collect();
    let rows = rows
        .iter()
        .map(|row| (0..row.len()).map(|i| cell(row, i)).collect())
        .collect();
    StructuredQueryResult { columns, rows }
}

/// The plan lines of an `EXPLAIN` result (one text row per line), header first.
pub(crate) fn plan_table(result: StructuredQueryResult) -> Vec<Vec<String>> {
    let header = result
        .columns
        .first()
        .cloned()
        .unwrap_or_else(|| "QUERY PLAN".to_string());
    let mut table = vec![vec![header]];
    table.extend(
        result
            .rows
            .into_iter()
            .map(|row| vec![row.into_iter().next().flatten().unwrap_or_default()]),
    );
    table
}
