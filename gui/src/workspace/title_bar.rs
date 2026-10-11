//! Window chrome: app mark, sidebar toggle, command search, theme and
//! settings buttons.
//!
//! There is deliberately no connection selector here: where a statement runs
//! is chosen in the query toolbar, next to Run, so there is one place to look
//! and one place to change it. The title bar holds only global commands.

use super::Workspace;
use crate::actions::{CommandPalette, OpenSettings, ToggleSidebar, ToggleTheme};
use crate::assets::brand_mark;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

impl Workspace {
    pub(super) fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().is_dark();
        let shortcut = Kbd::global_binding_for_action(&CommandPalette, window);

        TitleBar::new()
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .items_center()
                    .gap_3()
                    .pr_2()
                    // Left: identity, after the traffic-light inset
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(brand_mark(px(16.)))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("DBCrust"),
                            )
                            .child(
                                Button::new("toggle-sidebar")
                                    .ghost()
                                    .small()
                                    .icon(if self.sidebar_visible {
                                        IconName::PanelLeftClose
                                    } else {
                                        IconName::PanelLeftOpen
                                    })
                                    .tooltip_with_action(
                                        if self.sidebar_visible {
                                            "Hide sidebar"
                                        } else {
                                            "Show sidebar"
                                        },
                                        &ToggleSidebar,
                                        None,
                                    )
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(Box::new(ToggleSidebar), cx);
                                    })),
                            ),
                    )
                    // Center: the command palette, the way in to everything
                    .child(
                        Button::new("search-commands")
                            .outline()
                            .small()
                            .icon(IconName::Search)
                            .label("Search commands")
                            .when_some(shortcut, |this, kbd| this.child(kbd.appearance(false)))
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(CommandPalette), cx);
                            })),
                    )
                    // Right: global toggles
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_end()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("toggle-theme")
                                    .ghost()
                                    .small()
                                    .icon(if dark { IconName::Sun } else { IconName::Moon })
                                    .tooltip_with_action(
                                        if dark {
                                            "Switch to light theme"
                                        } else {
                                            "Switch to dark theme"
                                        },
                                        &ToggleTheme,
                                        None,
                                    )
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(Box::new(ToggleTheme), cx);
                                    })),
                            )
                            .child(
                                Button::new("open-settings")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Settings)
                                    .tooltip_with_action("Settings", &OpenSettings, None)
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(Box::new(OpenSettings), cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
