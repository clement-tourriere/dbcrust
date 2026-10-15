//! The workspace's tabs: what they hold, how they are titled, and the tab
//! bar that lists them.

use super::Workspace;
use crate::actions::{CloseTab, NewQuery, RenameTab};
use crate::app_state::AppState;
use crate::home::HomeView;
use crate::inspector::TableInspector;
use crate::query::{QueryStatus, QueryTab};
use crate::settings::SettingsView;
use crate::widgets::{connection_accent, db_badge_mini};
use dbcrust_gui_core::{ConnectionId, DatabaseType, TabId};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Escape, Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

/// What a tab shows. Every view is an entity so its state (editor text,
/// scroll, selection) survives while the tab is in the background.
pub(super) enum TabKind {
    Home(Entity<HomeView>),
    Query(Entity<QueryTab>),
    Table(Entity<TableInspector>),
    Settings(Entity<SettingsView>),
}

pub(super) struct WorkspaceTab {
    pub id: TabId,
    pub kind: TabKind,
    /// Keeps the tab's event subscription alive (query tabs).
    pub _subscription: Option<Subscription>,
    /// `display_url` of the connection a query tab was last bound to. It
    /// survives a disconnect so the draft re-binds when the same database
    /// opens again (also after a restart).
    pub connection_key: Option<String>,
}

/// The connection a tab is bound to, as the tab strip, its tooltip, the
/// overflow list and the "Go to tab…" palette show it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TabBinding {
    /// Not a database tab (Connections, Settings).
    None,
    /// A query tab with no open connection: never chosen, or closed since.
    Detached,
    Live(TabConnection),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabConnection {
    /// The short name the status bar shows (`shop`).
    pub name: SharedString,
    /// `SQLite`, `PostgreSQL`…: two connections can share a name.
    pub engine: &'static str,
    pub db_type: DatabaseType,
    /// Slot of the connection's accent colour.
    pub accent: usize,
}

impl TabBinding {
    /// `Top customers · shop (SQLite)`.
    pub fn describe(&self, title: &str) -> String {
        match self {
            TabBinding::None => title.to_string(),
            TabBinding::Detached => format!("{title} · no connection"),
            TabBinding::Live(connection) => {
                format!("{title} · {} ({})", connection.name, connection.engine)
            }
        }
    }
}

/// One tab as the "Go to tab…" palette lists it.
#[derive(Clone)]
pub(crate) struct TabChoice {
    pub id: TabId,
    pub title: SharedString,
    pub icon: IconName,
    pub binding: TabBinding,
}

/// The inline title editor of a query tab (double-click, F2, context menu).
pub(super) struct TabRename {
    pub tab: TabId,
    pub input: Entity<InputState>,
    pub _subscriptions: Vec<Subscription>,
}

/// How many closed query tabs "Reopen closed tab" remembers.
pub(super) const CLOSED_TABS_KEPT: usize = 20;

/// A closed query tab, kept in memory so the draft can come back.
pub(super) struct ClosedTab {
    pub title: String,
    pub sql: String,
    /// Where the tab sat in the tab bar.
    pub index: usize,
    /// The live connection it was bound to when it closed.
    pub connection: Option<ConnectionId>,
    /// `display_url` of that connection: finds it again if it was
    /// reconnected meanwhile, and re-binds the draft when it opens later.
    pub connection_key: Option<String>,
}

impl WorkspaceTab {
    pub fn new(kind: TabKind, subscription: Option<Subscription>) -> Self {
        Self {
            id: TabId::next(),
            kind,
            _subscription: subscription,
            connection_key: None,
        }
    }

    /// The connection page is pinned; everything else can be closed.
    pub fn is_closable(&self) -> bool {
        !matches!(self.kind, TabKind::Home(_))
    }

