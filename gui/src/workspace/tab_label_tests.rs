//! What the tab strip says about a tab's connection, against real
//! connections (SQLite workers, a headless window): the name follows
//! rebinding, a disconnect and a reconnect, and two connections that share a
//! name stay distinguishable.
//!
//! Imports are explicit on purpose: a glob of `gpui_kit` would shadow
//! `#[test]` with GPUI's own test macro under `test-support`.

use super::Workspace;
use super::tabs::{TabBinding, TabConnection};
use crate::RestoredState;
use crate::app_state::AppState;
use crate::preferences::Preferences;
use dbcrust_gui_core::dbcrust_app::config::Config;
use dbcrust_gui_core::gui_state::GuiState;
use dbcrust_gui_core::{ConfigStore, ConnectRequest, ConnectionId, TabId};
use gpui_kit::{AnyWindowHandle, Context, Entity, HeadlessAppContext, NoopTextSystem, Window};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(15);

fn wait_until(
    cx: &mut HeadlessAppContext,
    what: &str,
    mut done: impl FnMut(&mut HeadlessAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(TICK);
        cx.advance_clock(TICK);
    }
}

struct Live {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    /// Holds the database files; declared last so the app, and with it the
    /// connections, is gone before the directory is removed.
    dir: tempfile::TempDir,
}

impl Live {
    fn new() -> Self {
        crate::test_support::isolate();
        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(NoopTextSystem::new()),
            Arc::new(crate::assets()),
        );
        cx.allow_parking();
        cx.update(|cx| {
            crate::init(ConfigStore::from_config(Config::default()), cx);
            cx.set_global(Preferences::default());
            cx.set_global(RestoredState(GuiState::default()));
        });
        let (window, workspace) = cx.update(|cx| {
            let mut options = crate::main_window_options(cx);
            options.show = false;
            options.focus = false;
            crate::open_main_window(options, cx).expect("open the main window")
        });
        Self {
            cx,
            window,
            workspace,
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    /// Open `<folder>/shop.db`: two folders give two connections that are
    /// both called "shop".
    fn connect(&mut self, folder: &str) -> ConnectionId {
        let directory = self.dir.path().join(folder);
        std::fs::create_dir_all(&directory).expect("folder");
        let path = directory.join("shop.db");
        if !path.exists() {
            std::fs::File::create(&path).expect("database file");
        }
        let before = self.connection_count();
        let url = format!("sqlite://{}", path.display());
        self.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.connect(ConnectRequest::url(url), cx).detach();
            })
        });
        wait_until(&mut self.cx, "the connection", |cx| {
            cx.update(|cx| AppState::global(cx).read(cx).connections().len() == before + 1)
        });
        self.cx.update(|cx| {
            AppState::global(cx)
                .read(cx)
                .connections()
                .last()
                .map(|live| live.id())
                .expect("the new connection")
        })
    }

    fn connection_count(&mut self) -> usize {
        self.cx
            .update(|cx| AppState::global(cx).read(cx).connections().len())
    }

    /// The "Report" tab's binding and tooltip.
    fn report_tab(&mut self) -> (TabBinding, String) {
        let workspace = self.workspace.clone();
        self.cx.update(|cx| {
            let workspace = workspace.read(cx);
            let tab = workspace
                .tabs
                .iter()
                .find(|tab| tab.title(cx) == "Report")
                .expect("the Report tab");
            (tab.binding(cx), tab.tooltip(cx).to_string())
        })
    }

    fn set_connection(&mut self, connection: Option<ConnectionId>) {
        let workspace = self.workspace.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                let query = workspace
                    .read(cx)
                    .tabs
                    .iter()
                    .find(|tab| tab.title(cx) == "Report")
                    .and_then(|tab| tab.as_query().cloned())
                    .expect("the Report tab");
                query.update(cx, |query, cx| query.set_connection(connection, window, cx));
            })
            .expect("window");
        self.cx.run_until_parked();
    }
}

fn live(binding: &TabBinding) -> &TabConnection {
    match binding {
        TabBinding::Live(connection) => connection,
        other => panic!("expected a live connection, got {other:?}"),
    }
}

