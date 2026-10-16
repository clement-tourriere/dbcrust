//! The assistant's TLS regression tests: the genai client (reqwest 0.13, no
//! bundled rustls provider) depends on the process-wide provider that
//! [`dbcrust_core::tls`] installs. They live here, not in `tls.rs`, so the provider
//! module itself needs no genai/reqwest dependency.

use dbcrust_core::tls::install_default_crypto_provider;

/// The genai client (reqwest 0.13, no bundled provider) panics on
/// construction when no default provider is installed; this is the
/// regression test for dropping `aws-lc-rs`.
#[test]
fn genai_client_builds_with_installed_provider() {
    install_default_crypto_provider();
    // Building the underlying HTTP client is lazy-free in genai: reaching
    // this point without a panic or an error is the assertion
    let client = genai::Client::new().expect("genai client");
    drop(client);
}

#[test]
fn reqwest_clients_build() {
    install_default_crypto_provider();
    // reqwest 0.12 (rustls-tls: ring + webpki roots) — vault/chatgpt/update
    reqwest::Client::builder()
        .build()
        .expect("reqwest 0.12 client");
    // reqwest 0.13 (rustls-no-provider + platform verifier) — genai
    reqwest_genai::Client::builder()
        .build()
        .expect("reqwest 0.13 client");
}

/// Opt-in real TLS handshake (webpki roots via reqwest 0.12, platform
/// verifier via reqwest 0.13): `DBCRUST_NET_TESTS=1 cargo test net_tls`.
#[tokio::test]
async fn net_tls_handshake_verifies_certificates() {
    if std::env::var_os("DBCRUST_NET_TESTS").is_none() {
        return;
    }
    install_default_crypto_provider();
    let r12 = reqwest::get("https://github.com").await.expect("0.12 GET");
    assert!(r12.status().is_success() || r12.status().is_redirection());
    let r13 = reqwest_genai::get("https://github.com")
        .await
        .expect("0.13 GET");
    assert!(r13.status().is_success() || r13.status().is_redirection());
    // Verification stays on: an expired certificate must be rejected
    assert!(reqwest::get("https://expired.badssl.com").await.is_err());
    assert!(
        reqwest_genai::get("https://expired.badssl.com")
            .await
            .is_err()
    );
}