    pub fn title(&self, cx: &App) -> SharedString {
        match &self.kind {
            TabKind::Home(_) => "Connections".into(),
            TabKind::Query(tab) => tab.read(cx).title(),
            TabKind::Table(tab) => tab.read(cx).title(),
            TabKind::Settings(_) => "Settings".into(),
        }
    }

    /// A query tab whose text differs from the file or saved query it is
    /// bound to. Drafts (no target) are never "modified": autosave keeps them.
    pub fn is_modified(&self, cx: &App) -> bool {
        match &self.kind {
            TabKind::Query(tab) => tab.read(cx).is_modified(),
            _ => false,
        }
    }

    /// The connection this tab runs against, resolved against the open
    /// connections right now (so it follows rebinding, disconnects and
    /// reconnects).
    pub fn binding(&self, cx: &App) -> TabBinding {
        let connection = match &self.kind {
            TabKind::Home(_) | TabKind::Settings(_) => return TabBinding::None,
            TabKind::Query(_) | TabKind::Table(_) => self.connection(cx),
        };
        let state = AppState::global(cx);
        let state = state.read(cx);
        match connection.and_then(|id| state.connection(id)) {
            Some(live) => TabBinding::Live(TabConnection {
                name: live.summary.label.clone().into(),
                engine: live.summary.type_name(),
                db_type: live.summary.db_type.clone(),
                accent: live.accent,
            }),
            None => TabBinding::Detached,
        }
    }

    /// What hovering the tab says: `title · connection (engine)`, then where
    /// a query tab saves to.
    pub fn tooltip(&self, cx: &App) -> SharedString {
        let head = self.binding(cx).describe(&self.title(cx));
        match &self.kind {
            TabKind::Query(tab) => {
                let tab = tab.read(cx);
                let saved = match tab.target_description() {
                    Some(target) if tab.is_modified() => format!("{target} · modified"),
                    Some(target) => target,
                    None => "Not saved to a file or saved query".to_string(),
                };
                format!("{head}\n{saved}").into()
            }
            _ => head.into(),
        }
    }

    /// Only query tabs can be renamed.
    pub fn is_renamable(&self) -> bool {
        matches!(self.kind, TabKind::Query(_))
    }

    pub fn icon(&self) -> IconName {
        match &self.kind {
            TabKind::Home(_) => IconName::PlugZap,
            TabKind::Query(_) => IconName::SquareTerminal,
            TabKind::Table(_) => IconName::Table,
            TabKind::Settings(_) => IconName::Settings,
        }
    }

    /// The live connection this tab is bound to, if any.
    pub fn connection(&self, cx: &App) -> Option<ConnectionId> {
        match &self.kind {
            TabKind::Query(tab) => tab.read(cx).connection(),
            TabKind::Table(tab) => Some(tab.read(cx).connection()),
            TabKind::Home(_) | TabKind::Settings(_) => None,
        }
    }

