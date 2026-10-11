//! The one writer of the workspace's persisted state.
//!
//! Saves used to be fire-and-forget background tasks: an older write could
//! finish after a newer one (or after the final save on quit), a failed write
//! was recorded as done, and quitting never learned whether the last save
//! worked. [`OrderedWriter`] fixes all three:
//!
//! - **One thread writes, in submission order.** `submit` only replaces the
//!   value waiting in a slot (latest wins), so a slow disk never builds a
//!   backlog and an older value can never overwrite a newer one.
//! - **"Saved" means written.** The value is kept as the saved one only once
//!   the write returned `Ok`; a failed value stays available for
//!   [`OrderedWriter::retry_failed`] (the next autosave tick) unless
//!   something newer was submitted meanwhile.
//! - **Callers can wait.** [`Flusher::wait`] blocks (on a background thread)
//!   until everything submitted has been written or has failed, with a
//!   timeout, so quitting can refuse to lose data silently.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

struct State<T> {
    /// Newest value waiting to be written.
    pending: Option<T>,
    /// A write is in progress.
    writing: bool,
    /// The value the newest failed write could not store, until something
    /// newer is submitted or the retry is requested.
    retry: Option<T>,
    /// The newest value known to be in the file.
    saved: Option<T>,
    /// Why the newest write failed; cleared by the next success.
    error: Option<String>,
    /// `error` was handed out by [`OrderedWriter::take_unreported_error`].
    error_reported: bool,
    /// The writer thread could not be started.
    dead: bool,
    closed: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How a [`Flusher::wait`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FlushOutcome {
    /// Everything submitted is on disk.
    Saved,
    /// The newest write failed.
    Failed(String),
    /// A write is still running after the timeout.
    TimedOut,
}

/// Writes submitted values with `write`, one at a time, in order.
pub(super) struct OrderedWriter<T> {
    shared: Arc<Shared<T>>,
}

impl<T: Send + 'static> OrderedWriter<T> {
    /// `write` stores one value and reports why it could not.
    pub(super) fn new(write: impl FnMut(&T) -> Result<(), String> + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pending: None,
                writing: false,
                retry: None,
                saved: None,
                error: None,
                error_reported: false,
                dead: false,
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("dbcrust-gui-state-writer".into())
            .spawn(move || run(&worker, write));
        if let Err(error) = spawned {
            let mut state = shared.lock();
            state.dead = true;
            state.error = Some(format!("Could not start the writer thread: {error}"));
        }
        Self { shared }
    }

    /// Queue `value`: it replaces any value still waiting, and drops a
    /// failed one it supersedes.
    pub(super) fn submit(&self, value: T) {
        let mut state = self.shared.lock();
        state.retry = None;
        if state.dead {
            state.retry = Some(value);
            return;
        }
        state.pending = Some(value);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// Queue the value of the newest failed write again (no-op when the last
    /// write succeeded or something newer is already waiting).
    pub(super) fn retry_failed(&self) {
        let mut state = self.shared.lock();
        if state.dead || state.pending.is_some() {
            return;
        }
        let Some(value) = state.retry.take() else {
            return;
        };
        state.pending = Some(value);
        drop(state);
        self.shared.changed.notify_all();
    }

    /// The error of the newest failed write, once per failure streak.
    pub(super) fn take_unreported_error(&self) -> Option<String> {
        let mut state = self.shared.lock();
        if state.error_reported {
            return None;
        }
        let error = state.error.clone()?;
        state.error_reported = true;
        Some(error)
    }

    /// A handle to wait for the queue to drain.
    pub(super) fn flusher(&self) -> Flusher<T> {
        Flusher {
            shared: self.shared.clone(),
        }
    }
}

impl<T: PartialEq> OrderedWriter<T> {
    /// True when the file holds exactly `value` and nothing is queued,
    /// running or waiting for a retry.
    pub(super) fn is_saved(&self, value: &T) -> bool {
        let state = self.shared.lock();
        !state.writing
            && state.pending.is_none()
            && state.retry.is_none()
            && state.saved.as_ref() == Some(value)
    }
}

impl<T> Drop for OrderedWriter<T> {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_all();
    }
}

/// Waits for an [`OrderedWriter`] to drain. `Send`, so it can block on a
/// background thread.
pub(super) struct Flusher<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Flusher<T> {
    /// Block until nothing is queued or running (at most `timeout`).
    pub(super) fn wait(&self, timeout: Duration) -> FlushOutcome {
        let state = self.shared.lock();
        let (state, result) = self
            .shared
            .changed
            // Closing is not done: the writer drains what is queued first
            // (a system quit drops the owner before awaiting the final save)
            .wait_timeout_while(state, timeout, |state| {
                !state.dead && (state.writing || state.pending.is_some())
            })
            .unwrap_or_else(PoisonError::into_inner);
        if result.timed_out() {
            return FlushOutcome::TimedOut;
        }
        if state.retry.is_some() || state.dead || state.pending.is_some() || state.writing {
            let message = state
                .error
                .clone()
                .unwrap_or_else(|| "The state file could not be written.".to_string());
            FlushOutcome::Failed(message)
        } else {
            FlushOutcome::Saved
        }
    }
}

/// The writer thread: take the newest pending value, write it, record the
/// outcome, repeat until the owner is dropped.
fn run<T>(shared: &Shared<T>, mut write: impl FnMut(&T) -> Result<(), String>) {
    let mut state = shared.lock();
    loop {
        // Owner gone: finish what is queued, then stop
        if state.closed && state.pending.is_none() {
            return;
        }
        let Some(value) = state.pending.take() else {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        state.writing = true;
        drop(state);

        let result = catch_unwind(AssertUnwindSafe(|| write(&value)))
            .unwrap_or_else(|_| Err("The writer stopped unexpectedly.".to_string()));

        state = shared.lock();
        state.writing = false;
        match result {
            Ok(()) => {
                state.saved = Some(value);
                state.retry = None;
                state.error = None;
                state.error_reported = false;
            }
            Err(message) => {
                if state.error.is_none() {
                    state.error_reported = false;
                }
                state.error = Some(message);
                // A newer value is already waiting: it supersedes this one
                if state.pending.is_none() {
                    state.retry = Some(value);
                }
            }
        }
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    // GPUI's `test` macro (glob-imported elsewhere) would shadow the
    // built-in `#[test]`
    use std::prelude::v1::test;

    use super::*;
    use std::sync::mpsc;

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_value_queued_before_the_owner_is_dropped_is_still_written() {
        let (writer, log, _) = recording_writer();
        let flusher = writer.flusher();
        writer.submit(7);
        drop(writer); // what GPUI's shutdown does before awaiting quit hooks
        assert_eq!(flusher.wait(WAIT), FlushOutcome::Saved);
        assert_eq!(log.lock().unwrap().last(), Some(&7));
    }

    /// A writer whose writes are logged, and which fails while `failing` is set.
    fn recording_writer() -> (
        OrderedWriter<u32>,
        Arc<Mutex<Vec<u32>>>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let failing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = OrderedWriter::new({
            let (log, failing) = (log.clone(), failing.clone());
            move |value: &u32| {
                if failing.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(format!("disk full at {value}"));
                }
                log.lock().unwrap().push(*value);
                Ok(())
            }
        });
        (writer, log, failing)
    }

    #[test]
    fn writes_in_order_and_confirms_the_newest() {
        let (writer, log, _) = recording_writer();
        writer.submit(1);
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);
        writer.submit(2);
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);
        assert_eq!(*log.lock().unwrap(), [1, 2]);
        assert!(writer.is_saved(&2));
        assert!(!writer.is_saved(&1));
    }

