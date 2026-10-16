//! DBCrust core: the database-client abstraction and everything that is pure
//! data or pure text processing around it.
//!
//! This crate is the bottom layer of the workspace. It names no database
//! driver, no HTTP/Docker stack and no terminal UI crate; the drivers, the
//! credential broker, the AI assistant and the CLI all build on it. The root
//! `dbcrust` crate re-exports these modules under their historical paths
//! (`dbcrust::database`, `dbcrust::format`, …).

pub mod ai_config; // AI config types shared by `config` and `ai`
pub mod complex_display; // Unified display system for complex data types
pub mod database; // `DatabaseClient`/`MetadataProvider` traits, `ConnectionInfo`, `DatabaseError`
pub mod ddl; // `TableCatalog`, `collect_table_names`, `format_table_ddl` (`\ddl` + AI schema context)
pub mod docker_target; // `docker://` target parsing/rendering (pure, no daemon access)
pub mod explain_plan; // PostgreSQL EXPLAIN plan tree + parser (UI-independent)
pub mod format; // Query-result and metadata rendering
pub mod geojson_display; // GeoJSON display implementation
pub mod json_display; // JSON display implementation
pub mod model; // Metadata DTOs (TableDetails, ColumnInfo, …)
pub mod options; // Connection-option keys DBCrust interprets (vault, password command, hardening)
pub mod output_format; // `-o/--format` output format enum
pub mod password_sanitizer; // Secret redaction for URLs, options and error text
pub mod performance_analyzer; // Performance analysis for EXPLAIN queries
pub mod redis_command; // `redis-cli` style command-line tokenizer (shared by the Redis driver and the safety guard)
pub mod regex_operators; // Cross-backend translation of PG-style regex operators (~, ~*, !~, !~*)
pub mod safety; // Read-only statement classification (--read-only + AI agent guard)
pub mod sql_buffer; // Statement splitting / completeness lexer (the REPL validator is in root)
pub mod sql_parser; // Enhanced SQL parser for autocompletion
#[cfg(feature = "tls")]
pub mod tls; // Process-wide rustls crypto provider (ring)
pub mod vector_display; // Vector visualization for PostgreSQL extensions
