//! Sidebar schema explorer: one section per live connection, its schema
//! tree below it (schemas → kinds → tables → columns), a cross-connection
//! filter, and context actions.
//!
//! The tree is a flat, virtualized list (see [`rows`]): only expanded nodes
//! produce rows and only visible rows produce elements. Column lists load
//! lazily the first time a table is expanded.
//!
//! Contract used by the workspace: `Explorer::new(window, cx)`.

mod dialogs;
mod menus;
mod rows;
#[cfg(test)]
mod tests;

use crate::app_state::{AppEvent, AppState};
use crate::theme;
use crate::widgets;
use dbcrust_gui_core::schema;
use dbcrust_gui_core::{ConnectionId, DatabaseType, GuiError, TableDetails, TableEntry};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _},
    notification::Notification,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rows::{BuildContext, DetailKey, Row, RowKind};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// Every row has the same height: the list is virtualized by it.
const ROW_HEIGHT: f32 = 26.;
const INDENT: f32 = 12.;

/// What "Generate …" produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Generate {
    Select,
    Insert,
    Ddl,
}

pub struct Explorer {
    focus_handle: FocusHandle,
    filter: Entity<InputState>,
    filter_text: SharedString,
    rows: Vec<Row>,
    scroll: UniformListScrollHandle,
    selected: Option<SharedString>,
    /// Explicit open/closed state by row id (absent = the row's default).
    expansion: HashMap<SharedString, bool>,
    /// Table descriptions being fetched, and fetches that failed.
    loading: HashSet<DetailKey>,
    errors: HashMap<DetailKey, String>,
    /// What `rows` were built from (see [`rows::Shape`]), and the expanded
    /// tables still showing no columns, loading or failed.
    shape: rows::Shape,
    awaiting: Vec<DetailKey>,
    /// Per-connection display facts, gathered once per render for the rows.
    looks: HashMap<ConnectionId, Rc<ConnectionLook>>,
    active: Option<ConnectionId>,
    #[cfg(test)]
    rebuilds: usize,
    _subscriptions: Vec<Subscription>,
}

