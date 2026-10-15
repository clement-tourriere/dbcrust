//! DBCrust desktop app (GPUI Kit). `main.rs` and the screenshot harness
//! (`bin/screenshots.rs`) share [`init`] and [`open_main_window`].

pub mod about;
pub mod actions;
pub mod app_state;
pub mod assets;
pub mod clipboard;
pub mod commands;
pub mod explorer;
pub mod home;
pub mod inspector;
pub mod palette;
pub mod preferences;
pub mod query;
pub mod settings;
#[cfg(test)]
mod test_support;
pub mod theme;
pub mod tray;
pub mod widgets;
pub mod workspace;

use app_state::AppState;
pub use assets::assets;
use dbcrust_gui_core::ConfigStore;
use dbcrust_gui_core::gui_state::GuiState;
use gpui_kit::*;
use preferences::Preferences;
use workspace::Workspace;

/// Default main window size.
pub const WINDOW_SIZE: Size<Pixels> = Size {
    width: px(1440.),
    height: px(900.),
};

/// The desktop state of the previous session (drafts, sidebar), read once at
/// startup and consumed by the workspace.
#[derive(Clone)]
pub(crate) struct RestoredState(pub GuiState);

impl Global for RestoredState {}

/// The main window and its workspace: how the global `Quit` action reaches the
/// quit coordinator, and what the tray shows and brings back.
#[derive(Clone)]
pub(crate) struct MainWindow {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) workspace: WeakEntity<Workspace>,
}

impl Global for MainWindow {}

/// Everything the app registers before opening a window: GPUI Kit, themes,
/// key bindings, menus, global state. Call once, first thing in `run`; the
/// asset source passed to the application must be [`assets()`].
pub fn init(store: ConfigStore, cx: &mut App) {
    init_with_state(store, GuiState::load(), cx);
}

/// [`init`] with `gui_state.toml` already read (`main` reads it while the
/// login shell's `PATH` is being imported).
pub fn init_with_state(store: ConfigStore, gui_state: GuiState, cx: &mut App) {
    gpui_kit::init(cx);
    theme::init(&gui_state, cx);
    Preferences::init(&gui_state, cx);
    cx.set_global(RestoredState(gui_state));
    AppState::init(store, cx);

    commands::bind_keys(cx);
    // Copy and cut shortcuts in any window count as copies (see `clipboard`)
    clipboard::observe_copy_keys(cx);
    cx.set_menus(commands::menus());
    cx.set_dock_menu(commands::dock_menu());

    // Windows come and go (the main window hides in the tray, a closed one is
    // reopened from it): only `Quit` ends the app
    cx.set_quit_mode(QuitMode::Explicit);
    tray::init(cx);
    // One quit path: confirm if a query runs, save, disconnect, quit
    cx.on_action(|_: &actions::Quit, cx| quit(cx));
    // The last window went away without the quit path (closing normally
    // either hides it or quits): without a tray there is no way back, so end
    // the app instead of leaving it running invisibly
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() && !tray::keeps_running(cx) {
            cx.quit();
        }
    })
    .detach();
}

/// Quit through the workspace's coordinator: confirm running queries, final
/// save, disconnect, `cx.quit()`.
pub(crate) fn quit(cx: &mut App) {
    quit_with(true, cx);
}

fn quit_with(may_retry: bool, cx: &mut App) {
    let Some(main) = cx.try_global::<MainWindow>().cloned() else {
        quit_without_window(cx);
        return;
    };
    let requested = main.handle.update(cx, |_, window, cx| {
        main.workspace
            .update(cx, |workspace, cx| workspace.request_quit(window, cx))
    });
    if matches!(requested, Ok(Ok(()))) {
        return;
    }
    if may_retry && cx.windows().contains(&main.handle) {
        // Called from inside the window's own update (the global handler of
        // the ⌘Q action runs there): the window is busy, not gone
        cx.defer(|cx| quit_with(false, cx));
    } else {
        quit_without_window(cx);
    }
}

/// No workspace is left to coordinate (the window was destroyed while the
/// tray kept the app alive; its drafts were saved when it closed).
fn quit_without_window(cx: &mut App) {
    AppState::global(cx).update(cx, |state, cx| state.disconnect_all(cx));
    cx.quit();
}

/// Window options of the main window.
pub fn main_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, WINDOW_SIZE, cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(600.))),
        // Our own title bar: transparent native one, inset traffic lights
        ..gpui_kit::component::TitleBar::window_options()
    }
}

/// Open the main window with the workspace.
pub fn open_main_window(
    options: WindowOptions,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Workspace>)> {
    let (handle, workspace) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(window, cx))
    })?;
    cx.set_global(MainWindow {
        handle,
        workspace: workspace.downgrade(),
    });
    Ok((handle, workspace))
}
