//! Arrow cells rendered the way DuckDB prints them.
//!
//! DuckDB hands results over as Arrow arrays. Scalars are formatted here, not
//! by Arrow's display, wherever DuckDB's text differs (BLOB escapes, trimmed
//! fractional seconds, `INTERVAL` words, `infinity`, `TIMESTAMPTZ` in UTC);
//! `DECIMAL` and `HUGEINT` keep every digit. Nested values (`LIST`, `ARRAY`,
//! `STRUCT`, `MAP`, `UNION`) are rendered as JSON text so a cell is both
//! readable and parseable; a SQL NULL is `None`, never an empty string.

use chrono::{DateTime, Datelike, NaiveDate, Timelike};
use duckdb::arrow::array::{
    Array, AsArray, FixedSizeBinaryArray, LargeBinaryArray, MapArray, StructArray,
};
use duckdb::arrow::datatypes::{DataType, IntervalUnit, TimeUnit};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::arrow::util::display::{ArrayFormatter, FormatOptions};
use std::fmt::Write;

/// Days between 0001-01-01 (chrono's day 1) and the Unix epoch.
const UNIX_EPOCH_DAYS_FROM_CE: i32 = 719_163;

/// The rows of `batch` as text cells (`None` = SQL NULL), appended to `rows`.
pub(crate) fn append_rows(batch: &RecordBatch, rows: &mut Vec<Vec<Option<String>>>) {
    let start = rows.len();
    rows.resize_with(start + batch.num_rows(), || {
        Vec::with_capacity(batch.num_columns())
    });
    for column in batch.columns() {
        let formatter = column_formatter(column.as_ref());
        for row in 0..batch.num_rows() {
            rows[start + row].push(cell(column.as_ref(), row, formatter.as_ref()));
        }
    }
}

/// An Arrow display formatter, for the types whose text Arrow already gets
/// right (integers, floats, decimals, booleans…).
fn column_formatter(array: &dyn Array) -> Option<ArrayFormatter<'_>> {
    ArrayFormatter::try_new(array, &FormatOptions::default()).ok()
}

fn cell(array: &dyn Array, row: usize, formatter: Option<&ArrayFormatter<'_>>) -> Option<String> {
    if array.is_null(row) {
        return None;
    }
    Some(match scalar_text(array, row) {
        Some(text) => text,
        None => match array.data_type() {
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::FixedSizeList(..)
            | DataType::Struct(_)
            | DataType::Map(..) => {
                let mut out = String::new();
                write_json(array, row, &mut out);
                out
            }
            DataType::Union(..) => return union_value(array, row),
            _ => display(formatter, array, row),
        },
    })
}

fn display(formatter: Option<&ArrayFormatter<'_>>, array: &dyn Array, row: usize) -> String {
    match formatter {
        Some(formatter) => formatter.value(row).to_string(),
        None => format!("<{}>", array.data_type()),
    }
}

/// The active member of a `UNION`, as its own cell.
fn union_value(array: &dyn Array, row: usize) -> Option<String> {
    let union = array.as_union();
    let child = union.value(row);
    let formatter = column_formatter(child.as_ref());
    cell(child.as_ref(), 0, formatter.as_ref())
}

