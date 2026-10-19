//! UI tests of the Connections page: a real window, real keystrokes.

use super::*;
use crate::RestoredState;
use crate::workspace::Workspace;
use dbcrust_gui_core::dbcrust_app::config::Config;
use dbcrust_gui_core::docker::DockerDaemon;
use dbcrust_gui_core::gui_state::GuiState;
use dbcrust_gui_core::launcher::docker_key;
use dbcrust_gui_core::{ConfigStore, DatabaseType};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, WindowOptions};
// `gpui_kit::*` (via `super::*`) exports GPUI's `test` macro
use std::prelude::v1::test;

fn session(name: &str, db_type: DatabaseType, target: &str) -> SessionEntry {
    SessionEntry {
        name: name.into(),
        target: target.into(),
        db_type,
        user: "app".into(),
        database: "shop".into(),
    }
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>, Entity<HomeView>) {
    crate::test_support::isolate();
    // Connections run on their own threads and wake the app from there: the
    // scheduler must expect activity off the test thread (it flags it otherwise)
    cx.executor().allow_parking();
    let (window, workspace) = cx.update(|cx| {
        crate::init(ConfigStore::from_config(Config::default()), cx);
        cx.set_global(RestoredState(GuiState::default()));
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    });
    let home = cx
        .update(|cx| workspace.read(cx).connections_view())
        .expect("the connections page");
    (window, workspace, home)
}

/// Replace what the page lists (nothing is read from or written to disk).
fn list(
    cx: &mut TestAppContext,
    home: &Entity<HomeView>,
    sessions: Vec<SessionEntry>,
    recents: Vec<RecentEntry>,
) {
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.sessions = sessions;
            home.set_recents(recents);
            home.docker = DockerListState::Ready {
                containers: Vec::new(),
                context: Some("orbstack".into()),
            };
            home.selected = None;
            home.pending_restore = None;
            home.pane = Pane::Details;
            home.rebuild(cx);
        })
    });
}

fn press(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .expect("press");
}

fn selected(cx: &mut TestAppContext, home: &Entity<HomeView>) -> Option<String> {
    cx.update(|cx| home.read(cx).selected.clone())
}

fn focus_search(cx: &mut TestAppContext, window: AnyWindowHandle, home: &Entity<HomeView>) {
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.focus_primary(window, cx))
    })
    .expect("focus");
}

#[gpui_kit::test]
fn the_page_selects_the_first_connection_and_hides_the_explorer(cx: &mut TestAppContext) {
    let (window, workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![
            session("prod", DatabaseType::PostgreSQL, "app@db:5432/shop"),
            session("staging", DatabaseType::MySQL, "app@stg:3306/shop"),
        ],
        Vec::new(),
    );
    assert_eq!(selected(cx, &home).as_deref(), Some("session:prod"));
    // The launcher's own list is the navigation: no second list beside it
    cx.update(|cx| assert!(!workspace.read(cx).explorer_visible()));
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("render");
}

#[gpui_kit::test]
fn arrows_move_the_selection_from_the_search_box(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![
            session("a", DatabaseType::PostgreSQL, "app@a:5432/shop"),
            session("b", DatabaseType::PostgreSQL, "app@b:5432/shop"),
            session("c", DatabaseType::PostgreSQL, "app@c:5432/shop"),
        ],
        Vec::new(),
    );
    focus_search(cx, window, &home);
    press(cx, window, "down");
    assert_eq!(selected(cx, &home).as_deref(), Some("session:b"));
    press(cx, window, "down");
    press(cx, window, "down");
    assert_eq!(selected(cx, &home).as_deref(), Some("session:c"), "clamped");
    press(cx, window, "up");
    assert_eq!(selected(cx, &home).as_deref(), Some("session:b"));
}

#[gpui_kit::test]
fn searching_narrows_the_list_and_escape_clears_it(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![
            session("prod", DatabaseType::PostgreSQL, "app@db:5432/shop"),
            session("staging", DatabaseType::MySQL, "app@stg:3306/shop"),
        ],
        Vec::new(),
    );
    focus_search(cx, window, &home);
    cx.update_window(window, |_, window, cx| window.input("stag", cx))
        .expect("type");
    cx.update(|cx| {
        let home = home.read(cx);
        assert_eq!(home.listing.entries.len(), 1);
        assert_eq!(home.selected.as_deref(), Some("session:staging"));
    });
    press(cx, window, "escape");
    cx.update(|cx| {
        let home = home.read(cx);
        assert_eq!(home.listing.entries.len(), 2);
    });
}

#[gpui_kit::test]
fn the_filters_split_the_list(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    let recent = {
        // The epoch is old enough for a list that only needs one entry
        let timestamp = Default::default();
        RecentEntry {
            display_name: "shop.db".into(),
            url: "sqlite:///tmp/shop.db".into(),
            db_type: DatabaseType::SQLite,
            timestamp,
            connection: dbcrust_gui_core::dbcrust_app::config::RecentConnection {
                connection_url: "sqlite:///tmp/shop.db".into(),
                display_name: "shop.db".into(),
                timestamp,
                database_type: DatabaseType::SQLite,
                success: true,
                options: Default::default(),
            },
        }
    };
    list(
        cx,
        &home,
        vec![session(
            "prod",
            DatabaseType::PostgreSQL,
            "app@db:5432/shop",
        )],
        vec![recent],
    );
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.set_filter(Filter::Saved, cx);
            assert_eq!(home.listing.entries.len(), 1);
            home.set_filter(Filter::Recent, cx);
            assert_eq!(home.listing.entries.len(), 1);
            assert_eq!(
                home.selected.as_deref(),
                Some("recent:sqlite:///tmp/shop.db")
            );
            home.set_filter(Filter::Docker, cx);
            assert!(home.listing.entries.is_empty());
            assert!(home.listing.docker_block.is_some());
        })
    });
}

