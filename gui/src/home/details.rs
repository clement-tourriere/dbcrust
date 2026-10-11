//! The right pane for a selected item: what it is, how it will connect, and
//! the actions (Connect, Test, Edit, and the rest in the overflow menu).

use super::attempts::{self, EditHandler};
use super::ui::{
    docker_badge, footer_bar, mono, property, property_card, scroll_body, shortcut_hint,
    state_block, state_dot,
};
use super::{HomeView, ui};
use crate::app_state::AppState;
use crate::widgets::{engine_badge, env_tag};
use dbcrust_gui_core::ConnectionId;
use dbcrust_gui_core::DatabaseTypeExt;
use dbcrust_gui_core::launcher::{DockerListState, Entry, Publish, Source, publish_state};
use dbcrust_gui_core::url_builder as urls;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

impl HomeView {
    pub(super) fn render_details(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.selected_entry().cloned() else {
            return self.render_no_selection(cx);
        };
        let content = self.details_content(&entry, cx);
        let footer = self.details_footer(&entry, window, cx);
        v_flex()
            .size_full()
            .child(scroll_body("details-body", content))
            .child(footer)
            .into_any_element()
    }

    fn render_no_selection(&self, cx: &Context<Self>) -> AnyElement {
        let new = Button::new("empty-new")
            .primary()
            .icon(IconName::Plus)
            .label("New connection")
            .on_click(cx.listener(|this, _, window, cx| this.open_new_form(window, cx)))
            .into_any_element();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(state_block(
                IconName::PlugZap,
                "Pick a connection",
                "Choose one on the left to see how it connects, or create a new one.",
                vec![new],
                cx,
            ))
            .into_any_element()
    }

    fn container_of(&self, entry: &Entry) -> Option<&dbcrust_gui_core::docker::DockerContainer> {
        let Source::Docker(ix) = entry.source else {
            return None;
        };
        match &self.docker {
            DockerListState::Ready { containers, .. } => containers.get(ix),
            _ => None,
        }
    }