/// DuckDB's text for the scalar types it prints differently from Arrow;
/// `None` for every other type (nested ones and the types Arrow prints right).
fn scalar_text(array: &dyn Array, row: usize) -> Option<String> {
    Some(match array.data_type() {
        DataType::Binary => blob(array.as_binary::<i32>().value(row)),
        DataType::LargeBinary => blob(
            array
                .as_any()
                .downcast_ref::<LargeBinaryArray>()?
                .value(row),
        ),
        DataType::FixedSizeBinary(_) => blob(
            array
                .as_any()
                .downcast_ref::<FixedSizeBinaryArray>()?
                .value(row),
        ),
        DataType::Date32 => date(
            array
                .as_primitive::<duckdb::arrow::datatypes::Date32Type>()
                .value(row),
        ),
        DataType::Date64 => {
            let millis = array
                .as_primitive::<duckdb::arrow::datatypes::Date64Type>()
                .value(row);
            date(i32::try_from(millis.div_euclid(86_400_000)).ok()?)
        }
        DataType::Time64(TimeUnit::Microsecond) => time(
            array
                .as_primitive::<duckdb::arrow::datatypes::Time64MicrosecondType>()
                .value(row),
            1_000,
        ),
        DataType::Time64(TimeUnit::Nanosecond) => time(
            array
                .as_primitive::<duckdb::arrow::datatypes::Time64NanosecondType>()
                .value(row),
            1,
        ),
        DataType::Timestamp(unit, zone) => {
            let (value, per_second) = match unit {
                TimeUnit::Second => (
                    array
                        .as_primitive::<duckdb::arrow::datatypes::TimestampSecondType>()
                        .value(row),
                    1,
                ),
                TimeUnit::Millisecond => (
                    array
                        .as_primitive::<duckdb::arrow::datatypes::TimestampMillisecondType>()
                        .value(row),
                    1_000,
                ),
                TimeUnit::Microsecond => (
                    array
                        .as_primitive::<duckdb::arrow::datatypes::TimestampMicrosecondType>()
                        .value(row),
                    1_000_000,
                ),
                TimeUnit::Nanosecond => (
                    array
                        .as_primitive::<duckdb::arrow::datatypes::TimestampNanosecondType>()
                        .value(row),
                    1_000_000_000,
                ),
            };
            timestamp(value, per_second, if zone.is_some() { "+00" } else { "" })
        }
        DataType::Interval(IntervalUnit::MonthDayNano) => {
            let value = array
                .as_primitive::<duckdb::arrow::datatypes::IntervalMonthDayNanoType>()
                .value(row);
            interval(value.months, value.days, value.nanoseconds)
        }
        DataType::Interval(IntervalUnit::DayTime) => {
            let value = array
                .as_primitive::<duckdb::arrow::datatypes::IntervalDayTimeType>()
                .value(row);
            interval(0, value.days, i64::from(value.milliseconds) * 1_000_000)
        }
        DataType::Interval(IntervalUnit::YearMonth) => {
            let months = array
                .as_primitive::<duckdb::arrow::datatypes::IntervalYearMonthType>()
                .value(row);
            interval(months, 0, 0)
        }
        _ => return None,
    })
}

/// A BLOB the way DuckDB prints it: printable ASCII as is, every other byte
/// (and `\`, `'`, `"`) as `\xAB`.
fn blob(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for byte in bytes {
        match byte {
            b'\\' | b'\'' | b'"' | 0..=32 | 127..=255 => {
                let _ = write!(out, "\\x{byte:02X}");
            }
            printable => out.push(char::from(*printable)),
        }
    }
    out
}

fn date(days_since_epoch: i32) -> String {
    // DuckDB's infinities are the extreme values (minus one at the low end)
    if days_since_epoch == i32::MAX {
        return "infinity".to_string();
    }
    if days_since_epoch == i32::MIN || days_since_epoch == -i32::MAX {
        return "-infinity".to_string();
    }
    match days_since_epoch
        .checked_add(UNIX_EPOCH_DAYS_FROM_CE)
        .and_then(NaiveDate::from_num_days_from_ce_opt)
    {
        Some(date) => format_date(date),
        None => days_since_epoch.to_string(),
    }
}

/// `YYYY-MM-DD`, with a `(BC)` suffix before year 1 like DuckDB.
fn format_date(date: NaiveDate) -> String {
    let (era, year) = date.year_ce();
    let mut out = format!("{year:04}-{:02}-{:02}", date.month(), date.day());
    if !era {
        out.push_str(" (BC)");
    }
    out
}

/// `HH:MM:SS[.ffffff]` from a time of day in units of `unit_nanos` ns.
fn time(value: i64, unit_nanos: i64) -> String {
    let nanos = value.saturating_mul(unit_nanos);
    let seconds = nanos.div_euclid(1_000_000_000);
    let fraction = nanos.rem_euclid(1_000_000_000);
    let mut out = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    );
    push_fraction(&mut out, fraction);
    out
}

/// A `.fff` suffix with the trailing zeros trimmed, nothing for zero.
fn push_fraction(out: &mut String, nanos: i64) {
    if nanos == 0 {
        return;
    }
    let digits = format!("{nanos:09}");
    out.push('.');
    out.push_str(digits.trim_end_matches('0'));
}

/// `value` is a count of `1 / per_second` seconds since the Unix epoch.
/// Working in seconds and a nanosecond remainder (rather than total
/// nanoseconds) keeps years far outside 1677–2262 exact.
fn timestamp(value: i64, per_second: i64, suffix: &str) -> String {
    // DuckDB's infinities are the extreme values (minus one at the low end)
    if value == i64::MAX {
        return "infinity".to_string();
    }
    if value == i64::MIN || value == -i64::MAX {
        return "-infinity".to_string();
    }
    let seconds = value.div_euclid(per_second);
    let remainder = value.rem_euclid(per_second) * (1_000_000_000 / per_second);
    let nanos = u32::try_from(remainder).unwrap_or(0);
    match DateTime::from_timestamp(seconds, 0) {
        Some(moment) => {
            let naive = moment.naive_utc();
            let mut out = format!(
                "{} {:02}:{:02}:{:02}",
                format_date(naive.date()),
                naive.hour(),
                naive.minute(),
                naive.second()
            );
            push_fraction(&mut out, i64::from(nanos));
            out.push_str(suffix);
            out
        }
        None => value.to_string(),
    }
}