    pub fn is_running(&self, cx: &App) -> bool {
        match &self.kind {
            TabKind::Query(tab) => matches!(tab.read(cx).status(), QueryStatus::Running { .. }),
            _ => false,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.kind {
            TabKind::Home(view) => view.focus_handle(cx),
            TabKind::Query(view) => view.focus_handle(cx),
            TabKind::Table(view) => view.focus_handle(cx),
            TabKind::Settings(view) => view.focus_handle(cx),
        }
    }

    /// Table inspectors are cached views: they render again when they (or
    /// a view inside them) notify, not on every repaint of the workspace (a
    /// status bar tick, a tab hover, a caret move in the status line).
    ///
    /// Query tabs are not cached themselves: they embed their editor and
    /// their results as cached views, and a cached view that renders again
    /// renders everything inside it (GPUI's `prepaint_view` sets
    /// `window.refreshing` for its subtree), so a cached tab would repaint
    /// its results and their grid on every caret blink. Uncached, the tab's
    /// own render is the toolbar and the split; the editor and the results
    /// repaint only when they change.
    ///
    /// What these views read from elsewhere reaches them through
    /// [`WorkspaceTab::notify_content`].
    pub fn content(&self) -> AnyElement {
        let fill = || StyleRefinement::default().size_full();
        match &self.kind {
            TabKind::Home(view) => view.clone().into_any_element(),
            TabKind::Query(view) => view.clone().into_any_element(),
            TabKind::Table(view) => view.clone().cached(fill()).into_any_element(),
            TabKind::Settings(view) => view.clone().into_any_element(),
        }
    }

    /// Something a tab's cached views may show changed outside of them (the
    /// app state, preferences): render them again.
    pub fn notify_content(&self, cx: &mut App) {
        match &self.kind {
            TabKind::Query(view) => view.update(cx, |tab, cx| tab.notify_views(cx)),
            TabKind::Table(view) => view.update(cx, |_, cx| cx.notify()),
            TabKind::Home(_) | TabKind::Settings(_) => {}
        }
    }

    pub fn as_query(&self) -> Option<&Entity<QueryTab>> {
        match &self.kind {
            TabKind::Query(tab) => Some(tab),
            _ => None,
        }
    }
}

/// Height of the tab strip.
const TAB_BAR_HEIGHT: f32 = 34.;
/// A title keeps at most this much room; the connection name takes what is
/// left of the tab's own maximum width.
const TITLE_MAX_WIDTH: f32 = 168.;
const TAB_MAX_WIDTH: f32 = 312.;
/// Width of the inline title editor.
const RENAME_WIDTH: f32 = 160.;

impl Workspace {
    /// The tab strip: the tab in front is a raised surface continuous with
    /// the page below, with a 2 px edge in its connection's colour and a
    /// semibold label; the others are quiet text. Every database tab names
    /// its connection after the title (accent dot, name, engine badge).
    /// Drafts carry no mark; a tab bound to a file or saved query shows a dot
    /// while it differs from it.
    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let bindings: Vec<TabBinding> = self.tabs.iter().map(|tab| tab.binding(cx)).collect();
        let accents: Vec<Option<Hsla>> = bindings
            .iter()
            .map(|binding| match binding {
                TabBinding::Live(connection) => Some(connection_accent(connection.accent, cx)),
                _ => None,
            })
            .collect();
        let theme = cx.theme();
        let (border, muted, foreground) = (theme.border, theme.muted_foreground, theme.foreground);
        // The strip is a step off the page; the tab in front is the page
        // itself, so it reads as joined to what it shows
        let surfaces = crate::theme::surfaces(cx);
        let (surface, strip) = (surfaces.canvas, surfaces.raised);
        let hover = theme.muted.opacity(0.5);
        let primary = theme.primary;

        let entries: Vec<(TabId, String, bool)> = self
            .tabs
            .iter()
            .zip(&bindings)
            .map(|(tab, binding)| {
                (
                    tab.id,
                    binding.describe(&tab.title(cx)),
                    tab.is_modified(cx),
                )
            })
            .collect();

