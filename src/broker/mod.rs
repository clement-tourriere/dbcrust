//! The credential broker: `dbcrust broker serve|enroll|list|revoke|policy|status`.
//!
//! The broker owns every database credential an AI agent may use. Agents
//! never see them: they speak the length-prefixed JSON protocol of
//! `dbcrust-agent-protocol` (through the thin `dbcrust-agent` client) to a
//! Unix socket, naming only *aliases* the owner enrolled.
//!
//! ```text
//! dbcrust-agent ──► Unix socket ──► server ──► gateway ──► database
//!   (no keystore,     peer uid/gid     │          │ resolve alias → SecretRecord
//!    no drivers)      checked          │          │ connect from the record only
//!                                      │          │ classify · limit · audit
//!                                      └─ audit.jsonl (fail closed)
//! ```
//!
//! Modules:
//! * [`paths`] — modes (`user` / `service`), state-dir layout, permission rules;
//! * [`registry`] — `aliases.toml` descriptors and the authoritative records;
//! * [`admin`] — enrollment (URL validation, test connect), revoke, tighten;
//! * [`gateway`] — the single execution path (classify → limits → audit → run);
//! * [`audit`] — `audit.jsonl` with rotation;
//! * [`server`] — the socket server and handshake;
//! * [`hardening`] — umask, core dumps, debugger denial, environment scrub;
//! * [`privileges`] — the trusted-binary rule and verified privilege drops;
//! * [`service`] — `install-service` / `uninstall-service` (launchd, systemd);
//! * [`cli`] — the command-line front end.
//!
//! Unix only: on Windows the subcommands compile and report that the broker
//! is not supported yet.

use crate::secrets::SecretError;
use thiserror::Error;

/// Why a broker operation failed. Messages never contain credentials.
#[derive(Debug, Error)]
pub enum BrokerError {
    /// Bad input: alias, URL, limits.
    #[error("{0}")]
    Invalid(String),
    /// A rule forbids it (password in URL, loosening policy, …).
    #[error("{0}")]
    Forbidden(String),
    /// The engine is not available through the broker.
    #[error("unsupported_engine: {0}")]
    UnsupportedEngine(String),
    /// The test connection or a probe failed.
    #[error("connection failed: {0}")]
    Connect(String),
    /// Needs a terminal.
    #[error("{0}")]
    Interactive(String),
    /// Broker configuration (`broker.toml`) problem.
    #[error("{0}")]
    Config(String),
    /// State directory / files problem.
    #[error("{0}")]
    State(String),
    #[error(transparent)]
    Secret(#[from] SecretError),
}

impl BrokerError {
    /// Process exit code: 2 for "cannot be done as asked", 1 for failures.
    pub fn exit_code(&self) -> i32 {
        match self {
            BrokerError::Invalid(_)
            | BrokerError::Forbidden(_)
            | BrokerError::UnsupportedEngine(_)
            | BrokerError::Interactive(_) => 2,
            _ => 1,
        }
    }
}

#[cfg(unix)]
pub mod admin;
#[cfg(unix)]
pub mod audit;
#[cfg(unix)]
pub mod cli;
#[cfg(unix)]
pub mod gateway;
#[cfg(unix)]
pub mod hardening;
#[cfg(unix)]
pub mod paths;
#[cfg(unix)]
pub mod privileges;
#[cfg(unix)]
pub mod registry;
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod service;

#[cfg(unix)]
pub use cli::run;

/// Windows: the broker needs Unix sockets and peer credentials (a named-pipe
/// broker is planned). The subcommands exist so the CLI is the same everywhere.
#[cfg(not(unix))]
pub async fn run(
    _action: Option<&crate::cli::BrokerAction>,
    _root_format: Option<crate::cli::OutputFormat>,
    _interactive: bool,
) -> i32 {
    eprintln!("dbcrust broker is not supported on Windows yet (it needs Unix domain sockets).");
    2
}

#[cfg(all(test, unix))]
mod e2e_tests;
#[cfg(all(test, unix))]
pub(crate) mod testkit;