    fn details_content(&self, entry: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let meta = self.meta(&entry.key);
        let tag = meta.env_tag();
        let forced_read_only = forced_read_only(cx);
        let read_only = meta.read_only || forced_read_only;
        let live_id = self.live_for(entry, None, cx);
        let live = live_id.is_some();
        let container = self.container_of(entry);
        let engine_name = entry
            .db_type
            .as_ref()
            .map_or("Database", |db| db.display_name());

        // Header: badge, name, one line of identity
        let badge = match &entry.db_type {
            Some(db) => engine_badge(db, 44., cx),
            None => docker_badge(44., cx),
        };
        let subline = h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(engine_name)
            .when(container.is_some(), |this| this.child("·").child("Docker"))
            .children(tag.as_ref().map(|tag| env_tag(tag, cx)))
            .when(read_only, |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_color(theme.warning)
                        .child(Icon::new(IconName::Lock).size(px(12.)))
                        .child("Read-only"),
                )
            });
        let header = h_flex().gap_4().items_center().child(badge).child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .truncate()
                        .text_size(px(22.))
                        .line_height(relative(1.2))
                        .font_semibold()
                        .child(entry.title.clone()),
                )
                .child(subline),
        );

        // The state of the connection, then anything the attempts say
        let state_line = self.state_line(entry, live, cx);
        let view = cx.entity();
        let edit_entry = entry.clone();
        let edit: EditHandler = Rc::new(move |window, cx| {
            let entry = edit_entry.clone();
            view.update(cx, |this, cx| this.edit_entry(&entry, false, window, cx));
        });
        let panels = attempts::attempt_panels(&entry.key, Some(edit), cx);

        // Properties
        let mut rows: Vec<AnyElement> = Vec::new();
        let value = |text: String| div().min_w_0().child(text).into_any_element();
        match (container, entry.source) {
            (Some(container), _) => {
                rows.push(
                    property(
                        "Container",
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(state_dot(if container.running {
                                theme.success
                            } else {
                                theme.muted_foreground
                            }))
                            .child(if container.running {
                                "Running"
                            } else {
                                "Stopped"
                            })
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .truncate()
                                    .child(container.status.clone()),
                            ),
                        cx,
                    )
                    .into_any_element(),
                );
                if let DockerListState::Ready {
                    context: Some(context),
                    ..
                } = &self.docker
                {
                    rows.push(
                        property("Context", mono(context.clone(), cx), cx).into_any_element(),
                    );
                }
                rows.push(
                    property("Image", mono(container.image.clone(), cx), cx).into_any_element(),
                );
                rows.push(
                    property(
                        "Database port",
                        match container.container_port {
                            Some(port) => mono(port.to_string(), cx).into_any_element(),
                            None => div()
                                .text_color(theme.muted_foreground)
                                .child("Unknown")
                                .into_any_element(),
                        },
                        cx,
                    )
                    .into_any_element(),
                );
                let publish = publish_state(container);
                rows.push(
                    property(
                        "Publishing",
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(publish.status_line())
                            .when(matches!(publish, Publish::NotPublished), |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("DBCrust picks the route"),
                                )
                            }),
                        cx,
                    )
                    .into_any_element(),
                );
                rows.push(
                    property(
                        "Route",
                        div()
                            .text_color(theme.muted_foreground)
                            .child(self.route_text(entry, live_id, cx)),
                        cx,
                    )
                    .into_any_element(),
                );
            }
            (None, Source::Session(_) | Source::Recent(_)) => {
                rows.push(
                    property("Connection", mono(self.target_text(entry), cx), cx)
                        .into_any_element(),
                );
                rows.push(property("Engine", engine_name, cx).into_any_element());
                if let Some(age) = self.last_used(entry) {
                    rows.push(property("Last used", value(age), cx).into_any_element());
                }
            }
            (None, Source::Docker(_)) => {}
        }
        rows.push(
            property(
                "Environment",
                match &tag {
                    Some(tag) => env_tag(tag, cx).into_any_element(),
                    None => div()
                        .text_color(theme.muted_foreground)
                        .child("Not set")
                        .into_any_element(),
                },
                cx,
            )
            .into_any_element(),
        );
        let access_key = entry.key.clone();
        rows.push(
            property(
                "Access",
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Switch::new("access-read-only")
                            .checked(read_only)
                            .disabled(forced_read_only)
                            .label("Open read-only")
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.set_read_only_preference(&access_key, *checked, cx);
                            })),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        if forced_read_only {
                            "Settings make every connection read-only"
                        } else if read_only {
                            "Writes are blocked"
                        } else {
                            "Reads and writes"
                        },
                    )),
                cx,
            )
            .into_any_element(),
        );

        let note = self.credentials_note(entry);

        v_flex()
            .w_full()
            .gap_5()
            .child(header)
            .children(state_line)
            .children(panels)
            .child(property_card(rows, cx))
            .children(note.map(|note| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note)
            }))
            .into_any_element()
    }

    /// "Connected" / "Not connected" with its dot.
    fn state_line(&self, entry: &Entry, live: bool, cx: &App) -> Option<AnyElement> {
        let theme = cx.theme();
        let running = attempts::is_busy(&entry.key, cx);
        if running {
            return None;
        }
        if live {
            return Some(
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .font_medium()
                    .text_color(theme.success)
                    .child(state_dot(theme.success))
                    .child("Connected")
                    .into_any_element(),
            );
        }
        None
    }

    /// The target of a saved or recent connection, as stored (no password).
    fn target_text(&self, entry: &Entry) -> String {
        match entry.source {
            Source::Session(ix) => self
                .sessions
                .get(ix)
                .map(|session| session.target.clone())
                .unwrap_or_default(),
            Source::Recent(ix) => self
                .recents
                .get(ix)
                .map(|recent| {
                    let url = recent.url.split(" # ").next().unwrap_or(&recent.url);
                    urls::parse_target(url)
                        .map(|target| urls::describe_target(&target))
                        .unwrap_or_else(|| recent.display_name.clone())
                })
                .unwrap_or_default(),
            Source::Docker(_) => String::new(),
        }
    }

    /// When the connection was last opened, from the recent list.
    fn last_used(&self, entry: &Entry) -> Option<String> {
        let now = super::unix_now();
        let age = |recent: &dbcrust_gui_core::store::RecentEntry| {
            urls::relative_age((now - recent.timestamp.timestamp()).max(0))
        };
        match entry.source {
            Source::Recent(ix) => self.recents.get(ix).map(age),
            Source::Session(ix) => {
                let session = self.sessions.get(ix)?;
                self.recents
                    .iter()
                    .find(|recent| {
                        recent.db_type == session.db_type && recent.display_name == session.target
                    })
                    .map(age)
            }
            Source::Docker(_) => None,
        }
    }

    /// What the route will be, or was.
    fn route_text(&self, entry: &Entry, live: Option<ConnectionId>, cx: &App) -> String {
        let Some(container) = self.container_of(entry) else {
            return String::new();
        };
        if !container.running {
            return "Start the container to connect".to_string();
        }
        // After connecting, the route the backend actually used
        let used = live.and_then(|id| {
            AppState::global(cx)
                .read(cx)
                .connection(id)
                .and_then(|live| live.handle.route())
        });
        match (used, publish_state(container)) {
            (Some(route), _) => attempts::route_text(&route),
            (None, Publish::Published { host, port }) => format!("Direct through {host}:{port}"),
            (None, _) => "Chosen when you connect".to_string(),
        }
    }

    /// A reminder about credentials, where it matters.
    fn credentials_note(&self, entry: &Entry) -> Option<String> {
        match entry.source {
            Source::Docker(_) => Some(
                "User, password and database come from the container's environment variables."
                    .to_string(),
            ),
            Source::Session(ix) => {
                let session = self.sessions.get(ix)?;
                (!session.db_type.is_file_based()).then(|| {
                    "Passwords are never stored with a saved connection. DBCrust uses \
                     ~/.dbcrust, pgpass or .my.cnf."
                        .to_string()
                })
            }
            Source::Recent(_) => None,
        }
    }

    fn details_footer(
        &self,
        entry: &Entry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = entry.key.clone();
        let meta = self.meta(&key);
        let forced = forced_read_only(cx);
        let read_only = meta.read_only || forced;
        let live = self.live_for(entry, Some(read_only), cx).is_some();
        let busy = attempts::is_busy(&key, cx);
        let container = self.container_of(entry);
        let stopped = container.is_some_and(|container| !container.running);
        let editable = true;

        let left: AnyElement = if stopped {
            h_flex()
                .gap_2()
                .items_center()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Info).size(px(13.)))
                .child("Stopped containers can't be connected to. Start it, then refresh.")
                .into_any_element()
        } else {
            h_flex()
                .gap_4()
                .child(shortcut_hint(&["↵"], "Connect", cx))
                // The footer shares its width with four buttons
                .when(window.viewport_size().width >= px(1100.), |this| {
                    this.child(shortcut_hint(&["⇧", "↵"], "Read-only", cx))
                })
                .child(shortcut_hint(
                    &[&format!("{}E", ui::modifier_label())],
                    "Edit",
                    cx,
                ))
                .into_any_element()
        };

        let connect_key = key.clone();
        let test_key = key.clone();
        let view = cx.entity();
        let edit_entry = entry.clone();
        let menu_entry = entry.clone();
        let more = Button::new("details-more")
            .ghost()
            .icon(IconName::Ellipsis)
            .tooltip("More actions")
            .dropdown_menu(move |menu, window, _| {
                entry_menu(&view, &menu_entry, stopped, false, menu, window)
            });

        let edit = Button::new("details-edit")
            .ghost()
            .label("Edit")
            .tooltip(format!("Edit ({}E)", ui::modifier_label()))
            .disabled(!editable)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_entry(&edit_entry, false, window, cx);
            }));
        let test = Button::new("details-test")
            .outline()
            .label("Test")
            .disabled(stopped || busy)
            .tooltip("Connect and close again: nothing is opened or recorded")
            .on_click(cx.listener(move |this, _, _, cx| this.test_key(&test_key, cx)));
        let connect = Button::new("details-connect")
            .primary()
            .label(if live { "Open workspace" } else { "Connect" })
            .disabled(stopped || busy)
            .loading(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.connect_key(&connect_key, None, cx)));

        footer_bar(
            left,
            h_flex()
                .gap_2()
                .child(more)
                .child(edit)
                .child(test)
                .child(connect),
            cx,
        )
        .into_any_element()
    }

    /// What "Copy target" puts on the clipboard: the target without any
    /// password.
    fn copyable_target(&self, entry: &Entry) -> String {
        match entry.source {
            Source::Docker(_) => entry.title.clone(),
            _ => {
                let text = self.target_text(entry);
                if text.is_empty() {
                    entry.subtitle.clone()
                } else {
                    text
                }
            }
        }
    }
}

