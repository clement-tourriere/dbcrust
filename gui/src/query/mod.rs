//! Query tab: SQL editor bound to one connection, plus its results.
//!
//! Contract used by the workspace (keep these signatures):
//! - `QueryTab::new(connection, title, sql, window, cx)` — `connection` is
//!   `None` for a restored draft or after its connection closed; the tab
//!   then offers a connection picker and cannot run until bound.
//! - `title()`, `connection()`, `set_connection(..)`, `sql(cx)`,
//!   `status()`, `cursor(cx)`
//! - `run_current`, `run_all`, `explain`, `cancel`, `set_sql`
//! - emits [`QueryTabEvent`] when the title, the status or the binding
//!   changes. `StatusChanged` is also emitted when the caret moves to
//!   another line/column, so the status bar can show the position.
//!
//! Layout: a slim toolbar (execution target, Run statement/selection and
//! Run all, Explain, Save, saved queries, history, files) above a vertical
//! split of the SQL editor and the results panel.
//!
//! Files: `run.rs` executes statements and EXPLAIN, `toolbar.rs` renders the
//! toolbar, `files.rs` handles `.sql` files, named queries and what ⌘S
//! saves to, `dialogs.rs` holds the save form, `history.rs` the app-wide
//! query history.
//!
//! Honest state: the tab knows whether its results still match the editor
//! (`Stale`, pushed to the results panel) and whether its buffer differs from
//! the file or saved query it was loaded from or saved to (`is_modified`).
//! Draft autosave never changes either.

mod dialogs;
pub mod editor;
pub mod explain;
mod files;
pub mod history;
mod provenance;
pub mod results;
mod run;
#[cfg(test)]
mod tests;
mod toolbar;

use crate::actions::{
    CancelQuery, ExplainAnalyze, ExplainStatement, FormatSql, OpenSqlFile, RunAll, RunStatement,
    SaveNamedQuery, SaveSqlFile,
};
use crate::app_state::AppState;
use crate::preferences::{self, Preferences};
use dbcrust_gui_core::sql_format::{format_script, format_sql};
use dbcrust_gui_core::statements::{SplitConfig, StatementIndex, TargetKind};
use dbcrust_gui_core::{ConnectionHandle, ConnectionId, ConnectionSummary};
use files::Baseline;
use gpui_kit::component::{
    ActiveTheme as _, Theme, WindowExt as _,
    input::{
        Editor, EditorState, Enter, IndentInline, InputEvent, RangeDecorationCollection,
        RopeExt as _,
    },
    notification::Notification,
    resizable::{ResizableState, resizable_panel, v_resizable},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use provenance::Executed;
use results::{ResultsView, Stale};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the status bar shows for the active query tab.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryStatus {
    Idle,
    Running { since: Instant },
    Done { rows: usize, elapsed: Duration },
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub enum QueryTabEvent {
    TitleChanged,
    StatusChanged,
    ConnectionChanged,
}

/// What "Run" would execute right now: names the primary button and its
/// tooltip.
#[derive(Clone, Debug, PartialEq)]
enum TargetHint {
    /// Blank editor.
    None,
    /// A selection spanning (1-based) lines `first..=last`.
    Selection { first: usize, last: usize },
    /// The `index`-th (0-based) of `count` statements.
    Statement { index: usize, count: usize },
    /// The whole buffer (engines whose input is not split on `;`).
    Document,
}

impl TargetHint {
    /// The primary button says what it executes.
    fn run_label(&self) -> &'static str {
        match self {
            TargetHint::Selection { .. } => "Run selection",
            TargetHint::Document => "Run query",
            TargetHint::None | TargetHint::Statement { .. } => "Run statement",
        }
    }

    /// "Run all" runs the statements of the selection when there is one.
    fn run_all_label(&self, compact: bool) -> &'static str {
        match self {
            TargetHint::Selection { .. } if !compact => "Run all in selection",
            _ => "Run all",
        }
    }

    fn lines(first: usize, last: usize) -> String {
        if first == last {
            format!("line {first}")
        } else {
            format!("lines {first}–{last}")
        }
    }

    fn tooltip(&self) -> String {
        match self {
            TargetHint::None => "Write a statement to run".to_string(),
            TargetHint::Selection { first, last } => {
                format!("Run the selection ({})", Self::lines(*first, *last))
            }
            TargetHint::Statement { count: 1, .. } => {
                "Run the statement under the cursor".to_string()
            }
            TargetHint::Statement { index, count } => format!(
                "Run statement {} of {count}, the one under the cursor",
                index + 1
            ),
            TargetHint::Document => "Run the whole editor as one query".to_string(),
        }
    }

    /// A subtle detail shown beside the button (never a detached label).
    fn detail(&self) -> Option<String> {
        match self {
            TargetHint::Selection { first, last } => Some(Self::lines(*first, *last)),
            TargetHint::Statement { index, count } if *count > 1 => {
                Some(format!("{} of {count}", index + 1))
            }
            _ => None,
        }
    }
}

