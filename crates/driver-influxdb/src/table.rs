//! The driver's result shape before it becomes a `StructuredQueryResult`.

use dbcrust_core::database::StructuredQueryResult;

/// A reply mapped to one table, with what the budget cut off and what the
/// user should know about the reply's shape.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// The row or byte budget ended the read before the server did.
    pub truncated: bool,
    /// Remarks about the reply (statements or series that were not shown).
    pub notices: Vec<String>,
}

impl Table {
    pub fn into_result(self) -> StructuredQueryResult {
        StructuredQueryResult {
            columns: self.columns,
            rows: self.rows,
        }
    }
}
