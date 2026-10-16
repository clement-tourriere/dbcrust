//! Table inspector tab: columns, indexes, constraints, references, a data
//! preview and the DDL of one table.
//!
//! Contract used by the workspace: `TableInspector::new(connection, table,
//! window, cx)`, `title()`, `connection()`, `table()`.
//!
//! The description comes from `AppState::table_details` (cached per
//! connection until the next schema refresh); "Refresh" fetches it again
//! straight from the connection. The data preview runs one `SELECT * …
//! LIMIT 100` on first use and renders through the shared `ResultsView`.

mod model;
mod sections;

use crate::app_state::{AppEvent, AppState, SchemaState};
use crate::explorer::copy_text;
use crate::query::results::{ResultsContent, ResultsView};
use crate::theme;
use crate::widgets;
use dbcrust_gui_core::schema;
use dbcrust_gui_core::{
    ConnectionHandle, ConnectionId, DatabaseType, GuiResult, RequestId, TableDetails, TableEntry,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::EditorState,
    skeleton::Skeleton,
    spinner::Spinner,
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use model::Described;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

/// A piece of asynchronous state.
enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Section {
    Columns,
    Indexes,
    Constraints,
    ReferencedBy,
    Data,
    Ddl,
}

impl Section {
    /// The tabs an engine can fill with something real.
    fn available(db_type: &DatabaseType) -> Vec<Section> {
        match db_type {
            DatabaseType::MongoDB => vec![Section::Columns, Section::Indexes, Section::Data],
            DatabaseType::Elasticsearch
            | DatabaseType::Redis
            | DatabaseType::InfluxDB
            | DatabaseType::S3
            | DatabaseType::DynamoDB
            | DatabaseType::CloudWatchLogs => {
                vec![Section::Columns, Section::Data]
            }
            DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB
            | DatabaseType::WhiteDragon => vec![Section::Columns, Section::Data, Section::Ddl],
            _ => vec![
                Section::Columns,
                Section::Indexes,
                Section::Constraints,
                Section::ReferencedBy,
                Section::Data,
                Section::Ddl,
            ],
        }
    }

    fn label(self, db_type: &DatabaseType) -> &'static str {
        match self {
            Section::Columns => match db_type {
                DatabaseType::MongoDB | DatabaseType::Elasticsearch => "Fields",
                _ => "Columns",
            },
            Section::Indexes => "Indexes",
            Section::Constraints => "Constraints",
            Section::ReferencedBy => "Referenced by",
            Section::Data => "Data",
            Section::Ddl => "DDL",
        }
    }
}

/// The explicit row count: nothing runs until the user asks, and a running
/// count can be cancelled.
enum CountState {
    Idle,
    Running {
        handle: ConnectionHandle,
        request: RequestId,
    },
    Done {
        rows: u64,
        took: Duration,
    },
    Failed(String),
}

/// State of the data preview.
#[derive(Clone, Debug, PartialEq)]
enum DataState {
    Idle,
    Loading,
    Loaded { rows: usize },
    Failed,
}

pub struct TableInspector {
    focus_handle: FocusHandle,
    connection: ConnectionId,
    table: TableEntry,
    db_type: DatabaseType,
    /// The name the table is described by (also the cache key).
    key: String,
    section: Section,
    described: Load<Arc<Described>>,
    describe_task: Option<Task<()>>,
    ddl_editor: Option<Entity<EditorState>>,
    data_view: Entity<ResultsView>,
    data: DataState,
    data_task: Option<Task<()>>,
    /// The preview has shown rows or an error at least once.
    data_has_content: bool,
    data_request: Option<(ConnectionHandle, RequestId)>,
    count: CountState,
    count_task: Option<Task<()>>,
    /// The connection's schema was reloaded since we last described.
    schema_refreshing: bool,
    columns_scroll: UniformListScrollHandle,
    indexes_scroll: UniformListScrollHandle,
    references_scroll: UniformListScrollHandle,
    constraints_list: ListState,
    /// The connection's tables by name, for the foreign key links (see
    /// `sections.rs`); rebuilt when the connection's table list is replaced.
    table_lookup: RefCell<Option<schema::TableLookup>>,
    _subscriptions: Vec<Subscription>,
}

