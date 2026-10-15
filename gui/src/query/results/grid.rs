//! The data grid of one result set: GPUI Kit's virtualized `DataTable` with
//! cell selection, numeric-aware sorting, a right-click menu, copy, export
//! and the row inspector.
//!
//! The delegate owns an `Arc<ResultSet>` and a display→source row order; no
//! row data is ever cloned for rendering.

use super::inspector;
use super::model::{self, ColumnKind, ColumnProfile, SortDirection};
use crate::clipboard;
use dbcrust_gui_core::ResultSet;
use dbcrust_gui_core::export::{self, ExportFormat};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::table::{
    Column, DataTable, TableDelegate, TableEvent, TableSelection, TableState,
};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Size, WindowExt as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::borrow::Cow;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// Where the previous export went, so the next dialog opens there.
static LAST_EXPORT_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Marker types: a notification with the same marker replaces the previous
/// one instead of stacking toasts.
struct CopyNotice;
struct ExportNotice;

/// Index of the first data column in the table: column 0 is the fixed row
/// number.
const FIRST_DATA_COLUMN: usize = 1;

/// Row height of the grid, header included.
const ROW_HEIGHT: f32 = 28.;
/// Results with at least this many rows are sorted off the UI thread.
const BACKGROUND_SORT_ROWS: usize = 5_000;
/// Monospace values (identifiers, numbers, structured data) and headers.
const MONO_SIZE: f32 = 12.;
/// Prose values in the UI font read smaller than monospace at the same size.
const PROSE_SIZE: f32 = 12.5;

// ---------------------------------------------------------------------------
// Delegate
// ---------------------------------------------------------------------------

pub struct GridDelegate {
    result: Arc<ResultSet>,
    /// Display row → source row (shared with copies and exports running in
    /// the background).
    order: Arc<Vec<usize>>,
    /// Source row → display row.
    inverse: Vec<usize>,
    /// Bumped by every sort request: a background sort that finishes after
    /// a newer request is dropped.
    sort_generation: u64,
    sort_task: Option<Task<()>>,
    profiles: Vec<ColumnProfile>,
    /// Header labels, converted once.
    names: Vec<SharedString>,
    row_number_width: f32,
    /// The local sort: source column and direction. Only the loaded rows are
    /// ordered; the query is not re-run.
    sort: Option<(usize, SortDirection)>,
    /// Source row of the selected cell or row, for the row tint.
    selected_source: Option<usize>,
    /// `(display row, table column)` of the latest right press, consumed by
    /// the context menu when it opens.
    right_click: Rc<Cell<Option<(usize, usize)>>>,
}

impl GridDelegate {
    fn new(
        result: Arc<ResultSet>,
        profiles: Vec<ColumnProfile>,
        right_click: Rc<Cell<Option<(usize, usize)>>>,
    ) -> Self {
        let digits = result.rows.len().max(1).to_string().len();
        Self {
            order: Arc::new((0..result.rows.len()).collect()),
            inverse: (0..result.rows.len()).collect(),
            sort_generation: 0,
            sort_task: None,
            names: result
                .columns
                .iter()
                .map(|c| SharedString::from(c.clone()))
                .collect(),
            row_number_width: (digits as f32 * 7.4 + 22.).max(40.).round(),
            profiles,
            result,
            sort: None,
            selected_source: None,
            right_click,
        }
    }

    fn kind(&self, source_col: usize) -> ColumnKind {
        self.profiles
            .get(source_col)
            .map_or(ColumnKind::Prose, |p| p.kind)
    }

    /// Order the loaded rows by `sort` (or restore the order the query
    /// returned them in). A large result is sorted on the background
    /// executor: the header shows the new sort at once, the rows follow.
    fn apply_sort(
        &mut self,
        sort: Option<(usize, SortDirection)>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = sort;
        self.sort_generation += 1;
        let generation = self.sort_generation;
        let result = self.result.clone();
        let order_of = move || match sort {
            None => (0..result.rows.len()).collect(),
            Some((col, direction)) => model::sorted_order(&result, col, direction),
        };
        if self.result.rows.len() < BACKGROUND_SORT_ROWS {
            self.sort_task = None;
            self.install_order(order_of(), window, cx);
        } else {
            self.sort_task = Some(cx.spawn_in(window, async move |table, cx| {
                let order = cx.background_spawn(async move { order_of() }).await;
                table
                    .update_in(cx, |table, window, cx| {
                        let delegate = table.delegate_mut();
                        if delegate.sort_generation == generation {
                            delegate.sort_task = None;
                            delegate.install_order(order, window, cx);
                        }
                    })
                    .ok();
            }));
        }
        cx.notify();
    }

