//! UI integration tests of the workspace shell: real window, real actions.

use super::{Workspace, tabs::TabKind};
use crate::RestoredState;
use crate::actions as act;
use dbcrust_gui_core::ConfigStore;
use dbcrust_gui_core::dbcrust::config::Config;
use dbcrust_gui_core::gui_state::{DraftTarget, GuiState};
use gpui_kit::component::{Theme, WindowExt as _};
use gpui_kit::{Action, AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    crate::test_support::isolate();
    cx.update(|cx| {
        crate::init(ConfigStore::from_config(Config::default()), cx);
        // Other tests share the isolated config directory: never restore
        // drafts they may have saved
        cx.set_global(RestoredState(GuiState::default()));
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    })
}

fn dispatch(cx: &mut TestAppContext, window: AnyWindowHandle, action: impl Action) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(action), cx)
    })
    .expect("dispatch");
}

fn tab_titles(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> Vec<String> {
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        workspace
            .tabs
            .iter()
            .map(|tab| tab.title(cx).to_string())
            .collect()
    })
}

fn has_dialog(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
    cx.update_window(window, |_, window, cx| window.has_active_dialog(cx))
        .expect("window")
}

fn notification_count(cx: &mut TestAppContext, window: AnyWindowHandle) -> usize {
    cx.update_window(window, |_, window, cx| window.notifications(cx).len())
        .expect("window")
}

#[gpui_kit::test]
fn starts_with_a_pinned_home_tab(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    assert_eq!(tab_titles(cx, &workspace), ["Connections"]);

    // The connection page is pinned: closing it does nothing
    dispatch(cx, window, act::CloseTab);
    assert_eq!(tab_titles(cx, &workspace), ["Connections"]);
}

#[gpui_kit::test]
fn new_query_needs_a_connection(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    dispatch(cx, window, act::NewQuery);

    // Stays on the connection page and explains why
    assert_eq!(tab_titles(cx, &workspace), ["Connections"]);
    assert_eq!(notification_count(cx, window), 1);
}

#[gpui_kit::test]
fn settings_tab_is_reused_and_closable(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    dispatch(cx, window, act::OpenSettings);
    dispatch(cx, window, act::OpenSettings);
    assert_eq!(tab_titles(cx, &workspace), ["Connections", "Settings"]);

    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_index(), 1);
    });

    // Closing returns to the tab used before
    dispatch(cx, window, act::CloseTab);
    assert_eq!(tab_titles(cx, &workspace), ["Connections"]);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_index(), 0));
}

#[gpui_kit::test]
fn tab_commands_cycle_through_tabs(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    dispatch(cx, window, act::OpenSettings);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_index(), 1));

    dispatch(cx, window, act::NextTab);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_index(), 0));
    dispatch(cx, window, act::PreviousTab);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_index(), 1));
    dispatch(cx, window, act::ShowHome);
    cx.update(|cx| assert_eq!(workspace.read(cx).active_index(), 0));
}

#[gpui_kit::test]
fn toggle_sidebar_and_theme(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(|cx| assert!(workspace.read(cx).sidebar_visible));
    dispatch(cx, window, act::ToggleSidebar);
    cx.update(|cx| assert!(!workspace.read(cx).sidebar_visible));
    dispatch(cx, window, act::ToggleSidebar);
    cx.update(|cx| assert!(workspace.read(cx).sidebar_visible));

    let before = cx.update(|cx| Theme::global(cx).is_dark());
    dispatch(cx, window, act::ToggleTheme);
    cx.update(|cx| assert_ne!(Theme::global(cx).is_dark(), before));
    dispatch(cx, window, act::ToggleTheme);
    cx.update(|cx| assert_eq!(Theme::global(cx).is_dark(), before));
}

#[gpui_kit::test]
fn palette_and_about_open_dialogs_that_escape_closes(cx: &mut TestAppContext) {
    let (window, _workspace) = open(cx);
    assert!(!has_dialog(cx, window));

    dispatch(cx, window, act::CommandPalette);
    assert!(has_dialog(cx, window));
    cx.update_window(window, |_, window, cx| {
        gpui_kit::test::TestWindowExt::press(window, "escape", cx)
    })
    .expect("press escape");
    assert!(!has_dialog(cx, window));

    dispatch(cx, window, act::About);
    assert!(has_dialog(cx, window));
}

