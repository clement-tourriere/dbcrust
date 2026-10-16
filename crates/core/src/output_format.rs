/// Output format for query results (`-o/--format`, env `DBCRUST_FORMAT`).
///
/// A leaf module: the CLI grammar (`cli`), the renderer (`format`), the
/// session (`db`) and the broker all name it, and none of them should have
/// to depend on another to do so. Re-exported by the root crate as `cli::OutputFormat`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "usage", derive(usage::ValueEnum))]
pub enum OutputFormat {
    /// psql-style aligned table (default)
    #[default]
    Table,
    /// Vertical one-record-per-block layout (same as `\x`)
    Expanded,
    /// RFC 4180 CSV with a header row
    Csv,
    /// One `{"columns":[…],"rows":[[…]],"row_count":n,"truncated":bool}` envelope per statement
    Json,
    /// One JSON object per data row, keys in column order
    Jsonl,
}
