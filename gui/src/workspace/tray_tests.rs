//! Closing the main window: hide in the tray, ask the first time, quit when
//! there is no way back. The native window is faked (`tray::native::fake`):
//! what is checked is the decision, the dialog, and that nothing of the
//! workspace is lost while hidden.

use super::Workspace;
use crate::RestoredState;
use crate::preferences::Preferences;
use crate::tray;
use dbcrust_gui_core::ConfigStore;
use dbcrust_gui_core::dbcrust::config::Config;
use dbcrust_gui_core::gui_state::{Draft, GuiState};
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use std::time::Duration;

fn open_with(cx: &mut TestAppContext, restored: GuiState) -> (AnyWindowHandle, Entity<Workspace>) {
    crate::test_support::isolate();
    tray::test_reset_window();
    cx.update(|cx| {
        crate::init(ConfigStore::from_config(Config::default()), cx);
        // The preferences of the machine running the tests never matter
        cx.set_global(Preferences::default());
        cx.set_global(RestoredState(restored));
        crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
    })
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    open_with(cx, GuiState::default())
}

/// The window's close button. `true`: the window may close.
fn close_button(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
) -> bool {
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.should_close(window, cx))
    })
    .expect("window")
}

fn has_dialog(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
    cx.update_window(window, |_, window, cx| window.has_active_dialog(cx))
        .expect("window")
}

fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) {
    // A dialog ignores the pointer until its entrance animation is over
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .expect("window");
    cx.run_until_parked();
}

fn tab_titles(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> Vec<String> {
    cx.update(|cx| {
        workspace
            .read(cx)
            .tabs
            .iter()
            .map(|tab| tab.title(cx).to_string())
            .collect()
    })
}

/// The tab titles after the pinned first tab (whatever it is called).
fn tabs_after_the_first(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> Vec<String> {
    tab_titles(cx, workspace).split_off(1)
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

fn quitting(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> bool {
    cx.update(|cx| workspace.read(cx).quitting.get())
}

fn hidden(cx: &mut TestAppContext) -> bool {
    cx.update(|cx| tray::window_hidden(cx))
}

#[gpui_kit::test]
fn closing_without_a_tray_quits(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    // The window stays while the final save runs, then the app quits
    assert!(!close_button(cx, window, &workspace));
    cx.run_until_parked();
    assert!(quitting(cx, &workspace));
    assert!(tray::test_window_visible());
    assert!(!hidden(cx));
}

#[gpui_kit::test]
fn closing_quits_when_the_user_turned_background_mode_off(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(|cx| {
        tray::pretend_active(cx);
        Preferences::update(cx, |p| p.keep_running_in_tray = false);
    });
    assert!(!close_button(cx, window, &workspace));
    cx.run_until_parked();
    assert!(quitting(cx, &workspace));
    assert!(tray::test_window_visible());
}

#[gpui_kit::test]
fn the_first_close_asks_and_escape_changes_nothing(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(tray::pretend_active);

    assert!(!close_button(cx, window, &workspace));
    assert!(has_dialog(cx, window), "explains before hiding");
    assert!(tray::test_window_visible());
    assert!(!hidden(cx));
    cx.update(|cx| assert!(!Preferences::get(cx).tray_notice_shown));

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .expect("window");
    cx.run_until_parked();
    assert!(!has_dialog(cx, window));
    assert!(tray::test_window_visible());
    assert!(!quitting(cx, &workspace));
    cx.update(|cx| {
        assert!(
            !Preferences::get(cx).tray_notice_shown,
            "asked again next time"
        )
    });
}

#[gpui_kit::test]
fn keep_running_hides_the_window_and_it_comes_back_as_it_was(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(tray::pretend_active);
    open_query(cx, window, &workspace, "Report", "select 1");
    open_query(cx, window, &workspace, "Scratch", "select 2");
    let before = tab_titles(cx, &workspace);

    assert!(!close_button(cx, window, &workspace));
    click(cx, window, "tray-keep");

    assert!(!tray::test_window_visible());
    assert!(hidden(cx));
    assert!(!has_dialog(cx, window));
    assert!(!quitting(cx, &workspace));
    cx.update(|cx| assert!(Preferences::get(cx).tray_notice_shown, "answered once"));
    // The same workspace: tabs, text, active tab
    assert_eq!(tab_titles(cx, &workspace), before);
    cx.update(|cx| {
        let workspace = workspace.read(cx);
        let query = workspace.tabs[2].as_query().expect("a query tab");
        assert_eq!(query.read(cx).sql(cx), "select 2");
        assert_eq!(workspace.active_index(), 2);
    });

    // Open DBCrust / Dock click / relaunch
    cx.update(tray::show_main_window);
    assert!(tray::test_window_visible());
    assert!(!hidden(cx));
    assert_eq!(tab_titles(cx, &workspace), before);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1, "no second window");
}

#[gpui_kit::test]
fn after_the_first_answer_closing_hides_without_asking(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(|cx| {
        tray::pretend_active(cx);
        Preferences::update(cx, |p| p.tray_notice_shown = true);
    });
    assert!(!close_button(cx, window, &workspace));
    assert!(!has_dialog(cx, window));
    assert!(!tray::test_window_visible());
    assert!(hidden(cx));
    assert!(!quitting(cx, &workspace));
}

#[gpui_kit::test]
fn quit_in_the_first_close_dialog_quits(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(tray::pretend_active);
    assert!(!close_button(cx, window, &workspace));
    click(cx, window, "tray-quit");
    cx.run_until_parked();
    assert!(quitting(cx, &workspace));
    assert!(tray::test_window_visible());
    cx.update(|cx| assert!(Preferences::get(cx).tray_notice_shown));
}

#[gpui_kit::test]
fn a_destroyed_window_is_reopened_from_the_tray(cx: &mut TestAppContext) {
    let (window, _workspace) = open(cx);
    cx.update(tray::pretend_active);
    cx.update_window(window, |_, window, _| window.remove_window())
        .expect("window");
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);

    cx.update(tray::show_main_window);
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
}

#[gpui_kit::test]
fn command_q_goes_through_the_quit_coordinator(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press("secondary-q", cx);
    })
    .expect("window");
    cx.run_until_parked();
    assert!(quitting(cx, &workspace), "⌘Q ends in `finish_quit`");
}