#[gpui_kit::test]
fn query_tabs_are_saved_as_drafts_without_connection(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_query_tab(None, Some("Scratch".into()), "select 1".into(), window, cx);
            // Empty tabs are not worth restoring
            workspace.open_query_tab(None, None, String::new(), window, cx);
        });
    })
    .expect("window");

    cx.update(|cx| {
        let snapshot = workspace.read(cx).snapshot(cx);
        let drafts = snapshot.drafts();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].title, "Scratch");
        assert_eq!(drafts[0].sql, "select 1");
        assert_eq!(drafts[0].connection_key, None);
        assert!(snapshot.sidebar_visible());
    });
    // Default titles keep counting
    assert_eq!(
        tab_titles(cx, &workspace),
        ["Connections", "Scratch", "Query 1"]
    );
}

#[gpui_kit::test]
fn restored_drafts_reopen_as_unbound_tabs(cx: &mut TestAppContext) {
    crate::test_support::isolate();
    cx.update(|cx| {
        crate::init(ConfigStore::from_config(Config::default()), cx);
        cx.set_global(RestoredState(GuiState {
            sidebar_visible: false,
            drafts: vec![
                dbcrust_gui_core::gui_state::Draft {
                    title: "Query 4".into(),
                    sql: "select 4".into(),
                    connection_key: Some("sqlite:///tmp/shop.db".into()),
                    target: None,
                },
                dbcrust_gui_core::gui_state::Draft {
                    title: "Report".into(),
                    sql: "select 5".into(),
                    connection_key: None,
                    target: None,
                },
            ],
            ..GuiState::default()
        }));
    });
    let (window, workspace) = cx.update(|cx| {
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    });

    assert_eq!(
        tab_titles(cx, &workspace),
        ["Connections", "Query 4", "Report"]
    );
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert!(!workspace.sidebar_visible);
        // The connection page stays in front, and the next default title continues after "Query 4"
        assert_eq!(workspace.active_index(), 0);
        assert_eq!(workspace.query_counter, 4);
        assert_eq!(
            workspace.tabs[1].connection_key.as_deref(),
            Some("sqlite:///tmp/shop.db")
        );
        assert!(matches!(workspace.tabs[1].kind, TabKind::Query(_)));
    });

    // New default titles continue the numbering
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_query_tab(None, None, "select 6".into(), window, cx);
        });
    })
    .expect("window");
    assert_eq!(
        tab_titles(cx, &workspace),
        ["Connections", "Query 4", "Report", "Query 5"]
    );
}

#[gpui_kit::test]
fn restored_drafts_save_back_to_their_file(cx: &mut TestAppContext) {
    crate::test_support::isolate();
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("report.sql");
    std::fs::write(&file, "select 1").expect("write the file");
    let (missing_file, kept_file) = (dir.path().join("gone.sql"), file.clone());
    cx.update(|cx| {
        crate::init(ConfigStore::from_config(Config::default()), cx);
        cx.set_global(RestoredState(GuiState {
            drafts: vec![
                dbcrust_gui_core::gui_state::Draft {
                    title: "report.sql".into(),
                    sql: "select 2".into(),
                    connection_key: None,
                    target: Some(DraftTarget::File { path: kept_file }),
                },
                dbcrust_gui_core::gui_state::Draft {
                    title: "gone.sql".into(),
                    sql: "select 3".into(),
                    connection_key: None,
                    target: Some(DraftTarget::File { path: missing_file }),
                },
            ],
            ..GuiState::default()
        }));
    });
    let (_window, workspace) = cx.update(|cx| {
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    });
    cx.run_until_parked();

    cx.update(|cx| {
        let workspace = workspace.read(cx);
        let query = |index: usize| match &workspace.tabs[index].kind {
            TabKind::Query(query) => query.read(cx),
            _ => panic!("tab {index} is not a query tab"),
        };
        // Bound to its file again, and modified: the draft differs from it
        assert_eq!(query(1).file_path(), Some(file.as_path()));
        assert!(query(1).is_modified());
        assert_eq!(
            query(1).draft_target(),
            Some(DraftTarget::File { path: file.clone() })
        );
        // A file that is gone leaves a plain draft
        assert_eq!(query(2).draft_target(), None);
        assert!(!query(2).is_modified());
    });
}