/// An INTERVAL as DuckDB prints it: `1 year 2 months 3 days 04:05:06.5`.
fn interval(months: i32, days: i32, nanos: i64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let (years, months) = (months / 12, months % 12);
    let plural = |n: i32, unit: &str| format!("{n} {unit}{}", if n.abs() == 1 { "" } else { "s" });
    if years != 0 {
        parts.push(plural(years, "year"));
    }
    if months != 0 {
        parts.push(plural(months, "month"));
    }
    if days != 0 {
        parts.push(plural(days, "day"));
    }
    if nanos != 0 || parts.is_empty() {
        let sign = if nanos < 0 { "-" } else { "" };
        let magnitude = nanos.unsigned_abs();
        let seconds = magnitude / 1_000_000_000;
        let fraction = i64::try_from(magnitude % 1_000_000_000).unwrap_or(0);
        let mut clock = format!(
            "{sign}{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        );
        push_fraction(&mut clock, fraction);
        parts.push(clock);
    }
    parts.join(" ")
}

// ----- nested values as JSON ----------------------------------------------

/// Append `array[row]` as JSON. Numbers and booleans stay bare (a DECIMAL
/// keeps its exact digits); everything else is a JSON string.
fn write_json(array: &dyn Array, row: usize, out: &mut String) {
    if array.is_null(row) {
        out.push_str("null");
        return;
    }
    match array.data_type() {
        DataType::List(_) => write_json_list(&array.as_list::<i32>().value(row), out),
        DataType::LargeList(_) => write_json_list(&array.as_list::<i64>().value(row), out),
        DataType::FixedSizeList(..) => write_json_list(&array.as_fixed_size_list().value(row), out),
        DataType::Struct(_) => {
            let Some(strukt) = array.as_any().downcast_ref::<StructArray>() else {
                return out.push_str("null");
            };
            out.push('{');
            for (index, field) in strukt.fields().iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                push_json_string(out, field.name());
                out.push(':');
                write_json(strukt.column(index).as_ref(), row, out);
            }
            out.push('}');
        }
        DataType::Map(..) => {
            let Some(map) = array.as_any().downcast_ref::<MapArray>() else {
                return out.push_str("null");
            };
            let entries = map.value(row);
            out.push('{');
            for index in 0..entries.len() {
                if index > 0 {
                    out.push(',');
                }
                // JSON keys are strings: the key's own text
                let key = entries.column(0);
                let key_text = cell(key.as_ref(), index, column_formatter(key.as_ref()).as_ref())
                    .unwrap_or_else(|| "null".to_string());
                push_json_string(out, &key_text);
                out.push(':');
                write_json(entries.column(1).as_ref(), index, out);
            }
            out.push('}');
        }
        DataType::Union(..) => {
            let child = array.as_union().value(row);
            write_json(child.as_ref(), 0, out);
        }
        DataType::Boolean => {
            out.push_str(if array.as_boolean().value(row) {
                "true"
            } else {
                "false"
            });
        }
        DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Decimal128(..)
        | DataType::Decimal256(..) => {
            let text = cell(array, row, column_formatter(array).as_ref()).unwrap_or_default();
            out.push_str(&text);
        }
        DataType::Float16 | DataType::Float32 | DataType::Float64 => {
            let text = cell(array, row, column_formatter(array).as_ref()).unwrap_or_default();
            // NaN and the infinities are not JSON numbers
            if text.parse::<f64>().is_ok_and(f64::is_finite) {
                out.push_str(&text);
            } else {
                push_json_string(out, &text);
            }
        }
        _ => {
            let text = cell(array, row, column_formatter(array).as_ref()).unwrap_or_default();
            push_json_string(out, &text);
        }
    }
}

fn write_json_list(values: &duckdb::arrow::array::ArrayRef, out: &mut String) {
    out.push('[');
    for index in 0..values.len() {
        if index > 0 {
            out.push(',');
        }
        write_json(values.as_ref(), index, out);
    }
    out.push(']');
}

/// A JSON string literal (quotes, escapes).
fn push_json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