#[gpui_kit::test]
fn drafts_are_restored_only_when_asked(cx: &mut TestAppContext) {
    let saved = |restore_drafts| GuiState {
        restore_drafts,
        drafts: vec![Draft {
            title: "Old".into(),
            sql: "select 1".into(),
            connection_key: None,
            target: None,
        }],
        ..GuiState::default()
    };
    let (_, workspace) = open_with(cx, saved(true));
    assert_eq!(tabs_after_the_first(cx, &workspace), ["Old"]);

    let (_, workspace) = open_with(cx, saved(false));
    assert!(tabs_after_the_first(cx, &workspace).is_empty());
}

#[gpui_kit::test]
fn no_query_text_is_saved_when_restoring_is_off(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    open_query(cx, window, &workspace, "Report", "select secret");
    cx.update(|cx| assert_eq!(workspace.read(cx).snapshot(cx).drafts().len(), 1));

    cx.update(|cx| Preferences::update(cx, |p| p.restore_drafts = false));
    cx.update(|cx| assert!(workspace.read(cx).snapshot(cx).drafts().is_empty()));
}

#[gpui_kit::test]
fn tray_commands_reach_the_workspace(cx: &mut TestAppContext) {
    let (_window, workspace) = open(cx);
    cx.update(tray::pretend_active);

    // "Settings…" shows the window and opens (or reuses) the Settings tab
    cx.update(|cx| tray::perform(tray::TrayCommand::Settings, cx));
    cx.update(|cx| tray::perform(tray::TrayCommand::Settings, cx));
    cx.run_until_parked();
    assert_eq!(tabs_after_the_first(cx, &workspace), ["Settings"]);

    // "New query" without a connection brings Home forward instead
    cx.update(|cx| tray::perform(tray::TrayCommand::NewQuery, cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(workspace.read(cx).active_index(), 0));
}

#[gpui_kit::test]
fn a_second_close_click_keeps_the_window_while_the_question_is_open(cx: &mut TestAppContext) {
    let (window, workspace) = open(cx);
    cx.update(tray::pretend_active);

    assert!(!close_button(cx, window, &workspace));
    assert!(has_dialog(cx, window));
    // Impatient clicks on the close button: nothing hides under the dialog
    assert!(!close_button(cx, window, &workspace));
    assert!(!close_button(cx, window, &workspace));
    assert!(tray::test_window_visible());
    assert!(!hidden(cx));
    assert!(has_dialog(cx, window));
    cx.update(|cx| assert!(!Preferences::get(cx).tray_notice_shown));
    assert!(!quitting(cx, &workspace));

    // Answering it still works afterwards
    click(cx, window, "tray-keep");
    assert!(!tray::test_window_visible());
    assert!(hidden(cx));
}
