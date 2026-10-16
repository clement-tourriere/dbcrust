//! SQL Server values as display strings.
//!
//! A cell is `None` exactly when the server sent SQL NULL; everything else is
//! text in the form SSMS and `sqlcmd` show it: `bit` as `1`/`0`, `uniqueidentifier`
//! upper-case, binary as `0x…` hex, `datetime2(n)` with `n` fraction digits,
//! `datetimeoffset` with its `+hh:mm` offset, `money` with four decimals.

use chrono::NaiveDate;
use tiberius::numeric::Numeric;
use tiberius::time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time};
use tiberius::xml::XmlData;
use tiberius::{ColumnData, ColumnType};

/// Bytes of a `binary`/`varbinary` shown before the value is cut with its size.
pub(crate) const MAX_BINARY_BYTES: usize = 1024;

/// Characters of one text/XML cell kept before it is cut with its size; a
/// guard against a single `varchar(max)` cell exhausting memory downstream.
pub(crate) const MAX_TEXT_CHARS: usize = 1_000_000;

/// Format one cell. `column_type` tells `money` (which arrives as a float)
/// from a real `float`.
pub(crate) fn format_cell(column_type: ColumnType, data: &ColumnData<'_>) -> Option<String> {
    let money = matches!(column_type, ColumnType::Money | ColumnType::Money4);
    Some(match data {
        ColumnData::U8(v) => (*v)?.to_string(),
        ColumnData::I16(v) => (*v)?.to_string(),
        ColumnData::I32(v) => (*v)?.to_string(),
        ColumnData::I64(v) => (*v)?.to_string(),
        ColumnData::F32(v) => format_f32((*v)?),
        ColumnData::F64(v) if money => format!("{:.4}", (*v)?),
        ColumnData::F64(v) => format_f64((*v)?),
        ColumnData::Bit(v) => if (*v)? { "1" } else { "0" }.to_string(),
        ColumnData::String(v) => cap_text(v.as_deref()?),
        ColumnData::Guid(v) => v.as_ref()?.hyphenated().to_string().to_uppercase(),
        ColumnData::Binary(v) => format_binary(v.as_deref()?),
        ColumnData::Numeric(v) => format_numeric((*v)?),
        ColumnData::Xml(v) => cap_text(&XmlData::to_string(v.as_deref()?)),
        ColumnData::DateTime(v) => format_datetime((*v)?),
        ColumnData::SmallDateTime(v) => format_smalldatetime((*v)?),
        ColumnData::Time(v) => format_time((*v)?),
        ColumnData::Date(v) => format_date((*v)?),
        ColumnData::DateTime2(v) => format_datetime2((*v)?),
        ColumnData::DateTimeOffset(v) => format_datetimeoffset((*v)?),
    })
}

fn cap_text(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        Some((cut, _)) => format!("{}… [truncated; {} bytes]", &text[..cut], text.len()),
        None => text.to_string(),
    }
}

fn format_f32(value: f32) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        format!("{value}")
    }
}

fn format_f64(value: f64) -> String {
    value.to_string()
}

/// `0x` + upper-case hex, cut at [`MAX_BINARY_BYTES`] with the full size.
pub(crate) fn format_binary(bytes: &[u8]) -> String {
    let shown = bytes.len().min(MAX_BINARY_BYTES);
    let mut text = String::with_capacity(2 + shown * 2 + 24);
    text.push_str("0x");
    for byte in &bytes[..shown] {
        text.push_str(&format!("{byte:02X}"));
    }
    if bytes.len() > shown {
        text.push_str(&format!("… [truncated; {} bytes]", bytes.len()));
    }
    text
}

/// `decimal`/`numeric` exactly: the integer mantissa with the decimal point
/// placed by the scale, no floating point involved.
pub(crate) fn format_numeric(value: Numeric) -> String {
    format_scaled(value.value(), value.scale())
}

fn format_scaled(mantissa: i128, scale: u8) -> String {
    let negative = mantissa < 0;
    let digits = mantissa.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let body = if scale == 0 {
        digits
    } else if digits.len() > scale {
        let (whole, fraction) = digits.split_at(digits.len() - scale);
        format!("{whole}.{fraction}")
    } else {
        format!("0.{digits:0>scale$}")
    };
    if negative { format!("-{body}") } else { body }
}

fn date_from_ce_days(days_since_year_1: i64) -> Option<NaiveDate> {
    let ce_days = i32::try_from(days_since_year_1.checked_add(1)?).ok()?;
    NaiveDate::from_num_days_from_ce_opt(ce_days)
}

