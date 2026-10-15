//! The catalogue of user commands. Everything that lists commands — key
//! bindings, the native menu bar, the command palette — iterates
//! [`AppCommand::iter`], so adding an action means adding one variant here
//! (strum drives the rest) and nothing drifts.
//!
//! Handlers live with the views: query actions in the query tab, the rest
//! in the workspace (`workspace/mod.rs`) and `lifecycle.rs`.

use crate::actions as act;
use gpui_kit::assets::IconName;
use gpui_kit::*;
use std::rc::Rc;
use strum::{EnumIter, IntoEnumIterator};

/// Where a command belongs in the palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, EnumIter)]
pub enum CommandCategory {
    Query,
    Tabs,
    Connection,
    Results,
    View,
    App,
}

impl CommandCategory {
    pub fn label(self) -> &'static str {
        match self {
            CommandCategory::Query => "Query",
            CommandCategory::Tabs => "Tabs",
            CommandCategory::Connection => "Connection",
            CommandCategory::Results => "Results",
            CommandCategory::View => "View",
            CommandCategory::App => "App",
        }
    }
}

/// The key context of the query tab's root view.
pub const QUERY_TAB_CONTEXT: &str = "QueryTab";
/// The editor is an `Input`; inside a query tab our bindings must outrank the
/// editor's own (`secondary-enter` and `secondary-.` are taken there).
const QUERY_EDITOR_CONTEXT: &str = "QueryTab > Input";
/// The results panel's key context.
pub const RESULTS_CONTEXT: &str = "Results";

/// One variant per action in `actions.rs`. Declaration order is the order of
/// menu items and palette rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, EnumIter)]
pub enum AppCommand {
    // File
    NewQuery,
    NewConnection,
    OpenSqlFile,
    SaveSqlFile,
    SaveNamedQuery,
    SaveSession,
    Disconnect,
    CloseTab,
    ReopenClosedTab,
    // Query
    RunStatement,
    RunAll,
    CancelQuery,
    ExplainStatement,
    ExplainAnalyze,
    FormatSql,
    ToggleReadOnly,
    CopySelection,
    ExportResults,
    // View
    ToggleSidebar,
    FocusExplorer,
    RefreshSchema,
    ToggleTheme,
    CommandPalette,
    // Window
    ShowHome,
    NextTab,
    PreviousTab,
    SwitchTab,
    RenameTab,
    GoToTab1,
    GoToTab2,
    GoToTab3,
    GoToTab4,
    GoToTab5,
    GoToTab6,
    GoToTab7,
    GoToTab8,
    GoToTab9,
    // App
    About,
    OpenSettings,
    ManageSavedQueries,
    Quit,
}

/// The native menu a command appears in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    App,
    File,
    View,
    Query,
    Window,
}

impl MenuSlot {
    pub fn title(self) -> &'static str {
        match self {
            MenuSlot::App => "DBCrust",
            MenuSlot::File => "File",
            MenuSlot::View => "View",
            MenuSlot::Query => "Query",
            MenuSlot::Window => "Window",
        }
    }
}

