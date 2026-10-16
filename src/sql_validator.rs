//! Reedline [`Validator`] for the REPL's SQL input: Enter submits the buffer or
//! inserts a newline depending on [`crate::sql_buffer::is_buffer_complete`].
//! Re-exported as `sql_buffer::SqlValidator`.

use crate::sql_buffer::is_buffer_complete;
use reedline::{ValidationResult, Validator};

/// Reedline validator: keep the buffer open while a statement is clearly
/// unterminated. Deliberately conservative — it never demands a trailing
/// `;` for plain statements, so existing "type a query, press Enter" muscle
/// memory keeps working.
pub struct SqlValidator;

impl Validator for SqlValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if is_buffer_complete(line) {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validator_matches_buffer_completeness() {
        let validator = SqlValidator;
        assert!(matches!(
            validator.validate("SELECT 1"),
            ValidationResult::Complete
        ));
        assert!(matches!(
            validator.validate("SELECT 'open"),
            ValidationResult::Incomplete
        ));
    }
}
