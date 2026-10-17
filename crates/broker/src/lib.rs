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
//! * [`service`] — `install-service` / `uninstall-service` plans, generators
//!   and executor (launchd, systemd);
//!
//! # Crate boundary
//!
//! This crate is the broker *core*: the credential-holding process. It depends
//! on `dbcrust-core`, `dbcrust-secrets` (without its passphrase prompt),
//! `dbcrust-drivers` and `dbcrust-agent-protocol` only — never on the root
//! crate, the CLI grammar, `inquire`, `reedline`, the AI stack or Docker.
//! Anything that needs the person at the keyboard is a parameter:
//! [`admin::Admin::enroll`] takes the password as a `Zeroizing<String>`,
//! `service::install`/`uninstall` take a `service::ServicePrompts`. The
//! front end (`dbcrust broker …`: argument types, prompts, exit codes) lives in
//! the root crate's `src/broker/`, which re-exports this crate as
//! `dbcrust::broker`. `boundary_tests` below keeps the manifest honest.
//!
//! Unix only: the modules are `#[cfg(unix)]`; the root crate reports that the
//! broker is not supported on Windows.

// The connection task's future (server.rs) nests the drivers' futures; with
// CloudWatch Logs as the only engine, the AWS SDK's nesting takes rustc's
// layout query past the default 128 ("queries overflow the depth limit").
#![recursion_limit = "256"]

use dbcrust_secrets::SecretError;
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

/// Fixtures (temp state dir, in-memory store, seeded SQLite database,
/// gateway) shared with the root crate's end-to-end tests through the
/// `test-support` feature.
#[cfg(all(unix, any(test, feature = "test-support")))]
#[doc(hidden)]
pub mod testkit;

#[cfg(test)]
mod boundary_tests {
    /// Crates the broker core must never depend on: the front end (`inquire`
    /// prompts, `usage` grammar, `reedline`), the root crate, and everything
    /// the credential-holding process has no business linking.
    const FORBIDDEN: &[&str] = &[
        "dbcrust ",
        "dbcrust=",
        "dbcrust-ai",
        "dbcrust-docker",
        "dbcrust-agent ",
        "inquire",
        "reedline",
        "usage",
        "ratatui",
        "crossterm",
        "genai",
        "bollard",
        "pyo3",
        "arboard",
    ];

    #[test]
    fn manifest_has_no_front_end_dependencies() {
        let manifest = include_str!("../Cargo.toml");
        for line in manifest.lines() {
            let line = line.trim_start();
            if line.starts_with('#') {
                continue;
            }
            for token in FORBIDDEN {
                assert!(
                    !line.starts_with(token),
                    "dbcrust-broker must not depend on `{}` (found `{line}`); the broker core \
                     stays free of the front end, see the crate docs",
                    token.trim()
                );
            }
        }
    }

    /// Every source file of the core: none may name the root crate's modules
    /// or prompt the user.
    const SOURCES: &[(&str, &str)] = &[
        ("admin.rs", include_str!("admin.rs")),
        ("audit.rs", include_str!("audit.rs")),
        ("gateway.rs", include_str!("gateway.rs")),
        ("hardening.rs", include_str!("hardening.rs")),
        ("paths.rs", include_str!("paths.rs")),
        ("privileges.rs", include_str!("privileges.rs")),
        ("registry.rs", include_str!("registry.rs")),
        ("server.rs", include_str!("server.rs")),
        ("service.rs", include_str!("service.rs")),
        ("testkit.rs", include_str!("testkit.rs")),
    ];

    #[test]
    fn core_modules_do_not_reach_the_front_end() {
        let forbidden = ["dbcrust::", "inquire", "OutputFormat", "reedline"];
        for (file, source) in SOURCES {
            for token in forbidden {
                assert!(
                    !source.contains(token),
                    "{file} must not depend on the front end, but mentions `{token}`; \
                     move it to the root crate's src/broker/cli.rs or pass it in"
                );
            }
        }
    }
}
