//! The credential broker: `dbcrust broker serve|enroll|list|revoke|policy|status`.
//!
//! The credential-holding core (paths, registry, admin, gateway, audit,
//! server, hardening, privileges, service) is the `dbcrust-broker` crate,
//! which depends on core/secrets/drivers/agent-protocol only; it is
//! re-exported here so `dbcrust::broker::…` paths are unchanged. This module
//! is the **front end**: [`cli`] owns the `usage`-derived argument types,
//! every `inquire` prompt, `OutputFormat` and the process exit codes, and
//! `e2e_tests` drives it against the real core.
//!
//! Unix only: on Windows the subcommands compile and report that the broker
//! is not supported yet.

pub use dbcrust_broker::BrokerError;
#[cfg(unix)]
pub use dbcrust_broker::{
    admin, audit, gateway, hardening, paths, privileges, registry, server, service,
};

#[cfg(unix)]
pub mod cli;

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
use dbcrust_broker::testkit;
