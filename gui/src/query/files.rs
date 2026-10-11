//! `.sql` files and named queries: open a file into the tab, save the tab to
//! a file, save the buffer as a named query, and load a named query. File
//! I/O runs on background threads.
//!
//! A tab that was loaded from, or saved to, a file or a saved query is bound
//! to it: ⌘S writes there, and the tab shows as modified while its text
//! differs from what was last loaded or saved (`Baseline`). A tab with no
//! target is a draft (autosave keeps it); ⌘S asks where to save it.

use super::dialogs::{FileHandler, SaveQueryForm, SubmitHandler};
use super::{QueryTab, QueryTabEvent};
use crate::app_state::AppState;
use dbcrust_gui_core::gui_state::DraftTarget;
use dbcrust_gui_core::{DatabaseType, DatabaseTypeExt as _};
use gpui_kit::component::{WindowExt as _, dialog::DialogButtonProps, notification::Notification};
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Orders saves to the same file or saved query, across tabs: each save
/// takes a number when it is submitted, and one that runs after a newer save
/// of the same target already landed is skipped, so the target always ends
/// up with the newest text whatever order the background saves run in.
static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static NEWEST_SAVE: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

fn next_save() -> u64 {
    SAVE_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1
}

/// Run `save` unless a newer save of `key` already landed: true when it ran.
/// Saves of one key run one at a time.
fn save_in_order<E>(
    key: String,
    sequence: u64,
    save: impl FnOnce() -> Result<(), E>,
) -> Result<bool, E> {
    let mut newest = NEWEST_SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let newest = newest.get_or_insert_with(HashMap::new);
    if newest.get(&key).is_some_and(|&landed| landed > sequence) {
        return Ok(false);
    }
    save()?;
    newest.insert(key, sequence);
    Ok(true)
}

/// Write `text` unless a newer save to `path` already did: true when written.
fn write_in_order(path: &Path, text: &str, sequence: u64) -> Result<bool, String> {
    save_in_order(format!("file:{}", path.display()), sequence, || {
        std::fs::write(path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
    })
}

/// Files larger than this are refused: a SQL editor is not a text viewer.
const MAX_SQL_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// What a tab's text is saved to.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SaveTarget {
    File(PathBuf),
    /// A saved query; `scope` is `None` when it is available to every
    /// connection, else the engine it is saved for.
    Named {
        name: String,
        scope: Option<DatabaseType>,
    },
}

/// A save target and the text it held when it was last loaded or saved.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Baseline {
    target: SaveTarget,
    text: String,
}

impl Baseline {
    pub(super) fn new(target: SaveTarget, text: String) -> Self {
        Self { target, text }
    }