/// The actions of one list entry: the overflow menu of the details footer,
/// and (with `full`) the context menu of its row, which also offers the
/// actions the footer has as buttons.
pub(super) fn entry_menu(
    view: &Entity<HomeView>,
    entry: &Entry,
    stopped: bool,
    full: bool,
    menu: PopupMenu,
    window: &mut Window,
) -> PopupMenu {
    let key = entry.key.clone();
    let connect = {
        let key = key.clone();
        window.listener_for(view, move |this, _: &ClickEvent, _, cx| {
            this.connect_key(&key, None, cx)
        })
    };
    let connect_ro = {
        let key = key.clone();
        window.listener_for(view, move |this, _: &ClickEvent, _, cx| {
            this.connect_key(&key, Some(true), cx)
        })
    };
    let edit = {
        let entry = entry.clone();
        window.listener_for(view, move |this, _: &ClickEvent, window, cx| {
            this.edit_entry(&entry, false, window, cx)
        })
    };
    let duplicate = {
        let entry = entry.clone();
        window.listener_for(view, move |this, _: &ClickEvent, window, cx| {
            this.edit_entry(&entry, true, window, cx)
        })
    };
    let copy = {
        let entry = entry.clone();
        window.listener_for(view, move |this, _: &ClickEvent, window, cx| {
            let text = this.copyable_target(&entry);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            window.push_notification(Notification::success("Copied the target"), cx);
        })
    };
    let remove = {
        let entry = entry.clone();
        window.listener_for(view, move |this, _: &ClickEvent, window, cx| {
            this.confirm_remove(&entry, window, cx)
        })
    };
    let removable = !matches!(entry.source, Source::Docker(_));
    let remove_label = match entry.source {
        Source::Session(_) => "Delete…",
        _ => "Remove from Recent…",
    };
    let modifier = ui::modifier_label();
    let mut menu = menu;
    if full {
        menu = menu
            .item(
                PopupMenuItem::element(move |_, cx| menu_row("Connect", "↵", cx))
                    .disabled(stopped)
                    .on_click(connect),
            )
            .item(
                PopupMenuItem::element(move |_, cx| menu_row("Connect read-only", "⇧↵", cx))
                    .disabled(stopped)
                    .on_click(connect_ro),
            )
            .item(
                PopupMenuItem::element(move |_, cx| menu_row("Edit", &format!("{modifier}E"), cx))
                    .on_click(edit),
            )
            .separator();
    } else {
        menu = menu.item(
            PopupMenuItem::element(move |_, cx| menu_row("Connect read-only", "⇧↵", cx))
                .disabled(stopped)
                .on_click(connect_ro),
        );
    }
    menu = menu
        .item(
            PopupMenuItem::new(match entry.source {
                Source::Session(_) => "Duplicate",
                _ => "Save as connection…",
            })
            .on_click(duplicate),
        )
        .item(PopupMenuItem::new("Copy target").on_click(copy));
    if removable {
        menu = menu.separator().item(
            PopupMenuItem::element(move |_, cx| {
                menu_row(remove_label, &format!("{modifier}⌫"), cx)
            })
            .on_click(remove),
        );
    }
    menu
}

/// A menu line with its shortcut at the right.
fn menu_row(label: &str, shortcut: &str, cx: &App) -> Div {
    h_flex()
        .w_full()
        .min_w(px(190.))
        .gap_6()
        .justify_between()
        .child(label.to_string())
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(shortcut.to_string()),
        )
}

/// Settings → "Open connections read-only" makes every connection
/// read-only. Read without blocking: render paths never wait for the store.
pub(super) fn forced_read_only(cx: &App) -> bool {
    thread_local! {
        static LAST_KNOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    let current = AppState::global(cx)
        .read(cx)
        .store()
        .try_read(|config| config.read_only_default);
    match current {
        Some(forced) => {
            LAST_KNOWN.with(|last| last.set(forced));
            forced
        }
        None => LAST_KNOWN.with(std::cell::Cell::get),
    }
}
