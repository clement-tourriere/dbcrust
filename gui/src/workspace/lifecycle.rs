//! What the workspace persists and how the app shuts down.
//!
//! - Drafts (open query tabs with text), the sidebar width and visibility
//!   are saved to `gui_state.toml` every few seconds when they changed, and
//!   once more on quit. Every write goes through one [`OrderedWriter`]
//!   (`autosave.rs`): writes never overtake each other, a failed write is
//!   retried on the next tick, and "saved" only ever means "written".
//! - The close button hides the window in the tray when that is possible
//!   (`tray::close_action`): the drafts are queued for the writer, nothing
//!   waits for the disk, and the workspace keeps running untouched. The very
//!   first time a dialog explains it. Without a tray, closing quits.
//! - Quitting is one path: confirm if a query is running, wait (bounded) for
//!   the final save, close every connection, then `cx.quit()`. If the save
//!   fails the app stays open and offers "Quit anyway". GPUI's own quit hooks
//!   only get ~200 ms and cannot veto, so the real work happens before they
//!   run; the hook just does a best-effort save for the system-initiated
//!   quits (Dock → Quit, logout) that bypass this path.

use super::Workspace;
use super::autosave::{FlushOutcome, OrderedWriter};
use super::tabs::TabKind;
use crate::app_state::AppState;
use crate::preferences::{self, Preferences};
use crate::query::QueryTab;
use crate::tray::{self, CloseAction, Wording};
use dbcrust_gui_core::ConnectionId;
use dbcrust_gui_core::gui_state::{Draft, DraftTarget, GuiState};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogFooter,
    input::Rope,
    notification::Notification,
};
use gpui_kit::*;
use std::time::{Duration, Instant};

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(3);
/// How long quitting waits for the final save.
const FINAL_SAVE_TIMEOUT: Duration = Duration::from_secs(2);
/// What the system's quit hook can afford (GPUI allows ~200 ms in total).
const EXIT_SAVE_TIMEOUT: Duration = Duration::from_millis(150);

struct AutosaveNotice;
struct PreferencesNotice;

/// The persisted part of the workspace, compared to skip redundant writes.
/// Taking one every few seconds copies no query text: drafts hold the
/// editors' ropes, turned into text on the writer thread.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Snapshot {
    drafts: Vec<DraftSnapshot>,
    sidebar_visible: bool,
    sidebar_width: f32,
}

/// A query tab with text, as autosave sees it.
#[derive(Clone, Debug)]
struct DraftSnapshot {
    /// The tab and its edit revision stand for the text (see `eq`).
    tab: EntityId,
    revision: u64,
    title: String,
    /// A copy-on-write snapshot of the editor's text.
    sql: Rope,
    connection_key: Option<String>,
    target: Option<DraftTarget>,
}

impl PartialEq for DraftSnapshot {
    /// A tab holds the same text as long as its edit revision is the same
    /// (the length is compared too, against an edit that would not bump it):
    /// no buffer is compared character by character.
    fn eq(&self, other: &Self) -> bool {
        self.tab == other.tab
            && self.revision == other.revision
            && self.sql.len() == other.sql.len()
            && self.title == other.title
            && self.connection_key == other.connection_key
            && self.target == other.target
    }
}

impl DraftSnapshot {
    fn to_draft(&self) -> Draft {
        Draft {
            title: self.title.clone(),
            sql: self.sql.to_string(),
            connection_key: self.connection_key.clone(),
            target: self.target.clone(),
        }
    }
}

#[cfg(test)]
impl Snapshot {
    pub(super) fn drafts(&self) -> Vec<Draft> {
        self.drafts.iter().map(DraftSnapshot::to_draft).collect()
    }

    pub(super) fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }
}

impl Snapshot {
    /// Runs on the writer thread.
    fn apply(&self, state: &mut GuiState) {
        state.drafts = self.drafts.iter().map(DraftSnapshot::to_draft).collect();
        state.sidebar_visible = self.sidebar_visible;
        state.sidebar_width = self.sidebar_width;
    }
}

