//! `AttributeValue` → result cells.
//!
//! A top-level attribute becomes one cell: strings as they are, numbers as the
//! exact text DynamoDB sent (never through a float), binaries as base64,
//! booleans as `true`/`false`, the `NULL` type as a SQL NULL. Maps, lists and
//! the three set types become compact JSON text; inside that JSON numbers stay
//! exact too (they are written as the number text, not as quoted strings).

use aws_sdk_dynamodb::types::AttributeValue;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use dbcrust_core::database::StructuredQueryResult;
use std::collections::{HashMap, HashSet};

/// More attribute names than this fold into a trailing `_other` JSON cell.
pub(crate) const MAX_COLUMNS: usize = 200;
/// The folded cell.
pub(crate) const OTHER_COLUMN: &str = "_other";

pub(crate) type Item = HashMap<String, AttributeValue>;

fn base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// `text` as a JSON string literal.
pub(crate) fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A DynamoDB number as a JSON number. Numbers are decimal text already
/// (`-12.50`, `1E+3`); anything that would not be valid JSON is quoted instead
/// so the output always parses.
fn json_number(number: &str) -> String {
    let valid = !number.is_empty()
        && number.parse::<f64>().is_ok_and(f64::is_finite)
        && number
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
        && !number.starts_with('+')
        && !number.starts_with('.')
        && !number.ends_with('.')
        && !number.starts_with("-.");
    if valid {
        number.to_string()
    } else {
        json_string(number)
    }
}

/// Compact JSON for any value; objects list their keys alphabetically so the
/// same item always prints the same text.
pub(crate) fn json(value: &AttributeValue) -> String {
    match value {
        AttributeValue::S(text) => json_string(text),
        AttributeValue::N(number) => json_number(number),
        AttributeValue::B(blob) => json_string(&base64(blob.as_ref())),
        AttributeValue::Bool(flag) => flag.to_string(),
        AttributeValue::Null(_) => "null".to_string(),
        AttributeValue::Ss(values) => {
            let parts: Vec<String> = values.iter().map(|v| json_string(v)).collect();
            format!("[{}]", parts.join(","))
        }
        AttributeValue::Ns(values) => {
            let parts: Vec<String> = values.iter().map(|v| json_number(v)).collect();
            format!("[{}]", parts.join(","))
        }
        AttributeValue::Bs(values) => {
            let parts: Vec<String> = values
                .iter()
                .map(|v| json_string(&base64(v.as_ref())))
                .collect();
            format!("[{}]", parts.join(","))
        }
        AttributeValue::L(values) => {
            let parts: Vec<String> = values.iter().map(json).collect();
            format!("[{}]", parts.join(","))
        }
        AttributeValue::M(map) => json_object(map),
        // A type this client does not know yet
        _ => "null".to_string(),
    }
}

