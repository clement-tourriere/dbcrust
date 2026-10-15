//! Results panel of a query tab: data grid, multiple result sets, messages,
//! errors, copy/export and the row inspector.
//!
//! Contract used by `QueryTab` (keep these signatures):
//! - `ResultsView::new(window, cx)`
//! - `ResultsView::show(content, connection, window, cx)`
//!
//! Layout: `grid` is the virtualized table of one result set (with its
//! context menu, copy, export and inspector), `model` is the pure logic
//! behind it, `inspector` is the row sheet. This file holds the panel
//! itself: result tabs, footer, and the empty / running / error / plan
//! states.

mod grid;
mod inspector;
mod model;

use crate::actions::{CancelQuery, CopySelection, ExportResults, RunAll, RunStatement};
use crate::query::explain::ExplainView;
use dbcrust_gui_core::export::ExportFormat;
use dbcrust_gui_core::{ConnectionId, ExplainRaw, GuiError, ResultSet};
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use grid::ResultGrid;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the results panel displays.
#[derive(Clone, Debug)]
pub enum ResultsContent {
    /// Nothing run yet.
    Empty,
    /// A statement is running (with a cancel affordance).
    Running { since: Instant, statement: String },
    /// One result set per executed statement (several for "Run all").
    /// `total` is the wall-clock time of the whole run: shown next to each
    /// statement's own duration when there are several statements, ignored
    /// for one (its own duration is the only number).
    Results {
        sets: Vec<Arc<ResultSet>>,
        total: Duration,
    },
    /// An EXPLAIN plan.
    Plan(Arc<ExplainRaw>),
    /// The statement failed (or was cancelled); nothing else to show.
    Error { error: GuiError, statement: String },
    /// A run of a query tab failed (or was cancelled). Results stay on
    /// screen under the error: the `completed` statements of the same run
    /// ("Run all" stops at the first error), or — when there are none — the
    /// results the previous run left. Falls back to a plain error when
    /// there is nothing to keep.
    Failed {
        error: GuiError,
        statement: String,
        completed: Vec<Arc<ResultSet>>,
    },
}

/// Why the results on screen no longer match the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// A statement that produced them was edited.
    Edited,
    /// The tab is bound to another connection (or none) since they ran.
    ConnectionChanged,
}

impl Stale {
    fn reason(self) -> &'static str {
        match self {
            Stale::Edited => "The SQL was edited after this ran.",
            Stale::ConnectionChanged => "The connection changed after this ran.",
        }
    }
}

/// A statement that finishes within this time never replaces the previous
/// results with the running screen, so quick queries do not flash.
const RUNNING_GRACE: Duration = Duration::from_millis(150);

struct ResultTab {
    result: Arc<ResultSet>,
    /// Created the first time the tab is shown.
    grid: Option<Entity<ResultGrid>>,
    /// Repaints the footer when the grid's local sort changes.
    _observation: Option<Subscription>,
}

/// A failed run, shown as a banner above the results it left on screen.
struct Failure {
    error: GuiError,
    statement: String,
    kept: Kept,
}

/// Which results sit under the banner.
#[derive(Clone, Copy)]
enum Kept {
    /// The run produced none: these are the previous run's.
    Previous,
    /// Statements of this run that finished before the failing one.
    Completed(usize),
}

impl Kept {
    fn note(self) -> String {
        match self {
            Kept::Previous => "Showing the results of the previous run".to_string(),
            Kept::Completed(1) => "Showing the result of the statement that completed".to_string(),
            Kept::Completed(n) => {
                format!("Showing the results of the {n} statements that completed")
            }
        }
    }
}

enum View {
    Empty,
    /// `previous` stays on screen (dimmed) during [`RUNNING_GRACE`].
    Running {
        since: Instant,
        statement: String,
        previous: Option<Box<View>>,
    },
    Results {
        tabs: Vec<ResultTab>,
        active: usize,
        /// The run that produced (or replaced) these results failed.
        failure: Option<Failure>,
        /// Wall-clock time of the whole run (several statements only).
        total: Option<Duration>,
    },
    Plan(Entity<ExplainView>),
    Error {
        error: GuiError,
        statement: String,
    },
}

