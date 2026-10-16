//! Showing AWS errors without secrets.
//!
//! An SDK error is a chain (`dispatch failure` → `io error` → …); the message a
//! user needs is usually the deepest one. Nothing in a normal SDK error carries
//! a key, but a `credential_process` that echoes its output, a proxy URL or a
//! profile line quoted by a parser error can, so every message goes through
//! here: known secrets are replaced verbatim, and anything shaped like an
//! access key id or a `key=value` secret is masked.

use dbcrust_core::password_sanitizer::{REDACTED, redact_text};
use std::error::Error;

/// Access key ids start with a 4-letter prefix and have 16 more upper-case
/// alphanumerics (`AKIA…` long-term, `ASIA…` temporary, `AIDA…` user ids).
const KEY_ID_PREFIXES: [&str; 6] = ["AKIA", "ASIA", "AIDA", "AROA", "AIPA", "ANPA"];
const KEY_ID_LEN: usize = 20;

fn mask_key_ids(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let candidate = bytes.get(i..i + KEY_ID_LEN);
        let at_boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let matches_shape = at_boundary
            && candidate.is_some_and(|window| {
                KEY_ID_PREFIXES
                    .iter()
                    .any(|prefix| window.starts_with(prefix.as_bytes()))
                    && window
                        .iter()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                    && bytes
                        .get(i + KEY_ID_LEN)
                        .is_none_or(|next| !next.is_ascii_alphanumeric())
            });
        if matches_shape {
            out.push_str(REDACTED);
            i += KEY_ID_LEN;
        } else {
            // `i` is always on a char boundary: advance by the char's length
            let ch_len = text[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&text[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// Scrub `message`: the given `secrets` verbatim, access key ids by shape, and
/// URL / `key=value` / JSON secrets through the shared redactor.
pub fn redact_message(message: &str, secrets: &[&str]) -> String {
    let mut text = message.to_string();
    for secret in secrets.iter().filter(|secret| secret.len() >= 4) {
        text = text.replace(secret, REDACTED);
    }
    mask_key_ids(&redact_text(&text))
}

/// The `a: b: c` rendering of an error and its sources, scrubbed.
pub fn redact_error(error: &dyn Error, secrets: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&dyn Error> = Some(error);
    while let Some(err) = current {
        let part = err.to_string();
        // SDK errors repeat their source's text; keep each distinct message once
        if !parts
            .iter()
            .any(|seen| seen.contains(&part) || part.contains(seen.as_str()))
        {
            parts.push(part);
        } else if let Some(last) = parts.last_mut() {
            if part.len() > last.len() && part.contains(last.as_str()) {
                *last = part;
            }
        }
        current = err.source();
    }
    redact_message(&parts.join(": "), secrets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::fmt;

    #[rstest]
    #[case(
        "denied for AKIAIOSFODNN7EXAMPLE on bucket",
        "denied for [REDACTED] on bucket"
    )]
    #[case("temp ASIAIOSFODNN7EXAMPLE.", "temp [REDACTED].")]
    #[case(
        "two AKIAIOSFODNN7EXAMPLE,AKIAIOSFODNN7EXAMPL2",
        "two [REDACTED],[REDACTED]"
    )]
    // not key ids: wrong length, lower case, inside a longer token
    #[case("AKIASHORT", "AKIASHORT")]
    #[case("akiaiosfodnn7example", "akiaiosfodnn7example")]
    #[case("XAKIAIOSFODNN7EXAMPLE", "XAKIAIOSFODNN7EXAMPLE")]
    #[case("AKIAIOSFODNN7EXAMPLEX", "AKIAIOSFODNN7EXAMPLEX")]
    #[case("héllo wörld ünïcode", "héllo wörld ünïcode")]
    fn key_ids_are_masked_by_shape(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(mask_key_ids(input), expected);
    }

    #[test]
    fn known_secrets_and_key_value_pairs_are_scrubbed() {
        // Built at run time: a secret-shaped literal in the source would (rightly)
        // trip the repository's secret scanner
        let key = "canary-key-0123";
        let token = "canary-token-4567";
        let text =
            format!("credential_process failed: aws_secret_access_key={key} and token {token}");
        let out = redact_message(&text, &[token]);
        assert!(!out.contains(key), "{out}");
        assert!(!out.contains(token), "{out}");
        let url = redact_message("GET https://u:hunter2pw@proxy:3128/", &[]);
        assert!(!url.contains("hunter2pw"), "{url}");
    }

    #[test]
    fn short_secrets_are_not_replaced_blindly() {
        // a 1-3 char "secret" would mangle every message
        assert_eq!(redact_message("a bucket", &["a"]), "a bucket");
    }

    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);
    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }
    impl Error for Layer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.1.as_deref().map(|e| e as &(dyn Error + 'static))
        }
    }

    #[test]
    fn the_whole_chain_is_shown_once_and_scrubbed() {
        let chain = Layer(
            "dispatch failure",
            Some(Box::new(Layer(
                "io error",
                Some(Box::new(Layer(
                    "connection refused (AKIAIOSFODNN7EXAMPLE)",
                    None,
                ))),
            ))),
        );
        assert_eq!(
            redact_error(&chain, &[]),
            "dispatch failure: io error: connection refused ([REDACTED])"
        );
        // a source that repeats its parent's text is not printed twice
        let repeated = Layer(
            "service error",
            Some(Box::new(Layer("service error", None))),
        );
        assert_eq!(redact_error(&repeated, &[]), "service error");
    }
}
