//! Row inspector: one row of a result as `column → value`, in a sheet.
//!
//! Values are mono and selectable, NULL is distinct from an empty string,
//! JSON documents are pretty-printed in a code block, and every field has a
//! copy button.

use super::grid::copy_text;
use super::model;
use dbcrust_gui_core::ResultSet;
use dbcrust_gui_core::export;
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::Button;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// Longest value shown in full; the copy button still copies everything.
const MAX_SHOWN_CHARS: usize = 20_000;
const SHEET_WIDTH: f32 = 480.;

struct Field {
    name: SharedString,
    value: FieldValue,
}

enum FieldValue {
    Null,
    Empty,
    Text {
        shown: SharedString,
        omitted: usize,
        multiline: bool,
    },
    Json {
        shown: SharedString,
        omitted: usize,
    },
}

fn build_fields(result: &ResultSet, row: usize) -> Vec<Field> {
    let Some(cells) = result.rows.get(row) else {
        return Vec::new();
    };
    let names = export::unique_column_names(&result.columns);
    names
        .into_iter()
        .enumerate()
        .map(|(ix, name)| {
            let value = match cells.get(ix).and_then(|cell| cell.as_deref()) {
                None => FieldValue::Null,
                Some("") => FieldValue::Empty,
                Some(text) => {
                    let (shown, omitted) = cap(text);
                    match model::pretty_json(&shown).filter(|_| omitted == 0) {
                        Some(pretty) => FieldValue::Json {
                            shown: pretty.into(),
                            omitted,
                        },
                        None => FieldValue::Text {
                            multiline: shown.contains('\n'),
                            shown: shown.into(),
                            omitted,
                        },
                    }
                }
            };
            Field {
                name: name.into(),
                value,
            }
        })
        .collect()
}

/// `(text cut at MAX_SHOWN_CHARS, characters left out)`.
fn cap(text: &str) -> (String, usize) {
    match text.char_indices().nth(MAX_SHOWN_CHARS) {
        None => (text.to_string(), 0),
        Some((cut, _)) => (text[..cut].to_string(), text[cut..].chars().count()),
    }
}

/// Open the inspector for source row `row`. `open` is raised while the sheet
/// is on screen, so the grid can close it when its results are replaced.
pub fn open(
    result: Arc<ResultSet>,
    row: usize,
    open: Rc<Cell<bool>>,
    window: &mut Window,
    cx: &mut App,
) {
    if row >= result.rows.len() {
        return;
    }
    open.set(true);
    let fields = Rc::new(build_fields(&result, row));
    window.open_sheet(cx, move |sheet, _window, cx| {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();

        let title = h_flex()
            .gap_2()
            .items_baseline()
            .child(format!("Row {}", row + 1))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(muted)
                    .child(format!("{} columns", fields.len())),
            );

        let body = v_flex()
            .gap_4()
            .py_2()
            .children(fields.iter().enumerate().map(|(ix, field)| {
                let copy_result = result.clone();
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .text_color(muted)
                                    .child(field.name.clone()),
                            )
                            .child(
                                Clipboard::new(("inspect-copy", ix))
                                    .tooltip("Copy value")
                                    .value_fn(move |_, _| {
                                        let value =
                                            copy_result.rows.get(row).and_then(|r| r.get(ix));
                                        export::value_to_text(value.and_then(|v| v.as_deref()))
                                            .to_string()
                                            .into()
                                    }),
                            ),
                    )
                    .child(render_value(ix, &field.value, mono.clone(), cx))
            }));

        let footer = {
            let result = result.clone();
            Button::new("inspect-copy-row")
                .label("Copy row as JSON")
                .outline()
                .small()
                .on_click(move |_, window, cx| {
                    if let Some(json) = export::row_to_json(&result, row) {
                        copy_text(json, "Copied row as JSON", window, cx);
                    }
                })
        };

        let on_close = open.clone();
        sheet
            .title(title)
            .size(px(SHEET_WIDTH))
            .footer(footer)
            .on_close(move |_, _, _| on_close.set(false))
            .child(body)
    });
}

