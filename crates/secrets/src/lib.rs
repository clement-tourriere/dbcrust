//! Secret storage for DBCrust: OS keychain or an Argon2id passphrase file.
//!
//! This module owns every secret DBCrust persists (connection passwords, AI
//! API keys, OAuth tokens, and — later — broker credentials). The rules:
//!
//! * No plaintext at rest and **no machine-derived keys**, ever. The
//!   encrypted-file backend derives its key from a passphrase the user types
//!   (Argon2id) or from a random key file with strict permissions.
//! * No silent downgrade: a locked or unavailable backend is a typed error
//!   with an actionable hint, never a fallback to a weaker store.
//! * Non-interactive contexts (one-shot `-c/-f`, `--no-input`, no TTY, the
//!   desktop app) never prompt: they get [`SecretError::Locked`].
//! * Secret values live in [`zeroize::Zeroizing`] buffers and never implement
//!   `Display`/`Serialize`; `Debug` redacts.
//!
//! The crate deliberately has no dependency on the CLI, the REPL, the
//! config layer or any database driver. Where DBCrust's files live (and
//! whether the process is a test) is decided by [`paths`]; [`hints`] holds the
//! one-time stderr notices; [`legacy`] holds the read-only readers of the
//! formats DBCrust used before the store existed.
//!
//! # Features
//!
//! * `prompt` — [`PromptPassphrase`] asks for the Argon2id passphrase with
//!   `inquire`. Without it the same type reports [`SecretError::Locked`], so
//!   a crate that only needs the store (the credential broker) links no
//!   terminal-prompt stack. The `dbcrust` binary enables it.
//! * `test-mode` — forces [`paths::is_test_mode`] on in every build of this
//!   crate. `cfg!(test)` is only true for this crate's *own* unit tests, so a
//!   crate whose tests reach config or secrets enables it from its
//!   `[dev-dependencies]` (see [`paths`]).
//! * `test-support` — exposes the [`testing`] doubles to other crates' tests.
//!
//! # Entry points
//!
//! * [`SecretStore`] — the facade (`get_connection_secret`,
//!   `put_connection_secret`, generic `get_record`/`put_record` for
//!   `broker:<alias>` ids, `get_secret`/`put_secret` for `ai:`/`oauth:` ids).
//! * [`PassphraseFileBackend`] + [`KeySource`] — open an encrypted store file
//!   at an arbitrary location (the credential broker's service mode).
//! * [`SecretRecord`], [`TargetBinding`], [`AgentPolicy`] — the record model.

mod default;
mod file;
/// Private-file helpers shared with the broker and the legacy readers: an
/// implementation seam, not part of the supported API.
#[doc(hidden)]
pub mod fsutil;
pub mod hints;
mod index;
mod keychain;
pub mod legacy;
mod memory;
pub mod paths;
mod record;
mod store;

/// Test doubles; compiled for this crate's tests and, through the
/// `test-support` feature, for other crates' tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod testing;

pub use default::{default_store, reset_default_store, with_store_override};
pub use file::{
    FixedPassphrase, KdfParams, KeySource, PassphraseFileBackend, PassphraseProvider,
    PromptPassphrase, SECRETS_FILE_NAME, UnlockCache,
};
pub use index::{IndexEntry, IndexFile, SECRETS_INDEX_FILE_NAME, SecretsIndex};
pub use keychain::KeychainBackend;
pub use memory::MemoryBackend;
pub use record::{
    AgentAccess, AgentPolicy, PolicyRequest, RecordId, RecordKind, SecretRecord, TargetBinding,
    default_port_for_engine, normalize_engine,
};
pub use store::{
    BackendChoice, IndexDrift, RecordCounts, SecretStore, SecretsConfig, StoreSettings, StoreStatus,
};

use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use zeroize::Zeroizing;

/// Hint appended to [`SecretError::Locked`] messages.
pub const UNLOCK_HINT: &str = "run \"dbcrust secrets unlock\"";
/// Hint appended to [`SecretError::Unavailable`] messages.
pub const STATUS_HINT: &str = "run \"dbcrust secrets status\"";