        let tabs = self
            .tabs
            .iter()
            .zip(accents)
            .zip(bindings)
            .map(|((tab, accent), binding)| {
                let id = tab.id;
                let active = id == self.active;
                let title = tab.title(cx);
                let modified = tab.is_modified(cx);
                let tooltip = tab.tooltip(cx);
                let renamable = tab.is_renamable();
                let editing = self
                    .renaming
                    .as_ref()
                    .filter(|rename| rename.tab == id)
                    .map(|rename| rename.input.clone());
                let group = SharedString::from(format!("workspace-tab-{}", id.as_u64()));
                let leading = if tab.is_running(cx) {
                    Spinner::new().xsmall().into_any_element()
                } else {
                    Icon::new(tab.icon())
                        .xsmall()
                        .text_color(if active { foreground } else { muted })
                        .into_any_element()
                };
                let segment = match editing {
                    Some(_) => None,
                    None => connection_segment(&binding, muted, cx),
                };
                let has_segment = segment.is_some();

                let label = match editing {
                    Some(input) => rename_field(id, input, cx).into_any_element(),
                    None => div()
                        .when(has_segment, |this| {
                            this.flex_none().max_w(px(TITLE_MAX_WIDTH))
                        })
                        .when(!has_segment, |this| this.flex_1().min_w_0())
                        .truncate()
                        .whitespace_nowrap()
                        .child(title)
                        .into_any_element(),
                };

                let workspace = cx.entity().downgrade();
                let tab_el = h_flex()
                    .id(("workspace-tab", id.as_u64()))
                    .group(group.clone())
                    .relative()
                    .flex_none()
                    .h_full()
                    .min_w(px(112.))
                    .max_w(px(TAB_MAX_WIDTH))
                    .pl_2p5()
                    .pr_1()
                    .gap_1p5()
                    .items_center()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(border)
                    .text_size(px(13.))
                    .when(active, |this| {
                        this.bg(surface)
                            .text_color(foreground)
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!active, |this| {
                        this.text_color(muted)
                            .hover(move |style| style.bg(hover).text_color(foreground))
                    })
                    // A double click on a query tab edits its title in place
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if renamable && event.click_count() >= 2 {
                            this.start_rename(id, window, cx);
                        } else {
                            this.activate(id, window, cx);
                        }
                    }))
                    // Middle click closes, like in a browser
                    .when(tab.is_closable(), |this| {
                        this.on_mouse_up(
                            MouseButton::Middle,
                            cx.listener(move |this, _, window, cx| this.close_tab(id, window, cx)),
                        )
                    })
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx));

                // The children go on after the context menu wrapper, which only
                // takes layout and children
                let closable = tab.is_closable();
                let has_others = self
                    .tabs
                    .iter()
                    .any(|other| other.id != id && other.is_closable());
                let has_right = self
                    .tabs
                    .iter()
                    .skip_while(|other| other.id != id)
                    .skip(1)
                    .any(WorkspaceTab::is_closable);
                let edge = active.then(|| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(accent.unwrap_or(primary))
                });
                let close =
                    closable.then(|| close_slot(id, modified, active, &group, foreground, cx));

                if closable {
                    tab_el
                        .context_menu(move |menu, _, _| {
                            tab_menu(
                                menu,
                                id,
                                renamable,
                                has_others,
                                has_right,
                                workspace.clone(),
                            )
                        })
                        // The 2 px edge: the connection's colour (the accent
                        // elsewhere), only on the tab in front
                        .children(edge)
                        .child(leading)
                        .child(label)
                        .children(segment)
                        .children(close)
                        .into_any_element()
                } else {
                    tab_el
                        .children(edge)
                        .child(leading)
                        .child(label)
                        .children(segment)
                        .pr_2()
                        .into_any_element()
                }
            });

        let list = {
            let workspace = cx.entity().downgrade();
            let active = self.active;
            Button::new("tab-list")
                .ghost()
                .xsmall()
                .icon(IconName::ChevronDown)
                .tooltip("All tabs")
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.min_w(px(280.)).max_h(px(360.)).scrollable(true);
                    for (id, description, modified) in entries.clone() {
                        let workspace = workspace.clone();
                        menu = menu.item(
                            PopupMenuItem::new(if modified {
                                format!("{description} ●")
                            } else {
                                description
                            })
                            .checked(id == active)
                            .on_click(move |_, window, cx| {
                                workspace
                                    .update(cx, |this, cx| this.activate(id, window, cx))
                                    .ok();
                            }),
                        );
                    }
                    menu
                })
        };

        div()
            .id("workspace-tabs")
            .relative()
            .flex_none()
            .w_full()
            .h(px(TAB_BAR_HEIGHT))
            .bg(strip)
            // The line under the strip; the tab in front paints over it
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(border),
            )
            .child(
                h_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .id("workspace-tabs-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.tab_scroll)
                            .children(tabs),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .h_full()
                            .px_1()
                            .gap_0p5()
                            .items_center()
                            .child(list)
                            .child(
                                Button::new("new-query")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .tooltip_with_action("New query", &NewQuery, None)
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(Box::new(NewQuery), cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    // ----- Switching -----

    /// ⌘1…⌘9: the nth tab; 9 is always the last one (like browsers).
    pub(super) fn go_to_tab_number(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = if number >= 9 {
            self.tabs.len().checked_sub(1)
        } else {
            number.checked_sub(1)
        };
        if let Some(id) = index.and_then(|ix| self.tabs.get(ix)).map(|tab| tab.id) {
            self.activate(id, window, cx);
        }
    }

    /// What "Go to tab…" lists, in tab bar order.
    pub(super) fn tab_choices(&self, cx: &App) -> Vec<TabChoice> {
        self.tabs
            .iter()
            .map(|tab| TabChoice {
                id: tab.id,
                title: tab.title(cx),
                icon: tab.icon(),
                binding: tab.binding(cx),
            })
            .collect()
    }

    // ----- Closing in bulk -----

    pub(super) fn close_other_tabs(
        &mut self,
        keep: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doomed: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|tab| tab.id != keep && tab.is_closable())
            .map(|tab| tab.id)
            .collect();
        self.close_all(doomed, keep, window, cx);
    }

    pub(super) fn close_tabs_to_the_right(
        &mut self,
        of: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let doomed: Vec<TabId> = self
            .tabs
            .iter()
            .skip_while(|tab| tab.id != of)
            .skip(1)
            .filter(|tab| tab.is_closable())
            .map(|tab| tab.id)
            .collect();
        self.close_all(doomed, of, window, cx);
    }

    /// Close `doomed`, leaving `stay` in front.
    fn close_all(
        &mut self,
        doomed: Vec<TabId>,
        stay: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for id in doomed {
            self.close_tab(id, window, cx);
        }
        self.activate(stay, window, cx);
    }

    // ----- Renaming -----

    /// Edit the title of a query tab in place: Enter or leaving the field
    /// keeps it, Esc cancels, an empty title restores the previous one.
    pub(super) fn start_rename(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let Some(query) = tab.as_query().cloned() else {
            return;
        };
        if self
            .renaming
            .as_ref()
            .is_some_and(|rename| rename.tab == id)
        {
            return;
        }
        // Another tab was being renamed: keep what was typed
        self.commit_rename(false, window, cx);
        let title = query.read(cx).title();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Tab name")
                .default_value(title)
        });
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => this.commit_rename(true, window, cx),
                        // Clicking elsewhere keeps the new title; focus is already
                        // wherever the user put it
                        InputEvent::Blur => this.commit_rename(false, window, cx),
                        _ => {}
                    }
                }),
            ];
        self.renaming = Some(TabRename {
            tab: id,
            input: input.clone(),
            _subscriptions: subscriptions,
        });
        // Through the normal activation, so the active connection follows the
        // tab; only the focus stays for the title editor
        self.activate_tab(id, false, window, cx);
        window.defer(cx, move |window, cx| {
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        });
        cx.notify();
    }

    /// Apply the typed title (an empty one changes nothing) and close the
    /// editor. `refocus` hands the focus back to the tab's content.
    pub(super) fn commit_rename(
        &mut self,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.renaming.take() else {
            return;
        };
        let typed = rename.input.read(cx).value();
        if let Some(query) = self
            .tabs
            .iter()
            .find(|tab| tab.id == rename.tab)
            .and_then(WorkspaceTab::as_query)
        {
            query.update(cx, |query, cx| {
                query.rename(&typed, cx);
            });
        }
        self.update_window_title(window, cx);
        if refocus {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Esc: forget what was typed.
    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// F2 and "Rename tab": the tab in front, when it is a query tab.
    pub(super) fn on_rename_tab(
        &mut self,
        _: &RenameTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.active;
        self.start_rename(active, window, cx);
    }
}

/// `● shop [SL]`: the connection a database tab is bound to, in muted
/// secondary text after the title. The engine badge (and the dot's colour)
/// tell apart two connections that share a name; a tab without an open
/// connection says so.
fn connection_segment(binding: &TabBinding, muted: Hsla, cx: &App) -> Option<AnyElement> {
    let segment = h_flex()
        .min_w_0()
        .gap_1()
        .items_center()
        .text_size(px(11.))
        .font_weight(FontWeight::NORMAL)
        .text_color(muted);
    match binding {
        TabBinding::None => None,
        TabBinding::Detached => Some(
            segment
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .whitespace_nowrap()
                        .italic()
                        .opacity(0.75)
                        .child("No connection"),
                )
                .into_any_element(),
        ),
        TabBinding::Live(connection) => Some(
            segment
                .child(
                    div()
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .bg(connection_accent(connection.accent, cx)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .whitespace_nowrap()
                        .child(connection.name.clone()),
                )
                .child(db_badge_mini(&connection.db_type, cx))
                .into_any_element(),
        ),
    }
}

/// The inline title editor. It sits inside the tab, so pointer events stop
/// here (a click in the field must not activate the tab and take the focus
/// back), and Esc cancels.
fn rename_field(id: TabId, input: Entity<InputState>, cx: &Context<Workspace>) -> impl IntoElement {
    div()
        .id(("rename-tab", id.as_u64()))
        .w(px(RENAME_WIDTH))
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_action(cx.listener(|this, _: &Escape, window, cx| this.cancel_rename(window, cx)))
        .child(Input::new(&input).xsmall())
}

/// The tab's right-click menu.
fn tab_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    id: TabId,
    renamable: bool,
    has_others: bool,
    has_right: bool,
    workspace: WeakEntity<Workspace>,
) -> gpui_kit::component::menu::PopupMenu {
    let item =
        |label: &'static str,
         icon: IconName,
         enabled: bool,
         act: fn(&mut Workspace, TabId, &mut Window, &mut Context<Workspace>)| {
            let workspace = workspace.clone();
            PopupMenuItem::new(label)
                .icon(icon)
                .disabled(!enabled)
                .on_click(move |_, window, cx| {
                    workspace
                        .update(cx, |this, cx| act(this, id, window, cx))
                        .ok();
                })
        };
    menu.item(item(
        "Rename tab",
        IconName::Pencil,
        renamable,
        Workspace::start_rename,
    ))
    .separator()
    .item(item(
        "Close tab",
        IconName::Close,
        true,
        Workspace::close_tab,
    ))
    .item(item(
        "Close other tabs",
        IconName::Close,
        has_others,
        Workspace::close_other_tabs,
    ))
    .item(item(
        "Close tabs to the right",
        IconName::Close,
        has_right,
        Workspace::close_tabs_to_the_right,
    ))
}

/// The tab's trailing slot: a close button, or — for a tab that differs
/// from its file or saved query — a dot that turns into the close button
/// under the pointer.
fn close_slot(
    id: TabId,
    modified: bool,
    active: bool,
    group: &SharedString,
    dot: Hsla,
    cx: &Context<Workspace>,
) -> impl IntoElement {
    // The tab in front always offers it, unless the dot is showing
    let always = active && !modified;
    div()
        .relative()
        .flex_none()
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .when(modified, |this| {
            this.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .group_hover(group, |style| style.opacity(0.))
                    .child(div().size(px(7.)).rounded_full().bg(dot.opacity(0.75))),
            )
        })
        .child(
            div()
                .when(!always, |this| {
                    this.opacity(0.)
                        .group_hover(group, |style| style.opacity(1.))
                })
                .child(
                    Button::new(("close-tab", id.as_u64()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip_with_action("Close tab", &CloseTab, None)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(id, window, cx);
                        })),
                ),
        )
}