fn render_value(ix: usize, value: &FieldValue, mono: SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    match value {
        FieldValue::Null => div()
            .font_family(mono)
            .text_size(px(12.))
            .italic()
            .text_color(muted.opacity(0.7))
            .child("NULL")
            .into_any_element(),
        FieldValue::Empty => div()
            .font_family(mono)
            .text_size(px(12.))
            .italic()
            .text_color(muted.opacity(0.55))
            .child("empty")
            .into_any_element(),
        FieldValue::Text {
            shown,
            omitted,
            multiline,
        } => {
            let block = div()
                .w_full()
                .min_w_0()
                .font_family(mono)
                .text_size(px(12.))
                .child(SelectableText::new(("inspect-value", ix), shown.clone()));
            let block = if *multiline {
                code_block(block, cx)
            } else {
                block
            };
            with_omitted(block, *omitted, cx)
        }
        FieldValue::Json { shown, omitted } => {
            let block = code_block(
                div()
                    .w_full()
                    .min_w_0()
                    .font_family(mono)
                    .text_size(px(12.))
                    .child(SelectableText::new(("inspect-value", ix), shown.clone())),
                cx,
            );
            with_omitted(block, *omitted, cx)
        }
    }
}

/// Mono value on a subtle surface, for JSON and multi-line text.
fn code_block(block: Div, cx: &App) -> Div {
    let theme = cx.theme();
    block
        .p_2()
        .rounded(theme.radius)
        .bg(theme.muted.opacity(0.5))
        .border_1()
        .border_color(theme.border)
}

fn with_omitted(block: Div, omitted: usize, cx: &App) -> AnyElement {
    if omitted == 0 {
        return block.into_any_element();
    }
    v_flex()
        .gap_1()
        .child(block)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(format!(
                    "… {} more characters. Copy value copies everything.",
                    model::group_thousands(omitted)
                )),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Explicit imports: `use super::*` would pull in GPUI's `test` macro and
    // shadow `#[test]`.
    use super::{FieldValue, MAX_SHOWN_CHARS, build_fields, cap};
    use dbcrust_gui_core::ResultSet;
    use std::time::Duration;

    fn result(columns: &[&str], row: &[Option<&str>]) -> ResultSet {
        ResultSet {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: vec![row.iter().map(|c| c.map(str::to_string)).collect()],
            elapsed: Duration::ZERO,
            auto_limit: None,
            sql: String::new(),
        }
    }

    #[test]
    fn fields_distinguish_null_empty_text_and_json() {
        let r = result(
            &["a", "a", "b", "c", "d"],
            &[
                None,
                Some(""),
                Some("plain"),
                Some(r#"{"k":[1,2]}"#),
                Some("l1\nl2"),
            ],
        );
        let fields = build_fields(&r, 0);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_ref()).collect();
        assert_eq!(names, ["a", "a (2)", "b", "c", "d"]);
        assert!(matches!(fields[0].value, FieldValue::Null));
        assert!(matches!(fields[1].value, FieldValue::Empty));
        assert!(matches!(
            &fields[2].value,
            FieldValue::Text {
                multiline: false,
                omitted: 0,
                ..
            }
        ));
        assert!(
            matches!(&fields[3].value, FieldValue::Json { shown, .. } if shown.contains("\n    1,"))
        );
        assert!(matches!(
            &fields[4].value,
            FieldValue::Text {
                multiline: true,
                ..
            }
        ));
    }

    #[test]
    fn long_values_are_capped_but_counted() {
        let (shown, omitted) = cap(&"é".repeat(MAX_SHOWN_CHARS + 5));
        assert_eq!(shown.chars().count(), MAX_SHOWN_CHARS);
        assert_eq!(omitted, 5);
        assert_eq!(cap("short"), ("short".to_string(), 0));
    }

    #[test]
    fn missing_row_has_no_fields() {
        assert!(build_fields(&result(&["a"], &[Some("1")]), 3).is_empty());
    }
}
