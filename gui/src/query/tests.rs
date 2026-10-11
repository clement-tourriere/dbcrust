//! End-to-end tests of the query tab: a real window (headless), a real
//! SQLite worker, real key events.
//!
//! Imports are explicit on purpose: a glob of `gpui_kit` would shadow
//! `#[test]` with GPUI's own test macro under `test-support`.

use super::results::Stale;
use super::{QueryStatus, QueryTab, history};
use crate::app_state::{AppState, SchemaState};
use crate::preferences::Preferences;
use dbcrust_gui_core::dbcrust::config::Config;
use dbcrust_gui_core::gui_state::StatementSeparators;
use dbcrust_gui_core::{ConfigStore, ConnectRequest, ConnectionHandle, ConnectionId, RequestId};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, Focusable as _, HeadlessAppContext,
    NoopTextSystem, Point, WindowBounds, px, size,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SEED: &[&str] = &[
    "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT, email TEXT)",
    "CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER, status TEXT, total REAL)",
    "INSERT INTO customers (name, email) VALUES ('Ada', 'ada@example.com'), ('Alan', 'alan@example.com')",
    "INSERT INTO orders (customer_id, status, total) VALUES (1, 'paid', 10.5), (2, 'open', 3.0)",
];

#[cfg(target_os = "macos")]
const RUN_KEYS: &str = "cmd-enter";
#[cfg(not(target_os = "macos"))]
const RUN_KEYS: &str = "ctrl-enter";

const TICK: Duration = Duration::from_millis(15);

/// Let real time pass for the connection worker threads, and the same amount
/// of GPUI time for the timers the UI waits on (the test clock is virtual).
fn tick(cx: &mut HeadlessAppContext) {
    std::thread::sleep(TICK);
    cx.advance_clock(TICK);
    cx.run_until_parked();
}

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
        tick(cx);
    }
}

fn settle(cx: &mut HeadlessAppContext, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        tick(cx);
    }
    cx.run_until_parked();
}

struct Env {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    tab: Entity<QueryTab>,
    /// Holds the SQLite file (and its WAL files); declared last so the app,
    /// and with it the connection, is gone before the directory is removed.
    _dir: tempfile::TempDir,
}

impl Env {
    fn new(initial_sql: &str) -> Self {
        crate::test_support::isolate();
        let dir = tempfile::tempdir().expect("temp dir");
        let database = dir.path().join("shop.db");
        std::fs::File::create(&database).expect("database file");
        let url = format!("sqlite://{}", database.display());

        gpui_kit::block_on(async {
            let store = ConfigStore::from_config(Config::default());
            let (db, _) = ConnectionHandle::connect(ConnectRequest::url(url.clone()), store)
                .await
                .expect("seed connection");
            for statement in SEED {
                db.execute(RequestId::next(), *statement)
                    .await
                    .expect("seed statement");
            }
            db.close();
        });

        // `platform::current_platform` would give real text shaping, but it
        // must be created on the main thread and libtest runs tests on
        // worker threads; completion and key handling do not need glyphs
        let mut cx = HeadlessAppContext::with_asset_source(
            Arc::new(NoopTextSystem::new()),
            Arc::new(crate::assets()),
        );
        cx.allow_parking();
        cx.update(|cx| crate::init(ConfigStore::from_config(Config::default()), cx));
        cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.connect(ConnectRequest::url(url), cx).detach();
            })
        });
        wait_until(&mut cx, "the schema", |cx| {
            cx.update(|cx| {
                AppState::global(cx)
                    .read(cx)
                    .active()
                    .is_some_and(|live| matches!(live.schema, SchemaState::Ready(_)))
            })
        });
        let connection = cx
            .update(|cx| AppState::global(cx).read(cx).active_id())
            .expect("connected");
        let sql = initial_sql.to_string();
        let (window, tab) = cx
            .update(|cx| {
                let mut options = crate::main_window_options(cx);
                options.window_bounds = Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1000.), px(700.)),
                }));
                options.focus = false;
                options.show = false;
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| {
                        QueryTab::new(Some(connection), Some("Test".into()), sql, window, cx)
                    })
                })
            })
            .expect("open window");
        let mut env = Self {
            cx,
            window,
            tab,
            _dir: dir,
        };
        env.focus_editor();
        env
    }

    fn focus_editor(&mut self) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                let handle = tab.focus_handle(cx);
                window.focus(&handle, cx);
                window.render_frame(cx);
            })
            .expect("focus");
        self.cx.run_until_parked();
    }

    fn type_text(&mut self, text: &str) {
        self.cx
            .update_window(self.window, |_, window, cx| window.input(text, cx))
            .expect("type");
        self.cx.run_until_parked();
    }

    fn press(&mut self, keys: &str) {
        self.cx
            .update_window(self.window, |_, window, cx| window.press(keys, cx))
            .expect("press");
        self.cx.run_until_parked();
    }

    fn sql(&mut self) -> String {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).sql(cx))
    }

    fn status(&mut self) -> QueryStatus {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).status())
    }

    fn menu(&mut self) -> (bool, Vec<String>) {
        let tab = self.tab.clone();
        self.cx.update(|cx| {
            let editor = tab.read(cx).editor.read(cx);
            let menu = editor.completion_menu_state();
            (
                menu.open,
                menu.items.iter().map(|item| item.label.clone()).collect(),
            )
        })
    }

    fn set_sql(&mut self, sql: &str) {
        let tab = self.tab.clone();
        let sql = sql.to_string();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| tab.set_sql(sql, window, cx));
                window.render_frame(cx);
            })
            .expect("set sql");
    }

    fn run_all(&mut self) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| tab.run_all(window, cx));
            })
            .expect("run all");
    }

    fn run_current(&mut self) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| tab.run_current(window, cx));
            })
            .expect("run");
    }

    fn wait_for_status(&mut self, what: &str, mut accept: impl FnMut(&QueryStatus) -> bool) {
        let tab = self.tab.clone();
        wait_until(&mut self.cx, what, |cx| {
            cx.update(|cx| accept(&tab.read(cx).status()))
        });
    }

    fn history_len(&mut self) -> usize {
        self.cx
            .update(|cx: &mut App| history::recent(100, cx).len())
    }
}