/// What the results on screen were produced from, to tell when they no
/// longer match the editor (see `provenance.rs`).
struct RanFrom {
    /// The statements exactly as they were executed, and what stood against them.
    executed: Vec<Executed>,
    connection: Option<ConnectionId>,
}

/// The run in flight (one per tab).
struct ActiveRun {
    /// Identifies this run; replies of superseded runs are ignored.
    generation: u64,
    /// The request currently executing on the worker (changes per statement
    /// of a "Run all").
    request: dbcrust_gui_core::RequestId,
    handle: ConnectionHandle,
    /// Cancel was requested; the worker has not answered yet.
    cancelling: bool,
    /// What this run executes (see `provenance.rs`).
    executed: Vec<Executed>,
    _task: Task<()>,
}

/// A [`StatementIndex`] and what it was built for.
struct IndexEntry {
    revision: u64,
    config: SplitConfig,
    index: Arc<StatementIndex>,
}

/// Longest tab title the user can type.
pub const MAX_TITLE_CHARS: usize = 80;

pub struct QueryTab {
    connection: Option<ConnectionId>,
    /// Shared with the completion provider.
    connection_cell: Rc<Cell<Option<ConnectionId>>>,
    /// The last connection this tab was bound to — kept while unbound, so
    /// results never carry over to a different connection through a
    /// disconnect (A → none → B).
    last_bound: Option<ConnectionId>,
    title: SharedString,
    /// What ⌘S writes to, and the text it held when it was last loaded or
    /// saved. Draft autosave never touches this.
    baseline: Option<Baseline>,
    /// Bumped when a load, save or restore that may rebind the tab starts,
    /// and on every bind: a background one binds on completion only if
    /// nothing newer started meanwhile.
    target_generation: u64,
    /// The buffer differs from `baseline` (cached; see `refresh_modified`).
    modified: bool,
    /// Where the results on screen came from.
    ran: Option<RanFrom>,
    /// Pushed to the results panel (see `refresh_stale`).
    stale: Option<Stale>,
    /// Width of the toolbar as last painted: decides which labels fit.
    toolbar_width: Rc<Cell<Pixels>>,
    editor: Entity<EditorState>,
    results: Entity<ResultsView>,
    split: Entity<ResizableState>,
    status: QueryStatus,
    run: Option<ActiveRun>,
    generation: u64,
    /// Bumped on every edit; keys the cached execution hint.
    revision: u64,
    hint: TargetHint,
    hint_key: Option<(u64, std::ops::Range<usize>, SplitConfig)>,
    /// The statements of the buffer as of `revision`, shared by everything
    /// that needs boundaries (see [`QueryTab::statement_index`]).
    index: RefCell<Option<IndexEntry>>,
    /// The fill behind the statement the caret will run.
    highlight: RangeDecorationCollection,
    /// What the highlight was last worked out for: revision, split, selection
    /// and tint.
    highlight_key: Option<(u64, SplitConfig, std::ops::Range<usize>, Hsla)>,
    /// Colour the highlight was last painted with.
    highlight_tint: Option<Hsla>,
    /// How many times the highlight was written (tests watch that a caret
    /// move inside a statement writes nothing).
    #[cfg(test)]
    highlight_writes: usize,
    /// How many times the highlight was worked out (not just looked up).
    #[cfg(test)]
    highlight_scans: usize,
    cursor: (usize, usize),
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<QueryTabEvent> for QueryTab {}

impl QueryTab {
    pub fn new(
        connection: Option<ConnectionId>,
        title: Option<String>,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let connection_cell = Rc::new(Cell::new(connection));
        let editor = editor::new_editor_state(sql, connection_cell.clone(), window, cx);
        let highlight = editor.update(cx, |editor, cx| {
            editor.create_range_decorations_collection(Vec::new(), cx)
        });
        let results = cx.new(|cx| ResultsView::new(window, cx));
        let split = cx.new(|_| ResizableState::default());
        let app_state = AppState::global(cx);
        let editor_focus = editor.focus_handle(cx);
        let results_focus = results.focus_handle(cx);
        let subscriptions = vec![
            // Connection commands (disconnect, read-only, refresh…) act on
            // the connection of the tab being worked in
            cx.on_focus_in(&editor_focus, window, Self::on_focused),
            cx.on_focus_in(&results_focus, window, Self::on_focused),
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.observe(&editor, |this, _, cx| this.on_editor_notified(cx)),
            // Connections opening/closing and read-only toggles change the toolbar
            cx.observe(&app_state, |_, _, cx| cx.notify()),
            // The editor font size and the statement separators are settings
            cx.observe_global::<Preferences>(|this, cx| {
                this.on_editor_notified(cx);
                // What counts as a statement may have changed under the results
                this.refresh_stale(cx);
                cx.notify();
            }),
            // The highlight is a tint of the theme
            cx.observe_global::<Theme>(|this, cx| this.refresh_highlight(cx)),
        ];
        let mut tab = Self {
            connection,
            connection_cell,
            last_bound: connection,
            title: title.unwrap_or_else(|| "Query".to_string()).into(),
            baseline: None,
            target_generation: 0,
            modified: false,
            ran: None,
            stale: None,
            toolbar_width: Rc::new(Cell::new(px(0.))),
            editor,
            results,
            split,
            status: QueryStatus::Idle,
            run: None,
            generation: 0,
            revision: 0,
            hint: TargetHint::None,
            hint_key: None,
            index: RefCell::new(None),
            highlight,
            highlight_key: None,
            highlight_tint: None,
            #[cfg(test)]
            highlight_writes: 0,
            #[cfg(test)]
            highlight_scans: 0,
            cursor: (1, 1),
            _subscriptions: subscriptions,
        };
        tab.refresh_hint(cx);
        tab.refresh_highlight(cx);
        tab
    }