/// The workspace's single writer of `gui_state.toml`.
pub(super) fn new_writer() -> OrderedWriter<Snapshot> {
    OrderedWriter::new(|snapshot: &Snapshot| {
        GuiState::update(|state| snapshot.apply(state)).map_err(|error| error.to_string())
    })
}

/// The question asked before disconnecting while queries run: a title and
/// a description. `names` are the connections about to close.
fn disconnect_prompt(names: &[String], running: usize) -> (String, String) {
    let target = match names {
        [name] => format!(" on “{name}”"),
        _ => String::new(),
    };
    let closing = match names {
        [_] => "Disconnecting".to_string(),
        names => format!("Disconnecting {} connections", names.len()),
    };
    let description = if running == 1 {
        format!("A query is still running{target}. {closing} cancels it.")
    } else {
        format!("{running} queries are still running{target}. {closing} cancels them.")
    };
    (
        "Disconnect while a query is running?".to_string(),
        description,
    )
}

fn disconnect_now(targets: &[ConnectionId], cx: &mut App) {
    AppState::global(cx).update(cx, |state, cx| {
        for id in targets {
            state.disconnect(*id, cx);
        }
    });
}

/// `Query 7` → 7. Used to continue the numbering of restored drafts.
pub(super) fn query_number(title: &str) -> Option<usize> {
    title.strip_prefix("Query ")?.trim().parse().ok()
}