#[test]
fn typing_completes_tables_and_columns_and_tab_accepts() {
    let mut env = Env::new("");

    env.type_text("SELECT * FROM or");
    let (open, labels) = env.menu();
    assert!(open, "the menu opens while typing a table name");
    assert_eq!(
        labels.first().map(String::as_str),
        Some("orders"),
        "{labels:?}"
    );

    // Tab accepts the highlighted item (it must not indent)
    env.press("tab");
    settle(&mut env.cx, Duration::from_millis(100));
    assert_eq!(env.sql(), "SELECT * FROM orders");
    assert!(!env.menu().0, "accepting closes the menu");

    // Columns of an aliased table, described on demand
    env.type_text(" o WHERE o.cu");
    let tab = env.tab.clone();
    wait_until(&mut env.cx, "column completions", |cx| {
        cx.update(|cx| {
            let editor = tab.read(cx).editor.read(cx);
            editor
                .completion_menu_state()
                .items
                .first()
                .is_some_and(|item| item.label == "customer_id")
        })
    });
    env.press("enter");
    settle(&mut env.cx, Duration::from_millis(100));
    assert_eq!(env.sql(), "SELECT * FROM orders o WHERE o.customer_id");

    // Typing a word that is already complete leaves nothing to accept
    env.type_text(" = 1 AND o.status");
    settle(&mut env.cx, Duration::from_millis(200));
    assert!(!env.menu().0, "{:?}", env.menu().1);
}

#[test]
fn no_completion_inside_strings_and_comments() {
    let mut env = Env::new("");
    env.type_text("SELECT 'or");
    settle(&mut env.cx, Duration::from_millis(100));
    assert!(!env.menu().0);
    env.set_sql("");
    env.type_text("-- or");
    settle(&mut env.cx, Duration::from_millis(100));
    assert!(!env.menu().0, "{:?}", env.menu().1);
}