impl View {
    /// Every grid this view holds, including those of the results kept on
    /// screen while a statement starts.
    fn grids(&self) -> Vec<Entity<ResultGrid>> {
        match self {
            View::Results { tabs, .. } => tabs.iter().filter_map(|tab| tab.grid.clone()).collect(),
            View::Running {
                previous: Some(previous),
                ..
            } => previous.grids(),
            _ => Vec::new(),
        }
    }

    /// The result tabs this view holds, looking through a run that is
    /// covering them.
    fn into_results(self) -> Option<(Vec<ResultTab>, usize, Option<Duration>)> {
        match self {
            View::Results {
                tabs,
                active,
                total,
                ..
            } => Some((tabs, active, total)),
            View::Running {
                previous: Some(previous),
                ..
            } => previous.into_results(),
            _ => None,
        }
    }
}

fn result_tabs(sets: Vec<Arc<ResultSet>>) -> Vec<ResultTab> {
    sets.into_iter()
        .map(|result| ResultTab {
            result,
            grid: None,
            _observation: None,
        })
        .collect()
}

pub struct ResultsView {
    focus_handle: FocusHandle,
    connection: Option<ConnectionId>,
    view: View,
    /// The results on screen are not what the editor would run now.
    stale: Option<Stale>,
    /// Redraws the running timer; dropped (cancelled) when the run ends.
    ticker: Option<Task<()>>,
    /// How many times it rendered (tests watch what a repaint reaches).
    #[cfg(test)]
    renders: usize,
}

impl ResultsView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            connection: None,
            view: View::Empty,
            stale: None,
            ticker: None,
            #[cfg(test)]
            renders: 0,
        }
    }

    /// Mark the results on screen as no longer matching the editor (or clear
    /// the mark). [`Self::show`] always starts fresh.
    pub fn set_stale(&mut self, stale: Option<Stale>, cx: &mut Context<Self>) {
        if self.stale != stale {
            self.stale = stale;
            cx.notify();
        }
    }

    /// Results or a plan are on screen (as opposed to nothing, a running
    /// statement or a bare error).
    pub fn is_showing_results(&self) -> bool {
        matches!(self.view, View::Results { .. } | View::Plan(_))
    }

    /// The connection of the tab this panel belongs to.
    pub fn connection(&self) -> Option<ConnectionId> {
        self.connection
    }

    /// Replace what the panel shows. `connection` is the tab's connection
    /// (used for "open table"/re-run affordances).
    pub fn show(
        &mut self,
        content: ResultsContent,
        connection: Option<ConnectionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connection = connection;
        self.ticker = None;
        self.stale = None;
        for grid in self.view.grids() {
            grid.update(cx, |grid, cx| grid.close_inspector(window, cx));
        }
        let previous = std::mem::replace(&mut self.view, View::Empty);
        self.view = match content {
            ResultsContent::Empty => View::Empty,
            ResultsContent::Running { since, statement } => {
                self.start_ticker(since, window, cx);
                // Keep what was on screen (looking through a run that was
                // already in progress), unless there was nothing to keep.
                let previous = match previous {
                    View::Running { previous, .. } => previous,
                    View::Empty => None,
                    other => Some(Box::new(other)),
                };
                View::Running {
                    since,
                    statement,
                    previous,
                }
            }
            ResultsContent::Results { sets, .. } if sets.is_empty() => View::Empty,
            ResultsContent::Results { sets, total } => {
                let active = model::default_tab(&sets);
                let total = (sets.len() > 1).then_some(total);
                View::Results {
                    tabs: result_tabs(sets),
                    active,
                    failure: None,
                    total,
                }
            }
            ResultsContent::Plan(raw) => View::Plan(cx.new(|cx| ExplainView::new(raw, window, cx))),
            ResultsContent::Error { error, statement } => View::Error { error, statement },
            ResultsContent::Failed {
                error,
                statement,
                completed,
            } => {
                if !completed.is_empty() {
                    let kept = Kept::Completed(completed.len());
                    let active = model::default_tab(&completed);
                    View::Results {
                        tabs: result_tabs(completed),
                        active,
                        failure: Some(Failure {
                            error,
                            statement,
                            kept,
                        }),
                        total: None,
                    }
                } else if let Some((tabs, active, total)) = previous.into_results() {
                    View::Results {
                        tabs,
                        active,
                        failure: Some(Failure {
                            error,
                            statement,
                            kept: Kept::Previous,
                        }),
                        total,
                    }
                } else {
                    View::Error { error, statement }
                }
            }
        };
        self.ensure_grid(window, cx);
        cx.notify();
    }

    /// Repaint while the statement runs, when the timer would show
    /// something else (and to leave the grace period).
    fn start_ticker(&mut self, since: Instant, window: &mut Window, cx: &mut Context<Self>) {
        self.ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let wait = model::running_tick(since.elapsed());
                cx.background_executor().timer(wait).await;
                let running = this.update(cx, |this, cx| {
                    let running = matches!(this.view, View::Running { .. });
                    if running {
                        cx.notify();
                    }
                    running
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let View::Results { tabs, active, .. } = &mut self.view
            && ix < tabs.len()
            && *active != ix
        {
            *active = ix;
            self.ensure_grid(window, cx);
            cx.notify();
        }
    }

    /// Create the grid of the active tab if it has columns and none yet.
    fn ensure_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Results { tabs, active, .. } = &mut self.view else {
            return;
        };
        let Some(tab) = tabs.get_mut(*active) else {
            return;
        };
        if tab.grid.is_none() && !tab.result.columns.is_empty() {
            let result = tab.result.clone();
            let grid = cx.new(|cx| ResultGrid::new(result, window, cx));
            tab._observation = Some(cx.observe(&grid, |_, _, cx| cx.notify()));
            tab.grid = Some(grid);
        }
    }

    /// The grid on screen, if any (the previous results stay visible, but
    /// inert, while a fast statement runs).
    fn active_grid(&self) -> Option<&Entity<ResultGrid>> {
        match &self.view {
            View::Results { tabs, active, .. } => tabs.get(*active)?.grid.as_ref(),
            _ => None,
        }
    }

    /// Hide the error banner; the results under it stay.
    fn dismiss_failure(&mut self, cx: &mut Context<Self>) {
        if let View::Results { failure, .. } = &mut self.view {
            *failure = None;
            cx.notify();
        }
    }

    fn copy_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(grid) = self.active_grid().cloned() else {
            cx.propagate();
            return;
        };
        let copied = grid.update(cx, |grid, cx| grid.copy_selection(window, cx));
        if !copied {
            window.push_notification(
                Notification::info("Select a cell, row or column to copy.").id::<CopySelection>(),
                cx,
            );
        }
    }

    fn export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_grid().cloned() {
            Some(grid) => grid.update(cx, |grid, cx| grid.export(format, window, cx)),
            None => cx.propagate(),
        }
    }
}

