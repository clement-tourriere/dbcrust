//! Desktop preferences: the settings stored in `gui_state.toml` that the rest
//! of the app reads while it runs (close behaviour, draft restoration, editor
//! font size). The Settings page edits them through [`Preferences::update`];
//! everything else reads them with [`Preferences::get`] or the shortcuts at
//! the bottom of this file.
//!
//! The values live in a GPUI global, so they apply immediately and views can
//! react to a change with `cx.observe_global::<Preferences>(..)`. Changes are
//! written to disk in the background through an [`Outbox`]:
//!
//! - the newest value always wins, whatever order the writes run in;
//! - a value stays pending until a write *succeeded*; a failed write is
//!   retried by the workspace's autosave tick ([`retry_failed`]) and a
//!   persistent failure is surfaced once ([`take_unreported_error`]);
//! - the coordinated quit writes whatever is still pending ([`flush`]).

use dbcrust_gui_core::gui_state::{GuiState, StatementSeparators};
use gpui_kit::*;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    /// Closing the main window hides it and keeps the app in the menu bar /
    /// notification area (only when a tray icon exists).
    pub keep_running_in_tray: bool,
    /// macOS: also hide the Dock icon while the window is hidden.
    pub hide_dock_icon_when_closed: bool,
    /// The first-close explanation was answered.
    pub tray_notice_shown: bool,
    /// Reopen unsaved query tabs at launch (and persist them while running).
    pub restore_drafts: bool,
    /// SQL editor font size in pixels, within
    /// [`GuiState::MIN_EDITOR_FONT_SIZE`]..=[`GuiState::MAX_EDITOR_FONT_SIZE`].
    pub editor_font_size: f32,
    /// What ends a statement in the query editor.
    pub statement_separators: StatementSeparators,
}

impl Default for Preferences {
    fn default() -> Self {
        Self::from_state(&GuiState::default())
    }
}

impl Global for Preferences {}

/// Preferences not yet confirmed on disk (see [`Outbox`]).
static OUTBOX: Mutex<Outbox> = Mutex::new(Outbox::new());

impl Preferences {
    pub fn from_state(state: &GuiState) -> Self {
        Self {
            keep_running_in_tray: state.keep_running_in_tray,
            hide_dock_icon_when_closed: state.hide_dock_icon_when_closed,
            tray_notice_shown: state.tray_notice_shown,
            restore_drafts: state.restore_drafts,
            editor_font_size: GuiState::clamp_editor_font_size(state.editor_font_size),
            statement_separators: state.statement_separators,
        }
    }

    /// Copy these preferences into `state`, leaving everything else alone.
    pub fn apply(&self, state: &mut GuiState) {
        state.keep_running_in_tray = self.keep_running_in_tray;
        state.hide_dock_icon_when_closed = self.hide_dock_icon_when_closed;
        state.tray_notice_shown = self.tray_notice_shown;
        state.restore_drafts = self.restore_drafts;
        state.editor_font_size = GuiState::clamp_editor_font_size(self.editor_font_size);
        state.statement_separators = self.statement_separators;
    }

    /// Register the preferences of the state loaded at startup. Call once
    /// from [`crate::init`].
    pub fn init(state: &GuiState, cx: &mut App) {
        cx.set_global(Self::from_state(state));
    }

    /// The current preferences (the defaults before [`Preferences::init`],
    /// as in unit tests).
    pub fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }

    /// Change preferences: applied now, saved in the background. Observers of
    /// the global are notified only when something actually changed.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
        let mut next = Self::get(cx);
        change(&mut next);
        next.editor_font_size = GuiState::clamp_editor_font_size(next.editor_font_size);
        if cx.try_global::<Self>() == Some(&next) {
            return;
        }
        cx.set_global(next.clone());
        persist(next, cx);
    }
}

/// The edit a write applies to the file's state, under the file lock.
type Apply<'a> = &'a mut dyn FnMut(&mut GuiState);

/// Why a write failed, and which revision it was about.
#[derive(Debug, PartialEq)]
struct Failure {
    /// The newest revision a failed write was storing (or, when it failed
    /// before storing anything, the newest revision at that moment).
    revision: u64,
    message: String,
}