    fn install_order(
        &mut self,
        order: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let mut inverse = vec![0; order.len()];
        for (display, &source) in order.iter().enumerate() {
            inverse[source] = display;
        }
        self.inverse = inverse;
        let previous = std::mem::replace(&mut self.order, Arc::new(order));
        // The selection is by row position; keep it on the same source row.
        cx.defer_in(window, move |table, _window, cx| {
            let moved = |display: usize, table: &TableState<GridDelegate>| {
                let source = *previous.get(display)?;
                table.delegate().inverse.get(source).copied()
            };
            match table.selection() {
                TableSelection::Row(row) => {
                    if let Some(row) = moved(row, table) {
                        table.set_selected_row(row, cx);
                    }
                }
                TableSelection::Cell(row, col) => {
                    if let Some(row) = moved(row, table) {
                        table.set_selected_cell(row, col, cx);
                    }
                }
                TableSelection::Column(_) | TableSelection::None => {}
            }
        });
        cx.notify();
    }

    /// Header click: ascending, then descending, then the original order.
    fn cycle_sort(
        &mut self,
        source_col: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let next = match self.sort {
            Some((col, SortDirection::Ascending)) if col == source_col => {
                Some((col, SortDirection::Descending))
            }
            Some((col, SortDirection::Descending)) if col == source_col => None,
            _ => Some((source_col, SortDirection::Ascending)),
        };
        self.apply_sort(next, window, cx);
    }

    /// Source row shown at display row `row`.
    fn source_row(&self, row: usize) -> Option<usize> {
        self.order.get(row).copied()
    }

    /// Source column of table column `col` (`None` for the row number).
    fn source_col(col: usize) -> Option<usize> {
        col.checked_sub(FIRST_DATA_COLUMN)
    }

    fn is_numeric(&self, source_col: usize) -> bool {
        self.profiles.get(source_col).is_some_and(|p| p.numeric)
    }
}

impl TableDelegate for GridDelegate {
    fn columns_count(&self, _: &App) -> usize {
        FIRST_DATA_COLUMN + self.names.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let Some(ix) = Self::source_col(col_ix) else {
            let width = px(self.row_number_width);
            return Column::new("row-number", "#")
                .width(width)
                .min_width(width)
                .fixed_left()
                .resizable(false)
                .movable(false)
                .selectable(false)
                .text_right();
        };
        let width = self
            .profiles
            .get(ix)
            .map_or(model::MIN_COLUMN_WIDTH, |p| p.width);
        let name = self.names.get(ix).cloned().unwrap_or_default();
        // Sorting is the grid's own (see `render_th`): the header shows arrows
        // only for the sorted or hovered column
        let column = Column::new(format!("col-{ix}"), name)
            .width(px(width))
            .min_width(px(40.))
            .movable(false);
        if self.is_numeric(ix) {
            column.text_right()
        } else {
            column
        }
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let Some(ix) = Self::source_col(col_ix) else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_end()
                .font_family(mono)
                .text_size(px(MONO_SIZE))
                .text_color(muted)
                .child("#")
                .into_any_element();
        };

        let numeric = self.is_numeric(ix);
        let name = self.names.get(ix).cloned().unwrap_or_default();
        let direction = self
            .sort
            .filter(|(col, _)| *col == ix)
            .map(|(_, direction)| direction);
        let group = SharedString::from(format!("result-th-{ix}"));
        let tooltip = match direction {
            None => format!(
                "Sort the {} loaded {} by {name}",
                model::group_thousands(self.result.rows.len()),
                if self.result.rows.len() == 1 {
                    "row"
                } else {
                    "rows"
                },
            ),
            Some(SortDirection::Ascending) => {
                format!("Sorted by {name} ascending — click to reverse")
            }
            Some(SortDirection::Descending) => {
                format!("Sorted by {name} descending — click to restore the query order")
            }
        };
        let icon = match direction {
            Some(SortDirection::Descending) => IconName::SortDescending,
            Some(SortDirection::Ascending) => IconName::SortAscending,
            None => IconName::ChevronsUpDown,
        };