#[cfg(test)]
impl ResultsView {
    /// What is on screen, for tests: (result tabs, error banner shown,
    /// full-page error shown).
    pub(crate) fn shape(&self) -> (usize, bool, bool) {
        match &self.view {
            View::Results { tabs, failure, .. } => (tabs.len(), failure.is_some(), false),
            View::Error { .. } => (0, false, true),
            _ => (0, false, false),
        }
    }

    /// The message of the error on screen, banner or full page.
    pub(crate) fn error_text(&self) -> Option<String> {
        match &self.view {
            View::Results { failure, .. } => failure.as_ref().map(|f| f.error.to_string()),
            View::Error { error, .. } => Some(error.to_string()),
            _ => None,
        }
    }

    /// The execution time each result tab reports, in order.
    pub(crate) fn durations(&self) -> Vec<Duration> {
        match &self.view {
            View::Results { tabs, .. } => tabs.iter().map(|tab| tab.result.elapsed).collect(),
            _ => Vec::new(),
        }
    }

    /// What the panel says about the age of its results.
    pub(crate) fn stale(&self) -> Option<Stale> {
        self.stale
    }

    /// The local sort of the grid on screen, as `"name asc"`.
    pub(crate) fn local_sort(&self, cx: &App) -> Option<String> {
        let grid = self.active_grid()?;
        let (column, direction) = grid.read(cx).sort(cx)?;
        Some(format!(
            "{column} {}",
            match direction {
                model::SortDirection::Ascending => "asc",
                model::SortDirection::Descending => "desc",
            }
        ))
    }

    /// How many times the panel and the grid on screen rendered.
    pub(crate) fn renders(&self, cx: &App) -> (usize, Option<usize>) {
        (
            self.renders,
            self.active_grid().map(|grid| grid.read(cx).renders),
        )
    }