#[gpui_kit::test]
fn enter_connects_and_a_failure_stays_with_its_target(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![session(
            "ghost",
            DatabaseType::PostgreSQL,
            "app@nowhere:5432/shop",
        )],
        Vec::new(),
    );
    focus_search(cx, window, &home);
    press(cx, window, "enter");
    // The saved session does not exist in this store: the attempt fails
    // quickly, and the failure belongs to the item
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        let done = cx.update(|cx| {
            AppState::global(cx)
                .read(cx)
                .attempt_for("session:ghost")
                .map(|attempt| !attempt.is_running())
        });
        match done {
            Some(true) => break,
            Some(false) => {}
            None => panic!("Enter started no attempt"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the attempt never ended"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    cx.update(|cx| {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let attempt = state.attempt_for("session:ghost").expect("attempt");
        assert!(matches!(
            attempt.state,
            crate::app_state::AttemptState::Failed(_)
        ));
        assert!(state.attempt_for("session:other").is_none());
    });
}

#[gpui_kit::test]
fn escape_cancels_a_running_attempt(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![session(
            "slow",
            DatabaseType::PostgreSQL,
            "app@127.0.0.1:1/shop",
        )],
        Vec::new(),
    );
    // A server that accepts and never answers keeps the attempt running
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let held = std::thread::spawn(move || {
        let _connection = listener.accept();
        std::thread::sleep(std::time::Duration::from_secs(30));
    });
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                crate::app_state::AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: "session:slow".into(),
                    subject: "slow".into(),
                    request: ConnectRequest::url(format!("postgres://app@127.0.0.1:{port}/shop")),
                    identity: None,
                    environment: None,
                },
                cx,
            );
        })
    });
    focus_search(cx, window, &home);
    cx.run_until_parked();
    let running = cx.update(|cx| {
        AppState::global(cx)
            .read(cx)
            .attempt_for("session:slow")
            .is_some_and(|attempt| attempt.is_running())
    });
    assert!(running);

    press(cx, window, "escape");
    cx.update(|cx| {
        assert!(
            AppState::global(cx)
                .read(cx)
                .attempt_for("session:slow")
                .is_none()
        );
        assert!(!AppState::global(cx).read(cx).is_connecting());
    });
    drop(held);
}

#[gpui_kit::test]
fn the_form_reports_the_first_invalid_field_and_keeps_the_input(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.open_new_form(window, cx));
    })
    .expect("open");
    cx.update(|cx| {
        let form = home.read(cx).form.clone();
        form.update(cx, |form, cx| {
            assert_eq!(form.mode(), &FormMode::New);
            // Real defaults, not placeholders
            assert_eq!(form.text(&form.host, cx), "localhost");
            assert_eq!(form.text(&form.port, cx), "5432");
            assert_eq!(form.text(&form.user, cx), "postgres");
        });
    });
    cx.update_window(window, |_, window, cx| {
        let form = home.read(cx).form.clone();
        form.update(cx, |form, cx| {
            form.set_text(&form.host.clone(), "", window, cx);
            form.set_text(&form.port.clone(), "99999", window, cx);
            form.connect(window, cx);
        });
    })
    .expect("connect");
    cx.update(|cx| {
        let form = home.read(cx).form.clone();
        let form = form.read(cx);
        let error = form.error.as_ref().expect("an error");
        assert_eq!(error.field, form::Field::Host);
        assert_eq!(form.text(&form.port, cx), "99999", "input is kept");
        assert!(
            AppState::global(cx).read(cx).attempts().is_empty(),
            "nothing was started"
        );
    });
}

#[gpui_kit::test]
fn pasting_a_url_fills_the_form_and_keeps_unknown_options(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.open_new_form(window, cx));
        let form = home.read(cx).form.clone();
        form.update(cx, |form, cx| {
            form.set_text(
                &form.import.clone(),
                "mysql://app:s3cret@db.internal:3307/shop?tls=true&connect_timeout=5",
                window,
                cx,
            );
            form.apply_import(window, cx);
        });
    })
    .expect("import");
    cx.update(|cx| {
        let form = home.read(cx).form.clone();
        let form = form.read(cx);
        assert_eq!(form.kind, form::Kind::Engine(DatabaseType::MySQL));
        assert_eq!(form.text(&form.host, cx), "db.internal");
        assert_eq!(form.text(&form.port, cx), "3307");
        assert_eq!(form.text(&form.database, cx), "shop");
        assert!(form.tls);
        assert_eq!(
            form.extra_options,
            vec![("connect_timeout".to_string(), "5".to_string())]
        );
        let submission = form.submission(false, cx).expect("submission");
        assert!(
            submission.url.contains("connect_timeout=5"),
            "{}",
            submission.url
        );
        assert!(submission.url.contains("s3cret"));
    });
}

#[gpui_kit::test]
fn docker_entries_open_their_details_and_stopped_ones_cannot_connect(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    let container =
        |name: &str, running: bool, port: Option<u16>| dbcrust_gui_core::docker::DockerContainer {
            id: format!("{name}-id"),
            name: name.into(),
            image: "postgres:16".into(),
            status: "Up".into(),
            db_type: Some(DatabaseType::PostgreSQL),
            host_port: port,
            container_port: Some(5432),
            running,
        };
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.preview_docker(
                DockerListState::Ready {
                    containers: vec![container("up", true, None), container("down", false, None)],
                    context: Some("orbstack".into()),
                },
                cx,
            );
            home.select(&docker_key("down"), cx);
            let entry = home.selected_entry().cloned().expect("selected");
            assert!(home.request_for(&entry).is_none(), "stopped: no request");
            home.select(&docker_key("up"), cx);
            let entry = home.selected_entry().cloned().expect("selected");
            assert!(home.request_for(&entry).is_some());
        })
    });
}

#[gpui_kit::test]
fn arrows_inside_the_form_do_not_move_the_list(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![
            session("a", DatabaseType::PostgreSQL, "app@a:5432/shop"),
            session("b", DatabaseType::PostgreSQL, "app@b:5432/shop"),
        ],
        Vec::new(),
    );
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.preview_engine_picker(window, cx));
    })
    .expect("open");
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("render");
    cx.update(|cx| assert!(home.read(cx).pane == Pane::Form));
    press(cx, window, "down");
    cx.update(|cx| {
        let home = home.read(cx);
        assert!(home.pane == Pane::Form, "the form stays open");
        assert_eq!(home.selected.as_deref(), Some("session:a"));
    });
}

