//! Process-wide rustls crypto provider.
//!
//! ring is the only rustls provider compiled into DBCrust (aws-lc-rs is a large
//! C/asm build and breaks some cross-compilation targets). Most TLS users pick
//! ring themselves through their own feature flags — sqlx
//! (`tls-rustls-ring-webpki`), ClickHouse (`rustls-tls-ring`), MongoDB and
//! Elasticsearch/reqwest 0.12 (`rustls-tls`, ring + webpki roots). The exception
//! is reqwest 0.13, used by `genai` for the AI assistant: it is built with
//! `rustls-no-provider` and takes whatever provider is installed as the process
//! default, panicking on client construction when there is none.

/// Install ring as the process-default rustls crypto provider.
///
/// Idempotent and cheap: a provider that is already installed (by us earlier,
/// or by an embedding host such as a Python process) is left in place.
pub fn install_default_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return;
    }
    // Lost the race against another installer: its provider stays, which is
    // exactly the outcome we want
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent_and_sets_a_default() {
        install_default_crypto_provider();
        install_default_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }

    /// The genai client (reqwest 0.13, no bundled provider) panics on
    /// construction when no default provider is installed; this is the
    /// regression test for dropping `aws-lc-rs`.
    #[test]
    fn genai_client_builds_with_installed_provider() {
        install_default_crypto_provider();
        let client = genai::Client::default();
        // Building the underlying HTTP client is lazy-free in genai: reaching
        // this point without a panic is the assertion
        drop(client);
    }

    #[test]
    fn reqwest_clients_build() {
        install_default_crypto_provider();
        // reqwest 0.12 (rustls-tls: ring + webpki roots) — vault/chatgpt/update
        reqwest::Client::builder().build().expect("reqwest 0.12 client");
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
}