    /// Act on the grid on screen as the user would.
    pub(crate) fn with_grid(
        results: &Entity<Self>,
        window: &mut Window,
        cx: &mut App,
        act: impl FnOnce(&mut ResultGrid, &mut Window, &mut Context<ResultGrid>),
    ) {
        let grid = results
            .read(cx)
            .active_grid()
            .expect("a grid on screen")
            .clone();
        grid.update(cx, |grid, cx| act(grid, window, cx));
    }

    /// "Copy all" of the grid on screen (rendered in the background), then
    /// an immediate copy of one of its cells, as in quick succession.
    pub(crate) fn copy_all_then_cell(
        results: &Entity<Self>,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        let grid = results
            .read(cx)
            .active_grid()
            .expect("a grid on screen")
            .clone();
        grid.update(cx, |grid, cx| {
            grid.copy_all(ExportFormat::Csv, window, cx);
            grid.copy_cell(row, col, window, cx);
        });
    }
}

impl Focusable for ResultsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ResultsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let body = self.body(&self.view, window, cx);
        div()
            .id("results-panel")
            .key_context("Results")
            .track_focus(&self.focus_handle)
            .on_action(
                cx.listener(|this, _: &CopySelection, window, cx| this.copy_selection(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ExportResults, window, cx| {
                this.export(ExportFormat::Csv, window, cx)
            }))
            .size_full()
            .bg(cx.theme().background)
            .child(body)
    }
}

impl ResultsView {
    fn body(&self, view: &View, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match view {
            View::Empty => empty_state(window, cx),
            View::Running {
                since,
                statement,
                previous,
            } => match previous {
                Some(previous) if since.elapsed() < RUNNING_GRACE => div()
                    .size_full()
                    .relative()
                    .opacity(0.55)
                    .child(self.body(previous, window, cx))
                    // inert while it waits
                    .child(div().absolute().inset_0().occlude())
                    .into_any_element(),
                _ => running_state(*since, statement, cx),
            },
            View::Results {
                tabs,
                active,
                failure,
                total,
            } => self.results_panel(tabs, *active, failure.as_ref(), *total, cx),
            View::Plan(plan) => v_flex()
                .size_full()
                .when_some(self.stale, |this, stale| {
                    this.child(stale_banner(stale, "plan", cx))
                })
                .child(div().flex_1().min_h_0().child(plan.clone()))
                .into_any_element(),
            View::Error { error, statement } => error_state(error, statement, cx),
        }
    }

    /// Result tabs (when there are several), the grid or message of the
    /// active one, and the footer.
    fn results_panel(
        &self,
        tabs: &[ResultTab],
        active: usize,
        failure: Option<&Failure>,
        total: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(tab) = tabs.get(active) else {
            return div().into_any_element();
        };
        let result = &tab.result;
        let content = match &tab.grid {
            // Cached: repainting the panel (the stale banner, a spinner) does
            // not lay out the visible rows again; the grid repaints itself
            Some(grid) => grid
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            None => message_state(result, cx),
        };
        // After a failed run the banner already says the results are old
        let stale = self.stale.filter(|_| failure.is_none());
        let batch = (tabs.len() > 1).then_some(Batch {
            position: active + 1,
            statements: tabs.len(),
            total,
        });
        v_flex()
            .size_full()
            .when_some(failure, |this, failure| {
                this.child(self.failure_banner(failure, cx))
            })
            .when_some(stale, |this, stale| {
                this.child(stale_banner(stale, "results", cx))
            })
            .when(tabs.len() > 1, |this| {
                this.child(self.tab_strip(tabs, active, cx))
            })
            .child(div().flex_1().min_h_0().child(content))
            .when_some(tab.grid.as_ref(), |this, grid| {
                this.child(footer(result, batch, grid, cx))
            })
            .into_any_element()
    }

