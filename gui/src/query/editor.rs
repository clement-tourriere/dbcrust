//! SQL editor helpers: GPUI Kit's code editor configured for SQL, the view
//! that hosts it in a query tab, and the schema-aware completion provider
//! that feeds its completion menu.
//!
//! The provider takes cheap snapshots on the UI thread (the rope, the
//! connection's shared table list) and does the work — splitting the
//! statement, ranking every candidate, building the menu items — on the
//! background executor, so a keystroke never waits for a large schema or
//! buffer. It asks [`AppState::table_details`] for the columns of the tables
//! the current statement references (cached per connection). Waiting for a
//! table description is bounded, so the menu never hangs behind a busy
//! connection.

use super::IndexEntry;
use crate::app_state::AppState;
use crate::preferences::{self, Preferences};
use dbcrust_gui_core::completion::{
    self, Candidate, CandidateKind, ColumnCandidate, CompletionNames, Plan, SchemaView,
};
use dbcrust_gui_core::statements::{SplitConfig, StatementIndex};
use dbcrust_gui_core::{ConnectionId, DatabaseType, TableEntry};
use gpui_kit::component::input::{
    CompletionProvider, Editor, EditorState, RangeDecoration, RangeDecorationStyle, Rope,
    RopeExt as _,
};
use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::*;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range as LspRange, TextEdit,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a completion request waits for a table description before it
/// answers with what it has.
const DESCRIBE_PATIENCE: Duration = Duration::from_millis(450);
/// A table that failed to describe is not retried for this long.
const DESCRIBE_RETRY_AFTER: Duration = Duration::from_secs(15);

/// Build the editor state of a query tab: SQL highlighting, line numbers,
/// no soft wrap, the completion provider, and `sql` as its content.
/// `connection` is shared with the provider so completion follows the tab's
/// connection; so is the tab's statement `index`, which a request reuses
/// when it was built from the very text it completes.
pub(super) fn new_editor_state(
    sql: String,
    connection: Rc<Cell<Option<ConnectionId>>>,
    index: Rc<RefCell<Option<IndexEntry>>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let provider: Rc<dyn CompletionProvider> =
        Rc::new(SqlCompletionProvider::new(connection, index));
    cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language("sql")
            .soft_wrap(false)
            .placeholder("Write SQL — ⌘↵ runs the statement under the cursor, ⌘⇧↵ explains it")
            .default_value(sql);
        let lsp = state.lsp_mut();
        lsp.completion_provider = Some(provider);
        lsp.completion_menu.max_width = px(440.);
        state
    })
}

/// [`new_editor_state`] for an editor outside a query tab (the saved
/// queries manager): there is no tab statement index to share, so
/// completion builds its own.
pub fn new_detached_editor_state(
    sql: String,
    connection: Rc<Cell<Option<ConnectionId>>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    new_editor_state(sql, connection, Rc::new(RefCell::new(None)), window, cx)
}

/// The editor of a query tab as a view of its own, so the tab can embed it
/// cached ([`Entity::cached`]): it repaints when the editor does (typing,
/// the caret) or the editor font size changes, not when the toolbar or the
/// results do. Theme changes refresh every window, cached views included.
pub(super) struct EditorPane {
    editor: Entity<EditorState>,
    /// How many times it rendered (tests watch what a repaint reaches).
    #[cfg(test)]
    pub(super) renders: usize,
    _preferences: Subscription,
}

impl EditorPane {
    pub(super) fn new(editor: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        Self {
            editor,
            #[cfg(test)]
            renders: 0,
            // The font size is a setting
            _preferences: cx.observe_global::<Preferences>(|_, cx| cx.notify()),
        }
    }
}

impl Render for EditorPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        div().size_full().overflow_hidden().child(
            Editor::new(&self.editor)
                .bordered(false)
                .size_full()
                .text_size(preferences::editor_font_size(cx))
                .text_color(cx.theme().foreground),
        )
    }
}

/// The fill behind the statement the caret will run: the theme's primary
/// color at low opacity, clearly weaker than the selection (which is a
/// solid, opaque-ish wash) so the two never read as the same thing.
pub fn statement_tint(theme: &Theme) -> Hsla {
    theme
        .primary
        .opacity(if theme.is_dark() { 0.2 } else { 0.09 })
}

