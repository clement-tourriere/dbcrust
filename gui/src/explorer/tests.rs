//! The explorer against a real SQLite worker: which app state changes
//! rebuild its rows (walking every table) and which only repaint them.
//!
//! Imports are explicit on purpose: a glob of `gpui_kit` would shadow
//! `#[test]` with GPUI's own test macro under `test-support`.

use super::Explorer;
use super::rows::RowKind;
use crate::app_state::{AppState, SchemaState};
use dbcrust_gui_core::dbcrust_app::config::Config;
use dbcrust_gui_core::{ConfigStore, ConnectRequest, ConnectionHandle, ConnectionId, RequestId};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, HeadlessAppContext, NoopTextSystem, Point,
    WindowBounds, px, size,
};
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

struct Env {
    cx: HeadlessAppContext,
    _window: AnyWindowHandle,
    explorer: Entity<Explorer>,
    connection: ConnectionId,
    _dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        crate::test_support::isolate();
        let dir = tempfile::tempdir().expect("temp dir");
        let database = dir.path().join("explorer.db");
        std::fs::File::create(&database).expect("database file");
        let url = format!("sqlite://{}", database.display());
        gpui_kit::block_on(async {
            let store = ConfigStore::from_config(Config::default());
            let (db, _) = ConnectionHandle::connect(ConnectRequest::url(url.clone()), store)
                .await
                .expect("seed connection");
            for statement in [
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT)",
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER)",
            ] {
                db.execute(RequestId::next(), statement)
                    .await
                    .expect("seed statement");
            }
            db.close();
        });

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
        let (window, explorer) = cx
            .update(|cx| {
                let mut options = crate::main_window_options(cx);
                options.window_bounds = Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(320.), px(600.)),
                }));
                options.focus = false;
                options.show = false;
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| Explorer::new(window, cx))
                })
            })
            .expect("open window");
        cx.run_until_parked();
        Self {
            cx,
            _window: window,
            explorer,
            connection,
            _dir: dir,
        }
    }

    fn rebuilds(&mut self) -> usize {
        let explorer = self.explorer.clone();
        self.cx.update(|cx| explorer.read(cx).rebuilds)
    }

    fn column_rows(&mut self) -> usize {
        let explorer = self.explorer.clone();
        self.cx.update(|cx| {
            explorer
                .read(cx)
                .rows
                .iter()
                .filter(|row| matches!(row.kind, RowKind::Column { .. }))
                .count()
        })
    }

    /// Describe `table` the way completion and the inspector do.
    fn describe(&mut self, table: &str) {
        let (connection, table) = (self.connection, table.to_string());
        let described = self.cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| state.table_details(connection, table, cx))
        });
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        self.cx
            .update(|cx| {
                cx.spawn(async move |_| {
                    described.await.expect("describe");
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                })
            })
            .detach();
        wait_until(&mut self.cx, "the description", |_| {
            done.load(std::sync::atomic::Ordering::SeqCst)
        });
    }
}

#[test]
fn descriptions_fetched_elsewhere_and_label_changes_only_repaint() {
    let mut env = Env::new();
    let before = env.rebuilds();

    // Completion describing a table the tree does not show
    env.describe("customers");
    // A read-only toggle notifies the app state without an event
    let connection = env.connection;
    env.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.set_read_only_guard(connection, true, cx)
        })
    });
    env.cx.run_until_parked();

    assert_eq!(env.rebuilds(), before, "nothing the rows show changed");
    assert_eq!(env.column_rows(), 0);
}

#[test]
fn an_expanded_table_gets_its_columns_when_they_arrive() {
    let mut env = Env::new();
    let table_id = {
        let explorer = env.explorer.clone();
        env.cx.update(|cx| {
            explorer
                .read(cx)
                .rows
                .iter()
                .find(|row| {
                    matches!(&row.kind, RowKind::Table { table, .. } if table.entry().name == "orders")
                })
                .map(|row| row.id.clone())
                .expect("orders row")
        })
    };
    let explorer = env.explorer.clone();
    env.cx
        .update(|cx| explorer.update(cx, |explorer, cx| explorer.set_open(&table_id, true, cx)));
    wait_until(&mut env.cx, "the columns", |cx| {
        cx.update(|cx| {
            explorer.read(cx).rows.iter().any(
                |row| matches!(&row.kind, RowKind::Column { name, .. } if name == "customer_id"),
            )
        })
    });
    assert_eq!(env.column_rows(), 2);
    // Refreshing the schema drops the descriptions: the rows follow
    let connection = env.connection;
    env.cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| state.refresh_schema(connection, cx))
    });
    wait_until(&mut env.cx, "the reloaded tree", |cx| {
        cx.update(|cx| {
            explorer.read(cx).rows.iter().any(
                |row| matches!(&row.kind, RowKind::Column { name, .. } if name == "customer_id"),
            )
        })
    });
}

#[test]
fn a_failed_table_gets_its_columns_when_another_view_describes_it() {
    let mut env = Env::new();
    let explorer = env.explorer.clone();
    let connection = env.connection;
    let table_id = env.cx.update(|cx| {
        explorer
            .read(cx)
            .rows
            .iter()
            .find(|row| {
                matches!(&row.kind, RowKind::Table { table, .. } if table.entry().name == "orders")
            })
            .map(|row| row.id.clone())
            .expect("orders row")
    });
    // The explorer's own fetch failed: expanding shows the failure
    env.cx.update(|cx| {
        explorer.update(cx, |explorer, cx| {
            explorer
                .errors
                .insert((connection, "orders".into()), "timed out".into());
            explorer.set_open(&table_id, true, cx)
        })
    });
    env.cx.run_until_parked();
    let failed = |cx: &mut HeadlessAppContext| {
        cx.update(|cx| {
            explorer
                .read(cx)
                .rows
                .iter()
                .any(|row| matches!(row.kind, RowKind::ColumnsFailed { .. }))
        })
    };
    assert!(failed(&mut env.cx), "the failure shows");

    // The inspector describes it: the columns replace the failure
    env.describe("orders");
    wait_until(&mut env.cx, "the columns", |cx| {
        cx.update(|cx| {
            explorer.read(cx).rows.iter().any(
                |row| matches!(&row.kind, RowKind::Column { name, .. } if name == "customer_id"),
            )
        })
    });
    assert!(!failed(&mut env.cx));
}