    // ----- Contract used by the workspace -----

    pub fn title(&self) -> SharedString {
        self.title.clone()
    }

    pub fn connection(&self) -> Option<ConnectionId> {
        self.connection
    }

    pub fn set_connection(
        &mut self,
        connection: Option<ConnectionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection == connection {
            return;
        }
        // A run belongs to the connection it started on: rebinding stops it
        // there, so neither its results nor the rest of a "Run all" can land
        // on (or execute against) the new one
        let interrupted = connection.and_then(|connection| self.stop_run_elsewhere(connection));
        // Results of one connection must not be offered to another
        let switching =
            connection.is_some() && self.last_bound.is_some_and(|last| Some(last) != connection);
        self.connection = connection;
        if connection.is_some() {
            self.last_bound = connection;
        }
        self.connection_cell.set(connection);
        if switching || interrupted.is_some() {
            self.ran = None;
            self.stale = None;
            self.show_results(results::ResultsContent::Empty, window, cx);
            self.status = QueryStatus::Idle;
            cx.emit(QueryTabEvent::StatusChanged);
        }
        if let Some(previous) = interrupted {
            let label = AppState::global(cx)
                .read(cx)
                .connection(previous)
                .map_or_else(
                    || "the previous connection".to_string(),
                    |live| format!("“{}”", live.summary.label),
                );
            window.push_notification(
                Notification::info(format!("The statement running on {label} was cancelled."))
                    .title("Connection changed"),
                cx,
            );
        }
        cx.emit(QueryTabEvent::ConnectionChanged);
        self.refresh_hint(cx);
        self.refresh_highlight(cx);
        self.refresh_stale(cx);
        cx.notify();
    }

