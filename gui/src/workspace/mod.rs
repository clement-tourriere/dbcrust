//! Root view: title bar, sidebar (explorer), tabs, status bar.
//!
//! The workspace owns the tabs and reacts to [`AppEvent`]s; views never
//! open each other (see `ARCHITECTURE.md`).
//!
//! - `tabs.rs`: tab model and tab bar
//! - `title_bar.rs`, `status_bar.rs`: window chrome
//! - `lifecycle.rs`: draft/layout persistence and quitting
//! - `dialogs.rs`: dialogs opened by commands

mod autosave;
mod dialogs;
mod lifecycle;
mod status_bar;
pub(crate) mod tabs;
mod title_bar;

#[cfg(test)]
mod tab_label_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tray_live_tests;
#[cfg(test)]
mod tray_tests;

use crate::actions as act;
use crate::app_state::{AppEvent, AppState};
use crate::explorer::Explorer;
use crate::home::HomeView;
use crate::inspector::TableInspector;
use crate::preferences::Preferences;
use crate::query::{QueryTab, QueryTabEvent};
use crate::settings::SettingsView;
use crate::{about, palette, theme};
use autosave::OrderedWriter;
use dbcrust_gui_core::gui_state::GuiState;
use dbcrust_gui_core::{ConnectionId, TabId, TableEntry};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    notification::Notification,
    resizable::{ResizablePanelEvent, ResizableState, h_resizable, resizable_panel},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use lifecycle::Snapshot;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use tabs::{CLOSED_TABS_KEPT, ClosedTab, TabKind, TabRename, WorkspaceTab};

/// Replaces the previous "closed tab" notice instead of stacking them.
struct ClosedTabNotice;

/// `⇧⌘T` / `Ctrl+Shift+T`, as the notice names it.
fn reopen_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "⇧⌘T"
    } else {
        "Ctrl+Shift+T"
    }
}

/// The range the user can drag the sidebar through.
const SIDEBAR_MIN_WIDTH: Pixels = px(200.);
const SIDEBAR_MAX_WIDTH: Pixels = px(520.);

pub struct Workspace {
    focus_handle: FocusHandle,
    explorer: Entity<Explorer>,
    tabs: Vec<WorkspaceTab>,
    active: TabId,
    /// Tab activation history, most recent last: closing the active tab
    /// returns to the one used before it.
    recent: Vec<TabId>,
    query_counter: usize,
    sidebar_visible: bool,
    sidebar_width: Pixels,
    sidebar_state: Entity<ResizableState>,
    tab_scroll: ScrollHandle,
    /// The query tab whose title is being edited in the tab strip.
    renaming: Option<TabRename>,
    /// Repaints the status bar while a query runs (live elapsed time).
    ticker: Option<Task<()>>,
    /// Running queries the tray menu was last told about.
    tray_running: usize,
    /// Closed query tabs, most recent last (see "Reopen closed tab").
    closed_tabs: Vec<ClosedTab>,
    /// The only writer of `gui_state.toml` for drafts and layout.
    writer: OrderedWriter<Snapshot>,
    /// The state last handed to `writer` (written or not).
    submitted: Option<Snapshot>,
    /// Set once the app is shutting down; lets the window close without
    /// asking again.
    quitting: Rc<Cell<bool>>,
    /// The final save before quitting, while it runs.
    quit_task: Option<Task<()>>,
    _autosave: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app_state = AppState::global(cx);
        let home = cx.new(|cx| HomeView::new(window, cx));
        let home_focus = home.focus_handle(cx);
        let explorer = cx.new(|cx| Explorer::new(window, cx));
        let home_tab = WorkspaceTab::new(TabKind::Home(home), None);
        let active = home_tab.id;
        let focus_handle = cx.focus_handle();
        let sidebar_state = cx.new(|_| ResizableState::default());
        let quitting = Rc::new(Cell::new(false));

