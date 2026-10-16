//! The HTTP client every AWS service client of DBCrust uses: hyper 1.x with
//! rustls on **ring**, trusting the operating system's root certificates (plus
//! an optional PEM file for a private CA such as a MinIO with its own
//! certificate).
//!
//! The SDK is compiled with `default-features = false`, so it has no HTTP client
//! of its own; a request without one fails with a "no HTTP client" error. This
//! is the single place that builds the client, and it names the ring crypto mode
//! explicitly, so there is no process-default provider to get wrong.

use crate::AwsError;
use aws_smithy_http_client::Builder;
use aws_smithy_http_client::tls::rustls_provider::CryptoMode;
use aws_smithy_http_client::tls::{Provider, TlsContext, TrustStore};
use aws_types::sdk_config::SharedHttpClient;
use std::path::Path;

/// A rustls-on-ring HTTP client, with `ca_cert` (a PEM file, one or several
/// certificates) added to the operating system's roots when given.
pub fn ring_client(ca_cert: Option<&Path>) -> Result<SharedHttpClient, AwsError> {
    let builder = Builder::new().tls_provider(Provider::Rustls(CryptoMode::Ring));
    let Some(path) = ca_cert else {
        return Ok(builder.build_https());
    };
    let pem = std::fs::read(path).map_err(|error| {
        AwsError::Configuration(format!(
            "cannot read the CA certificate {}: {error}",
            path.display()
        ))
    })?;
    if !pem.windows(11).any(|window| window == b"-----BEGIN ") {
        return Err(AwsError::Configuration(format!(
            "{} holds no PEM certificate (expected a '-----BEGIN CERTIFICATE-----' block)",
            path.display()
        )));
    }
    let context = TlsContext::builder()
        .with_trust_store(TrustStore::default().with_pem_certificate(pem))
        .build()
        .map_err(|error| {
            AwsError::Configuration(format!(
                "the CA certificate {} was rejected: {error}",
                path.display()
            ))
        })?;
    Ok(builder.tls_context(context).build_https())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_builds_without_extra_certificates() {
        ring_client(None).expect("ring client");
    }

    #[test]
    fn a_missing_certificate_file_is_a_configuration_error() {
        let error = ring_client(Some(Path::new("/nonexistent/dbcrust-ca.pem"))).unwrap_err();
        assert!(matches!(error, AwsError::Configuration(_)), "{error}");
        assert!(error.to_string().contains("cannot read the CA certificate"));
    }

    #[test]
    fn a_file_without_a_pem_block_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, "not a certificate").unwrap();
        let error = ring_client(Some(&path)).unwrap_err();
        assert!(
            error.to_string().contains("holds no PEM certificate"),
            "{error}"
        );
    }
}
