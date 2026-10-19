//! Settings tab: desktop preferences (closing the window, startup, theme,
//! editor font size), every shared DBCrust setting, and managers for the
//! user data next to them (saved queries, SSH tunnels, the Vault cache,
//! recent connections).
//!
//! DBCrust settings are edited through the same schema as the CLI's
//! `\config` (`dbcrust_app::config_editor`), so labels, ranges and validation
//! are shared; [`schema`] puts each field on one page. Controls read a
//! cached snapshot and changes are saved in the background, keeping the
//! documentation comments of `config.toml` ([`storage`]). Desktop
//! preferences live in [`Preferences`] and `gui_state.toml`.

mod saved_queries;
mod schema;
mod storage;
mod tunnels;

pub(crate) use storage::{flush_on_quit, has_unsaved_settings};

use crate::about::{DOCS_URL, GITHUB_URL, VERSION};
use crate::actions::About;
use crate::app_state::{AppEvent, AppState};
use crate::preferences::Preferences;
use crate::theme;
use crate::tray::{self, Availability, Wording};
use dbcrust_gui_core::ConfigStore;
use dbcrust_gui_core::dbcrust_app::config::Config;
use dbcrust_gui_core::dbcrust_app::config_editor;
use dbcrust_gui_core::gui_state::{GuiState, StatementSeparators, ThemeModeSetting};
use dbcrust_gui_core::store::VaultCache;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, ThemeMode,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    group_box::GroupBoxVariant,
    h_flex,
    notification::Notification,
    setting::{
        NumberFieldOptions, RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem,
        SettingPage, Settings,
    },
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use storage::{Clearable, SettingsCache};
use strum::IntoEnumIterator;

/// Width of the page list.
const SETTINGS_NAV_WIDTH: Pixels = px(200.);
/// Navigation plus a content column; wide enough for the saved query
/// manager's list and editor side by side.
const SETTINGS_MAX_WIDTH: Pixels = px(1040.);
/// Space around the manager pages' content (title bar, tabs, status bar,
/// page header): the manager fills the rest of the window.
const MANAGER_CHROME_HEIGHT: Pixels = px(268.);
const MANAGER_MIN_HEIGHT: Pixels = px(420.);

/// Sentinel dropdown value for "no custom theme".
const DEFAULT_THEME_OPTION: &str = "default";

/// The Settings pages, in navigation order. Commands and other views open
/// Settings on one of them ([`AppState::open_settings_page`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum SettingsPage {
    General,
    Appearance,
    Queries,
    SavedQueries,
    Connections,
    SshTunnels,
    Vault,
    ResultsDisplay,
    Ai,
    Terminal,
    Logging,
    Files,
}

impl SettingsPage {
    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Queries => "Queries",
            Self::SavedQueries => "Saved queries",
            Self::Connections => "Connections",
            Self::SshTunnels => "SSH tunnels",
            Self::Vault => "Vault",
            Self::ResultsDisplay => "Results display",
            Self::Ai => "AI assistant",
            Self::Terminal => "Terminal (CLI)",
            Self::Logging => "Logging",
            Self::Files => "Files",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Queries => IconName::SquareTerminal,
            Self::SavedQueries => IconName::Bookmark,
            Self::Connections => IconName::Database,
            Self::SshTunnels => IconName::Route,
            Self::Vault => IconName::KeyRound,
            Self::ResultsDisplay => IconName::Table,
            Self::Ai => IconName::Sparkles,
            Self::Terminal => IconName::Terminal,
            Self::Logging => IconName::ScrollText,
            Self::Files => IconName::FolderOpen,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::General => "Closing the window and starting up.",
            Self::Appearance => "Theme and text size.",
            Self::Queries => "Execution and the SQL editor.",
            Self::SavedQueries => {
                "Saved queries of every scope, shared with the dbcrust command line. Use $1, $2 \
                 for parameters."
            }
            Self::Connections => "Defaults, recent connections and saved connections.",
            Self::SshTunnels => {
                "A connection to a host that matches a pattern goes through the pattern’s SSH \
                 server. Patterns are regular expressions matched against the database host."
            }
            Self::Vault => "HashiCorp Vault credentials for vault:// connections.",
            Self::ResultsDisplay => {
                "How vectors, JSON, arrays and other complex values are formatted in results."
            }
            Self::Ai => {
                "The AI assistant of the dbcrust command line (?? and \\ai). The desktop app \
                 does not use it yet."
            }
            Self::Terminal => {
                "These settings apply to the dbcrust command line only: its prompt, pager, \
                 EXPLAIN mode and history."
            }
            Self::Logging => {
                "Diagnostic logs, for the app and the command line. Changes take effect next \
                 session."
            }
            Self::Files => "",
        }
    }

    /// The page with its title, icon and description, before its groups.
    fn page(self) -> SettingPage {
        let page = SettingPage::new(self.title()).icon(Icon::new(self.icon()));
        match self.description() {
            "" => page,
            description => page.description(description),
        }
    }
}

