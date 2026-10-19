//! Running statements: "Run" (statement under the caret or the selection),
//! "Run all" (every statement in order, stopping at the first error),
//! "Explain", and cancellation.
//!
//! Every run gets a generation number. Replies carry it back, and a reply
//! whose generation is no longer the tab's current run is dropped, so a
//! superseded or abandoned run can never overwrite newer results.

use super::history;
use super::provenance::Executed;
use super::results::ResultsContent;
use super::{ActiveRun, QueryStatus, QueryTab, QueryTabEvent};
use dbcrust_gui_core::completion::leading_keyword;
use dbcrust_gui_core::dbcrust_core::sql_buffer::{
    has_blank_line_separator, parses_as_single_statement,
};
use dbcrust_gui_core::history::{HistoryEntry, HistoryOutcome};
use dbcrust_gui_core::statements::{Execution, SqlDialect, StatementIndex, TargetKind, dialect_of};
use dbcrust_gui_core::{
    ConnectionHandle, ConnectionId, ExplainRaw, GuiError, RequestId, ResultSet,
};
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a run executes.
enum Job {
    /// `sliced`: the statements are those of a selection, not of the whole
    /// buffer.
    Statements {
        list: Vec<Execution>,
        sliced: bool,
    },
    Explain {
        execution: Execution,
        analyze: bool,
    },
}

impl Job {
    fn first_sql(&self) -> &str {
        match self {
            Job::Statements { list, .. } => list.first().map_or("", |e| e.sql.as_str()),
            Job::Explain { execution, .. } => execution.sql.as_str(),
        }
    }

    /// What is being run, and how it sits in `buffer`, for telling later
    /// whether an edit changed it.
    fn executed(&self, index: &StatementIndex) -> Vec<Executed> {
        let executed = |execution: &Execution, sliced: bool| {
            let aligned = !sliced && execution.kind != TargetKind::Selection;
            Executed::new(index, execution.range.clone(), aligned)
        };
        match self {
            Job::Statements { list, sliced } => list.iter().map(|e| executed(e, *sliced)).collect(),
            Job::Explain { execution, .. } => vec![executed(execution, false)],
        }
    }
}

