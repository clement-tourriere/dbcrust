//! Desktop-only persisted state, kept apart from DBCrust's `config.toml`
//! (which the CLI also reads): theme, sidebar, unsaved query drafts, the
//! Vault address, the menu bar / tray behaviour. Lives in
//! `<config dir>/gui_state.toml`.
//!
//! Several parts of the app write to it (the workspace saves drafts and
//! layout, the connect page saves the Vault address), so writers use
//! [`GuiState::update`]: it re-reads the file under a process-wide lock,
//! applies the change, and replaces the file atomically. A crash or a full
//! disk can never leave a half-written file behind.

use crate::error::{GuiError, GuiResult};
use crate::launcher::LauncherState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE_NAME: &str = "gui_state.toml";
const HEADER: &str = "# DBCrust desktop app state. Managed automatically: edit theme and\n\
                      # sizes from the app (Settings), not here.\n\n";

/// How the app picks light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeModeSetting {
    /// Follow the operating system's appearance.
    #[default]
    System,
    Light,
    Dark,
}

/// What separates two statements in the query editor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementSeparators {
    /// `;`, and blank lines between plain `SELECT` queries that cannot be
    /// read as one statement.
    #[default]
    SemicolonsAndBlankLines,
    /// `;` only, as in the CLI.
    SemicolonsOnly,
}

/// An unsaved query tab, restored on the next launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub title: String,
    pub sql: String,
    /// `display_url` of the connection the tab was bound to; the draft
    /// re-binds when a connection with the same URL opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_key: Option<String>,
    /// The `.sql` file or saved query the tab saves to with ⌘S.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<DraftTarget>,
}

/// Where a restored tab saves: the `.sql` file or saved query it was loaded
/// from or last saved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftTarget {
    File {
        path: PathBuf,
    },
    SavedQuery {
        name: String,
        /// The engine the query is saved for; `None` for every connection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        engine: Option<dbcrust::database::DatabaseType>,
    },
    /// A saved query of any scope (sessions included), by its storage key.
    SavedQueryKey {
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiState {
    pub theme_mode: ThemeModeSetting,
    /// A bundled or custom theme by name; `None` uses the default for the
    /// current mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme_name: Option<String>,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    /// Base font size in pixels: the whole interface scales with it
    /// (GPUI sizes everything in `rem`).
    pub ui_font_size: f32,
    /// Last Vault address typed on the connect page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault_addr: Option<String>,
    /// Closing the window keeps DBCrust running in the menu bar (macOS) or
    /// the notification area (Windows) instead of quitting. Only honoured
    /// when the platform has a tray.
    pub keep_running_in_tray: bool,
    /// macOS: hide the Dock icon while the window is closed to the menu bar
    /// (off: the Dock icon and ⌘Tab entry stay).
    pub hide_dock_icon_when_closed: bool,
    /// The one-time "DBCrust keeps running in the menu bar" explanation was
    /// answered.
    pub tray_notice_shown: bool,
    /// Reopen the unsaved query tabs of the last session at launch. When
    /// off, drafts are not written to this file at all.
    pub restore_drafts: bool,
    /// Font size of the SQL editor in pixels.
    pub editor_font_size: f32,
    /// What ends a statement in the query editor (Run statement, Explain,
    /// Format and the highlight of the statement under the caret).
    pub statement_separators: StatementSeparators,
    // Arrays of tables must come after plain values in TOML
    pub drafts: Vec<Draft>,
    /// What the Connections page remembers: environment labels and colours,
    /// the last selected item.
    pub launcher: LauncherState,
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            theme_mode: ThemeModeSetting::System,
            theme_name: None,
            sidebar_width: Self::DEFAULT_SIDEBAR_WIDTH,
            sidebar_visible: true,
            ui_font_size: Self::DEFAULT_UI_FONT_SIZE,
            vault_addr: None,
            keep_running_in_tray: true,
            hide_dock_icon_when_closed: false,
            tray_notice_shown: false,
            restore_drafts: true,
            editor_font_size: Self::DEFAULT_EDITOR_FONT_SIZE,
            statement_separators: StatementSeparators::default(),
            drafts: Vec::new(),
            launcher: LauncherState::default(),
        }
    }
}

/// Serializes every read-modify-write of the file inside this process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

