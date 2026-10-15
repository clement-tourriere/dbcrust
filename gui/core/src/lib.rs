//! Backend of the DBCrust desktop app. No GPUI here: everything in this
//! crate is plain Rust, unit-tested with `cargo test -p dbcrust-gui-core`
//! on any OS.
//!
//! - [`worker`]: one thread + runtime per live connection
//!   ([`ConnectionHandle`]), with out-of-band cancellation.
//! - [`store`]: the single owner of DBCrust's config and storage files.
//! - Pure models: [`statements`] (what to run), [`schema`], [`export`],
//!   [`completion`], [`explain`], [`history`], [`gui_state`].
//! - Discovery: [`docker`], [`vault`].

pub mod completion;
mod connect;
pub mod docker;
pub mod env;
pub mod error;
pub mod explain;
pub mod export;
pub mod gui_state;
pub mod history;
pub mod ids;
pub mod launcher;
pub mod rt;
pub mod schema;
pub mod sql_format;
pub mod statements;
pub mod store;
#[cfg(test)]
pub(crate) mod test_support;
pub mod types;
pub mod url_builder;
pub mod vault;
pub mod worker;

/// The DBCrust core crate, re-exported so the app needs one backend dependency.
pub use dbcrust;
pub use dbcrust::database::{DatabaseType, DatabaseTypeExt};
pub use error::{GuiError, GuiResult};
pub use ids::{ConnectionId, RequestId, TabId};
pub use store::ConfigStore;
pub use types::*;
pub use worker::ConnectionHandle;