#[test]
fn the_run_shortcut_runs_the_statement_and_does_not_insert_a_newline() {
    let mut env = Env::new("SELECT id, name FROM customers ORDER BY id");
    env.press(RUN_KEYS);
    env.wait_for_status("the run to finish", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    match env.status() {
        QueryStatus::Done { rows, .. } => assert_eq!(rows, 2),
        other => panic!("unexpected status {other:?}"),
    }
    assert_eq!(
        env.sql(),
        "SELECT id, name FROM customers ORDER BY id",
        "no newline was inserted"
    );
    assert_eq!(env.history_len(), 1);
}

#[test]
fn run_all_stops_at_the_first_error() {
    let mut env = Env::new("");
    env.set_sql("SELECT 1; SELECT * FROM missing_table; SELECT 3");
    env.run_all();
    env.wait_for_status("the script to fail", |status| {
        matches!(status, QueryStatus::Failed)
    });
    // the first statement succeeded, the second failed, the third never ran
    assert_eq!(env.history_len(), 2);
}

#[test]
fn run_all_runs_every_statement() {
    let mut env = Env::new("");
    env.set_sql("SELECT 1 AS a; SELECT 2 AS b; SELECT name FROM customers");
    env.run_all();
    env.wait_for_status("the script to finish", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    match env.status() {
        QueryStatus::Done { rows, .. } => assert_eq!(rows, 2, "rows of the last statement"),
        other => panic!("unexpected status {other:?}"),
    }
    assert_eq!(env.history_len(), 3);
}

#[test]
fn cancel_stops_a_long_statement() {
    let mut env = Env::new("");
    env.set_sql("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 60000000) SELECT count(*) FROM c");
    env.run_current();
    env.wait_for_status("the run to start", |status| {
        matches!(status, QueryStatus::Running { .. })
    });
    settle(&mut env.cx, Duration::from_millis(150));
    let tab = env.tab.clone();
    env.cx.update(|cx| tab.update(cx, |tab, cx| tab.cancel(cx)));
    env.wait_for_status("the cancellation", |status| {
        matches!(status, QueryStatus::Cancelled)
    });
}

#[test]
fn a_superseded_tab_keeps_its_status_when_the_connection_changes_to_none() {
    let mut env = Env::new("SELECT 1");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    let tab = env.tab.clone();
    env.cx
        .update_window(env.window, |_, window, cx| {
            tab.update(cx, |tab, cx| tab.set_connection(None, window, cx));
        })
        .expect("unbind");
    assert!(matches!(env.status(), QueryStatus::Done { .. }));
    // unbound: Run explains instead of running
    env.run_current();
    assert!(matches!(env.status(), QueryStatus::Done { .. }));
}

impl Env {
    /// What the results panel shows: see `ResultsView::shape`.
    fn shape(&mut self) -> (usize, bool, bool) {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).results.read(cx).shape())
    }

    /// Open a second, empty SQLite connection.
    fn connect_second(&mut self) -> ConnectionId {
        let path = self._dir.path().join("other.db");
        std::fs::File::create(&path).expect("second database file");
        let url = format!("sqlite://{}", path.display());
        self.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.connect(ConnectRequest::url(url), cx).detach();
            })
        });
        wait_until(&mut self.cx, "the second connection", |cx| {
            cx.update(|cx| AppState::global(cx).read(cx).connections().len() == 2)
        });
        self.cx.update(|cx| {
            AppState::global(cx)
                .read(cx)
                .connections()
                .last()
                .map(|live| live.id())
                .expect("second connection")
        })
    }

    fn handle(&mut self, id: ConnectionId) -> ConnectionHandle {
        self.cx.update(|cx| {
            AppState::global(cx)
                .read(cx)
                .handle(id)
                .expect("live connection")
        })
    }

    fn stale(&mut self) -> Option<Stale> {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).stale)
    }

    /// What the results panel itself says about its results.
    fn results_stale(&mut self) -> Option<Stale> {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).results.read(cx).stale())
    }

    fn durations(&mut self) -> Vec<Duration> {
        let tab = self.tab.clone();
        self.cx
            .update(|cx| tab.read(cx).results.read(cx).durations())
    }

    fn modified(&mut self) -> bool {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).is_modified())
    }

    fn target(&mut self) -> Option<String> {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).target_description())
    }

    fn title(&mut self) -> String {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).title().to_string())
    }

    /// Open `text` as if it had been read from `path`.
    fn load_file(&mut self, path: std::path::PathBuf, text: &str) {
        let tab = self.tab.clone();
        let text = text.to_string();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| tab.loaded_file(path, text, window, cx));
            })
            .expect("load file");
    }

    /// Select `range` of the buffer (byte offsets), as the user would.
    fn select(&mut self, range: std::ops::Range<usize>) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| {
                    tab.editor
                        .update(cx, |editor, cx| editor.set_selected_range(range, cx));
                });
                window.render_frame(cx);
            })
            .expect("select");
    }

    fn save(&mut self) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| tab.save_query(window, cx));
            })
            .expect("save");
    }

    fn switch_to(&mut self, connection: ConnectionId) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| {
                    tab.set_connection(Some(connection), window, cx)
                });
            })
            .expect("switch connection");
    }
}

const SLOW: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 60000000) SELECT count(*) FROM c";