        let subscriptions = vec![
            cx.subscribe_in(&app_state, window, Self::on_app_event),
            cx.observe(&app_state, |this, _, cx| this.app_state_changed(cx)),
            cx.observe_global::<Preferences>(|this, cx| this.app_state_changed(cx)),
            cx.subscribe(
                &sidebar_state,
                |this, state, _: &ResizablePanelEvent, cx| {
                    if let Some(width) = state.read(cx).sizes().first().copied() {
                        this.sidebar_width = width;
                    }
                },
            ),
            cx.observe_window_appearance(window, |_, _, cx| {
                theme::on_system_appearance_changed(cx);
            }),
            // Copying in another app means leaving this window first: a
            // background copy started before must not replace it
            crate::clipboard::track_window_activation(window, cx),
            // Dock → Quit and other paths that bypass `request_quit`
            cx.on_app_quit(|this, cx| {
                let saving = this.save_before_exit(cx);
                async move {
                    if let Some(saving) = saving {
                        saving.await;
                    }
                }
            }),
        ];

        window.set_window_title("DBCrust");
        // The page decides where the caret goes (search, or the form on first run)
        window.focus(&home_focus, cx);
        {
            let this = cx.weak_entity();
            let quitting = quitting.clone();
            window.on_window_should_close(cx, move |window, cx| {
                if quitting.get() {
                    return true;
                }
                this.update(cx, |this, cx| this.should_close(window, cx))
                    .unwrap_or(true)
            });
        }

