//! DBCrust desktop app entry point.

use dbcrust_gui_core::ConfigStore;

fn main() {
    // Before any thread exists: import the login shell's PATH (Finder/Dock
    // launches get a minimal one)
    dbcrust_gui_core::env::import_login_shell_path();
    let store = ConfigStore::load();
    dbcrust_gui_core::dbcrust::database::set_query_timeout_seconds(
        store.read(|config| config.query_timeout_seconds),
    );

    let application = gpui_kit::application().with_assets(dbcrust_gui::assets());
    // Dock click or relaunch while the window is hidden in the tray
    application.on_reopen(dbcrust_gui::tray::show_main_window);
    application.run(move |cx| {
        dbcrust_gui::init(store, cx);
        let options = dbcrust_gui::main_window_options(cx);
        dbcrust_gui::open_main_window(options, cx).expect("failed to open the DBCrust window");
        // The event loop is running now, which the tray icon needs
        dbcrust_gui::tray::install(cx);
        cx.activate(true);
    });
}