#[test]
fn switching_connection_mid_run_stops_the_run_on_the_old_one() {
    let mut env = Env::new("");
    let first = env
        .cx
        .update(|cx| AppState::global(cx).read(cx).active_id())
        .expect("connected");
    let second = env.connect_second();
    // A slow statement, then one that must never run
    env.set_sql(&format!("{SLOW}; CREATE TABLE marker (x INTEGER)"));
    env.run_all();
    env.wait_for_status("the run to start", |status| {
        matches!(status, QueryStatus::Running { .. })
    });
    settle(&mut env.cx, Duration::from_millis(150));

    env.switch_to(second);
    // The run is over for this tab at once, and nothing of it shows up here
    assert_eq!(env.status(), QueryStatus::Idle);
    assert_eq!(env.shape(), (0, false, false));
    let tab = env.tab.clone();
    env.cx.update(|cx| {
        assert_eq!(tab.read(cx).connection(), Some(second));
        assert!(!tab.read(cx).is_running());
    });

    // The old connection stops working on the slow statement and never
    // starts the next one
    let old = env.handle(first);
    wait_until(&mut env.cx, "the old connection to go idle", |_| {
        old.pending_operations() == 0
    });
    settle(&mut env.cx, Duration::from_millis(200));
    assert_eq!(
        env.status(),
        QueryStatus::Idle,
        "a late reply must not land here"
    );
    assert_eq!(env.shape(), (0, false, false));
    let marker = gpui_kit::block_on(old.execute(
        RequestId::next(),
        "SELECT name FROM sqlite_master WHERE name = 'marker'",
    ))
    .expect("query the old connection");
    assert_eq!(
        marker.row_count(),
        0,
        "the rest of Run all kept running on the old connection"
    );
}

#[test]
fn results_do_not_carry_over_to_another_connection_through_a_disconnect() {
    let mut env = Env::new("");
    let second = env.connect_second();
    let first = env
        .cx
        .update(|cx| AppState::global(cx).read(cx).connections()[0].id());
    env.switch_to(first);
    env.set_sql("SELECT name FROM customers");
    env.run_current();
    env.wait_for_status("the query to finish", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.shape().0, 1, "results are shown");

    // A → unbound (its connection closed) → B
    let tab = env.tab.clone();
    env.cx
        .update_window(env.window, |_, window, cx| {
            tab.update(cx, |tab, cx| tab.set_connection(None, window, cx));
        })
        .expect("unbind");
    env.switch_to(second);
    assert_eq!(
        env.shape(),
        (0, false, false),
        "A's results must not show under B"
    );
}

#[test]
fn a_failed_run_all_keeps_the_statements_that_completed() {
    let mut env = Env::new("");
    env.set_sql("SELECT 1 AS a; SELECT 2 AS b; SELECT * FROM missing_table; SELECT 4");
    env.run_all();
    env.wait_for_status("the script to fail", |status| {
        matches!(status, QueryStatus::Failed)
    });
    // Two result tabs and the error banner, not a bare error
    assert_eq!(env.shape(), (2, true, false));
}

#[test]
fn a_failed_rerun_keeps_the_previous_results() {
    let mut env = Env::new("");
    // Nothing to keep: the plain error
    env.set_sql("SELEC nonsense");
    env.run_current();
    env.wait_for_status("the first failure", |status| {
        matches!(status, QueryStatus::Failed)
    });
    assert_eq!(env.shape(), (0, false, true));

    env.set_sql("SELECT name FROM customers");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.shape(), (1, false, false));

    // A syntax error leaves the grid in place, under a banner
    env.set_sql("SELEC nonsense");
    env.run_current();
    env.wait_for_status("the failure", |status| {
        matches!(status, QueryStatus::Failed)
    });
    assert_eq!(env.shape(), (1, true, false));

    // And again: still the same results
    env.run_current();
    env.wait_for_status("the second failure", |status| {
        matches!(status, QueryStatus::Failed)
    });
    assert_eq!(env.shape(), (1, true, false));

    // A good run replaces them, banner gone
    env.set_sql("SELECT 1");
    env.run_current();
    env.wait_for_status("the good run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.shape(), (1, false, false));
}