    pub(super) fn target(&self) -> &SaveTarget {
        &self.target
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn file_path(&self) -> Option<&Path> {
        match &self.target {
            SaveTarget::File(path) => Some(path),
            SaveTarget::Named { .. } => None,
        }
    }

    pub(super) fn describe(&self) -> String {
        match &self.target {
            SaveTarget::File(path) => path.display().to_string(),
            SaveTarget::Named { name, scope } => format!(
                "Saved query “{name}” · {}",
                scope
                    .as_ref()
                    .map_or("every connection", DatabaseType::display_name)
            ),
        }
    }
}

impl QueryTab {
    /// Ask for a `.sql` file and load it into this tab.
    pub fn open_sql_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = path.clone();
            let content = cx
                .background_spawn(async move { read_sql_file(&read) })
                .await;
            this.update_in(cx, |tab, window, cx| match content {
                Ok(text) => tab.loaded_file(path, text, window, cx),
                Err(message) => window.push_notification(
                    Notification::error(message).title("Couldn’t open the file"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn loaded_file(
        &mut self,
        path: PathBuf,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_text(text.clone(), window, cx);
        // Loading replaces the content, so it also replaces a user title
        self.set_title(file_name(&path), cx);
        self.bind(SaveTarget::File(path), text, cx);
    }

    /// The title rule when a save lands (see also `loaded_file` and
    /// `apply_named_query`, which always title the tab after what they load):
    /// saving to the target the tab is already bound to keeps the title, so a
    /// name the user gave the tab with "Rename tab" survives ⌘S; saving to a
    /// different target (first save, "Save as…") titles the tab after it,
    /// like opening it would.
    fn title_after_save(&mut self, name: String, target: &SaveTarget, cx: &mut Context<Self>) {
        let same_target = self
            .baseline
            .as_ref()
            .is_some_and(|baseline| baseline.target() == target);
        if !same_target {
            self.set_title(name, cx);
        }
    }

    /// The tab's text is now what `target` holds.
    pub(super) fn bind(&mut self, target: SaveTarget, text: String, cx: &mut Context<Self>) {
        self.target_generation += 1;
        self.baseline = Some(Baseline::new(target, text));
        self.refresh_modified(cx);
        // The tab's tooltip names the target
        cx.emit(QueryTabEvent::TitleChanged);
        cx.notify();
    }

    /// Start a background load, save or restore that may bind the tab: it
    /// supersedes those still running. Bind on completion only while
    /// `still_current` holds.
    fn begin_rebind(&mut self) -> u64 {
        self.target_generation += 1;
        self.target_generation
    }

    fn still_current(&self, generation: u64) -> bool {
        self.target_generation == generation
    }

    /// The file or saved query ⌘S writes to, kept with the tab's draft.
    pub fn draft_target(&self) -> Option<DraftTarget> {
        self.baseline
            .as_ref()
            .map(|baseline| match baseline.target() {
                SaveTarget::File(path) => DraftTarget::File { path: path.clone() },
                SaveTarget::Named { name, scope } => DraftTarget::SavedQuery {
                    name: name.clone(),
                    engine: scope.clone(),
                },
            })
    }

    /// Bind a restored draft to the file or saved query it saved to, as it
    /// is now; the draft keeps its own text and shows as modified when they
    /// differ. A file or saved query that no longer exists leaves the tab a
    /// plain draft.
    pub fn restore_target(
        &mut self,
        target: DraftTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            DraftTarget::File { path } => {
                let read = path.clone();
                let generation = self.begin_rebind();
                let reading = cx.background_spawn(async move { read_sql_file(&read) });
                cx.spawn_in(window, async move |this, cx| match reading.await {
                    Ok(text) => {
                        this.update(cx, |tab, cx| {
                            // Opened or saved elsewhere meanwhile: that wins
                            if tab.still_current(generation) {
                                tab.bind(SaveTarget::File(path), text, cx);
                            }
                        })
                        .ok();
                    }
                    Err(message) => tracing::info!("restored tab left unbound: {message}"),
                })
                .detach();
            }
            DraftTarget::SavedQuery { name, engine } => {
                let store = AppState::global(cx).read(cx).store().clone();
                if let Some(saved) = store.named_query(&name, engine.as_ref()) {
                    // Saved queries are stored trimmed: compare like ⌘S does
                    let buffer = self.sql(cx);
                    let text = if buffer.trim() == saved.trim() {
                        buffer
                    } else {
                        saved
                    };
                    self.bind(
                        SaveTarget::Named {
                            name,
                            scope: engine,
                        },
                        text,
                        cx,
                    );
                }
            }
        }
    }

    /// ⌘S: write to the file or saved query this tab is bound to; a tab
    /// with no target opens the save dialog.
    pub fn save_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .baseline
            .as_ref()
            .map(|baseline| baseline.target().clone());
        match target {
            Some(SaveTarget::File(path)) => {
                let text = self.sql(cx);
                self.write_file(path, text, window, cx);
            }
            Some(SaveTarget::Named { name, scope }) => {
                let buffer = self.sql(cx);
                let sql = buffer.trim().to_string();
                if sql.is_empty() {
                    window.push_notification(
                        Notification::info("Write a query first.").title("Nothing to save"),
                        cx,
                    );
                    return;
                }
                self.store_named_query(name, scope, sql, Some(buffer), window, cx);
            }
            None => self.save_as(window, cx),
        }
    }

    /// ⇧⌘S and a new tab's ⌘S: ask where to save (a saved query, or a file).
    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_named_query_dialog(window, cx);
    }

    /// Write the buffer to a `.sql` file: ask for a path when `always_ask`
    /// or when the tab has no file yet.
    pub fn save_sql_file(&mut self, always_ask: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.sql(cx);
        match self
            .file_path()
            .map(Path::to_path_buf)
            .filter(|_| !always_ask)
        {
            Some(path) => self.write_file(path, text, window, cx),
            None => {
                let directory = self
                    .file_path()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("."));
                let suggested = suggested_file_name(&self.title);
                let picked = cx.prompt_for_new_path(&directory, Some(&suggested));
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(path))) = picked.await else {
                        return;
                    };
                    this.update_in(cx, |tab, window, cx| tab.write_file(path, text, window, cx))
                        .ok();
                })
                .detach();
            }
        }
    }

    pub(super) fn write_file(
        &mut self,
        path: PathBuf,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = path.clone();
        let written = text.clone();
        let generation = self.begin_rebind();
        let sequence = next_save();
        let writing = cx.background_spawn(async move { write_in_order(&target, &text, sequence) });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = writing.await;
            this.update_in(cx, |tab, window, cx| match outcome {
                // A newer save of this file landed first: it reports itself
                Ok(false) => {}
                Ok(true) => {
                    let name = file_name(&path);
                    // A save that finishes after the tab moved on (opened,
                    // saved or restored elsewhere since) doesn't take it back
                    if tab.still_current(generation) {
                        let target = SaveTarget::File(path);
                        tab.title_after_save(name.clone(), &target, cx);
                        tab.bind(target, written, cx);
                    }
                    window.push_notification(Notification::success(format!("Saved {name}")), cx);
                }
                Err(message) => window.push_notification(
                    Notification::error(message).title("Couldn’t save the file"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    // ----- Named queries -----

    /// The selection if any, else the whole buffer.
    fn text_to_save(&self, cx: &App) -> String {
        let editor = self.editor.read(cx);
        let selected = editor.selected_value().to_string();
        if selected.trim().is_empty() {
            editor.text().to_string().trim().to_string()
        } else {
            selected.trim().to_string()
        }
    }

    /// Open the "Save query" dialog.
    pub fn save_named_query_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sql = self.text_to_save(cx);
        if sql.is_empty() {
            window.push_notification(
                Notification::info("Write a query first.").title("Nothing to save"),
                cx,
            );
            return;
        }
        // Saving the whole editor binds the tab to the saved query; saving a
        // selection is just a snippet and leaves the tab alone
        let buffer = self.sql(cx);
        let binds = (sql == buffer.trim()).then_some(buffer);
        let engine = self
            .summary(cx)
            .map(|s| s.type_name().to_string())
            .unwrap_or_else(|| "Database".to_string());
        let initial = if is_default_title(&self.title) {
            String::new()
        } else {
            self.title.to_string()
        };
        let view = cx.entity().downgrade();
        let submit: SubmitHandler = Rc::new({
            let sql = sql.clone();
            let view = view.clone();
            move |name, global, window, cx| {
                let name = name.trim().to_string();
                if name.is_empty() {
                    window.push_notification(
                        Notification::warning("Give the query a name.").title("Name required"),
                        cx,
                    );
                    return false;
                }
                let (sql, binds) = (sql.clone(), binds.clone());
                view.update(cx, |tab, cx| {
                    tab.save_named_query(name, global, sql, binds, window, cx)
                })
                .unwrap_or(false)
            }
        });
        let to_file: FileHandler = Rc::new(move |window, cx| {
            view.update(cx, |tab, cx| tab.save_sql_file(true, window, cx))
                .ok();
        });
        let form =
            cx.new(|cx| SaveQueryForm::new(initial, engine, submit.clone(), to_file, window, cx));
        let confirm = form.clone();
        let content = form.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let confirm = confirm.clone();
            let submit = submit.clone();
            dialog
                .title("Save query")
                .w(px(440.))
                .child(content.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Save")
                        .show_cancel(true)
                        .on_ok(move |_, window, cx| {
                            let (name, global) = {
                                let form = confirm.read(cx);
                                (form.name(cx), form.is_global())
                            };
                            submit(name, global, window, cx)
                        }),
                )
        });
        window.defer(cx, move |window, cx| {
            form.update(cx, |form, cx| form.focus(window, cx));
        });
    }

    /// Save from the dialog: false (dialog stays open) when the scope cannot
    /// be resolved.
    fn save_named_query(
        &mut self,
        name: String,
        global: bool,
        sql: String,
        binds: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let scope = if global {
            None
        } else {
            match self.summary(cx) {
                Some(summary) => Some(summary.db_type),
                None => {
                    self.warn(
                        "No connection",
                        "Choose a connection to save this query for its engine, or make it available for every connection.",
                        window,
                        cx,
                    );
                    return false;
                }
            }
        };
        self.store_named_query(name, scope, sql, binds, window, cx);
        true
    }

    /// Store the query (in a background task). When `binds` holds the
    /// buffer text, the tab becomes the saved query's editor.
    fn store_named_query(
        &mut self,
        name: String,
        scope: Option<DatabaseType>,
        sql: String,
        binds: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_state = AppState::global(cx);
        let store = app_state.read(cx).store().clone();
        let saved_name = name.clone();
        let saved_scope = scope.clone();
        let generation = self.begin_rebind();
        let sequence = next_save();
        let key = format!("query:{saved_scope:?}:{saved_name}");
        let saving = cx.background_spawn(async move {
            save_in_order(key, sequence, || {
                store.save_named_query(&saved_name, &sql, saved_scope)
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = saving.await;
            this.update_in(cx, |tab, window, cx| match outcome {
                // A newer save of this query landed first: it reports itself
                Ok(false) => {}
                Ok(true) => {
                    app_state.update(cx, |state, cx| state.storage_changed(cx));
                    if let Some(text) = binds.filter(|_| tab.still_current(generation)) {
                        let target = SaveTarget::Named {
                            name: name.clone(),
                            scope,
                        };
                        tab.title_after_save(name.clone(), &target, cx);
                        tab.bind(target, text, cx);
                    }
                    window.push_notification(
                        Notification::success(format!("Saved “{name}”")).title("Query saved"),
                        cx,
                    );
                }
                Err(error) => window.push_notification(
                    Notification::error(error.to_string()).title("Couldn’t save the query"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// A saved query was picked in the toolbar: open it in an empty tab
    /// (which then saves back to it), or insert it at the caret when the
    /// editor already holds something. `scope_label` is the store's label
    /// for where the query is available ("global", an engine name, …).
    pub(super) fn apply_named_query(
        &mut self,
        name: String,
        sql: String,
        scope_label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_blank(cx) {
            let scope = self.named_scope(scope_label, cx);
            self.load_text(sql.clone(), window, cx);
            self.set_title(name.clone(), cx);
            if let Some(scope) = scope {
                self.bind(SaveTarget::Named { name, scope }, sql, cx);
            }
            cx.emit(QueryTabEvent::StatusChanged);
        } else {
            let separator = if sql.ends_with('\n') { "" } else { "\n" };
            self.insert_text(format!("{sql}{separator}"), window, cx);
        }
    }

    /// The scope a saved query can be written back to: `Some(None)` for
    /// global queries, `Some(Some(engine))` when it belongs to this tab's
    /// engine, `None` for anything else (such as session scopes).
    fn named_scope(&self, scope_label: &str, cx: &App) -> Option<Option<DatabaseType>> {
        if scope_label == "global" {
            return Some(None);
        }
        let summary = self.summary(cx)?;
        (summary.type_name() == scope_label).then_some(Some(summary.db_type))
    }
}

fn read_sql_file(path: &Path) -> Result<String, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?
        .len();
    if size > MAX_SQL_FILE_BYTES {
        return Err(format!(
            "{} is {} MB; the editor opens files up to {} MB.",
            file_name(path),
            size / (1024 * 1024),
            MAX_SQL_FILE_BYTES / (1024 * 1024)
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("Could not read {}: {e}", path.display()))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `Query 3` and friends are placeholders, not names worth keeping.
fn is_default_title(title: &str) -> bool {
    title == "Query"
        || title
            .strip_prefix("Query ")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn suggested_file_name(title: &str) -> String {
    let stem: String = title
        .trim_end_matches(".sql")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim();
    format!("{}.sql", if stem.is_empty() { "query" } else { stem })
}

#[cfg(test)]
mod tests {
    // Explicit imports: see the note in `editor.rs` tests
    use super::{
        is_default_title, read_sql_file, save_in_order, suggested_file_name, write_in_order,
    };
    use dbcrust_gui_core::completion::leading_keyword;
    use std::path::Path;

    use super::SaveTarget;
    use dbcrust_gui_core::ConfigStore;
    use dbcrust_gui_core::dbcrust::config::Config;
    use gpui_kit::{AppContext as _, TestAppContext, WindowOptions};
    use std::path::PathBuf;

    /// The title rule: saving to the target the tab is bound to keeps a title
    /// the user gave it; saving somewhere else (and loading, which always
    /// retitles) names the tab after the target.
    #[gpui_kit::test]
    fn saving_keeps_a_user_title_only_for_the_same_target(cx: &mut TestAppContext) {
        crate::test_support::isolate();
        let (window, workspace) = cx.update(|cx| {
            crate::init(ConfigStore::from_config(Config::default()), cx);
            cx.set_global(crate::RestoredState(
                dbcrust_gui_core::gui_state::GuiState::default(),
            ));
            crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
        });
        let query = cx
            .update_window(window, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.open_query_tab(
                        None,
                        Some("Query 1".into()),
                        "select 1".into(),
                        window,
                        cx,
                    )
                })
            })
            .expect("window");

        let report = SaveTarget::File(PathBuf::from("/tmp/report.sql"));
        let other = SaveTarget::File(PathBuf::from("/tmp/other.sql"));
        let title = |cx: &mut TestAppContext| cx.update(|cx| query.read(cx).title().to_string());

        // First save: the tab takes the file's name
        cx.update(|cx| {
            query.update(cx, |tab, cx| {
                tab.title_after_save("report.sql".into(), &report, cx);
                tab.bind(report.clone(), "select 1".into(), cx);
            })
        });
        assert_eq!(title(cx), "report.sql");

        // The user's name survives saving to the same file…
        cx.update(|cx| {
            query.update(cx, |tab, cx| {
                assert!(tab.rename("Monthly revenue", cx));
                tab.title_after_save("report.sql".into(), &report, cx);
            })
        });
        assert_eq!(title(cx), "Monthly revenue");

        // …but not "Save as" to another one
        cx.update(|cx| {
            query.update(cx, |tab, cx| {
                tab.title_after_save("other.sql".into(), &other, cx);
            })
        });
        assert_eq!(title(cx), "other.sql");

        // A blank name is refused
        cx.update(|cx| query.update(cx, |tab, cx| assert!(!tab.rename("  ", cx))));
        assert_eq!(title(cx), "other.sql");
    }

    #[test]
    fn default_titles() {
        for (title, expected) in [
            ("Query", true),
            ("Query 12", true),
            ("Query x", false),
            ("Monthly revenue", false),
            ("Queries", false),
        ] {
            assert_eq!(is_default_title(title), expected, "{title}");
        }
    }

    #[test]
    fn suggested_names() {
        for (title, expected) in [
            ("Query 3", "Query 3.sql"),
            ("report.sql", "report.sql"),
            ("a/b: c", "a_b_ c.sql"),
            ("   ", "query.sql"),
        ] {
            assert_eq!(suggested_file_name(title), expected, "{title}");
        }
    }

    #[test]
    fn an_older_save_that_runs_last_does_not_overwrite_a_newer_one() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("report.sql");
        // Submitted A (1) then B (2), but B's write runs first
        assert_eq!(write_in_order(&path, "SELECT 2;", 2), Ok(true));
        assert_eq!(write_in_order(&path, "SELECT 1;", 1), Ok(false));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "SELECT 2;");
        // In order, each write lands
        assert_eq!(write_in_order(&path, "SELECT 3;", 3), Ok(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "SELECT 3;");
        // Another file is ordered on its own
        let other = dir.path().join("other.sql");
        assert_eq!(write_in_order(&other, "SELECT 0;", 1), Ok(true));
    }

    #[test]
    fn saves_of_one_target_land_in_submission_order() {
        let landed = std::cell::RefCell::new(Vec::new());
        let save = |key: &str, sequence: u64| {
            save_in_order::<()>(format!("test:{key}"), sequence, || {
                landed.borrow_mut().push((key.to_string(), sequence));
                Ok(())
            })
        };
        // B (11) runs before A (10): A is skipped, so B stays
        assert_eq!(save("daily", 11), Ok(true));
        assert_eq!(save("daily", 10), Ok(false));
        // Another query is ordered on its own
        assert_eq!(save("weekly", 1), Ok(true));
        assert_eq!(
            landed.into_inner(),
            [("daily".to_string(), 11), ("weekly".to_string(), 1)]
        );
        // A failed save doesn't count as landed
        assert_eq!(
            save_in_order(String::from("test:failing"), 5, || Err("disk full")),
            Err("disk full")
        );
        assert_eq!(
            save_in_order::<()>(String::from("test:failing"), 4, || Ok(())),
            Ok(true)
        );
    }

    #[test]
    fn missing_files_report_the_path() {
        let error = read_sql_file(Path::new("/definitely/not/here.sql")).unwrap_err();
        assert!(error.contains("here.sql"), "{error}");
    }

    #[test]
    fn leading_keyword_is_reexported_for_run() {
        // `explain` relies on this helper to avoid a double EXPLAIN
        assert_eq!(
            leading_keyword("explain select 1").as_deref(),
            Some("EXPLAIN")
        );
    }
}