pub struct SettingsView {
    focus_handle: FocusHandle,
    /// Default settings, the target of every reset button.
    defaults: Rc<Config>,
    selected_page: SettingsPage,
    /// Bumped to re-apply `selected_page` to an open Settings tab.
    navigation: u64,
    saved_queries: Entity<saved_queries::SavedQueriesView>,
    tunnels: Entity<tunnels::TunnelsView>,
    tunnel_tester: Entity<tunnels::TunnelTester>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        storage::ensure(cx);
        let saved_queries = cx.new(|cx| saved_queries::SavedQueriesView::new(window, cx));
        let tunnels = cx.new(|cx| tunnels::TunnelsView::new(window, cx));
        let tunnel_tester = cx.new(|cx| tunnels::TunnelTester::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(&AppState::global(cx), |_, _, event, cx| {
                if matches!(event, AppEvent::StorageChanged) {
                    storage::refresh(cx);
                }
            }),
            cx.observe_global::<SettingsCache>(|_, cx| cx.notify()),
        ];
        storage::refresh(cx);
        Self {
            focus_handle: cx.focus_handle(),
            defaults: Rc::new(Config::default_settings()),
            selected_page: SettingsPage::General,
            navigation: 0,
            saved_queries,
            tunnels,
            tunnel_tester,
            _subscriptions: subscriptions,
        }
    }

    /// Show `page`, also when this Settings tab is already open.
    pub fn select_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.selected_page = page;
        self.navigation += 1;
        cx.notify();
    }

    /// Type `host` in SSH tunnels → Test a host (screenshot fixtures).
    pub fn test_tunnel_host(&mut self, host: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tunnel_tester
            .update(cx, |tester, cx| tester.set_host(host, window, cx));
    }

    /// Every page, in [`SettingsPage`] order.
    fn pages(&self, cx: &App) -> Vec<SettingPage> {
        SettingsPage::iter()
            .map(|page| match page {
                SettingsPage::General => self.general_page(cx),
                SettingsPage::Appearance => self.appearance_page(cx),
                SettingsPage::Queries => self.queries_page(),
                SettingsPage::SavedQueries => self.saved_queries_page(),
                SettingsPage::Connections => self.connections_page(cx),
                SettingsPage::SshTunnels => self.tunnels_page(),
                SettingsPage::Vault => self.vault_page(cx),
                SettingsPage::ResultsDisplay
                | SettingsPage::Ai
                | SettingsPage::Terminal
                | SettingsPage::Logging => page
                    .page()
                    .groups(schema::schema_groups(page, &self.defaults)),
                SettingsPage::Files => self.files_page(cx),
            })
            .collect()
    }

    fn general_page(&self, cx: &App) -> SettingPage {
        let hide_dock_icon = Preferences::get(cx).hide_dock_icon_when_closed;
        let wording = Wording::current(hide_dock_icon);

        let mut window = SettingGroup::new().title("Window");
        match tray::availability(cx) {
            Availability::Available => {
                window = window.item(preference_switch(
                    format!("Keep running in the {}", wording.area),
                    format!(
                        "Closing the window hides it instead of quitting. Connections and \
                         queries stay active. {}; {} quits.",
                        wording.reopen, wording.quit_shortcut
                    ),
                    |p| p.keep_running_in_tray,
                    |p, value| p.keep_running_in_tray = value,
                    [
                        "tray",
                        "background",
                        "quit",
                        "close",
                        "menu bar",
                        "notification area",
                    ],
                ));
                if cfg!(target_os = "macos") {
                    window = window.item(preference_switch(
                        "Hide the Dock icon while the window is closed".to_string(),
                        "Leaves only the menu bar icon, and DBCrust leaves ⌘Tab until reopened."
                            .to_string(),
                        |p| p.hide_dock_icon_when_closed,
                        |p, value| p.hide_dock_icon_when_closed = value,
                        ["dock", "tray", "background", "menu bar"],
                    ));
                }
            }
            Availability::Unsupported => {
                window = window.item(note(
                    "Background mode isn’t available in this build. Closing the window quits \
                     DBCrust.",
                ));
            }
            Availability::Failed(reason) => {
                window = window.item(note(format!(
                    "DBCrust couldn’t create its {} icon ({reason}), so closing the window \
                     quits it.",
                    wording.area
                )));
            }
        }

        SettingsPage::General
            .page()
            .default_open(true)
            .group(window)
            .group(
                SettingGroup::new().title("Startup").item(preference_switch(
                    "Reopen unsaved queries at launch".to_string(),
                    "Restores the query tabs you had open, with their text. When off, query \
                     text is not saved to disk."
                        .to_string(),
                    |p| p.restore_drafts,
                    |p, value| p.restore_drafts = value,
                    ["drafts", "restore", "tabs", "session", "startup"],
                )),
            )
    }

    fn appearance_page(&self, cx: &App) -> SettingPage {
        let themes = theme::available_themes(cx);
        let mut theme_options: Vec<(SharedString, SharedString)> =
            vec![(DEFAULT_THEME_OPTION.into(), "Default".into())];
        for choice in &themes {
            let is_default = choice.name == theme::default_theme_name(choice.mode, cx);
            if is_default {
                continue;
            }
            let mode = if choice.mode == ThemeMode::Dark {
                "dark"
            } else {
                "light"
            };
            theme_options.push((
                choice.name.clone(),
                format!("{} ({mode})", choice.name).into(),
            ));
        }

        SettingsPage::Appearance.page().group(
            SettingGroup::new()
                .title("Theme")
                .item(
                    SettingItem::new(
                        "Mode",
                        SettingField::dropdown(
                            vec![
                                ("system".into(), "System".into()),
                                ("light".into(), "Light".into()),
                                ("dark".into(), "Dark".into()),
                            ],
                            |cx: &App| mode_value(theme::appearance(cx).mode).into(),
                            |value: SharedString, cx: &mut App| {
                                theme::set_mode(parse_mode(&value), cx);
                            },
                        )
                        .default_value("system"),
                    )
                    .description("System follows your OS appearance.")
                    .keywords(["dark", "light", "appearance"]),
                )
                .item(
                    SettingItem::new(
                        "Interface size",
                        SettingField::dropdown(
                            vec![
                                ("14".into(), "Compact".into()),
                                ("16".into(), "Default".into()),
                                ("18".into(), "Large".into()),
                            ],
                            |cx: &App| {
                                format!("{}", theme::appearance(cx).ui_font_size.round() as u32)
                                    .into()
                            },
                            |value: SharedString, cx: &mut App| {
                                if let Ok(size) = value.parse::<f32>() {
                                    theme::set_ui_font_size(size, cx);
                                }
                            },
                        )
                        .default_value("16"),
                    )
                    .description("Scales text and controls together.")
                    .keywords(["zoom", "font", "density", "compact", "size"]),
                )
                .item(
                    SettingItem::new(
                        "Color theme",
                        SettingField::scrollable_dropdown(
                            theme_options,
                            |cx: &App| {
                                theme::appearance(cx)
                                    .theme_name
                                    .unwrap_or_else(|| DEFAULT_THEME_OPTION.into())
                            },
                            |value: SharedString, cx: &mut App| {
                                let name =
                                    (value.as_ref() != DEFAULT_THEME_OPTION).then_some(value);
                                theme::set_theme(name, cx);
                            },
                        )
                        .default_value(DEFAULT_THEME_OPTION),
                    )
                    .description("A light or dark theme applies in its own mode.")
                    .keywords([
                        "catppuccin",
                        "tokyo",
                        "gruvbox",
                        "solarized",
                        "colors",
                    ]),
                ),
        )
    }

    fn queries_page(&self) -> SettingPage {
        SettingsPage::Queries
            .page()
            .group(
                SettingGroup::new()
                    .title("Execution")
                    .items(schema::schema_items(SettingsPage::Queries, &self.defaults))
                    .item(statement_separators_item()),
            )
            .group(
                SettingGroup::new()
                    .title("Editor")
                    .item(editor_font_size_item()),
            )
    }

    /// A manager (list + editor) filling the window below the page header.
    fn saved_queries_page(&self) -> SettingPage {
        let manager = self.saved_queries.clone();
        SettingsPage::SavedQueries.page().resettable(false).group(
            SettingGroup::new().item(
                SettingItem::render(move |_, window, _| {
                    let height = (window.viewport_size().height - MANAGER_CHROME_HEIGHT)
                        .max(MANAGER_MIN_HEIGHT);
                    div().w_full().h(height).child(manager.clone())
                })
                .keywords([
                    "saved queries",
                    "named queries",
                    "snippets",
                    "\\ns",
                    "sql",
                ]),
            ),
        )
    }

    fn connections_page(&self, cx: &App) -> SettingPage {
        let recents = storage::cache(cx).config.get_recent_connections().len();
        let field = |path| schema::schema_item(path, &self.defaults);
        SettingsPage::Connections
            .page()
            .group(
                SettingGroup::new()
                    .title("Safety")
                    .items(field("read_only_default")),
            )
            .group(
                SettingGroup::new()
                    .title("Recent connections")
                    .items(field("max_recent_connections"))
                    .item(action_row(
                        match recents {
                            0 => "No recent connections stored.".to_string(),
                            1 => "1 recent connection stored.".to_string(),
                            n => format!("{n} recent connections stored."),
                        },
                        move || {
                            Button::new("clear-recents")
                                .outline()
                                .small()
                                .label("Clear recent connections…")
                                .disabled(recents == 0)
                                .on_click(|_, window, cx| {
                                    storage::confirm_clear(Clearable::RecentConnections, window, cx)
                                })
                        },
                        ["recent", "history", "clear"],
                    )),
            )
            .group(
                SettingGroup::new()
                    .title("Saved connections")
                    .item(action_row(
                        "Saved connections, their credentials and environments are managed on \
                         the Connections page."
                            .to_string(),
                        || {
                            Button::new("open-connections")
                                .outline()
                                .small()
                                .label("Open Connections")
                                .on_click(|_, _, cx| {
                                    AppState::global(cx).update(cx, |state, cx| state.open_home(cx))
                                })
                        },
                        ["sessions", "saved connections", "credentials"],
                    )),
            )
    }

    fn tunnels_page(&self) -> SettingPage {
        let tunnels = self.tunnels.clone();
        let tester = self.tunnel_tester.clone();
        SettingsPage::SshTunnels
            .page()
            .resettable(false)
            .group(SettingGroup::new().title("Tunnels").item(
                SettingItem::render(move |_, _, _| tunnels.clone()).keywords([
                    "ssh",
                    "tunnel",
                    "bastion",
                    "jump host",
                    "pattern",
                ]),
            ))
            .group(
                SettingGroup::new()
                    .title("Test a host")
                    .description("Shows which tunnel a database host would use. Nothing connects.")
                    .item(
                        SettingItem::render(move |_, _, _| tester.clone())
                            .keywords(["ssh", "tunnel", "test", "host", "match"]),
                    ),
            )
    }

    fn vault_page(&self, cx: &App) -> SettingPage {
        let vault = storage::cache(cx).vault.clone();
        let mut cached = SettingGroup::new().title("Cached credentials");
        let (note_text, can_clear) = match &vault {
            None => (Some("Reading the credential cache…"), false),
            Some(VaultCache::Locked) => (
                Some(
                    "The cache is encrypted and no Vault token is available to read it. \
                     Connect with Vault, then come back to see what it holds.",
                ),
                true,
            ),
            Some(VaultCache::Entries(entries)) if entries.is_empty() => (
                Some(
                    "No cached credentials. Vault connections cache theirs here until they \
                     expire.",
                ),
                false,
            ),
            Some(VaultCache::Entries(_)) => (None, true),
        };
        if let Some(text) = note_text {
            cached = cached.item(note(text));
        }
        if let Some(VaultCache::Entries(entries)) = vault {
            for entry in entries {
                cached = cached.item(vault_row(entry));
            }
        }
        cached = cached.item(
            SettingItem::render(move |_, _, _| {
                h_flex().child(
                    Button::new("clear-vault-cache")
                        .outline()
                        .small()
                        .label("Clear cached credentials…")
                        .disabled(!can_clear)
                        .on_click(|_, window, cx| {
                            storage::confirm_clear(Clearable::VaultCache, window, cx)
                        }),
                )
            })
            .keywords(["vault", "cache", "credentials", "clear"]),
        );
        SettingsPage::Vault
            .page()
            .groups(schema::schema_groups(SettingsPage::Vault, &self.defaults))
            .group(cached)
    }

    fn files_page(&self, cx: &App) -> SettingPage {
        let directory = storage::cache(cx).directory.clone();
        let files: [(&'static str, &'static str); 5] = [
            ("Settings", "config.toml"),
            ("Saved sessions", "sessions.toml"),
            ("Saved queries", "named_queries.toml"),
            ("Recent connections", "recent.toml"),
            ("Desktop state", "gui_state.toml"),
        ];

        let mut storage_group = SettingGroup::new()
            .title("Storage")
            .description("Plain files you can back up or edit. The CLI uses the same ones.");
        if let Some(directory) = &directory {
            storage_group =
                storage_group.item(path_item("Config directory", directory.clone(), true));
            for (title, file) in files {
                storage_group = storage_group.item(path_item(title, directory.join(file), false));
            }
        } else {
            storage_group = storage_group.item(note("The config directory could not be located."));
        }
        let config_file = directory.map(|directory| directory.join("config.toml"));

        SettingsPage::Files
            .page()
            .resettable(false)
            .group(storage_group)
            .group(
                SettingGroup::new()
                    .title("Edits outside the app")
                    .item(action_row(
                        "Read every file again after editing them, or after changes made with \
                         the command line."
                            .to_string(),
                        || {
                            Button::new("reload-config")
                                .outline()
                                .small()
                                .icon(IconName::RefreshCw)
                                .label("Reload from disk")
                                .on_click(|_, _, cx| storage::reload(cx))
                        },
                        ["reload", "refresh", "disk", "external"],
                    ))
                    .item(action_row(
                        "Open config.toml in your default editor. Reload when you are done."
                            .to_string(),
                        move || {
                            let config_file = config_file.clone();
                            Button::new("edit-config")
                                .outline()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label("Open config.toml")
                                .disabled(config_file.is_none())
                                .on_click(move |_, _, cx| {
                                    if let Some(path) = &config_file {
                                        cx.open_with_system(path);
                                    }
                                })
                        },
                        ["config.toml", "editor", "edit", "open"],
                    )),
            )
            .group(
                SettingGroup::new()
                    .title("About")
                    .item(SettingItem::render(|options, _, cx| about_row(options, cx))),
            )
    }
}