#[gpui_kit::test]
fn a_late_file_restore_does_not_take_back_a_newer_target(cx: &mut TestAppContext) {
    crate::test_support::isolate();
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("old.sql");
    std::fs::write(&file, "select 1").expect("write the file");
    let store = ConfigStore::from_config(Config::default());
    store
        .save_named_query("daily", "select 2", None)
        .expect("save a query");
    cx.update(|cx| {
        crate::init(store, cx);
        cx.set_global(RestoredState(GuiState {
            drafts: vec![dbcrust_gui_core::gui_state::Draft {
                title: "old.sql".into(),
                sql: "select 2".into(),
                connection_key: None,
                target: Some(DraftTarget::File { path: file }),
            }],
            ..GuiState::default()
        }));
    });
    let (window, workspace) = cx.update(|cx| {
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    });
    // Bound to a saved query before the file has been read
    cx.update_window(window, |_, window, cx| {
        let query = match &workspace.read(cx).tabs[1].kind {
            TabKind::Query(query) => query.clone(),
            _ => panic!("tab 1 is not a query tab"),
        };
        query.update(cx, |query, cx| {
            let target = DraftTarget::SavedQuery {
                name: "daily".into(),
                engine: None,
            };
            query.restore_target(target, window, cx);
        });
    })
    .expect("window");
    cx.run_until_parked();

    cx.update(|cx| {
        let TabKind::Query(query) = &workspace.read(cx).tabs[1].kind else {
            panic!("tab 1 is not a query tab");
        };
        assert_eq!(
            query.read(cx).draft_target(),
            Some(DraftTarget::SavedQuery {
                name: "daily".into(),
                engine: None
            })
        );
    });
}

fn open_with(cx: &mut TestAppContext, store: ConfigStore) -> (AnyWindowHandle, Entity<Workspace>) {
    crate::test_support::isolate();
    cx.executor().allow_parking();
    cx.update(|cx| {
        crate::init(store, cx);
        cx.set_global(RestoredState(GuiState::default()));
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    })
}

#[gpui_kit::test]
fn a_stalled_settings_write_cannot_hang_the_quit(cx: &mut TestAppContext) {
    // The config.toml write stalls until the end of the test
    let (release, stalled) = std::sync::mpsc::channel::<()>();
    let stalled = std::sync::Mutex::new(stalled);
    let store =
        ConfigStore::from_config(Config::default_settings()).with_settings_writer(move |_| {
            let _ = stalled
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(60));
            Ok(())
        });
    let (_window, workspace) = open_with(cx, store.clone());
    store.update_settings(|config| config.default_limit = 5);
    let started = std::time::Instant::now();
    cx.update(|cx| cx.dispatch_action(&act::Quit));
    cx.run_until_parked();
    // The deadline (2 s) covers the settings write: the user is asked
    // instead of the quit hanging on it, or quitting without it
    let waited = started.elapsed();
    drop(release);
    assert!(
        waited < std::time::Duration::from_secs(10),
        "the quit waited {waited:?} for the stalled write"
    );
    cx.update(|cx| assert!(!workspace.read(cx).quitting.get()));
}

#[gpui_kit::test]
fn a_setting_changed_during_the_final_save_is_saved_before_quitting(cx: &mut TestAppContext) {
    let cell: std::sync::Arc<std::sync::OnceLock<ConfigStore>> = Default::default();
    let written: std::sync::Arc<std::sync::Mutex<Vec<usize>>> = Default::default();
    let (edit_from, record) = (cell.clone(), written.clone());
    let store = ConfigStore::from_config(Config::default_settings()).with_settings_writer(
        move |snapshot| {
            let mut written = record.lock().unwrap();
            written.push(snapshot.default_limit);
            if written.len() == 1 {
                // The Settings page stays usable while the app quits
                edit_from
                    .get()
                    .unwrap()
                    .update_settings(|config| config.default_limit = 8);
            }
            Ok(())
        },
    );
    cell.set(store.clone()).ok().unwrap();
    let (_window, workspace) = open_with(cx, store.clone());
    store.update_settings(|config| config.default_limit = 7);
    cx.update(|cx| cx.dispatch_action(&act::Quit));
    cx.run_until_parked();
    cx.update(|cx| assert!(workspace.read(cx).quitting.get()));
    assert!(
        !store.has_unsaved_settings(),
        "quit before the last change was saved"
    );
    assert_eq!(written.lock().unwrap().last(), Some(&8));
}

