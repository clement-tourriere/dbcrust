//! SQL editor helpers: GPUI Kit's code editor configured for SQL, and the
//! schema-aware completion provider that feeds its completion menu.
//!
//! The provider runs on the UI thread. It reads the connection's cached
//! table list from [`AppState`], asks [`AppState::table_details`] for the
//! columns of the tables the current statement references (cached per
//! connection), and lets `dbcrust_gui_core::completion` rank the candidates.
//! Waiting for a table description is bounded, so the menu never hangs
//! behind a busy connection.

use crate::app_state::AppState;
use crate::preferences;
use dbcrust_gui_core::completion::{
    self, Candidate, CandidateKind, ColumnCandidate, Plan, SchemaView,
};
use dbcrust_gui_core::{ConnectionId, DatabaseType, TableEntry};
use gpui_kit::component::Theme;
use gpui_kit::component::input::{
    CompletionProvider, EditorState, RangeDecoration, RangeDecorationStyle, Rope, RopeExt as _,
};
use gpui_kit::*;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range as LspRange, TextEdit,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long a completion request waits for a table description before it
/// answers with what it has.
const DESCRIBE_PATIENCE: Duration = Duration::from_millis(450);
/// How often the cache is re-checked while waiting.
const DESCRIBE_POLL: Duration = Duration::from_millis(20);
/// A table that failed to describe is not retried for this long.
const DESCRIBE_RETRY_AFTER: Duration = Duration::from_secs(15);

/// Build the editor state of a query tab: SQL highlighting, line numbers,
/// no soft wrap, the completion provider, and `sql` as its content.
/// `connection` is shared with the provider so completion follows the tab's
/// connection.
pub fn new_editor_state(
    sql: String,
    connection: Rc<Cell<Option<ConnectionId>>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let provider: Rc<dyn CompletionProvider> = Rc::new(SqlCompletionProvider::new(connection));
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
    inflight: HashMap<(ConnectionId, String), Task<()>>,
    failed: HashMap<(ConnectionId, String), Instant>,
}

pub struct SqlCompletionProvider {
    connection: Rc<Cell<Option<ConnectionId>>>,
    fetches: Rc<RefCell<Fetches>>,
}

impl SqlCompletionProvider {
    pub fn new(connection: Rc<Cell<Option<ConnectionId>>>) -> Self {
        Self {
            connection,
            fetches: Rc::new(RefCell::new(Fetches::default())),
        }
    }
}

/// What a completion request needs from the app state, copied out so the
/// request can wait without holding any borrow.
struct Snapshot {
    id: ConnectionId,
    db_type: DatabaseType,
    tables: Vec<TableEntry>,
}

fn snapshot(id: ConnectionId, cx: &App) -> Option<Snapshot> {
    let state = AppState::global(cx);
    let state = state.read(cx);
    let live = state.connection(id)?;
    let tables = live
        .schema
        .tables()
        .map(|t| t.as_ref().clone())
        .unwrap_or_default();
    Some(Snapshot {
        id,
        db_type: live.summary.db_type.clone(),
        tables,
    })
}

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
    /// others. Returns the map and whether anything is still missing.
    fn gather(
        &self,
        id: ConnectionId,
        needed: &[TableEntry],
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
                if fetches.inflight.contains_key(&slot) {
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
                fetches.inflight.remove(&finished);
                if outcome.is_err() {
                    fetches.failed.insert(finished, Instant::now());
                }
            });
            self.fetches.borrow_mut().inflight.insert(slot, task);
        }
        (columns, pending)
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
        let source = text.to_string();
        let rope = text.clone();
        let connection = self.connection.get();
        let snap = connection.and_then(|id| snapshot(id, cx));
        let db_type = snap.as_ref().map(|s| s.db_type.clone());
        let separators = preferences::statement_separators(cx);
        let Some(plan) = completion::analyze_with(&source, offset, db_type.as_ref(), separators)
        else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let Some(snap) = snap else {
            // Not bound to a connection: keywords and functions only
            let items = build_items(&plan, None, &[], &HashMap::new(), &rope);
            return Task::ready(Ok(CompletionResponse::Array(items)));
        };

        let needed: Vec<TableEntry> = plan
            .needed_tables(&snap.tables, Some(&snap.db_type))
            .into_iter()
            .cloned()
            .collect();
        let (mut columns, mut pending) = self.gather(snap.id, &needed, cx);
        if !pending {
            let items = build_items(&plan, Some(&snap.db_type), &snap.tables, &columns, &rope);
            return Task::ready(Ok(CompletionResponse::Array(items)));
        }

        // Wait (briefly) for the descriptions to land in the cache
        let executor = cx.background_executor().clone();
        let fetches = self.fetches.clone();
        cx.spawn(async move |cx| {
            let deadline = Instant::now() + DESCRIBE_PATIENCE;
            while pending && Instant::now() < deadline {
                executor.timer(DESCRIBE_POLL).await;
                let (found, still) = cx.update(|cx| {
                    let mut found = HashMap::new();
                    let mut still = false;
                    for entry in &needed {
                        match cached_columns(snap.id, entry, cx) {
                            Some(c) => {
                                found.insert(entry.qualified_name(), c);
                            }
                            None => {
                                let slot = (snap.id, entry.qualified_name());
                                // Still being fetched? A failed table stops the wait
                                still |= fetches.borrow().inflight.contains_key(&slot);
                            }
                        }
                    }
                    (found, still)
                });
                columns = found;
                pending = still;
            }
            let items = build_items(&plan, Some(&snap.db_type), &snap.tables, &columns, &rope);
            Ok(CompletionResponse::Array(items))
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
) -> Vec<CompletionItem> {
    let candidates = completion::suggest(
        plan,
        &SchemaView {
            db_type,
            tables,
            columns,
        },
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