/// Text on the left, one button on the right.
fn action_row<const N: usize>(
    text: String,
    button: impl Fn() -> Button + 'static,
    keywords: [&'static str; N],
) -> SettingItem {
    let text: SharedString = text.into();
    SettingItem::render(move |_, _, cx| {
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(text.clone()),
            )
            .child(div().flex_none().child(button()))
    })
    .keywords(keywords)
}

/// One cached Vault credential: where it is from and when it expires.
fn vault_row(entry: dbcrust_gui_core::store::VaultCacheEntry) -> SettingItem {
    let now = std::time::SystemTime::now().into();
    let expiry = entry.expiry_label(now);
    let expired = entry.is_expired(now);
    let path = format!("{}/{}", entry.mount, entry.database);
    let role = format!("role {}", entry.role);
    SettingItem::render(move |_, _, cx| {
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .child(
                Icon::new(IconName::KeyRound)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(path.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(role.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(120.))
                    .text_right()
                    .text_xs()
                    .text_color(if expired {
                        cx.theme().warning
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(expiry.clone()),
            )
    })
    .keywords(["vault", "cached", "credentials", "lease"])
}

fn mode_value(mode: ThemeModeSetting) -> &'static str {
    match mode {
        ThemeModeSetting::System => "system",
        ThemeModeSetting::Light => "light",
        ThemeModeSetting::Dark => "dark",
    }
}

fn parse_mode(value: &str) -> ThemeModeSetting {
    match value {
        "light" => ThemeModeSetting::Light,
        "dark" => ThemeModeSetting::Dark,
        _ => ThemeModeSetting::System,
    }
}

// ----- Preferences (gui_state.toml) -----

/// A switch bound to one boolean of [`Preferences`]; its reset button
/// restores the default.
fn preference_switch<const N: usize>(
    title: String,
    description: String,
    read: fn(&Preferences) -> bool,
    write: fn(&mut Preferences, bool),
    keywords: [&'static str; N],
) -> SettingItem {
    let default = read(&Preferences::default());
    SettingItem::new(
        title,
        SettingField::switch(
            move |cx: &App| read(&Preferences::get(cx)),
            move |value: bool, cx: &mut App| Preferences::update(cx, |p| write(p, value)),
        )
        .default_value(default),
    )
    .description(description)
    .keywords(keywords)
}

fn separators_value(separators: StatementSeparators) -> &'static str {
    match separators {
        StatementSeparators::SemicolonsAndBlankLines => "smart",
        StatementSeparators::SemicolonsOnly => "semicolons",
    }
}

fn parse_separators(value: &str) -> StatementSeparators {
    match value {
        "semicolons" => StatementSeparators::SemicolonsOnly,
        _ => StatementSeparators::SemicolonsAndBlankLines,
    }
}

fn statement_separators_item() -> SettingItem {
    SettingItem::new(
        "Statement separators",
        SettingField::dropdown(
            vec![
                (
                    "smart".into(),
                    "Semicolons and blank lines between SELECT queries".into(),
                ),
                ("semicolons".into(), "Semicolons only".into()),
            ],
            |cx: &App| separators_value(Preferences::get(cx).statement_separators).into(),
            |value: SharedString, cx: &mut App| {
                Preferences::update(cx, |p| p.statement_separators = parse_separators(&value));
            },
        )
        .default_value(separators_value(StatementSeparators::default())),
    )
    .description(
        "What ends a statement for Run statement, Explain and Format. Blank lines only split \
         plain SELECT queries that are not valid as one statement; a semicolon always does.",
    )
    .keywords([
        "statement",
        "semicolon",
        "blank line",
        "split",
        "delimiter",
        "run",
    ])
}

fn editor_font_size_item() -> SettingItem {
    let default = f64::from(GuiState::DEFAULT_EDITOR_FONT_SIZE);
    SettingItem::new(
        "Editor font size",
        SettingField::number_input(
            NumberFieldOptions {
                min: f64::from(GuiState::MIN_EDITOR_FONT_SIZE),
                max: f64::from(GuiState::MAX_EDITOR_FONT_SIZE),
                step: 1.0,
            },
            |cx: &App| f64::from(Preferences::get(cx).editor_font_size),
            |value: f64, cx: &mut App| {
                Preferences::update(cx, |p| p.editor_font_size = value as f32);
            },
        )
        .default_value(default),
    )
    .description("Size of SQL text in query tabs.")
    .keywords(["font", "text", "size", "zoom", "sql", "code"])
}

/// A muted explanation in place of a control.
fn note(text: impl Into<SharedString>) -> SettingItem {
    let text: SharedString = text.into();
    SettingItem::render(move |_, _, cx| {
        div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(text.clone())
    })
}

// ----- Config-backed fields -----

fn store(cx: &App) -> ConfigStore {
    AppState::global(cx).read(cx).store().clone()
}

fn notify(cx: &mut App, note: Notification) {
    let window = cx.active_window().or_else(|| cx.windows().first().copied());
    if let Some(window) = window {
        window
            .update(cx, |_, window, cx| window.push_notification(note, cx))
            .ok();
    }
}

// ----- Files and About -----

fn path_item(title: &'static str, path: PathBuf, is_directory: bool) -> SettingItem {
    let shown = path.display().to_string();
    SettingItem::render(move |options: &RenderOptions, _, cx| {
        let reveal_target = path.clone();
        let copy_target = shown.clone();
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().text_sm().child(title))
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(shown.clone()),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("copy-{title}")))
                            .ghost()
                            .with_size(options.size())
                            .icon(IconName::Copy)
                            .tooltip("Copy path")
                            .on_click(move |_, _, cx| {
                                crate::clipboard::write(copy_target.clone(), cx);
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("reveal-{title}")))
                            .outline()
                            .with_size(options.size())
                            .icon(IconName::FolderOpen)
                            .label(if is_directory { "Open" } else { "Reveal" })
                            .on_click(move |_, _, cx| reveal(&reveal_target, is_directory, cx)),
                    ),
            )
    })
    .keywords([title, "file", "folder", "path"])
}

