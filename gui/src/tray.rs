//! Staying in the menu bar / notification area while the window is closed.
//!
//! - `tray/model.rs`: the pure half — menu content, command ids, the
//!   close-button decision, wording. Unit-tested without a tray.
//! - `tray/system.rs`: the OS icon (`tray-icon`, macOS and Windows only).
//! - `tray/native.rs`: hiding / showing the main window through its raw
//!   handle.
//! - this file: the glue — the icon's lifetime, rebuilding its menu when the
//!   app state changes, running its commands, and hiding / revealing the
//!   main window.
//!
//! How it fits together:
//!
//! - The window's close button asks [`close_action`]. [`CloseAction::Hide`]
//!   (setting on, tray icon alive, platform able to hide a window) saves the
//!   drafts and hides the window instead of destroying it, so tabs, editors,
//!   results, running queries and connections all stay as they are.
//!   Otherwise closing quits, through the one quit coordinator.
//! - [`show_main_window`] undoes it: from the tray, the Dock (reopen), or any
//!   tray entry that needs the window. If the window is gone anyway it opens
//!   a new one that restores the drafts from `gui_state.toml`.
//! - The tray menu is rebuilt only when its content (a [`TraySnapshot`])
//!   changes, a moment after the app state or a query's status changed.
//!   Native callbacks never touch GPUI: they push into a channel that a
//!   foreground task drains.
//! - If the icon cannot be created (or on Linux, where GPUI cannot hide a
//!   window and `tray-icon` needs a GTK main loop) closing the window quits,
//!   as before.

mod model;
mod native;
mod system;

pub use model::{CloseAction, MAX_RECENTS, MAX_SESSIONS, TrayCommand, Wording};

use crate::MainWindow;
use crate::RestoredState;
use crate::actions as act;
use crate::app_state::AppState;
use crate::preferences::Preferences;
use dbcrust_gui_core::gui_state::GuiState;
use dbcrust_gui_core::{ConnectRequest, ConnectionId};
use gpui_kit::*;
use model::{TrayConnection, TrayRecent, TraySnapshot, build_menu, recent_key, status_text};
use std::cell::{Cell, RefCell};
use std::time::Duration;
use system::{SystemTray, TrayEvent};

/// How long state changes are collected before the menu is compared and,
/// when it differs, rebuilt.
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(150);

/// This build has a tray implementation (macOS and Windows).
pub const SUPPORTED: bool = system::SUPPORTED;

/// Whether closing to the tray can work on this machine, for the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    /// No tray in this build (Linux).
    Unsupported,
    /// The icon exists or has not been needed yet.
    Available,
    /// The system refused the icon; closing quits.
    Failed(String),
}

enum TrayState {
    Active(Tray),
    Failed(String),
}

impl Global for TrayState {}

/// A tray icon exists (set by [`install`]; tests set it directly, since they
/// have no system tray).
struct TrayReady;

impl Global for TrayReady {}

struct Tray {
    system: SystemTray,
    /// The content of the menu on screen.
    shown: RefCell<Option<TraySnapshot>>,
    refresh_pending: Cell<bool>,
    _app_state: Subscription,
    _events: Task<()>,
}

/// Where the main window is right now.
#[derive(Default)]
struct WindowPresence {
    /// Hidden by the close button (not minimized, not destroyed).
    hidden: bool,
    /// macOS: the Dock icon was removed while hidden.
    dock_icon_hidden: bool,
}

impl Global for WindowPresence {}

// ----- Setup -----

/// Register the app-level commands that must work while the window is hidden
/// (the Dock menu's entries). Call from [`crate::init`].
pub fn init(cx: &mut App) {
    // The window's own handlers take these first; they only get here when no
    // window handles them, e.g. from the Dock menu while the window is hidden
    cx.on_action(|_: &act::NewQuery, cx| perform(TrayCommand::NewQuery, cx));
    cx.on_action(|_: &act::NewConnection, cx| perform(TrayCommand::NewConnection, cx));
    cx.on_action(|_: &act::OpenSettings, cx| perform(TrayCommand::Settings, cx));
}

