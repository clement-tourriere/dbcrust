//! Connection-option keys DBCrust itself interprets (vault, password command,
//! per-connection hardening) and the pure helpers that classify them.
//!
//! A leaf module: `config`, `password_sanitizer`, `database` and the drivers all
//! need these keys, so they must not live in any of those. Re-exported from
//! `config` and `database` under their historical paths.

use std::collections::HashMap;

pub const VAULT_OPTION_KEYS: [&str; 3] = ["vault_mount", "vault_database", "vault_role"];
pub const PASSWORD_COMMAND_OPTION_KEYS: [&str; 3] =
    ["password_command", "password_cmd", "password-command"];

pub fn is_vault_option_key(key: &str) -> bool {
    VAULT_OPTION_KEYS.contains(&key)
}

/// Return true when a URL/session option is one of DBCrust's password command keys.
pub fn is_password_command_option_key(key: &str) -> bool {
    PASSWORD_COMMAND_OPTION_KEYS
        .iter()
        .any(|option_key| key.eq_ignore_ascii_case(option_key))
}

/// Return true for DBCrust-only options that must not be forwarded to database drivers.
pub fn is_dbcrust_internal_connection_option(key: &str) -> bool {
    is_vault_option_key(key)
        || is_password_command_option_key(key)
        || key.eq_ignore_ascii_case(OPT_HARDEN_READ_ONLY)
        || key.eq_ignore_ascii_case(OPT_HARDEN_TIMEOUT_SECS)
}

/// Per-connection hardening request carried in `ConnectionInfo::options`.
///
/// The process-wide [`crate::database::read_only_requested`] flag cannot serve the credential
/// broker, which hosts aliases with different access levels in one process.
/// Only the broker gateway sets these keys (the connection is built from the
/// stored record, never from request data); the CLI never forwards them to a
/// driver (see [`is_dbcrust_internal_connection_option`]).
pub const OPT_HARDEN_READ_ONLY: &str = "dbcrust_harden_read_only";
/// Server-side statement timeout in seconds for this connection.
pub const OPT_HARDEN_TIMEOUT_SECS: &str = "dbcrust_harden_timeout_secs";

/// The per-connection server-side statement timeout, when requested.
pub fn connection_timeout_secs(options: &HashMap<String, String>) -> Option<u64> {
    options
        .get(OPT_HARDEN_TIMEOUT_SECS)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
}

/// True for connections built by the credential broker: no `.pgpass`, no
/// environment-derived defaults beyond what the record provides.
pub fn is_hardened_connection(options: &HashMap<String, String>) -> bool {
    options.contains_key(OPT_HARDEN_READ_ONLY) || options.contains_key(OPT_HARDEN_TIMEOUT_SECS)
}
