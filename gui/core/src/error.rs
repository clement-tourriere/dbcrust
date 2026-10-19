//! Error type crossing the worker → UI boundary. Always `Send + Clone` so a
//! result can be shown, retained in tab state, and re-rendered freely.

/// Everything the desktop app can report to the user. Messages are
/// user-facing: they are rendered verbatim in alerts and notifications.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuiError {
    /// Connecting failed (bad URL, unreachable host, auth, tunnel, …).
    #[error("{}", dbcrust_core::password_sanitizer::redact_text(.0))]
    Connection(String),
    /// The database rejected or failed the statement.
    #[error("{}", dbcrust_core::password_sanitizer::redact_text(.0))]
    Query(String),
    /// The read-only guard refused a statement classified as a write.
    #[error("{0}")]
    ReadOnly(String),
    /// The user cancelled the running statement. `abandoned` is true when the
    /// engine has no server-side cancel: the client stopped waiting, but the
    /// server may still finish the statement.
    #[error("{}", if *abandoned {
        "Stopped waiting — the server may still finish the statement"
    } else {
        "Statement cancelled"
    })]
    Cancelled { abandoned: bool },
    /// The connection's worker is gone (closed, or crashed).
    #[error("This connection is closed")]
    Disconnected,
    /// Reading or writing DBCrust's config/storage files failed.
    #[error("{}", dbcrust_core::password_sanitizer::redact_text(.0))]
    Storage(String),
    /// Docker discovery failed.
    #[error("{}", dbcrust_core::password_sanitizer::redact_text(.0))]
    Docker(String),
    /// Vault discovery or credential issuance failed.
    #[error("{}", dbcrust_core::password_sanitizer::redact_text(.0))]
    Vault(String),
    /// The operation is not available for this database engine.
    #[error("{0}")]
    Unsupported(String),
}

impl GuiError {
    /// True for a user-initiated cancellation (rendered as a notice, not an
    /// error).
    pub fn is_cancelled(&self) -> bool {
        matches!(self, GuiError::Cancelled { .. })
    }
}

pub type GuiResult<T> = Result<T, GuiError>;
