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
}
