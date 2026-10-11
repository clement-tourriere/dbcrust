//! The About dialog.

use crate::assets::brand_mark;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogContent, DialogFooter},
    h_flex, v_flex,
};
use gpui_kit::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DOCS_URL: &str = "https://clement-tourriere.github.io/dbcrust/";
pub const GITHUB_URL: &str = "https://github.com/clement-tourriere/dbcrust";

pub const DESCRIPTION: &str = "A fast, keyboard-first workbench for PostgreSQL, MySQL, SQLite, \
ClickHouse, MongoDB, Elasticsearch and SQL over Parquet, CSV and JSON files.";

/// Open the About dialog on `window`.
pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, _| {
        dialog
            .width(px(420.))
            .close_button(false)
            .content(|content, _, cx| about_content(content, cx))
    });
}

fn about_content(content: DialogContent, cx: &mut App) -> DialogContent {
    let muted = cx.theme().muted_foreground;
    content
        .child(
            v_flex()
                .items_center()
                .gap_3()
                .pt_4()
                .pb_2()
                .child(brand_mark(px(64.)))
                .child(
                    v_flex()
                        .items_center()
                        .gap_0p5()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("DBCrust"),
                        )
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(muted)
                                .child(format!("Version {VERSION}")),
                        ),
                )
                .child(
                    div()
                        .max_w(px(340.))
                        .text_sm()
                        .text_center()
                        .text_color(muted)
                        .child(DESCRIPTION),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("MIT license · Built with GPUI Kit"),
                ),
        )
        .child(
            DialogFooter::new()
                .justify_center()
                .child(
                    Button::new("about-docs")
                        .outline()
                        .small()
                        .icon(IconName::BookOpen)
                        .label("Documentation")
                        .on_click(|_, _, cx| cx.open_url(DOCS_URL)),
                )
                .child(
                    Button::new("about-github")
                        .outline()
                        .small()
                        .icon(IconName::Github)
                        .label("GitHub")
                        .on_click(|_, _, cx| cx.open_url(GITHUB_URL)),
                )
                .child(
                    h_flex().child(
                        Button::new("about-close")
                            .primary()
                            .small()
                            .label("Close")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                ),
        )
}