    /// The error of a failed run, above the results that stayed on screen:
    /// what failed, what is being shown instead, and the way to copy or
    /// dismiss it.
    fn failure_banner(&self, failure: &Failure, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tone = ErrorTone::of(&failure.error);
        let (color, icon) = tone.style(cx);
        let (title, _) = error_title(&failure.error);
        let message = failure.error.to_string();
        let (preview, _) = model::statement_preview(&failure.statement, 1, 160);
        let show_message = tone != ErrorTone::Notice && !message.is_empty();

        v_flex()
            .id("results-failure")
            .flex_none()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(color.opacity(0.07))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(icon).size_4().text_color(color))
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(failure.kept.note()),
                    )
                    .when(show_message, |this| {
                        let message = message.clone();
                        this.child(
                            Button::new("copy-error")
                                .label("Copy error")
                                .icon(IconName::Copy)
                                .ghost()
                                .xsmall()
                                .on_click(move |_, window, cx| {
                                    crate::clipboard::write(message.clone(), cx);
                                    window.push_notification(
                                        Notification::success("Copied error").id::<ErrorNotice>(),
                                        cx,
                                    );
                                }),
                        )
                    })
                    .child(
                        Button::new("dismiss-failure")
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_failure(cx))),
                    ),
            )
            .when(show_message, |this| {
                this.child(
                    div()
                        .id("results-failure-message")
                        .max_h(px(112.))
                        .overflow_y_scroll()
                        .child(message_block(message.clone(), color, cx)),
                )
            })
            .when(!preview.is_empty(), |this| {
                this.child(
                    div()
                        .truncate()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(preview),
                )
            })
    }

    fn tab_strip(
        &self,
        tabs: &[ResultTab],
        active: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id("result-tabs")
            .flex_none()
            .h(px(30.))
            .px_2()
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(tabs.iter().enumerate().map(|(ix, tab)| {
                let (preview, _) = model::statement_preview(&tab.result.sql, 1, 100);
                Button::new(("result-tab", ix))
                    .xsmall()
                    .ghost()
                    .label(model::tab_label(ix, &tab.result))
                    .selected(ix == active)
                    .when(!preview.is_empty(), |button| button.tooltip(preview))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select_tab(ix, window, cx)),
                    )
            }))
    }
}

// ---------------------------------------------------------------------------
// Provenance and footer
// ---------------------------------------------------------------------------

/// "Results from previous run": the editor or the connection moved on.
fn stale_banner(stale: Stale, what: &'static str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .id("results-stale")
        .flex_none()
        .h(px(28.))
        .px_3()
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.warning.opacity(0.08))
        .text_xs()
        .child(
            Icon::new(gpui_kit::assets::IconName::Clock)
                .size_3p5()
                .text_color(theme.warning),
        )
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Results from previous run"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "{} Run again to refresh the {what}.",
                    stale.reason()
                )),
        )
}

/// Where a result sits in a multi-statement run.
#[derive(Clone, Copy)]
struct Batch {
    /// 1-based.
    position: usize,
    statements: usize,
    /// Wall-clock time of the whole run.
    total: Option<Duration>,
}

/// What an export or a "copy all" covers, in words.
fn scope_phrase(result: &ResultSet) -> String {
    let rows = result.row_count();
    if result.is_truncated() {
        format!(
            "the {} loaded rows (LIMIT reached)",
            model::group_thousands(rows)
        )
    } else if rows == 1 {
        "the row".to_string()
    } else {
        format!("all {} rows", model::group_thousands(rows))
    }
}

/// `24 rows · 1 ms`, the LIMIT and sort notes, and the Copy / Export menus.
///
/// One duration per statement: the time the engine took to run it. A
/// multi-statement run adds the batch total once, at the right.
fn footer(
    result: &ResultSet,
    batch: Option<Batch>,
    grid: &Entity<ResultGrid>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let sort = grid.read(cx).sort(cx);
    let summary = format!(
        "{} · {}",
        model::row_count_label(result.row_count()),
        model::format_elapsed(result.elapsed)
    );
    let truncated = result.is_truncated();
    let auto_limit = result.auto_limit;
    h_flex()
        .flex_none()
        .h(px(30.))
        .px_3()
        .gap_3()
        .items_center()
        .overflow_hidden()
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_xs()
        .when_some(batch, |this, batch| {
            this.child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("Statement {} of {}", batch.position, batch.statements)),
            )
        })
        .child(
            div()
                .id("results-summary")
                .flex_none()
                .text_color(if batch.is_some() {
                    theme.muted_foreground
                } else {
                    theme.foreground
                })
                .child(summary)
                .tooltip(|window, cx| {
                    Tooltip::new("Rows returned · time the engine took to run this statement")
                        .build(window, cx)
                }),
        )
        .when_some(auto_limit.filter(|_| truncated), |this, limit| {
            this.child(truncation_notice(limit, cx))
        })
        .when_some(auto_limit.filter(|_| !truncated), |this, limit| {
            this.child(limit_note(limit, cx))
        })
        .when_some(sort, |this, (column, direction)| {
            this.child(sort_note(column, direction, result, grid, cx))
        })
        .child(div().flex_1())
        .when_some(batch.and_then(|b| b.total.map(|t| (b.statements, t))), |this, (n, total)| {
            this.child(
                div()
                    .id("batch-total")
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .child(format!("{n} statements · {} total", model::format_elapsed(total)))
                    .tooltip(|window, cx| {
                        Tooltip::new(
                            "Wall-clock time of the whole run, including the gaps between statements",
                        )
                        .build(window, cx)
                    }),
            )
        })
        .child(copy_menu(result, grid))
        .child(export_menu(result, grid))
}

