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
//!   changes, and `CursorMoved` when the caret moves to another line/column
//!   (only the status bar's position depends on it).
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
        EditorState, Enter, IndentInline, InputEvent, RangeDecorationCollection, Rope, RopeExt as _,
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
    /// [`QueryTab::status`] changed (a run started, finished, failed or was
    /// cancelled, or a rebind reset it).
    StatusChanged,
    /// The caret moved to another line or column ([`QueryTab::cursor`]).
    /// Nothing else changed: no running-query or tray state depends on it.
    CursorMoved,
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
    /// The statements exactly as they were executed, and what stood against
    /// them (shared with the background split of a large buffer, which checks
    /// them against the new index).
    executed: Arc<[Executed]>,
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

/// A [`StatementIndex`] and what it was built for. Shared with the
/// completion provider, which reuses it when it matches the text it is
/// asked about.
struct IndexEntry {
    revision: u64,
    config: SplitConfig,
    index: Arc<StatementIndex>,
}

/// Up to this size, an edit re-splits the buffer at once (well under a
/// millisecond): the hint, the highlight and the freshness of the results
/// follow every keystroke. A larger buffer is split off the UI thread once
/// typing pauses for [`INDEX_DEBOUNCE`]; until then they keep their previous
/// state (the editor moves the highlight along with the text). Run, Explain
/// and Format always use an index of the current text.
const SYNC_INDEX_BYTES: usize = 64 * 1024;
const INDEX_DEBOUNCE: Duration = Duration::from_millis(75);
/// Comparing a buffer of the same length as the file it was loaded from (to
/// show the unsaved-changes dot) moves off the UI thread above this size.
const SYNC_COMPARE_BYTES: usize = 64 * 1024;

/// Longest tab title the user can type.
pub const MAX_TITLE_CHARS: usize = 80;