    #[test]
    fn a_slow_write_never_lets_an_older_value_win() {
        // The first write blocks until released; 2..=4 queue up behind it
        let (release, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel::<()>();
        let log = Arc::new(Mutex::new(Vec::new()));
        let writer = OrderedWriter::new({
            let log = log.clone();
            let mut first = true;
            move |value: &u32| {
                if std::mem::take(&mut first) {
                    started_tx.send(()).ok();
                    gate.recv().ok();
                }
                log.lock().unwrap().push(*value);
                Ok(())
            }
        });
        writer.submit(1);
        started.recv_timeout(WAIT).expect("first write started");
        for value in 2..=4 {
            writer.submit(value);
        }
        // Still blocked: a bounded wait gives up
        assert_eq!(
            writer.flusher().wait(Duration::from_millis(50)),
            FlushOutcome::TimedOut
        );
        assert!(!writer.is_saved(&4));
        release.send(()).expect("release");
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);
        // 2 and 3 were superseded; the final value is the last one written
        assert_eq!(*log.lock().unwrap(), [1, 4]);
        assert!(writer.is_saved(&4));
    }

    #[test]
    fn a_failed_write_is_not_saved_and_is_retried() {
        let (writer, log, failing) = recording_writer();
        writer.submit(1);
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);

        failing.store(true, std::sync::atomic::Ordering::SeqCst);
        writer.submit(2);
        assert_eq!(
            writer.flusher().wait(WAIT),
            FlushOutcome::Failed("disk full at 2".into())
        );
        // 2 is not on disk, and the failure is reported once
        assert!(!writer.is_saved(&2));
        assert!(!writer.is_saved(&1));
        assert_eq!(
            writer.take_unreported_error().as_deref(),
            Some("disk full at 2")
        );
        assert_eq!(writer.take_unreported_error(), None);