/// "Showing the first N rows — DBCrust added LIMIT N", with the way out in
/// the tooltip.
fn truncation_notice(limit: usize, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .id("truncation-notice")
        .h(px(20.))
        .px_2()
        .gap_1p5()
        .items_center()
        .flex_none()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.warning.opacity(0.45))
        .bg(theme.warning.opacity(0.12))
        .text_color(theme.foreground)
        .child(
            Icon::new(IconName::TriangleAlert)
                .size_3()
                .text_color(theme.warning),
        )
        .child(format!(
            "Showing the first {} rows — DBCrust added LIMIT {limit}",
            model::group_thousands(limit)
        ))
        .tooltip(|window, cx| {
            Tooltip::new(
                "DBCrust adds a default LIMIT to SELECT statements to keep results fast. \
                 Add your own LIMIT to the query to fetch more rows.",
            )
            .build(window, cx)
        })
}

/// The default LIMIT was added but not reached: the result is complete.
fn limit_note(limit: usize, cx: &App) -> impl IntoElement {
    div()
        .id("limit-note")
        .flex_none()
        .text_color(cx.theme().muted_foreground)
        .child(format!("LIMIT {} added", model::group_thousands(limit)))
        .tooltip(move |window, cx| {
            Tooltip::new(format!(
                "DBCrust added LIMIT {} to this SELECT. It returned fewer rows, so the result is complete.",
                model::group_thousands(limit)
            ))
            .build(window, cx)
        })
}

/// "Sorted by customer ↑ · loaded rows only", with a way back.
fn sort_note(
    column: SharedString,
    direction: model::SortDirection,
    result: &ResultSet,
    grid: &Entity<ResultGrid>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let arrow = match direction {
        model::SortDirection::Ascending => "↑",
        model::SortDirection::Descending => "↓",
    };
    let loaded = model::group_thousands(result.row_count());
    let tooltip = if result.is_truncated() {
        format!(
            "Sorting is local: it orders the {loaded} loaded rows only, not the whole table. \
             Add ORDER BY to the query to sort on the server."
        )
    } else {
        format!(
            "Sorting is local: it orders the {loaded} loaded rows and does not re-run the query."
        )
    };
    let grid = grid.clone();
    h_flex()
        .id("sort-note")
        .flex_none()
        .gap_1p5()
        .items_center()
        .child(
            h_flex().gap_1().items_center().child("Sorted by").child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .child(format!("{column} {arrow}")),
            ),
        )
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child("· loaded rows only"),
        )
        .child(
            Button::new("clear-sort")
                .label("Clear")
                .ghost()
                .xsmall()
                .on_click(move |_, window, cx| {
                    grid.update(cx, |grid, cx| grid.clear_sort(window, cx));
                }),
        )
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
}

fn copy_menu(result: &ResultSet, grid: &Entity<ResultGrid>) -> impl IntoElement {
    let grid = grid.clone();
    let scope = scope_phrase(result);
    Button::new("results-copy")
        .xsmall()
        .ghost()
        .icon(IconName::Copy)
        .label("Copy")
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _window, cx| {
            let has_selection = grid.read(cx).has_selection(cx);
            let sorted = grid.read(cx).sort(cx).is_some();
            let menu = menu.min_w(px(220.)).item(
                PopupMenuItem::new("Copy selection")
                    .disabled(!has_selection)
                    .on_click({
                        let grid = grid.clone();
                        move |_, window, cx| {
                            grid.update(cx, |grid, cx| {
                                grid.copy_selection(window, cx);
                            });
                        }
                    }),
            );
            let heading = if sorted {
                format!("Copy {scope} as · sorted order")
            } else {
                format!("Copy {scope} as")
            };
            ExportFormat::ALL
                .iter()
                .fold(menu.separator().label(heading), |menu, &format| {
                    let grid = grid.clone();
                    menu.item(
                        PopupMenuItem::new(format.label()).on_click(move |_, window, cx| {
                            grid.update(cx, |grid, cx| grid.copy_all(format, window, cx));
                        }),
                    )
                })
        })
}