pub struct QueryTab {
    connection: Option<ConnectionId>,
    /// The summary of the bound connection for what is worked out on every
    /// repaint (the toolbar, the split): copied from [`AppState`] when the
    /// binding changes and on every `AppState` notification (every change of
    /// a live connection's summary comes with one), not on each editor
    /// notification. Commands read [`Self::summary`] instead.
    cached_summary: Option<Arc<ConnectionSummary>>,
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
    /// The editor as a cached view: a repaint of the toolbar or the results
    /// (a spinner frame, a grid scroll) does not lay the editor out again.
    editor_pane: Entity<editor::EditorPane>,
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
    index: Rc<RefCell<Option<IndexEntry>>>,
    /// The background split of a large buffer, and the revision and split it
    /// is for (see [`SYNC_INDEX_BYTES`]).
    index_task: Option<(u64, SplitConfig, Task<()>)>,
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
    /// How many times the results were checked against the buffer on the UI
    /// thread.
    #[cfg(test)]
    stale_scans: usize,
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
        let index = Rc::new(RefCell::new(None));
        let editor =
            editor::new_editor_state(sql, connection_cell.clone(), index.clone(), window, cx);
        let editor_pane = cx.new(|cx| editor::EditorPane::new(editor.clone(), cx));
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
            cx.observe(&app_state, |this, _, cx| {
                if this.refresh_summary(cx) {
                    // The split follows the engine
                    this.schedule_index(cx);
                }
                cx.notify();
            }),
            // The editor font size and the statement separators are settings
            cx.observe_global::<Preferences>(|this, cx| {
                this.schedule_index(cx);
                this.on_editor_notified(cx);
                // What counts as a statement may have changed under the results
                this.refresh_stale(cx);
                this.notify_views(cx);
            }),
            // The highlight is a tint of the theme
            cx.observe_global::<Theme>(|this, cx| this.refresh_highlight(cx)),
        ];
        let mut tab = Self {
            connection,
            cached_summary: None,
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
            editor_pane,
            results,
            split,
            status: QueryStatus::Idle,
            run: None,
            generation: 0,
            revision: 0,
            hint: TargetHint::None,
            hint_key: None,
            index,
            index_task: None,
            highlight,
            highlight_key: None,
            highlight_tint: None,
            #[cfg(test)]
            highlight_writes: 0,
            #[cfg(test)]
            highlight_scans: 0,
            #[cfg(test)]
            stale_scans: 0,
            cursor: (1, 1),
            _subscriptions: subscriptions,
        };
        tab.refresh_summary(cx);
        tab.schedule_index(cx);
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
        self.refresh_summary(cx);
        self.schedule_index(cx);
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
        self.editor.read(cx).text().to_string()
    }

    /// The buffer as an immutable, cheaply cloned snapshot (draft autosave
    /// turns it into text on its writer thread).
    pub fn sql_snapshot(&self, cx: &App) -> Rope {
        self.editor.read(cx).text().clone()
    }

    /// Bumped on every change of the buffer (typing, undo, programmatic
    /// edits); a tab keeps the same text as long as it stays the same.
    pub fn edit_revision(&self) -> u64 {
        self.revision
    }

    /// The buffer holds nothing but whitespace.
    pub fn is_blank(&self, cx: &App) -> bool {
        !self
            .editor
            .read(cx)
            .text()
            .chars()
            .any(|c| !c.is_whitespace())
    }

    pub fn status(&self) -> QueryStatus {
        self.status.clone()
    }

    /// Render the tab again, and the views it embeds cached (the editor, the
    /// results and their grid): for a change outside the tab that any of
    /// them may show (the app state, the preferences). A change of their own
    /// repaints only them.
    pub fn notify_views(&mut self, cx: &mut Context<Self>) {
        self.editor_pane.update(cx, |_, cx| cx.notify());
        // The grid is cached inside the results: it renders with them
        self.results.update(cx, |_, cx| cx.notify());
        cx.notify();
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
        // A large buffer is split in the background; the hint, the highlight
        // and the freshness of the results follow when that lands
        self.schedule_index(cx);
        self.refresh_stale(cx);
        self.on_editor_notified(cx);
        cx.notify();
    }

    /// Recompute whether the buffer differs from what was loaded or saved.
    pub(super) fn refresh_modified(&mut self, cx: &mut Context<Self>) {
        let rope = self.editor.read(cx).text();
        let modified = match self.baseline.as_ref() {
            None => false,
            // Most edits change the length
            Some(baseline) if rope.len() != baseline.text().len() => true,
            Some(baseline) if rope.len() <= SYNC_COMPARE_BYTES => *rope != *baseline.text(),
            Some(baseline) => {
                // Same length, large: compare off the UI thread. The answer
                // applies only to this text and this very baseline: a newer
                // edit or bind asks again, while a save that starts (and may
                // fail) meanwhile changes neither.
                let rope = rope.clone();
                let text = baseline.shared_text();
                let revision = self.revision;
                let comparing = cx.background_spawn({
                    let text = text.clone();
                    async move { rope != *text }
                });
                cx.spawn(async move |this, cx| {
                    let modified = comparing.await;
                    this.update(cx, |tab, cx| {
                        let same_baseline = tab
                            .baseline
                            .as_ref()
                            .is_some_and(|baseline| baseline.holds(&text));
                        if tab.revision == revision && same_baseline {
                            tab.set_modified(modified, cx);
                        }
                    })
                    .ok();
                })
                .detach();
                return;
            }
        };
        self.set_modified(modified, cx);
    }

    fn set_modified(&mut self, modified: bool, cx: &mut Context<Self>) {
        if modified != self.modified {
            self.modified = modified;
            // The tab chrome (title bar of the tab) shows it
            cx.emit(QueryTabEvent::TitleChanged);
            cx.notify();
        }
    }

    /// Tell the results panel whether it is showing a previous run.
    pub(super) fn refresh_stale(&mut self, cx: &mut Context<Self>) {
        let stale = match &self.ran {
            None => None,
            Some(ran) if ran.connection != self.connection => Some(Stale::ConnectionChanged),
            Some(ran) => {
                // A large buffer being split: the split checks it when it lands
                let Some(now) = self.index_now(cx) else {
                    return;
                };
                let executed = ran.executed.clone();
                #[cfg(test)]
                {
                    self.stale_scans += 1;
                }
                provenance::is_stale(&executed, &now).then_some(Stale::Edited)
            }
        };
        self.show_stale(stale, cx);
    }

    fn show_stale(&mut self, stale: Option<Stale>, cx: &mut Context<Self>) {
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
            executed: executed.into(),
            connection: self.connection,
        });
        self.stale = None;
        self.refresh_stale(cx);
    }

    /// The editor repainted (the caret blinks, the text or the selection
    /// changed): only a move of the caret or the selection does any work.
    fn on_editor_notified(&mut self, cx: &mut Context<Self>) {
        let cursor = self.cursor(cx).unwrap_or((1, 1));
        if cursor != self.cursor {
            self.cursor = cursor;
            cx.emit(QueryTabEvent::CursorMoved);
        }
        if self.refresh_hint(cx) {
            cx.notify();
        }
        self.refresh_highlight(cx);
    }

    /// How this tab's buffer is split: the engine's grammar and whether it
    /// splits at all, under the user's preference.
    pub(super) fn split_config(&self, cx: &App) -> SplitConfig {
        let summary = self.cached_summary.as_deref();
        SplitConfig::new(
            preferences::statement_separators(cx),
            summary.map(|s| &s.db_type),
            summary.is_some_and(|s| s.runs_unsplit()),
        )
    }

    /// Copy the summary of the bound connection from [`AppState`] when it
    /// differs from the one held; true when it did.
    fn refresh_summary(&mut self, cx: &App) -> bool {
        let state = AppState::global(cx);
        let live = self
            .connection
            .and_then(|id| state.read(cx).connection(id))
            .map(|live| &live.summary);
        let changed = self.cached_summary.as_deref() != live;
        if changed {
            self.cached_summary = live.cloned().map(Arc::new);
        }
        changed
    }

    /// The index of the current text, if it can be had without blocking the
    /// UI thread: always for a small buffer (built now if needed), for a
    /// large one only once its background split landed.
    fn index_now(&self, cx: &App) -> Option<Arc<StatementIndex>> {
        (self.editor.read(cx).text().len() <= SYNC_INDEX_BYTES || self.index_is_current(cx))
            .then(|| self.statement_index(cx))
    }

    fn index_is_current(&self, cx: &App) -> bool {
        let config = self.split_config(cx);
        let len = self.editor.read(cx).text().len();
        self.index.borrow().as_ref().is_some_and(|entry| {
            entry.revision == self.revision
                && entry.config == config
                && entry.index.text().len() == len
        })
    }

    /// For a large buffer whose index is out of date: split it on the
    /// background executor once edits pause, then refresh what depends on it.
    fn schedule_index(&mut self, cx: &mut Context<Self>) {
        let rope = self.editor.read(cx).text();
        if rope.len() <= SYNC_INDEX_BYTES || self.index_is_current(cx) {
            self.index_task = None;
            return;
        }
        let (revision, config) = (self.revision, self.split_config(cx));
        if matches!(&self.index_task, Some((r, c, _)) if *r == revision && *c == config) {
            return;
        }
        let rope = rope.clone();
        // Whether the results on screen still match is checked against the
        // new index there too: it looks up every statement of the buffer
        let ran = self.ran.as_ref().map(|ran| ran.executed.clone());
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INDEX_DEBOUNCE).await;
            let (index, checked) = cx
                .background_spawn(async move {
                    let index = StatementIndex::from_shared(Arc::from(rope.to_string()), config);
                    let checked = ran.map(|executed| {
                        let edited = provenance::is_stale(&executed, &index);
                        (executed, edited)
                    });
                    (index, checked)
                })
                .await;
            this.update(cx, |tab, cx| {
                if tab.revision != revision || tab.split_config(cx) != config {
                    // Superseded: the newer change scheduled its own split
                    return;
                }
                *tab.index.borrow_mut() = Some(IndexEntry {
                    revision,
                    config,
                    index: Arc::new(index),
                });
                match checked {
                    // The same results, on the same connection: the check holds
                    Some((executed, edited))
                        if tab.ran.as_ref().is_some_and(|ran| {
                            Arc::ptr_eq(&ran.executed, &executed)
                                && ran.connection == tab.connection
                        }) =>
                    {
                        tab.show_stale(edited.then_some(Stale::Edited), cx)
                    }
                    _ => tab.refresh_stale(cx),
                }
                tab.on_editor_notified(cx);
                cx.notify();
            })
            .ok();
        });
        self.index_task = Some((revision, config, task));
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
        let index = Arc::new(StatementIndex::from_shared(
            Arc::from(editor.text().to_string()),
            config,
        ));
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
        // A large buffer being split: worked out when the split lands
        let Some(index) = self.index_now(cx) else {
            return false;
        };
        self.hint_key = Some(key);
        let hint = match index.target(selection) {
            None => TargetHint::None,
            Some((range, kind)) => match kind {
                TargetKind::Selection => {
                    let rope = state.text();
                    TargetHint::Selection {
                        first: rope.offset_to_point(range.start).row + 1,
                        last: rope.offset_to_point(range.end).row + 1,
                    }
                }
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
        // A large buffer being split: the editor moves the highlight along
        // with the text until the split lands
        let Some(index) = self.index_now(cx) else {
            return;
        };
        self.highlight_key = Some(key);
        #[cfg(test)]
        {
            self.highlight_scans += 1;
        }
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
        // Cached views: each repaints only when its own state changes, so
        // typing does not lay out the results, and a results repaint (a
        // spinner frame, a grid scroll) does not lay out the editor. This only
        // holds because the tab itself is not cached: a cached view that
        // renders again renders everything inside it (GPUI's `prepaint_view`
        // sets `window.refreshing` for its subtree). See `WorkspaceTab::content`.
        let editor = self
            .editor_pane
            .clone()
            .cached(StyleRefinement::default().size_full());
        let results = self
            .results
            .clone()
            .cached(StyleRefinement::default().size_full());
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
                    .child(results),
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
