//! SDK errors as messages a person can act on, with secrets scrubbed.

use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::operation::RequestId;
use dbcrust_core::database::DatabaseError;

/// The hint that goes with a well-known S3 error code.
fn hint(code: &str) -> Option<&'static str> {
    Some(match code {
        "NoSuchBucket" => "check the bucket name (and ?endpoint= / ?region= for other providers)",
        "NoSuchKey" => "there is no object with that exact key; `ls` shows what is there",
        "AccessDenied" | "AllAccessDisabled" => {
            "the credentials are valid but not allowed to do this: ListBucket / GetObject on the \
             bucket (and prefix) are what the commands need"
        }
        "InvalidAccessKeyId" => "the access key id is unknown to this endpoint",
        "SignatureDoesNotMatch" => {
            "the secret access key is wrong, or the region / endpoint is not the one the key was \
             issued for"
        }
        "ExpiredToken" | "ExpiredTokenException" | "TokenRefreshRequired" => {
            "the session credentials expired: refresh them (aws sso login, a new session token)"
        }
        "InvalidToken" => "the session token is not valid for these keys",
        "RequestTimeTooSkewed" => "the clock of this machine differs from the server's by too much",
        "NotFound" => "there is no such bucket or object at this endpoint",
        "SlowDown" | "RequestLimitExceeded" => "the endpoint is throttling requests: retry later",
        _ => return None,
    })
}

/// The HTTP status of the answer, when there was one. A HEAD answer has no
/// body, so for HeadBucket / HeadObject the status is all S3 says.
pub(crate) fn status<E>(error: &SdkError<E>) -> Option<u16> {
    error
        .raw_response()
        .map(|response| response.status().as_u16())
}

fn is_region_error(code: &str) -> bool {
    matches!(
        code,
        "PermanentRedirect" | "AuthorizationHeaderMalformed" | "IllegalLocationConstraintException"
    ) || code == "301"
}

/// One line for `error`: `Code: message (hint)`, or the redacted source chain
/// for failures that never got an S3 answer (DNS, TLS, timeouts…).
pub(crate) fn describe<E>(error: &SdkError<E>, secrets: &[&str]) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    let region_header = error
        .raw_response()
        .and_then(|response| response.headers().get("x-amz-bucket-region"))
        .map(str::to_string);
    let status = status(error);
    let text = match error.code() {
        // A HEAD answer carries no S3 error document: the SDK reports a generic
        // "unhandled" error, and the status code is the information
        Some(code) if code != "Unhandled" => {
            let message = error.message().unwrap_or_default();
            let mut text = if message.is_empty() {
                code.to_string()
            } else {
                format!("{code}: {message}")
            };
            let wrong_region = is_region_error(code) || status == Some(301);
            match (&region_header, wrong_region) {
                (Some(region), true) => text.push_str(&format!(
                    " (the bucket is in region {region}: connect with ?region={region})"
                )),
                _ => {
                    if let Some(hint) = hint(code) {
                        text.push_str(&format!(" ({hint})"));
                    }
                }
            }
            if let Some(id) = error.request_id() {
                text.push_str(&format!(" [request id {id}]"));
            }
            text
        }
        _ => match (status, &region_header) {
            (Some(301), Some(region)) => {
                format!("the bucket is in region {region}: connect with ?region={region}")
            }
            (Some(403), _) => "Forbidden: access denied (the answer to a HEAD request has no \
                               detail: check the credentials and the bucket policy)"
                .to_string(),
            (Some(404), _) => {
                "NotFound: there is no such bucket or object at this endpoint".to_string()
            }
            _ => dbcrust_aws::redact_error(error, secrets),
        },
    };
    dbcrust_aws::redact_message(&text, secrets)
}

/// The [`DatabaseError`] for a failed request: an S3 answer is a query error,
/// anything that never reached S3 (dispatch, timeout, construction) a
/// connection error.
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

/// Whether `error` is an S3 answer carrying one of `codes`.
pub(crate) fn has_code<E>(error: &SdkError<E>, codes: &[&str]) -> bool
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    error.code().is_some_and(|code| codes.contains(&code))
}

/// The credentials were understood but are not allowed this (HTTP 403, with
/// or without an S3 error document).
pub(crate) fn is_denied<E>(error: &SdkError<E>) -> bool
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    status(error) == Some(403)
        && !has_code(
            error,
            &[
                "InvalidAccessKeyId",
                "SignatureDoesNotMatch",
                "ExpiredToken",
                "ExpiredTokenException",
                "InvalidToken",
                "RequestTimeTooSkewed",
                "AuthorizationHeaderMalformed",
            ],
        )
}
