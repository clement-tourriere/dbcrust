//! The left pane: title and New button, search, the All / Saved / Recent /
//! Docker filters, one virtualized list, and the fixed Vault action.

use super::details::entry_menu;
use super::ui::{ROW_HEIGHT, state_dot};
use super::{HomeView, Pane, vault};
use crate::app_state::AppState;
use crate::widgets::{engine_badge, env_tag};
use dbcrust_gui_core::launcher::{Entry, Filter, ListRow, Section, Source};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    menu::ContextMenuExt as _,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl HomeView {
    pub(super) fn render_left(
        &mut self,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let width = if narrow { 280. } else { 320. };
        let list_focused = self.list_focus.contains_focused(window, cx)
            || self.search.focus_handle(cx).is_focused(window);

        let body = match &self.listing.docker_block {
            Some(note) if self.listing.rows.is_empty() => self.render_docker_block(note, cx),
            _ if self.listing.rows.is_empty() => self.render_nothing_found(cx),
            _ => div()
                .id("connection-list")
                .size_full()
                .relative()
                .child(
                    list(
                        self.list.clone(),
                        cx.processor(move |this, ix, window, cx| {
                            this.render_row(ix, list_focused, window, cx)
                        }),
                    )
                    .size_full(),
                )
                .vertical_scrollbar(&self.list)
                .into_any_element(),
        };

        // Nothing saved or opened yet: say where connections will live
        let first_run_hint = (self.sessions.is_empty()
            && self.recents.is_empty()
            && self.search.read(cx).value().is_empty()
            && self.filter == Filter::All)
            .then(|| {
                h_flex()
                    .mx_4()
                    .mt_3()
                    .mb_1()
                    .p_3()
                    .gap_3()
                    .items_start()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.sidebar_border)
                    .bg(theme.muted.opacity(0.3))
                    .child(
                        Icon::new(IconName::Bookmark)
                            .size(px(16.))
                            .text_color(theme.muted_foreground)
                            .flex_none(),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child("Saved connections appear here"),
                            )
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                "Connect once and it shows up under Recent. Save it \
                                         to keep it.",
                            )),
                    )
            });

        v_flex()
            .w(px(width))
            .flex_none()
            .h_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(self.render_list_header(cx))
            .child(
                div().flex_none().px_3().child(
                    Input::new(&self.search)
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size(px(14.))),
                ),
            )
            .child(self.render_filters(cx))
            .child(
                v_flex()
                    .id("connection-list-area")
                    .track_focus(&self.list_focus)
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .children(first_run_hint)
                    .child(div().flex_1().min_h_0().child(body)),
            )
            .child(self.render_vault_action(cx))
            .into_any_element()
    }

    fn render_list_header(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_none()
            .h(px(56.))
            .px_4()
            .items_center()
            .justify_between()
            .child(div().text_base().font_semibold().child("Connections"))
            .child(
                Button::new("new-connection")
                    .small()
                    .outline()
                    .icon(IconName::Plus)
                    .label("New")
                    .tooltip(format!("New connection ({}N)", super::ui::modifier_label()))
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_form(window, cx))),
            )
    }

    fn render_filters(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (selected_bg, hover_bg) = (theme.secondary, theme.list_hover);
        let (strong, quiet) = (theme.foreground, theme.muted_foreground);
        h_flex()
            .flex_none()
            .h(px(44.))
            .px_3()
            .gap_1()
            .items_center()
            .children(Filter::ALL.into_iter().map(|filter| {
                let selected = self.filter == filter;
                div()
                    .id(SharedString::from(format!("filter-{}", filter.label())))
                    .flex_none()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_xs()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(selected_bg).text_color(strong).font_semibold()
                    })
                    .when(!selected, |this| {
                        this.text_color(quiet)
                            .hover(move |style| style.bg(hover_bg).text_color(strong))
                    })
                    .child(filter.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
            }))
    }

    fn render_vault_action(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .h(px(52.))
            .px_3()
            .items_center()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(
                Button::new("connect-vault")
                    .ghost()
                    .w_full()
                    .justify_start()
                    .icon(IconName::KeyRound)
                    .label("Connect with Vault…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let read_only = this
                            .selected
                            .as_deref()
                            .map(|key| this.meta(key).read_only)
                            .unwrap_or(false);
                        vault::open_wizard(Default::default(), read_only, window, cx);
                    })),
            )
    }

    /// The list when a search or a filter matches nothing.
    fn render_nothing_found(&self, cx: &Context<Self>) -> AnyElement {
        let query = self.search.read(cx).value().trim().to_string();
        let (title, body) = if !query.is_empty() {
            (
                format!("Nothing matches “{query}”"),
                "Try a name, a host, an engine or an image.".to_string(),
            )
        } else {
            match self.filter {
                Filter::Saved => (
                    "No saved connections".to_string(),
                    "Fill in the form and choose Save.".to_string(),
                ),
                Filter::Recent => (
                    "No recent connections".to_string(),
                    "Connections you open appear here, newest first.".to_string(),
                ),
                _ => (
                    "Nothing here yet".to_string(),
                    "Create a connection to get started.".to_string(),
                ),
            }
        };
        super::ui::state_block(IconName::Inbox, title, body, Vec::new(), cx).into_any_element()
    }

    /// One row of the list.
    pub(super) fn render_row(
        &mut self,
        ix: usize,
        list_focused: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.listing.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        // The list lays rows out at their content width: pin them to the pane's
        let content = match row {
            ListRow::Header { section, count } => self.render_section_header(section, count, cx),
            ListRow::Note(note) => self.render_docker_note(&note, cx),
            ListRow::Entry(entry_ix) => match self.listing.entries.get(entry_ix).cloned() {
                Some(entry) => self.render_entry(&entry, list_focused, cx),
                None => div().into_any_element(),
            },
        };
        div().w_full().child(content).into_any_element()
    }

    fn render_section_header(
        &self,
        section: Section,
        count: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .h(px(32.))
            .px_4()
            .pt_1()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(section.label().to_uppercase())
                    .when(count > 0, |this| {
                        this.child(
                            div()
                                .font_normal()
                                .text_color(theme.muted_foreground.opacity(0.7))
                                .child(count.to_string()),
                        )
                    }),
            )
            .when(section == Section::Docker, |this| {
                this.child(
                    Button::new("docker-refresh")
                        .ghost()
                        .xsmall()
                        .icon(IconName::RefreshCw)
                        .tooltip("Refresh containers")
                        .loading(self.docker_in_flight)
                        .on_click(cx.listener(|this, _, _, cx| this.discover_docker(cx))),
                )
            })
            .into_any_element()
    }

    fn render_entry(&self, entry: &Entry, list_focused: bool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self.pane == Pane::Details && self.selected.as_deref() == Some(&entry.key);
        let live = self.live_for(entry, None, cx).is_some();
        let attempt_running = {
            let state = AppState::global(cx);
            state
                .read(cx)
                .attempt_for(&entry.key)
                .is_some_and(|attempt| attempt.is_running())
        };
        let stopped = matches!(entry.source, Source::Docker(_)) && entry.trailing == "Stopped";
        let tag = self.env_tag(&entry.key);

        let badge = match (&entry.db_type, entry.source) {
            (Some(db), _) => engine_badge(db, 32., cx),
            (None, _) => super::ui::docker_badge(32., cx),
        };
        let badge = div()
            .relative()
            .flex_none()
            .when(stopped, |this| this.opacity(0.55))
            .child(badge)
            .when(live, |this| {
                this.child(
                    div()
                        .absolute()
                        .right(px(-3.))
                        .bottom(px(-3.))
                        .p(px(2.))
                        .rounded_full()
                        .bg(if selected {
                            theme.list_active
                        } else {
                            theme.sidebar
                        })
                        .child(state_dot(theme.success)),
                )
            });

        let trailing = if attempt_running {
            h_flex()
                .flex_none()
                .gap_1p5()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Spinner::new().small())
                .into_any_element()
        } else if live {
            div()
                .flex_none()
                .text_xs()
                .font_medium()
                .text_color(theme.success)
                .child("Connected")
                .into_any_element()
        } else if !entry.trailing.is_empty() {
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(entry.trailing.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let key = entry.key.clone();
        let (ring, active, hover) = (theme.ring, theme.list_active, theme.list_hover);
        let row = h_flex()
            .id(SharedString::from(format!("entry-{}", entry.key)))
            .w_full()
            .px_2()
            .min_h(px(ROW_HEIGHT))
            .gap_3()
            .items_center()
            .rounded(px(8.))
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(active)
                    .when(list_focused, |this| this.border_color(ring.opacity(0.7)))
            })
            .when(!selected, |this| this.hover(move |style| style.bg(hover)))
            .child(badge)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_medium()
                                    .when(stopped, |this| this.opacity(0.65))
                                    .child(entry.title.clone()),
                            )
                            .children(tag.as_ref().map(|tag| env_tag(tag, cx))),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(entry.subtitle.clone()),
                    ),
            )
            .child(trailing)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.list_focus, cx);
                this.select(&key, cx);
                if event.click_count() >= 2 {
                    this.connect_key(&key, None, cx);
                }
            }));
        let view = cx.entity();
        let menu_entry = entry.clone();
        let select_key = entry.key.clone();
        let row = row
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| this.select(&select_key, cx)),
            )
            .context_menu(move |menu, window, _| {
                entry_menu(&view, &menu_entry, stopped, true, menu, window)
            });
        div().w_full().px_2().child(row).into_any_element()
    }
}