#[gpui_kit::test]
fn quit_never_finishes_with_a_setting_changed_after_every_save(cx: &mut TestAppContext) {
    let saves = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = saves.clone();
    let store =
        ConfigStore::from_config(Config::default_settings()).with_settings_writer(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
    let (_window, workspace) = open_with(cx, store.clone());
    cx.run_until_parked();
    store.update_settings(|config| config.default_limit = 1);
    cx.update(|cx| cx.dispatch_action(&act::Quit));
    for attempt in 1..=3 {
        // Run tasks until this attempt's flush (its settings save included)
        // is over, but not the check that follows it on the UI thread
        while saves.load(std::sync::atomic::Ordering::SeqCst) < attempt {
            assert!(cx.executor().tick(), "attempt {attempt} never saved");
        }
        // The user changes a setting while the quit finishes saving
        store.update_settings(|config| config.default_limit = attempt + 1);
    }
    cx.run_until_parked();
    // Out of retries with a change still unsaved: ask, never quit
    cx.update(|cx| assert!(!workspace.read(cx).quitting.get()));
    assert!(store.has_unsaved_settings());
    assert_eq!(saves.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[gpui_kit::test]
fn quit_without_running_queries_quits_immediately(cx: &mut TestAppContext) {
    let (_window, workspace) = open(cx);
    cx.update(|cx| {
        cx.dispatch_action(&act::Quit);
    });
    // The final save is awaited first, then the app quits
    cx.run_until_parked();
    cx.update(|cx| assert!(workspace.read(cx).quitting.get()));
}

fn open_query(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
    title: &str,
    sql: &str,
) {
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_query_tab(None, Some(title.into()), sql.into(), window, cx);
        });
    })
    .expect("window");
}

#[gpui_kit::test]
fn a_closed_query_tab_can_be_reopened_with_its_draft(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Report", "select 1");
    open_query(cx, window, &workspace, "Scratch", "select 2");
    open_query(cx, window, &workspace, "Last", "select 3");

    // Close "Scratch" (the middle one): the notice names the way back
    let scratch = cx.update(|cx| workspace.read(cx).tabs[2].id);
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.close_tab(scratch, window, cx));
    })
    .expect("window");
    assert_eq!(
        tab_titles(cx, &workspace),
        ["Connections", "Report", "Last"]
    );
    assert_eq!(notification_count(cx, window), 1);

    dispatch(cx, window, act::ReopenClosedTab);
    // Back where it was, in front, with its text
    assert_eq!(
        tab_titles(cx, &workspace),
        ["Connections", "Report", "Scratch", "Last"]
    );
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.active_index(), 2);
        let query = workspace.tabs[2].as_query().expect("a query tab");
        assert_eq!(query.read(cx).sql(cx), "select 2");
        assert!(workspace.closed_tabs.is_empty());
    });
}

#[gpui_kit::test]
fn blank_tabs_are_not_kept_and_the_stack_is_bounded(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Blank", "  \n");
    dispatch(cx, window, act::CloseTab);
    cx.update(|cx| assert!(workspace.read(cx).closed_tabs.is_empty()));
    assert_eq!(notification_count(cx, window), 0);

    // Nothing to reopen: says so, opens nothing
    dispatch(cx, window, act::ReopenClosedTab);
    assert_eq!(tab_titles(cx, &workspace), ["Connections"]);
    assert_eq!(notification_count(cx, window), 1);

    for n in 0..25 {
        open_query(cx, window, &workspace, &format!("Tab {n}"), "select 1");
        dispatch(cx, window, act::CloseTab);
    }
    cx.update(|cx| {
        let closed = &workspace.read(cx).closed_tabs;
        assert_eq!(closed.len(), super::tabs::CLOSED_TABS_KEPT);
        // The oldest were forgotten, the newest comes back first
        assert_eq!(closed.first().map(|tab| tab.title.as_str()), Some("Tab 5"));
        assert_eq!(closed.last().map(|tab| tab.title.as_str()), Some("Tab 24"));
    });
    dispatch(cx, window, act::ReopenClosedTab);
    assert_eq!(tab_titles(cx, &workspace), ["Connections", "Tab 24"]);
}

