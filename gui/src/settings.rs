//! Settings tab: desktop preferences (closing the window, startup, theme,
//! editor font size) and the DBCrust settings that matter in the app, edited
//! through the same schema as the CLI's `\config` (`dbcrust::config_editor`)
//! so labels, ranges and validation are shared.
//!
//! Every change is applied immediately. DBCrust settings go through
//! `ConfigStore::update` and the documented `config.toml` is saved in the
//! background (comments are preserved); desktop preferences live in
//! [`Preferences`] and `gui_state.toml`.

use crate::about::{DOCS_URL, GITHUB_URL, VERSION};
use crate::actions::About;
use crate::app_state::AppState;
use crate::preferences::Preferences;
use crate::theme;
use crate::tray::{self, Availability, Wording};
use dbcrust_gui_core::dbcrust::config::Config;
use dbcrust_gui_core::dbcrust::config_editor::{self, FieldKind, FieldSpec};
use dbcrust_gui_core::gui_state::{GuiState, StatementSeparators, ThemeModeSetting};
use dbcrust_gui_core::{ConfigStore, GuiResult};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, ThemeMode, WindowExt as _,
    button::{Button, ButtonVariants as _},
    group_box::GroupBoxVariant,
    h_flex,
    notification::Notification,
    setting::{
        NumberFieldOptions, RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings,
    },
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Width of the page list.
const SETTINGS_NAV_WIDTH: Pixels = px(200.);
/// Navigation plus a 720 px content column.
const SETTINGS_MAX_WIDTH: Pixels = px(920.);

/// Sentinel dropdown value for "no custom theme".
const DEFAULT_THEME_OPTION: &str = "default";

pub struct SettingsView {
    focus_handle: FocusHandle,
    /// `Config::default()`, the target of every reset button.
    defaults: Rc<Config>,
}

impl SettingsView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            defaults: Rc::new(Config::default()),
        }
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

        SettingPage::new("General")
            .icon(Icon::new(IconName::Settings))
            .description("Closing the window and starting up.")
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

        SettingPage::new("Appearance")
            .icon(Icon::new(IconName::Palette))
            .description("Theme and text size.")
            .group(
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
        let defaults = &self.defaults;
        SettingPage::new("Queries")
            .icon(Icon::new(IconName::SquareTerminal))
            .description("Execution and the SQL editor.")
            .group(
                SettingGroup::new().title("Execution").items(
                    [
                        number_item(
                            "default_limit",
                            "Default row limit",
                            "Rows fetched by a SELECT without its own LIMIT. 0 turns the limit off.",
                            defaults,
                        ),
                        number_item(
                            "query_timeout_seconds",
                            "Query timeout",
                            "Seconds before a running statement is stopped.",
                            defaults,
                        ),
                        number_item(
                            "metadata_timeout_seconds",
                            "Schema timeout",
                            "Seconds allowed to load tables, columns and completions.",
                            defaults,
                        ),
                    ]
                    .into_iter()
                    .flatten(),
                )
                .item(statement_separators_item()),
            )
            .group(
                SettingGroup::new()
                    .title("Editor")
                    .item(editor_font_size_item())
                    .items(bool_item(
                        "autocomplete_enabled",
                        "SQL completion",
                        "Suggest keywords, tables and columns while typing.",
                        defaults,
                    )),
            )
    }

    fn connections_page(&self) -> SettingPage {
        let defaults = &self.defaults;
        SettingPage::new("Connections")
            .icon(Icon::new(IconName::Database))
            .description("Defaults for new connections.")
            .group(SettingGroup::new().title("Safety").items(bool_item(
                "read_only_default",
                "Open connections read-only",
                "Block statements that change data on new connections. Turn it off per \
                     connection when you need to write.",
                defaults,
            )))
            .group(SettingGroup::new().title("History").items(number_item(
                "max_recent_connections",
                "Recent connections",
                "How many recent connections to remember.",
                defaults,
            )))
            .group(
                SettingGroup::new().title("Vault").items(
                    [
                        bool_item(
                            "vault_credential_cache_enabled",
                            "Cache Vault credentials",
                            "Reuse short-lived credentials until they expire.",
                            defaults,
                        ),
                        text_item(
                            "vault_credential_process",
                            "Credential process",
                            "Command that prints a Vault token or credentials when a vault:// \
                                 connection opens.",
                            defaults,
                        ),
                    ]
                    .into_iter()
                    .flatten(),
                ),
            )
    }

    fn files_page(&self) -> SettingPage {
        let directory = Config::get_config_directory().ok();
        let files: Vec<(&'static str, &'static str, Option<PathBuf>)> = vec![
            (
                "Settings",
                "config.toml",
                Config::get_config_file_path().ok(),
            ),
            (
                "Saved sessions",
                "sessions.toml",
                Config::get_saved_sessions_path().ok(),
            ),
            (
                "Named queries",
                "named_queries.toml",
                Config::get_named_queries_path().ok(),
            ),
            (
                "Recent connections",
                "recent.toml",
                Config::get_recent_connections_path().ok(),
            ),
            ("Desktop state", "gui_state.toml", GuiState::path().ok()),
        ];

        let mut storage = SettingGroup::new()
            .title("Storage")
            .description("Plain files you can back up or edit. The CLI uses the same ones.");
        if let Some(directory) = directory {
            storage = storage.item(path_item("Config directory", directory.clone(), true));
            for (title, file, path) in files {
                let path = path.unwrap_or_else(|| directory.join(file));
                storage = storage.item(path_item(title, path, false));
            }
        } else {
            storage = storage.item(SettingItem::render(|_, _, cx| {
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child("The config directory could not be located.")
            }));
        }

        SettingPage::new("Files")
            .icon(Icon::new(IconName::FolderOpen))
            .resettable(false)
            .group(storage)
            .group(
                SettingGroup::new()
                    .title("About")
                    .item(SettingItem::render(|options, _, cx| about_row(options, cx))),
            )
    }
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