fn format_date_days(days_since_year_1: i64) -> String {
    match date_from_ce_days(days_since_year_1) {
        Some(date) => date.format("%Y-%m-%d").to_string(),
        None => format!("<invalid date {days_since_year_1}>"),
    }
}

/// Days since 1900-01-01 (`datetime`, `smalldatetime`) as a date.
fn format_date_days_1900(days: i64) -> String {
    // 1900-01-01 is day 693_595 since 0001-01-01
    format_date_days(693_595 + days)
}

pub(crate) fn format_date(date: Date) -> String {
    format_date_days(i64::from(date.days()))
}

/// `hh:mm:ss` plus `scale` fraction digits (`time(n)`, `datetime2(n)`).
fn format_time_parts(increments: u64, scale: u8) -> String {
    let scale = u32::from(scale.min(7));
    let per_second = 10u64.pow(scale);
    let seconds = increments / per_second;
    let fraction = increments % per_second;
    let (hours, minutes, seconds) = (seconds / 3600 % 24, seconds / 60 % 60, seconds % 60);
    if scale == 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!(
            "{hours:02}:{minutes:02}:{seconds:02}.{fraction:0>width$}",
            width = scale as usize
        )
    }
}

pub(crate) fn format_time(time: Time) -> String {
    format_time_parts(time.increments(), time.scale())
}

pub(crate) fn format_datetime2(value: DateTime2) -> String {
    format!(
        "{} {}",
        format_date(value.date()),
        format_time(value.time())
    )
}

/// `datetimeoffset` is sent as UTC plus the offset; SSMS shows the local time
/// (UTC + offset) followed by the offset, so that is what is rendered.
pub(crate) fn format_datetimeoffset(value: DateTimeOffset) -> String {
    let offset = i64::from(value.offset());
    let (date, time) = (value.datetime2().date(), value.datetime2().time());
    let scale = u32::from(time.scale().min(7));
    let per_second = 10u64.pow(scale);
    let utc_seconds = i64::from(date.days()) * 86_400 + (time.increments() / per_second) as i64;
    let local_seconds = utc_seconds + offset * 60;
    let (days, seconds_of_day) = (
        local_seconds.div_euclid(86_400),
        local_seconds.rem_euclid(86_400),
    );
    let local_time = format_time_parts(
        seconds_of_day as u64 * per_second + time.increments() % per_second,
        time.scale(),
    );
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.abs();
    format!(
        "{} {local_time} {sign}{:02}:{:02}",
        format_date_days(days),
        minutes / 60,
        minutes % 60
    )
}