        let mut workspace = Self {
            focus_handle,
            explorer,
            tabs: vec![home_tab],
            active,
            recent: vec![active],
            query_counter: 0,
            sidebar_visible: true,
            sidebar_width: px(GuiState::DEFAULT_SIDEBAR_WIDTH),
            sidebar_state,
            tab_scroll: ScrollHandle::new(),
            renaming: None,
            ticker: None,
            tray_running: 0,
            closed_tabs: Vec::new(),
            writer: lifecycle::new_writer(),
            submitted: None,
            quitting,
            quit_task: None,
            _autosave: Self::start_autosave(window, cx),
            _subscriptions: subscriptions,
        };
        if let Some(restored) = cx.try_global::<crate::RestoredState>().cloned() {
            workspace.restore(&restored.0, window, cx);
        }
        workspace
    }

    // ----- Tab model -----

    fn active_index(&self) -> usize {
        self.tabs
            .iter()
            .position(|tab| tab.id == self.active)
            .unwrap_or(0)
    }

    fn active_tab(&self) -> Option<&WorkspaceTab> {
        self.tabs.iter().find(|tab| tab.id == self.active)
    }

    /// The active tab's query view, when a query tab is in front.
    fn active_query_tab(&self) -> Option<&Entity<QueryTab>> {
        self.active_tab().and_then(WorkspaceTab::as_query)
    }

    fn add_tab(
        &mut self,
        kind: TabKind,
        subscription: Option<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TabId {
        let tab = WorkspaceTab::new(kind, subscription);
        let id = tab.id;
        self.tabs.push(tab);
        self.activate(id, window, cx);
        id
    }

    pub(crate) fn activate(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_tab(id, true, window, cx);
    }

    /// Bring a tab in front: the one way `active` changes, so the active
    /// connection, the window title and the scroll position always follow.
    /// `focus: false` leaves the keyboard focus where it is (the inline title
    /// editor takes it).
    pub(super) fn activate_tab(
        &mut self,
        id: TabId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        self.active = id;
        self.recent.retain(|recent| *recent != id);
        self.recent.push(id);
        // The connection in the title bar and status bar follows the tab in front
        if let Some(connection) = self.active_tab().and_then(|tab| tab.connection(cx)) {
            AppState::global(cx).update(cx, |state, cx| state.set_active(Some(connection), cx));
        }
        self.tab_scroll.scroll_to_item(self.active_index());
        if focus {
            self.focus_active(window, cx);
        }
        self.update_window_title(window, cx);
        cx.notify();
    }

    /// `Top customers — DBCrust`: what the Window menu and Mission Control show.
    fn update_window_title(&self, window: &mut Window, cx: &App) {
        let title = match self.active_tab() {
            Some(tab) => format!("{} — DBCrust", tab.title(cx)),
            None => "DBCrust".to_string(),
        };
        window.set_window_title(&title);
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            window.focus(&tab.focus_handle(cx), cx);
        }
    }

    pub(crate) fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if !self.tabs[ix].is_closable() {
            return;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|rename| rename.tab == id)
        {
            self.renaming = None;
        }
        let closed = self.tabs.remove(ix);
        if let Some(query) = closed.as_query() {
            query.update(cx, |query, cx| query.cancel(cx));
            self.remember_closed_query(&closed, ix, window, cx);
        }
        self.recent.retain(|recent| *recent != id);
        if self.active == id {
            let next = self
                .recent
                .last()
                .copied()
                .or_else(|| self.tabs.first().map(|tab| tab.id));
            if let Some(next) = next {
                self.activate(next, window, cx);
            }
        }
        cx.notify();
    }

    /// Keep a closed query tab's draft so "Reopen closed tab" can bring it
    /// back (autosave forgets it from `gui_state.toml` as soon as the tab is
    /// gone). Tabs without text are not worth keeping.
    fn remember_closed_query(
        &mut self,
        tab: &WorkspaceTab,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(query) = tab.as_query() else {
            return;
        };
        let (title, sql, connection) = {
            let query = query.read(cx);
            (query.title().to_string(), query.sql(cx), query.connection())
        };
        if sql.trim().is_empty() {
            return;
        }
        self.closed_tabs.push(ClosedTab {
            title: title.clone(),
            sql,
            index,
            connection,
            connection_key: tab
                .connection_key
                .clone()
                .or_else(|| Self::connection_key(connection, cx)),
        });
        if self.closed_tabs.len() > CLOSED_TABS_KEPT {
            self.closed_tabs.remove(0);
        }
        window.push_notification(
            Notification::info(format!(
                "Closed “{title}” — {} to reopen",
                reopen_shortcut()
            ))
            .id::<ClosedTabNotice>(),
            cx,
        );
    }

    /// The connection a closed tab was bound to, if it is still open: the
    /// same one, or another one opened on the same database since.
    fn live_connection_for(closed: &ClosedTab, cx: &App) -> Option<ConnectionId> {
        let state = AppState::global(cx);
        let state = state.read(cx);
        closed
            .connection
            .filter(|id| state.connection(*id).is_some())
            .or_else(|| {
                let key = closed.connection_key.as_deref()?;
                state
                    .connections()
                    .iter()
                    .find(|live| live.summary.display_url == key)
                    .map(|live| live.id())
            })
    }

    /// Bring back the most recently closed query tab (title, text, place in
    /// the tab bar and connection).
    pub(crate) fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closed) = self.closed_tabs.pop() else {
            window.push_notification(
                Notification::info("No closed tab to reopen").id::<ClosedTabNotice>(),
                cx,
            );
            return;
        };
        let connection = Self::live_connection_for(&closed, cx);
        let (id, _) = self.create_query_tab(
            connection,
            Some(closed.title.clone()),
            closed.sql.clone(),
            window,
            cx,
        );
        if let Some(from) = self.tabs.iter().position(|tab| tab.id == id) {
            let mut tab = self.tabs.remove(from);
            if closed.connection_key.is_some() {
                tab.connection_key = closed.connection_key.clone();
            }
            // Home stays first
            let at = closed.index.clamp(1, self.tabs.len());
            self.tabs.insert(at, tab);
        }
        self.activate(id, window, cx);
    }

    fn next_query_title(&mut self) -> String {
        self.query_counter += 1;
        format!("Query {}", self.query_counter)
    }

    /// The `display_url` of a live connection, the key drafts re-bind by.
    fn connection_key(connection: Option<ConnectionId>, cx: &App) -> Option<String> {
        let connection = connection?;
        AppState::global(cx)
            .read(cx)
            .connection(connection)
            .map(|live| live.summary.display_url.clone())
    }

    /// Add a query tab at the end without activating it.
    fn create_query_tab(
        &mut self,
        connection: Option<ConnectionId>,
        title: Option<String>,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (TabId, Entity<QueryTab>) {
        let title = title.unwrap_or_else(|| self.next_query_title());
        let query = cx.new(|cx| QueryTab::new(connection, Some(title), sql, window, cx));
        let subscription = cx.subscribe_in(&query, window, Self::on_query_tab_event);
        let mut tab = WorkspaceTab::new(TabKind::Query(query.clone()), Some(subscription));
        tab.connection_key = Self::connection_key(connection, cx);
        let id = tab.id;
        self.tabs.push(tab);
        cx.notify();
        (id, query)
    }

    pub(crate) fn open_query_tab(
        &mut self,
        connection: Option<ConnectionId>,
        title: Option<String>,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<QueryTab> {
        let (id, query) = self.create_query_tab(connection, title, sql, window, cx);
        self.activate(id, window, cx);
        query
    }

    fn on_query_tab_event(
        &mut self,
        query: &Entity<QueryTab>,
        event: &QueryTabEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, QueryTabEvent::TitleChanged) {
            self.update_window_title(window, cx);
        }
        // The tray's status line counts running queries; a status change is
        // also every caret move, which must not rebuild the tray menu
        let tray_stale = match event {
            QueryTabEvent::ConnectionChanged => true,
            QueryTabEvent::StatusChanged => self.running_queries(cx) != self.tray_running,
            _ => false,
        };
        if tray_stale {
            self.tray_running = self.running_queries(cx);
            crate::tray::state_changed(cx);
        }
        if matches!(event, QueryTabEvent::ConnectionChanged)
            && let Some(ix) = self.tab_index_of_query(query)
        {
            // Remember what the draft was bound to, also after a disconnect
            let bound = query.read(cx).connection();
            if let Some(key) = Self::connection_key(bound, cx) {
                self.tabs[ix].connection_key = Some(key);
            }
        }
        cx.notify();
    }

    fn open_table_tab(
        &mut self,
        connection: ConnectionId,
        table: TableEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let qualified = table.qualified_name();
        let existing = self.tabs.iter().find_map(|tab| match &tab.kind {
            TabKind::Table(inspector) => {
                let inspector = inspector.read(cx);
                (inspector.connection() == connection
                    && inspector.table().qualified_name() == qualified)
                    .then_some(tab.id)
            }
            _ => None,
        });
        match existing {
            Some(id) => self.activate(id, window, cx),
            None => {
                let inspector = cx.new(|cx| TableInspector::new(connection, table, window, cx));
                self.add_tab(TabKind::Table(inspector), None, window, cx);
            }
        }
    }

    fn open_settings_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self
            .tabs
            .iter()
            .find(|tab| matches!(tab.kind, TabKind::Settings(_)))
            .map(|tab| tab.id);
        match existing {
            Some(id) => self.activate(id, window, cx),
            None => {
                let settings = cx.new(|cx| SettingsView::new(window, cx));
                self.add_tab(TabKind::Settings(settings), None, window, cx);
            }
        }
    }

    fn open_settings_page(
        &mut self,
        page: crate::settings::SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_tab(window, cx);
        if let Some(settings) = self.settings_view() {
            settings.update(cx, |settings, cx| settings.select_page(page, cx));
        }
    }

    /// The Settings view, used by UI tests and screenshot fixtures.
    pub fn settings_view(&self) -> Option<Entity<SettingsView>> {
        self.tabs.iter().find_map(|tab| match &tab.kind {
            TabKind::Settings(view) => Some(view.clone()),
            _ => None,
        })
    }

    /// The Connections page (the screenshot harness and tests drive it).
    pub fn connections_view(&self) -> Option<Entity<HomeView>> {
        self.home_view().map(|(_, home)| home)
    }

    fn home_view(&self) -> Option<(TabId, Entity<HomeView>)> {
        self.tabs.iter().find_map(|tab| match &tab.kind {
            TabKind::Home(home) => Some((tab.id, home.clone())),
            _ => None,
        })
    }

    fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.home_view() {
            self.activate(id, window, cx);
        }
    }

    // ----- App events -----

    fn on_app_event(
        &mut self,
        _: &Entity<AppState>,
        event: &AppEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AppEvent::OpenQuery {
                connection,
                title,
                sql,
                run,
            } => {
                let query =
                    self.open_query_tab(Some(*connection), title.clone(), sql.clone(), window, cx);
                if *run {
                    query.update(cx, |query, cx| query.run_all(window, cx));
                }
            }
            AppEvent::OpenTable { connection, table } => {
                self.open_table_tab(*connection, table.clone(), window, cx);
            }
            AppEvent::OpenSettings => self.open_settings_tab(window, cx),
            AppEvent::OpenSettingsPage(page) => self.open_settings_page(*page, window, cx),
            AppEvent::OpenSavedQuery { entry, sql } => {
                // An engine query runs on a connection of that engine: the
                // active one if it fits, else the first that does
                let connection = {
                    let state = AppState::global(cx).read(cx);
                    match &entry.scope {
                        dbcrust_gui_core::dbcrust::config::NamedQueryScope::DatabaseType(
                            engine,
                        ) => state
                            .active()
                            .filter(|live| &live.summary.db_type == engine)
                            .or_else(|| {
                                state
                                    .connections()
                                    .iter()
                                    .find(|live| &live.summary.db_type == engine)
                            })
                            .or(state.active())
                            .map(|live| live.id()),
                        _ => state.active().map(|live| live.id()),
                    }
                };
                let query = self.open_query_tab(
                    connection,
                    Some(entry.name.clone()),
                    sql.clone(),
                    window,
                    cx,
                );
                query.update(cx, |query, cx| query.bind_saved_query(entry.clone(), cx));
            }
            AppEvent::OpenHome => self.show_home(window, cx),
            AppEvent::Connected(id) => self.on_connected(*id, window, cx),
            AppEvent::ConnectFailed(error) => {
                window.push_notification(
                    Notification::error(error.to_string()).title("Couldn’t connect"),
                    cx,
                );
            }
            AppEvent::Disconnected(id) => self.on_disconnected(*id, window, cx),
            AppEvent::OpenWorkspace(id) => self.open_workspace(*id, window, cx),
            AppEvent::ConnectStarted
            | AppEvent::AttemptChanged(_)
            | AppEvent::ActiveConnectionChanged(_)
            | AppEvent::SchemaChanged(_)
            | AppEvent::StorageChanged => self.app_state_changed(cx),
        }
    }

    /// The tab in front holds cached views (a table inspector is one, a
    /// query tab embeds its editor and results as ones; see
    /// [`WorkspaceTab::content`]): what they read from the app state or the
    /// preferences must reach them. Tabs in the back render from scratch when
    /// they come forward.
    fn app_state_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            tab.notify_content(cx);
        }
        cx.notify();
    }

    /// Bring a connection that is already open forward: the query tab of it
    /// used last, or a fresh one.
    fn open_workspace(&mut self, id: ConnectionId, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self
            .recent
            .iter()
            .rev()
            .copied()
            .find(|tab_id| {
                self.tabs.iter().any(|tab| {
                    tab.id == *tab_id && tab.as_query().is_some() && tab.connection(cx) == Some(id)
                })
            })
            .or_else(|| {
                self.tabs
                    .iter()
                    .find(|tab| tab.as_query().is_some() && tab.connection(cx) == Some(id))
                    .map(|tab| tab.id)
            });
        match existing {
            Some(tab) => self.activate(tab, window, cx),
            None => {
                self.open_query_tab(Some(id), None, String::new(), window, cx);
            }
        }
    }

    /// Drafts of the same database re-bind; otherwise a fresh query tab
    /// opens so there is always somewhere to type.
    fn on_connected(&mut self, id: ConnectionId, window: &mut Window, cx: &mut Context<Self>) {
        let key = Self::connection_key(Some(id), cx);
        let drafts: Vec<Entity<QueryTab>> = self
            .tabs
            .iter()
            .filter(|tab| tab.connection_key.is_some() && tab.connection_key == key)
            .filter_map(WorkspaceTab::as_query)
            .filter(|query| query.read(cx).connection().is_none())
            .cloned()
            .collect();
        for draft in &drafts {
            draft.update(cx, |query, cx| query.set_connection(Some(id), window, cx));
        }

        let has_query = self.tabs.iter().any(|tab| {
            tab.as_query()
                .is_some_and(|query| query.read(cx).connection() == Some(id))
        });
        if has_query {
            let first_draft = drafts
                .first()
                .and_then(|draft| self.tab_index_of_query(draft))
                .map(|ix| self.tabs[ix].id);
            if let Some(tab) = first_draft {
                self.activate(tab, window, cx);
            }
        } else {
            self.open_query_tab(Some(id), None, String::new(), window, cx);
        }
    }

    /// Query tabs keep their draft but lose the connection; inspectors of a
    /// closed connection have nothing left to show and are closed.
    fn on_disconnected(&mut self, id: ConnectionId, window: &mut Window, cx: &mut Context<Self>) {
        let inspectors: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|tab| matches!(tab.kind, TabKind::Table(_)) && tab.connection(cx) == Some(id))
            .map(|tab| tab.id)
            .collect();
        for tab in inspectors {
            self.close_tab(tab, window, cx);
        }
        let bound: Vec<Entity<QueryTab>> = self
            .tabs
            .iter()
            .filter_map(WorkspaceTab::as_query)
            .filter(|query| query.read(cx).connection() == Some(id))
            .cloned()
            .collect();
        for query in bound {
            query.update(cx, |query, cx| query.set_connection(None, window, cx));
        }
        cx.notify();
    }

    // ----- Live status -----

    /// While any query runs, repaint every 100 ms so elapsed times stay live.
    fn sync_ticker(&mut self, cx: &mut Context<Self>) {
        let running = self.tabs.iter().any(|tab| tab.is_running(cx));
        if running && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    let keep_going = this
                        .update(cx, |this, cx| {
                            let running = this.tabs.iter().any(|tab| tab.is_running(cx));
                            if running {
                                cx.notify();
                            } else {
                                this.ticker = None;
                            }
                            running
                        })
                        .unwrap_or(false);
                    if !keep_going {
                        break;
                    }
                }
            }));
        }
    }

    // ----- Actions -----

    fn on_new_query(&mut self, _: &act::NewQuery, window: &mut Window, cx: &mut Context<Self>) {
        let active = AppState::global(cx).read(cx).active_id();
        match active {
            Some(connection) => {
                self.open_query_tab(Some(connection), None, String::new(), window, cx);
            }
            None => {
                self.show_home(window, cx);
                window.push_notification(Notification::info("Connect to a database first"), cx);
            }
        }
    }

    fn on_close_tab(&mut self, _: &act::CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active, window, cx);
    }

    fn on_reopen_closed_tab(
        &mut self,
        _: &act::ReopenClosedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reopen_closed_tab(window, cx);
    }

    fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len() as isize;
        if count < 2 {
            return;
        }
        let next = (self.active_index() as isize + step).rem_euclid(count) as usize;
        let id = self.tabs[next].id;
        self.activate(id, window, cx);
    }

    fn on_next_tab(&mut self, _: &act::NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_tab(1, window, cx);
    }

    fn on_previous_tab(
        &mut self,
        _: &act::PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab(-1, window, cx);
    }

    fn on_go_to_tab(&mut self, number: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.go_to_tab_number(number, window, cx);
    }

    fn on_new_connection(
        &mut self,
        _: &act::NewConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_home(window, cx);
        if let Some((_, home)) = self.home_view() {
            home.update(cx, |home, cx| home.focus_new_connection(window, cx));
        }
    }

    fn on_show_home(&mut self, _: &act::ShowHome, window: &mut Window, cx: &mut Context<Self>) {
        self.show_home(window, cx);
    }

    fn on_open_settings(
        &mut self,
        _: &act::OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_tab(window, cx);
    }

    fn on_command_palette(
        &mut self,
        _: &act::CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette(palette::Scope::Everything, window, cx);
    }

    /// "Go to tab…": the palette, limited to the open tabs.
    fn on_switch_tab(&mut self, _: &act::SwitchTab, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(palette::Scope::Tabs, window, cx);
    }

    fn open_palette(&mut self, scope: palette::Scope, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx.weak_entity();
        let pick: palette::TabPicker = Rc::new(move |id, window, cx| {
            let workspace = workspace.clone();
            // After the palette closed, so its focus restore doesn't win
            window.defer(cx, move |window, cx| {
                workspace
                    .update(cx, |this, cx| this.activate(id, window, cx))
                    .ok();
            });
        });
        palette::open(scope, self.tab_choices(cx), pick, window, cx);
    }

    fn on_about(&mut self, _: &act::About, window: &mut Window, cx: &mut Context<Self>) {
        about::open(window, cx);
    }

    fn on_toggle_sidebar(
        &mut self,
        _: &act::ToggleSidebar,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn on_focus_explorer(
        &mut self,
        _: &act::FocusExplorer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_visible = true;
        window.focus(&self.explorer.focus_handle(cx), cx);
        cx.notify();
    }

    fn on_toggle_theme(&mut self, _: &act::ToggleTheme, _: &mut Window, cx: &mut Context<Self>) {
        theme::toggle(cx);
    }

    fn on_disconnect(&mut self, _: &act::Disconnect, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = AppState::global(cx).read(cx).active_id() {
            // Asks first when a query is still running on it
            self.request_disconnect(vec![id], window, cx);
        }
    }

    fn on_refresh_schema(
        &mut self,
        _: &act::RefreshSchema,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = AppState::global(cx);
        state.update(cx, |state, cx| {
            if let Some(id) = state.active_id() {
                state.refresh_schema(id, cx);
            }
        });
    }

    fn on_toggle_read_only(
        &mut self,
        _: &act::ToggleReadOnly,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = AppState::global(cx);
        state.update(cx, |state, cx| {
            if let Some(live) = state.active() {
                let (id, read_only) = (live.id(), live.summary.read_only);
                state.set_read_only_guard(id, !read_only, cx);
            }
        });
    }

    fn on_save_session(
        &mut self,
        _: &act::SaveSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_save_session_dialog(window, cx);
    }

    // Query actions fall back to the query tab in front when focus is
    // elsewhere (explorer, title bar, menu bar); the focused query tab
    // handles them first otherwise.

    fn on_run_statement(
        &mut self,
        _: &act::RunStatement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(query) = self.active_query_tab().cloned() {
            query.update(cx, |query, cx| query.run_current(window, cx));
        }
    }

    fn on_run_all(&mut self, _: &act::RunAll, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(query) = self.active_query_tab().cloned() {
            query.update(cx, |query, cx| query.run_all(window, cx));
        }
    }

    fn on_explain_statement(
        &mut self,
        _: &act::ExplainStatement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(query) = self.active_query_tab().cloned() {
            query.update(cx, |query, cx| query.explain(false, window, cx));
        }
    }

    fn on_explain_analyze(
        &mut self,
        _: &act::ExplainAnalyze,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(query) = self.active_query_tab().cloned() {
            query.update(cx, |query, cx| query.explain(true, window, cx));
        }
    }

    fn on_cancel_query(&mut self, _: &act::CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(query) = self.active_query_tab().cloned() {
            query.update(cx, |query, cx| query.cancel(cx));
        }
    }

    fn on_format_sql(&mut self, _: &act::FormatSql, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(query) = self.active_query_tab().cloned() {
            query.update(cx, |query, cx| query.format_sql(window, cx));
        }
    }

    // ----- Rendering -----

    /// How many tabs are open (the Connections page counts).
    #[cfg(test)]
    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The Explorer sits beside the tabs, except on the Connections page: its
    /// own list is the navigation there, and two lists of connections side
    /// by side would be one too many.
    pub(crate) fn explorer_visible(&self) -> bool {
        let on_connections = matches!(
            self.active_tab().map(|tab| &tab.kind),
            Some(TabKind::Home(_))
        );
        self.sidebar_visible && !on_connections
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let content = self
            .active_tab()
            .map(WorkspaceTab::content)
            .unwrap_or_else(|| div().into_any_element());
        let main = v_flex()
            .size_full()
            .min_w_0()
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .id("tab-content")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(content),
            );

        if !self.explorer_visible() {
            return main.into_any_element();
        }
        h_resizable("workspace-split")
            .with_state(&self.sidebar_state)
            .child(
                resizable_panel()
                    .size(self.sidebar_width)
                    .size_range(SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH)
                    .flex_none()
                    .child(
                        div()
                            .id("sidebar")
                            .size_full()
                            .bg(cx.theme().sidebar)
                            .text_color(cx.theme().sidebar_foreground)
                            // Cached: it watches the app state itself, and
                            // must not re-render each visible row whenever
                            // the editor or the status bar repaints
                            .child(
                                self.explorer
                                    .clone()
                                    .cached(StyleRefinement::default().size_full()),
                            ),
                    ),
            )
            .child(resizable_panel().child(main))
            .into_any_element()
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_ticker(cx);
        let title_bar = self.render_title_bar(window, cx);
        let body = self.render_body(cx);
        let status_bar = self.render_status_bar(cx);

        // The editor, inputs and selectable text copy without the app's
        // helpers: count those copies before they happen
        crate::clipboard::track_copy_actions(v_flex())
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_new_query))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_reopen_closed_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_previous_tab))
            .on_action(cx.listener(Self::on_switch_tab))
            .on_action(cx.listener(Self::on_rename_tab))
            .on_action(
                cx.listener(|this, _: &act::GoToTab1, window, cx| this.on_go_to_tab(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab2, window, cx| this.on_go_to_tab(2, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab3, window, cx| this.on_go_to_tab(3, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab4, window, cx| this.on_go_to_tab(4, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab5, window, cx| this.on_go_to_tab(5, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab6, window, cx| this.on_go_to_tab(6, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab7, window, cx| this.on_go_to_tab(7, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab8, window, cx| this.on_go_to_tab(8, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &act::GoToTab9, window, cx| this.on_go_to_tab(9, window, cx)),
            )
            .on_action(cx.listener(Self::on_new_connection))
            .on_action(cx.listener(Self::on_show_home))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(
                cx.listener(|this, _: &act::ManageSavedQueries, window, cx| {
                    this.open_settings_page(crate::settings::SettingsPage::SavedQueries, window, cx)
                }),
            )
            .on_action(cx.listener(Self::on_command_palette))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_focus_explorer))
            .on_action(cx.listener(Self::on_toggle_theme))
            .on_action(cx.listener(Self::on_disconnect))
            .on_action(cx.listener(Self::on_refresh_schema))
            .on_action(cx.listener(Self::on_toggle_read_only))
            .on_action(cx.listener(Self::on_save_session))
            .on_action(cx.listener(Self::on_run_statement))
            .on_action(cx.listener(Self::on_run_all))
            .on_action(cx.listener(Self::on_explain_statement))
            .on_action(cx.listener(Self::on_explain_analyze))
            .on_action(cx.listener(Self::on_cancel_query))
            .on_action(cx.listener(Self::on_format_sql))
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
            .child(status_bar)
    }
}