/// What still has to reach the file, and how the last attempts went.
struct Outbox {
    /// Bumped by every change.
    revision: u64,
    /// The newest value not yet confirmed on disk.
    value: Option<Preferences>,
    /// The newest failure. A success clears it only if that success stored
    /// this revision or a newer one: writes finish in any order, and a late
    /// success of an older value must not hide a failure of a newer one.
    failure: Option<Failure>,
    /// `failure` was handed out by [`Outbox::take_unreported_error`].
    reported: bool,
    /// Write tasks currently running.
    writing: usize,
}

impl Outbox {
    const fn new() -> Self {
        Self {
            revision: 0,
            value: None,
            failure: None,
            reported: false,
            writing: 0,
        }
    }

    fn submit(&mut self, value: Preferences) {
        self.revision += 1;
        self.value = Some(value);
    }

    /// The value a write should store now, with its revision.
    fn current(&self) -> Option<(u64, Preferences)> {
        self.value.clone().map(|value| (self.revision, value))
    }

    /// A write finished. `stored` is the revision it put in the file. The
    /// value is acknowledged only if the write succeeded and nothing newer
    /// arrived meanwhile; a failure keeps it for the next attempt.
    fn finish(&mut self, stored: Option<u64>, result: Result<(), String>) {
        match result {
            Ok(()) => {
                if stored == Some(self.revision) {
                    self.value = None;
                }
                // A success covers a failure only up to the revision it stored
                let covered = stored.unwrap_or(0);
                if self.failure.as_ref().is_some_and(|f| f.revision <= covered) {
                    self.failure = None;
                    self.reported = false;
                }
            }
            Err(message) => {
                let attempted = stored.unwrap_or(self.revision);
                match &mut self.failure {
                    // The same streak: keep the newest revision, and the
                    // fact that it was already reported
                    Some(failure) => {
                        failure.revision = failure.revision.max(attempted);
                        failure.message = message;
                    }
                    None => {
                        self.reported = false;
                        self.failure = Some(Failure {
                            revision: attempted,
                            message,
                        });
                    }
                }
            }
        }
    }

    /// The current failure, once per failure streak.
    fn take_unreported_error(&mut self) -> Option<String> {
        if self.reported {
            return None;
        }
        let error = self.failure.as_ref()?.message.clone();
        self.reported = true;
        Some(error)
    }

    /// A failed value waits for a retry and no write is already on its way.
    fn needs_retry(&self) -> bool {
        self.value.is_some() && self.failure.is_some() && self.writing == 0
    }
}

fn outbox(outbox: &Mutex<Outbox>) -> std::sync::MutexGuard<'_, Outbox> {
    outbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Store the newest pending value through `store`, which runs its callback