fn export_menu(result: &ResultSet, grid: &Entity<ResultGrid>) -> impl IntoElement {
    let grid = grid.clone();
    let scope = scope_phrase(result);
    Button::new("results-export")
        .xsmall()
        .ghost()
        .label("Export")
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _window, cx| {
            let sorted = grid.read(cx).sort(cx).is_some();
            let heading = if sorted {
                format!("Export {scope} · sorted order")
            } else {
                format!("Export {scope}")
            };
            ExportFormat::ALL.iter().fold(
                menu.min_w(px(220.)).label(heading).separator(),
                |menu, &format| {
                    let grid = grid.clone();
                    menu.item(PopupMenuItem::new(format!("{}…", format.label())).on_click(
                        move |_, window, cx| {
                            grid.update(cx, |grid, cx| grid.export(format, window, cx));
                        },
                    ))
                },
            )
        })
}

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

/// Statement with no result columns (DDL, DML): the backend reports no
/// affected-row count, so only the outcome and timing are shown.
fn message_state(result: &ResultSet, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (preview, cut) = model::statement_preview(&result.sql, 4, 120);
    centered()
        .child(
            Icon::new(IconName::CircleCheck)
                .size_6()
                .text_color(theme.success),
        )
        .child(
            h_flex()
                .gap_2()
                .items_baseline()
                .text_sm()
                .child("Statement executed")
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("· {}", model::format_elapsed(result.elapsed))),
                ),
        )
        .when(!preview.is_empty(), |this| {
            this.child(statement_block(preview, cut, cx))
        })
        .into_any_element()
}

fn empty_state(window: &mut Window, cx: &App) -> AnyElement {
    let theme = cx.theme();
    centered()
        .child(
            Icon::new(IconName::SquareTerminal)
                .size_8()
                .text_color(theme.muted_foreground.opacity(0.6)),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground.opacity(0.85))
                .child("Run a query to see results"),
        )
        .child(
            h_flex()
                .gap_5()
                .child(shortcut_hint(
                    "Run statement",
                    Kbd::binding_for_action(&RunStatement, None, window),
                    if cfg!(target_os = "macos") {
                        "⌘↵"
                    } else {
                        "Ctrl+Enter"
                    },
                    cx,
                ))
                .child(shortcut_hint(
                    "Run all",
                    Kbd::binding_for_action(&RunAll, None, window),
                    if cfg!(target_os = "macos") {
                        "⇧⌘↵"
                    } else {
                        "Ctrl+Shift+Enter"
                    },
                    cx,
                )),
        )
        .into_any_element()
}

/// `⌘↵ Run statement`: the real binding when there is one, else `fallback`.
fn shortcut_hint(
    label: &'static str,
    binding: Option<Kbd>,
    fallback: &'static str,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .gap_2()
        .items_center()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(match binding {
            Some(kbd) => kbd.into_any_element(),
            None => div()
                .h(px(18.))
                .px_1p5()
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .font_family(theme.mono_font_family.clone())
                .text_size(px(11.))
                .child(fallback)
                .into_any_element(),
        })
        .child(label)
}

fn running_state(since: Instant, statement: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (preview, cut) = model::statement_preview(statement, 6, 120);
    centered()
        .child(Spinner::new().large().color(theme.muted_foreground))
        .child(
            h_flex()
                .gap_2()
                .items_baseline()
                .text_sm()
                .child("Running…")
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(model::format_running(since.elapsed())),
                ),
        )
        .when(!preview.is_empty(), |this| {
            this.child(statement_block(preview, cut, cx))
        })
        .child(
            Button::new("cancel-query")
                .label("Cancel")
                .outline()
                .small()
                .on_click(|_, window, cx| window.dispatch_action(Box::new(CancelQuery), cx)),
        )
        .into_any_element()
}