/// The decoration that paints `tint` behind `range` (UTF-8 byte offsets).
pub fn statement_decoration(range: std::ops::Range<usize>, tint: Hsla) -> RangeDecoration {
    RangeDecoration::new(range)
        .with_style(RangeDecorationStyle::Fill)
        .with_color(tint)
}

/// Table descriptions being fetched or recently failed, shared by every
/// completion request of one editor.
#[derive(Default)]
struct Fetches {
    inflight: HashMap<(ConnectionId, String), Fetch>,
    failed: HashMap<(ConnectionId, String), Instant>,
}

/// A table description being fetched, and the requests waiting for it.
struct Fetch {
    _task: Task<()>,
    waiters: Vec<async_channel::Sender<()>>,
}

#[derive(Clone)]
pub struct SqlCompletionProvider {
    connection: Rc<Cell<Option<ConnectionId>>>,
    fetches: Rc<RefCell<Fetches>>,
    /// The statement index of the query tab (see [`new_editor_state`]).
    index: Rc<RefCell<Option<IndexEntry>>>,
    /// The table labels of the last schema snapshot completed against,
    /// prepared for matching; rebuilt when the snapshot changes (a schema
    /// refresh replaces the shared list).
    names: Rc<RefCell<Option<PreparedNames>>>,
}

/// A schema snapshot and its table labels prepared for matching.
type PreparedNames = (Arc<Vec<TableEntry>>, Arc<CompletionNames>);

impl SqlCompletionProvider {
    fn new(
        connection: Rc<Cell<Option<ConnectionId>>>,
        index: Rc<RefCell<Option<IndexEntry>>>,
    ) -> Self {
        Self {
            connection,
            fetches: Rc::default(),
            index,
            names: Rc::default(),
        }
    }

    /// The prepared labels of `tables`, if this snapshot was seen before.
    fn prepared_names(&self, tables: &Arc<Vec<TableEntry>>) -> Option<Arc<CompletionNames>> {
        self.names
            .borrow()
            .as_ref()
            .filter(|(seen, _)| Arc::ptr_eq(seen, tables))
            .map(|(_, names)| names.clone())
    }

    /// Snapshot what a request needs now.
    fn request(&self, cx: &App) -> Request {
        let snap = self.connection.get().and_then(|id| snapshot(id, cx));
        let db_type = snap.as_ref().map(|s| &s.db_type);
        let config = completion::completion_split(preferences::statement_separators(cx), db_type);
        let tab_index = self
            .index
            .borrow()
            .as_ref()
            .filter(|entry| entry.config == config)
            .map(|entry| entry.index.clone());
        let prepared = snap
            .as_ref()
            .and_then(|snap| snap.tables.as_ref())
            .and_then(|tables| self.prepared_names(tables));
        Request {
            snap,
            config,
            tab_index,
            prepared,
        }
    }

    /// The app still holds what `request` was worked out from: the same
    /// live connection, and the same table list (or still none).
    fn is_current(&self, request: &Request, cx: &App) -> bool {
        let now = self.connection.get().and_then(|id| snapshot(id, cx));
        match (&request.snap, &now) {
            (None, None) => true,
            (Some(then), Some(now)) => {
                then.id == now.id
                    && match (&then.tables, &now.tables) {
                        (Some(then), Some(now)) => Arc::ptr_eq(then, now),
                        (None, None) => true,
                        _ => false,
                    }
            }
            _ => false,
        }
    }
}

/// What a completion request needs from the app state, copied out so the
/// request can wait without holding any borrow. The table list is the
/// connection's shared snapshot, not a copy.
struct Snapshot {
    id: ConnectionId,
    db_type: DatabaseType,
    /// `None` while the schema loads (or failed to).
    tables: Option<Arc<Vec<TableEntry>>>,
}

fn snapshot(id: ConnectionId, cx: &App) -> Option<Snapshot> {
    let state = AppState::global(cx);
    let state = state.read(cx);
    let live = state.connection(id)?;
    Some(Snapshot {
        id,
        db_type: live.summary.db_type.clone(),
        tables: live.schema.tables().cloned(),
    })
}

/// One round of a completion request: what it reads from the app, taken
/// on the UI thread.
struct Request {
    /// The live connection the editor is bound to (`None`: keywords and
    /// functions only).
    snap: Option<Snapshot>,
    config: SplitConfig,
    /// The tab's statement index, if it may be of this text.
    tab_index: Option<Arc<StatementIndex>>,
    prepared: Option<Arc<CompletionNames>>,
}