/// Create the tray icon. Call on the main thread from the `Application::run`
/// callback, after the main window opened (the event loop is running by
/// then). A failure is logged and remembered; the app then behaves as if the
/// tray did not exist.
pub fn install(cx: &mut App) {
    if !SUPPORTED || cx.has_global::<TrayState>() {
        return;
    }
    let snapshot = snapshot(cx, None);
    let (sender, receiver) = async_channel::unbounded();
    let system = match SystemTray::create(sender, &build_menu(&snapshot)) {
        Ok(system) => system,
        Err(error) => {
            tracing::warn!("no tray icon: {error}");
            cx.set_global(TrayState::Failed(error));
            return;
        }
    };
    system.set_tooltip(&tooltip(&snapshot));

    let events = cx.spawn(async move |cx| {
        while let Ok(event) = receiver.recv().await {
            cx.update(|cx| handle_event(event, cx));
        }
    });
    let app_state = AppState::global(cx);
    let observer = cx.observe(&app_state, |_, cx| state_changed(cx));
    cx.set_global(TrayState::Active(Tray {
        system,
        shown: RefCell::new(Some(snapshot)),
        refresh_pending: Cell::new(false),
        _app_state: observer,
        _events: events,
    }));
    cx.set_global(TrayReady);
}

/// A tray icon exists, so the window can be hidden without losing the app.
pub fn is_active(cx: &App) -> bool {
    cx.has_global::<TrayReady>()
}

/// Whether the (fake) native main window is shown.
#[cfg(test)]
pub(crate) fn test_window_visible() -> bool {
    native::fake::is_visible()
}

/// Make the (fake) native main window visible again.
#[cfg(test)]
pub(crate) fn test_reset_window() {
    native::fake::set_visible(true);
}

/// Behave as if a tray icon had been created.
#[cfg(test)]
pub(crate) fn pretend_active(cx: &mut App) {
    cx.set_global(TrayReady);
}

pub fn availability(cx: &App) -> Availability {
    if !SUPPORTED {
        Availability::Unsupported
    } else if let Some(TrayState::Failed(error)) = cx.try_global::<TrayState>() {
        Availability::Failed(error.clone())
    } else {
        Availability::Available
    }
}

/// What the window's close button does now.
pub fn close_action(cx: &App) -> CloseAction {
    model::close_action(
        Preferences::get(cx).keep_running_in_tray,
        is_active(cx),
        native::CAN_HIDE,
    )
}

/// The app would keep running without a window.
pub(crate) fn keeps_running(cx: &App) -> bool {
    close_action(cx) == CloseAction::Hide
}

// ----- The main window: hide and show -----

/// The main window is hidden in the tray.
pub fn window_hidden(cx: &App) -> bool {
    cx.try_global::<WindowPresence>()
        .is_some_and(|presence| presence.hidden)
}

/// Hide the main window (and, on macOS with the setting on, the Dock icon).
/// Returns `false` when the platform refused; the caller must then do
/// something else with the close click.
pub(crate) fn hide_main_window(window: &Window, cx: &mut App) -> bool {
    if !native::hide(window) {
        tracing::warn!("could not hide the main window; closing it instead");
        return false;
    }
    let hide_dock_icon =
        cfg!(target_os = "macos") && Preferences::get(cx).hide_dock_icon_when_closed;
    if hide_dock_icon {
        cx.set_activation_policy(ActivationPolicy::Accessory);
    }
    let presence = cx.default_global::<WindowPresence>();
    presence.hidden = true;
    presence.dock_icon_hidden = hide_dock_icon;
    true
}

/// Bring `window` back in front: the Dock icon returns, the app activates,
/// the window un-hides and takes focus.
pub(crate) fn reveal_window(window: &Window, cx: &mut App) {
    let presence = cx.default_global::<WindowPresence>();
    presence.hidden = false;
    if std::mem::take(&mut presence.dock_icon_hidden) {
        cx.set_activation_policy(ActivationPolicy::Regular);
    }
    cx.activate(true);
    native::show(window);
    window.activate_window();
}

/// Show the main window: from the tray, the Dock, a second launch. Opens a
/// new one (restoring the drafts from disk) if the window no longer exists.
pub fn show_main_window(cx: &mut App) {
    show_main_window_with(true, cx);
}

