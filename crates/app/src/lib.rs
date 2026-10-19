//! `dbcrust-app`: the application layer the CLI and the desktop app share.
//!
//! Configuration and its storage files (`config`, `config_editor`'s schema),
//! credential lookups (`pgpass`, `myconf`, `vault_client`), SSH tunnels, and
//! the connection orchestrator [`db::Database`] (drivers, Docker routes,
//! tunnels, read-only guard, EXPLAIN, column views).
//!
//! It never talks to the terminal: no REPL, no `inquire`, no TUI. Prompts and
//! full-screen viewers are asked of the front end through
//! [`terminal_ui::TerminalUi`], which the CLI installs and the desktop app
//! leaves empty.

#[cfg(feature = "ai")]
mod ai_session_impl; // `impl dbcrust_ai::AiSession for db::Database`
pub mod config;
pub mod config_editor; // Schema behind `\config get|set` and the GUI settings
pub mod db;
pub mod features; // Add-ons a build may leave out + their "not compiled in" error
pub mod myconf; // MySQL option-file passwords
pub mod pgpass;
pub mod ssh_tunnel;
pub mod terminal_ui;
#[cfg(test)]
mod test_isolation; // canaries for the dbcrust-secrets `test-mode` wiring
pub mod vault_client;

pub use config::Config;
pub use db::Database;