impl Workspace {
    pub(super) fn snapshot(&self, cx: &App) -> Snapshot {
        // With "Reopen unsaved queries at launch" off, query text is not
        // written to disk at all
        let drafts = if preferences::restore_drafts(cx) {
            self.tabs
                .iter()
                .filter_map(|tab| {
                    let TabKind::Query(query) = &tab.kind else {
                        return None;
                    };
                    let id = query.entity_id();
                    let query = query.read(cx);
                    (!query.is_blank(cx)).then(|| DraftSnapshot {
                        tab: id,
                        revision: query.edit_revision(),
                        title: query.title().to_string(),
                        sql: query.sql_snapshot(cx),
                        connection_key: tab.connection_key.clone(),
                        target: query.draft_target(),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        Snapshot {
            drafts,
            sidebar_visible: self.sidebar_visible,
            sidebar_width: f32::from(self.sidebar_width),
        }
    }

    /// Queue the current state for writing when it changed since the last
    /// submission, retry a failed write otherwise, and tell the user once
    /// when saving keeps failing.
    fn autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.snapshot(cx);
        if self.submitted.as_ref() == Some(&snapshot) {
            self.writer.retry_failed();
        } else {
            self.writer.submit(snapshot.clone());
            self.submitted = Some(snapshot);
        }
        // Preferences that could not be written wait for this tick too
        preferences::retry_failed(cx);
        // While the window is hidden nobody would see a notice: keep failures
        // unreported until the next tick after it is back
        if tray::window_hidden(cx) {
            return;
        }
        if let Some(error) = self.writer.take_unreported_error() {
            tracing::warn!("could not save the workspace state: {error}");
            window.push_notification(
                Notification::warning(format!("{error} DBCrust will keep trying."))
                    .title("Couldn’t save your open queries")
                    .id::<AutosaveNotice>(),
                cx,
            );
        }
        if let Some(error) = preferences::take_unreported_error() {
            tracing::warn!("could not save the preferences: {error}");
            window.push_notification(
                Notification::warning(format!("{error} DBCrust will keep trying."))
                    .title("Couldn’t save your settings")
                    .id::<PreferencesNotice>(),
                cx,
            );
        }
    }

    pub(super) fn start_autosave(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTOSAVE_INTERVAL).await;
                if this
                    .update_in(cx, |this, window, cx| this.autosave(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// Queue the current state (unless the file already holds it) and wait,
    /// on a background thread, until the writer is idle. Settings and
    /// preferences still waiting for the disk are written first, on a
    /// thread of their own: `timeout` bounds the whole flush, so a write
    /// that stalls ends in `TimedOut` (the user decides) instead of a hang.
    /// A settings save that fails is reported like the drafts; failing to
    /// save the preferences is only logged.
    fn flush_state(&mut self, timeout: Duration, cx: &App) -> Task<FlushOutcome> {
        let snapshot = self.snapshot(cx);
        if !self.writer.is_saved(&snapshot) {
            self.writer.submit(snapshot.clone());
            self.submitted = Some(snapshot);
        }
        let flusher = self.writer.flusher();
        let settings = crate::settings::flush_on_quit(cx);
        cx.background_spawn(async move {
            let deadline = Instant::now() + timeout;
            let (done, finished) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let settings = settings(deadline);
                if let Err(error) = preferences::flush() {
                    tracing::warn!("could not save the preferences on exit: {error}");
                }
                let _ = done.send(settings);
            });
            let Ok(settings) = finished.recv_timeout(timeout) else {
                return FlushOutcome::TimedOut;
            };
            match flusher.wait(deadline.saturating_duration_since(Instant::now())) {
                FlushOutcome::Saved => match settings {
                    Ok(()) => FlushOutcome::Saved,
                    Err(error) => FlushOutcome::Failed(error),
                },
                outcome => outcome,
            }
        })
    }

    /// Best-effort final save for quits that bypass [`Self::request_quit`]
    /// (Dock → Quit, logout): GPUI polls the returned task for ~200 ms and
    /// cannot be vetoed. `None` when the regular quit path already saved.
    pub(super) fn save_before_exit(&mut self, cx: &mut Context<Self>) -> Option<Task<()>> {
        if self.quitting.replace(true) {
            return None;
        }
        let flush = self.flush_state(EXIT_SAVE_TIMEOUT, cx);
        Some(cx.spawn(async move |_, _| match flush.await {
            FlushOutcome::Saved => {}
            FlushOutcome::Failed(error) => {
                tracing::warn!("could not save the workspace state on exit: {error}");
            }
            FlushOutcome::TimedOut => {
                tracing::warn!("saving the workspace state on exit timed out")
            }
        }))
    }

    /// Reopen the layout and the unsaved query tabs of the last session.
    pub(super) fn restore(
        &mut self,
        state: &GuiState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_visible = state.sidebar_visible;
        self.sidebar_width = px(state.sidebar_width);
        let drafts = if state.restore_drafts {
            state.drafts.as_slice()
        } else {
            &[]
        };
        for draft in drafts {
            if let Some(number) = query_number(&draft.title) {
                self.query_counter = self.query_counter.max(number);
            }
            let title = (!draft.title.is_empty()).then(|| draft.title.clone());
            let (id, query) = self.create_query_tab(None, title, draft.sql.clone(), window, cx);
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                tab.connection_key = draft.connection_key.clone();
            }
            if let Some(target) = draft.target.clone() {
                query.update(cx, |query, cx| query.restore_target(target, window, cx));
            }
        }
    }

    // ----- Quitting -----

    pub(crate) fn running_queries(&self, cx: &App) -> usize {
        self.tabs.iter().filter(|tab| tab.is_running(cx)).count()
    }

    /// Save, close every connection, quit. If the final save fails or takes
    /// too long the app stays open and the user decides.
    fn quit_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_task.is_some() {
            return;
        }
        self.flush_then_quit(0, window, cx);
    }

    /// Save, then quit — unless the drafts or the settings changed while
    /// saving (the app stays usable meanwhile): then save again, a few
    /// times at most, and ask the user rather than quit with them unsaved.
    fn flush_then_quit(&mut self, attempt: u8, window: &mut Window, cx: &mut Context<Self>) {
        const MAX_ATTEMPTS: u8 = 3;
        let flush = self.flush_state(FINAL_SAVE_TIMEOUT, cx);
        self.quit_task = Some(cx.spawn_in(window, async move |this, cx| {
            let outcome = flush.await;
            this.update_in(cx, |this, window, cx| {
                this.quit_task = None;
                match outcome {
                    FlushOutcome::Saved => {
                        // Checked here, on the UI thread, right before
                        // quitting: no edit can land in between
                        let snapshot = this.snapshot(cx);
                        let saved = this.writer.is_saved(&snapshot)
                            && !crate::settings::has_unsaved_settings(cx);
                        if saved {
                            this.finish_quit(cx);
                        } else if attempt + 1 < MAX_ATTEMPTS {
                            this.flush_then_quit(attempt + 1, window, cx);
                        } else {
                            // Never quit with changes that are not on disk:
                            // the user decides
                            this.save_failed_on_quit(
                                "Your queries or settings kept changing while they were saved."
                                    .to_string(),
                                window,
                                cx,
                            );
                        }
                    }
                    FlushOutcome::Failed(error) => this.save_failed_on_quit(error, window, cx),
                    FlushOutcome::TimedOut => this.save_failed_on_quit(
                        format!(
                            "Saving took longer than {} seconds.",
                            FINAL_SAVE_TIMEOUT.as_secs()
                        ),
                        window,
                        cx,
                    ),
                }
            })
            .ok();
        }));
    }

    /// The state is on disk (or the user chose to leave without it).
    fn finish_quit(&mut self, cx: &mut Context<Self>) {
        self.quitting.set(true);
        AppState::global(cx).update(cx, |state, cx| state.disconnect_all(cx));
        cx.quit();
    }

    fn save_failed_on_quit(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        tracing::warn!("could not save the workspace state on quit: {error}");
        // The user must see the question, also when quitting from the tray
        tray::reveal_window(window, cx);
        window.push_notification(
            Notification::error(error.clone())
                .title("Couldn’t save before quitting")
                .autohide(false)
                .id::<AutosaveNotice>(),
            cx,
        );
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let this = this.clone();
            alert
                .icon(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(cx.theme().warning),
                )
                .title("Couldn’t save before quitting")
                .description(format!(
                    "{error} Quitting now loses what is not saved: the text of your query \
                     tabs or recent settings changes."
                ))
                .confirm()
                .ok_text("Quit anyway")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Keep working")
                .on_ok(move |_, _, cx| {
                    _ = this.update(cx, |this, cx| this.finish_quit(cx));
                    true
                })
        });
    }