        // The arrow's slot is always reserved, so a header never changes
        // width when it is sorted or hovered
        let arrow = div()
            .flex_none()
            .w(px(14.))
            .flex()
            .justify_center()
            .when(direction.is_none(), |this| {
                this.opacity(0.)
                    .group_hover(&group, |this| this.opacity(0.55))
            })
            .child(Icon::new(icon).size_3().text_color(if direction.is_some() {
                theme.foreground
            } else {
                muted
            }));
        let label = div().min_w_0().truncate().child(name);

        h_flex()
            .id(("result-th", ix))
            .test_support()
            .group(group)
            .size_full()
            .gap_1()
            .items_center()
            .cursor_pointer()
            .font_family(mono)
            .font_weight(FontWeight::MEDIUM)
            .text_size(px(MONO_SIZE))
            .text_color(if direction.is_some() {
                theme.foreground
            } else {
                muted
            })
            .on_click(cx.listener(move |table, _, window, cx| {
                cx.stop_propagation();
                table.delegate_mut().cycle_sort(ix, window, cx);
            }))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            // Numbers end at the cell's right edge, like their values: the
            // arrow goes before the label there, after it everywhere else
            .map(|this| {
                if numeric {
                    this.justify_end().child(arrow).child(label)
                } else {
                    this.child(label.flex_1()).child(arrow)
                }
            })
            .into_any_element()
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // Keyed by the source row, so a row keeps its identity across sorts.
        let source = self.source_row(row_ix).unwrap_or(row_ix);
        let theme = cx.theme();
        div()
            .id(("result-row", source))
            // Hairlines between rows, not a ruled sheet
            .border_color(theme.border.opacity(0.4))
            .when(self.selected_source == Some(source), |this| {
                this.bg(theme.tokens.table_active.opacity(0.45))
            })
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let source_row = self.source_row(row_ix).unwrap_or(row_ix);
        let right_click = self.right_click.clone();