/// Show a file in the OS file manager (or open the directory). A file that
/// does not exist yet reveals its parent directory instead.
fn reveal(path: &Path, is_directory: bool, cx: &mut App) {
    let path = path.to_path_buf();
    cx.spawn(async move |cx| {
        let exists = if is_directory {
            true
        } else {
            let check = path.clone();
            cx.background_spawn(async move { check.exists() }).await
        };
        cx.update(|cx| {
            if is_directory {
                cx.open_with_system(&path);
            } else if exists {
                cx.reveal_path(&path);
            } else if let Some(parent) = path.parent() {
                cx.open_with_system(parent);
            }
        });
    })
    .detach();
}

fn about_row(options: &RenderOptions, cx: &App) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_sm().child("DBCrust desktop"))
                .child(
                    div()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Version {VERSION}")),
                ),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1()
                .child(
                    Button::new("settings-docs")
                        .outline()
                        .with_size(options.size())
                        .icon(IconName::BookOpen)
                        .label("Documentation")
                        .on_click(|_, _, cx| cx.open_url(DOCS_URL)),
                )
                .child(
                    Button::new("settings-github")
                        .outline()
                        .with_size(options.size())
                        .icon(IconName::Github)
                        .label("GitHub")
                        .on_click(|_, _, cx| cx.open_url(GITHUB_URL)),
                )
                .child(
                    Button::new("settings-about")
                        .ghost()
                        .with_size(options.size())
                        .label("About…")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(About), cx)),
                ),
        )
        .into_any_element()
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings")
            .key_context("Settings")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .justify_center()
            .bg(cx.theme().background)
            .child(
                // A ~200 px navigation and a content column that stops
                // instead of stretching across a wide window
                div().h_full().w_full().max_w(SETTINGS_MAX_WIDTH).child(
                    Settings::new(("dbcrust-settings", self.navigation))
                        .default_selected_index(SelectIndex {
                            page_ix: self.selected_page as usize,
                            group_ix: None,
                        })
                        .with_group_variant(GroupBoxVariant::Outline)
                        .sidebar_width(SETTINGS_NAV_WIDTH)
                        .sidebar_size_range(px(160.)..px(280.))
                        .pages(self.pages(cx)),
                ),
            )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;
    use crate::RestoredState;
    use gpui_kit::test::TestWindowExt as _;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    /// Open the main window with `store` and Settings on `page`.
    pub(crate) fn open(
        cx: &mut TestAppContext,
        store: ConfigStore,
        page: SettingsPage,
    ) -> (
        AnyWindowHandle,
        Entity<crate::workspace::Workspace>,
        Entity<SettingsView>,
    ) {
        crate::test_support::isolate();
        cx.executor().allow_parking();
        let (window, workspace) = cx.update(|cx| {
            crate::init(store, cx);
            cx.set_global(RestoredState(GuiState::default()));
            crate::open_main_window(WindowOptions::default(), cx).expect("open window")
        });
        cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| state.open_settings_page(page, cx))
        });
        cx.run_until_parked();
        let settings = cx.update(|cx| workspace.read(cx).settings_view().expect("Settings tab"));
        (window, workspace, settings)
    }

    /// Let background work (and the delayed settings save) finish until
    /// `done` holds.
    pub(crate) fn settle(cx: &mut TestAppContext, done: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            cx.executor().advance_clock(storage::SAVE_DELAY);
            cx.run_until_parked();
            if cx.update(|cx| done(cx)) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background Settings work timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[gpui_kit::test]
    fn every_schema_field_is_on_exactly_one_page(cx: &mut TestAppContext) {
        let (_, _, settings) = open(
            cx,
            ConfigStore::from_config(Config::default_settings()),
            SettingsPage::General,
        );
        let rendered = cx.update(|cx| {
            schema::tests::rendered_by(|| {
                settings.read(cx).pages(cx);
            })
        });
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for path in rendered {
            *counts.entry(path).or_default() += 1;
        }
        for spec in config_editor::schema() {
            assert_eq!(
                counts.remove(spec.path),
                Some(1),
                "`{}` must be on exactly one Settings page (see settings/schema.rs)",
                spec.path
            );
        }
        assert!(counts.is_empty(), "not in the schema: {counts:?}");
    }

    #[gpui_kit::test]
    fn pages_follow_the_navigation_order(cx: &mut TestAppContext) {
        let (_, _, settings) = open(
            cx,
            ConfigStore::from_config(Config::default_settings()),
            SettingsPage::General,
        );
        cx.update(|cx| {
            assert_eq!(
                settings.read(cx).pages(cx).len(),
                SettingsPage::iter().count()
            )
        });
    }

    #[gpui_kit::test]
    fn opening_a_page_selects_it_in_the_open_settings_tab(cx: &mut TestAppContext) {
        let (window, workspace, settings) = open(
            cx,
            ConfigStore::from_config(Config::default_settings()),
            SettingsPage::General,
        );
        cx.update(|cx| {
            AppState::global(cx).update(cx, |state, cx| {
                state.open_settings_page(SettingsPage::SavedQueries, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(workspace.read(cx).settings_view().unwrap(), settings);
            assert_eq!(settings.read(cx).selected_page, SettingsPage::SavedQueries);
        });
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    /// A store whose `config.toml` writes are recorded (the value of
    /// `pager_command` and `default_limit` each time) instead of written.
    fn recording_store() -> (ConfigStore, Arc<Mutex<Vec<(String, usize)>>>) {
        let writes: Arc<Mutex<Vec<(String, usize)>>> = Arc::default();
        let recorded = writes.clone();
        let store = ConfigStore::from_config(Config::default_settings()).with_settings_writer(
            move |config| {
                recorded
                    .lock()
                    .unwrap()
                    .push((config.pager_command.clone(), config.default_limit));
                Ok(())
            },
        );
        (store, writes)
    }

    #[gpui_kit::test]
    fn edits_reach_the_store_at_once_and_disk_once(cx: &mut TestAppContext) {
        let (store, writes) = recording_store();
        let (_, _, _) = open(cx, store.clone(), SettingsPage::Terminal);
        let pager = config_editor::find_spec("pager_command").unwrap();
        cx.update(|cx| {
            // Typing: every keystroke is a value, the space is kept
            for text in ["less", "less ", "less -", "less -S"] {
                storage::write_value(cx, pager, text.to_string());
            }
            assert_eq!(storage::read_value(cx, pager), "less -S");
            // In the store already; config.toml is written after the burst
            assert_eq!(store.read(|config| config.pager_command.clone()), "less -S");
            assert!(store.has_unsaved_settings());
        });
        assert!(
            writes.lock().unwrap().is_empty(),
            "nothing written per keystroke"
        );
        settle(cx, |_| !store.has_unsaved_settings());
        // A few more save delays: nothing else to write
        for _ in 0..3 {
            cx.executor().advance_clock(storage::SAVE_DELAY);
            cx.run_until_parked();
        }
        let writes = writes.lock().unwrap();
        assert_eq!(writes.len(), 1, "{writes:?}");
        assert_eq!(writes[0].0, "less -S");
    }

    #[gpui_kit::test]
    fn a_safety_setting_applies_before_the_next_connection(cx: &mut TestAppContext) {
        let store = ConfigStore::from_config(Config::default_settings());
        let (_, _, _) = open(cx, store.clone(), SettingsPage::Connections);
        let read_only = config_editor::find_spec("read_only_default").unwrap();
        cx.update(|cx| {
            storage::write_value(cx, read_only, "true".into());
            storage::enqueue(
                storage::Edit::Tunnel {
                    old: None,
                    pattern: r"^db\.prod$".into(),
                    target: "deploy@bastion".into(),
                },
                cx,
            )
            .unwrap();
        });
        // No save has run (the clock has not moved): a connection opened now
        // reads its settings from the store and already sees both
        let snapshot = store.snapshot();
        assert!(snapshot.read_only_default);
        assert!(snapshot.ssh_tunnel_patterns.contains_key(r"^db\.prod$"));
    }

    #[gpui_kit::test]
    fn reload_never_saves_over_the_files_it_reads(cx: &mut TestAppContext) {
        let (store, writes) = recording_store();
        let (_, _, _) = open(cx, store.clone(), SettingsPage::Files);
        let limit = config_editor::find_spec("default_limit").unwrap();
        cx.update(|cx| {
            storage::write_value(cx, limit, "321".into());
            storage::reload(cx);
        });
        // The reload ran (no clock advance, so the delayed save did not):
        // it was refused instead of writing this edit over the files first
        cx.run_until_parked();
        assert!(
            writes.lock().unwrap().is_empty(),
            "Reload wrote config.toml before reading it"
        );
        assert!(store.has_unsaved_settings());
        assert_eq!(store.read(|config| config.default_limit), 321);
        settle(cx, |_| !store.has_unsaved_settings());
        assert_eq!(
            writes.lock().unwrap().last().map(|write| write.1),
            Some(321)
        );
    }

    #[gpui_kit::test]
    fn the_quit_flush_writes_what_is_still_pending(cx: &mut TestAppContext) {
        let store = ConfigStore::from_config(Config::default_settings());
        let (_, _, _) = open(cx, store.clone(), SettingsPage::Queries);
        let limit = config_editor::find_spec("default_limit").unwrap();
        let flush = cx.update(|cx| {
            storage::write_value(cx, limit, "222".into());
            flush_on_quit(cx)
        });
        assert!(store.has_unsaved_settings());
        flush(std::time::Instant::now() + std::time::Duration::from_secs(5)).unwrap();
        assert!(!store.has_unsaved_settings());
    }

    #[gpui_kit::test]
    fn invalid_values_are_refused_with_the_schema_message(cx: &mut TestAppContext) {
        let store = ConfigStore::from_config(Config::default_settings());
        let (_, _, _) = open(cx, store.clone(), SettingsPage::Queries);
        cx.update(|cx| {
            let error = storage::enqueue(
                storage::Edit::Value("query_timeout_seconds", "0".into()),
                cx,
            )
            .unwrap_err();
            assert!(error.contains("between"), "{error}");
            let spec = config_editor::find_spec("query_timeout_seconds").unwrap();
            assert_eq!(
                storage::read_value(cx, spec),
                Config::default_settings().query_timeout_seconds.to_string()
            );
        });
    }

    #[test]
    fn separator_values_round_trip() {
        for separators in [
            StatementSeparators::SemicolonsAndBlankLines,
            StatementSeparators::SemicolonsOnly,
        ] {
            assert_eq!(parse_separators(separators_value(separators)), separators);
        }
        assert_eq!(
            parse_separators("garbage"),
            StatementSeparators::SemicolonsAndBlankLines
        );
    }

    #[test]
    fn mode_values_round_trip() {
        for mode in [
            ThemeModeSetting::System,
            ThemeModeSetting::Light,
            ThemeModeSetting::Dark,
        ] {
            assert_eq!(parse_mode(mode_value(mode)), mode);
        }
        assert_eq!(parse_mode("garbage"), ThemeModeSetting::System);
    }
}
