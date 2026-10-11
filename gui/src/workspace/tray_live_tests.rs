//! Tray commands against real connections (SQLite workers, a headless
//! window): "Open" brings the connection's own tab forward, and
//! Disconnect / Disconnect all ask before cancelling a running query.
//!
//! Imports are explicit on purpose: a glob of `gpui_kit` would shadow
//! `#[test]` with GPUI's own test macro under `test-support`.

use super::Workspace;
use crate::RestoredState;
use crate::app_state::AppState;
use crate::preferences::Preferences;
use crate::query::QueryStatus;
use crate::tray::{self, TrayCommand};
use dbcrust_gui_core::dbcrust::config::Config;
use dbcrust_gui_core::gui_state::GuiState;
use dbcrust_gui_core::{ConfigStore, ConnectRequest, ConnectionId};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, NoopTextSystem};
use std::sync::Arc;
use std::time::{Duration, Instant};

const LONG_QUERY: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 60000000) SELECT count(*) FROM c";

const TICK: Duration = Duration::from_millis(15);

/// Pump GPUI tasks while the connection worker threads make progress.
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
        // Real time for the workers, and the same amount of GPUI time for the
        // timers the UI waits on (the test clock is virtual)
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
    /// A main window with `databases` open connections, each with its own
    /// query tab.
    fn new(databases: usize) -> Self {
        crate::test_support::isolate();
        tray::test_reset_window();
        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(NoopTextSystem::new()),
            Arc::new(crate::assets()),
        );
        cx.allow_parking();
        cx.update(|cx| {
            crate::init(ConfigStore::from_config(Config::default()), cx);
            cx.set_global(Preferences::default());
            cx.set_global(RestoredState(GuiState::default()));
            tray::pretend_active(cx);
        });
        let (window, workspace) = cx.update(|cx| {
            let mut options = crate::main_window_options(cx);
            options.show = false;
            options.focus = false;
            crate::open_main_window(options, cx).expect("open the main window")
        });
        let mut live = Self {
            cx,
            window,
            workspace,
            dir: tempfile::tempdir().expect("temp dir"),
        };
        for n in 0..databases {
            live.connect(n);
        }
        live
    }

    fn connect(&mut self, n: usize) {
        let path = self.dir.path().join(format!("db{n}.db"));
        std::fs::File::create(&path).expect("database file");
        let url = format!("sqlite://{}", path.display());
        self.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.connect(ConnectRequest::url(url), cx).detach();
            })
        });
        wait_until(&mut self.cx, "the connection", |cx| {
            cx.update(|cx| AppState::global(cx).read(cx).connections().len() == n + 1)
        });
    }

    fn connections(&mut self) -> Vec<ConnectionId> {
        self.cx.update(|cx| {
            AppState::global(cx)
                .read(cx)
                .connections()
                .iter()
                .map(|live| live.id())
                .collect()
        })
    }

    /// The connection of the tab in front.
    fn front_connection(&mut self) -> Option<ConnectionId> {
        let workspace = self.workspace.clone();
        self.cx.update(|cx| {
            workspace
                .read(cx)
                .active_tab()
                .and_then(|tab| tab.connection(cx))
        })
    }

    fn bring_forward_tab_of(&mut self, connection: ConnectionId) {
        let workspace = self.workspace.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    let tab = workspace
                        .tabs
                        .iter()
                        .find(|tab| {
                            tab.as_query().is_some() && tab.connection(cx) == Some(connection)
                        })
                        .map(|tab| tab.id)
                        .expect("a query tab of the connection");
                    workspace.activate(tab, window, cx);
                });
            })
            .expect("window");
    }

    /// Start a long statement on the connection's query tab and wait until
    /// it is running.
    fn start_long_query(&mut self, connection: ConnectionId) {
        let workspace = self.workspace.clone();
        let tab = self.cx.update(|cx| {
            workspace
                .read(cx)
                .tabs
                .iter()
                .find(|tab| tab.as_query().is_some() && tab.connection(cx) == Some(connection))
                .and_then(|tab| tab.as_query().cloned())
                .expect("a query tab of the connection")
        });
        let query = tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                query.update(cx, |query, cx| {
                    query.set_sql(LONG_QUERY.to_string(), window, cx);
                    query.run_all(window, cx);
                });
            })
            .expect("window");
        wait_until(&mut self.cx, "the query to run", |cx| {
            cx.update(|cx| matches!(tab.read(cx).status(), QueryStatus::Running { .. }))
        });
    }

    fn has_dialog(&mut self) -> bool {
        self.cx
            .update_window(self.window, |_, window, cx| window.has_active_dialog(cx))
            .expect("window")
    }

    /// Click a dialog button (a dialog ignores the pointer until its
    /// entrance animation is over).
    fn click(&mut self, id: &'static str) {
        self.cx.advance_clock(Duration::from_secs(1));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                window.click(id, cx);
            })
            .expect("window");
        self.cx.run_until_parked();
    }

    fn perform(&mut self, command: TrayCommand) {
        self.cx.update(|cx| tray::perform(command, cx));
        self.cx.run_until_parked();
    }

    /// The tray hides the window while the user is elsewhere.
    fn hide_to_tray(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                assert!(tray::hide_main_window(window, cx));
            })
            .expect("window");
        assert!(!tray::test_window_visible());
    }
}