/// Pump the app until `done` holds (the backends run on their own threads).
fn wait_until(cx: &mut TestAppContext, what: &str, done: impl Fn(&mut gpui_kit::App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if cx.update(|cx| done(cx)) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn sqlite_file() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("shop.db");
    std::fs::File::create(&path).expect("sqlite file");
    let url = format!("sqlite://{}", path.display());
    (dir, url)
}

fn start(cx: &mut TestAppContext, kind: AttemptKind, target: &str, url: &str) {
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                crate::app_state::AttemptSpec {
                    kind,
                    target: target.into(),
                    subject: "shop".into(),
                    request: ConnectRequest::url(url),
                    identity: None,
                    environment: None,
                },
                cx,
            );
        })
    });
}

#[gpui_kit::test]
fn a_successful_connect_hands_over_to_the_workspace(cx: &mut TestAppContext) {
    let (_window, workspace, _home) = open(cx);
    let (_dir, url) = sqlite_file();
    start(cx, AttemptKind::Connect, "form:new", &url);
    wait_until(cx, "the connection", |cx| {
        !AppState::global(cx).read(cx).connections().is_empty()
    });
    cx.update(|cx| {
        let state = AppState::global(cx);
        let state = state.read(cx);
        assert!(
            state.attempts().is_empty(),
            "a success leaves no attempt behind"
        );
        assert!(!state.is_connecting());
        // A query tab opened next to the Connections page
        assert_eq!(workspace.read(cx).tab_count(), 2);
    });
}

#[gpui_kit::test]
fn a_test_connects_and_closes_without_a_trace(cx: &mut TestAppContext) {
    let (_window, workspace, _home) = open(cx);
    let (_dir, url) = sqlite_file();
    start(cx, AttemptKind::Test, "form:new#test", &url);
    wait_until(cx, "the test result", |cx| {
        AppState::global(cx)
            .read(cx)
            .attempt_for("form:new#test")
            .is_some_and(|attempt| !attempt.is_running())
    });
    cx.update(|cx| {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let attempt = state.attempt_for("form:new#test").expect("attempt");
        assert!(
            matches!(attempt.state, crate::app_state::AttemptState::Tested(_)),
            "{:?}",
            attempt.state
        );
        assert!(state.connections().is_empty(), "nothing stays open");
        assert!(
            state.store().recent_connections().is_empty(),
            "no recent entry"
        );
        assert_eq!(workspace.read(cx).tab_count(), 1, "no tab");
    });
}

fn open_sized(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
) -> (AnyWindowHandle, Entity<Workspace>, Entity<HomeView>) {
    crate::test_support::isolate();
    // Connections run on their own threads and wake the app from there: the
    // scheduler must expect activity off the test thread (it flags it otherwise)
    cx.executor().allow_parking();
    let (window, workspace) = cx.update(|cx| {
        crate::init(ConfigStore::from_config(Config::default()), cx);
        cx.set_global(RestoredState(GuiState::default()));
        let options = WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
                origin: gpui_kit::Point::default(),
                size: gpui_kit::size(gpui_kit::px(width), gpui_kit::px(height)),
            })),
            ..WindowOptions::default()
        };
        crate::open_main_window(options, cx).expect("open the main window")
    });
    let home = cx
        .update(|cx| workspace.read(cx).connections_view())
        .expect("the connections page");
    (window, workspace, home)
}

fn right_edge(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) -> f32 {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find(id).bounds();
        f32::from(bounds.origin.x + bounds.size.width)
    })
    .expect("window")
}

#[gpui_kit::test]
fn the_page_fits_the_smallest_window_and_narrows_its_list(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open_sized(cx, 960., 600.);
    list(
        cx,
        &home,
        vec![session(
            "prod",
            DatabaseType::PostgreSQL,
            "app@db:5432/shop",
        )],
        Vec::new(),
    );
    // The list is 280 wide (title row padding 16): the New button ends at 264
    let new_edge = right_edge(cx, window, "new-connection");
    assert!((new_edge - 264.).abs() < 2., "list pane width: {new_edge}");
    // Every footer action stays inside the window
    let connect = right_edge(cx, window, "details-connect");
    assert!(connect <= 960. - 24., "Connect ends at {connect}");

    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.open_new_form(window, cx));
    })
    .expect("form");
    let connect = right_edge(cx, window, "form-connect");
    assert!(connect <= 960. - 24., "form Connect ends at {connect}");
}

#[gpui_kit::test]
fn the_list_is_wider_in_a_roomy_window(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open_sized(cx, 1440., 900.);
    list(
        cx,
        &home,
        vec![session(
            "prod",
            DatabaseType::PostgreSQL,
            "app@db:5432/shop",
        )],
        Vec::new(),
    );
    let new_edge = right_edge(cx, window, "new-connection");
    assert!((new_edge - 304.).abs() < 2., "list pane width: {new_edge}");
}

#[gpui_kit::test]
fn a_read_only_preference_decides_how_enter_connects(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(
        cx,
        &home,
        vec![session(
            "ghost",
            DatabaseType::PostgreSQL,
            "app@nowhere:5432/shop",
        )],
        Vec::new(),
    );
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.set_read_only_preference("session:ghost", true, cx)
        })
    });
    focus_search(cx, window, &home);
    press(cx, window, "enter");
    cx.update(|cx| {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let attempt = state.attempt_for("session:ghost").expect("attempt");
        assert!(attempt.request.read_only, "the preference is applied");
    });
    // ⇧↵ asks for read-only whatever the preference says; the attempt of a
    // target is never duplicated while it runs
    wait_until(cx, "the attempt to end", |cx| {
        AppState::global(cx)
            .read(cx)
            .attempt_for("session:ghost")
            .is_some_and(|attempt| !attempt.is_running())
    });
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.set_read_only_preference("session:ghost", false, cx);
            home.connect_key("session:ghost", Some(true), cx);
        });
        let state = AppState::global(cx);
        let state = state.read(cx);
        assert!(
            state
                .attempt_for("session:ghost")
                .expect("attempt")
                .request
                .read_only
        );
    });
}

// ── Reuse of live connections, forms, saves, removals (review findings) ──

fn store_of(cx: &mut TestAppContext) -> ConfigStore {
    cx.update(|cx| AppState::global(cx).read(cx).store().clone())
}

/// A saved session pointing at a new SQLite file, picked up by the page.
fn saved_sqlite(
    cx: &mut TestAppContext,
    home: &Entity<HomeView>,
    name: &str,
) -> (tempfile::TempDir, String) {
    let (dir, url) = sqlite_file();
    form::save_session(&store_of(cx), name, &url, None).expect("save the session");
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));
    (dir, url)
}