    fn confirm_quit_while_running(
        &self,
        running: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        let description = if running == 1 {
            "A query is still running. Quitting cancels it.".to_string()
        } else {
            format!("{running} queries are still running. Quitting cancels them.")
        };
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let this = this.clone();
            alert
                .icon(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(cx.theme().warning),
                )
                .title("Quit while a query is running?")
                .description(description.clone())
                .confirm()
                .ok_text("Quit")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Keep working")
                .on_ok(move |_, window, cx| {
                    _ = this.update(cx, |this, cx| this.quit_now(window, cx));
                    true
                })
        });
    }

    /// Quit command, quit shortcut, Dock menu.
    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.running_queries(cx);
        if running == 0 {
            self.quit_now(window, cx);
        } else {
            // Quitting from the tray with the window hidden: the question
            // needs a window
            tray::reveal_window(window, cx);
            self.confirm_quit_while_running(running, window, cx);
        }
    }

    // ----- Disconnecting -----

    /// Query tabs executing on one of `connections`.
    fn running_on(&self, connections: &[ConnectionId], cx: &App) -> usize {
        self.tabs
            .iter()
            .filter(|tab| tab.is_running(cx))
            .filter(|tab| {
                tab.connection(cx)
                    .is_some_and(|id| connections.contains(&id))
            })
            .count()
    }

    /// Close `targets` (the tray's Disconnect and Disconnect all). Closing a
    /// connection cancels what runs on it, so when a query runs on any of
    /// them the window is shown and the user confirms first; with nothing
    /// running there is no question.
    pub(crate) fn request_disconnect(
        &mut self,
        targets: Vec<ConnectionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let running = self.running_on(&targets, cx);
        if running == 0 {
            disconnect_now(&targets, cx);
            return;
        }
        // The question needs a window (the tray works with it hidden)
        tray::reveal_window(window, cx);
        if window.has_active_dialog(cx) {
            // Another question is open: it comes first
            return;
        }
        let names: Vec<String> = {
            let state = AppState::global(cx);
            let state = state.read(cx);
            targets
                .iter()
                .filter_map(|id| state.connection(*id))
                .map(|live| live.summary.label.clone())
                .collect()
        };
        let (title, description) = disconnect_prompt(&names, running);
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let targets = targets.clone();
            alert
                .icon(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(cx.theme().warning),
                )
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text("Disconnect")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Keep working")
                .on_ok(move |_, _, cx| {
                    disconnect_now(&targets, cx);
                    true
                })
        });
    }

    /// The window's close button. Returning `false` keeps the window: either
    /// it was hidden in the tray, or a dialog (or the final save) is pending.
    /// `true` lets it close, which only happens once the app is quitting.
    pub(super) fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.quitting.get() {
            return true;
        }
        if self.quit_task.is_some() {
            // Quitting is already under way (the final save is running)
            return false;
        }
        if tray::close_action(cx) == CloseAction::Hide {
            // Running queries keep running: the app is not going away
            self.hide_to_tray(window, cx);
            return false;
        }
        let running = self.running_queries(cx);
        if running > 0 {
            if !window.has_active_dialog(cx) {
                self.confirm_quit_while_running(running, window, cx);
            }
        } else {
            // Closing the main window quits the app: save first, and let the
            // window go once the state is on disk
            self.quit_now(window, cx);
        }
        false
    }

    /// Close to the tray. The first time, explain it before the window
    /// disappears (an OS notification would need permission, and a hidden
    /// window cannot show its own).
    fn hide_to_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Preferences::get(cx).tray_notice_shown {
            self.hide_now(window, cx);
        } else if !window.has_active_dialog(cx) {
            self.explain_background_mode(window, cx);
        }
        // Otherwise the question (or another dialog) is already open: another
        // click on the close button must not hide the window under it
    }

    /// Queue the drafts for the writer (nothing waits for the disk), then
    /// hide the window. If the platform refuses, fall back to quitting so the
    /// click is never silently ignored.
    fn hide_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autosave(window, cx);
        if !tray::hide_main_window(window, cx) {
            self.request_quit(window, cx);
        }
    }

    /// "DBCrust will keep running in the menu bar…": Keep running (primary)
    /// or Quit DBCrust. Esc leaves the window open and asks again next time.
    /// The answer is remembered either way.
    fn explain_background_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wording = Wording::current(Preferences::get(cx).hide_dock_icon_when_closed);
        let title = format!("Keep DBCrust running in the {}?", wording.area);
        let message = wording.first_close_message();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let keep = this.clone();
            let quit = this.clone();
            alert
                .icon(
                    Icon::new(IconName::Info)
                        .small()
                        .text_color(cx.theme().primary),
                )
                .title(title.clone())
                .description(message.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("tray-quit")
                                .outline()
                                .label("Quit DBCrust")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    Preferences::update(cx, |p| p.tray_notice_shown = true);
                                    _ = quit.update(cx, |this, cx| this.request_quit(window, cx));
                                }),
                        )
                        .child(
                            Button::new("tray-keep")
                                .primary()
                                .label("Keep running")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    Preferences::update(cx, |p| p.tray_notice_shown = true);
                                    _ = keep.update(cx, |this, cx| this.hide_now(window, cx));
                                }),
                        ),
                )
        });
    }

    /// A query tab by entity, for event handlers.
    pub(super) fn tab_index_of_query(&self, query: &Entity<QueryTab>) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.as_query().is_some_and(|candidate| candidate == query))
    }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[rstest::rstest]
    #[case(&["prod"], 1, "A query is still running on “prod”. Disconnecting cancels it.")]
    #[case(&["prod"], 3, "3 queries are still running on “prod”. Disconnecting cancels them.")]
    #[case(&["prod", "staging"], 1, "A query is still running. Disconnecting 2 connections cancels it.")]
    #[case(&["a", "b", "c"], 2, "2 queries are still running. Disconnecting 3 connections cancels them.")]
    fn names_what_disconnecting_cancels(
        #[case] names: &[&str],
        #[case] running: usize,
        #[case] expected: &str,
    ) {
        let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        let (title, description) = disconnect_prompt(&names, running);
        assert_eq!(title, "Disconnect while a query is running?");
        assert_eq!(description, expected);
    }

    #[test]
    fn parses_default_query_titles() {
        assert_eq!(query_number("Query 7"), Some(7));
        assert_eq!(query_number("Query 12 "), Some(12));
        assert_eq!(query_number("Top customers"), None);
        assert_eq!(query_number("Query"), None);
        assert_eq!(query_number("Query x"), None);
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::Workspace;
    use crate::RestoredState;
    use dbcrust_gui_core::ConfigStore;
    use dbcrust_gui_core::dbcrust_app::config::Config;
    use dbcrust_gui_core::gui_state::GuiState;
    use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

    fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
        crate::test_support::isolate();
        cx.update(|cx| {
            crate::init(ConfigStore::from_config(Config::default()), cx);
            cx.set_global(RestoredState(GuiState::default()));
            crate::open_main_window(WindowOptions::default(), cx).expect("open the main window")
        })
    }

    #[gpui_kit::test]
    fn a_snapshot_changes_with_the_text_and_keeps_it_exactly(cx: &mut TestAppContext) {
        let (window, workspace) = open(cx);
        let query = cx
            .update_window(window, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.open_query_tab(None, None, String::new(), window, cx);
                    workspace.open_query_tab(
                        None,
                        Some("Scratch".into()),
                        "select 1".into(),
                        window,
                        cx,
                    )
                })
            })
            .expect("window");
        let snapshot = |cx: &mut TestAppContext| cx.update(|cx| workspace.read(cx).snapshot(cx));

        // Nothing changed: equal, so autosave writes nothing
        let first = snapshot(cx);
        assert_eq!(snapshot(cx), first);
        let drafts = first.drafts();
        assert_eq!(drafts.len(), 1, "a blank tab is not a draft");
        assert_eq!(drafts[0].sql, "select 1");

        // An edit of the same length is a change, and the text is kept as is
        let text = "select 2\n\n  -- é\tdone ";
        cx.update_window(window, |_, window, cx| {
            query.update(cx, |query, cx| query.set_sql("select 2".into(), window, cx));
        })
        .expect("window");
        let second = snapshot(cx);
        assert_ne!(second, first);
        assert_eq!(second.drafts()[0].sql, "select 2");
        cx.update_window(window, |_, window, cx| {
            query.update(cx, |query, cx| query.set_sql(text.into(), window, cx));
        })
        .expect("window");
        let third = snapshot(cx);
        assert_ne!(third, second);
        let mut state = GuiState::default();
        third.apply(&mut state);
        assert_eq!(state.drafts.len(), 1);
        assert_eq!(state.drafts[0].sql, text);
        assert_eq!(state.drafts[0].title, "Scratch");

        // A rename alone is a change too
        cx.update(|cx| query.update(cx, |query, cx| query.rename("Renamed", cx)));
        assert_ne!(snapshot(cx), third);
    }
}