    pub fn sql(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    pub fn status(&self) -> QueryStatus {
        self.status.clone()
    }

    /// 1-based (line, column) of the caret, for the status bar.
    pub fn cursor(&self, cx: &App) -> Option<(usize, usize)> {
        let position = self.editor.read(cx).cursor_position();
        Some((position.line as usize + 1, position.character as usize + 1))
    }

    /// The buffer differs from the `.sql` file or saved query this tab was
    /// loaded from or saved to. A tab with no such target is never
    /// "modified": its text is a draft, kept by autosave.
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Where ⌘S saves, in words (a file path, or the saved query); `None`
    /// for a draft.
    pub fn target_description(&self) -> Option<String> {
        self.baseline.as_ref().map(|baseline| baseline.describe())
    }

    /// The `.sql` file this tab is bound to.
    pub fn file_path(&self) -> Option<&Path> {
        self.baseline.as_ref().and_then(Baseline::file_path)
    }

    /// Replace the whole buffer (programmatic: no undo history, no edit
    /// event).
    pub fn set_sql(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.set_value(sql, window, cx));
        self.edited(cx);
    }

    // ----- Editor plumbing -----

    /// Work moved into this tab: global connection commands follow it.
    fn on_focused(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connection else {
            return;
        };
        let state = AppState::global(cx);
        if state.read(cx).active_id() != Some(connection) {
            state.update(cx, |state, cx| state.set_active(Some(connection), cx));
        }
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<EditorState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.edited(cx);
        }
    }

    fn edited(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.refresh_modified(cx);
        self.refresh_stale(cx);
        self.on_editor_notified(cx);
        cx.notify();
    }

    /// Recompute whether the buffer differs from what was loaded or saved.
    pub(super) fn refresh_modified(&mut self, cx: &mut Context<Self>) {
        let modified = self.baseline.as_ref().is_some_and(|baseline| {
            let editor = self.editor.read(cx);
            // Cheap check first: most edits change the length
            editor.text().len() != baseline.text().len() || *editor.text() != *baseline.text()
        });
        if modified != self.modified {
            self.modified = modified;
            // The tab chrome (title bar of the tab) shows it
            cx.emit(QueryTabEvent::TitleChanged);
        }
    }

    /// Why the results on screen no longer match the editor, if they don't.
    fn compute_stale(&self, cx: &App) -> Option<Stale> {
        let ran = self.ran.as_ref()?;
        if ran.connection != self.connection {
            return Some(Stale::ConnectionChanged);
        }
        let now = self.statement_index(cx);
        provenance::is_stale(&ran.executed, &now).then_some(Stale::Edited)
    }

    /// Tell the results panel whether it is showing a previous run.
    pub(super) fn refresh_stale(&mut self, cx: &mut Context<Self>) {
        let stale = self.compute_stale(cx);
        if stale != self.stale {
            self.stale = stale;
            self.results
                .update(cx, |results, cx| results.set_stale(stale, cx));
        }
    }

    /// New results are on screen: they came from `executed` on the tab's
    /// current connection.
    pub(in crate::query) fn results_from(
        &mut self,
        executed: Vec<Executed>,
        cx: &mut Context<Self>,
    ) {
        self.ran = Some(RanFrom {
            executed,
            connection: self.connection,
        });
        self.stale = None;
        self.refresh_stale(cx);
    }

