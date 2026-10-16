//! Turning bytes and timestamps into cells.

use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// ISO-8601 UTC (`2026-10-16T09:00:00Z`), the form every DBCrust engine shows.
pub(crate) fn timestamp(time: &DateTime) -> Option<String> {
    time.fmt(DateTimeFormat::DateTime).ok()
}

/// What a byte range holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Content {
    /// Valid UTF-8 (a multi-byte character cut by the end of the range is dropped).
    Text(String),
    /// NUL bytes or invalid UTF-8: shown as a hex dump.
    Binary,
}

pub(crate) fn classify(bytes: &[u8]) -> Content {
    if bytes.contains(&0) {
        return Content::Binary;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Content::Text(text.to_string()),
        // `error_len() == None`: the input just ends in the middle of a
        // character, which a byte range will do
        Err(error) if error.error_len().is_none() => {
            let valid = &bytes[..error.valid_up_to()];
            // SAFETY-free: `valid_up_to` is a char boundary by definition
            Content::Text(String::from_utf8_lossy(valid).into_owned())
        }
        Err(_) => Content::Binary,
    }
}

/// `line | text` rows, line numbers counted from the start of the range.
pub(crate) fn text_rows(text: &str, max_rows: usize) -> (Vec<Vec<Option<String>>>, bool) {
    let mut rows = Vec::new();
    let mut lines = text.lines();
    for (index, line) in lines.by_ref().take(max_rows).enumerate() {
        rows.push(vec![Some((index + 1).to_string()), Some(line.to_string())]);
    }
    (rows, lines.next().is_some())
}

/// Sixteen bytes per row: `offset | hex | ascii`, like `hexdump -C`.
pub(crate) fn hex_rows(bytes: &[u8], base_offset: u64) -> Vec<Vec<Option<String>>> {
    bytes
        .chunks(16)
        .enumerate()
        .map(|(index, chunk)| {
            let offset = base_offset + (index * 16) as u64;
            let hex = chunk
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            let ascii: String = chunk
                .iter()
                .map(|&byte| {
                    if byte.is_ascii_graphic() || byte == b' ' {
                        byte as char
                    } else {
                        '.'
                    }
                })
                .collect();
            vec![Some(format!("{offset:08x}")), Some(hex), Some(ascii)]
        })
        .collect()
}

/// 48 bytes (64 base64 characters) per row.
pub(crate) fn base64_rows(bytes: &[u8], base_offset: u64) -> Vec<Vec<Option<String>>> {
    bytes
        .chunks(48)
        .enumerate()
        .map(|(index, chunk)| {
            vec![
                Some(format!("{:08x}", base_offset + (index * 48) as u64)),
                Some(STANDARD.encode(chunk)),
            ]
        })
        .collect()
}

/// `1536` → `1.5 KiB`, for notices.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `bytes 0-1023/5000` → (0, 1023, Some(5000)); `bytes 0-9/*` → total None.
pub(crate) fn parse_content_range(header: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = header.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.trim().split_once('-')?;
    Some((
        start.trim().parse().ok()?,
        end.trim().parse().ok()?,
        total.trim().parse().ok(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn text_and_binary_are_told_apart() {
        assert_eq!(
            classify(b"hello\nworld"),
            Content::Text("hello\nworld".into())
        );
        assert_eq!(classify("héllo".as_bytes()), Content::Text("héllo".into()));
        assert_eq!(classify(b"a\0b"), Content::Binary);
        assert_eq!(classify(&[0xff, 0xfe, 0x41]), Content::Binary);
        assert_eq!(classify(b""), Content::Text(String::new()));
    }

    #[test]
    fn a_character_cut_by_the_range_end_is_dropped_not_binary() {
        let bytes = "héllo".as_bytes();
        // cut inside the two-byte 'é'
        assert_eq!(classify(&bytes[..2]), Content::Text("h".into()));
        // a bad byte in the middle is still binary
        let mut broken = b"abc".to_vec();
        broken.push(0xc3);
        broken.extend_from_slice(b"(def");
        assert_eq!(classify(&broken), Content::Binary);
    }

    #[test]
    fn text_rows_are_numbered_and_capped() {
        let (rows, cut) = text_rows("a\nb\nc\n", 10);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], vec![Some("2".into()), Some("b".into())]);
        assert!(!cut);
        let (rows, cut) = text_rows("a\nb\nc\n", 2);
        assert_eq!(rows.len(), 2);
        assert!(cut);
        let (rows, _) = text_rows("one\r\ntwo", 5);
        assert_eq!(rows[0][1].as_deref(), Some("one"));
        assert_eq!(rows[1][1].as_deref(), Some("two"));
    }

    #[test]
    fn hex_rows_follow_hexdump() {
        let rows = hex_rows(b"ABC\x00\x7f\xff", 32);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0].as_deref(), Some("00000020"));
        assert_eq!(rows[0][1].as_deref(), Some("41 42 43 00 7f ff"));
        assert_eq!(rows[0][2].as_deref(), Some("ABC..."));
        let long = hex_rows(&[7u8; 40], 0);
        assert_eq!(long.len(), 3);
        assert_eq!(long[2][0].as_deref(), Some("00000020"));
    }

    #[test]
    fn base64_rows_chunk_by_48_bytes() {
        let rows = base64_rows(&[1u8; 100], 0);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][1].as_deref().map(str::len), Some(64));
        assert_eq!(rows[1][0].as_deref(), Some("00000030"));
        assert_eq!(rows[0][1].as_deref(), Some(&*STANDARD.encode([1u8; 48])));
    }

    #[rstest]
    #[case(0, "0 B")]
    #[case(1023, "1023 B")]
    #[case(1024, "1.0 KiB")]
    #[case(1536, "1.5 KiB")]
    #[case(5 * 1024 * 1024, "5.0 MiB")]
    fn sizes_are_human(#[case] bytes: u64, #[case] expected: &str) {
        assert_eq!(human_bytes(bytes), expected);
    }

    #[rstest]
    #[case("bytes 0-1023/5000", Some((0, 1023, Some(5000))))]
    #[case("bytes 10-19/*", Some((10, 19, None)))]
    #[case(" bytes 5-5/6 ", Some((5, 5, Some(6))))]
    #[case("items 0-1/2", None)]
    #[case("bytes x-1/2", None)]
    #[case("bytes */100", None)]
    fn content_ranges_parse(
        #[case] header: &str,
        #[case] expected: Option<(u64, u64, Option<u64>)>,
    ) {
        assert_eq!(parse_content_range(header), expected);
    }
}
