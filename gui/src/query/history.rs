//! App-wide query history: every statement run from any query tab, newest
//! first (the pure model lives in `dbcrust_gui_core::history`).

use dbcrust_gui_core::history::{HistoryEntry, QueryHistory};
use gpui_kit::*;

/// The single history instance, installed lazily as a GPUI global.
#[derive(Default)]
struct AppHistory(QueryHistory);

impl Global for AppHistory {}

/// Remember a finished statement.
pub fn record(entry: HistoryEntry, cx: &mut App) {
    cx.default_global::<AppHistory>().0.push(entry);
}

/// The `limit` most recent statements, newest first.
pub fn recent(limit: usize, cx: &App) -> Vec<HistoryEntry> {
    cx.try_global::<AppHistory>()
        .map(|history| history.0.recent(limit))
        .unwrap_or_default()
}

/// Forget everything.
pub fn clear(cx: &mut App) {
    cx.default_global::<AppHistory>().0.clear();
}
