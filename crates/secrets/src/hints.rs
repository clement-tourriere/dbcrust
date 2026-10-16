//! One-time, stderr-only notices about the secret store.
//!
//! Both fire at most once per run. They are advisory: they never change what
//! a lookup returns.

use crate::{SecretError, is_interactive};
use std::sync::atomic::{AtomicBool, Ordering};

static STORE_NOTICE_SHOWN: AtomicBool = AtomicBool::new(false);
static LEGACY_HINT_SHOWN: AtomicBool = AtomicBool::new(false);

/// Tell the user (once per run, on stderr) why the store could not answer.
pub fn warn_store_problem(error: &SecretError) {
    if !STORE_NOTICE_SHOWN.swap(true, Ordering::Relaxed) {
        eprintln!("Saved credentials are not available: {error}");
    }
}

/// Suggest `dbcrust secrets migrate` once per run, interactive sessions only
/// (stderr; never in one-shot output: the CLI turns [`is_interactive`] off
/// for one-shot runs).
pub fn hint_legacy_secret_used() {
    if !is_interactive() {
        return;
    }
    if !LEGACY_HINT_SHOWN.swap(true, Ordering::Relaxed) {
        eprintln!(
            "Note: a password came from a legacy file (~/.dbcrust, ai_keys.enc or the old keychain entry). \
             Run `dbcrust secrets migrate` to move it into the secure store."
        );
    }
}