impl Explorer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter tables")
                .clean_on_escape()
        });
        let subscriptions = vec![
            // A description landing emits `SchemaChanged` without notifying
            cx.subscribe(&state, |this, _, event: &AppEvent, cx| match event {
                AppEvent::SchemaChanged(_) | AppEvent::Connected(_) | AppEvent::Disconnected(_) => {
                    this.sync_with_state(cx)
                }
                AppEvent::ActiveConnectionChanged(_) => cx.notify(),
                _ => {}
            }),
            // read-only toggles, session renames, … notify without an event
            cx.observe(&state, |this, _, cx| this.sync_with_state(cx)),
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter_text = input.read(cx).value();
                    this.rebuild(cx);
                }
            }),
        ];
        let mut explorer = Self {
            focus_handle: cx.focus_handle(),
            filter,
            filter_text: SharedString::default(),
            rows: Vec::new(),
            scroll: UniformListScrollHandle::default(),
            selected: None,
            expansion: HashMap::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
            shape: rows::Shape::default(),
            awaiting: Vec::new(),
            looks: HashMap::new(),
            active: None,
            #[cfg(test)]
            rebuilds: 0,
            _subscriptions: subscriptions,
        };
        explorer.rebuild(cx);
        explorer
    }

    /// Put the caret in the filter box.
    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter.read(cx).focus_handle(cx).focus(window, cx);
    }

    // ---- model ----------------------------------------------------------

    /// The app state changed. Rebuilding walks every table of every
    /// connection, so it only happens when what the rows are made of
    /// changed; anything else (labels, read-only, the active connection, a
    /// description fetched for completion) is a repaint.
    fn sync_with_state(&mut self, cx: &mut Context<Self>) {
        let stale = {
            let state = AppState::global(cx).read(cx);
            let connections = state.connections();
            !self.shape.matches(connections) || rows::details_arrived(&self.awaiting, connections)
        };
        if stale {
            self.rebuild(cx);
        } else {
            cx.notify();
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let state = AppState::global(cx);
        let built = {
            let state = state.read(cx);
            let context = BuildContext {
                filter: self.filter_text.as_ref(),
                expansion: &self.expansion,
                errors: &self.errors,
            };
            self.shape = rows::Shape::of(state.connections());
            rows::build_rows(state.connections(), &context)
        };
        self.rows = built.rows;
        self.awaiting = built.awaiting;
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
        if let Some(selected) = &self.selected
            && !self.rows.iter().any(|row| &row.id == selected)
        {
            self.selected = None;
        }
        for (connection, key) in built.needs_details {
            self.request_details(connection, key, cx);
        }
        cx.notify();
    }

    /// Fetch a table's description once (the answer lands in
    /// `LiveConnection::details`, which re-renders the tree).
    fn request_details(&mut self, connection: ConnectionId, key: String, cx: &mut Context<Self>) {
        let slot = (connection, key.clone());
        if !self.loading.insert(slot.clone()) {
            return;
        }
        self.errors.remove(&slot);
        let described =
            AppState::global(cx).update(cx, |state, cx| state.table_details(connection, key, cx));
        cx.spawn(async move |this, cx| {
            let result = described.await;
            let _ = this.update(cx, |this, cx| {
                this.loading.remove(&slot);
                if let Err(error) = result {
                    this.errors.insert(slot, error.to_string());
                }
                this.rebuild(cx);
            });
        })
        .detach();
    }

    fn row_index(&self, id: &SharedString) -> Option<usize> {
        self.rows.iter().position(|row| &row.id == id)
    }

    fn set_open(&mut self, id: &SharedString, open: bool, cx: &mut Context<Self>) {
        self.expansion.insert(id.clone(), open);
        self.rebuild(cx);
    }

    fn toggle(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if let Some(row) = self.row_index(id).map(|ix| &self.rows[ix])
            && row.expandable
        {
            let open = !row.expanded;
            self.set_open(id, open, cx);
        }
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        let open: Vec<SharedString> = self
            .rows
            .iter()
            .filter(|row| row.expandable && row.expanded && row.depth > 0)
            .map(|row| row.id.clone())
            .collect();
        for id in open {
            self.expansion.insert(id, false);
        }
        self.rebuild(cx);
    }

    fn select(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(id) {
            self.selected = Some(id.clone());
            cx.notify();
        }
    }

    fn set_active(&self, connection: ConnectionId, cx: &mut Context<Self>) {
        AppState::global(cx).update(cx, |state, cx| state.set_active(Some(connection), cx));
    }

    // ---- interaction ----------------------------------------------------

    fn click_row(
        &mut self,
        id: &SharedString,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.row_index(id) else { return };
        window.focus(&self.focus_handle, cx);
        self.select(id, cx);
        let row = self.rows[ix].clone();
        match &row.kind {
            RowKind::Connection => {
                self.set_active(row.connection, cx);
                if !row.expanded {
                    self.set_open(id, true, cx);
                }
            }
            RowKind::Section { .. } | RowKind::Group { .. } => {
                if click_count == 1 {
                    self.toggle(id, cx);
                }
            }
            RowKind::Table { table, .. } => {
                self.set_active(row.connection, cx);
                if click_count >= 2 {
                    self.open_table(row.connection, table.entry().clone(), cx);
                }
            }
            RowKind::Column { .. } => self.set_active(row.connection, cx),
            _ => {}
        }
    }

    /// Enter / double click: the row's primary action.
    fn open_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        match &row.kind {
            RowKind::Connection => {
                self.set_active(row.connection, cx);
                if !row.expanded {
                    self.set_open(&row.id, true, cx);
                }
            }
            RowKind::Section { .. } | RowKind::Group { .. } => self.toggle(&row.id, cx),
            RowKind::Table { table, .. } => {
                self.set_active(row.connection, cx);
                self.open_table(row.connection, table.entry().clone(), cx);
            }
            RowKind::SchemaFailed { .. } => self.refresh(row.connection, cx),
            RowKind::ColumnsFailed { key, .. } => {
                self.retry_details(row.connection, key.clone(), cx)
            }
            _ => {}
        }
    }

    fn open_table(&self, connection: ConnectionId, table: TableEntry, cx: &mut Context<Self>) {
        AppState::global(cx).update(cx, |state, cx| state.open_table(connection, table, cx));
    }

    fn refresh(&self, connection: ConnectionId, cx: &mut Context<Self>) {
        AppState::global(cx).update(cx, |state, cx| state.refresh_schema(connection, cx));
    }

    fn retry_details(&mut self, connection: ConnectionId, key: String, cx: &mut Context<Self>) {
        self.errors.remove(&(connection, key));
        self.rebuild(cx);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| self.row_index(id))
            .map(|ix| ix as isize);
        let next = match current {
            Some(ix) => (ix + delta).clamp(0, self.rows.len() as isize - 1),
            None if delta >= 0 => 0,
            None => self.rows.len() as isize - 1,
        } as usize;
        self.selected = Some(self.rows[next].id.clone());
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let filtering = self.filter.read(cx).focus_handle(cx).is_focused(window);
        let modifiers = event.keystroke.modifiers;
        if modifiers.modified() {
            return;
        }
        let selected = self
            .selected
            .as_ref()
            .and_then(|id| self.row_index(id))
            .map(|ix| (ix, self.rows[ix].clone()));
        match event.keystroke.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "home" if !filtering => self.move_selection(isize::MIN / 2, cx),
            "end" if !filtering => self.move_selection(isize::MAX / 2, cx),
            "enter" => {
                if let Some((ix, _)) = selected {
                    self.open_row(ix, cx);
                }
            }
            "right" if !filtering => {
                if let Some((ix, row)) = selected {
                    if row.expandable && !row.expanded {
                        self.set_open(&row.id, true, cx);
                    } else if row.expandable && ix + 1 < self.rows.len() {
                        self.move_selection(1, cx);
                    }
                }
            }
            "left" if !filtering => {
                if let Some((ix, row)) = selected {
                    if row.expandable && row.expanded {
                        self.set_open(&row.id, false, cx);
                    } else if let Some(parent) = (0..ix)
                        .rev()
                        .find(|&candidate| self.rows[candidate].depth < row.depth)
                    {
                        self.selected = Some(self.rows[parent].id.clone());
                        self.scroll.scroll_to_item(parent, ScrollStrategy::Nearest);
                        cx.notify();
                    }
                }
            }
            "space" if !filtering => {
                if let Some((_, row)) = selected {
                    self.toggle(&row.id, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- actions invoked from menus and dialogs -------------------------

    pub(crate) fn run_in_new_tab(
        &self,
        connection: ConnectionId,
        title: Option<String>,
        sql: String,
        run: bool,
        cx: &mut App,
    ) {
        AppState::global(cx).update(cx, |state, cx| {
            state.open_query(connection, title, sql, run, cx)
        });
    }

    /// Generate SQL from a table's description (fetched when not cached)
    /// and open it in a new, unexecuted tab — or copy the DDL.
    pub(crate) fn generate(
        &mut self,
        connection: ConnectionId,
        entry: TableEntry,
        kind: Generate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = AppState::global(cx);
        let Some(db_type) = state
            .read(cx)
            .connection(connection)
            .map(|live| live.summary.db_type.clone())
        else {
            return;
        };
        let key = schema::describe_name(&db_type, &entry);
        let described = state.update(cx, |state, cx| state.table_details(connection, key, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = described.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(details) => {
                    this.finish_generate(connection, &db_type, &entry, &details, kind, window, cx)
                }
                Err(error) => notify_error(window, "Could not describe the table", &error, cx),
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_generate(
        &mut self,
        connection: ConnectionId,
        db_type: &DatabaseType,
        entry: &TableEntry,
        details: &Arc<TableDetails>,
        kind: Generate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            Generate::Select => {
                let columns: Vec<String> = details.columns.iter().map(|c| c.name.clone()).collect();
                let sql =
                    schema::select_columns_sql(db_type, entry, &columns, schema::PREVIEW_ROWS);
                self.run_in_new_tab(connection, Some(entry.name.clone()), sql, false, cx);
            }
            Generate::Insert => match schema::insert_template_sql(db_type, entry, &details.columns)
            {
                Some(sql) => {
                    self.run_in_new_tab(
                        connection,
                        Some(format!("insert {}", entry.name)),
                        sql,
                        false,
                        cx,
                    );
                }
                None => window.push_notification(
                    Notification::warning("INSERT templates are not available for this engine"),
                    cx,
                ),
            },
            Generate::Ddl => {
                let ddl = schema::ddl_summary(db_type, details);
                copy_text(ddl, &format!("DDL of {}", entry.name), window, cx);
            }
        }
    }

    pub(crate) fn pick_database(
        &mut self,
        connection: ConnectionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handle) = AppState::global(cx).read(cx).handle(connection) else {
            return;
        };
        let listing = handle.list_databases();
        cx.spawn_in(window, async move |this, cx| {
            let result = listing.await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(databases) => dialogs::open_database_picker(connection, databases, window, cx),
                Err(error) => notify_error(window, "Could not list databases", &error, cx),
            });
        })
        .detach();
    }

    // ---- rendering ------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .h(px(32.))
            .px_3()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
                    .child("Explorer"),
            )
            .child(
                Button::new("explorer-new-connection")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("New connection")
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |state, cx| state.open_home(cx));
                    }),
            )
            .child(
                Button::new("explorer-refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh the active connection")
                    .on_click(cx.listener(|_, _, _, cx| {
                        let active = AppState::global(cx).read(cx).active_id();
                        if let Some(active) = active {
                            AppState::global(cx)
                                .update(cx, |state, cx| state.refresh_schema(active, cx));
                        }
                    })),
            )
            .child(
                Button::new("explorer-collapse")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronsUpDown)
                    .tooltip("Collapse all")
                    .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
            )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .child(
                Icon::new(IconName::HardDrive)
                    .size(px(28.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("No connections"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child("Connect to a database to browse its tables."),
            )
            .child(
                Button::new("explorer-connect")
                    .primary()
                    .small()
                    .label("Connect to a database")
                    .on_click(|_, _, cx| {
                        AppState::global(cx).update(cx, |state, cx| state.open_home(cx));
                    }),
            )
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let weak = cx.weak_entity();
        let selected = self.selected.as_ref() == Some(&row.id);
        let focused = self.focus_handle.contains_focused(window, cx);
        let filter = self.filter_text.clone();

        let connection = self.looks.get(&row.connection).cloned();
        let active = self.active == Some(row.connection);
        let surfaces = theme::surfaces(cx);
        let theme = cx.theme();
        let (fg, muted, danger) = (theme.sidebar_foreground, surfaces.label, theme.danger);
        let row_bg = match (selected, focused) {
            (true, true) => Some(surfaces.selected),
            (true, false) => Some(surfaces.selected_inactive),
            (false, _) => None,
        };

        let id = row.id.clone();
        let mut line = h_flex()
            .id(id.clone())
            .group(id.clone())
            .relative()
            .size_full()
            .rounded(px(5.))
            .pl(px(4. + row.depth as f32 * INDENT))
            .pr(px(6.))
            .gap(px(4.))
            .text_size(px(12.))
            .text_color(fg)
            .cursor_pointer()
            .when_some(row_bg, |this, bg| this.bg(bg))
            .when(row_bg.is_none(), |this| {
                this.hover(move |style| style.bg(surfaces.hover))
            })
            // the marker of the selected row: this row only, never the tree
            .when(selected && focused, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(5.))
                        .bottom(px(5.))
                        .w(px(2.))
                        .rounded_full()
                        .bg(surfaces.selected_edge),
                )
            })
            .on_click(cx.listener({
                let id = id.clone();
                move |this, event: &ClickEvent, window, cx| {
                    if event.standard_click() {
                        this.click_row(&id, event.click_count(), window, cx);
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let id = id.clone();
                    move |this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.select(&id, cx);
                    }
                }),
            );

        // chevron (toggles without selecting)
        line = line.child(if row.expandable {
            let toggle_id = id.clone();
            div()
                .id(child_id(&id, "chevron"))
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(if row.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(12.))
                    .text_color(muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle(&toggle_id, cx);
                    cx.stop_propagation();
                }))
                .into_any_element()
        } else {
            div().flex_none().size(px(14.)).into_any_element()
        });

        // Actions that appear on hover (and on the focused row) on top of
        // the row's right end: the label never moves, and the row keeps its
        // full width while idle.
        let hover_actions = |actions: Vec<AnyElement>| {
            let backdrop = row_bg.unwrap_or(surfaces.hover);
            h_flex()
                .absolute()
                .right(px(2.))
                .top_0()
                .bottom_0()
                .items_center()
                .gap_0p5()
                .pl_2()
                .pr_0p5()
                .rounded_r(px(5.))
                .bg(backdrop)
                // a click on an action is not a click on the row
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when(!(selected && focused), |this| {
                    this.invisible()
                        .group_hover(id.clone(), |style| style.visible())
                })
                .children(actions)
        };
        let more_button = || {
            let menu_row = row.clone();
            let weak = weak.clone();
            Button::new(child_id(&id, "more"))
                .ghost()
                .xsmall()
                .icon(IconName::Ellipsis)
                .tooltip("More actions")
                .dropdown_menu(move |menu, window, cx| {
                    menus::build(menu, &menu_row, weak.clone(), window, cx)
                })
                .into_any_element()
        };

        match &row.kind {
            RowKind::Connection => {
                let Some(look) = &connection else {
                    return div()
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px(px(4.))
                        .child(line)
                        .into_any_element();
                };
                line = line
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .bg(look.accent),
                    )
                    .child(widgets::db_badge(&look.db_type, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if active { fg } else { muted })
                            .child(look.label.clone()),
                    );
                if look.read_only {
                    line = line.child(
                        div()
                            .id(SharedString::from(format!("{id}-read-only")))
                            .flex_none()
                            .text_color(cx.theme().warning)
                            .child(Icon::new(IconName::Lock).xsmall())
                            .tooltip(|window, cx| Tooltip::new("Read-only").build(window, cx)),
                    );
                }
                if look.loading {
                    line = line.child(Spinner::new().xsmall().color(muted));
                }
                let connection_id = row.connection;
                let new_query = Button::new(child_id(&id, "new-query"))
                    .ghost()
                    .xsmall()
                    .icon(IconName::SquareTerminal)
                    .tooltip("New query")
                    .on_click(move |_, _, cx| {
                        AppState::global(cx).update(cx, |state, cx| {
                            state.open_query(connection_id, None, String::new(), false, cx)
                        });
                        cx.stop_propagation();
                    })
                    .into_any_element();
                line = line.child(hover_actions(vec![new_query, more_button()]));
                let tooltip = look.target.clone();
                line =
                    line.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx));
            }
            RowKind::SchemaLoading => {
                line = line
                    .text_color(muted)
                    .child(Spinner::new().xsmall().color(muted))
                    .child("Loading tables…");
            }
            RowKind::SchemaFailed { message } => {
                let connection_id = row.connection;
                let noun = connection
                    .as_ref()
                    .map(|look| rows::object_noun(&look.db_type))
                    .unwrap_or("tables");
                let message = message.clone();
                line = line
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(12.))
                            .text_color(danger),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(danger)
                            .child(format!("Couldn’t load {noun}")),
                    )
                    .child(
                        Button::new(child_id(&id, "retry"))
                            .outline()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .label("Retry")
                            .on_click(move |_, _, cx| {
                                AppState::global(cx).update(cx, |state, cx| {
                                    state.refresh_schema(connection_id, cx)
                                });
                                cx.stop_propagation();
                            }),
                    )
                    .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx));
            }
            RowKind::FailureDetail { text, full } => {
                let full = full.clone();
                line = line
                    .cursor_default()
                    // line up under the failure's title, past its icon
                    .pl(px(4. + row.depth as f32 * INDENT + 18.))
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(div().flex_1().min_w_0().truncate().child(text.clone()))
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx));
            }
            RowKind::Note { text } => {
                line = line.text_color(muted).child(text.clone());
            }
            RowKind::Section {
                title,
                count,
                system,
            } => {
                line = line
                    .child(Icon::new(IconName::Folder).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if *system { muted } else { fg })
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.clone()),
                    )
                    .child(count_label(*count, muted));
            }
            RowKind::Group { title, count } => {
                line = line
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(title.clone()),
                    )
                    .child(count_label(*count, muted));
            }
            RowKind::Table { table, system, .. } => {
                let entry = table.entry();
                let (icon, tint) = kind_icon(&entry.kind, cx);
                line = line
                    .child(Icon::new(icon).size(px(14.)).text_color(tint))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(*system, |this| this.text_color(muted))
                            .child(highlighted(&entry.name, &filter, cx)),
                    );
                if let Some(look) = &connection {
                    // Built on click: rows render on every scroll frame
                    let (connection_id, db_type, table) =
                        (row.connection, look.db_type.clone(), table.clone());
                    let query_data = Button::new(child_id(&id, "query-data"))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Play)
                        .tooltip("Query data")
                        .on_click(move |_, _, cx| {
                            let entry = table.entry();
                            let sql =
                                schema::select_star_sql(&db_type, entry, schema::PREVIEW_ROWS);
                            let title = Some(entry.name.clone());
                            AppState::global(cx).update(cx, |state, cx| {
                                state.open_query(connection_id, title, sql, true, cx)
                            });
                            cx.stop_propagation();
                        })
                        .into_any_element();
                    line = line.child(hover_actions(vec![query_data, more_button()]));
                }
                line = line.tooltip({
                    let table = table.clone();
                    move |window, cx| Tooltip::new(table.entry().qualified_name()).build(window, cx)
                });
            }
            RowKind::Column {
                name,
                data_type,
                flags,
                ..
            } => {
                let (warning, info) = (cx.theme().warning, cx.theme().info);
                let mono = cx.theme().mono_font_family.clone();
                line = line.child(div().flex_1().min_w_0().truncate().child(name.clone()));
                if flags.primary_key {
                    line = line.child(chip("PK", warning, "Primary key", &id, cx));
                }
                if flags.foreign_key {
                    line = line.child(chip("FK", info, "Foreign key", &id, cx));
                }
                if flags.not_null && !flags.primary_key {
                    line = line.child(chip("NN", muted, "Not null", &id, cx));
                }
                line = line.child(
                    div()
                        .flex_none()
                        .max_w(px(96.))
                        .truncate()
                        .font_family(mono)
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(data_type.clone()),
                );
            }
            RowKind::ColumnsLoading => {
                line = line
                    .text_color(muted)
                    .child(Spinner::new().xsmall().color(muted))
                    .child("Loading columns…");
            }
            RowKind::ColumnsFailed { message, key, .. } => {
                let connection_id = row.connection;
                let key = key.clone();
                let message = message.clone();
                line = line
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(12.))
                            .text_color(danger),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(danger)
                            .child("Couldn’t load columns"),
                    )
                    .child(
                        Button::new(child_id(&id, "retry"))
                            .outline()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .label("Retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.retry_details(connection_id, key.clone(), cx);
                                cx.stop_propagation();
                            })),
                    )
                    .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx));
            }
            RowKind::ColumnsEmpty => {
                line = line.text_color(muted).child("No columns reported");
            }
        }

        // context menu per row kind
        let menu_row = row.clone();
        let line = line.context_menu(move |menu, window, cx| {
            menus::build(menu, &menu_row, weak.clone(), window, cx)
        });
        // the highlight is inset from the panel edges
        div()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px(px(4.))
            .child(line)
            .into_any_element()
    }
}

