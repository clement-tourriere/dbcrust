//! A stand-in for the `zip` crate, patched in for `libduckdb-sys`'s build
//! script (see `DBCRUST_PATCH.md`).
//!
//! That build script only calls `ZipArchive::new(file)?.extract(dir)?` to unpack
//! a prebuilt `libduckdb` it just downloaded (`DUCKDB_DOWNLOAD_LIB=1`, the
//! non-bundled mode). DBCrust always compiles DuckDB from source, so that path
//! never runs; if somebody selects it anyway, it fails with a message instead
//! of unpacking.

use std::fmt;
use std::io::{Read, Seek};
use std::path::Path;

/// The one error this crate reports.
#[derive(Debug)]
pub struct ZipError;

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "DBCrust builds DuckDB from source (the `bundled` feature); unpacking a downloaded \
             libduckdb is not available (see third_party/zip-stub/DBCRUST_PATCH.md)",
        )
    }
}

impl std::error::Error for ZipError {}

pub mod result {
    pub use super::ZipError;
    pub type ZipResult<T> = Result<T, ZipError>;
}

/// Never constructed: [`ZipArchive::new`] always fails.
pub struct ZipArchive<R> {
    _reader: R,
}

impl<R: Read + Seek> ZipArchive<R> {
    pub fn new(_reader: R) -> result::ZipResult<Self> {
        Err(ZipError)
    }

    pub fn extract<P: AsRef<Path>>(&mut self, _directory: P) -> result::ZipResult<()> {
        Err(ZipError)
    }
}