        // Still failing: the retry fails again, without a second report
        writer.retry_failed();
        assert!(matches!(
            writer.flusher().wait(WAIT),
            FlushOutcome::Failed(_)
        ));
        assert_eq!(writer.take_unreported_error(), None);

        failing.store(false, std::sync::atomic::Ordering::SeqCst);
        writer.retry_failed();
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);
        assert!(writer.is_saved(&2));
        assert_eq!(*log.lock().unwrap(), [1, 2]);

        // A new failure streak is reported again
        failing.store(true, std::sync::atomic::Ordering::SeqCst);
        writer.submit(3);
        assert!(matches!(
            writer.flusher().wait(WAIT),
            FlushOutcome::Failed(_)
        ));
        assert_eq!(
            writer.take_unreported_error().as_deref(),
            Some("disk full at 3")
        );
    }

    #[test]
    fn a_newer_value_supersedes_a_failed_one() {
        let (writer, log, failing) = recording_writer();
        failing.store(true, std::sync::atomic::Ordering::SeqCst);
        writer.submit(1);
        assert!(matches!(
            writer.flusher().wait(WAIT),
            FlushOutcome::Failed(_)
        ));

        failing.store(false, std::sync::atomic::Ordering::SeqCst);
        writer.submit(2);
        // The retry slot was dropped: retrying must not resurrect 1
        writer.retry_failed();
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);
        assert_eq!(*log.lock().unwrap(), [2]);
        assert!(writer.is_saved(&2));
    }

    #[test]
    fn nothing_queued_means_nothing_to_wait_for() {
        let writer = OrderedWriter::new(|_: &u32| Ok(()));
        assert!(!writer.is_saved(&7));
        assert_eq!(writer.flusher().wait(WAIT), FlushOutcome::Saved);
    }

    #[test]
    fn a_panicking_write_is_a_failure_not_a_hang() {
        let writer = OrderedWriter::new(|_: &u32| -> Result<(), String> {
            panic!("boom");
        });
        writer.submit(1);
        assert!(matches!(
            writer.flusher().wait(WAIT),
            FlushOutcome::Failed(_)
        ));
        assert!(!writer.is_saved(&1));
    }
}
