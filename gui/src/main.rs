//! DBCrust desktop app entry point.

use dbcrust_gui_core::ConfigStore;
use dbcrust_gui_core::gui_state::GuiState;

fn main() {
    dbcrust_gui_core::dbcrust::tls::install_default_crypto_provider();

    // Import the login shell's PATH (Finder/Dock launches get a minimal
    // one). The shell runs its profile while the settings are read; its
    // PATH is set before any thread that reads the environment exists.
    let login_path = dbcrust_gui_core::env::start_login_shell_path();
    // A GUI never prompts for the secrets passphrase on a terminal it may
    // have been launched from: a locked store is reported as locked instead
    dbcrust_gui_core::dbcrust::secrets::set_interactive(false);
    let store = ConfigStore::load();
    let gui_state = GuiState::load();
    login_path.apply();
    dbcrust_gui_core::dbcrust::database::set_query_timeout_seconds(
        store.read(|config| config.query_timeout_seconds),
    );

    let application = gpui_kit::application().with_assets(dbcrust_gui::assets());
    // Dock click or relaunch while the window is hidden in the tray
    application.on_reopen(dbcrust_gui::tray::show_main_window);
    application.run(move |cx| {
        dbcrust_gui::init_with_state(store, gui_state, cx);
        let options = dbcrust_gui::main_window_options(cx);
        dbcrust_gui::open_main_window(options, cx).expect("failed to open the DBCrust window");
        // The event loop is running now, which the tray icon needs
        dbcrust_gui::tray::install(cx);
        cx.activate(true);
    });
}
