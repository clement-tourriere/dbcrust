//! The operating system's tray icon: the macOS menu bar item and the Windows
//! notification area icon, through `tray-icon` and its `muda` menu
//! re-export. Both need the platform's main-thread event loop, which GPUI
//! already runs, so nothing here starts a loop of its own. Linux has no tray
//! this round: [`SUPPORTED`] is `false` and nothing is compiled or linked.
//!
//! Native callbacks arrive on the main thread but outside GPUI. They only
//! translate the event and push it into an `async-channel`, whose receiver a
//! foreground GPUI task awaits (`tray.rs`): GPUI state is never touched from
//! here.

use super::model::{TrayCommand, TrayItem};
use async_channel::Sender;

/// What the tray reports to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// A menu entry was chosen.
    Menu(TrayCommand),
    /// The icon itself was clicked (Windows: left click opens the app; on
    /// macOS the click opens the menu instead and this is never sent).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    IconClicked,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use native::SystemTray;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use unsupported::SystemTray;

/// This build has a tray implementation.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use super::*;
    use tray_icon::menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

    /// Monochrome (alpha-only) mark: macOS draws it as a template image so it
    /// follows the menu bar's light or dark appearance.
    #[cfg(target_os = "macos")]
    const ICON_PNG: &[u8] = include_bytes!("../../assets/brand/dbcrust-tray-template-32.png");
    /// The colour mark: a white glyph would vanish on a light taskbar.
    #[cfg(not(target_os = "macos"))]
    const ICON_PNG: &[u8] = include_bytes!("../../assets/brand/dbcrust-tray-color-32.png");

    pub struct SystemTray {
        icon: TrayIcon,
    }

    fn load_icon() -> Result<Icon, String> {
        let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
            .map_err(|error| format!("the tray icon is not a valid PNG: {error}"))?
            .into_rgba8();
        let (width, height) = image.dimensions();
        Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| format!("the tray icon is unusable: {error}"))
    }

    fn menu_item(item: &TrayItem) -> Result<Box<dyn IsMenuItem>, tray_icon::menu::Error> {
        Ok(match item {
            TrayItem::Label(text) => Box::new(MenuItem::new(escape(text), false, None)),
            TrayItem::Action {
                command,
                label,
                enabled,
            } => Box::new(MenuItem::with_id(
                command.id(),
                escape(label),
                *enabled,
                None,
            )),
            TrayItem::Submenu {
                label,
                enabled,
                items,
            } => {
                let submenu = Submenu::new(escape(label), *enabled);
                for child in items {
                    submenu.append(menu_item(child)?.as_ref())?;
                }
                Box::new(submenu)
            }
            TrayItem::Separator => Box::new(PredefinedMenuItem::separator()),
        })
    }

    fn escape(label: &str) -> String {
        super::super::model::escape_mnemonics(label)
    }

    fn menu(items: &[TrayItem]) -> Result<Menu, tray_icon::menu::Error> {
        let menu = Menu::new();
        for item in items {
            menu.append(menu_item(item)?.as_ref())?;
        }
        Ok(menu)
    }

    impl SystemTray {
        /// Create the icon with `items` as its menu. Must run on the main
        /// thread once the event loop is running.
        pub fn create(events: Sender<TrayEvent>, items: &[TrayItem]) -> Result<Self, String> {
            let menu = menu(items).map_err(|error| format!("the tray menu failed: {error}"))?;

            let sender = events.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some(command) = TrayCommand::parse(&event.id.0) {
                    // Unbounded: never blocks the native callback
                    let _ = sender.try_send(TrayEvent::Menu(command));
                }
            }));
            // Windows opens the app on a left click and the menu on a right
            // click; on macOS the menu opens on click and nothing is needed.
            #[cfg(target_os = "windows")]
            {
                use tray_icon::{MouseButton, MouseButtonState};
                let sender = events;
                TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let _ = sender.try_send(TrayEvent::IconClicked);
                    }
                }));
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = events;
                // Without a handler, hover events pile up in a channel nobody reads
                TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| {}));
            }

            let builder = TrayIconBuilder::new()
                .with_tooltip("DBCrust")
                .with_icon(load_icon()?)
                .with_menu(Box::new(menu));
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_as_template(true);
            #[cfg(target_os = "windows")]
            let builder = builder.with_menu_on_left_click(false);

            let icon = builder
                .build()
                .map_err(|error| format!("the system refused the tray icon: {error}"))?;
            Ok(Self { icon })
        }

        /// Replace the whole menu.
        pub fn set_menu(&self, items: &[TrayItem]) {
            match menu(items) {
                Ok(menu) => self.icon.set_menu(Some(Box::new(menu))),
                Err(error) => tracing::warn!("could not rebuild the tray menu: {error}"),
            }
        }

        pub fn set_tooltip(&self, text: &str) {
            if let Err(error) = self.icon.set_tooltip(Some(text)) {
                tracing::warn!("could not set the tray tooltip: {error}");
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use super::*;

    /// Never constructed: [`SystemTray::create`] always fails here.
    pub struct SystemTray;

    impl SystemTray {
        pub fn create(_: Sender<TrayEvent>, _: &[TrayItem]) -> Result<Self, String> {
            Err("this platform has no tray support in this build".into())
        }

        pub fn set_menu(&self, _: &[TrayItem]) {}

        pub fn set_tooltip(&self, _: &str) {}
    }
}