impl GuiState {
    pub const DEFAULT_SIDEBAR_WIDTH: f32 = 264.0;
    pub const MIN_SIDEBAR_WIDTH: f32 = 160.0;
    pub const MAX_SIDEBAR_WIDTH: f32 = 800.0;
    pub const DEFAULT_UI_FONT_SIZE: f32 = 16.0;
    pub const MIN_UI_FONT_SIZE: f32 = 12.0;
    pub const MAX_UI_FONT_SIZE: f32 = 22.0;
    /// What the editor used before the size became a setting (GPUI Kit's
    /// monospace size).
    pub const DEFAULT_EDITOR_FONT_SIZE: f32 = 13.0;
    pub const MIN_EDITOR_FONT_SIZE: f32 = 10.0;
    pub const MAX_EDITOR_FONT_SIZE: f32 = 24.0;

    /// `<config dir>/gui_state.toml`.
    pub fn path() -> GuiResult<PathBuf> {
        dbcrust::config::Config::get_config_directory()
            .map(|dir| dir.join(FILE_NAME))
            .map_err(|e| GuiError::Storage(format!("Could not locate the config directory: {e}")))
    }

    /// Load the saved state; a missing or unreadable file yields defaults
    /// (a corrupt file is kept as `gui_state.toml.corrupt` rather than
    /// silently overwritten).
    pub fn load() -> Self {
        match Self::path() {
            Ok(path) => Self::load_from(&path),
            Err(error) => {
                tracing::warn!("{error}");
                Self::default()
            }
        }
    }