#[test]
fn the_tab_names_its_connection_and_follows_changes() {
    let mut app = Live::new();
    let first = app.connect("one");
    let second = app.connect("two");

    // Opened on the first connection
    let workspace = app.workspace.clone();
    app.cx
        .update_window(app.window, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.open_query_tab(
                    Some(first),
                    Some("Report".into()),
                    "select 1".into(),
                    window,
                    cx,
                );
            });
        })
        .expect("window");
    let (binding, tooltip) = app.report_tab();
    let one = live(&binding).clone();
    assert_eq!(one.engine, "SQLite");
    assert!(!one.name.is_empty());
    assert_eq!(
        tooltip.lines().next(),
        Some(format!("Report · {} (SQLite)", one.name).as_str())
    );

    // Rebinding updates the label; both connections are called "shop", so
    // only the accent tells them apart
    app.set_connection(Some(second));
    let (binding, _) = app.report_tab();
    let two = live(&binding).clone();
    assert_eq!(two.name, one.name);
    assert_ne!(two.accent, one.accent);

    // A disconnect leaves the draft with no connection…
    app.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| state.disconnect(second, cx));
    });
    app.cx.run_until_parked();
    let (binding, tooltip) = app.report_tab();
    assert_eq!(binding, TabBinding::Detached);
    assert_eq!(tooltip.lines().next(), Some("Report · no connection"));

    // …and opening the same database again binds it back
    app.connect("two");
    app.cx.run_until_parked();
    let (binding, _) = app.report_tab();
    assert_eq!(live(&binding).name, one.name);
}

impl Live {
    fn tab_of(&mut self, connection: ConnectionId) -> TabId {
        let workspace = self.workspace.clone();
        self.cx.update(|cx| {
            workspace
                .read(cx)
                .tabs
                .iter()
                .find(|tab| tab.as_query().is_some() && tab.connection(cx) == Some(connection))
                .map(|tab| tab.id)
                .expect("a query tab of the connection")
        })
    }

    fn active_connection(&mut self) -> Option<ConnectionId> {
        self.cx
            .update(|cx| AppState::global(cx).read(cx).active_id())
    }

    fn with_workspace(
        &mut self,
        change: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) {
        let workspace = self.workspace.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                workspace.update(cx, |workspace, cx| change(workspace, window, cx));
            })
            .expect("window");
        self.cx.run_until_parked();
    }
}

/// Whatever brings a tab in front must also make its connection the active
/// one, or "New query" (⌘T) opens on the connection of the tab left behind.
#[test]
fn every_way_to_bring_a_tab_forward_follows_its_connection() {
    let mut app = Live::new();
    let a = app.connect("one");
    let b = app.connect("two");
    let (tab_a, tab_b) = (app.tab_of(a), app.tab_of(b));

    // Renaming an inactive tab (context menu → Rename)
    app.with_workspace(|workspace, window, cx| workspace.activate(tab_a, window, cx));
    assert_eq!(app.active_connection(), Some(a));
    app.with_workspace(|workspace, window, cx| workspace.start_rename(tab_b, window, cx));
    assert_eq!(app.active_connection(), Some(b));
    app.with_workspace(|workspace, window, cx| {
        assert_eq!(workspace.active, tab_b);
        workspace.cancel_rename(window, cx);
    });

    // ⌘1…⌘9 (tab 1 is Connections, 2 is the first query tab)
    app.with_workspace(|workspace, window, cx| workspace.activate(tab_a, window, cx));
    app.with_workspace(|workspace, window, cx| workspace.go_to_tab_number(3, window, cx));
    assert_eq!(app.active_connection(), Some(b));
    app.with_workspace(|workspace, window, cx| workspace.go_to_tab_number(2, window, cx));
    assert_eq!(app.active_connection(), Some(a));

    // The palette's "Go to tab…" picks through `activate`; the bulk closers
    // end on the tab that stays
    app.with_workspace(|workspace, window, cx| workspace.close_other_tabs(tab_b, window, cx));
    assert_eq!(app.active_connection(), Some(b));
    app.with_workspace(|workspace, window, cx| {
        workspace.close_tabs_to_the_right(tab_b, window, cx)
    });
    assert_eq!(app.active_connection(), Some(b));
}