impl QueryTab {
    /// The selection if there is one, else the statement under the caret. A
    /// selection that holds several statements runs them as a batch, like
    /// "Run all" on the selection; a single statement, or part of one, is
    /// sent exactly as selected.
    pub fn run_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.prepare(window, cx, |index, selection| {
            let execution = index.resolve(selection.clone())?;
            if execution.kind == TargetKind::Selection {
                let batch = index.all(selection);
                if batch.len() > 1 {
                    return Some(Job::Statements {
                        list: batch,
                        sliced: true,
                    });
                }
            }
            Some(Job::Statements {
                list: vec![execution],
                sliced: false,
            })
        }) else {
            return;
        };
        self.start(job, window, cx);
    }

    /// Every statement of the selection (or the buffer), in order.
    pub fn run_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(job) = self.prepare(window, cx, |index, selection| {
            let sliced = !selection.is_empty();
            let all = index.all(selection);
            (!all.is_empty()).then_some(Job::Statements { list: all, sliced })
        }) else {
            return;
        };
        self.start(job, window, cx);
    }

    /// EXPLAIN the statement under the caret (or the selection). A statement
    /// that already starts with EXPLAIN just runs, as a normal query.
    pub fn explain(&mut self, analyze: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.run.is_some() {
            return;
        }
        let unsupported = self
            .summary(cx)
            .is_some_and(|summary| !summary.supports_explain_analyze());
        if analyze && unsupported {
            self.warn(
                "Explain Analyze isn’t available",
                "EXPLAIN ANALYZE runs the statement and returns a structured plan; it is only available for PostgreSQL connections.",
                window,
                cx,
            );
            return;
        }
        // One statement at a time: a selection holding several is refused
        // rather than explaining only the first
        let (index, selection) = (
            self.statement_index(cx),
            self.editor.read(cx).selected_range(),
        );
        if !selection.is_empty() && index.all(selection).len() > 1 {
            self.warn(
                "Can’t explain several statements",
                "Select one statement to explain.",
                window,
                cx,
            );
            return;
        }
        let Some(job) = self.prepare(window, cx, move |index, selection| {
            index.resolve(selection).map(|execution| {
                if leading_keyword(&execution.sql).as_deref() == Some("EXPLAIN") {
                    Job::Statements {
                        list: vec![execution],
                        sliced: false,
                    }
                } else {
                    Job::Explain { execution, analyze }
                }
            })
        }) else {
            return;
        };
        self.start(job, window, cx);
    }

    /// Ask the worker to stop the running statement. The run ends when the
    /// worker answers (immediately for engines that cannot cancel server
    /// side: the client just stops waiting).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.cancelling {
            return;
        }
        run.cancelling = true;
        run.handle.cancel(run.request);
        cx.notify();
    }

    /// The tab is being bound to `connection`: end a run that started on
    /// another one. Its statement is cancelled there (a queued one never
    /// starts), and the run is dropped, which ends its task, so the rest of
    /// a "Run all" is never sent and a late reply finds no current run.
    /// Returns the connection the run was on.
    pub(super) fn stop_run_elsewhere(&mut self, connection: ConnectionId) -> Option<ConnectionId> {
        if self.run.as_ref()?.handle.id() == connection {
            return None;
        }
        let run = self.run.take()?;
        run.handle.cancel(run.request);
        // Never reuse the dropped run's number
        self.generation += 1;
        self.status = QueryStatus::Idle;
        Some(run.handle.id())
    }

    /// The engine rejected `statement`: if it looks like several statements
    /// written without a delimiter, say so (the engine's message stays).
    fn with_delimiter_hint(&self, error: GuiError, statement: &str, cx: &App) -> GuiError {
        let Some(summary) = self.summary(cx) else {
            return error;
        };
        if summary.runs_unsplit() {
            return error;
        }
        let dialect = dialect_of(Some(&summary.db_type));
        missing_delimiter_hint(error, statement, dialect)
    }

    /// True while a run is in flight and not yet asked to stop.
    pub(super) fn is_running(&self) -> bool {
        self.run.is_some()
    }

    pub(super) fn is_cancelling(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.cancelling)
    }

    /// Resolve what to run from the editor, or tell the user why nothing
    /// runs. `None` when nothing should start.
    fn prepare(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        resolve: impl FnOnce(&StatementIndex, std::ops::Range<usize>) -> Option<Job>,
    ) -> Option<Job> {
        if self.run.is_some() {
            return None;
        }
        if self.live(cx).is_none() {
            self.warn(
                "No connection",
                "Choose a connection in the toolbar to run this query.",
                window,
                cx,
            );
            return None;
        }
        let selection = self.editor.read(cx).selected_range();
        let job = resolve(&self.statement_index(cx), selection);
        if job.is_none() {
            window.push_notification(
                Notification::info("Write a statement, or select the part to run.")
                    .title("Nothing to run"),
                cx,
            );
        }
        job
    }

    fn start(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        let Some((handle, summary)) = self.live(cx) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let since = Instant::now();
        let first_request = RequestId::next();
        self.status = QueryStatus::Running { since };
        // The panel shows the running statement now, not a previous run
        self.stale = None;
        let statement = job.first_sql().to_string();
        // What is about to run, and where it sits, to tell later whether an
        // edit changed it
        let executed = job.executed(&self.statement_index(cx));
        self.show_results(ResultsContent::Running { since, statement }, window, cx);
        cx.emit(QueryTabEvent::StatusChanged);

        let label = summary.label.clone();
        let worker = handle.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            execute(
                this,
                cx,
                generation,
                worker,
                label,
                first_request,
                since,
                job,
            )
            .await;
        });
        self.run = Some(ActiveRun {
            generation,
            request: first_request,
            handle,
            cancelling: false,
            executed,
            _task: task,
        });
        cx.notify();
    }

    // ----- Replies (called from the run task, on the UI thread) -----

    fn is_current(&self, generation: u64) -> bool {
        self.run
            .as_ref()
            .is_some_and(|run| run.generation == generation)
    }

    /// A later statement of "Run all" is about to start: register its
    /// request (so Cancel targets it) and show it as running. False when
    /// the run was superseded or cancelled in between.
    fn begin_statement(
        &mut self,
        generation: u64,
        request: RequestId,
        sql: &str,
        completed: Vec<Arc<ResultSet>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        if self.is_cancelling() {
            self.finish(
                generation,
                Finish::Failed {
                    error: GuiError::Cancelled { abandoned: false },
                    statement: sql.to_string(),
                    note: None,
                    completed,
                },
                window,
                cx,
            );
            return false;
        }
        if let Some(run) = self.run.as_mut() {
            run.request = request;
        }
        if let QueryStatus::Running { since } = self.status {
            self.show_results(
                ResultsContent::Running {
                    since,
                    statement: sql.to_string(),
                },
                window,
                cx,
            );
        }
        true
    }

    fn finish(
        &mut self,
        generation: u64,
        outcome: Finish,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_current(generation) {
            return;
        }
        // What ran, to compare with the editor once its results are shown
        let executed = self.run.take().map(|run| run.executed).unwrap_or_default();
        match outcome {
            Finish::Results { sets, total } => {
                let rows = sets.last().map_or(0, |set| set.row_count());
                // One statement: the time the engine took, the number the
                // results footer shows. Several: the whole run.
                let elapsed = match sets.as_slice() {
                    [only] => only.elapsed,
                    _ => total,
                };
                self.status = QueryStatus::Done { rows, elapsed };
                let ran = executed.into_iter().take(sets.len()).collect();
                self.show_results(ResultsContent::Results { sets, total }, window, cx);
                self.results_from(ran, cx);
            }
            Finish::Plan { raw } => {
                let rows = raw.rows.len().saturating_sub(1);
                self.status = QueryStatus::Done {
                    rows,
                    elapsed: raw.elapsed,
                };
                let ran = executed.into_iter().take(1).collect();
                self.show_results(ResultsContent::Plan(Arc::new(raw)), window, cx);
                self.results_from(ran, cx);
            }
            Finish::Failed {
                error,
                statement,
                note,
                completed,
            } => {
                self.status = if error.is_cancelled() {
                    QueryStatus::Cancelled
                } else {
                    QueryStatus::Failed
                };
                let error = match note {
                    Some(prefix) => with_prefix(error, &prefix),
                    None => error,
                };
                let error = self.with_delimiter_hint(error, &statement, cx);
                // The statements that finished are the first ones of the run
                let ran: Vec<Executed> = executed.into_iter().take(completed.len()).collect();
                // What finished before the failure (or, failing that, the
                // previous results) stays on screen under the error
                self.show_results(
                    ResultsContent::Failed {
                        error,
                        statement,
                        completed,
                    },
                    window,
                    cx,
                );
                if !ran.is_empty() {
                    self.results_from(ran, cx);
                } else if self.results.read(cx).is_showing_results() {
                    // The previous results stayed: they still age with the editor
                    self.stale = None;
                    self.refresh_stale(cx);
                } else {
                    // A bare error: no results left to compare with the editor
                    self.ran = None;
                    self.stale = None;
                }
            }
        }
        cx.emit(QueryTabEvent::StatusChanged);
        cx.notify();
    }
}

