//! AI configuration types now live in [`dbcrust_core::ai_config`] so `config.rs` does
//! not depend on the `ai` module; this path stays for existing callers.

pub use dbcrust_core::ai_config::*;