    pub fn load_from(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                tracing::warn!("Could not read {}: {error}", path.display());
                return Self::default();
            }
        };
        match toml::from_str::<GuiState>(&text) {
            Ok(state) => state.sanitized(),
            Err(error) => {
                tracing::warn!("Ignoring invalid {}: {error}", path.display());
                let mut backup = path.as_os_str().to_owned();
                backup.push(".corrupt");
                let _ = std::fs::rename(path, backup);
                Self::default()
            }
        }
    }

    /// Replace the file with this state, atomically.
    pub fn save(&self) -> GuiResult<()> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Path) -> GuiResult<()> {
        let storage = |what: &str, error: &dyn std::fmt::Display| {
            GuiError::Storage(format!("Could not {what} {}: {error}", path.display()))
        };
        let body = toml::to_string_pretty(self).map_err(|e| storage("encode", &e))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| storage("create the directory of", &e))?;
        }
        // Atomic, and private (drafts can contain sensitive SQL): DBCrust's
        // writer creates the temp file 0600 before writing
        dbcrust::config::write_file_atomically(path, format!("{HEADER}{body}").as_bytes())
            .map_err(|e| storage("write", &e))
    }

    /// Re-read the file, apply `change`, and save: concurrent writers in
    /// this process never overwrite each other's fields.
    pub fn update(change: impl FnOnce(&mut GuiState)) -> GuiResult<()> {
        Self::update_at(&Self::path()?, change)
    }

    pub fn update_at(path: &Path, change: impl FnOnce(&mut GuiState)) -> GuiResult<()> {
        let _guard = WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = Self::load_from(path);
        change(&mut state);
        state.sanitized().save_to(path)
    }

    /// An editor size inside the supported range (the default for NaN).
    pub fn clamp_editor_font_size(size: f32) -> f32 {
        if size.is_finite() {
            size.clamp(Self::MIN_EDITOR_FONT_SIZE, Self::MAX_EDITOR_FONT_SIZE)
        } else {
            Self::DEFAULT_EDITOR_FONT_SIZE
        }
    }

    /// Clamp values a hand-edited file could break, drop unusable drafts.
    fn sanitized(mut self) -> Self {
        if !self.sidebar_width.is_finite() {
            self.sidebar_width = Self::DEFAULT_SIDEBAR_WIDTH;
        }
        self.sidebar_width = self
            .sidebar_width
            .clamp(Self::MIN_SIDEBAR_WIDTH, Self::MAX_SIDEBAR_WIDTH);
        self.ui_font_size = if self.ui_font_size.is_finite() {
            self.ui_font_size
                .clamp(Self::MIN_UI_FONT_SIZE, Self::MAX_UI_FONT_SIZE)
        } else {
            Self::DEFAULT_UI_FONT_SIZE
        };
        self.editor_font_size = Self::clamp_editor_font_size(self.editor_font_size);
        self.drafts.retain(|draft| !draft.sql.trim().is_empty());
        self.launcher = std::mem::take(&mut self.launcher).sanitized();
        self.vault_addr = self.vault_addr.filter(|addr| !addr.trim().is_empty());
        self.theme_name = self.theme_name.filter(|name| !name.trim().is_empty());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> GuiState {
        GuiState {
            theme_mode: ThemeModeSetting::Dark,
            theme_name: Some("Tokyo Night".into()),
            sidebar_width: 300.0,
            sidebar_visible: false,
            ui_font_size: 14.0,
            vault_addr: Some("https://vault.example.com:8200".into()),
            keep_running_in_tray: false,
            hide_dock_icon_when_closed: true,
            tray_notice_shown: true,
            restore_drafts: false,
            editor_font_size: 15.0,
            statement_separators: StatementSeparators::SemicolonsOnly,
            drafts: vec![
                Draft {
                    title: "Query 1".into(),
                    sql: "SELECT 1;\n-- \"quoted\" and 'single'\nSELECT 2".into(),
                    connection_key: Some("postgres://app@db.internal:5432/shop".into()),
                    target: Some(DraftTarget::SavedQuery {
                        name: "daily".into(),
                        engine: Some(dbcrust::database::DatabaseType::PostgreSQL),
                    }),
                },
                Draft {
                    title: "Scratch".into(),
                    sql: "select now()".into(),
                    connection_key: None,
                    target: Some(DraftTarget::File {
                        path: "/tmp/scratch.sql".into(),
                    }),
                },
                Draft {
                    title: "Health".into(),
                    sql: "SELECT 1".into(),
                    connection_key: None,
                    target: Some(DraftTarget::SavedQueryKey {
                        key: "session::warehouse::health".into(),
                    }),
                },
            ],
            launcher: LauncherState {
                last_selected: Some("session:prod".into()),
                entries: vec![crate::launcher::ConnectionMeta {
                    key: "session:prod".into(),
                    environment: "Production".into(),
                    color: Some(crate::launcher::EnvColor::Red),
                    read_only: true,
                }],
            },
        }
    }

    #[test]
    fn launcher_metadata_survives_a_round_trip_and_older_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        sample().save_to(&path).expect("save");
        let loaded = GuiState::load_from(&path);
        assert_eq!(loaded.launcher, sample().launcher);
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("[launcher]"), "{text}");

        // A file written before the Connections page kept anything
        std::fs::write(&path, "theme_mode = \"light\"\n").expect("write");
        assert_eq!(
            GuiState::load_from(&path).launcher,
            LauncherState::default()
        );
        // ...and a write by another part of the app keeps the metadata
        sample().save_to(&path).expect("save");
        GuiState::update_at(&path, |state| state.sidebar_visible = true).expect("update");
        assert_eq!(GuiState::load_from(&path).launcher, sample().launcher);
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        sample().save_to(&path).expect("save");
        assert_eq!(GuiState::load_from(&path), sample());
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            GuiState::load_from(&dir.path().join("nope.toml")),
            GuiState::default()
        );
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "theme_mode = \"light\"\nfuture_setting = 42\n\n[[drafts]]\nsql = \"select 1\"\n",
        )
        .expect("write");
        let state = GuiState::load_from(&path);
        assert_eq!(state.theme_mode, ThemeModeSetting::Light);
        assert!(state.sidebar_visible);
        assert_eq!(state.sidebar_width, GuiState::DEFAULT_SIDEBAR_WIDTH);
        assert_eq!(state.drafts.len(), 1);
        assert_eq!(state.drafts[0].title, "");
    }

    #[test]
    fn tray_settings_default_on_and_notice_unseen() {
        let state = GuiState::default();
        assert!(state.keep_running_in_tray);
        assert!(!state.hide_dock_icon_when_closed, "the Dock icon stays");
        assert!(!state.tray_notice_shown);
        assert!(state.restore_drafts);
        assert_eq!(state.editor_font_size, GuiState::DEFAULT_EDITOR_FONT_SIZE);
    }

    #[test]
    fn files_without_tray_settings_load_with_the_defaults() {
        // A file written before the tray existed
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "theme_mode = \"dark\"\nsidebar_width = 300.0\nsidebar_visible = false\n\n\
             [[drafts]]\nsql = \"select 1\"\n",
        )
        .expect("write");
        let state = GuiState::load_from(&path);
        assert!(state.keep_running_in_tray);
        assert!(!state.hide_dock_icon_when_closed);
        assert!(!state.tray_notice_shown);
        assert!(state.restore_drafts);
        assert_eq!(state.editor_font_size, GuiState::DEFAULT_EDITOR_FONT_SIZE);
        // ...and nothing else was disturbed
        assert_eq!(state.theme_mode, ThemeModeSetting::Dark);
        assert!(!state.sidebar_visible);
        assert_eq!(state.drafts.len(), 1);
    }

    #[test]
    fn tray_settings_survive_a_round_trip_and_partial_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "keep_running_in_tray = false\n").expect("write");
        let state = GuiState::load_from(&path);
        assert!(!state.keep_running_in_tray);
        // Only the key that was present changed
        assert!(!state.hide_dock_icon_when_closed);

        GuiState::update_at(&path, |state| state.tray_notice_shown = true).expect("update");
        let state = GuiState::load_from(&path);
        assert!(!state.keep_running_in_tray, "other fields are kept");
        assert!(state.tray_notice_shown);
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("keep_running_in_tray = false"));
        assert!(text.contains("tray_notice_shown = true"));
    }

    #[test]
    fn statement_separators_default_to_smart_and_survive_old_files() {
        assert_eq!(
            GuiState::default().statement_separators,
            StatementSeparators::SemicolonsAndBlankLines
        );
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        // A file written before the setting existed
        std::fs::write(&path, "sidebar_visible = false\n").expect("write");
        assert_eq!(
            GuiState::load_from(&path).statement_separators,
            StatementSeparators::SemicolonsAndBlankLines
        );
        GuiState::update_at(&path, |state| {
            state.statement_separators = StatementSeparators::SemicolonsOnly
        })
        .expect("update");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("statement_separators = \"semicolons_only\""));
        assert_eq!(
            GuiState::load_from(&path).statement_separators,
            StatementSeparators::SemicolonsOnly
        );
    }

    #[test]
    fn editor_font_size_is_clamped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        for (written, expected) in [
            ("2.0", GuiState::MIN_EDITOR_FONT_SIZE),
            ("200.0", GuiState::MAX_EDITOR_FONT_SIZE),
            ("nan", GuiState::DEFAULT_EDITOR_FONT_SIZE),
            ("16.0", 16.0),
        ] {
            std::fs::write(&path, format!("editor_font_size = {written}\n")).expect("write");
            assert_eq!(GuiState::load_from(&path).editor_font_size, expected);
        }
    }

    #[test]
    fn corrupt_file_is_kept_aside_and_defaults_returned() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "theme_mode = [not toml").expect("write");
        assert_eq!(GuiState::load_from(&path), GuiState::default());
        assert!(!path.exists());
        assert!(dir.path().join("gui_state.toml.corrupt").exists());
    }

    #[test]
    fn sanitizes_hand_edited_values() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "sidebar_width = 99999.0\nui_font_size = 3.0\nvault_addr = \"  \"\ntheme_name = \"\"\n\n\
             [[drafts]]\ntitle = \"empty\"\nsql = \"  \\n\"\n",
        )
        .expect("write");
        let state = GuiState::load_from(&path);
        assert_eq!(state.sidebar_width, GuiState::MAX_SIDEBAR_WIDTH);
        assert_eq!(state.ui_font_size, GuiState::MIN_UI_FONT_SIZE);
        assert_eq!(state.vault_addr, None);
        assert_eq!(state.theme_name, None);
        assert!(state.drafts.is_empty());
    }

    #[test]
    fn save_replaces_atomically_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        sample().save_to(&path).expect("first save");
        GuiState::default().save_to(&path).expect("second save");
        assert_eq!(GuiState::load_from(&path), GuiState::default());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn update_keeps_other_writers_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        GuiState::update_at(&path, |state| {
            state.vault_addr = Some("https://v:8200".into())
        })
        .expect("vault");
        GuiState::update_at(&path, |state| state.sidebar_visible = false).expect("sidebar");
        let state = GuiState::load_from(&path);
        assert_eq!(state.vault_addr.as_deref(), Some("https://v:8200"));
        assert!(!state.sidebar_visible);
    }

    #[test]
    fn file_has_a_header_and_is_readable_toml() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        sample().save_to(&path).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.starts_with("# DBCrust desktop app state"));
        assert!(text.contains("[[drafts]]"));
        assert!(text.contains("theme_mode = \"dark\""));
    }
}