/// How a run ended.
enum Finish {
    Results {
        sets: Vec<Arc<ResultSet>>,
        /// Wall-clock time of the whole run.
        total: Duration,
    },
    Plan {
        raw: ExplainRaw,
    },
    Failed {
        error: GuiError,
        statement: String,
        /// Which statement of a script failed ("Statement 2 of 5 failed").
        note: Option<String>,
        /// Results of the statements that finished before it.
        completed: Vec<Arc<ResultSet>>,
    },
}

/// What to tell the user when a rejected statement holds a blank line.
const MISSING_DELIMITER_HINT: &str =
    "This may contain several statements: separate them with `;`, or select the one to run.";

/// Whether an engine's message is about the SQL's grammar (as opposed to a
/// missing table, a permission, a timeout…).
fn is_syntax_error(message: &str) -> bool {
    let message = message.to_lowercase();
    [
        "syntax",
        "parse error",
        "parser error",
        "parsererror",
        "sql parser",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

/// `error` with the missing-delimiter hint appended when it is a syntax
/// error of `sql`, `sql` holds a blank line between pieces of SQL (outside
/// strings, comments and parentheses), and it does not read as one statement.
/// Cancellations, connection errors and the like never get it.
fn missing_delimiter_hint(error: GuiError, sql: &str, dialect: SqlDialect) -> GuiError {
    match error {
        GuiError::Query(message)
            if is_syntax_error(&message)
                && has_blank_line_separator(sql)
                && !parses_as_single_statement(sql, dialect) =>
        {
            GuiError::Query(format!("{message}\n\n{MISSING_DELIMITER_HINT}"))
        }
        other => other,
    }
}

/// `prefix` in front of an engine/guard message; other errors keep their text.
fn with_prefix(error: GuiError, prefix: &str) -> GuiError {
    match error {
        GuiError::Query(message) => GuiError::Query(format!("{prefix}: {message}")),
        GuiError::ReadOnly(message) => GuiError::ReadOnly(format!("{prefix}: {message}")),
        other => other,
    }
}

fn history_outcome(error: &GuiError) -> HistoryOutcome {
    match error {
        GuiError::Cancelled { .. } => HistoryOutcome::Cancelled,
        other => HistoryOutcome::Error(other.to_string()),
    }
}

/// The body of a run task.
#[allow(clippy::too_many_arguments)]
async fn execute(
    this: WeakEntity<QueryTab>,
    cx: &mut AsyncWindowContext,
    generation: u64,
    handle: ConnectionHandle,
    label: String,
    first_request: RequestId,
    since: Instant,
    job: Job,
) {
    match job {
        Job::Statements { list, .. } => {
            let total = list.len();
            let mut done: Vec<Arc<ResultSet>> = Vec::with_capacity(total);
            for (index, execution) in list.into_iter().enumerate() {
                let request = if index == 0 {
                    first_request
                } else {
                    RequestId::next()
                };
                if index > 0 {
                    let sql = execution.sql.clone();
                    let proceed = this
                        .update_in(cx, |tab, window, cx| {
                            tab.begin_statement(generation, request, &sql, done.clone(), window, cx)
                        })
                        .unwrap_or(false);
                    if !proceed {
                        return;
                    }
                }
                let started = Instant::now();
                match handle.execute(request, execution.sql.clone()).await {
                    Ok(set) => {
                        let entry = HistoryEntry::finished(
                            execution.sql.clone(),
                            label.clone(),
                            set.elapsed,
                            Some(set.row_count()),
                            HistoryOutcome::Ok,
                        );
                        done.push(Arc::new(set));
                        this.update(cx, |_, cx| history::record(entry, cx)).ok();
                    }
                    Err(error) => {
                        let entry = HistoryEntry::finished(
                            execution.sql.clone(),
                            label.clone(),
                            started.elapsed(),
                            None,
                            history_outcome(&error),
                        );
                        let note = (total > 1 && !error.is_cancelled())
                            .then(|| format!("Statement {} of {total} failed", index + 1));
                        let statement = execution.sql;
                        this.update_in(cx, |tab, window, cx| {
                            history::record(entry, cx);
                            tab.finish(
                                generation,
                                Finish::Failed {
                                    error,
                                    statement,
                                    note,
                                    completed: done,
                                },
                                window,
                                cx,
                            );
                        })
                        .ok();
                        return;
                    }
                }
            }
            this.update_in(cx, |tab, window, cx| {
                tab.finish(
                    generation,
                    Finish::Results {
                        sets: done,
                        total: since.elapsed(),
                    },
                    window,
                    cx,
                );
            })
            .ok();
        }
        Job::Explain { execution, analyze } => {
            let started = Instant::now();
            let shown = format!(
                "EXPLAIN {}{}",
                if analyze { "ANALYZE " } else { "" },
                execution.sql
            );
            match handle
                .explain(first_request, execution.sql.clone(), analyze)
                .await
            {
                Ok(raw) => {
                    let entry =
                        HistoryEntry::finished(shown, label, raw.elapsed, None, HistoryOutcome::Ok);
                    this.update_in(cx, |tab, window, cx| {
                        history::record(entry, cx);
                        tab.finish(generation, Finish::Plan { raw }, window, cx);
                    })
                    .ok();
                }
                Err(error) => {
                    let entry = HistoryEntry::finished(
                        shown,
                        label,
                        started.elapsed(),
                        None,
                        history_outcome(&error),
                    );
                    let statement = execution.sql;
                    this.update_in(cx, |tab, window, cx| {
                        history::record(entry, cx);
                        tab.finish(
                            generation,
                            Finish::Failed {
                                error,
                                statement,
                                note: None,
                                completed: Vec::new(),
                            },
                            window,
                            cx,
                        );
                    })
                    .ok();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: see the note in `editor.rs` tests
    use super::{MISSING_DELIMITER_HINT, history_outcome, missing_delimiter_hint, with_prefix};
    use dbcrust_gui_core::GuiError;
    use dbcrust_gui_core::history::HistoryOutcome;
    use dbcrust_gui_core::statements::SqlDialect;
    use rstest::rstest;

    const PG_SYNTAX: &str = "syntax error at or near \"select\"";

    #[rstest]
    // Two queries without a delimiter, rejected as a syntax error
    #[case("select 1\n\nselect 2", PG_SYNTAX, true)]
    #[case("select 1\r\n\r\nselect 2", "near \"select\": syntax error", true)]
    #[case(
        "select 1\n\nselect * from",
        "You have an error in your SQL syntax",
        true
    )]
    #[case(
        "select 1\n\nfoo bar",
        "sql parser error: Expected: end of statement",
        true
    )]
    // Not a syntax error
    #[case("select 1\n\nselect * from nope", "no such table: nope", false)]
    #[case(
        "select 1\n\nselect 2",
        "canceling statement due to user request",
        false
    )]
    // No blank line between pieces of SQL
    #[case("select 1 select 2", PG_SYNTAX, false)]
    #[case("select 'a\n\nb' frm", PG_SYNTAX, false)]
    #[case("select (1\n\n", PG_SYNTAX, false)]
    // It is one statement: the engine just disagrees with the grammar
    #[case("select a\n\nfrom t", PG_SYNTAX, false)]
    fn the_missing_delimiter_hint_needs_a_syntax_error_and_a_blank_line(
        #[case] sql: &str,
        #[case] message: &str,
        #[case] shown: bool,
    ) {
        let error = GuiError::Query(message.to_string());
        let hinted = missing_delimiter_hint(error.clone(), sql, SqlDialect::PostgreSql);
        if shown {
            assert_eq!(
                hinted,
                GuiError::Query(format!("{message}\n\n{MISSING_DELIMITER_HINT}"))
            );
            assert!(MISSING_DELIMITER_HINT.contains("separate them with `;`"));
        } else {
            assert_eq!(hinted, error);
        }
    }

    #[test]
    fn only_engine_query_errors_get_the_hint() {
        for error in [
            GuiError::Disconnected,
            GuiError::Cancelled { abandoned: false },
            GuiError::ReadOnly("syntax".into()),
        ] {
            assert_eq!(
                missing_delimiter_hint(error.clone(), "select 1\n\nselect 2", SqlDialect::Generic),
                error
            );
        }
    }

    #[test]
    fn prefix_only_applies_to_engine_messages() {
        assert_eq!(
            with_prefix(GuiError::Query("boom".into()), "Statement 2 of 3 failed"),
            GuiError::Query("Statement 2 of 3 failed: boom".into())
        );
        assert_eq!(
            with_prefix(GuiError::Disconnected, "Statement 2 of 3 failed"),
            GuiError::Disconnected
        );
    }

    #[test]
    fn cancellation_is_recorded_as_such() {
        assert_eq!(
            history_outcome(&GuiError::Cancelled { abandoned: true }),
            HistoryOutcome::Cancelled
        );
        assert!(matches!(
            history_outcome(&GuiError::Query("x".into())),
            HistoryOutcome::Error(message) if message == "x"
        ));
    }
}