    /// The editor repainted: the caret or the selection may have moved.
    fn on_editor_notified(&mut self, cx: &mut Context<Self>) {
        let cursor = self.cursor(cx).unwrap_or((1, 1));
        if cursor != self.cursor {
            self.cursor = cursor;
            cx.emit(QueryTabEvent::StatusChanged);
        }
        if self.refresh_hint(cx) {
            cx.notify();
        }
        self.refresh_highlight(cx);
    }

    /// How this tab's buffer is split: the engine's grammar and whether it
    /// splits at all, under the user's preference.
    pub(super) fn split_config(&self, cx: &App) -> SplitConfig {
        let summary = self.summary(cx);
        SplitConfig::new(
            preferences::statement_separators(cx),
            summary.as_ref().map(|s| &s.db_type),
            summary.as_ref().is_some_and(|s| s.runs_unsplit()),
        )
    }

    /// The statements of the buffer as it is now. Built once per document
    /// revision and split configuration, then shared by the toolbar hint, the
    /// highlight, Run, Run all, Explain, Format and the freshness of results,
    /// so they always agree on where a statement starts and ends.
    pub(super) fn statement_index(&self, cx: &App) -> Arc<StatementIndex> {
        let config = self.split_config(cx);
        let editor = self.editor.read(cx);
        let mut cached = self.index.borrow_mut();
        if let Some(entry) = cached.as_ref()
            && entry.revision == self.revision
            && entry.config == config
            // A revision bump is the contract; this only guards against a
            // path that edits without one
            && entry.index.text().len() == editor.text().len()
        {
            return entry.index.clone();
        }
        let index = Arc::new(StatementIndex::new(&editor.text().to_string(), config));
        *cached = Some(IndexEntry {
            revision: self.revision,
            config,
            index: index.clone(),
        });
        index
    }

    /// Recompute what Run would execute; true when it changed.
    fn refresh_hint(&mut self, cx: &App) -> bool {
        let config = self.split_config(cx);
        let state = self.editor.read(cx);
        let selection = state.selected_range();
        let key = (self.revision, selection.clone(), config);
        if self.hint_key.as_ref() == Some(&key) {
            return false;
        }
        self.hint_key = Some(key);
        let rope = state.text().clone();
        let index = self.statement_index(cx);
        let hint = match index.resolve(selection) {
            None => TargetHint::None,
            Some(execution) => match execution.kind {
                TargetKind::Selection => TargetHint::Selection {
                    first: rope.offset_to_point(execution.range.start).row + 1,
                    last: rope.offset_to_point(execution.range.end).row + 1,
                },
                TargetKind::Statement { index, count } => TargetHint::Statement { index, count },
                TargetKind::Document => TargetHint::Document,
            },
        };
        let changed = hint != self.hint;
        self.hint = hint;
        changed
    }

    /// Paint the statement the caret will run, when there is something to
    /// tell it from. Editor notifications arrive for every repaint (the
    /// caret blinks), so nothing is looked at unless the document revision,
    /// the split configuration, the selection or the tint changed. The editor
    /// tracks decoration ranges through edits, so even then the collection is
    /// only written when what it holds differs from what it should.
    fn refresh_highlight(&mut self, cx: &mut Context<Self>) {
        let tint = editor::statement_tint(cx.theme());
        let selection = self.editor.read(cx).selected_range();
        let key = (
            self.revision,
            self.split_config(cx),
            selection.clone(),
            tint,
        );
        if self.highlight_key.as_ref() == Some(&key) {
            return;
        }
        self.highlight_key = Some(key);
        #[cfg(test)]
        {
            self.highlight_scans += 1;
        }
        let index = self.statement_index(cx);
        let wanted = index.highlight(selection);
        let held = self.highlight.get_ranges(cx);
        let up_to_date = held.len() == usize::from(wanted.is_some())
            && held.first() == wanted.as_ref()
            && (wanted.is_none() || self.highlight_tint == Some(tint));
        if up_to_date {
            return;
        }
        #[cfg(test)]
        {
            self.highlight_writes += 1;
        }
        match wanted {
            Some(range) => {
                self.highlight
                    .set(vec![editor::statement_decoration(range, tint)], cx);
                self.highlight_tint = Some(tint);
            }
            None => self.highlight.clear(cx),
        }
    }