// ----- Tab switching and renaming -----

use super::tabs::{TabBinding, TabConnection};
use dbcrust_gui_core::DatabaseType;
use gpui_kit::test::TestWindowExt as _;
use rstest::rstest;

fn active_index(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> usize {
    cx.update(|cx| workspace.read(cx).active_index())
}

fn render(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .expect("window");
}

fn press(cx: &mut TestAppContext, window: AnyWindowHandle, key: &'static str) {
    cx.update_window(window, |_, window, cx| window.press(key, cx))
        .expect("press");
}

#[test]
fn every_number_has_one_command_bound_to_command_number() {
    use crate::commands::AppCommand;
    use strum::IntoEnumIterator;

    let numbered: Vec<(usize, AppCommand)> = AppCommand::iter()
        .filter_map(|command| command.tab_number().map(|number| (number, command)))
        .collect();
    assert_eq!(
        numbered
            .iter()
            .map(|(number, _)| *number)
            .collect::<Vec<_>>(),
        (1..=9).collect::<Vec<_>>()
    );
    for (number, command) in numbered {
        assert_eq!(command.shortcuts(), [format!("secondary-{number}")]);
        // Nine rows of "Go to tab N" would bury the palette
        assert!(!command.in_palette());
    }
}

#[gpui_kit::test]
fn command_n_switches_to_the_nth_tab_and_9_to_the_last(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    for title in ["A", "B", "C"] {
        open_query(cx, window, &workspace, title, "select 1");
    }
    assert_eq!(active_index(cx, &workspace), 3);

    dispatch(cx, window, act::GoToTab2);
    assert_eq!(active_index(cx, &workspace), 1);
    dispatch(cx, window, act::GoToTab4);
    assert_eq!(active_index(cx, &workspace), 3);
    dispatch(cx, window, act::GoToTab1);
    assert_eq!(active_index(cx, &workspace), 0);
    // Past the end: nothing happens
    dispatch(cx, window, act::GoToTab7);
    assert_eq!(active_index(cx, &workspace), 0);
    dispatch(cx, window, act::GoToTab9);
    assert_eq!(active_index(cx, &workspace), 3);

    // The real key binding reaches the workspace
    press(cx, window, "secondary-2");
    assert_eq!(active_index(cx, &workspace), 1);
    press(cx, window, "ctrl-tab");
    assert_eq!(active_index(cx, &workspace), 2);
    press(cx, window, "ctrl-shift-tab");
    assert_eq!(active_index(cx, &workspace), 1);
}

#[gpui_kit::test]
fn go_to_tab_opens_a_palette_of_tabs(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Report", "select 1");
    dispatch(cx, window, act::SwitchTab);
    assert!(has_dialog(cx, window));
    press(cx, window, "escape");
    assert!(!has_dialog(cx, window));
    assert_eq!(active_index(cx, &workspace), 1);
}

/// The inline editor of the tab in front.
fn rename_input(
    cx: &mut TestAppContext,
    workspace: &Entity<Workspace>,
) -> Option<Entity<gpui_kit::component::input::InputState>> {
    cx.update(|cx| {
        workspace
            .read(cx)
            .renaming
            .as_ref()
            .map(|rename| rename.input.clone())
    })
}

fn type_title(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
    text: &str,
) {
    let input = rename_input(cx, workspace).expect("the title editor is open");
    cx.update_window(window, |_, window, cx| {
        input.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
    })
    .expect("window");
}

#[gpui_kit::test]
fn rename_tab_edits_in_place_and_enter_commits(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Query 1", "select 1");
    // Connections has no title to edit
    dispatch(cx, window, act::ShowHome);
    dispatch(cx, window, act::RenameTab);
    assert!(rename_input(cx, &workspace).is_none());
    dispatch(cx, window, act::GoToTab2);

    dispatch(cx, window, act::RenameTab);
    cx.run_until_parked();
    render(cx, window);
    assert!(rename_input(cx, &workspace).is_some());

    type_title(cx, window, &workspace, "  Monthly revenue ");
    render(cx, window);
    press(cx, window, "enter");
    assert!(rename_input(cx, &workspace).is_none());
    assert_eq!(
        tab_titles(cx, &workspace),
        ["Connections", "Monthly revenue"]
    );
    // The editor text and nothing else changed
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        let query = workspace.tabs[1].as_query().expect("a query tab");
        assert_eq!(query.read(cx).sql(cx), "select 1");
        assert_eq!(query.read(cx).draft_target(), None);
    });
}