/// Failed (red), blocked (amber) or cancelled (calm) statements.
fn error_state(error: &GuiError, statement: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let tone = ErrorTone::of(error);
    let (color, icon) = tone.style(cx);
    let (title, hint) = error_title(error);
    let message = error.to_string();
    let (preview, cut) = model::statement_preview(statement, 12, 200);

    let content = v_flex()
        .w_full()
        .max_w(px(820.))
        .p_4()
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(icon).size_4().text_color(color))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                ),
        )
        .when(tone != ErrorTone::Notice && !message.is_empty(), |this| {
            this.child(message_block(message.clone(), color, cx))
        })
        .when_some(hint, |this, hint| {
            this.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(hint),
            )
        })
        .when(!preview.is_empty(), |this| {
            this.child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Statement"),
                    )
                    .child(statement_block(preview, cut, cx)),
            )
        })
        .when(tone != ErrorTone::Notice, |this| {
            this.child(
                h_flex().child(
                    Button::new("copy-error")
                        .label("Copy error")
                        .icon(IconName::Copy)
                        .outline()
                        .small()
                        .on_click(move |_, window, cx| {
                            crate::clipboard::write(message.clone(), cx);
                            window.push_notification(
                                Notification::success("Copied error").id::<ErrorNotice>(),
                                cx,
                            );
                        }),
                ),
            )
        });

    div()
        .size_full()
        .child(
            div()
                .id("results-error")
                .size_full()
                .child(content)
                .overflow_y_scrollbar(),
        )
        .into_any_element()
}

/// The engine's message in a mono block with a tone-colored edge.
fn message_block(message: String, color: Hsla, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .items_stretch()
        .overflow_hidden()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.5))
        .child(div().w(px(3.)).flex_none().bg(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px_3()
                .py_2()
                .font_family(theme.mono_font_family.clone())
                .text_size(px(12.5))
                .child(SelectableText::new("error-message", message)),
        )
}

struct ErrorNotice;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorTone {
    /// The statement ran and failed.
    Failure,
    /// Refused before it ran (read-only mode, closed connection).
    Blocked,
    /// The user stopped it.
    Notice,
}

impl ErrorTone {
    /// Accent color and icon of this tone.
    fn style(self, cx: &App) -> (Hsla, IconName) {
        let theme = cx.theme();
        match self {
            ErrorTone::Failure => (theme.danger, IconName::CircleX),
            ErrorTone::Blocked => (theme.warning, IconName::Ban),
            ErrorTone::Notice => (theme.muted_foreground, IconName::Info),
        }
    }

    fn of(error: &GuiError) -> Self {
        match error {
            GuiError::Cancelled { .. } => ErrorTone::Notice,
            GuiError::ReadOnly(_) | GuiError::Disconnected => ErrorTone::Blocked,
            _ => ErrorTone::Failure,
        }
    }
}

/// Headline by kind, plus an optional line on what to do next.
fn error_title(error: &GuiError) -> (&'static str, Option<&'static str>) {
    match error {
        GuiError::Query(_) => ("Query failed", None),
        GuiError::ReadOnly(_) => (
            "Blocked by read-only mode",
            Some("Turn off read-only mode for this connection to run statements that change data."),
        ),
        GuiError::Cancelled { abandoned: false } => ("Statement cancelled", None),
        GuiError::Cancelled { abandoned: true } => (
            "Stopped waiting — the server may still finish the statement",
            Some(
                "This database cannot cancel a running statement; DBCrust stopped waiting for it.",
            ),
        ),
        GuiError::Disconnected => (
            "Connection closed",
            Some("Choose a connection for this tab to run the statement again."),
        ),
        GuiError::Connection(_) => ("Connection failed", None),
        GuiError::Unsupported(_) => ("Not available for this database", None),
        GuiError::Storage(_) | GuiError::Docker(_) | GuiError::Vault(_) => {
            ("Something went wrong", None)
        }
    }
}

/// A statement, as a muted mono block.
fn statement_block(preview: String, cut: bool, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .w_full()
        .max_w(px(640.))
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .bg(theme.muted.opacity(0.4))
        .border_1()
        .border_color(theme.border)
        .font_family(theme.mono_font_family.clone())
        .text_size(px(12.))
        .text_color(theme.muted_foreground)
        .child(if cut {
            format!("{preview}\n…")
        } else {
            preview
        })
}

fn centered() -> Div {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_6()
}