impl Request {
    fn tables(&self) -> Arc<Vec<TableEntry>> {
        self.snap
            .as_ref()
            .and_then(|snap| snap.tables.clone())
            .unwrap_or_default()
    }

    fn db_type(&self) -> Option<DatabaseType> {
        self.snap.as_ref().map(|snap| snap.db_type.clone())
    }
}

/// A request answers only for the connection and the table list it was
/// worked out from; a new attempt follows when either changed meanwhile.
const COMPLETION_ROUNDS: usize = 3;

/// Columns of `entry` if its description is cached.
fn cached_columns(id: ConnectionId, entry: &TableEntry, cx: &App) -> Option<Vec<ColumnCandidate>> {
    let state = AppState::global(cx);
    let state = state.read(cx);
    let live = state.connection(id)?;
    let details = live
        .details
        .get(&entry.qualified_name())
        .or_else(|| live.details.get(&entry.name))?;
    Some(
        details
            .columns
            .iter()
            .map(|c| ColumnCandidate::new(c.name.clone(), c.data_type.clone()))
            .collect(),
    )
}

impl SqlCompletionProvider {
    /// Collect the columns of `needed` that are cached; start fetching the
    /// others. Returns the map and whether anything is still missing; `wake`
    /// hears once each of the missing descriptions settles.
    fn gather(
        &self,
        id: ConnectionId,
        needed: &[TableEntry],
        wake: &async_channel::Sender<()>,
        cx: &mut App,
    ) -> (HashMap<String, Vec<ColumnCandidate>>, bool) {
        let mut columns = HashMap::new();
        let mut pending = false;
        for entry in needed {
            let key = entry.qualified_name();
            if let Some(found) = cached_columns(id, entry, cx) {
                columns.insert(key, found);
                continue;
            }
            let slot = (id, key.clone());
            {
                let mut fetches = self.fetches.borrow_mut();
                if let Some(failed_at) = fetches.failed.get(&slot) {
                    if failed_at.elapsed() < DESCRIBE_RETRY_AFTER {
                        continue;
                    }
                    fetches.failed.remove(&slot);
                }
                if let Some(fetch) = fetches.inflight.get_mut(&slot) {
                    // Requests that gave up are not woken
                    fetch.waiters.retain(|waiter| !waiter.is_closed());
                    fetch.waiters.push(wake.clone());
                    pending = true;
                    continue;
                }
            }
            pending = true;
            let describing = AppState::global(cx)
                .update(cx, |state, cx| state.table_details(id, key.clone(), cx));
            let fetches = self.fetches.clone();
            let finished = slot.clone();
            let task = cx.spawn(async move |_| {
                let outcome = describing.await;
                let mut fetches = fetches.borrow_mut();
                if let Some(fetch) = fetches.inflight.remove(&finished) {
                    for waiter in fetch.waiters {
                        waiter.try_send(()).ok();
                    }
                }
                if outcome.is_err() {
                    fetches.failed.insert(finished, Instant::now());
                }
            });
            self.fetches.borrow_mut().inflight.insert(
                slot,
                Fetch {
                    _task: task,
                    waiters: vec![wake.clone()],
                },
            );
        }
        (columns, pending)
    }

    /// Whether a description of `needed` (on connection `id`) is still being
    /// fetched. A failed one stops the wait.
    fn still_fetching(&self, id: ConnectionId, needed: &[TableEntry]) -> bool {
        let fetches = self.fetches.borrow();
        needed
            .iter()
            .any(|entry| fetches.inflight.contains_key(&(id, entry.qualified_name())))
    }
}

/// What the background half of a request works out from the snapshots.
struct Analysis {
    plan: Option<Plan>,
    names: Arc<CompletionNames>,
    /// The tables the statement references, to describe for their columns.
    needed: Vec<TableEntry>,
}