        let (content, mono) = match Self::source_col(col_ix) {
            None => (
                div()
                    .flex_1()
                    .text_right()
                    .text_color(muted.opacity(0.7))
                    .child(SharedString::from((source_row + 1).to_string()))
                    .into_any_element(),
                true,
            ),
            Some(ix) => {
                let numeric = self.is_numeric(ix);
                let mono = self.kind(ix) != ColumnKind::Prose;
                match self.result.rows.get(source_row).and_then(|row| row.get(ix)) {
                    // Empty text and NULL are not values: UI font, italic,
                    // and distinct from each other
                    Some(Some(value)) if value.is_empty() => (
                        div()
                            .flex_1()
                            .italic()
                            .text_color(muted.opacity(0.55))
                            .when(numeric, |this| this.text_right())
                            .child("empty")
                            .into_any_element(),
                        false,
                    ),
                    Some(Some(value)) => {
                        let text = match model::display_text(value) {
                            Cow::Borrowed(text) => SharedString::from(text.to_string()),
                            Cow::Owned(text) => SharedString::from(text),
                        };
                        (
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(numeric, |this| this.text_right())
                                .child(text)
                                .into_any_element(),
                            mono,
                        )
                    }
                    _ => (
                        div()
                            .flex_1()
                            .italic()
                            .text_color(muted.opacity(0.75))
                            .when(numeric, |this| this.text_right())
                            .child("NULL")
                            .into_any_element(),
                        false,
                    ),
                }
            }
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .when(mono, |this| {
                this.font_family(theme.mono_font_family.clone())
                    .text_size(px(MONO_SIZE))
            })
            .when(!mono, |this| this.text_size(px(PROSE_SIZE)))
            .on_mouse_down(MouseButton::Right, move |_, _, _| {
                right_click.set(Some((row_ix, col_ix)));
            })
            .child(content)
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(
                Icon::new(IconName::Inbox)
                    .size_6()
                    .text_color(cx.theme().muted_foreground.opacity(0.6)),
            )
            .child("No rows")
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        let Some(source_row) = self.source_row(row_ix) else {
            return String::new();
        };
        match Self::source_col(col_ix) {
            None => (source_row + 1).to_string(),
            Some(ix) => self
                .result
                .rows
                .get(source_row)
                .and_then(|row| row.get(ix))
                .and_then(|cell| cell.clone())
                .unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// What a right press landed on, in source coordinates.
#[derive(Clone, Copy)]
struct ClickTarget {
    display_row: usize,
    row: usize,
    /// `None` on the row number.
    col: Option<usize>,
}

pub struct ResultGrid {
    result: Arc<ResultSet>,
    table: Entity<TableState<GridDelegate>>,
    right_click: Rc<Cell<Option<(usize, usize)>>>,
    /// The row inspector sheet opened from this grid is (probably) showing.
    inspector_open: Rc<Cell<bool>>,
    /// Text for the clipboard being rendered in the background; a newer copy
    /// of this grid replaces (cancels) it.
    copy_task: Option<Task<()>>,
    /// How many times it rendered (tests watch what a repaint reaches).
    #[cfg(test)]
    pub(super) renders: usize,
    _subscriptions: Vec<Subscription>,
}

impl ResultGrid {
    pub fn new(result: Arc<ResultSet>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profiles = model::profile_columns(&result);
        let right_click = Rc::new(Cell::new(None));
        let delegate = GridDelegate::new(result.clone(), profiles, right_click.clone());
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .cell_selectable(true)
                .row_header(false)
                .col_movable(false)
                // A header click sorts; whole columns are copied from the menu
                .col_selectable(false)
                .loop_selection(false)
        });
        let subscription = cx.subscribe_in(&table, window, Self::on_table_event);
        // The footer says what the local sort did: repaint with the table
        let observation = cx.observe(&table, |_, _, cx| cx.notify());
        Self {
            result,
            table,
            right_click,
            inspector_open: Rc::new(Cell::new(false)),
            copy_task: None,
            #[cfg(test)]
            renders: 0,
            _subscriptions: vec![subscription, observation],
        }
    }

    /// The local sort, if any: column name and direction. It orders the rows
    /// that were loaded; the query is not re-run.
    pub fn sort(&self, cx: &App) -> Option<(SharedString, SortDirection)> {
        let delegate = self.table.read(cx).delegate();
        let (col, direction) = delegate.sort?;
        Some((delegate.names.get(col)?.clone(), direction))
    }

    /// Back to the order the query returned.
    pub fn clear_sort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().apply_sort(None, window, cx);
        });
    }

    fn on_table_event(
        &mut self,
        table: &Entity<TableState<GridDelegate>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // The row number selects the whole row
            TableEvent::SelectCell(row, col) if *col < FIRST_DATA_COLUMN => {
                let row = *row;
                table.update(cx, |table, cx| table.set_selected_row(row, cx));
            }
            TableEvent::DoubleClickedCell(row, _) | TableEvent::DoubleClickedRow(row) => {
                self.inspect_row(*row, window, cx);
            }
            _ => {}
        }
        // The tint of the selected row follows the cell, by mouse or keyboard
        let selected = match event {
            TableEvent::SelectCell(row, _) | TableEvent::SelectRow(row) => Some(Some(*row)),
            TableEvent::ClearSelection => Some(None),
            _ => None,
        };
        if let Some(selected) = selected {
            table.update(cx, |table, cx| {
                let source = selected.and_then(|row| table.delegate().source_row(row));
                if table.delegate().selected_source != source {
                    table.delegate_mut().selected_source = source;
                    cx.notify();
                }
            });
        }
    }

    // --- selection helpers --------------------------------------------------

    /// Select table column `col` (the row number is column 0), as the user would.
    #[cfg(test)]
    pub(crate) fn select_column(&mut self, col: usize, cx: &mut Context<Self>) {
        self.table
            .update(cx, |table, cx| table.set_selected_col(col, cx));
    }

    fn source_row(&self, display_row: usize, cx: &App) -> Option<usize> {
        self.table.read(cx).delegate().source_row(display_row)
    }

    fn display_order(&self, cx: &App) -> Arc<Vec<usize>> {
        self.table.read(cx).delegate().order.clone()
    }

    /// Whether Copy has something to copy.
    pub fn has_selection(&self, cx: &App) -> bool {
        self.table.read(cx).selection() != TableSelection::None
    }

    fn take_click_target(&self, cx: &App) -> Option<ClickTarget> {
        let (display_row, col) = self.right_click.take()?;
        Some(ClickTarget {
            display_row,
            row: self.source_row(display_row, cx)?,
            col: GridDelegate::source_col(col),
        })
    }

    // --- copy ---------------------------------------------------------------

    pub fn copy_cell(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = self.result.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        let text = export::value_to_text(value.as_deref()).to_string();
        let notice = if value.is_none() {
            "Copied NULL"
        } else {
            "Copied value"
        };
        copy_text(text, notice, window, cx);
    }

    pub fn copy_row_tsv(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = export::row_to_tsv(&self.result, row) {
            copy_text(text, "Copied row as TSV", window, cx);
        }
    }

    pub fn copy_row_json(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = export::row_to_json(&self.result, row) {
            copy_text(text, "Copied row as JSON", window, cx);
        }
    }

    pub fn copy_column_name(&mut self, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = self.result.columns.get(col) {
            copy_text(name.clone(), "Copied column name", window, cx);
        }
    }

    pub fn copy_column_values(&mut self, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.result.clone();
        let order = self.display_order(cx);
        let notice = format!(
            "Copied {} values",
            model::group_thousands(self.result.row_count())
        );
        self.copy_rendered(
            move || export::column_to_text(&result, col, Some(&order)),
            notice,
            window,
            cx,
        );
    }

    /// The whole result (in the order shown) in `format`.
    pub fn copy_all(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.result.clone();
        let order = self.display_order(cx);
        let notice = format!(
            "Copied {} as {}",
            model::row_count_label(self.result.row_count()),
            format.label()
        );
        self.copy_rendered(
            move || format.render(&result, Some(&order)),
            notice,
            window,
            cx,
        );
    }

    /// Render the text of a copy of many rows on the background executor,
    /// then put it on the clipboard — unless another copy was asked for
    /// meanwhile, here or anywhere else in the app (see `crate::clipboard`).
    fn copy_rendered(
        &mut self,
        render: impl FnOnce() -> String + Send + 'static,
        notice: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ticket = clipboard::reserve(cx);
        let text = cx.background_spawn(async move { render() });
        self.copy_task = Some(cx.spawn_in(window, async move |_, cx| {
            let text = text.await;
            cx.update(|window, cx| {
                if ticket.write(text, cx) {
                    copy_notice(notice, window, cx);
                }
            })
            .ok();
        }));
    }

    /// Copy what is selected: a cell, a row (as TSV) or a column.
    pub fn copy_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let selection = self.table.read(cx).selection();
        match selection {
            TableSelection::Cell(display_row, col) => {
                let Some(row) = self.source_row(display_row, cx) else {
                    return false;
                };
                match GridDelegate::source_col(col) {
                    Some(col) => self.copy_cell(row, col, window, cx),
                    None => self.copy_row_tsv(row, window, cx),
                }
                true
            }
            TableSelection::Row(display_row) => {
                let Some(row) = self.source_row(display_row, cx) else {
                    return false;
                };
                self.copy_row_tsv(row, window, cx);
                true
            }
            TableSelection::Column(col) => match GridDelegate::source_col(col) {
                Some(col) => {
                    self.copy_column_values(col, window, cx);
                    true
                }
                None => false,
            },
            TableSelection::None => false,
        }
    }

    // --- inspect --------------------------------------------------------------

    pub fn inspect_row(&mut self, display_row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.source_row(display_row, cx) {
            inspector::open(
                self.result.clone(),
                row,
                self.inspector_open.clone(),
                window,
                cx,
            );
        }
    }

    /// Close the row inspector if this grid opened it (its row would be
    /// stale once the results are replaced).
    pub fn close_inspector(&self, window: &mut Window, cx: &mut App) {
        if self.inspector_open.replace(false) {
            window.close_sheet(cx);
        }
    }

    // --- export ---------------------------------------------------------------

    /// Ask for a file, then write the result (in the order shown) to it off
    /// the UI thread.
    pub fn export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.result.clone();
        let order = self.display_order(cx);
        let file_name = format!("results.{}", format.extension());
        let receiver = cx.prompt_for_new_path(&export_directory(), Some(&file_name));

        cx.spawn_in(window, async move |_this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                // Cancelled in the dialog
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    cx.update(|window, cx| {
                        window.push_notification(
                            Notification::error(format!("Couldn’t open the save dialog: {error}"))
                                .id::<ExportNotice>(),
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            };

            let rows = order.len();
            let target = path.clone();
            let written = cx
                .background_spawn(async move {
                    let text = format.render(&result, Some(&order));
                    std::fs::write(&target, text)
                })
                .await;

            cx.update(|window, cx| match written {
                Ok(()) => {
                    remember_export_directory(&path);
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    let reveal = path.clone();
                    window.push_notification(
                        Notification::success(format!(
                            "Exported {} to {name}",
                            model::row_count_label(rows)
                        ))
                        .id::<ExportNotice>()
                        .action(move |_, _, _| {
                            let reveal = reveal.clone();
                            Button::new("reveal-export")
                                .label("Show in folder")
                                .ghost()
                                .on_click(move |_, _, cx| cx.reveal_path(&reveal))
                        })
                        .autohide(true),
                        cx,
                    );
                }
                Err(error) => {
                    window.push_notification(
                        Notification::error(format!("Couldn’t write {}: {error}", path.display()))
                            .id::<ExportNotice>(),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Put `text` on the clipboard and say so (one toast, replaced by the next).
pub(super) fn copy_text(
    text: String,
    notice: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    clipboard::write(text, cx);
    copy_notice(notice, window, cx);
}

fn copy_notice(notice: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::success(notice).id::<CopyNotice>(), cx);
}

fn export_directory() -> PathBuf {
    if let Ok(last) = LAST_EXPORT_DIR.lock()
        && let Some(dir) = last.as_ref()
    {
        return dir.clone();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    match home {
        Some(home) => {
            let downloads = home.join("Downloads");
            if downloads.is_dir() { downloads } else { home }
        }
        None => std::env::temp_dir(),
    }
}

fn remember_export_directory(file: &Path) {
    if let (Ok(mut last), Some(dir)) = (LAST_EXPORT_DIR.lock(), file.parent()) {
        *last = Some(dir.to_path_buf());
    }
}

impl Render for ResultGrid {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let grid = cx.entity();
        div()
            .id(("result-grid", self.table.entity_id()))
            .size_full()
            .context_menu(move |menu, window, cx| build_context_menu(menu, &grid, window, cx))
            .child(
                DataTable::new(&self.table)
                    .with_size(Size::Size(px(ROW_HEIGHT)))
                    .bordered(false)
                    .stripe(false),
            )
    }
}

// ---------------------------------------------------------------------------
// Context menu
// ---------------------------------------------------------------------------

/// The right-click menu of a cell. Built (once per right click) after the
/// press has been recorded by the cell under the pointer; a press anywhere
/// else (header, empty area) leaves nothing to act on, so the menu is empty
/// and GPUI Kit does not show it.
fn build_context_menu(
    menu: PopupMenu,
    grid: &Entity<ResultGrid>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(target) = grid.read(cx).take_click_target(cx) else {
        return menu;
    };
    let col = target.col;

    let item =
        |label: &'static str,
         icon: Option<IconName>,
         run: fn(&mut ResultGrid, ClickTarget, &mut Window, &mut Context<ResultGrid>)| {
            let grid = grid.clone();
            PopupMenuItem::new(label)
                .when_some(icon, |item, icon| item.icon(icon))
                .on_click(move |_, window, cx| {
                    grid.update(cx, |grid, cx| run(grid, target, window, cx));
                })
        };

    let mut menu = menu.min_w(px(210.));
    if col.is_some() {
        menu = menu
            .item(item("Copy value", Some(IconName::Copy), |g, t, w, cx| {
                if let Some(col) = t.col {
                    g.copy_cell(t.row, col, w, cx);
                }
            }))
            .item(item("Copy column name", None, |g, t, w, cx| {
                if let Some(col) = t.col {
                    g.copy_column_name(col, w, cx);
                }
            }))
            .item(item("Copy column values", None, |g, t, w, cx| {
                if let Some(col) = t.col {
                    g.copy_column_values(col, w, cx);
                }
            }));
    }
    menu = menu
        .item(item("Copy row as TSV", None, |g, t, w, cx| {
            g.copy_row_tsv(t.row, w, cx)
        }))
        .item(item("Copy row as JSON", None, |g, t, w, cx| {
            g.copy_row_json(t.row, w, cx)
        }))
        .separator()
        .submenu("Copy all as", window, cx, {
            let grid = grid.clone();
            move |menu, _, _| {
                ExportFormat::ALL.iter().fold(menu, |menu, &format| {
                    let grid = grid.clone();
                    menu.item(
                        PopupMenuItem::new(format.label()).on_click(move |_, window, cx| {
                            grid.update(cx, |grid, cx| grid.copy_all(format, window, cx));
                        }),
                    )
                })
            }
        })
        .separator()
        .item(item(
            "Inspect row…",
            Some(IconName::Eye),
            |g, t, w, cx| g.inspect_row(t.display_row, w, cx),
        ));
    menu
}