fn json_object(map: &Item) -> String {
    let mut names: Vec<&String> = map.keys().collect();
    names.sort();
    let parts: Vec<String> = names
        .into_iter()
        .map(|name| format!("{}:{}", json_string(name), json(&map[name])))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// The cell of a top-level attribute (`None` = SQL NULL).
pub(crate) fn cell(value: &AttributeValue) -> Option<String> {
    match value {
        AttributeValue::S(text) => Some(text.clone()),
        AttributeValue::N(number) => Some(number.clone()),
        AttributeValue::B(blob) => Some(base64(blob.as_ref())),
        AttributeValue::Bool(flag) => Some(flag.to_string()),
        AttributeValue::Null(_) => None,
        AttributeValue::Ss(_)
        | AttributeValue::Ns(_)
        | AttributeValue::Bs(_)
        | AttributeValue::L(_)
        | AttributeValue::M(_) => Some(json(value)),
        _ => None,
    }
}

/// Items → a table. The columns are the union of the attribute names: `keys`
/// first (the table's key attributes, when known), then every other name in the
/// order it is first seen. An item's own attributes have no order in DynamoDB,
/// so the names an item adds are taken alphabetically. A name missing from an
/// item is a SQL NULL, as is the `NULL` type. Past [`MAX_COLUMNS`] names the
/// rest fold into an `_other` JSON cell.
pub(crate) fn items_to_result(items: &[Item], keys: &[String]) -> StructuredQueryResult {
    let mut columns: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut folded = false;
    let mut add = |name: &str| {
        if seen.contains(name) {
            return;
        }
        if columns.len() >= MAX_COLUMNS {
            folded = true;
            return;
        }
        seen.insert(name.to_string());
        columns.push(name.to_string());
    };
    // `keys` only count when some item has them: an empty result has no
    // columns of its own to claim
    let present: HashSet<&str> = items
        .iter()
        .flat_map(|item| item.keys().map(String::as_str))
        .collect();
    for key in keys {
        if present.contains(key.as_str()) {
            add(key);
        }
    }
    for item in items {
        let mut names: Vec<&String> = item.keys().collect();
        names.sort();
        for name in names {
            add(name);
        }
    }
    let known: HashSet<String> = columns.iter().cloned().collect();
    if folded {
        columns.push(OTHER_COLUMN.to_string());
    }
    let rows = items
        .iter()
        .map(|item| {
            let mut row: Vec<Option<String>> = columns
                .iter()
                .take(columns.len() - usize::from(folded))
                .map(|column| item.get(column).and_then(cell))
                .collect();
            if folded {
                let rest: Item = item
                    .iter()
                    .filter(|(name, _)| !known.contains(*name))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect();
                row.push((!rest.is_empty()).then(|| json_object(&rest)));
            }
            row
        })
        .collect();
    StructuredQueryResult { columns, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_dynamodb::primitives::Blob;
    use rstest::rstest;

    fn s(text: &str) -> AttributeValue {
        AttributeValue::S(text.to_string())
    }

    fn n(text: &str) -> AttributeValue {
        AttributeValue::N(text.to_string())
    }

    fn item(pairs: Vec<(&str, AttributeValue)>) -> Item {
        pairs
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect()
    }

    #[rstest]
    #[case::string(s("héllo"), Some("héllo"))]
    #[case::empty_string(s(""), Some(""))]
    #[case::number(
        n("12345678901234567890.123456789"),
        Some("12345678901234567890.123456789")
    )]
    #[case::negative_exponent(n("-1.5E-7"), Some("-1.5E-7"))]
    #[case::binary(AttributeValue::B(Blob::new(vec![0u8, 255, 16])), Some("AP8Q"))]
    #[case::boolean(AttributeValue::Bool(true), Some("true"))]
    #[case::boolean_false(AttributeValue::Bool(false), Some("false"))]
    #[case::null_type(AttributeValue::Null(true), None)]
    fn scalars_become_cells(#[case] value: AttributeValue, #[case] expected: Option<&str>) {
        assert_eq!(cell(&value).as_deref(), expected);
    }

    #[rstest]
    #[case::string_set(AttributeValue::Ss(vec!["a".into(), "b\"c".into()]), r#"["a","b\"c"]"#)]
    #[case::number_set(AttributeValue::Ns(vec!["1".into(), "2.50".into(), "-3E+2".into()]), "[1,2.50,-3E+2]")]
    #[case::binary_set(AttributeValue::Bs(vec![Blob::new(vec![1u8, 2]), Blob::new(vec![255u8])]), r#"["AQI=","/w=="]"#)]
    #[case::list(AttributeValue::L(vec![s("x"), n("7"), AttributeValue::Bool(false), AttributeValue::Null(true)]), r#"["x",7,false,null]"#)]
    #[case::empty_list(AttributeValue::L(vec![]), "[]")]
    #[case::empty_map(AttributeValue::M(HashMap::new()), "{}")]
    fn collections_become_json(#[case] value: AttributeValue, #[case] expected: &str) {
        assert_eq!(cell(&value).as_deref(), Some(expected));
    }

    #[test]
    fn nested_maps_are_sorted_json_with_exact_numbers() {
        let value = AttributeValue::M(item(vec![
            ("zeta", n("10.10")),
            (
                "address",
                AttributeValue::M(item(vec![
                    ("street", s("1 Main \"St\"\n")),
                    ("tags", AttributeValue::Ss(vec!["a".into()])),
                    ("geo", AttributeValue::L(vec![n("1.5"), n("2.5")])),
                ])),
            ),
            ("alpha", AttributeValue::Null(true)),
        ]));
        assert_eq!(
            cell(&value).unwrap(),
            r#"{"address":{"geo":[1.5,2.5],"street":"1 Main \"St\"\n","tags":["a"]},"alpha":null,"zeta":10.10}"#
        );
    }

    #[test]
    fn json_text_is_always_valid_json() {
        // control characters escape, numbers that are not JSON numbers are quoted
        assert_eq!(json(&s("a\u{1}\tb")), "\"a\\u0001\\tb\"");
        assert_eq!(json(&n("NaN")), "\"NaN\"");
        assert_eq!(json(&n("")), "\"\"");
        assert_eq!(json(&n("+5")), "\"+5\"");
        assert_eq!(json(&n("5.")), "\"5.\"");
    }

    #[test]
    fn columns_are_the_union_with_keys_first() {
        let items = vec![
            item(vec![("b", s("1")), ("a", s("2")), ("pk", s("p1"))]),
            item(vec![("pk", s("p2")), ("sk", n("5")), ("c", s("x"))]),
        ];
        let result = items_to_result(&items, &["pk".to_string(), "sk".to_string()]);
        assert_eq!(result.columns, ["pk", "sk", "a", "b", "c"]);
        assert_eq!(
            result.rows[0],
            [
                Some("p1".into()),
                None,
                Some("2".into()),
                Some("1".into()),
                None
            ]
        );
        assert_eq!(
            result.rows[1],
            [
                Some("p2".into()),
                Some("5".into()),
                None,
                None,
                Some("x".into())
            ]
        );
    }

    #[test]
    fn a_dynamodb_null_and_a_missing_attribute_are_both_sql_null() {
        let items = vec![
            item(vec![("pk", s("1")), ("v", AttributeValue::Null(true))]),
            item(vec![("pk", s("2"))]),
        ];
        let result = items_to_result(&items, &[]);
        assert_eq!(result.columns, ["pk", "v"]);
        assert_eq!(result.rows[0][1], None);
        assert_eq!(result.rows[1][1], None);
    }

    #[test]
    fn key_columns_without_data_do_not_appear() {
        let result = items_to_result(&[], &["pk".to_string()]);
        assert!(result.columns.is_empty() && result.rows.is_empty());
    }

    #[test]
    fn too_many_attribute_names_fold_into_an_other_column() {
        let wide: Item = (0..MAX_COLUMNS + 3)
            .map(|i| (format!("c{i:04}"), n(&i.to_string())))
            .collect();
        let result = items_to_result(&[wide], &[]);
        assert_eq!(result.columns.len(), MAX_COLUMNS + 1);
        assert_eq!(result.columns.last().unwrap(), OTHER_COLUMN);
        let other = result.rows[0].last().unwrap().as_deref().unwrap();
        assert_eq!(other, r#"{"c0200":200,"c0201":201,"c0202":202}"#);
    }

    #[test]
    fn narrow_results_have_no_other_column() {
        let result = items_to_result(&[item(vec![("a", s("1"))])], &[]);
        assert_eq!(result.columns, ["a"]);
    }
}
