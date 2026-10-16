//! Statement splitting and buffer-completeness lexer.
//!
//! The pure lexer/splitter lives in `dbcrust-core`; the reedline
//! [`SqlValidator`] needs the REPL's line editor, so it stays in this crate
//! (`sql_validator`) and is re-exported here for existing paths.

pub use crate::sql_validator::SqlValidator;
pub use dbcrust_core::sql_buffer::*;