    /// Replace the buffer with `text` as an undoable edit, and focus it.
    fn load_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.replace_all(text, window, cx);
            editor.focus(window, cx);
        });
        self.edited(cx);
    }

    /// Insert `text` at the caret (replacing the selection), and focus.
    fn insert_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.replace(text, window, cx);
            editor.focus(window, cx);
        });
        self.edited(cx);
    }

    /// Re-flow the selection or the statement under the caret (whitespace
    /// and keyword case only; as one undoable edit).
    pub fn format_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = self.split_config(cx);
        let selection = self.editor.read(cx).selected_range();
        let target = (!config.unsplit)
            .then(|| self.statement_index(cx).resolve(selection))
            .flatten();
        let Some(target) = target else {
            window.push_notification(
                Notification::info("Format SQL works on SQL statements: write or select one.")
                    .title("Nothing to format"),
                cx,
            );
            return;
        };
        let formatted = match target.kind {
            TargetKind::Selection => format_script(&target.sql, false, config),
            _ => format_sql(&target.sql, false),
        };
        match formatted {
            None => self.warn(
                "Can’t format this statement",
                "Format SQL supports SELECT, INSERT, UPDATE, DELETE and WITH statements.",
                window,
                cx,
            ),
            Some(formatted) if formatted == target.sql => {}
            Some(formatted) => {
                let range = target.range;
                self.editor.update(cx, |editor, cx| {
                    editor.set_selected_range(range, cx);
                    editor.replace(formatted, window, cx);
                    editor.focus(window, cx);
                });
                self.edited(cx);
            }
        }
    }

    fn is_blank(&self, cx: &App) -> bool {
        !self
            .editor
            .read(cx)
            .text()
            .chars()
            .any(|c| !c.is_whitespace())
    }

    fn set_title(&mut self, title: impl Into<SharedString>, cx: &mut Context<Self>) {
        let title = title.into();
        if self.title != title {
            self.title = title;
            cx.emit(QueryTabEvent::TitleChanged);
            cx.notify();
        }
    }

    /// The user renamed the tab. Only the label changes: the file or saved
    /// query the tab saves to keeps its own name. A blank name is refused
    /// (the caller keeps the previous title). Returns whether it was applied.
    pub fn rename(&mut self, title: &str, cx: &mut Context<Self>) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        self.set_title(title.chars().take(MAX_TITLE_CHARS).collect::<String>(), cx);
        true
    }

    // ----- Connection access -----

    /// The live connection this tab is bound to, if it is still open.
    fn live(&self, cx: &App) -> Option<(ConnectionHandle, ConnectionSummary)> {
        let id = self.connection?;
        let state = AppState::global(cx);
        let state = state.read(cx);
        let live = state.connection(id)?;
        Some((live.handle.clone(), live.summary.clone()))
    }

    fn summary(&self, cx: &App) -> Option<ConnectionSummary> {
        self.live(cx).map(|(_, summary)| summary)
    }

    fn warn(&self, title: &str, message: &str, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(
            Notification::warning(message.to_string()).title(title.to_string()),
            cx,
        );
    }

    fn show_results(
        &mut self,
        content: results::ResultsContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection = self.connection;
        self.results.update(cx, |results, cx| {
            results.show(content, connection, window, cx)
        });
    }
}