/// while it holds the file lock: whichever write gets there last therefore
/// stores the newest value, and two quick changes can never leave the older
/// one on disk. The value is acknowledged only after `store` succeeded.
fn write_pending(
    pending: &Mutex<Outbox>,
    store: impl FnOnce(Apply<'_>) -> Result<(), String>,
) -> Result<(), String> {
    let mut stored = None;
    let result = store(&mut |state: &mut GuiState| {
        if let Some((revision, value)) = outbox(pending).current() {
            value.apply(state);
            stored = Some(revision);
        }
    });
    outbox(pending).finish(stored, result.clone());
    result
}

/// The real file.
fn store_in_file(change: Apply<'_>) -> Result<(), String> {
    GuiState::update(|state| change(state)).map_err(|error| error.to_string())
}

/// Queue `preferences` for the file and start a write.
fn persist(preferences: Preferences, cx: &App) {
    // Tests share one config directory (and run in parallel): they must not
    // rewrite each other's saved preferences
    #[cfg(test)]
    let _ = (preferences, cx);
    #[cfg(not(test))]
    {
        outbox(&OUTBOX).submit(preferences);
        spawn_write(cx);
    }
}

#[cfg(not(test))]
fn spawn_write(cx: &App) {
    outbox(&OUTBOX).writing += 1;
    cx.background_spawn(async {
        let result = write_pending(&OUTBOX, store_in_file);
        outbox(&OUTBOX).writing -= 1;
        if let Err(error) = result {
            tracing::warn!("could not save the preferences: {error}");
        }
    })
    .detach();
}

/// Try again after a failed write. Called by the workspace's autosave tick.
pub(crate) fn retry_failed(cx: &App) {
    #[cfg(not(test))]
    if outbox(&OUTBOX).needs_retry() {
        spawn_write(cx);
    }
    #[cfg(test)]
    let _ = cx;
}

/// Why saving the preferences keeps failing, once per failure streak, for a
/// notification.
pub(crate) fn take_unreported_error() -> Option<String> {
    outbox(&OUTBOX).take_unreported_error()
}

/// Write what is still pending, now. Blocks on the file: call it from a
/// background thread (the coordinated quit does).
pub(crate) fn flush() -> Result<(), String> {
    if outbox(&OUTBOX).value.is_none() {
        return Ok(());
    }
    write_pending(&OUTBOX, store_in_file)
}

/// Font size of the SQL editor. Read it when building or rendering an editor
/// and re-apply it from an `observe_global::<Preferences>` callback.
pub fn editor_font_size(cx: &App) -> Pixels {
    px(Preferences::get(cx).editor_font_size)
}

/// What ends a statement in the query editor.
pub fn statement_separators(cx: &App) -> StatementSeparators {
    Preferences::get(cx).statement_separators
}

/// Whether unsaved query tabs are persisted and restored.
pub fn restore_drafts(cx: &App) -> bool {
    Preferences::get(cx).restore_drafts
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported through `super::*`) would shadow
    // the built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn defaults_match_the_state_defaults() {
        let preferences = Preferences::default();
        assert!(preferences.keep_running_in_tray);
        assert!(!preferences.hide_dock_icon_when_closed);
        assert!(!preferences.tray_notice_shown);
        assert!(preferences.restore_drafts);
        assert_eq!(preferences.editor_font_size, 13.0);
        assert_eq!(
            preferences.statement_separators,
            StatementSeparators::SemicolonsAndBlankLines
        );
    }

    #[test]
    fn apply_only_touches_its_own_fields() {
        let mut state = GuiState {
            sidebar_width: 321.0,
            theme_name: Some("Tokyo Night".into()),
            ..GuiState::default()
        };
        let preferences = Preferences {
            keep_running_in_tray: false,
            hide_dock_icon_when_closed: true,
            tray_notice_shown: true,
            restore_drafts: false,
            editor_font_size: 17.0,
            statement_separators: StatementSeparators::SemicolonsOnly,
        };
        preferences.apply(&mut state);
        assert_eq!(Preferences::from_state(&state), preferences);
        assert_eq!(state.sidebar_width, 321.0);
        assert_eq!(state.theme_name.as_deref(), Some("Tokyo Night"));
    }

    fn prefs(editor_font_size: f32) -> Preferences {
        Preferences {
            editor_font_size,
            ..Preferences::default()
        }
    }

    /// A file that can be made to fail.
    #[derive(Default)]
    struct FakeDisk {
        state: GuiState,
        writes: usize,
        failing: bool,
    }

    impl FakeDisk {
        fn store(&mut self) -> impl FnOnce(Apply<'_>) -> Result<(), String> + '_ {
            move |change| {
                self.writes += 1;
                if self.failing {
                    return Err("disk full".to_string());
                }
                change(&mut self.state);
                Ok(())
            }
        }
    }

    fn outbox_with(value: Preferences) -> Mutex<Outbox> {
        let mut outbox = Outbox::new();
        outbox.submit(value);
        Mutex::new(outbox)
    }

    #[test]
    fn a_successful_write_stores_the_newest_value_and_acknowledges_it() {
        let pending = outbox_with(prefs(14.0));
        // A second change arrives before the first write ran: only it is stored
        outbox(&pending).submit(Preferences {
            restore_drafts: false,
            ..prefs(16.0)
        });
        let mut disk = FakeDisk::default();
        assert_eq!(write_pending(&pending, disk.store()), Ok(()));
        assert_eq!(disk.state.editor_font_size, 16.0);
        assert!(!disk.state.restore_drafts);
        assert!(outbox(&pending).value.is_none());

        // A later write task finds nothing left and changes nothing
        let mut untouched = FakeDisk::default();
        assert_eq!(write_pending(&pending, untouched.store()), Ok(()));
        assert_eq!(untouched.state, GuiState::default());
    }

    #[test]
    fn a_failed_write_keeps_the_value_for_a_retry() {
        let pending = outbox_with(prefs(18.0));
        let mut disk = FakeDisk {
            failing: true,
            ..FakeDisk::default()
        };
        assert_eq!(
            write_pending(&pending, disk.store()),
            Err("disk full".to_string())
        );
        // Still pending, and a retry is due
        assert_eq!(
            outbox(&pending).current().map(|(_, v)| v),
            Some(prefs(18.0))
        );
        assert!(outbox(&pending).needs_retry());

        // The retry stores it and clears the failure
        disk.failing = false;
        assert_eq!(write_pending(&pending, disk.store()), Ok(()));
        assert_eq!(disk.state.editor_font_size, 18.0);
        assert!(outbox(&pending).value.is_none());
        assert!(outbox(&pending).failure.is_none());
        assert!(!outbox(&pending).needs_retry());
    }

    #[test]
    fn a_persistent_failure_is_reported_once_per_streak() {
        let pending = outbox_with(prefs(15.0));
        let mut disk = FakeDisk {
            failing: true,
            ..FakeDisk::default()
        };
        assert_eq!(
            outbox(&pending).take_unreported_error(),
            None,
            "nothing failed yet"
        );
        let _ = write_pending(&pending, disk.store());
        assert_eq!(
            outbox(&pending).take_unreported_error().as_deref(),
            Some("disk full")
        );
        // Still failing on the retries: no second notice
        let _ = write_pending(&pending, disk.store());
        let _ = write_pending(&pending, disk.store());
        assert_eq!(outbox(&pending).take_unreported_error(), None);
        assert_eq!(disk.writes, 3);

        // It recovers, then fails again: that is news again
        disk.failing = false;
        let _ = write_pending(&pending, disk.store());
        outbox(&pending).submit(prefs(17.0));
        disk.failing = true;
        let _ = write_pending(&pending, disk.store());
        assert_eq!(
            outbox(&pending).take_unreported_error().as_deref(),
            Some("disk full")
        );
    }

    #[test]
    fn a_change_made_during_a_write_is_not_lost() {
        let pending = outbox_with(prefs(14.0));
        let mut disk = FakeDisk::default();
        let result = write_pending(&pending, |change| {
            let mut state = GuiState::default();
            change(&mut state);
            disk.state = state;
            // The user changes a setting while the write is still in flight
            outbox(&pending).submit(prefs(20.0));
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(disk.state.editor_font_size, 14.0);
        // The newer value is still pending: it was not what the write stored
        assert_eq!(
            outbox(&pending).current().map(|(_, v)| v),
            Some(prefs(20.0))
        );
        assert_eq!(write_pending(&pending, disk.store()), Ok(()));
        assert_eq!(disk.state.editor_font_size, 20.0);
        assert!(outbox(&pending).value.is_none());
    }

    #[test]
    fn a_late_success_of_an_older_value_does_not_hide_a_newer_failure() {
        let pending = outbox_with(prefs(14.0)); // revision 1
        let mut disk = FakeDisk::default();
        let mut failing_disk = FakeDisk {
            failing: true,
            ..FakeDisk::default()
        };
        // Write A stores revision 1 and releases the file lock; before it
        // reports back, the user changes the setting (revision 2) and write
        // B runs to completion — and fails. Only then does A's success land.
        let result = write_pending(&pending, |change| {
            let mut state = GuiState::default();
            change(&mut state);
            disk.state = state;
            outbox(&pending).submit(prefs(16.0));
            assert!(write_pending(&pending, failing_disk.store()).is_err());
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(disk.state.editor_font_size, 14.0);

        // Revision 2 is still pending, its failure is still known, and the
        // autosave tick will retry it
        let state = outbox(&pending);
        assert_eq!(state.current().map(|(_, v)| v), Some(prefs(16.0)));
        assert_eq!(state.failure.as_ref().map(|f| f.revision), Some(2));
        assert!(state.needs_retry());
        drop(state);

        // The retry stores revision 2 and only now is everything clean
        assert_eq!(write_pending(&pending, disk.store()), Ok(()));
        assert_eq!(disk.state.editor_font_size, 16.0);
        let state = outbox(&pending);
        assert!(state.value.is_none());
        assert!(state.failure.is_none());
        assert!(!state.needs_retry());
    }

    #[test]
    fn failures_are_tracked_per_revision() {
        let mut outbox = Outbox::new();
        outbox.submit(prefs(14.0)); // 1
        outbox.submit(prefs(15.0)); // 2
        // Revision 2 failed after storing nothing, then a late success of 1
        outbox.finish(None, Err("disk full".into()));
        outbox.finish(Some(1), Ok(()));
        assert!(outbox.needs_retry(), "the failure is about revision 2");
        // A success of a newer value than the failure's clears it
        outbox.submit(prefs(16.0)); // 3
        outbox.finish(Some(3), Ok(()));
        assert!(outbox.failure.is_none());
        assert!(outbox.value.is_none());
    }

    #[test]
    fn a_newer_failure_in_the_same_streak_is_not_reported_again() {
        let mut outbox = Outbox::new();
        outbox.submit(prefs(14.0));
        outbox.finish(Some(1), Err("disk full".into()));
        assert_eq!(outbox.take_unreported_error().as_deref(), Some("disk full"));
        outbox.submit(prefs(15.0));
        outbox.finish(Some(2), Err("disk full".into()));
        assert_eq!(outbox.take_unreported_error(), None);
        assert_eq!(outbox.failure.as_ref().map(|f| f.revision), Some(2));
    }

    #[test]
    fn retries_wait_for_a_write_that_is_already_running() {
        let mut outbox = Outbox::new();
        outbox.submit(prefs(14.0));
        outbox.finish(Some(1), Err("busy".into()));
        assert!(outbox.needs_retry());
        outbox.writing = 1;
        assert!(!outbox.needs_retry());
    }

    #[test]
    fn flushing_with_nothing_pending_writes_nothing() {
        // The shared outbox is never filled in tests
        assert_eq!(flush(), Ok(()));
    }

    #[gpui_kit::test]
    fn updates_apply_at_once_and_notify_only_real_changes(cx: &mut gpui_kit::TestAppContext) {
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            Preferences::init(&GuiState::default(), cx);
            let changes = changes.clone();
            cx.observe_global::<Preferences>(move |_| changes.set(changes.get() + 1))
                .detach();
        });
        cx.update(|cx| {
            Preferences::update(cx, |p| p.editor_font_size = 17.0);
            assert_eq!(editor_font_size(cx), px(17.0));
        });
        // Same value again: nothing changed
        cx.update(|cx| Preferences::update(cx, |p| p.editor_font_size = 17.0));
        cx.update(|cx| {
            Preferences::update(cx, |p| p.restore_drafts = false);
            assert!(!restore_drafts(cx));
        });
        // Out of range: clamped
        cx.update(|cx| {
            Preferences::update(cx, |p| p.editor_font_size = 500.0);
            assert_eq!(
                Preferences::get(cx).editor_font_size,
                GuiState::MAX_EDITOR_FONT_SIZE
            );
        });
        cx.run_until_parked();
        // 17 px, restore off, 24 px: the repeated write changed nothing
        assert_eq!(changes.get(), 3);
    }

    #[test]
    fn font_size_is_kept_in_range() {
        let preferences = Preferences {
            editor_font_size: 400.0,
            ..Preferences::default()
        };
        let mut state = GuiState::default();
        preferences.apply(&mut state);
        assert_eq!(state.editor_font_size, GuiState::MAX_EDITOR_FONT_SIZE);
        state.editor_font_size = 1.0;
        assert_eq!(
            Preferences::from_state(&state).editor_font_size,
            GuiState::MIN_EDITOR_FONT_SIZE
        );
    }
}