#[gpui_kit::test]
fn escape_cancels_and_an_empty_title_restores_the_previous_one(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Report", "select 1");

    dispatch(cx, window, act::RenameTab);
    cx.run_until_parked();
    render(cx, window);
    type_title(cx, window, &workspace, "Discarded");
    render(cx, window);
    press(cx, window, "escape");
    assert!(rename_input(cx, &workspace).is_none());
    assert_eq!(tab_titles(cx, &workspace), ["Connections", "Report"]);

    dispatch(cx, window, act::RenameTab);
    cx.run_until_parked();
    render(cx, window);
    type_title(cx, window, &workspace, "   ");
    render(cx, window);
    press(cx, window, "enter");
    assert_eq!(tab_titles(cx, &workspace), ["Connections", "Report"]);
}

#[gpui_kit::test]
fn leaving_the_editor_commits_the_title(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    // Focus changes only blur the field in an active window
    cx.update_window(window, |_, window, _| window.activate_window())
        .expect("window");
    open_query(cx, window, &workspace, "Report", "select 1");
    dispatch(cx, window, act::RenameTab);
    cx.run_until_parked();
    render(cx, window);
    type_title(cx, window, &workspace, "Blurred");
    render(cx, window);

    // Moving to another tab takes the focus away from the field
    dispatch(cx, window, act::GoToTab1);
    cx.run_until_parked();
    render(cx, window);
    assert!(rename_input(cx, &workspace).is_none());
    assert_eq!(tab_titles(cx, &workspace), ["Connections", "Blurred"]);
}

#[gpui_kit::test]
fn closing_a_tab_drops_its_title_editor(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Report", "select 1");
    dispatch(cx, window, act::RenameTab);
    cx.run_until_parked();
    assert!(rename_input(cx, &workspace).is_some());
    dispatch(cx, window, act::CloseTab);
    assert!(rename_input(cx, &workspace).is_none());
}

#[gpui_kit::test]
fn a_renamed_tab_keeps_its_title_in_the_draft(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Query 1", "select 1");
    dispatch(cx, window, act::RenameTab);
    cx.run_until_parked();
    render(cx, window);
    type_title(cx, window, &workspace, "Monthly revenue");
    render(cx, window);
    press(cx, window, "enter");

    let drafts = cx.update(|cx| workspace.read(cx).snapshot(cx).drafts().to_vec());
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].title, "Monthly revenue");

    // …and comes back with it after a restart
    crate::test_support::isolate();
    let (_, restored) = open_with_drafts(cx, drafts);
    assert_eq!(
        tab_titles(cx, &restored),
        ["Connections", "Monthly revenue"]
    );
}

fn open_with_drafts(
    cx: &mut TestAppContext,
    drafts: Vec<dbcrust_gui_core::gui_state::Draft>,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(|cx| {
        cx.set_global(RestoredState(GuiState {
            drafts,
            ..GuiState::default()
        }));
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    })
}

#[rstest]
#[case(TabBinding::None, "Settings")]
#[case(TabBinding::Detached, "Report · no connection")]
#[case(
    TabBinding::Live(TabConnection {
        name: "shop".into(),
        engine: "SQLite",
        db_type: DatabaseType::SQLite,
        accent: 0,
    }),
    "Report · shop (SQLite)"
)]
fn tabs_are_described_with_their_connection(#[case] binding: TabBinding, #[case] expected: &str) {
    let title = if binding == TabBinding::None {
        "Settings"
    } else {
        "Report"
    };
    assert_eq!(binding.describe(title), expected);
}
