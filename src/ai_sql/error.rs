//! Error types for AI SQL generation

use thiserror::Error;

/// Result type for AI SQL operations
pub type AiResult<T> = Result<T, AiError>;

/// Errors that can occur during AI SQL generation
#[derive(Error, Debug)]
pub enum AiError {
    #[error("AI provider error: {0}")]
    ProviderError(String),

    #[error("Configuration error: {0}")]
    ConfigurationError(String),

    #[error("Schema extraction error: {0}")]
    SchemaError(String),

    #[error("SQL validation error: {0}")]
    ValidationError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error: {status_code} - {message}")]
    ApiError { status_code: u16, message: String },

    #[error("Authentication error: {0}")]
    AuthenticationError(String),

    #[error("Timeout error: operation took longer than {timeout_secs}s")]
    TimeoutError { timeout_secs: u64 },

    #[error("Cache error: {0}")]
    CacheError(String),

    #[error("Unsupported database type: {0}")]
    UnsupportedDatabase(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] crate::database::DatabaseError),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("HTTP client error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("User cancelled operation")]
    UserCancelled,
}

impl AiError {
    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AiError::NetworkError(_) | AiError::TimeoutError { .. }
        )
    }

    /// Get user-friendly error message
    pub fn user_message(&self) -> String {
        match self {
            AiError::ProviderError(msg) => format!("AI provider error: {}", msg),
            AiError::ConfigurationError(msg) => {
                format!("Configuration issue: {}. Check your config file or environment variables.", msg)
            }
            AiError::SchemaError(msg) => {
                format!("Schema extraction failed: {}. Ensure database connection is active.", msg)
            }
            AiError::ValidationError(msg) => format!("SQL validation failed: {}", msg),
            AiError::NetworkError(msg) => {
                format!("Network error: {}. Check your internet connection.", msg)
            }
            AiError::ApiError {
                status_code,
                message,
            } => format!("API error ({}): {}", status_code, message),
            AiError::AuthenticationError(msg) => {
                format!("Authentication error: {}. Please re-authenticate using 'dbcrust ai-auth login'", msg)
            }
            AiError::TimeoutError { timeout_secs } => {
                format!("Request timed out after {} seconds. Try again or increase timeout in config.", timeout_secs)
            }
            AiError::UnsupportedDatabase(db) => {
                format!("AI SQL generation not yet supported for {} databases", db)
            }
            AiError::UserCancelled => "Operation cancelled by user".to_string(),
            _ => self.to_string(),
        }
    }
}