impl Focusable for QueryTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for QueryTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.render_toolbar(window, cx);
        let editor = div().size_full().overflow_hidden().child(
            Editor::new(&self.editor)
                .bordered(false)
                .size_full()
                .text_size(preferences::editor_font_size(cx))
                .text_color(cx.theme().foreground),
        );
        let split = v_resizable("query-split")
            .with_state(&self.split)
            .child(
                resizable_panel()
                    .size(px(300.))
                    .size_range(px(96.)..Pixels::MAX)
                    .child(editor),
            )
            .child(
                resizable_panel()
                    .size_range(px(120.)..Pixels::MAX)
                    .child(self.results.clone()),
            );

        v_flex()
            .key_context("QueryTab")
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .on_action(
                cx.listener(|this, _: &RunStatement, window, cx| this.run_current(window, cx)),
            )
            .on_action(cx.listener(|this, _: &RunAll, window, cx| this.run_all(window, cx)))
            .on_action(
                cx.listener(|this, _: &ExplainStatement, window, cx| {
                    this.explain(false, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ExplainAnalyze, window, cx| this.explain(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CancelQuery, _, cx| this.cancel(cx)))
            // ⌘S: to the tab's file or saved query; a new tab asks where
            .on_action(
                cx.listener(|this, _: &SaveNamedQuery, window, cx| this.save_query(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FormatSql, window, cx| this.format_sql(window, cx)))
            .on_action(
                cx.listener(|this, _: &OpenSqlFile, window, cx| this.open_sql_file(window, cx)),
            )
            // ⇧⌘S: always asks where
            .on_action(cx.listener(|this, _: &SaveSqlFile, window, cx| this.save_as(window, cx)))
            // ⌘↵ reaches the editor as `Enter { secondary: true }` (which would
            // insert a newline): intercept it before the editor sees it
            .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                if action.secondary && !action.shift {
                    cx.stop_propagation();
                    this.run_current(window, cx);
                }
            }))
            // Tab accepts the highlighted completion (it would indent otherwise)
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.editor.read(cx).completion_menu_state().open {
                    cx.stop_propagation();
                    window.dispatch_action(
                        Box::new(Enter {
                            secondary: false,
                            shift: false,
                        }),
                        cx,
                    );
                }
            }))
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(split))
    }
}

#[cfg(test)]
mod hint_tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::TargetHint;
    use rstest::rstest;

    #[rstest]
    #[case(TargetHint::None, "Run statement")]
    #[case(TargetHint::Statement { index: 0, count: 1 }, "Run statement")]
    #[case(TargetHint::Statement { index: 1, count: 3 }, "Run statement")]
    #[case(TargetHint::Selection { first: 2, last: 4 }, "Run selection")]
    #[case(TargetHint::Document, "Run query")]
    fn the_primary_button_names_what_runs(#[case] hint: TargetHint, #[case] label: &str) {
        assert_eq!(hint.run_label(), label);
    }

    #[rstest]
    #[case(TargetHint::None, None)]
    #[case(TargetHint::Statement { index: 0, count: 1 }, None)]
    #[case(TargetHint::Statement { index: 1, count: 3 }, Some("2 of 3"))]
    #[case(TargetHint::Selection { first: 5, last: 5 }, Some("line 5"))]
    #[case(TargetHint::Selection { first: 2, last: 4 }, Some("lines 2–4"))]
    #[case(TargetHint::Document, None)]
    fn the_detail_sits_beside_the_button(#[case] hint: TargetHint, #[case] detail: Option<&str>) {
        assert_eq!(hint.detail().as_deref(), detail);
    }

    #[rstest]
    #[case(TargetHint::None, false, "Run all")]
    #[case(TargetHint::Statement { index: 1, count: 3 }, false, "Run all")]
    #[case(TargetHint::Selection { first: 2, last: 4 }, false, "Run all in selection")]
    // The narrow toolbar keeps the short label
    #[case(TargetHint::Selection { first: 2, last: 4 }, true, "Run all")]
    fn run_all_says_when_it_works_on_the_selection(
        #[case] hint: TargetHint,
        #[case] compact: bool,
        #[case] label: &str,
    ) {
        assert_eq!(hint.run_all_label(compact), label);
    }

    #[test]
    fn tooltips_say_which_statement() {
        assert_eq!(
            TargetHint::Statement { index: 1, count: 3 }.tooltip(),
            "Run statement 2 of 3, the one under the cursor"
        );
        assert_eq!(
            TargetHint::Selection { first: 2, last: 4 }.tooltip(),
            "Run the selection (lines 2–4)"
        );
    }
}
