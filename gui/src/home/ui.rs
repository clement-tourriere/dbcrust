//! Presentation primitives shared by the Connections page's panes. Theme
//! tokens only; the literal colours (engine and environment hues) live in
//! `widgets.rs`.

use crate::widgets::source_badge;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, StyledExt as _, h_flex, scroll::ScrollableElement as _,
    skeleton::Skeleton, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Row height of the list; the rows grow with larger interface text.
pub(super) const ROW_HEIGHT: f32 = 52.;
/// Width of the right pane's content column.
pub(super) const CONTENT_MAX_WIDTH: f32 = 640.;
/// Height of the sticky footer.
pub(super) const FOOTER_HEIGHT: f32 = 56.;

/// ⌘ on macOS, `Ctrl+` elsewhere.
pub(super) fn modifier_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}

/// A key cap: `⌘E`, `↵`, `Esc`.
pub(super) fn key_cap(text: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(5.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.5))
        .text_color(theme.muted_foreground)
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// `[↵] Connect`: a shortcut and what it does.
pub(super) fn shortcut_hint(keys: &[&str], label: &'static str, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .children(keys.iter().map(|key| key_cap(SharedString::from(*key), cx)))
        .child(label)
}

/// Quiet monospace text for targets, images, ports.
pub(super) fn mono(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .min_w_0()
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(12.5))
        .child(text.into())
}

/// Dimmed monospace caption (targets, images, ports).
pub(super) fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    mono(text, cx)
        .truncate()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
}

/// A dot for a state (connected, stopped).
pub(super) fn state_dot(color: Hsla) -> Div {
    div().size(px(7.)).flex_none().rounded_full().bg(color)
}

/// One `label  value` line of the details pane.
pub(super) fn property(label: &'static str, value: impl IntoElement, cx: &App) -> impl IntoElement {
    h_flex()
        .w_full()
        .min_h(px(34.))
        .py(px(7.))
        .gap_4()
        .items_start()
        .child(
            div()
                .w(px(124.))
                .flex_none()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().text_sm().child(value))
}

/// A group of properties on a bordered surface.
pub(super) fn property_card(rows: Vec<AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    let last = rows.len().saturating_sub(1);
    v_flex()
        .w_full()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .px_4()
        .children(rows.into_iter().enumerate().map(move |(ix, row)| {
            div()
                .w_full()
                .when(ix < last, |this| {
                    this.border_b_1().border_color(theme.border.opacity(0.7))
                })
                .child(row)
        }))
}

/// Small caps over a block of the form or details pane.
pub(super) fn group_label(text: &'static str, cx: &App) -> Div {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// The list's skeleton while Docker loads: rows shaped like the real ones.
pub(super) fn skeleton_row(ix: usize, cx: &App) -> Div {
    // The theme's skeleton token nearly vanishes on the sidebar surface
    let fill = cx.theme().foreground.opacity(0.10);
    h_flex()
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_4()
        .gap_3()
        .child(
            Skeleton::new()
                .size(px(32.))
                .rounded(px(8.))
                .bg(fill)
                .flex_none(),
        )
        .child(
            v_flex()
                .flex_1()
                .gap(px(7.))
                .child(
                    Skeleton::new()
                        .w(px(112. + 28. * (ix % 3) as f32))
                        .h(px(11.))
                        .bg(fill),
                )
                .child(
                    Skeleton::new()
                        .secondary()
                        .w(px(168.))
                        .h(px(9.))
                        .bg(fill.opacity(0.7)),
                ),
        )
}

/// A full-height explanation: what is missing and what to do about it.
pub(super) fn state_block(
    icon: IconName,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
    actions: Vec<AnyElement>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .items_center()
        .gap_2()
        .px_6()
        .py_8()
        .text_center()
        .child(
            div()
                .p(px(10.))
                .rounded(theme.radius_lg)
                .bg(theme.muted)
                .text_color(theme.muted_foreground)
                .child(Icon::new(icon).size(px(18.))),
        )
        .child(div().text_sm().font_medium().child(title.into()))
        .child(
            div()
                .max_w(px(260.))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(body.into()),
        )
        .when(!actions.is_empty(), |this| {
            this.child(h_flex().mt_2().gap_2().children(actions))
        })
}

/// Badge of a Docker container whose engine is unknown, and of the Vault
/// entry: drawn like an engine badge.
pub(super) fn docker_badge(size: f32, cx: &App) -> Div {
    source_badge("Dkr", 205., size, cx)
}

pub(super) fn vault_badge(size: f32, cx: &App) -> Div {
    source_badge("Vlt", 45., size, cx)
}

/// The right pane's scrolling body: content centred in a column of at most
/// [`CONTENT_MAX_WIDTH`].
pub(super) fn scroll_body(id: &'static str, content: impl IntoElement) -> impl IntoElement {
    v_flex()
        .id(id)
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_y_scrollbar()
        .child(
            h_flex().w_full().justify_center().child(
                v_flex()
                    .w_full()
                    .max_w(px(CONTENT_MAX_WIDTH + 64.))
                    .px_8()
                    .pt_8()
                    .pb_6()
                    .gap_5()
                    .child(content),
            ),
        )
}

/// The sticky footer: `left` (a hint or the target) and `right` (buttons),
/// aligned with the content column above.
pub(super) fn footer_bar(
    left: impl IntoElement,
    right: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .w_full()
        .h(px(FOOTER_HEIGHT))
        .justify_center()
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            h_flex()
                .w_full()
                .max_w(px(CONTENT_MAX_WIDTH + 64.))
                .px_8()
                .gap_4()
                .items_center()
                .child(div().flex_1().min_w_0().overflow_hidden().child(left))
                .child(h_flex().flex_none().gap_2().items_center().child(right)),
        )
}