/// Legacy `datetime`: days since 1900-01-01 and 1/300 s ticks since midnight,
/// shown as `yyyy-mm-dd hh:mm:ss.mmm` (the three digits SQL Server prints).
pub(crate) fn format_datetime(value: DateTime) -> String {
    let millis = (u64::from(value.seconds_fragments()) * 1000 + 150) / 300;
    let (seconds, millis) = (millis / 1000, millis % 1000);
    format!(
        "{} {:02}:{:02}:{:02}.{millis:03}",
        format_date_days_1900(i64::from(value.days())),
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// `smalldatetime`: days since 1900-01-01 and minutes since midnight.
pub(crate) fn format_smalldatetime(value: SmallDateTime) -> String {
    let minutes = u32::from(value.seconds_fragments());
    format!(
        "{} {:02}:{:02}:00",
        format_date_days_1900(i64::from(value.days())),
        minutes / 60 % 24,
        minutes % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::borrow::Cow;
    use tiberius::Uuid;

    fn cell(data: ColumnData<'_>) -> Option<String> {
        format_cell(ColumnType::Null, &data)
    }

    #[rstest]
    #[case::tinyint(ColumnData::U8(Some(255)), "255")]
    #[case::smallint(ColumnData::I16(Some(-32768)), "-32768")]
    #[case::int(ColumnData::I32(Some(-1)), "-1")]
    #[case::bigint(ColumnData::I64(Some(i64::MAX)), "9223372036854775807")]
    #[case::real(ColumnData::F32(Some(1.5)), "1.5")]
    #[case::float(ColumnData::F64(Some(0.1)), "0.1")]
    #[case::float_whole(ColumnData::F64(Some(3.0)), "3")]
    #[case::bit_true(ColumnData::Bit(Some(true)), "1")]
    #[case::bit_false(ColumnData::Bit(Some(false)), "0")]
    #[case::nvarchar(ColumnData::String(Some(Cow::Borrowed("héllo ✓"))), "héllo ✓")]
    #[case::empty_string(ColumnData::String(Some(Cow::Borrowed(""))), "")]
    #[case::guid(
        ColumnData::Guid(Some(Uuid::parse_str("6f9619ff-8b86-d011-b42d-00c04fc964ff").unwrap())),
        "6F9619FF-8B86-D011-B42D-00C04FC964FF"
    )]
    #[case::binary(ColumnData::Binary(Some(Cow::Borrowed(&[0x01, 0xAB, 0xFF][..]))), "0x01ABFF")]
    #[case::empty_binary(ColumnData::Binary(Some(Cow::Borrowed(&[][..]))), "0x")]
    #[case::xml(
        ColumnData::Xml(Some(Cow::Owned(XmlData::new("<a x=\"1\"/>")))),
        "<a x=\"1\"/>"
    )]
    fn plain_values(#[case] data: ColumnData<'static>, #[case] expected: &str) {
        assert_eq!(cell(data).as_deref(), Some(expected));
    }

    #[rstest]
    #[case(ColumnData::U8(None))]
    #[case(ColumnData::I16(None))]
    #[case(ColumnData::I32(None))]
    #[case(ColumnData::I64(None))]
    #[case(ColumnData::F32(None))]
    #[case(ColumnData::F64(None))]
    #[case(ColumnData::Bit(None))]
    #[case(ColumnData::String(None))]
    #[case(ColumnData::Guid(None))]
    #[case(ColumnData::Binary(None))]
    #[case(ColumnData::Numeric(None))]
    #[case(ColumnData::Xml(None))]
    #[case(ColumnData::DateTime(None))]
    #[case(ColumnData::SmallDateTime(None))]
    #[case(ColumnData::Time(None))]
    #[case(ColumnData::Date(None))]
    #[case(ColumnData::DateTime2(None))]
    #[case(ColumnData::DateTimeOffset(None))]
    fn null_is_none_for_every_type(#[case] data: ColumnData<'static>) {
        assert_eq!(cell(data), None);
    }

    #[test]
    fn empty_string_is_not_null() {
        assert_eq!(
            cell(ColumnData::String(Some(Cow::Borrowed("")))),
            Some(String::new())
        );
    }

    #[rstest]
    #[case::money(ColumnType::Money, 12.5, "12.5000")]
    #[case::smallmoney(ColumnType::Money4, -0.0001, "-0.0001")]
    #[case::large_money(ColumnType::Money, 214748.3647, "214748.3647")]
    fn money_has_four_decimals(
        #[case] kind: ColumnType,
        #[case] value: f64,
        #[case] expected: &str,
    ) {
        assert_eq!(
            format_cell(kind, &ColumnData::F64(Some(value))).as_deref(),
            Some(expected)
        );
        // the same bits in a float column are a plain float
        assert_ne!(
            format_cell(ColumnType::Float8, &ColumnData::F64(Some(12.5))).as_deref(),
            Some("12.5000")
        );
    }

    #[rstest]
    #[case::integer(12345, 0, "12345")]
    #[case::two_decimals(12345, 2, "123.45")]
    #[case::leading_zeros(5, 4, "0.0005")]
    #[case::negative(-12345, 2, "-123.45")]
    #[case::negative_small(-5, 3, "-0.005")]
    #[case::zero_scaled(0, 2, "0.00")]
    #[case::exact_digits(123, 3, "0.123")]
    #[case::max_precision(
        99_999_999_999_999_999_999_999_999_999_999_999_999,
        38,
        "0.99999999999999999999999999999999999999"
    )]
    fn decimals_are_exact(#[case] mantissa: i128, #[case] scale: u8, #[case] expected: &str) {
        assert_eq!(
            cell(ColumnData::Numeric(Some(Numeric::new_with_scale(
                mantissa, scale
            ))))
            .as_deref(),
            Some(expected)
        );
    }

    #[rstest]
    #[case::epoch(0, "0001-01-01")]
    #[case::unix_epoch(719_162, "1970-01-01")]
    #[case::y2k(730_119, "2000-01-01")]
    #[case::leap_day(738_944, "2024-02-29")]
    #[case::max(3_652_058, "9999-12-31")]
    fn dates(#[case] days: u32, #[case] expected: &str) {
        assert_eq!(
            cell(ColumnData::Date(Some(Date::new(days)))).as_deref(),
            Some(expected)
        );
    }

    #[rstest]
    #[case::midnight(0, 0, "00:00:00")]
    #[case::whole_seconds(3_723, 0, "01:02:03")]
    #[case::scale3(3_723_456, 3, "01:02:03.456")]
    #[case::scale7(863_999_999_999, 7, "23:59:59.9999999")]
    #[case::scale7_leading_zero_fraction(10_000_001, 7, "00:00:01.0000001")]
    fn times(#[case] increments: u64, #[case] scale: u8, #[case] expected: &str) {
        assert_eq!(
            cell(ColumnData::Time(Some(Time::new(increments, scale)))).as_deref(),
            Some(expected)
        );
    }

    #[test]
    fn datetime2_joins_date_and_time() {
        let value = DateTime2::new(Date::new(738_944), Time::new(450_000_001, 7));
        assert_eq!(
            cell(ColumnData::DateTime2(Some(value))).as_deref(),
            Some("2024-02-29 00:00:45.0000001")
        );
    }

    #[rstest]
    #[case::utc(0, "2024-02-29 00:00:01.0000000 +00:00")]
    #[case::plus_5_30(330, "2024-02-29 05:30:01.0000000 +05:30")]
    #[case::minus_8_crosses_midnight(-480, "2024-02-28 16:00:01.0000000 -08:00")]
    #[case::minus_half(-30, "2024-02-28 23:30:01.0000000 -00:30")]
    fn datetimeoffset_shows_local_time_and_the_offset(#[case] offset: i16, #[case] expected: &str) {
        // the wire value is UTC: 2024-02-29 00:00:01
        let value = DateTimeOffset::new(
            DateTime2::new(Date::new(738_944), Time::new(10_000_000, 7)),
            offset,
        );
        assert_eq!(
            cell(ColumnData::DateTimeOffset(Some(value))).as_deref(),
            Some(expected)
        );
    }

    #[test]
    fn datetimeoffset_keeps_scale_zero_without_a_fraction() {
        let value =
            DateTimeOffset::new(DateTime2::new(Date::new(738_944), Time::new(7_800, 0)), 60);
        assert_eq!(
            cell(ColumnData::DateTimeOffset(Some(value))).as_deref(),
            Some("2024-02-29 03:10:00 +01:00")
        );
    }

    #[rstest]
    #[case::epoch(0, 0, "1900-01-01 00:00:00.000")]
    #[case::one_tick(0, 1, "1900-01-01 00:00:00.003")]
    #[case::two_ticks(0, 2, "1900-01-01 00:00:00.007")]
    #[case::one_second(44_973, 300, "2023-02-18 00:00:01.000")]
    #[case::end_of_day(0, 25_919_999, "1900-01-01 23:59:59.997")]
    #[case::before_1900(-1, 0, "1899-12-31 00:00:00.000")]
    fn legacy_datetime(#[case] days: i32, #[case] ticks: u32, #[case] expected: &str) {
        assert_eq!(
            cell(ColumnData::DateTime(Some(DateTime::new(days, ticks)))).as_deref(),
            Some(expected)
        );
    }

    #[rstest]
    #[case(0, 0, "1900-01-01 00:00:00")]
    #[case(44_973, 1_439, "2023-02-18 23:59:00")]
    fn smalldatetime_counts_minutes(
        #[case] days: u16,
        #[case] minutes: u16,
        #[case] expected: &str,
    ) {
        assert_eq!(
            cell(ColumnData::SmallDateTime(Some(SmallDateTime::new(
                days, minutes
            ))))
            .as_deref(),
            Some(expected)
        );
    }

    #[test]
    fn hostile_values_never_panic() {
        // out-of-range day counts and huge tick counts from a broken server
        assert!(cell(ColumnData::Date(Some(Date::new(0xFF_FFFF)))).is_some());
        assert_eq!(
            format_date_days(i64::MAX),
            "<invalid date 9223372036854775807>"
        );
        assert!(cell(ColumnData::Time(Some(Time::new(u64::MAX, 7)))).is_some());
        assert!(
            cell(ColumnData::SmallDateTime(Some(SmallDateTime::new(
                u16::MAX,
                u16::MAX
            ))))
            .is_some()
        );
        assert!(
            cell(ColumnData::DateTime(Some(DateTime::new(
                i32::MIN,
                u32::MAX
            ))))
            .is_some()
        );
    }

    #[test]
    fn long_binary_is_cut_with_its_size() {
        let bytes = vec![0xAB; MAX_BINARY_BYTES + 5];
        let text = format_binary(&bytes);
        assert!(text.starts_with("0xABAB"));
        assert!(text.ends_with(&format!("[truncated; {} bytes]", bytes.len())));
        assert_eq!(text.matches("AB").count(), MAX_BINARY_BYTES);
    }

    #[test]
    fn long_text_is_cut_with_its_size() {
        let text = "x".repeat(MAX_TEXT_CHARS + 10);
        let shown = cell(ColumnData::String(Some(Cow::Owned(text.clone())))).unwrap();
        assert!(shown.ends_with(&format!("[truncated; {} bytes]", text.len())));
        assert!(shown.len() < text.len() + 40);
    }
}