impl AppCommand {
    /// Sentence-case label (palette, tooltips). A trailing `…` means the
    /// command asks for more input.
    pub fn label(self) -> &'static str {
        match self {
            AppCommand::NewQuery => "New query",
            AppCommand::NewConnection => "New connection…",
            AppCommand::OpenSqlFile => "Open SQL file…",
            AppCommand::SaveSqlFile => "Save as…",
            AppCommand::SaveNamedQuery => "Save",
            AppCommand::SaveSession => "Save connection as session…",
            AppCommand::Disconnect => "Disconnect",
            AppCommand::CloseTab => "Close tab",
            AppCommand::ReopenClosedTab => "Reopen closed tab",
            AppCommand::RunStatement => "Run statement",
            AppCommand::RunAll => "Run all",
            AppCommand::CancelQuery => "Cancel query",
            AppCommand::ExplainStatement => "Explain statement",
            AppCommand::ExplainAnalyze => "Explain analyze",
            AppCommand::FormatSql => "Format SQL",
            AppCommand::ToggleReadOnly => "Toggle read-only mode",
            AppCommand::CopySelection => "Copy selected cells",
            AppCommand::ExportResults => "Export results…",
            AppCommand::ToggleSidebar => "Toggle sidebar",
            AppCommand::FocusExplorer => "Focus explorer",
            AppCommand::RefreshSchema => "Refresh schema",
            AppCommand::ToggleTheme => "Toggle theme",
            AppCommand::CommandPalette => "Show command palette",
            AppCommand::ShowHome => "Show connections",
            AppCommand::NextTab => "Next tab",
            AppCommand::PreviousTab => "Previous tab",
            AppCommand::SwitchTab => "Go to tab…",
            AppCommand::RenameTab => "Rename tab",
            AppCommand::GoToTab1 => "Go to tab 1",
            AppCommand::GoToTab2 => "Go to tab 2",
            AppCommand::GoToTab3 => "Go to tab 3",
            AppCommand::GoToTab4 => "Go to tab 4",
            AppCommand::GoToTab5 => "Go to tab 5",
            AppCommand::GoToTab6 => "Go to tab 6",
            AppCommand::GoToTab7 => "Go to tab 7",
            AppCommand::GoToTab8 => "Go to tab 8",
            AppCommand::GoToTab9 => "Go to last tab",
            AppCommand::About => "About DBCrust",
            AppCommand::OpenSettings => "Settings…",
            AppCommand::ManageSavedQueries => "Manage saved queries…",
            AppCommand::Quit => "Quit DBCrust",
        }
    }

    /// Title-case label for the native menu bar.
    pub fn menu_label(self) -> String {
        title_case(self.label())
    }

    /// Extra search terms for the palette.
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            AppCommand::NewQuery => &["tab", "editor", "sql", "console"],
            AppCommand::NewConnection => &["connect", "database", "home", "open"],
            AppCommand::OpenSqlFile => &["load", "import", "script"],
            AppCommand::SaveSqlFile => &[
                "save to file",
                "export",
                "write",
                "script",
                "sql file",
                "named query",
            ],
            AppCommand::SaveNamedQuery => &[
                "write",
                "file",
                "bookmark",
                "snippet",
                "favorite",
                "named query",
            ],
            AppCommand::SaveSession => &["bookmark", "favorite", "connection"],
            AppCommand::Disconnect => &["close", "connection", "logout"],
            AppCommand::CloseTab => &["remove"],
            AppCommand::ReopenClosedTab => &["undo close", "restore", "recover", "draft"],
            AppCommand::RunStatement => &["execute", "current", "play"],
            AppCommand::RunAll => &["execute", "script", "everything"],
            AppCommand::CancelQuery => &["stop", "abort", "interrupt"],
            AppCommand::ExplainStatement => &["plan", "execution plan", "optimizer"],
            AppCommand::ExplainAnalyze => &["plan", "profile", "timing", "analyze"],
            AppCommand::FormatSql => &["beautify", "pretty", "indent"],
            AppCommand::ToggleReadOnly => &["guard", "safe", "lock", "write"],
            AppCommand::CopySelection => &["clipboard", "cells", "rows"],
            AppCommand::ExportResults => &["csv", "json", "markdown", "save", "download"],
            AppCommand::ToggleSidebar => &["explorer", "panel", "hide", "show"],
            AppCommand::FocusExplorer => &["sidebar", "tables", "schema", "tree"],
            AppCommand::RefreshSchema => &["reload", "tables", "metadata"],
            AppCommand::ToggleTheme => &["dark", "light", "appearance", "mode"],
            AppCommand::CommandPalette => &["search", "commands", "quick open"],
            AppCommand::ShowHome => &["home", "connect", "welcome", "start"],
            AppCommand::NextTab => &["switch", "forward"],
            AppCommand::PreviousTab => &["switch", "back"],
            AppCommand::SwitchTab => &["switch", "find", "open tabs", "jump", "quick open"],
            AppCommand::RenameTab => &["title", "name", "label", "tab name"],
            AppCommand::GoToTab1
            | AppCommand::GoToTab2
            | AppCommand::GoToTab3
            | AppCommand::GoToTab4
            | AppCommand::GoToTab5
            | AppCommand::GoToTab6
            | AppCommand::GoToTab7
            | AppCommand::GoToTab8
            | AppCommand::GoToTab9 => &["switch", "jump", "number"],
            AppCommand::About => &["version", "license", "help"],
            AppCommand::OpenSettings => &["preferences", "config", "options", "appearance"],
            AppCommand::ManageSavedQueries => {
                &["named queries", "snippets", "bookmarks", "edit", "delete"]
            }
            AppCommand::Quit => &["exit", "close app"],
        }
    }

    pub fn category(self) -> CommandCategory {
        match self {
            AppCommand::RunStatement
            | AppCommand::RunAll
            | AppCommand::CancelQuery
            | AppCommand::ExplainStatement
            | AppCommand::ExplainAnalyze
            | AppCommand::SaveNamedQuery
            | AppCommand::OpenSqlFile
            | AppCommand::SaveSqlFile
            | AppCommand::FormatSql => CommandCategory::Query,
            AppCommand::NewQuery
            | AppCommand::CloseTab
            | AppCommand::ReopenClosedTab
            | AppCommand::NextTab
            | AppCommand::PreviousTab
            | AppCommand::SwitchTab
            | AppCommand::RenameTab
            | AppCommand::GoToTab1
            | AppCommand::GoToTab2
            | AppCommand::GoToTab3
            | AppCommand::GoToTab4
            | AppCommand::GoToTab5
            | AppCommand::GoToTab6
            | AppCommand::GoToTab7
            | AppCommand::GoToTab8
            | AppCommand::GoToTab9
            | AppCommand::ShowHome => CommandCategory::Tabs,
            AppCommand::NewConnection
            | AppCommand::Disconnect
            | AppCommand::RefreshSchema
            | AppCommand::ToggleReadOnly
            | AppCommand::SaveSession => CommandCategory::Connection,
            AppCommand::CopySelection | AppCommand::ExportResults => CommandCategory::Results,
            AppCommand::FocusExplorer
            | AppCommand::ToggleSidebar
            | AppCommand::ToggleTheme
            | AppCommand::CommandPalette => CommandCategory::View,
            AppCommand::ManageSavedQueries
            | AppCommand::OpenSettings
            | AppCommand::About
            | AppCommand::Quit => CommandCategory::App,
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            AppCommand::NewQuery => IconName::FilePlus,
            AppCommand::NewConnection => IconName::PlugZap,
            AppCommand::OpenSqlFile => IconName::FolderOpen,
            AppCommand::SaveSqlFile => IconName::FileDown,
            AppCommand::SaveNamedQuery => IconName::BookmarkPlus,
            AppCommand::SaveSession => IconName::Save,
            AppCommand::Disconnect => IconName::Unplug,
            AppCommand::CloseTab => IconName::Close,
            AppCommand::ReopenClosedTab => IconName::Undo2,
            AppCommand::RunStatement => IconName::Play,
            AppCommand::RunAll => IconName::ListVideo,
            AppCommand::CancelQuery => IconName::CircleStop,
            AppCommand::ExplainStatement => IconName::ChartNetwork,
            AppCommand::ExplainAnalyze => IconName::Gauge,
            AppCommand::FormatSql => IconName::WandSparkles,
            AppCommand::ToggleReadOnly => IconName::Lock,
            AppCommand::CopySelection => IconName::Copy,
            AppCommand::ExportResults => IconName::Download,
            AppCommand::ToggleSidebar => IconName::PanelLeft,
            AppCommand::FocusExplorer => IconName::FolderTree,
            AppCommand::RefreshSchema => IconName::RefreshCw,
            AppCommand::ToggleTheme => IconName::SunMoon,
            AppCommand::CommandPalette => IconName::Command,
            AppCommand::ShowHome => IconName::House,
            AppCommand::NextTab => IconName::ChevronRight,
            AppCommand::PreviousTab => IconName::ChevronLeft,
            AppCommand::SwitchTab => IconName::PanelTop,
            AppCommand::RenameTab => IconName::Pencil,
            AppCommand::GoToTab1
            | AppCommand::GoToTab2
            | AppCommand::GoToTab3
            | AppCommand::GoToTab4
            | AppCommand::GoToTab5
            | AppCommand::GoToTab6
            | AppCommand::GoToTab7
            | AppCommand::GoToTab8
            | AppCommand::GoToTab9 => IconName::Hash,
            AppCommand::About => IconName::Info,
            AppCommand::OpenSettings => IconName::Settings,
            AppCommand::ManageSavedQueries => IconName::Bookmark,
            AppCommand::Quit => IconName::Power,
        }
    }

    /// Default key bindings, strongest first (the first is the one shown in
    /// menus and tooltips). `secondary` is ⌘ on macOS and Ctrl elsewhere.
    pub fn shortcuts(self) -> &'static [&'static str] {
        let mac = cfg!(target_os = "macos");
        match self {
            AppCommand::RunStatement => &["secondary-enter"],
            AppCommand::RunAll => &["secondary-shift-enter"],
            AppCommand::ExplainStatement => &["secondary-e"],
            AppCommand::ExplainAnalyze => &["secondary-shift-e"],
            AppCommand::CancelQuery => &["secondary-."],
            AppCommand::FormatSql => &["shift-alt-f"],
            AppCommand::SaveNamedQuery => &["secondary-s"],
            AppCommand::OpenSqlFile => &["secondary-o"],
            AppCommand::SaveSqlFile => &["secondary-shift-s"],
            AppCommand::NewQuery => &["secondary-t"],
            AppCommand::CloseTab => &["secondary-w"],
            AppCommand::ReopenClosedTab => &["secondary-shift-t"],
            AppCommand::NextTab if mac => &["ctrl-tab", "cmd-shift-]"],
            AppCommand::NextTab => &["ctrl-tab", "ctrl-pagedown"],
            AppCommand::PreviousTab if mac => &["ctrl-shift-tab", "cmd-shift-["],
            AppCommand::PreviousTab => &["ctrl-shift-tab", "ctrl-pageup"],
            AppCommand::SwitchTab => &["secondary-p"],
            AppCommand::RenameTab => &["f2"],
            AppCommand::GoToTab1 => &["secondary-1"],
            AppCommand::GoToTab2 => &["secondary-2"],
            AppCommand::GoToTab3 => &["secondary-3"],
            AppCommand::GoToTab4 => &["secondary-4"],
            AppCommand::GoToTab5 => &["secondary-5"],
            AppCommand::GoToTab6 => &["secondary-6"],
            AppCommand::GoToTab7 => &["secondary-7"],
            AppCommand::GoToTab8 => &["secondary-8"],
            AppCommand::GoToTab9 => &["secondary-9"],
            AppCommand::NewConnection => &["secondary-n"],
            AppCommand::CommandPalette => &["secondary-k", "secondary-shift-p"],
            AppCommand::ToggleSidebar => &["secondary-b"],
            AppCommand::OpenSettings => &["secondary-,"],
            AppCommand::RefreshSchema => &["secondary-r"],
            AppCommand::FocusExplorer => &["secondary-0"],
            AppCommand::ShowHome => &["secondary-shift-h"],
            AppCommand::ToggleTheme => &["secondary-alt-t"],
            AppCommand::CopySelection => &["secondary-c"],
            AppCommand::ExportResults => &["secondary-alt-e"],
            AppCommand::Quit => &["secondary-q"],
            AppCommand::ManageSavedQueries
            | AppCommand::Disconnect
            | AppCommand::ToggleReadOnly
            | AppCommand::SaveSession
            | AppCommand::About => &[],
        }
    }

    /// Key contexts the shortcuts are bound in. `None` binds everywhere;
    /// query commands are bound again inside the editor to outrank its
    /// built-in bindings.
    fn key_contexts(self) -> &'static [Option<&'static str>] {
        match self {
            AppCommand::CopySelection => &[Some(RESULTS_CONTEXT)],
            _ if self.category() == CommandCategory::Query => &[None, Some(QUERY_EDITOR_CONTEXT)],
            _ => &[None],
        }
    }

    /// The position `Go to tab N` jumps to (1-based; 9 is the last tab).
    pub fn tab_number(self) -> Option<usize> {
        match self {
            AppCommand::GoToTab1 => Some(1),
            AppCommand::GoToTab2 => Some(2),
            AppCommand::GoToTab3 => Some(3),
            AppCommand::GoToTab4 => Some(4),
            AppCommand::GoToTab5 => Some(5),
            AppCommand::GoToTab6 => Some(6),
            AppCommand::GoToTab7 => Some(7),
            AppCommand::GoToTab8 => Some(8),
            AppCommand::GoToTab9 => Some(9),
            _ => None,
        }
    }

    /// Listed in the command palette. ⌘1…⌘9 are not: nine "Go to tab N" rows
    /// would bury the commands, and "Go to tab…" lists the tabs by name.
    pub fn in_palette(self) -> bool {
        self.tab_number().is_none()
    }

    /// Native menu this command is listed in, if any.
    pub fn menu(self) -> Option<MenuSlot> {
        match self {
            AppCommand::NewQuery
            | AppCommand::NewConnection
            | AppCommand::OpenSqlFile
            | AppCommand::SaveSqlFile
            | AppCommand::SaveNamedQuery
            | AppCommand::SaveSession
            | AppCommand::Disconnect
            | AppCommand::CloseTab
            | AppCommand::ReopenClosedTab => Some(MenuSlot::File),
            AppCommand::RunStatement
            | AppCommand::RunAll
            | AppCommand::CancelQuery
            | AppCommand::ExplainStatement
            | AppCommand::ExplainAnalyze
            | AppCommand::FormatSql
            | AppCommand::ToggleReadOnly
            | AppCommand::CopySelection
            | AppCommand::ExportResults => Some(MenuSlot::Query),
            AppCommand::ToggleSidebar
            | AppCommand::FocusExplorer
            | AppCommand::RefreshSchema
            | AppCommand::ToggleTheme
            | AppCommand::CommandPalette => Some(MenuSlot::View),
            AppCommand::ShowHome
            | AppCommand::NextTab
            | AppCommand::PreviousTab
            | AppCommand::SwitchTab
            | AppCommand::RenameTab => Some(MenuSlot::Window),
            // ⌘1…⌘9 are shortcuts of the tab bar, not menu items
            AppCommand::GoToTab1
            | AppCommand::GoToTab2
            | AppCommand::GoToTab3
            | AppCommand::GoToTab4
            | AppCommand::GoToTab5
            | AppCommand::GoToTab6
            | AppCommand::GoToTab7
            | AppCommand::GoToTab8
            | AppCommand::GoToTab9 => None,
            AppCommand::ManageSavedQueries => Some(MenuSlot::File),
            AppCommand::About | AppCommand::OpenSettings | AppCommand::Quit => Some(MenuSlot::App),
        }
    }

    /// A divider is drawn above this command in its menu.
    fn separator_before(self) -> bool {
        matches!(
            self,
            AppCommand::OpenSqlFile
                | AppCommand::SaveSession
                | AppCommand::CloseTab
                | AppCommand::CancelQuery
                | AppCommand::ExplainStatement
                | AppCommand::FormatSql
                | AppCommand::CopySelection
                | AppCommand::FocusExplorer
                | AppCommand::ToggleTheme
                | AppCommand::NextTab
                | AppCommand::OpenSettings
                | AppCommand::Quit
        )
    }

    pub fn action(self) -> Box<dyn Action> {
        match self {
            AppCommand::RunStatement => Box::new(act::RunStatement),
            AppCommand::RunAll => Box::new(act::RunAll),
            AppCommand::ExplainStatement => Box::new(act::ExplainStatement),
            AppCommand::ExplainAnalyze => Box::new(act::ExplainAnalyze),
            AppCommand::CancelQuery => Box::new(act::CancelQuery),
            AppCommand::SaveNamedQuery => Box::new(act::SaveNamedQuery),
            AppCommand::OpenSqlFile => Box::new(act::OpenSqlFile),
            AppCommand::SaveSqlFile => Box::new(act::SaveSqlFile),
            AppCommand::FormatSql => Box::new(act::FormatSql),
            AppCommand::NewQuery => Box::new(act::NewQuery),
            AppCommand::CloseTab => Box::new(act::CloseTab),
            AppCommand::ReopenClosedTab => Box::new(act::ReopenClosedTab),
            AppCommand::NextTab => Box::new(act::NextTab),
            AppCommand::PreviousTab => Box::new(act::PreviousTab),
            AppCommand::SwitchTab => Box::new(act::SwitchTab),
            AppCommand::RenameTab => Box::new(act::RenameTab),
            AppCommand::GoToTab1 => Box::new(act::GoToTab1),
            AppCommand::GoToTab2 => Box::new(act::GoToTab2),
            AppCommand::GoToTab3 => Box::new(act::GoToTab3),
            AppCommand::GoToTab4 => Box::new(act::GoToTab4),
            AppCommand::GoToTab5 => Box::new(act::GoToTab5),
            AppCommand::GoToTab6 => Box::new(act::GoToTab6),
            AppCommand::GoToTab7 => Box::new(act::GoToTab7),
            AppCommand::GoToTab8 => Box::new(act::GoToTab8),
            AppCommand::GoToTab9 => Box::new(act::GoToTab9),
            AppCommand::NewConnection => Box::new(act::NewConnection),
            AppCommand::Disconnect => Box::new(act::Disconnect),
            AppCommand::RefreshSchema => Box::new(act::RefreshSchema),
            AppCommand::ToggleReadOnly => Box::new(act::ToggleReadOnly),
            AppCommand::SaveSession => Box::new(act::SaveSession),
            AppCommand::CopySelection => Box::new(act::CopySelection),
            AppCommand::ExportResults => Box::new(act::ExportResults),
            AppCommand::ShowHome => Box::new(act::ShowHome),
            AppCommand::OpenSettings => Box::new(act::OpenSettings),
            AppCommand::ManageSavedQueries => Box::new(act::ManageSavedQueries),
            AppCommand::CommandPalette => Box::new(act::CommandPalette),
            AppCommand::FocusExplorer => Box::new(act::FocusExplorer),
            AppCommand::ToggleSidebar => Box::new(act::ToggleSidebar),
            AppCommand::ToggleTheme => Box::new(act::ToggleTheme),
            AppCommand::About => Box::new(act::About),
            AppCommand::Quit => Box::new(act::Quit),
        }
    }

    /// The command behind an action, if it is one of ours.
    pub fn from_action(action: &dyn Action) -> Option<AppCommand> {
        AppCommand::iter().find(|command| command.action().partial_eq(action))
    }
}