fn connect_and_wait(cx: &mut TestAppContext, home: &Entity<HomeView>, key: &str, wanted: usize) {
    cx.update(|cx| home.update(cx, |home, cx| home.connect_key(key, None, cx)));
    wait_until(cx, "the connection", |cx| {
        AppState::global(cx).read(cx).connections().len() >= wanted
    });
}

fn entry_of(cx: &mut TestAppContext, home: &Entity<HomeView>, key: &str) -> Entry {
    cx.update(|cx| home.read(cx).listing.entry(key).cloned())
        .expect("the entry is listed")
}

fn daemon_at(socket: &str) -> DockerDaemon {
    use dbcrust_gui_core::dbcrust_docker::docker::DockerEndpoint;
    DockerDaemon::of(&DockerEndpoint::explicit(socket).expect("endpoint"))
}

fn container(name: &str) -> dbcrust_gui_core::docker::DockerContainer {
    dbcrust_gui_core::docker::DockerContainer {
        id: format!("{name}-id"),
        name: name.into(),
        image: "postgres:16".into(),
        status: "Up".into(),
        db_type: Some(DatabaseType::PostgreSQL),
        host_port: None,
        container_port: Some(5432),
        running: true,
    }
}

fn show_containers(
    cx: &mut TestAppContext,
    home: &Entity<HomeView>,
    names: &[&str],
    daemon: Option<DockerDaemon>,
) {
    let containers = names.iter().map(|name| container(name)).collect();
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.set_docker_daemon(daemon);
            home.preview_docker(
                DockerListState::Ready {
                    containers,
                    context: Some("orbstack".into()),
                },
                cx,
            );
        })
    });
}

#[gpui_kit::test]
fn an_edited_session_is_not_the_connection_it_used_to_open(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    let (_dir_a, _) = saved_sqlite(cx, &home, "shop");
    connect_and_wait(cx, &home, "session:shop", 1);

    let entry = entry_of(cx, &home, "session:shop");
    assert!(
        cx.update(|cx| home.read(cx).live_for(&entry, None, cx))
            .is_some()
    );

    // The session is edited to open another file: the open connection (still
    // named "shop") is no longer what it opens
    let (_dir_b, url_b) = sqlite_file();
    form::save_session(&store_of(cx), "shop", &url_b, None).expect("edit");
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));
    let entry = entry_of(cx, &home, "session:shop");
    assert!(
        cx.update(|cx| home.read(cx).live_for(&entry, None, cx))
            .is_none(),
        "the old connection must not stand for the edited session"
    );
    connect_and_wait(cx, &home, "session:shop", 2);
    cx.update(|cx| assert_eq!(AppState::global(cx).read(cx).connections().len(), 2));
}

#[gpui_kit::test]
fn a_container_connection_is_reused_only_on_the_daemon_that_listed_it(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    let orbstack = daemon_at("/tmp/orbstack.sock");
    let desktop = daemon_at("/tmp/desktop.sock");
    let pin = launcher::daemon_pin(&orbstack);
    assert_ne!(pin, launcher::daemon_pin(&desktop));

    // A connection the page opened for "pg" on OrbStack (a SQLite file stands
    // in for the database: what matters is the identity it was opened with)
    let (_dir, url) = sqlite_file();
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: "docker:pg".into(),
                    subject: "pg".into(),
                    request: ConnectRequest::url(url),
                    identity: Some(TargetIdentity::container("pg", pin.as_deref())),
                    environment: None,
                },
                cx,
            );
        })
    });
    wait_until(cx, "the connection", |cx| {
        !AppState::global(cx).read(cx).connections().is_empty()
    });

    let live = |cx: &mut TestAppContext, home: &Entity<HomeView>| {
        let entry = entry_of(cx, home, "docker:pg");
        cx.update(|cx| home.read(cx).live_for(&entry, None, cx))
    };
    show_containers(cx, &home, &["pg"], Some(orbstack.clone()));
    assert!(
        live(cx, &home).is_some(),
        "the same container on the same daemon"
    );
    show_containers(cx, &home, &["pg"], Some(desktop));
    assert!(
        live(cx, &home).is_none(),
        "a same-named container elsewhere"
    );
    show_containers(cx, &home, &["pg"], None);
    assert!(
        live(cx, &home).is_none(),
        "an unpinned listing is not the pinned one"
    );
}

#[gpui_kit::test]
fn the_global_read_only_default_is_part_of_what_is_reused(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    let (_dir, _) = saved_sqlite(cx, &home, "shop");
    connect_and_wait(cx, &home, "session:shop", 1);
    let entry = entry_of(cx, &home, "session:shop");
    cx.update(|cx| {
        let home = home.read(cx);
        assert!(!home.effective_read_only("session:shop", None, cx));
        assert!(home.live_for(&entry, Some(false), cx).is_some());
    });

    // Settings now open everything read-only: the writable connection is not
    // handed back for a connect that promises read-only
    store_of(cx).update_settings(|config| config.read_only_default = true);
    cx.update(|cx| {
        let home = home.read(cx);
        assert!(home.effective_read_only("session:shop", None, cx));
        assert!(home.effective_read_only("session:shop", Some(false), cx));
        assert!(home.live_for(&entry, Some(true), cx).is_none());
    });
    connect_and_wait(cx, &home, "session:shop", 2);
    cx.update(|cx| {
        let state = AppState::global(cx);
        let state = state.read(cx);
        assert!(
            state
                .connections()
                .iter()
                .filter(|live| live.summary.read_only)
                .count()
                >= 1
        );
        let home = home.read(cx);
        assert!(home.live_for(&entry, Some(true), cx).is_some());
    });
}

fn import(cx: &mut TestAppContext, window: AnyWindowHandle, home: &Entity<HomeView>, url: &str) {
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.open_new_form(window, cx));
        let form = home.read(cx).form.clone();
        form.update(cx, |form, cx| {
            form.set_text(&form.import.clone(), url, window, cx);
            form.apply_import(window, cx);
        });
    })
    .expect("import");
}

fn info_of(
    request: &ConnectRequest,
) -> Box<dbcrust_gui_core::dbcrust_core::database::ConnectionInfo> {
    match &request.target {
        dbcrust_gui_core::ConnectTarget::Info(info) => info.clone(),
        other => panic!("expected resolved connection details, got {other:?}"),
    }
}

