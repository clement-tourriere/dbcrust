//! Bottom status bar: the connection of the tab in front on the left (a
//! short `shop · SQLite`, the full endpoint in its tooltip); the running /
//! failed state of the query, caret position and connection count on the
//! right.
//!
//! Finished results are summarised once, in the results footer: the status
//! bar never repeats them.

use super::Workspace;
use crate::app_state::AppState;
use crate::query::QueryStatus;
use crate::widgets::connection_accent;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex, separator::Separator, spinner::Spinner,
    status_bar::StatusBar, tooltip::Tooltip,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::Duration;

/// `12 ms`, `1.24 s`, `1 m 05 s`.
pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    let millis = elapsed.as_millis();
    if millis < 1_000 {
        format!("{millis} ms")
    } else if millis < 60_000 {
        format!("{:.2} s", elapsed.as_secs_f64())
    } else {
        let seconds = elapsed.as_secs();
        format!("{} m {:02} s", seconds / 60, seconds % 60)
    }
}

impl Workspace {
    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let state = AppState::global(cx);
        let state = state.read(cx);

        // The connection of the tab in front; tabs without one (the
        // connection page, settings) show the one global commands act on
        let focused = self
            .active_tab()
            .and_then(|tab| tab.connection(cx))
            .and_then(|id| state.connection(id))
            .or_else(|| state.active());

        let left = match focused {
            Some(live) => {
                let summary = &live.summary;
                let tooltip = format!(
                    "{} · {}\n{}",
                    summary.label,
                    summary.type_name(),
                    summary.target()
                );
                h_flex()
                    .id("status-connection")
                    .gap_1p5()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .size_1p5()
                            .flex_none()
                            .rounded_full()
                            .bg(connection_accent(live.accent, cx)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(cx.theme().foreground)
                            .child(summary.label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .child(format!("· {}", summary.type_name())),
                    )
                    .when(summary.read_only, |this| {
                        this.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .items_center()
                                .text_color(cx.theme().warning)
                                .child(Icon::new(IconName::Lock).xsmall())
                                .child("read-only"),
                        )
                    })
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .into_any_element()
            }
            None => h_flex()
                .gap_1p5()
                .items_center()
                .child(Icon::new(IconName::Unplug).xsmall())
                .child("Not connected")
                .into_any_element(),
        };

        let query = self.active_query_tab().map(|tab| {
            let tab = tab.read(cx);
            (tab.status(), tab.cursor(cx))
        });
        let (status, cursor) = match query {
            Some((status, cursor)) => (Some(status), cursor),
            None => (None, None),
        };

        // Only what is happening or went wrong: a finished query's rows and
        // time are in the results footer
        let status_element = status.and_then(|status| match status {
            QueryStatus::Idle | QueryStatus::Done { .. } => None,
            QueryStatus::Running { since } => Some(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_color(cx.theme().foreground)
                    .child(Spinner::new().xsmall())
                    .child(format!("Running {}", format_elapsed(since.elapsed())))
                    .into_any_element(),
            ),
            QueryStatus::Failed => Some(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_color(cx.theme().danger)
                    .child(Icon::new(IconName::CircleAlert).xsmall())
                    .child("Failed")
                    .into_any_element(),
            ),
            QueryStatus::Cancelled => Some(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Icon::new(IconName::CircleX).xsmall())
                    .child("Cancelled")
                    .into_any_element(),
            ),
        });

        let live_count = state.connections().len();
        let mono = cx.theme().mono_font_family.clone();

        StatusBar::new()
            .text_color(muted)
            .left(left)
            .right(
                h_flex()
                    .gap_3()
                    .items_center()
                    .when_some(status_element, |this, status| {
                        this.child(status).child(Separator::vertical().h_3())
                    })
                    .when_some(cursor, |this, (line, column)| {
                        this.child(
                            div()
                                .font_family(mono)
                                .child(format!("Ln {line}, Col {column}")),
                        )
                        .child(Separator::vertical().h_3())
                    })
                    .child(div().child(match live_count {
                        0 => "No connections".to_string(),
                        1 => "1 connection".to_string(),
                        n => format!("{n} connections"),
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn formats_elapsed_time() {
        let cases = [
            (0, "0 ms"),
            (12, "12 ms"),
            (999, "999 ms"),
            (1_000, "1.00 s"),
            (1_240, "1.24 s"),
            (63_000, "1 m 03 s"),
            (3_725_000, "62 m 05 s"),
        ];
        for (millis, expected) in cases {
            assert_eq!(format_elapsed(Duration::from_millis(millis)), expected);
        }
    }
}