impl TableInspector {
    pub fn new(
        connection: ConnectionId,
        table: TableEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = AppState::global(cx);
        let (db_type, schema_loading) = {
            let state = state.read(cx);
            let live = state.connection(connection);
            (
                live.map(|live| live.summary.db_type.clone())
                    .unwrap_or(DatabaseType::PostgreSQL),
                live.is_some_and(|live| matches!(live.schema, SchemaState::Loading)),
            )
        };
        let key = schema::describe_name(&db_type, &table);
        let data_view = cx.new(|cx| ResultsView::new(window, cx));
        let subscription = cx.subscribe_in(
            &state,
            window,
            |this, _, event: &AppEvent, window, cx| match event {
                AppEvent::SchemaChanged(id) if *id == this.connection => {
                    this.on_schema_changed(window, cx)
                }
                AppEvent::Disconnected(id) if *id == this.connection => cx.notify(),
                _ => {}
            },
        );
        let mut inspector = Self {
            focus_handle: cx.focus_handle(),
            connection,
            table,
            db_type,
            key,
            section: Section::Columns,
            described: Load::Loading,
            describe_task: None,
            ddl_editor: None,
            data_view,
            data: DataState::Idle,
            data_task: None,
            data_has_content: false,
            data_request: None,
            count: CountState::Idle,
            count_task: None,
            schema_refreshing: schema_loading,
            columns_scroll: UniformListScrollHandle::default(),
            indexes_scroll: UniformListScrollHandle::default(),
            references_scroll: UniformListScrollHandle::default(),
            constraints_list: ListState::new(0, ListAlignment::Top, px(240.)),
            table_lookup: RefCell::new(None),
            _subscriptions: vec![subscription],
        };
        inspector.load_details(false, window, cx);
        inspector
    }

    pub fn title(&self) -> SharedString {
        self.table.name.clone().into()
    }

    pub fn connection(&self) -> ConnectionId {
        self.connection
    }

    pub fn table(&self) -> &TableEntry {
        &self.table
    }