#[gpui_kit::test]
fn a_docker_url_keeps_its_options_through_import_edit_test_connect_and_save(
    cx: &mut TestAppContext,
) {
    let (window, _workspace, home) = open(cx);
    import(
        cx,
        window,
        &home,
        "docker://app@pg/shop?sslmode=verify-full&ssl=true",
    );
    let form = cx.update(|cx| home.read(cx).form.clone());
    let submission = cx
        .update(|cx| form.read(cx).submission(true, cx))
        .unwrap_or_else(|error| panic!("no submission: {}", error.message));
    let info = info_of(&submission.request);
    assert_eq!(info.docker_container.as_deref(), Some("pg"));
    assert_eq!(info.username.as_deref(), Some("app"));
    assert_eq!(info.database.as_deref(), Some("shop"));
    assert_eq!(
        info.options.get("sslmode").map(String::as_str),
        Some("verify-full")
    );
    assert!(info.use_tls, "ssl=true is kept");
    assert_eq!(
        submission.recent_url,
        "docker://app@pg/shop?ssl=true&sslmode=verify-full"
    );
    assert_eq!(
        Some(submission.identity.clone()),
        TargetIdentity::from_url("docker://app@pg/shop?sslmode=verify-full&ssl=true")
    );

    // Saving keeps them too, and editing the saved session shows them again
    let store = store_of(cx);
    form::save_session(&store, "pg-tls", &submission.url, None).expect("save");
    let saved = store
        .read(|config| config.get_session("pg-tls").cloned())
        .expect("saved");
    assert_eq!(
        saved.options.get("sslmode").map(String::as_str),
        Some("verify-full")
    );
    assert_eq!(saved.options.get("ssl").map(String::as_str), Some("true"));
    cx.update_window(window, |_, window, cx| {
        let form = home.read(cx).form.clone();
        form.update(cx, |form, cx| {
            form.load_session(
                "pg-tls",
                &saved,
                &ConnectionMeta::new("session:pg-tls"),
                false,
                window,
                cx,
            )
        });
    })
    .expect("edit");
    let again = cx
        .update(|cx| form.read(cx).submission(false, cx))
        .unwrap_or_else(|error| panic!("no submission: {}", error.message));
    let info = info_of(&again.request);
    assert_eq!(
        info.options.get("sslmode").map(String::as_str),
        Some("verify-full")
    );
    assert!(info.use_tls);
}

#[gpui_kit::test]
fn editing_a_discovered_container_keeps_its_daemon_and_its_own_credentials(
    cx: &mut TestAppContext,
) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    let daemon = daemon_at("/tmp/orbstack.sock");
    let pin = launcher::daemon_pin(&daemon);
    show_containers(cx, &home, &["mysql-1"], Some(daemon));
    cx.update(|cx| home.update(cx, |home, cx| home.select("docker:mysql-1", cx)));
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.edit_selected(window, cx))
    })
    .expect("edit");

    let form = cx.update(|cx| home.read(cx).form.clone());
    cx.update(|cx| {
        let form = form.read(cx);
        assert_eq!(form.kind, form::Kind::Docker);
        // No PostgreSQL login injected into a MySQL container
        assert_eq!(form.text(&form.user, cx), "");
        assert_eq!(form.text(&form.database, cx), "");
    });
    let submission = cx
        .update(|cx| form.read(cx).submission(false, cx))
        .unwrap_or_else(|error| panic!("no submission: {}", error.message));
    let info = info_of(&submission.request);
    assert_eq!(info.username, None, "the container's environment decides");
    assert_eq!(info.database, None);
    assert_eq!(
        launcher::daemon_pin_of(&submission.request),
        pin,
        "Test and Connect use the listing's daemon"
    );
    assert_eq!(
        submission.identity,
        TargetIdentity::container("mysql-1", pin.as_deref())
    );

    // The user's own entries are applied on top, and the pin stays
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| {
            form.set_text(&form.user.clone(), "app", window, cx);
            form.set_text(&form.database.clone(), "shop", window, cx);
        });
    })
    .expect("type");
    let typed = cx
        .update(|cx| form.read(cx).submission(false, cx))
        .unwrap_or_else(|error| panic!("no submission: {}", error.message));
    let info = info_of(&typed.request);
    assert_eq!(info.username.as_deref(), Some("app"));
    assert_eq!(info.database.as_deref(), Some("shop"));
    assert_eq!(launcher::daemon_pin_of(&typed.request), pin);
}

#[gpui_kit::test]
fn a_form_attempt_belongs_to_its_subject_only(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.open_new_form(window, cx))
    })
    .expect("open");
    let form = cx.update(|cx| home.read(cx).form.clone());
    let first_key = cx.update(|cx| form.read(cx).attempt_key());
    // A failing attempt of the first subject
    start(
        cx,
        AttemptKind::Connect,
        &first_key,
        "sqlite:///nonexistent-dir/nowhere.db",
    );
    wait_until(cx, "the failure", |cx| {
        AppState::global(cx)
            .read(cx)
            .attempt_for(&first_key)
            .is_some_and(|attempt| !attempt.is_running())
    });
    assert!(!cx.update(|cx| attempts::is_busy(&first_key, cx)));

    // The form is pointed at another subject: the result stays behind
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| form.reset(window, cx));
    })
    .expect("reset");
    let second_key = cx.update(|cx| form.read(cx).attempt_key());
    assert_ne!(first_key, second_key);
    cx.update(|cx| {
        assert!(
            AppState::global(cx)
                .read(cx)
                .attempt_for(&first_key)
                .is_none()
        );
        assert!(attempts::attempt_panels(&second_key, None, cx).is_empty());
    });

    // A running attempt of one subject does not block the next one
    let (_dir, url) = sqlite_file();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let _held = std::thread::spawn(move || {
        let _connection = listener.accept();
        std::thread::sleep(std::time::Duration::from_secs(4));
    });
    start(
        cx,
        AttemptKind::Connect,
        &second_key,
        &format!("postgres://app@127.0.0.1:{port}/shop"),
    );
    cx.run_until_parked();
    assert!(cx.update(|cx| attempts::is_busy(&second_key, cx)));
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| form.reset(window, cx));
    })
    .expect("reset again");
    let third_key = cx.update(|cx| form.read(cx).attempt_key());
    cx.update(|cx| {
        assert!(
            !attempts::is_busy(&third_key, cx),
            "the new subject is free to submit"
        );
        let state = AppState::global(cx);
        assert!(
            state.read(cx).attempt_for(&second_key).is_none(),
            "the attempt the form left behind is cancelled, not left to open a connection"
        );
        assert!(!state.read(cx).is_connecting());
    });
    let _ = url;
}