/// `Save connection as session…` → `Save Connection as Session…`.
fn title_case(label: &str) -> String {
    const SMALL_WORDS: [&str; 6] = ["as", "a", "an", "the", "to", "of"];
    label
        .split(' ')
        .enumerate()
        .map(|(ix, word)| {
            if ix > 0 && SMALL_WORDS.contains(&word) {
                return word.to_string();
            }
            let mut chars = word.chars();
            match chars.next() {
                Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
                _ => word.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Register every command's shortcuts. Call after `gpui_kit::init` so ours
/// are defined later than (and so outrank) the components' bindings where
/// they share a context.
pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for command in AppCommand::iter() {
        // Later bindings win ties, so register the primary shortcut last:
        // it is the one menus, tooltips and the palette display
        for shortcut in command.shortcuts().iter().rev() {
            for context in command.key_contexts() {
                match key_binding(shortcut, command.action(), *context) {
                    Some(binding) => bindings.push(binding),
                    None => tracing::error!(
                        "invalid shortcut {shortcut:?} for {command:?} in {context:?}"
                    ),
                }
            }
        }
    }
    cx.bind_keys(bindings);
}

fn key_binding(
    keystrokes: &str,
    action: Box<dyn Action>,
    context: Option<&str>,
) -> Option<KeyBinding> {
    let predicate = match context {
        Some(context) => Some(Rc::new(KeyBindingContextPredicate::parse(context).ok()?)),
        None => None,
    };
    KeyBinding::load(
        keystrokes,
        action,
        predicate,
        false,
        None,
        &DummyKeyboardMapper,
    )
    .ok()
}

/// The native menu bar, built from [`AppCommand::iter`].
pub fn menus() -> Vec<Menu> {
    let command_items = |slot: MenuSlot| {
        let mut items = Vec::new();
        for command in AppCommand::iter().filter(|command| command.menu() == Some(slot)) {
            if command.separator_before() && !items.is_empty() {
                items.push(MenuItem::separator());
            }
            items.push(MenuItem::Action {
                name: command.menu_label().into(),
                action: command.action(),
                os_action: None,
                checked: false,
                disabled: false,
            });
        }
        items
    };

    let mut app_menu = command_items(MenuSlot::App);
    // Standard macOS Services submenu, between Settings and Quit
    if let Some(quit_ix) = app_menu.len().checked_sub(1) {
        app_menu.insert(quit_ix, MenuItem::separator());
        app_menu.insert(
            quit_ix,
            MenuItem::os_submenu("Services", SystemMenuType::Services),
        );
    }

    let edit_item =
        |name: &'static str, action: Box<dyn Action>, os_action: OsAction| MenuItem::Action {
            name: name.into(),
            action,
            os_action: Some(os_action),
            checked: false,
            disabled: false,
        };
    use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    let edit_menu = vec![
        edit_item("Undo", Box::new(Undo), OsAction::Undo),
        edit_item("Redo", Box::new(Redo), OsAction::Redo),
        MenuItem::separator(),
        edit_item("Cut", Box::new(Cut), OsAction::Cut),
        edit_item("Copy", Box::new(Copy), OsAction::Copy),
        edit_item("Paste", Box::new(Paste), OsAction::Paste),
        edit_item("Select All", Box::new(SelectAll), OsAction::SelectAll),
    ];

    vec![
        Menu::new(MenuSlot::App.title()).items(app_menu),
        Menu::new(MenuSlot::File.title()).items(command_items(MenuSlot::File)),
        Menu::new("Edit").items(edit_menu),
        Menu::new(MenuSlot::View.title()).items(command_items(MenuSlot::View)),
        Menu::new(MenuSlot::Query.title()).items(command_items(MenuSlot::Query)),
        Menu::new(MenuSlot::Window.title()).items(command_items(MenuSlot::Window)),
    ]
}

/// The dock menu (right-click on the app icon).
pub fn dock_menu() -> Vec<MenuItem> {
    [AppCommand::NewQuery, AppCommand::NewConnection]
        .into_iter()
        .map(|command| MenuItem::Action {
            name: command.menu_label().into(),
            action: command.action(),
            os_action: None,
            checked: false,
            disabled: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;
    use gpui_kit::{
        Context, Entity, IntoElement, Render, TestAppContext, Window, WindowOptions, div,
    };
    use std::collections::HashMap;

    #[test]
    fn every_shortcut_and_context_parses() {
        for command in AppCommand::iter() {
            for shortcut in command.shortcuts() {
                assert!(
                    Keystroke::parse(shortcut).is_ok(),
                    "{command:?}: bad shortcut {shortcut}"
                );
            }
            for context in command.key_contexts().iter().flatten() {
                assert!(
                    KeyBindingContextPredicate::parse(context).is_ok(),
                    "{command:?}: bad context {context}"
                );
            }
        }
    }

    #[test]
    fn no_two_commands_share_a_keystroke_in_one_context() {
        let mut seen: HashMap<(String, Option<&str>), AppCommand> = HashMap::new();
        for command in AppCommand::iter() {
            for shortcut in command.shortcuts() {
                for context in command.key_contexts() {
                    let key = (shortcut.to_string(), *context);
                    if let Some(other) = seen.insert(key, command) {
                        panic!("{command:?} and {other:?} both bind {shortcut} in {context:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn labels_are_unique_and_actions_round_trip() {
        let mut labels = std::collections::HashSet::new();
        for command in AppCommand::iter() {
            assert!(
                labels.insert(command.label()),
                "duplicate label {}",
                command.label()
            );
            let action = command.action();
            assert_eq!(AppCommand::from_action(action.as_ref()), Some(command));
        }
    }

    #[test]
    fn menu_labels_use_title_case() {
        assert_eq!(
            title_case("Save connection as session…"),
            "Save Connection as Session…"
        );
        assert_eq!(title_case("Open SQL file…"), "Open SQL File…");
        assert_eq!(title_case("Toggle read-only mode"), "Toggle Read-only Mode");
        assert_eq!(AppCommand::Quit.menu_label(), "Quit DBCrust");
    }

    #[test]
    fn menus_list_every_menu_command_once() {
        let menus = menus();
        let listed: usize = menus
            .iter()
            .flat_map(|menu| menu.items.iter())
            .filter(|item| {
                matches!(
                    item,
                    MenuItem::Action {
                        os_action: None,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            listed,
            AppCommand::iter().filter(|c| c.menu().is_some()).count()
        );
    }

    #[test]
    fn query_commands_outrank_the_editor_bindings() {
        assert!(
            AppCommand::RunStatement
                .key_contexts()
                .contains(&Some(QUERY_EDITOR_CONTEXT))
        );
        assert_eq!(AppCommand::RunStatement.shortcuts(), &["secondary-enter"]);
    }

    // ----- Key dispatch: our bindings against the editor's own -----

    use gpui_kit::component::input::{Editor, EditorState};
    use gpui_kit::test::TestWindowExt as _;

    /// A query tab's shape: a `QueryTab` key context around an editor.
    struct EditorHost {
        editor: Entity<EditorState>,
        runs: usize,
        cancels: usize,
        explains: usize,
        analyzes: usize,
    }

    impl Render for EditorHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context(QUERY_TAB_CONTEXT)
                .on_action(cx.listener(|this, _: &act::RunStatement, _, _| this.runs += 1))
                .on_action(cx.listener(|this, _: &act::CancelQuery, _, _| this.cancels += 1))
                .on_action(cx.listener(|this, _: &act::ExplainStatement, _, _| this.explains += 1))
                .on_action(cx.listener(|this, _: &act::ExplainAnalyze, _, _| this.analyzes += 1))
                .child(Editor::new(&self.editor))
        }
    }

    #[gpui_kit::test]
    fn query_shortcuts_win_inside_the_editor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
        });
        let (handle, host) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| EditorHost {
                    editor: cx.new(|cx| EditorState::new(window, cx).default_value("select 1")),
                    runs: 0,
                    cancels: 0,
                    explains: 0,
                    analyzes: 0,
                })
            })
            .expect("open test window")
        });

        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let editor = host.read(cx).editor.clone();
            window.focus(&editor.focus_handle(cx), cx);
            window.render_frame(cx);

            // ⌘↵ is `Enter { secondary }` in the editor, ⌘. is "code actions"
            window.press("secondary-enter", cx);
            window.press("secondary-.", cx);
            window.press("secondary-e", cx);
            window.press("secondary-shift-e", cx);
        })
        .expect("window");

        cx.update(|cx| {
            let host = host.read(cx);
            assert_eq!(host.runs, 1, "run statement");
            assert_eq!(host.cancels, 1, "cancel query");
            assert_eq!(host.explains, 1, "explain");
            assert_eq!(host.analyzes, 1, "explain analyze");
            // The editor did not take ⌘↵ for a newline
            assert_eq!(host.editor.read(cx).value().to_string(), "select 1");
        });
    }
}