#[test]
fn a_single_statement_has_one_duration_and_a_batch_adds_its_total() {
    let mut env = Env::new("SELECT name FROM customers");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    // The status and the results footer read the same number
    let shown = env.durations();
    assert_eq!(shown.len(), 1);
    match env.status() {
        QueryStatus::Done { elapsed, .. } => assert_eq!(elapsed, shown[0]),
        other => panic!("unexpected status {other:?}"),
    }

    env.set_sql("SELECT 1 AS a; SELECT 2 AS b; SELECT name FROM customers");
    env.run_all();
    env.wait_for_status("the batch", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    let each = env.durations();
    assert_eq!(each.len(), 3);
    match env.status() {
        // The total spans the whole run, gaps included
        QueryStatus::Done { elapsed, .. } => assert!(elapsed >= each.iter().sum::<Duration>()),
        other => panic!("unexpected status {other:?}"),
    }
}

#[test]
fn editing_a_statement_that_ran_marks_its_results_as_previous() {
    let mut env = Env::new("SELECT name FROM customers");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.stale(), None);
    assert_eq!(env.results_stale(), None);

    env.set_sql("SELECT email FROM customers");
    assert_eq!(env.stale(), Some(Stale::Edited));
    assert_eq!(env.results_stale(), Some(Stale::Edited));

    // Back to the text that ran: the results match again
    env.set_sql("SELECT name FROM customers");
    assert_eq!(env.stale(), None);
    assert_eq!(env.results_stale(), None);

    // A new run starts fresh
    env.set_sql("SELECT id FROM customers");
    env.run_current();
    env.wait_for_status("the second run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.stale(), None);
    assert_eq!(env.results_stale(), None);
}

#[test]
fn editing_another_statement_leaves_the_results_alone() {
    let mut env = Env::new("SELECT 1 AS a;\nSELECT 2 AS b;");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    env.set_sql("SELECT 1 AS a;\nSELECT 3 AS b;");
    assert_eq!(env.stale(), None);
    // But the one that ran is gone
    env.set_sql("SELECT 5 AS a;\nSELECT 2 AS b;");
    assert_eq!(env.stale(), Some(Stale::Edited));
}

#[test]
fn results_are_previous_after_the_connection_goes_away() {
    let mut env = Env::new("SELECT name FROM customers");
    let connection = env
        .cx
        .update(|cx| AppState::global(cx).read(cx).active_id())
        .expect("connected");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    let tab = env.tab.clone();
    env.cx
        .update_window(env.window, |_, window, cx| {
            tab.update(cx, |tab, cx| tab.set_connection(None, window, cx));
        })
        .expect("unbind");
    assert_eq!(env.stale(), Some(Stale::ConnectionChanged));
    assert_eq!(env.results_stale(), Some(Stale::ConnectionChanged));

    // Bound to the same one again, the results are as good as they were
    env.switch_to(connection);
    assert_eq!(env.stale(), None);
}

#[test]
fn a_draft_is_never_modified_but_a_bound_file_shows_when_it_differs() {
    let mut env = Env::new("SELECT 1");
    env.set_sql("SELECT 2");
    assert!(!env.modified(), "a draft has nothing to differ from");
    assert_eq!(env.target(), None);

    let path = env._dir.path().join("report.sql");
    std::fs::write(&path, "SELECT 1;").expect("write the file");
    env.load_file(path.clone(), "SELECT 1;");
    assert_eq!(env.title(), "report.sql");
    assert_eq!(env.target(), Some(path.display().to_string()));
    assert!(!env.modified());

    env.set_sql("SELECT 2;");
    assert!(env.modified());
    // Typing it back is no longer a change
    env.set_sql("SELECT 1;");
    assert!(!env.modified());
}

#[test]
fn save_writes_to_the_bound_file_and_clears_the_mark() {
    let mut env = Env::new("");
    let path = env._dir.path().join("report.sql");
    std::fs::write(&path, "SELECT 1;").expect("write the file");
    env.load_file(path.clone(), "SELECT 1;");
    env.set_sql("SELECT 2;");
    assert!(env.modified());

    env.save();
    let tab = env.tab.clone();
    wait_until(&mut env.cx, "the file to be written", |cx| {
        std::fs::read_to_string(&path).is_ok_and(|text| text == "SELECT 2;")
            && !cx.update(|cx| tab.read(cx).is_modified())
    });
    assert!(!env.modified());
}

#[test]
fn a_save_as_started_while_a_draft_restores_keeps_its_own_file() {
    let mut env = Env::new("SELECT 2;");
    let restored = env._dir.path().join("restored.sql");
    let saved = env._dir.path().join("saved.sql");
    std::fs::write(&restored, "SELECT 1;").expect("write the file");
    let tab = env.tab.clone();
    // The restore starts reading, then Save As starts writing elsewhere
    // before either finishes: whichever completes first, the newer one wins
    env.cx
        .update_window(env.window, |_, window, cx| {
            tab.update(cx, |tab, cx| {
                let target = dbcrust_gui_core::gui_state::DraftTarget::File {
                    path: restored.clone(),
                };
                tab.restore_target(target, window, cx);
                tab.write_file(saved.clone(), "SELECT 2;".into(), window, cx);
            });
        })
        .expect("window");
    wait_until(&mut env.cx, "the save to bind the tab", |cx| {
        cx.update(|cx| tab.read(cx).target_description())
            .is_some_and(|target| target == saved.display().to_string())
    });
    settle(&mut env.cx, Duration::from_millis(100));
    assert_eq!(env.target(), Some(saved.display().to_string()));
    assert_eq!(env.title(), "saved.sql");
    assert!(!env.modified());
}

#[test]
fn a_saved_query_saves_back_to_itself() {
    let mut env = Env::new("");
    let tab = env.tab.clone();
    env.cx
        .update_window(env.window, |_, window, cx| {
            tab.update(cx, |tab, cx| {
                tab.apply_named_query("Monthly".into(), "SELECT 1".into(), "global", window, cx)
            });
        })
        .expect("open the saved query");
    assert_eq!(env.title(), "Monthly");
    assert!(!env.modified());
    assert!(
        env.target()
            .is_some_and(|target| target.contains("Monthly"))
    );

    env.set_sql("SELECT 2");
    assert!(env.modified());
    env.save();
    let store = env
        .cx
        .update(|cx| AppState::global(cx).read(cx).store().clone());
    let tab = env.tab.clone();
    wait_until(&mut env.cx, "the saved query to be updated", |cx| {
        store
            .named_queries(None)
            .iter()
            .any(|entry| entry.name == "Monthly" && entry.query == "SELECT 2")
            && !cx.update(|cx| tab.read(cx).is_modified())
    });
}

#[test]
fn a_header_click_sorts_the_loaded_rows_and_says_so() {
    let mut env = Env::new("SELECT name, email FROM customers ORDER BY id");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    let tab = env.tab.clone();
    let sort = |env: &mut Env| {
        env.cx
            .update(|cx| tab.read(cx).results.read(cx).local_sort(cx))
    };
    assert_eq!(sort(&mut env), None);

    // Ascending, then descending, then the order the query returned
    for expected in [Some("name asc"), Some("name desc"), None] {
        env.cx
            .update_window(env.window, |_, window, cx| {
                window.render_frame(cx);
                window.click(("result-th", 0usize), cx);
            })
            .expect("click the header");
        env.cx.run_until_parked();
        assert_eq!(sort(&mut env).as_deref(), expected);
    }
}

#[test]
fn a_statement_that_grows_or_gains_a_clause_is_a_different_statement() {
    // `SELECT 1` is a substring of `SELECT 10`; it is not the same statement
    let mut env = Env::new("SELECT 1");
    env.run_current();
    env.wait_for_status("the run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.stale(), None);
    env.set_sql("SELECT 10");
    assert_eq!(env.stale(), Some(Stale::Edited));
    assert_eq!(env.results_stale(), Some(Stale::Edited));
    env.set_sql("SELECT 1");
    assert_eq!(env.stale(), None);

    // Neither is the same query with a clause on the end
    env.set_sql("SELECT name FROM customers");
    env.run_current();
    env.wait_for_status("the second run", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.stale(), None);
    env.set_sql("SELECT name FROM customers WHERE id = 1");
    assert_eq!(env.stale(), Some(Stale::Edited));
    assert_eq!(env.results_stale(), Some(Stale::Edited));
}

#[test]
fn a_batch_is_stale_once_any_statement_of_it_changes() {
    let script = "SELECT 1 AS a;\nSELECT 2 AS b;\nSELECT 3 AS c";
    let mut env = Env::new(script);
    env.run_all();
    env.wait_for_status("the batch", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.stale(), None);
    // Blank lines and new statements around them are not changes
    env.set_sql("\n SELECT 1 AS a;\nSELECT 2 AS b;\n\nSELECT 3 AS c;\nSELECT 4 AS d");
    assert_eq!(env.stale(), None);
    env.set_sql("SELECT 1 AS a;\nSELECT 2 AS b;\nSELECT 33 AS c");
    assert_eq!(env.stale(), Some(Stale::Edited));
}

#[test]
fn a_selection_that_ran_is_only_changed_by_edits_inside_it() {
    let script = "SELECT 1 AS a;\nSELECT name FROM customers;";
    let mut env = Env::new(script);
    let start = script.find("SELECT name").expect("second statement");
    env.select(start..start + "SELECT name FROM customers".len());
    env.run_current();
    env.wait_for_status("the selection", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
    assert_eq!(env.stale(), None);

    // Before it, and after it: the selection is intact
    env.set_sql("SELECT 9 AS a;\nSELECT name FROM customers;");
    assert_eq!(env.stale(), None);
    env.set_sql("SELECT 1 AS a;\nSELECT name FROM customers; -- later");
    assert_eq!(env.stale(), None);
    // Inside it
    env.set_sql("SELECT 1 AS a;\nSELECT email FROM customers;");
    assert_eq!(env.stale(), Some(Stale::Edited));
}

// ----- Statements separated by blank lines -----

const TWO_QUERIES: &str = "select name from customers where id = 1\n\nselect count(*) from orders";

impl Env {
    fn caret(&mut self, offset: usize) {
        self.select(offset..offset);
        self.cx.run_until_parked();
    }

    /// The ranges the highlight holds, as the editor tracks them.
    fn highlighted(&mut self) -> Vec<std::ops::Range<usize>> {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).highlight.get_ranges(cx))
    }

    fn highlight_writes(&mut self) -> usize {
        let tab = self.tab.clone();
        self.cx.update(|cx| tab.read(cx).highlight_writes)
    }

    fn error_text(&mut self) -> Option<String> {
        let tab = self.tab.clone();
        self.cx
            .update(|cx| tab.read(cx).results.read(cx).error_text())
    }

    fn run_explain(&mut self) {
        let tab = self.tab.clone();
        self.cx
            .update_window(self.window, |_, window, cx| {
                tab.update(cx, |tab, cx| tab.explain(false, window, cx));
            })
            .expect("explain");
    }

    fn separators(&mut self, separators: StatementSeparators) {
        self.cx
            .update(|cx| Preferences::update(cx, |p| p.statement_separators = separators));
        self.cx
            .update_window(self.window, |_, window, cx| window.render_frame(cx))
            .expect("frame");
        self.cx.run_until_parked();
    }

    /// The SQL of the most recent history entries, newest first.
    fn history_sql(&mut self) -> Vec<String> {
        self.cx.update(|cx: &mut App| {
            history::recent(100, cx)
                .into_iter()
                .map(|entry| entry.sql)
                .collect()
        })
    }
}

impl Env {
    /// Pump until `count` statements are in the history.
    fn wait_for_history(&mut self, count: usize) {
        wait_until(&mut self.cx, "the history", |cx| {
            cx.update(|cx: &mut App| history::recent(100, cx).len() >= count)
        });
    }
}

#[test]
fn the_caret_picks_the_query_when_blank_lines_separate_them() {
    let mut env = Env::new("");
    env.set_sql(TWO_QUERIES);
    let second = TWO_QUERIES.find("select count").unwrap();
    env.caret(second + 5);
    env.run_current();
    env.wait_for_history(1);
    assert_eq!(env.history_sql(), vec!["select count(*) from orders"]);

    // The first line runs the first query, not the whole buffer
    env.caret(3);
    env.run_current();
    env.wait_for_history(2);
    assert_eq!(
        env.history_sql()[0],
        "select name from customers where id = 1"
    );
}

#[test]
fn semicolons_only_runs_the_whole_buffer_as_before() {
    let mut env = Env::new("");
    env.separators(StatementSeparators::SemicolonsOnly);
    env.set_sql(TWO_QUERIES);
    env.caret(3);
    env.run_current();
    env.wait_for_history(1);
    assert_eq!(env.history_sql(), vec![TWO_QUERIES.to_string()]);
}

#[test]
fn the_highlight_follows_the_caret_and_hides_during_a_selection() {
    let mut env = Env::new("");
    env.set_sql(TWO_QUERIES);
    let first = 0..TWO_QUERIES.find("\n\n").unwrap();
    let second_start = TWO_QUERIES.find("select count").unwrap();
    let second = second_start..TWO_QUERIES.len();

    env.caret(3);
    assert_eq!(env.highlighted(), vec![first.clone()]);
    env.caret(second_start + 4);
    assert_eq!(env.highlighted(), vec![second.clone()]);

    // Moving inside the statement writes nothing
    let writes = env.highlight_writes();
    env.caret(second_start + 8);
    env.caret(second_start + 2);
    assert_eq!(env.highlight_writes(), writes);
    assert_eq!(env.highlighted(), vec![second.clone()]);

    // A selection hides it, and the caret brings it back
    env.select(second_start..second_start + 6);
    env.cx.run_until_parked();
    assert_eq!(env.highlighted(), Vec::<std::ops::Range<usize>>::new());
    env.caret(1);
    assert_eq!(env.highlighted(), vec![first]);
}

#[test]
fn the_highlight_tracks_edits_and_the_merged_region_of_an_unfinished_query() {
    let mut env = Env::new("");
    env.set_sql("select 1\n\nselect 2");
    env.caret(2);
    assert_eq!(env.highlighted(), vec![0..8]);

    // One statement: nothing to tell apart
    env.set_sql("select 1");
    env.caret(2);
    assert_eq!(env.highlighted(), Vec::<std::ops::Range<usize>>::new());

    // Typing the second query: while it does not parse the region is one
    // statement, and the highlight covers all of it
    env.set_sql("select 1\n\nselect * from");
    env.caret(2);
    assert_eq!(env.highlighted(), vec![0..23]);
    env.caret(23);
    env.cx
        .update_window(env.window, |_, window, cx| window.input(" orders", cx))
        .expect("type");
    env.cx.run_until_parked();
    assert_eq!(env.sql(), "select 1\n\nselect * from orders");
    env.caret(2);
    assert_eq!(env.highlighted(), vec![0..8]);

    // Turning the smart split off puts the whole block back
    env.separators(StatementSeparators::SemicolonsOnly);
    assert_eq!(env.highlighted(), vec![0..env.sql().len()]);
}

#[test]
fn a_selection_of_several_statements_runs_them_as_a_batch() {
    let mut env = Env::new("");
    env.set_sql("select 1 as a\n\nselect 2 as b\n\nselect 3 as c");
    let len = env.sql().len();
    env.select(0..len);
    env.run_current();
    env.wait_for_history(3);
    assert_eq!(
        env.history_sql(),
        vec!["select 3 as c", "select 2 as b", "select 1 as a"]
    );
    assert_eq!(env.shape().0, 3, "one result tab per statement");

    // Part of a statement is sent exactly as selected
    env.select(0..8);
    env.run_current();
    env.wait_for_history(4);
    assert_eq!(env.history_sql()[0], "select 1");
}

#[test]
fn explain_refuses_a_selection_of_several_statements() {
    let mut env = Env::new("");
    env.set_sql("select 1\n\nselect 2");
    let len = env.sql().len();
    env.select(0..len);
    env.run_explain();
    settle(&mut env.cx, Duration::from_millis(100));
    assert_eq!(env.status(), QueryStatus::Idle);
    assert_eq!(env.history_len(), 0);

    // One statement explains
    env.caret(2);
    env.run_explain();
    env.wait_for_status("the plan", |status| {
        matches!(status, QueryStatus::Done { .. })
    });
}

#[test]
fn a_syntax_error_in_a_block_with_a_blank_line_suggests_a_semicolon() {
    let mut env = Env::new("");
    env.separators(StatementSeparators::SemicolonsOnly);
    env.set_sql("select name from customers\n\nselct 1");
    env.run_current();
    env.wait_for_status("the failure", |status| {
        matches!(status, QueryStatus::Failed)
    });
    let message = env.error_text().expect("an error is shown");
    assert!(
        message.contains("separate them with `;`, or select the one to run."),
        "{message}"
    );
    assert!(message.contains("syntax error"), "{message}");

    // The same typo without a blank line gets no hint
    env.set_sql("selct 1");
    env.run_current();
    env.wait_for_status("the second failure", |status| {
        matches!(status, QueryStatus::Failed)
    });
    let message = env.error_text().expect("an error is shown");
    assert!(!message.contains("separate them"), "{message}");
}

#[test]
fn editor_repaints_do_not_rework_the_highlight() {
    let mut env = Env::new("");
    env.set_sql(TWO_QUERIES);
    env.caret(3);
    let tab = env.tab.clone();
    let scans = env.cx.update(|cx| tab.read(cx).highlight_scans);
    // Caret blinks and repaints notify the editor without changing anything
    for _ in 0..5 {
        let editor = env.cx.update(|cx| tab.read(cx).editor.clone());
        env.cx.update(|cx| editor.update(cx, |_, cx| cx.notify()));
        env.cx.run_until_parked();
    }
    assert_eq!(env.cx.update(|cx| tab.read(cx).highlight_scans), scans);
    // A move does
    env.caret(TWO_QUERIES.len() - 2);
    assert!(env.cx.update(|cx| tab.read(cx).highlight_scans) > scans);
}

#[test]
fn changing_the_separators_refreshes_whether_the_results_are_current() {
    let mut env = Env::new("");
    env.set_sql("SELECT 1 AS a\n\nSELECT 2 AS b");
    env.caret(2);
    env.run_current();
    env.wait_for_history(1);
    settle(&mut env.cx, Duration::from_millis(50));
    assert_eq!(env.stale(), None, "the first query ran on its own");

    // Now one statement covers both: what ran is not a statement any more
    env.separators(StatementSeparators::SemicolonsOnly);
    assert_eq!(env.stale(), Some(Stale::Edited));
    assert_eq!(env.results_stale(), Some(Stale::Edited));

    // …and back
    env.separators(StatementSeparators::SemicolonsAndBlankLines);
    assert_eq!(env.stale(), None);
}

#[test]
fn a_selection_that_grows_a_clause_below_a_blank_line_is_stale() {
    let mut env = Env::new("SELECT name FROM customers");
    let len = env.sql().len();
    env.select(0..len);
    env.run_current();
    env.wait_for_history(1);
    settle(&mut env.cx, Duration::from_millis(50));
    assert_eq!(env.stale(), None);
    env.caret(len);
    env.type_text("\n\nWHERE id = 1");
    assert_eq!(env.stale(), Some(Stale::Edited));
}