#[gpui_kit::test]
fn a_late_save_does_not_reset_the_form_that_moved_on_and_saves_are_not_doubled(
    cx: &mut TestAppContext,
) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    import(cx, window, &home, "postgres://app@db.internal:5432/shop");
    let form = cx.update(|cx| home.read(cx).form.clone());

    // Two quick Saves of the same subject produce one request
    let saved_events = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let counter = saved_events.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&form, move |_, event: &FormEvent, _| {
            if matches!(event, FormEvent::Save(_)) {
                counter.set(counter.get() + 1);
            }
        })
    });
    let submission = cx
        .update(|cx| form.read(cx).submission(true, cx))
        .unwrap_or_else(|error| panic!("no submission: {}", error.message));
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| {
            form.set_text(&form.name.clone(), "late", window, cx);
            form.save(window, cx);
            form.save(window, cx);
        });
    })
    .expect("save twice");
    assert_eq!(
        saved_events.get(),
        1,
        "the second Save is ignored while one is in flight"
    );
    let _ = submission;

    // The Save finishes after the user moved to something else: the form
    // they are on is left alone
    cx.update_window(window, |_, window, cx| {
        let before = home.read(cx).form.read(cx).generation();
        form.update(cx, |form, cx| form.reset(window, cx));
        form.update(cx, |form, cx| {
            form.set_text(&form.name.clone(), "something else", window, cx)
        });
        assert_ne!(before, form.read(cx).generation());
    })
    .expect("move on");
    let generation = cx.update(|cx| form.read(cx).generation());
    wait_until(cx, "the save", |cx| {
        store_of_app(cx).read(|config| config.get_session("late").is_some())
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let home = home.read(cx);
        assert!(home.pane == Pane::Form, "still on the form");
        let form = home.form.read(cx);
        assert_eq!(form.generation(), generation);
        assert_eq!(
            form.text(&form.name, cx),
            "something else",
            "the typed name is kept"
        );
    });
}

fn store_of_app(cx: &gpui_kit::App) -> ConfigStore {
    AppState::global(cx).read(cx).store().clone()
}

#[gpui_kit::test]
fn a_removal_names_what_it_removes_when_the_dialog_opens(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    let store = store_of(cx);
    for (name, port) in [("alpha", 5432), ("omega", 5433)] {
        form::save_session(
            &store,
            name,
            &format!("postgres://app@db.internal:{port}/shop"),
            None,
        )
        .expect("save");
    }
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));
    let entry = entry_of(cx, &home, "session:omega");
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.confirm_remove(&entry, window, cx))
    })
    .expect("open the dialog");

    // The list changes before the user confirms: "omega" moves
    let (_dir, url) = sqlite_file();
    form::save_session(&store, "aaa-new", &url, None).expect("save another");
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));
    cx.update_window(window, |_, window, cx| window.press("enter", cx))
        .expect("confirm");
    wait_until(cx, "the removal", |cx| {
        store_of_app(cx).read(|config| config.get_session("omega").is_none())
    });
    let names: Vec<String> = store
        .sessions()
        .into_iter()
        .map(|session| session.name)
        .collect();
    assert!(
        names.contains(&"alpha".to_string()) && names.contains(&"aaa-new".to_string()),
        "{names:?}"
    );
    assert!(!names.contains(&"omega".to_string()));
}

#[gpui_kit::test]
fn a_live_connection_shows_the_environment_its_entry_was_given(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    let (_dir, _) = saved_sqlite(cx, &home, "shop");
    connect_and_wait(cx, &home, "session:shop", 1);
    let id = cx.update(|cx| AppState::global(cx).read(cx).connections()[0].id());
    cx.update(|cx| assert_eq!(AppState::global(cx).read(cx).environment_of(id), None));

    let mut meta = ConnectionMeta::new("session:shop");
    meta.environment = "Production".into();
    cx.update(|cx| home.update(cx, |home, cx| home.store_meta(meta, cx)));
    cx.update(|cx| {
        let tag = AppState::global(cx)
            .read(cx)
            .environment_of(id)
            .expect("an environment tag");
        assert_eq!(tag.label, "Production");
        assert_eq!(tag.color, launcher::EnvColor::Red);
    });
}

// ── Second review round ──────────────────────────────────────────────

fn pin_a() -> Option<String> {
    Some("unix:///a.sock".into())
}

fn pin_b() -> Option<String> {
    Some("unix:///b.sock".into())
}

fn pin_unknown() -> Option<String> {
    None
}

#[gpui_kit::test]
fn a_docker_session_is_reused_only_on_the_daemon_it_was_connected_on(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    form::save_session(&store_of(cx), "dock", "docker://app@pg/shop", None).expect("save");
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));

    // Connected on daemon A (a SQLite file stands in; what counts is the
    // identity the live connection records, with its resolved daemon)
    let (_dir, url) = sqlite_file();
    let identity = cx.update(|cx| {
        let home = home.read(cx);
        let entry = home.listing.entry("session:dock").cloned().expect("entry");
        let mut identity = home.entry_identity(&entry).expect("identity");
        identity = identity.with_docker_daemon(Some("unix:///a.sock"));
        identity
    });
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: "session:dock".into(),
                    subject: "dock".into(),
                    request: ConnectRequest::url(url),
                    identity: Some(identity),
                    environment: None,
                },
                cx,
            );
        })
    });
    wait_until(cx, "the connection", |cx| {
        !AppState::global(cx).read(cx).connections().is_empty()
    });
    let live = |cx: &mut TestAppContext, pin: Option<&'static str>| {
        cx.update(|cx| {
            home.update(cx, |home, _| {
                home.current_docker_pin = pin.map(str::to_string);
            });
            let home = home.read(cx);
            let entry = home.listing.entry("session:dock").cloned().expect("entry");
            home.live_for(&entry, None, cx)
        })
    };
    assert!(live(cx, Some("unix:///a.sock")).is_some(), "same daemon");
    assert!(
        live(cx, Some("unix:///b.sock")).is_none(),
        "the context was switched"
    );
    assert!(
        live(cx, None).is_none(),
        "a daemon that cannot be confirmed is declined"
    );

    // Connecting re-resolves the current context (off the UI thread): on B
    // the session opens anew
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.resolve_docker_pin = pin_b;
            home.connect_key("session:dock", None, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(
            AppState::global(cx)
                .read(cx)
                .attempt_for("session:dock")
                .is_some(),
            "no reuse across daemons"
        );
    });
    let _ = (pin_a, pin_unknown);
}