fn read_value(cx: &App, spec: &'static FieldSpec) -> String {
    store(cx).read(|config| (spec.get)(config))
}

/// Apply `value` to the config through the shared schema (validated), then
/// save and re-apply runtime settings. Failures are shown as notifications.
fn write_value(cx: &mut App, spec: &'static FieldSpec, value: String) {
    let store = store(cx);
    let applied = store.update_settings(|config| {
        config_editor::apply_value(config, spec.path, &value).map(|_| ())
    });
    if let Err(error) = applied {
        notify(
            cx,
            Notification::error(error).title(format!("Couldn’t change {}", spec.label)),
        );
        return;
    }
    config_editor::reapply_runtime_settings(&store.snapshot());
    cx.spawn(async move |cx| {
        let saved: GuiResult<()> = cx
            .background_spawn(async move { store.save_settings() })
            .await;
        if let Err(error) = saved {
            cx.update(|cx| {
                notify(
                    cx,
                    Notification::error(error.to_string()).title("Couldn’t save settings"),
                );
            });
        }
    })
    .detach();
}

fn notify(cx: &mut App, note: Notification) {
    let window = cx.active_window().or_else(|| cx.windows().first().copied());
    if let Some(window) = window {
        window
            .update(cx, |_, window, cx| window.push_notification(note, cx))
            .ok();
    }
}

fn bool_item(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    defaults: &Rc<Config>,
) -> Option<SettingItem> {
    let spec = config_editor::find_spec(key)?;
    let default = (spec.get)(defaults) == "true";
    Some(
        SettingItem::new(
            title,
            SettingField::switch(
                move |cx: &App| read_value(cx, spec) == "true",
                move |value: bool, cx: &mut App| write_value(cx, spec, value.to_string()),
            )
            .default_value(default),
        )
        .description(description)
        .keywords([spec.path, spec.label]),
    )
}

fn number_item(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    defaults: &Rc<Config>,
) -> Option<SettingItem> {
    let spec = config_editor::find_spec(key)?;
    let FieldKind::UInt { min, max } = spec.kind else {
        return None;
    };
    let default = (spec.get)(defaults).parse::<f64>().unwrap_or(min as f64);
    Some(
        SettingItem::new(
            title,
            SettingField::number_input(
                NumberFieldOptions {
                    min: min as f64,
                    max: max as f64,
                    step: 1.0,
                },
                move |cx: &App| read_value(cx, spec).parse::<f64>().unwrap_or(default),
                move |value: f64, cx: &mut App| {
                    write_value(cx, spec, format!("{}", value.round().max(0.0) as u64));
                },
            )
            .default_value(default),
        )
        .description(description)
        .keywords([spec.path, spec.label]),
    )
}

fn text_item(
    key: &'static str,
    title: &'static str,
    description: &'static str,
    defaults: &Rc<Config>,
) -> Option<SettingItem> {
    let spec = config_editor::find_spec(key)?;
    let default = (spec.get)(defaults);
    Some(
        SettingItem::new(
            title,
            SettingField::input(
                move |cx: &App| SharedString::from(read_value(cx, spec)),
                move |value: SharedString, cx: &mut App| write_value(cx, spec, value.to_string()),
            )
            .default_value(default),
        )
        .layout(Axis::Vertical)
        .description(description)
        .keywords([spec.path, spec.label]),
    )
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
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    copy_target.clone(),
                                ));
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
fn reveal(path: &Path, is_directory: bool, cx: &App) {
    if is_directory {
        cx.open_with_system(path);
    } else if path.exists() {
        cx.reveal_path(path);
    } else if let Some(parent) = path.parent() {
        cx.open_with_system(parent);
    }
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
                // A ~200 px navigation and a content column that stops at
                // ~720 px instead of stretching across a wide window
                div().h_full().w_full().max_w(SETTINGS_MAX_WIDTH).child(
                    Settings::new("dbcrust-settings")
                        .with_group_variant(GroupBoxVariant::Outline)
                        .sidebar_width(SETTINGS_NAV_WIDTH)
                        .sidebar_size_range(px(160.)..px(280.))
                        .pages([
                            self.general_page(cx),
                            self.appearance_page(cx),
                            self.queries_page(),
                            self.connections_page(),
                            self.files_page(),
                        ]),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn every_setting_key_exists_in_the_shared_schema() {
        for key in [
            "default_limit",
            "query_timeout_seconds",
            "metadata_timeout_seconds",
            "autocomplete_enabled",
            "read_only_default",
            "max_recent_connections",
            "vault_credential_cache_enabled",
            "vault_credential_process",
        ] {
            assert!(config_editor::find_spec(key).is_some(), "{key} missing");
        }
    }

    #[test]
    fn numeric_settings_are_unsigned_integers() {
        for key in [
            "default_limit",
            "query_timeout_seconds",
            "max_recent_connections",
        ] {
            let spec = config_editor::find_spec(key).expect("spec");
            assert!(matches!(spec.kind, FieldKind::UInt { .. }), "{key}");
        }
    }

    #[test]
    fn writing_through_the_schema_validates_ranges() {
        let mut config = Config::default();
        assert!(config_editor::apply_value(&mut config, "query_timeout_seconds", "45").is_ok());
        assert_eq!(config.query_timeout_seconds, 45);
        assert!(config_editor::apply_value(&mut config, "query_timeout_seconds", "0").is_err());
        assert_eq!(config.query_timeout_seconds, 45);
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