    /// Fetch the description and the data preview again.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_details(true, window, cx);
        if self.data != DataState::Idle {
            self.load_data(window, cx);
        }
    }

    // ---- loading --------------------------------------------------------

    fn handle(&self, cx: &App) -> Option<ConnectionHandle> {
        AppState::global(cx).read(cx).handle(self.connection)
    }

    fn load_details(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.described = Load::Loading;
        let described: Task<GuiResult<Arc<TableDetails>>> = if force {
            let Some(handle) = self.handle(cx) else {
                self.described = Load::Failed("This connection is closed".into());
                cx.notify();
                return;
            };
            let future = handle.describe_table(self.key.clone());
            cx.background_spawn(async move { future.await.map(Arc::new) })
        } else {
            let connection = self.connection;
            let key = self.key.clone();
            AppState::global(cx).update(cx, |state, cx| state.table_details(connection, key, cx))
        };
        self.describe_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = described.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_details(result, window, cx)
            });
        }));
        cx.notify();
    }

    fn finish_details(
        &mut self,
        result: GuiResult<Arc<TableDetails>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ddl_editor = None;
        self.described = match result {
            Ok(details) => Load::Ready(Arc::new(Described::new(
                &self.db_type,
                &self.table,
                details,
            ))),
            Err(error) => Load::Failed(error.to_string()),
        };
        if self.section == Section::Ddl {
            self.ensure_ddl_editor(window, cx);
        }
        cx.notify();
    }

    fn on_schema_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let schema_loading = AppState::global(cx)
            .read(cx)
            .connection(self.connection)
            .map(|live| matches!(live.schema, SchemaState::Loading));
        match schema_loading {
            Some(true) => self.schema_refreshing = true,
            Some(false) if self.schema_refreshing => {
                // the schema was reloaded: the table may have changed
                self.schema_refreshing = false;
                self.load_details(false, window, cx);
            }
            _ => {}
        }
    }

    fn load_data(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(handle) = self.handle(cx) else {
            return;
        };
        if let Some((previous, request)) = self.data_request.take() {
            previous.cancel(request);
        }
        let request = RequestId::next();
        let sql = schema::select_star_sql(&self.db_type, &self.table, schema::PREVIEW_ROWS);
        let future = handle.execute(request, sql.clone());
        self.data = DataState::Loading;
        self.data_request = Some((handle, request));
        let connection = self.connection;
        self.data_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.data_request = None;
                let content = match result {
                    Ok(set) => {
                        this.data = DataState::Loaded {
                            rows: set.row_count(),
                        };
                        let total = set.elapsed;
                        ResultsContent::Results {
                            sets: vec![Arc::new(set)],
                            total,
                        }
                    }
                    Err(error) if error.is_cancelled() => return,
                    Err(error) => {
                        this.data = DataState::Failed;
                        ResultsContent::Error {
                            error,
                            statement: sql,
                        }
                    }
                };
                this.data_has_content = true;
                this.data_view.update(cx, |view, cx| {
                    view.show(content, Some(connection), window, cx)
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// `SELECT COUNT(*)`, only when asked: it can scan a whole table.
    fn count_rows(&mut self, cx: &mut Context<Self>) {
        if matches!(self.count, CountState::Running { .. }) {
            return;
        }
        let Some(handle) = self.handle(cx) else {
            self.count = CountState::Failed("This connection is closed".into());
            cx.notify();
            return;
        };
        let request = RequestId::next();
        let future = handle.execute(request, schema::count_sql(&self.db_type, &self.table));
        self.count = CountState::Running { handle, request };
        self.count_task = Some(cx.spawn(async move |this, cx| {
            let result = future.await;
            let _ = this.update(cx, |this, cx| {
                // a newer count (or a cancel) replaced this one
                if !matches!(&this.count, CountState::Running { request: current, .. } if *current == request)
                {
                    return;
                }
                this.count = match result {
                    Ok(set) => match schema::parse_count(&set.rows) {
                        Some(rows) => CountState::Done {
                            rows,
                            took: set.elapsed,
                        },
                        None => CountState::Failed("The engine returned no number".into()),
                    },
                    Err(error) if error.is_cancelled() => CountState::Idle,
                    Err(error) => CountState::Failed(error.to_string()),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn cancel_count(&mut self, cx: &mut Context<Self>) {
        if let CountState::Running { handle, request } = &self.count {
            handle.cancel(*request);
            self.count = CountState::Idle;
            self.count_task = None;
            cx.notify();
        }
    }

    fn ensure_ddl_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ddl_editor.is_some() {
            return;
        }
        let Load::Ready(described) = &self.described else {
            return;
        };
        let Some(ddl) = described.ddl.clone() else {
            return;
        };
        self.ddl_editor = Some(cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .soft_wrap(false)
                .default_value(ddl)
        }));
    }

    fn select_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if self.section == section {
            return;
        }
        self.section = section;
        match section {
            Section::Data if self.data == DataState::Idle => self.load_data(window, cx),
            Section::Ddl => self.ensure_ddl_editor(window, cx),
            _ => {}
        }
        cx.notify();
    }

    // ---- actions --------------------------------------------------------

    fn open_query(&self, title: String, sql: String, cx: &mut App) {
        let connection = self.connection;
        AppState::global(cx).update(cx, |state, cx| {
            state.open_query(connection, Some(title), sql, true, cx)
        });
    }

    fn copy_ddl(&self, window: &mut Window, cx: &mut App) {
        if let Load::Ready(described) = &self.described
            && let Some(ddl) = &described.ddl
        {
            copy_text(ddl.clone(), "DDL", window, cx);
        }
    }

    // ---- rendering ------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = AppState::global(cx);
        let live = {
            let state = state.read(cx);
            state.connection(self.connection).map(|live| {
                (
                    widgets::connection_accent(live.accent, cx),
                    live.summary.label.clone(),
                    live.summary.type_name(),
                )
            })
        };
        let theme = cx.theme();
        let (muted, border, mono) = (
            theme.muted_foreground,
            theme.border,
            theme.mono_font_family.clone(),
        );
        let closed = live.is_none();
        let ready = matches!(self.described, Load::Ready(_));

        let name = {
            let schema = self
                .table
                .schema
                .as_deref()
                .filter(|schema| !schema::is_default_schema(&self.db_type, schema));
            h_flex()
                .gap_0()
                .font_family(mono.clone())
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .when_some(schema, |this, schema| {
                    this.child(div().text_color(muted).child(format!("{schema}.")))
                })
                .child(self.table.name.clone())
        };

        let counts = match &self.described {
            Load::Ready(described) => described.summary(&self.db_type),
            Load::Loading => "Loading…".to_string(),
            Load::Failed(_) => "Could not load the description".to_string(),
        };

        let select_sql = schema::select_star_sql(&self.db_type, &self.table, schema::PREVIEW_ROWS);
        let table_name = self.table.name.clone();
        let can_ddl = schema::supports_ddl(&self.db_type);

        h_flex()
            .flex_none()
            .px_4()
            .py_3()
            .gap_3()
            .border_b_1()
            .border_color(border)
            .child(match &live {
                Some((accent, _, _)) => div()
                    .flex_none()
                    .size(px(8.))
                    .rounded_full()
                    .bg(*accent)
                    .into_any_element(),
                None => div().into_any_element(),
            })
            .child(widgets::db_badge(&self.db_type, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(name)
                            .child(kind_tag(&self.table.kind, cx))
                            .when_some(self.table.owner.clone(), |this, owner| {
                                this.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(muted)
                                        .child(format!("owner {owner}")),
                                )
                            })
                            .child(self.render_count_result(cx)),
                    )
                    .child(div().text_size(px(12.)).text_color(muted).truncate().child(
                        match &live {
                            Some((_, label, engine)) => format!("{counts} · {label} ({engine})"),
                            None => format!("{counts} · connection closed"),
                        },
                    )),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Button::new("inspector-query")
                            .primary()
                            .small()
                            .icon(IconName::Play)
                            .label("Query data")
                            .disabled(closed)
                            .on_click({
                                let (title, sql) = (table_name.clone(), select_sql);
                                cx.listener(move |this, _, _, cx| {
                                    this.open_query(title.clone(), sql.clone(), cx)
                                })
                            }),
                    )
                    .child(self.render_count_control(closed, cx))
                    .when(can_ddl, |this| {
                        this.child(
                            Button::new("inspector-copy-ddl")
                                .ghost()
                                .small()
                                .icon(IconName::Copy)
                                .label("Copy DDL")
                                .disabled(!ready)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.copy_ddl(window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("inspector-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .tooltip("Describe the table again")
                            .disabled(closed)
                            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
                    ),
            )
    }

    /// "Count rows", or while it runs a spinner and a way to stop it.
    fn render_count_control(&self, closed: bool, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        match &self.count {
            CountState::Running { .. } => h_flex()
                .gap_1p5()
                .items_center()
                .child(Spinner::new().small().color(muted))
                .child(
                    Button::new("inspector-count-cancel")
                        .outline()
                        .small()
                        .icon(IconName::X)
                        .label("Cancel count")
                        .tooltip("Stop counting; the engine is told to cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_count(cx))),
                )
                .into_any_element(),
            state => Button::new("inspector-count")
                .outline()
                .small()
                .label(if matches!(state, CountState::Done { .. }) {
                    "Count again"
                } else {
                    "Count rows"
                })
                .tooltip("Run SELECT COUNT(*); large tables can take a while")
                .disabled(closed)
                .on_click(cx.listener(|this, _, _, cx| this.count_rows(cx)))
                .into_any_element(),
        }
    }

    /// The exact row count (or why there is none), next to the table name.
    fn render_count_result(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, mono) = (
            theme.muted_foreground,
            theme.danger,
            theme.mono_font_family.clone(),
        );
        match &self.count {
            CountState::Done { rows, took } => {
                let tooltip = format!("Exact count, counted in {}", format_took(*took));
                h_flex()
                    .id("inspector-count-result")
                    .gap_1()
                    .text_size(px(12.))
                    .child(
                        div()
                            .font_family(mono)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(schema::group_digits(*rows)),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child(if *rows == 1 { "row" } else { "rows" }),
                    )
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .into_any_element()
            }
            CountState::Failed(message) => {
                let message = message.clone();
                div()
                    .id("inspector-count-failed")
                    .text_size(px(12.))
                    .text_color(danger)
                    .child("Count failed")
                    .tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx))
                    .into_any_element()
            }
            CountState::Idle | CountState::Running { .. } => div().into_any_element(),
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sections = Section::available(&self.db_type);
        let selected = sections
            .iter()
            .position(|section| *section == self.section)
            .unwrap_or(0);
        let muted = cx.theme().muted_foreground;
        let tabs: Vec<Tab> = sections
            .iter()
            .map(|section| {
                let tab = Tab::new().label(section.label(&self.db_type));
                match (&self.described, self.count_of(*section)) {
                    (Load::Ready(_), Some(count)) => tab.suffix(
                        div()
                            .ml_1()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(count.to_string()),
                    ),
                    _ => tab,
                }
            })
            .collect();
        TabBar::new("inspector-tabs")
            .underline()
            .small()
            .selected_index(selected)
            .on_click(cx.listener(move |this, index: &usize, window, cx| {
                if let Some(section) = Section::available(&this.db_type).get(*index).copied() {
                    this.select_section(section, window, cx);
                }
            }))
            .children(tabs)
    }

    fn count_of(&self, section: Section) -> Option<usize> {
        let Load::Ready(described) = &self.described else {
            return None;
        };
        let details = &described.details;
        match section {
            Section::Columns => Some(details.columns.len()),
            Section::Indexes => Some(details.indexes.len()),
            Section::Constraints => {
                Some(details.foreign_keys.len() + details.check_constraints.len())
            }
            Section::ReferencedBy => Some(details.referenced_by.len()),
            Section::Data | Section::Ddl => None,
        }
    }

    fn render_loading(&self, cx: &mut Context<Self>) -> AnyElement {
        let widths = [0.9, 0.7, 0.8, 0.55, 0.75, 0.6, 0.85, 0.5];
        v_flex()
            .p_4()
            .gap_3()
            .children(widths.iter().enumerate().map(|(index, width)| {
                let _ = index;
                Skeleton::new()
                    .w(relative(*width))
                    .h(px(14.))
                    .rounded(cx.theme().radius)
            }))
            .into_any_element()
    }

    fn render_failed(&self, message: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(
                Icon::new(IconName::TriangleAlert)
                    .size(px(24.))
                    .text_color(danger),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Could not describe {}", self.table.name)),
            )
            .child(
                div()
                    .max_w(px(560.))
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(message.to_string()),
            )
            .child(
                Button::new("inspector-retry")
                    .outline()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label("Retry")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.load_details(true, window, cx)),
                    ),
            )
            .into_any_element()
    }
}

impl Drop for TableInspector {
    fn drop(&mut self) {
        if let Some((handle, request)) = self.data_request.take() {
            handle.cancel(request);
        }
        if let CountState::Running { handle, request } = &self.count {
            handle.cancel(*request);
        }
    }
}

/// `3 ms`, `1.2 s`: how long a count took.
fn format_took(took: Duration) -> String {
    let ms = took.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// `table`, `view`, … as a small tag.
fn kind_tag(kind: &dbcrust_gui_core::TableKind, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let color = match kind {
        dbcrust_gui_core::TableKind::View | dbcrust_gui_core::TableKind::MaterializedView => {
            theme.info
        }
        _ => theme.muted_foreground,
    };
    div()
        .flex_none()
        .px(px(6.))
        .h(px(18.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .bg(color.opacity(0.14))
        .text_color(theme::tint_text(color, cx))
        .text_size(px(11.))
        .child(kind.label().to_string())
}

impl Focusable for TableInspector {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TableInspector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, border) = (theme.background, theme.foreground, theme.border);

        let body = match &self.described {
            Load::Loading => self.render_loading(cx),
            Load::Failed(message) => {
                let message = message.clone();
                self.render_failed(&message, cx)
            }
            Load::Ready(described) => {
                let described = described.clone();
                self.render_section(&described, cx)
            }
        };

        v_flex()
            .id("table-inspector")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(self.render_header(cx))
            .child(
                div()
                    .flex_none()
                    .border_b_1()
                    .border_color(border)
                    .px_2()
                    .child(self.render_tabs(cx)),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn counting_time_is_short_and_honest() {
        assert_eq!(format_took(Duration::from_micros(400)), "0 ms");
        assert_eq!(format_took(Duration::from_millis(3)), "3 ms");
        assert_eq!(format_took(Duration::from_millis(999)), "999 ms");
        assert_eq!(format_took(Duration::from_millis(1500)), "1.5 s");
    }
}
