//! SDK errors as messages a person can act on, with secrets scrubbed.

use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_dynamodb::operation::RequestId;
use dbcrust_core::database::DatabaseError;

/// The hint that goes with a well-known DynamoDB error code.
fn hint(code: &str) -> Option<&'static str> {
    Some(match code {
        "ResourceNotFoundException" => {
            "there is no such table (or index) in this region and account: `tables` lists them; \
             check ?region= and ?endpoint="
        }
        "AccessDeniedException" => {
            "the credentials are valid but not allowed to do this: the commands need \
             dynamodb:ListTables, DescribeTable, GetItem, Query, Scan and PartiQLSelect"
        }
        "UnrecognizedClientException" | "InvalidClientTokenId" => {
            "the access key id is unknown to this endpoint, or the session token is invalid"
        }
        "InvalidSignatureException" | "SignatureDoesNotMatch" => {
            "the secret access key is wrong, or the region / endpoint is not the one the key was \
             issued for"
        }
        "ExpiredTokenException" | "ExpiredToken" => {
            "the session credentials expired: refresh them (aws sso login, a new session token)"
        }
        "MissingAuthenticationToken" | "IncompleteSignature" => {
            "the request was not signed: this endpoint needs credentials"
        }
        "ProvisionedThroughputExceededException"
        | "ThrottlingException"
        | "RequestLimitExceeded" => {
            "the table or account is throttled: wait, narrow the read, or lower the limit"
        }
        "ValidationException" => {
            "the request is malformed: check the PartiQL, the key names and types"
        }
        "InternalServerError" | "ServiceUnavailable" => "DynamoDB failed on its side: retry",
        _ => return None,
    })
}

/// One line for `error`: `Code: message (hint)`, or the redacted source chain
/// for failures that never got a DynamoDB answer (DNS, TLS, timeouts…).
pub(crate) fn describe<E>(error: &SdkError<E>, secrets: &[&str]) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    let text = match error.code() {
        Some(code) if code != "Unhandled" => {
            let message = error.message().unwrap_or_default();
            let mut text = if message.is_empty() {
                code.to_string()
            } else {
                format!("{code}: {message}")
            };
            if let Some(hint) = hint(code) {
                text.push_str(&format!(" ({hint})"));
            }
            if let Some(id) = error.request_id() {
                text.push_str(&format!(" [request id {id}]"));
            }
            text
        }
        _ => dbcrust_aws::redact_error(error, secrets),
    };
    dbcrust_aws::redact_message(&text, secrets)
}

/// The [`DatabaseError`] for a failed request: a DynamoDB answer is a query
/// error, anything that never reached DynamoDB (dispatch, timeout,
/// construction) a connection error.
pub(crate) fn map_error<E>(error: SdkError<E>, secrets: &[&str]) -> DatabaseError
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    let text = describe(&error, secrets);
    match error {
        SdkError::ServiceError(_) | SdkError::ResponseError(_) => DatabaseError::QueryError(text),
        _ => DatabaseError::ConnectionError(text),
    }
}

/// Whether `error` is a DynamoDB answer carrying one of `codes`.
pub(crate) fn has_code<E>(error: &SdkError<E>, codes: &[&str]) -> bool
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    error.code().is_some_and(|code| codes.contains(&code))
}

/// The credentials were understood but are not allowed this.
pub(crate) fn is_denied<E>(error: &SdkError<E>) -> bool
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    has_code(error, &["AccessDeniedException"])
        || (error
            .raw_response()
            .is_some_and(|response| response.status().as_u16() == 403)
            && !has_code(
                error,
                &[
                    "InvalidSignatureException",
                    "SignatureDoesNotMatch",
                    "ExpiredTokenException",
                    "UnrecognizedClientException",
                    "MissingAuthenticationToken",
                ],
            ))
}