#[test]
fn open_brings_the_connections_own_query_tab_forward() {
    let mut live = Live::new(2);
    let ids = live.connections();
    let (a, b) = (ids[0], ids[1]);

    // The tab of the connection opened last is in front: bring A's forward
    live.bring_forward_tab_of(a);
    assert_eq!(live.front_connection(), Some(a));

    // The tray's "Open" for B: its retained tab, not only the active connection
    live.hide_to_tray();
    live.perform(TrayCommand::OpenConnection(b.as_u64()));
    assert_eq!(live.front_connection(), Some(b));
    assert!(tray::test_window_visible(), "Open shows the window");
    live.cx
        .update(|cx| assert_eq!(AppState::global(cx).read(cx).active_id(), Some(b)));
}

#[test]
fn disconnecting_with_nothing_running_does_not_ask() {
    let mut live = Live::new(2);
    let ids = live.connections();

    live.perform(TrayCommand::Disconnect(ids[0].as_u64()));
    assert_eq!(live.connections(), vec![ids[1]]);
    assert!(!live.has_dialog());

    live.perform(TrayCommand::DisconnectAll);
    assert!(live.connections().is_empty());
    assert!(!live.has_dialog());
}

#[test]
fn disconnect_all_asks_while_a_query_runs_and_shows_the_window() {
    let mut live = Live::new(2);
    let ids = live.connections();
    live.start_long_query(ids[0]);
    live.hide_to_tray();

    live.perform(TrayCommand::DisconnectAll);
    assert!(tray::test_window_visible(), "the question needs a window");
    assert!(live.has_dialog());
    assert_eq!(live.connections(), ids, "nothing was closed yet");

    // "Keep working": the query goes on, both connections stay open
    live.cx
        .update_window(live.window, |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        })
        .expect("window");
    live.cx.run_until_parked();
    assert!(!live.has_dialog());
    assert_eq!(live.connections(), ids);

    // Confirming closes every connection
    live.perform(TrayCommand::DisconnectAll);
    assert!(live.has_dialog());
    live.click("ok");
    assert!(!live.has_dialog());
    assert!(live.connections().is_empty());
}

#[test]
fn disconnecting_an_idle_connection_does_not_ask_about_anothers_query() {
    let mut live = Live::new(2);
    let ids = live.connections();
    live.start_long_query(ids[0]);

    // B runs nothing: closing it is silent, A keeps running
    live.perform(TrayCommand::Disconnect(ids[1].as_u64()));
    assert!(!live.has_dialog());
    assert_eq!(live.connections(), vec![ids[0]]);

    // A runs a query: asked, and only A closes
    live.perform(TrayCommand::Disconnect(ids[0].as_u64()));
    assert!(live.has_dialog());
    assert_eq!(live.connections(), vec![ids[0]]);
    live.click("ok");
    assert!(live.connections().is_empty());
}