fn show_main_window_with(may_retry: bool, cx: &mut App) {
    let Some(main) = cx.try_global::<MainWindow>().cloned() else {
        return;
    };
    let shown = main
        .handle
        .update(cx, |_, window, cx| reveal_window(window, cx));
    if shown.is_ok() {
        return;
    }
    if cx.windows().contains(&main.handle) {
        // Busy: this runs inside one of the window's own updates (a global
        // action handler). Try again once it is over
        if may_retry {
            cx.defer(|cx| show_main_window_with(false, cx));
        }
    } else {
        reopen_main_window(cx);
    }
}

fn reopen_main_window(cx: &mut App) {
    // What the closed window last saved, not the state of the launch
    cx.set_global(RestoredState(GuiState::load()));
    let options = crate::main_window_options(cx);
    match crate::open_main_window(options, cx) {
        Ok((handle, _)) => {
            handle
                .update(cx, |_, window, cx| reveal_window(window, cx))
                .ok();
        }
        Err(error) => tracing::error!("could not reopen the DBCrust window: {error}"),
    }
}

// ----- The menu -----

/// Something the menu shows may have changed: the app state, a query's
/// status. The menu is compared and rebuilt shortly after, only if its
/// content differs.
pub fn state_changed(cx: &mut App) {
    let Some(TrayState::Active(tray)) = cx.try_global::<TrayState>() else {
        return;
    };
    if tray.refresh_pending.replace(true) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(REFRESH_DEBOUNCE).await;
        cx.update(refresh_menu);
    })
    .detach();
}

fn refresh_menu(cx: &mut App) {
    let Some(TrayState::Active(tray)) = cx.try_global::<TrayState>() else {
        return;
    };
    tray.refresh_pending.set(false);
    let snapshot = snapshot(cx, tray.shown.borrow().as_ref());
    if tray.shown.borrow().as_ref() == Some(&snapshot) {
        return;
    }
    tray.system.set_menu(&build_menu(&snapshot));
    tray.system.set_tooltip(&tooltip(&snapshot));
    *tray.shown.borrow_mut() = Some(snapshot);
}

fn tooltip(snapshot: &TraySnapshot) -> String {
    format!("DBCrust — {}", status_text(snapshot))
}

/// Copy what the menu shows out of the app. `previous` stands in for the
/// saved and recent connections while the store is busy writing (the UI
/// thread never waits for it; the write ends with a storage change anyway).
fn snapshot(cx: &App, previous: Option<&TraySnapshot>) -> TraySnapshot {
    let app_state = AppState::global(cx);
    let state = app_state.read(cx);
    let (sessions, recents) = state
        .store()
        .try_read(|config| {
            let sessions = config
                .list_sessions()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            let recents = config
                .get_recent_connections()
                .iter()
                .map(|recent| TrayRecent {
                    key: recent_key(&recent.connection_url),
                    label: recent.display_name.clone(),
                })
                .collect();
            (sessions, recents)
        })
        .unwrap_or_else(|| {
            previous
                .map(|previous| (previous.sessions.clone(), previous.recents.clone()))
                .unwrap_or_default()
        });
    TraySnapshot {
        connections: state
            .connections()
            .iter()
            .map(|live| TrayConnection {
                id: live.id().as_u64(),
                label: live.summary.label.clone(),
                engine: live.summary.type_name().to_string(),
                read_only: live.summary.read_only,
            })
            .collect(),
        running_queries: running_queries(cx),
        connecting: state.is_connecting(),
        sessions,
        recents,
    }
}

fn running_queries(cx: &App) -> usize {
    cx.try_global::<MainWindow>()
        .and_then(|main| main.workspace.upgrade())
        .map_or(0, |workspace| workspace.read(cx).running_queries(cx))
}

// ----- Commands -----

fn handle_event(event: TrayEvent, cx: &mut App) {
    match event {
        TrayEvent::Menu(command) => perform(command, cx),
        TrayEvent::IconClicked => show_main_window(cx),
    }
}

fn live_connection(id: u64, cx: &App) -> Option<ConnectionId> {
    AppState::global(cx)
        .read(cx)
        .connections()
        .iter()
        .map(|live| live.id())
        .find(|live| live.as_u64() == id)
}