fn child_id(parent: &SharedString, name: &str) -> SharedString {
    format!("{parent}/{name}").into()
}

struct ConnectionLook {
    accent: Hsla,
    db_type: DatabaseType,
    label: String,
    target: String,
    read_only: bool,
    loading: bool,
}

fn count_label(count: usize, color: Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(px(11.))
        .text_color(color)
        .child(count.to_string())
}

/// A tiny tinted marker (PK, FK, NN, RO).
fn chip(
    text: &'static str,
    color: Hsla,
    tooltip: &'static str,
    owner: &SharedString,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(child_id(owner, text))
        .flex_none()
        .px(px(3.))
        .h(px(14.))
        .flex()
        .items_center()
        .rounded(px(3.))
        .bg(color.opacity(0.14))
        .text_color(theme::tint_text(color, cx))
        .text_size(px(9.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
}

/// Icon and tint of a schema object kind.
fn kind_icon(kind: &dbcrust_gui_core::TableKind, cx: &App) -> (IconName, Hsla) {
    use dbcrust_gui_core::TableKind;
    let theme = cx.theme();
    match kind {
        TableKind::Table => (IconName::Table, theme.muted_foreground),
        TableKind::View => (IconName::Eye, theme.info),
        TableKind::MaterializedView => (IconName::Layers, theme.info),
        TableKind::ForeignTable => (IconName::Globe, theme.muted_foreground),
        TableKind::Collection => (IconName::FileText, theme.muted_foreground),
        TableKind::Index => (IconName::Search, theme.muted_foreground),
        TableKind::Other(_) => (IconName::Square, theme.muted_foreground),
    }
}

/// `name` with the filter's matches emphasised.
fn highlighted(name: &str, filter: &str, cx: &App) -> AnyElement {
    let ranges = schema::highlight_ranges(name, filter);
    if ranges.is_empty() {
        return name.to_string().into_any_element();
    }
    let theme = cx.theme();
    let style = HighlightStyle {
        color: Some(theme.primary),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    StyledText::new(name.to_string())
        .with_highlights(ranges.into_iter().map(|range: Range<usize>| (range, style)))
        .into_any_element()
}

pub(crate) fn copy_text(text: String, what: &str, window: &mut Window, cx: &mut App) {
    crate::clipboard::write(text, cx);
    window.push_notification(Notification::success(format!("Copied {what}")), cx);
}

pub(crate) fn notify_error(window: &mut Window, what: &str, error: &GuiError, cx: &mut App) {
    window.push_notification(Notification::error(format!("{what}: {error}")), cx);
}

impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (looks, active) = {
            let state = AppState::global(cx).read(cx);
            let looks = state
                .connections()
                .iter()
                .map(|live| {
                    let look = ConnectionLook {
                        accent: widgets::connection_accent(live.accent, cx),
                        db_type: live.summary.db_type.clone(),
                        label: live.summary.label.clone(),
                        target: live.summary.display_url.clone(),
                        read_only: live.summary.read_only,
                        loading: matches!(live.schema, crate::app_state::SchemaState::Loading),
                    };
                    (live.id(), Rc::new(look))
                })
                .collect();
            (looks, state.active_id())
        };
        self.looks = looks;
        self.active = active;
        let has_connections = !self.looks.is_empty();
        let theme = cx.theme();
        let (background, foreground) = (theme.sidebar, theme.sidebar_foreground);
        let body = if has_connections {
            div()
                .id("explorer-list")
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list(
                        "explorer-rows",
                        self.rows.len(),
                        cx.processor(|this, range: Range<usize>, window, cx| {
                            range
                                .map(|ix| this.render_row(ix, window, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .vertical_scrollbar(&self.scroll)
                .into_any_element()
        } else {
            self.render_empty(cx).into_any_element()
        };

        v_flex()
            .id("explorer")
            .key_context("Explorer")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(self.render_header(cx))
            .when(has_connections, |this| {
                this.child(
                    div().flex_none().px_2().pb_1p5().child(
                        Input::new(&self.filter)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).size(px(14.))),
                    ),
                )
            })
            .child(body)
    }
}
