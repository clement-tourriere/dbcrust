//! Driver registry.
//!
//! The registry itself is the `dbcrust-drivers` crate: one table
//! (`connectors!`) maps every engine to the `dbcrust-driver-*` crate that
//! serves it. Everything above (`Database`, the broker gateway) asks
//! [`create_database_client`] for a `Box<dyn DatabaseClient>` and never names a
//! driver type.

pub use dbcrust_drivers::*;