impl SqlCompletionProvider {
    /// Work out the items for `request`: the analysis and the ranking on the
    /// background executor, waiting (briefly) for the descriptions of the
    /// tables the statement references.
    async fn answer(
        &self,
        request: &Request,
        rope: &Rope,
        offset: usize,
        cx: &mut AsyncApp,
    ) -> Vec<CompletionItem> {
        let tables = request.tables();
        let db_type = request.db_type();
        let analysis = cx
            .background_spawn({
                let (rope, tables, db_type) = (rope.clone(), tables.clone(), db_type.clone());
                let (tab_index, prepared, config) = (
                    request.tab_index.clone(),
                    request.prepared.clone(),
                    request.config,
                );
                async move {
                    // The request may come before the tab saw the edit:
                    // its index serves only if it holds this very text
                    let index = tab_index
                        .filter(|index| rope == *index.text())
                        .unwrap_or_else(|| {
                            Arc::new(StatementIndex::from_shared(
                                Arc::from(rope.to_string()),
                                config,
                            ))
                        });
                    let names = prepared.unwrap_or_else(|| Arc::new(CompletionNames::new(&tables)));
                    let plan = completion::analyze_indexed(&index, offset, db_type.as_ref());
                    let needed = plan
                        .as_ref()
                        .map(|plan| {
                            plan.needed_tables(&tables, db_type.as_ref())
                                .into_iter()
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    Analysis {
                        plan,
                        names,
                        needed,
                    }
                }
            })
            .await;
        if request
            .snap
            .as_ref()
            .is_some_and(|snap| snap.tables.is_some())
        {
            *self.names.borrow_mut() = Some((tables.clone(), analysis.names.clone()));
        }
        let Some(plan) = analysis.plan else {
            return Vec::new();
        };
        let needed = analysis.needed;
        // Not bound to a connection: keywords and functions only
        let mut columns = HashMap::new();
        if let Some(snap) = &request.snap {
            let (wake, woken) = async_channel::unbounded();
            let (found, pending) = cx.update(|cx| self.gather(snap.id, &needed, &wake, cx));
            columns = found;
            if pending {
                // Wait (briefly) for the descriptions to land in the
                // cache: woken as each one settles, no polling
                let settled = async {
                    while woken.recv().await.is_ok() && self.still_fetching(snap.id, &needed) {}
                };
                let executor = cx.background_executor().clone();
                settled
                    .with_timeout(DESCRIBE_PATIENCE, &executor)
                    .await
                    .ok();
                columns = cx.update(|cx| {
                    needed
                        .iter()
                        .filter_map(|entry| {
                            cached_columns(snap.id, entry, cx)
                                .map(|found| (entry.qualified_name(), found))
                        })
                        .collect()
                });
            }
        }
        let names = analysis.names;
        let rope = rope.clone();
        cx.background_spawn(async move {
            build_items(
                &plan,
                db_type.as_ref(),
                &tables,
                &columns,
                &rope,
                Some(&names),
            )
        })
        .await
    }
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        // Cheap snapshots here; parsing, ranking and building the items run
        // on the background executor
        let rope = text.clone();
        let first = self.request(cx);
        let provider = self.clone();
        cx.spawn(async move |cx| {
            let mut request = first;
            for _ in 0..COMPLETION_ROUNDS {
                let items = provider.answer(&request, &rope, offset, cx).await;
                // The schema was replaced (or loaded, or the tab rebound)
                // while this was worked out: answer from what is there now
                if cx.update(|cx| provider.is_current(&request, cx)) {
                    return Ok(CompletionResponse::Array(items));
                }
                request = cx.update(|cx| provider.request(cx));
            }
            Ok(CompletionResponse::Array(Vec::new()))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        completion::is_trigger(new_text)
    }
}

/// Turn ranked candidates into LSP completion items whose text edit
/// replaces exactly the word being completed.
fn build_items(
    plan: &Plan,
    db_type: Option<&DatabaseType>,
    tables: &[TableEntry],
    columns: &HashMap<String, Vec<ColumnCandidate>>,
    rope: &Rope,
    names: Option<&CompletionNames>,
) -> Vec<CompletionItem> {
    let candidates = completion::suggest_prepared(
        plan,
        &SchemaView {
            db_type,
            tables,
            columns,
        },
        names,
    );
    let range = LspRange::new(
        rope.offset_to_position(plan.replace.start.min(rope.len())),
        rope.offset_to_position(plan.replace.end.min(rope.len())),
    );
    candidates
        .into_iter()
        .enumerate()
        .map(|(rank, candidate)| item_for(candidate, rank, range))
        .collect()
}

fn item_for(candidate: Candidate, rank: usize, range: LspRange) -> CompletionItem {
    let kind = match candidate.kind {
        CandidateKind::Table | CandidateKind::View | CandidateKind::Cte => {
            CompletionItemKind::STRUCT
        }
        CandidateKind::Schema => CompletionItemKind::MODULE,
        CandidateKind::Column => CompletionItemKind::FIELD,
        CandidateKind::Alias => CompletionItemKind::VARIABLE,
        CandidateKind::Function => CompletionItemKind::FUNCTION,
        CandidateKind::Keyword => CompletionItemKind::KEYWORD,
    };
    // The menu highlights `filter_text.len()` bytes of the label as the
    // matched part; keep that on a character boundary
    let highlighted = if candidate.matched > 0 {
        candidate.matched
    } else {
        candidate.label.chars().next().map_or(0, char::len_utf8)
    };
    let filter_text = candidate
        .label
        .get(..highlighted)
        .unwrap_or_default()
        .to_string();
    CompletionItem {
        label: candidate.label,
        kind: Some(kind),
        detail: (!candidate.detail.is_empty()).then_some(candidate.detail),
        filter_text: Some(filter_text),
        sort_text: Some(format!("{rank:05}")),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: candidate.insert,
        })),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob of the parent would pull in `gpui_kit::*`,
    // whose `test` macro shadows `#[test]` under `test-support`
    use super::{Plan, build_items};
    use dbcrust_gui_core::completion;
    use dbcrust_gui_core::{DatabaseType, TableEntry, TableKind};
    use gpui_kit::component::input::Rope;
    use lsp_types::{CompletionItem, CompletionTextEdit, Position, TextEdit};
    use std::collections::HashMap;

    fn tables() -> Vec<TableEntry> {
        ["orders", "order_items", "products"]
            .into_iter()
            .map(|name| TableEntry {
                schema: None,
                name: name.into(),
                kind: TableKind::Table,
                owner: None,
            })
            .collect()
    }

    fn items(marked: &str) -> (Vec<CompletionItem>, Plan) {
        let offset = marked.find('|').expect("caret marker");
        let text = marked.replacen('|', "", 1);
        let plan = completion::analyze(&text, offset, Some(&DatabaseType::SQLite)).expect("a plan");
        let rope = Rope::from(text.as_str());
        let items = build_items(
            &plan,
            Some(&DatabaseType::SQLite),
            &tables(),
            &HashMap::new(),
            &rope,
            None,
        );
        (items, plan)
    }

    fn edit(item: &CompletionItem) -> &TextEdit {
        match item.text_edit.as_ref().expect("an explicit text edit") {
            CompletionTextEdit::Edit(edit) => edit,
            CompletionTextEdit::InsertAndReplace(_) => panic!("unexpected insert/replace edit"),
        }
    }

    #[test]
    fn the_text_edit_replaces_the_typed_prefix_in_line_and_character_coordinates() {
        let (items, _) = items("SELECT 1;\nSELECT * FROM or|");
        let first = &items[0];
        assert_eq!(first.label, "orders");
        let edit = edit(first);
        assert_eq!(edit.new_text, "orders");
        assert_eq!(edit.range.start, Position::new(1, 14));
        assert_eq!(edit.range.end, Position::new(1, 16));
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        // `é` is two bytes but one LSP character: the edit starts at column 20
        let (items, plan) = items("SELECT 'é', 'é' FROM or|");
        assert_eq!(plan.prefix, "or");
        let edit = edit(&items[0]);
        let expected_column = "SELECT 'é', 'é' FROM ".chars().count() as u32;
        assert_eq!(edit.range.start, Position::new(0, expected_column));
        assert_eq!(edit.range.end, Position::new(0, expected_column + 2));
    }

    #[test]
    fn items_keep_their_ranking_and_highlight_on_char_boundaries() {
        let (items, _) = items("SELECT * FROM order|");
        let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["orders", "order_items"]);
        assert_eq!(items[0].filter_text.as_deref(), Some("order"));
        assert_eq!(items[0].sort_text.as_deref(), Some("00000"));
        assert_eq!(items[1].sort_text.as_deref(), Some("00001"));
        assert_eq!(items[0].detail.as_deref(), Some("table"));
    }

    #[test]
    fn non_prefix_matches_highlight_a_single_character() {
        let (items, _) = items("SELECT * FROM ite|");
        let item = items
            .iter()
            .find(|i| i.label == "order_items")
            .expect("boundary match");
        assert_eq!(item.filter_text.as_deref(), Some("o"));
    }

    #[test]
    fn strings_and_comments_produce_no_items() {
        let text = "SELECT 'or";
        assert!(completion::analyze(text, text.len(), Some(&DatabaseType::SQLite)).is_none());
    }
}
