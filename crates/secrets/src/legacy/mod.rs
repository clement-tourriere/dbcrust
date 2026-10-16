//! Readers for the credential formats DBCrust used before the secret store.
//!
//! Everything here is read-only (plus the removal of lines the migration has
//! moved): no module may create new legacy values, so nothing outside the
//! tests ever writes a machine-key `enc:` value or a new `~/.dbcrust` entry.
//! `dbcrust secrets migrate` is the way out.
//!
//! * [`password_encryption`] — decrypt-only reader of the machine-key `enc:`
//!   values.
//! * [`dbcrust_pass`] — the `~/.dbcrust` password file.
//! * [`vault_encryption`] — Vault credential cache encryption.
//! * [`ai_keys`] — the retired AI key keychain entries and `ai_keys.enc`.

pub mod ai_keys;
pub mod dbcrust_pass;
pub mod password_encryption;
pub mod vault_encryption;