/// Failure of the secret store. Messages never contain secret values.
#[derive(Debug, Error)]
pub enum SecretError {
    /// No usable backend (no keychain, keyutils missing, store dir unusable).
    #[error("secret storage is unavailable: {0} ({STATUS_HINT} for details)")]
    Unavailable(String),
    /// The store needs unlocking and cannot prompt here.
    #[error("secret storage is locked: {0} ({UNLOCK_HINT} in a terminal)")]
    Locked(String),
    /// The backend refused access, or the passphrase/key was wrong.
    #[error("access to secret storage was denied: {0}")]
    Denied(String),
    /// No such record. The payload is the record id, never a value.
    #[error("no stored secret with id {0}")]
    NotFound(String),
    /// The stored bytes failed their integrity check or could not be parsed.
    #[error("stored secret data is corrupt: {0}")]
    Corrupt(String),
    /// A record exists but is bound to a different target than requested.
    #[error("stored secret is bound to a different target ({0})")]
    TargetMismatch(String),
    /// Reading or writing the store failed.
    #[error("secret storage I/O error: {0}")]
    Io(String),
}

impl SecretError {
    /// True for failures a user can fix by unlocking the store.
    pub fn is_locked(&self) -> bool {
        matches!(self, SecretError::Locked(_))
    }
}

impl From<std::io::Error> for SecretError {
    fn from(error: std::io::Error) -> Self {
        SecretError::Io(error.to_string())
    }
}

/// Which storage mechanism a backend implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// macOS Keychain / Windows Credential Manager through `keyring`.
    Keychain,
    /// Passphrase- or key-file-encrypted `secrets.enc`.
    EncryptedFile,
    /// Process memory (tests, ephemeral tooling).
    Memory,
}

impl BackendKind {
    /// Short label for status output.
    pub fn label(&self) -> &'static str {
        match self {
            BackendKind::Keychain => "OS keychain",
            BackendKind::EncryptedFile => "encrypted file (Argon2id passphrase)",
            BackendKind::Memory => "in-memory (not persistent)",
        }
    }
}

/// Availability of a backend right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendState {
    /// Reads and writes work.
    Ready,
    /// Exists but needs unlocking.
    Locked,
    /// Cannot be used at all.
    Unavailable,
    /// Usable, but nothing has been stored yet (the encrypted file is created
    /// with a passphrase on the first save).
    Empty,
}

/// Snapshot of a backend's health. Never contains secret values.
#[derive(Debug, Clone)]
pub struct BackendStatus {
    pub kind: BackendKind,
    pub state: BackendState,
    /// Human-readable detail (path, reason), safe to print.
    pub detail: String,
}

/// A place secret values are stored under opaque string ids.
///
/// Implementations must be safe to share across threads and must never log
/// or format values. `get` returns `Ok(None)` for a missing id and an error
/// for every other failure; callers must not treat an error as "missing".
pub trait SecretBackend: Send + Sync {
    /// Which mechanism this is.
    fn kind(&self) -> BackendKind;

    /// Read one value.
    fn get(&self, id: &str) -> Result<Option<Zeroizing<String>>, SecretError>;

    /// Create or replace one value.
    fn put(&self, id: &str, value: &str) -> Result<(), SecretError>;

    /// Remove one value; `Ok(false)` when it did not exist.
    fn delete(&self, id: &str) -> Result<bool, SecretError>;

    /// Current health, without prompting.
    fn status(&self) -> BackendStatus;

    /// Every stored id, when the backend can enumerate (`None` otherwise —
    /// keychains cannot, so the non-secret index is the inventory there).
    fn list_ids(&self) -> Result<Option<Vec<String>>, SecretError> {
        Ok(None)
    }

    /// Unlock interactively and cache the key (encrypted file only).
    fn unlock(&self) -> Result<(), SecretError> {
        Err(SecretError::Unavailable(format!(
            "the {} backend has no lock to open",
            self.kind().label()
        )))
    }

    /// Forget any cached unlock key (encrypted file only).
    fn lock(&self) -> Result<(), SecretError> {
        Err(SecretError::Unavailable(format!(
            "the {} backend has no lock to close",
            self.kind().label()
        )))
    }
}

/// `None` = no explicit override: prompts depend on the TTY check alone.
static PROMPTS_DISABLED: AtomicBool = AtomicBool::new(false);

/// Globally allow or forbid secret prompts. The CLI mirrors `--no-input` and
/// one-shot mode here; the desktop app disables prompts at startup.
pub fn set_interactive(allowed: bool) {
    PROMPTS_DISABLED.store(!allowed, Ordering::Relaxed);
}

/// Whether the store may prompt right now: not disabled and stdin is a TTY.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal;
    !PROMPTS_DISABLED.load(Ordering::Relaxed) && std::io::stdin().is_terminal()
}