fn type_into(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    form: &Entity<ConnectionForm>,
    field: fn(&ConnectionForm) -> Entity<InputState>,
    text: &str,
) {
    cx.update_window(window, |_, window, cx| {
        let input = field(form.read(cx));
        input.update(cx, |input, cx| input.focus(window, cx));
        window.render_frame(cx);
        window.input(text, cx);
    })
    .expect("type");
}

#[gpui_kit::test]
fn editing_the_fields_leaves_old_attempts_behind_but_keeps_the_daemon(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    let daemon = daemon_at("/tmp/orbstack.sock");
    let pin = launcher::daemon_pin(&daemon);
    show_containers(cx, &home, &["pg"], Some(daemon));
    cx.update(|cx| home.update(cx, |home, cx| home.select("docker:pg", cx)));
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.edit_selected(window, cx))
    })
    .expect("edit");
    let form = cx.update(|cx| home.read(cx).form.clone());

    // A test of these fields fails
    let before = cx.update(|cx| form.read(cx).attempt_key());
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.preview_attempt(
                AttemptSpec {
                    kind: AttemptKind::Test,
                    target: attempts::test_target(&before),
                    subject: "pg".into(),
                    request: ConnectRequest::url("docker://pg"),
                    identity: None,
                    environment: None,
                },
                crate::app_state::AttemptState::Failed(crate::app_state::AttemptFailure {
                    message: "no".into(),
                    docker: None,
                }),
                &[],
                std::time::Duration::from_secs(1),
                cx,
            )
        });
        assert_eq!(attempts::attempt_panels(&before, None, cx).len(), 1);
    });

    // The user edits a field: that result describes other fields now
    type_into(cx, window, &form, |form| form.user.clone(), "app");
    let after = cx.update(|cx| form.read(cx).attempt_key());
    assert_ne!(before, after);
    cx.update(|cx| {
        assert!(attempts::attempt_panels(&after, None, cx).is_empty());
        assert!(
            AppState::global(cx)
                .read(cx)
                .attempt_for(&attempts::test_target(&before))
                .is_none(),
            "Retry can no longer resubmit the old fields"
        );
        // ...while the discovered container still goes to the listing's daemon
        let submission = form.read(cx).submission(false, cx).expect("submission");
        assert_eq!(launcher::daemon_pin_of(&submission.request), pin);
        assert_eq!(
            info_of(&submission.request).username.as_deref(),
            Some("app")
        );
    });

    // The engine of a server form counts as an edit too
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| form.reset(window, cx));
    })
    .expect("reset");
    let key = cx.update(|cx| form.read(cx).attempt_key());
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| {
            form.set_engine(DatabaseType::MySQL, window, cx)
        });
    })
    .expect("engine");
    assert_ne!(key, cx.update(|cx| form.read(cx).attempt_key()));
}

#[gpui_kit::test]
fn edits_made_while_saving_are_not_discarded(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    import(cx, window, &home, "postgres://app@db.internal:5432/shop");
    let form = cx.update(|cx| home.read(cx).form.clone());
    cx.update_window(window, |_, window, cx| {
        form.update(cx, |form, cx| {
            form.set_text(&form.name.clone(), "kept", window, cx);
            form.save(window, cx);
        });
    })
    .expect("save");
    // The user keeps typing before the save completes
    type_into(cx, window, &form, |form| form.host.clone(), "-2");
    wait_until(cx, "the save", |cx| {
        store_of_app(cx).read(|config| config.get_session("kept").is_some())
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let home = home.read(cx);
        assert!(home.pane == Pane::Form, "the form is still open");
        let form = home.form.read(cx);
        assert!(
            form.text(&form.host, cx).ends_with("-2"),
            "the new edit is kept"
        );
        assert_eq!(form.text(&form.name, cx), "kept");
        // The form now owns the saved connection: saving again updates "kept"
        // instead of rejecting it as an existing name
        assert_eq!(
            form.mode(),
            &FormMode::Edit {
                session: "kept".into()
            }
        );
        form.submission(true, cx)
            .unwrap_or_else(|error| panic!("a second save is refused: {}", error.message));
    });
}

#[gpui_kit::test]
fn editing_a_field_cancels_the_running_attempt_it_orphans(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    import(cx, window, &home, "postgres://app@127.0.0.1:1/shop");
    let form = cx.update(|cx| home.read(cx).form.clone());
    // A connection that never answers
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().expect("addr").port();
    let _held = std::thread::spawn(move || {
        let _connection = listener.accept();
        std::thread::sleep(std::time::Duration::from_secs(30));
    });
    let key = cx.update(|cx| form.read(cx).attempt_key());
    start(
        cx,
        AttemptKind::Connect,
        &key,
        &format!("postgres://app@127.0.0.1:{port}/shop"),
    );
    cx.run_until_parked();
    assert!(cx.update(|cx| attempts::is_busy(&key, cx)));

    // Editing the host supersedes it: it must not linger invisibly
    type_into(cx, window, &form, |form| form.host.clone(), "x");
    cx.update(|cx| {
        let state = AppState::global(cx);
        assert!(state.read(cx).attempt_for(&key).is_none());
        assert!(
            !state.read(cx).is_connecting(),
            "nothing can open a connection later"
        );
    });
}

