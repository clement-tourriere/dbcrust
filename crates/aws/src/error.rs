use dbcrust_core::database::DatabaseError;

/// A problem with the AWS part of a connection, before any request is sent.
#[derive(Debug, thiserror::Error)]
pub enum AwsError {
    /// An option or URL part that cannot be used (bad value, contradiction).
    #[error("{0}")]
    InvalidOption(String),
    /// The SDK configuration could not be built (certificate file, profile…).
    #[error("{0}")]
    Configuration(String),
}

impl From<AwsError> for DatabaseError {
    fn from(error: AwsError) -> Self {
        match error {
            AwsError::InvalidOption(message) => DatabaseError::InvalidUrl(message),
            AwsError::Configuration(message) => DatabaseError::ConnectionError(message),
        }
    }
}