fn connect(request: ConnectRequest, cx: &mut App) {
    // Failures surface through `AppEvent::ConnectFailed`, in the window we
    // just showed
    AppState::global(cx)
        .update(cx, |state, cx| state.connect(request, cx))
        .detach();
}

/// Disconnect through the workspace, which asks first when a query runs on
/// one of `targets` (and shows the window for the question). Without a
/// workspace no query can be running, so they just close.
pub(crate) fn request_disconnect(targets: Vec<ConnectionId>, cx: &mut App) {
    request_disconnect_with(true, targets, cx);
}

fn request_disconnect_with(may_retry: bool, targets: Vec<ConnectionId>, cx: &mut App) {
    let Some(main) = cx.try_global::<MainWindow>().cloned() else {
        return disconnect_all_of(&targets, cx);
    };
    let asked = main.handle.update(cx, |_, window, cx| {
        main.workspace.update(cx, |workspace, cx| {
            workspace.request_disconnect(targets.clone(), window, cx)
        })
    });
    if matches!(asked, Ok(Ok(()))) {
        return;
    }
    if may_retry && cx.windows().contains(&main.handle) {
        // Inside one of the window's own updates: try again once it is over
        cx.defer(move |cx| request_disconnect_with(false, targets, cx));
    } else {
        disconnect_all_of(&targets, cx);
    }
}

fn disconnect_all_of(targets: &[ConnectionId], cx: &mut App) {
    AppState::global(cx).update(cx, |state, cx| {
        for id in targets {
            state.disconnect(*id, cx);
        }
    });
}

/// Run a tray command (also the Dock menu's entries while no window is
/// active). Everything goes through `AppState` and the existing actions.
pub fn perform(command: TrayCommand, cx: &mut App) {
    let app_state = AppState::global(cx);
    match command {
        TrayCommand::Open => show_main_window(cx),
        TrayCommand::NewQuery => {
            show_main_window(cx);
            app_state.update(cx, |state, cx| match state.active_id() {
                Some(connection) => state.open_query(connection, None, String::new(), false, cx),
                None => state.open_home(cx),
            });
        }
        TrayCommand::NewConnection => {
            show_main_window(cx);
            if let Some(main) = cx.try_global::<MainWindow>().cloned() {
                // Deferred by GPUI until the window is front and rendered
                main.handle
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(act::NewConnection), cx)
                    })
                    .ok();
            }
        }
        TrayCommand::ConnectSession(name) => {
            show_main_window(cx);
            connect(ConnectRequest::session(name), cx);
        }
        TrayCommand::ConnectRecent(key) => {
            let entry = app_state
                .read(cx)
                .store()
                .recent_connections()
                .into_iter()
                .find(|recent| recent_key(&recent.url) == key);
            show_main_window(cx);
            if let Some(entry) = entry {
                connect(ConnectRequest::recent(entry), cx);
            }
        }
        TrayCommand::OpenConnection(id) => {
            if let Some(id) = live_connection(id, cx) {
                show_main_window(cx);
                // Its own workspace: the query tab used last, not only the
                // active connection
                app_state.update(cx, |state, cx| state.open_workspace(id, cx));
            }
        }
        TrayCommand::ConnectionNewQuery(id) => {
            if let Some(id) = live_connection(id, cx) {
                show_main_window(cx);
                app_state.update(cx, |state, cx| {
                    state.set_active(Some(id), cx);
                    state.open_query(id, None, String::new(), false, cx);
                });
            }
        }
        TrayCommand::Disconnect(id) => {
            if let Some(id) = live_connection(id, cx) {
                request_disconnect(vec![id], cx);
            }
        }
        TrayCommand::DisconnectAll => {
            let all = app_state
                .read(cx)
                .connections()
                .iter()
                .map(|live| live.id())
                .collect();
            request_disconnect(all, cx);
        }
        TrayCommand::Settings => {
            show_main_window(cx);
            app_state.update(cx, |state, cx| state.open_settings(cx));
        }
        TrayCommand::Quit => crate::quit(cx),
    }
}