#[gpui_kit::test]
fn the_environment_a_connection_was_opened_with_wins_over_the_entry_binding(
    cx: &mut TestAppContext,
) {
    let (_window, _workspace, _home) = open(cx);
    let identity = TargetIdentity::from_url("postgres://app@db.internal:5432/shop").expect("id");
    let local = EnvTag {
        label: "Local".into(),
        color: launcher::EnvColor::Green,
    };
    let production = EnvTag {
        label: "Production".into(),
        color: launcher::EnvColor::Red,
    };
    // The saved session this was copied from is tagged Local
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.set_environments(vec![(identity.clone(), local.clone())], cx)
        })
    });
    let (_dir, url) = sqlite_file();
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: "form:9.9".into(),
                    subject: "copy".into(),
                    request: ConnectRequest::url(url),
                    identity: Some(identity),
                    environment: Some(production.clone()),
                },
                cx,
            );
        })
    });
    wait_until(cx, "the connection", |cx| {
        !AppState::global(cx).read(cx).connections().is_empty()
    });
    cx.update(|cx| {
        let state = AppState::global(cx);
        let id = state.read(cx).connections()[0].id();
        assert_eq!(state.read(cx).environment_of(id), Some(production));
    });
}

#[gpui_kit::test]
fn the_environment_picked_on_the_form_goes_with_the_live_connection(cx: &mut TestAppContext) {
    let (window, _workspace, home) = open(cx);
    list(cx, &home, Vec::new(), Vec::new());
    let daemon = daemon_at("/tmp/orbstack.sock");
    show_containers(cx, &home, &["pg"], Some(daemon));
    cx.update(|cx| home.update(cx, |home, cx| home.select("docker:pg", cx)));
    cx.update_window(window, |_, window, cx| {
        home.update(cx, |home, cx| home.edit_selected(window, cx))
    })
    .expect("edit");
    let form = cx.update(|cx| home.read(cx).form.clone());
    type_into(cx, window, &form, |form| form.user.clone(), "app");
    type_into(cx, window, &form, |form| form.database.clone(), "shop");
    cx.update(|cx| {
        form.update(cx, |form, cx| {
            form.env = Some(EnvTag {
                label: "Production".into(),
                color: launcher::EnvColor::Red,
            });
            form.touch(false, cx);
        })
    });

    // The submission carries the tag, the pinned identity with the overrides
    let submission = cx
        .update(|cx| form.read(cx).submission(false, cx))
        .expect("submission");
    assert_eq!(
        submission.env.as_ref().map(|tag| tag.label.as_str()),
        Some("Production")
    );
    cx.update(|cx| home.update(cx, |home, cx| home.connect_submission(&submission, cx)));
    cx.update(|cx| {
        let state = AppState::global(cx);
        let attempt = state
            .read(cx)
            .attempt_for(&submission.attempt_key)
            .expect("the connect attempt");
        assert_eq!(
            attempt.environment.as_ref().map(|tag| tag.label.as_str()),
            Some("Production"),
            "the tag is attached to the attempt itself"
        );
    });

    // ...so the live connection shows it although no list entry matches
    // (the overrides make the identity differ from the bare container's)
    let (_dir, url) = sqlite_file();
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: "direct".into(),
                    subject: "pg".into(),
                    request: ConnectRequest::url(url),
                    identity: Some(submission.identity.clone()),
                    environment: submission.env.clone(),
                },
                cx,
            );
        })
    });
    wait_until(cx, "the connection", |cx| {
        !AppState::global(cx).read(cx).connections().is_empty()
    });
    cx.update(|cx| {
        let state = AppState::global(cx);
        let state = state.read(cx);
        let id = state.connections()[0].id();
        assert_eq!(
            state.environment_of(id).map(|tag| tag.label),
            Some("Production".to_string())
        );
    });
}

#[gpui_kit::test]
fn a_docker_connect_survives_a_filter_change_while_the_context_resolves(cx: &mut TestAppContext) {
    let (_window, _workspace, home) = open(cx);
    form::save_session(&store_of(cx), "dock", "docker://app@pg/shop", None).expect("save");
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));

    // Connect, then filter the session out before the Docker context is known
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.connect_key("session:dock", None, cx);
            home.set_filter(Filter::Recent, cx);
            assert!(home.listing.entry("session:dock").is_none());
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let attempt = AppState::global(cx)
            .read(cx)
            .attempt_for("session:dock")
            .map(|attempt| attempt.subject.clone());
        assert_eq!(
            attempt.as_deref(),
            Some("dock"),
            "the connect still started"
        );
    });
}

#[gpui_kit::test]
fn a_session_edited_while_the_context_resolves_is_recorded_as_what_it_opens(
    cx: &mut TestAppContext,
) {
    let (_window, _workspace, home) = open(cx);
    let store = store_of(cx);
    form::save_session(&store, "dock", "docker://app@pg/shop", None).expect("save");
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));
    let listed = cx.update(|cx| {
        let home = home.read(cx);
        let entry = home.listing.entry("session:dock").cloned().expect("entry");
        home.entry_identity(&entry).expect("identity")
    });

    // Connect, then the session points elsewhere before the context is known
    cx.update(|cx| {
        home.update(cx, |home, cx| {
            home.resolve_docker_pin = pin_b;
            home.connect_key("session:dock", None, cx);
        })
    });
    form::save_session(&store, "dock", "docker://app@pg/other", None).expect("edit");
    cx.run_until_parked();

    // The connection opens the edited session (the request names it) and is
    // recorded as that, never as the target it was listed with
    let edited = store
        .read(|config| config.get_session("dock").map(TargetIdentity::from_session))
        .expect("the edited session")
        .with_docker_daemon(pin_b().as_deref());
    assert_ne!(edited, listed);
    cx.update(|cx| {
        let state = AppState::global(cx);
        let attempt = state
            .read(cx)
            .attempt_for("session:dock")
            .expect("an attempt");
        assert_eq!(attempt.identity.as_ref(), Some(&edited));
    });

    // Deleted meanwhile: the attempt fails as not found and records nothing
    cx.update(|cx| {
        AppState::global(cx).update(cx, |state, cx| {
            if let Some(id) = state.attempt_for("session:dock").map(|attempt| attempt.id) {
                state.dismiss_attempt(id, cx);
            }
        })
    });
    cx.update(|cx| home.update(cx, |home, cx| home.read_lists(cx)));
    cx.update(|cx| home.update(cx, |home, cx| home.connect_key("session:dock", None, cx)));
    store.delete_session("dock").expect("delete");
    cx.run_until_parked();
    cx.update(|cx| {
        let state = AppState::global(cx);
        let attempt = state
            .read(cx)
            .attempt_for("session:dock")
            .expect("an attempt");
        assert_eq!(attempt.identity, None);
    });
}
