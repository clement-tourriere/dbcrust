//! OS keychain backend (macOS Keychain, Windows Credential Manager).
//!
//! Every record is one keychain item: service `dbcrust.store`, account =
//! record id. Keychain failures surface as typed errors; this backend never
//! falls back to anything else.

use super::{BackendKind, BackendState, BackendStatus, SecretBackend, SecretError};
use zeroize::Zeroizing;

/// Keyring service name of the DBCrust secret store.
pub const KEYCHAIN_SERVICE: &str = "dbcrust.store";

/// Account probed by `status()`; never written.
const PROBE_ACCOUNT: &str = "dbcrust.store.probe";

/// The OS keychain through the `keyring` crate.
#[derive(Debug, Clone)]
pub struct KeychainBackend {
    service: String,
}

impl Default for KeychainBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainBackend {
    pub fn new() -> Self {
        Self::with_service(KEYCHAIN_SERVICE)
    }

    /// Use another service name (tests against a throwaway keychain).
    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, id: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, id).map_err(map_error)
    }
}

fn map_error(error: keyring::Error) -> SecretError {
    use keyring::Error;
    match error {
        Error::NoEntry => SecretError::Unavailable("keychain item vanished".to_string()),
        Error::NoStorageAccess(e) => {
            SecretError::Locked(format!("the keychain refused access: {e}"))
        }
        Error::PlatformFailure(e) => SecretError::Unavailable(format!("keychain failure: {e}")),
        Error::BadEncoding(_) => {
            SecretError::Corrupt("keychain item is not valid text".to_string())
        }
        Error::Ambiguous(_) => {
            SecretError::Corrupt("several keychain items share one id".to_string())
        }
        Error::TooLong(attribute, limit) => {
            SecretError::Io(format!("{attribute} exceeds the keychain limit of {limit}"))
        }
        Error::Invalid(attribute, reason) => {
            SecretError::Io(format!("invalid keychain {attribute}: {reason}"))
        }
        other => SecretError::Unavailable(format!("keychain error: {other}")),
    }
}

impl SecretBackend for KeychainBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Keychain
    }

    fn get(&self, id: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        match self.entry(id)?.get_password() {
            Ok(value) => Ok(Some(Zeroizing::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_error(e)),
        }
    }

    fn put(&self, id: &str, value: &str) -> Result<(), SecretError> {
        self.entry(id)?.set_password(value).map_err(map_error)
    }

    fn delete(&self, id: &str) -> Result<bool, SecretError> {
        match self.entry(id)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(map_error(e)),
        }
    }

    fn status(&self) -> BackendStatus {
        let (state, detail) =
            match self
                .entry(PROBE_ACCOUNT)
                .and_then(|entry| match entry.get_password() {
                    Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(e) => Err(map_error(e)),
                }) {
                Ok(()) => (BackendState::Ready, format!("service \"{}\"", self.service)),
                Err(SecretError::Locked(reason)) => (BackendState::Locked, reason),
                Err(other) => (BackendState::Unavailable, other.to_string()),
            };
        BackendStatus {
            kind: BackendKind::Keychain,
            state,
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual test against the real credential store. Skipped unless
    /// `DBCRUST_TEST_KEYCHAIN=1`. On macOS it first creates a throwaway
    /// keychain under a temp dir and makes it the default keychain for the
    /// test's duration (the previous default is restored afterwards), so the
    /// login keychain is never written to.
    #[test]
    fn real_keychain_round_trip_is_opt_in() {
        if std::env::var("DBCRUST_TEST_KEYCHAIN").as_deref() != Ok("1") {
            return;
        }
        #[cfg(target_os = "macos")]
        let _throwaway = macos::ThrowawayKeychain::create();

        let service = format!("dbcrust.store.test.{}", std::process::id());
        let backend = KeychainBackend::with_service(&service);
        assert_eq!(backend.get("conn:test").unwrap().as_deref(), None);
        backend.put("conn:test", "canary-value").unwrap();
        assert_eq!(
            backend
                .get("conn:test")
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("canary-value")
        );
        assert!(backend.delete("conn:test").unwrap());
        assert!(!backend.delete("conn:test").unwrap());
        assert!(backend.get("conn:test").unwrap().is_none());
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use std::path::PathBuf;
        use std::process::Command;

        fn security(args: &[&str]) -> String {
            let output = Command::new("security")
                .args(args)
                .output()
                .expect("run /usr/bin/security");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        pub(super) struct ThrowawayKeychain {
            path: PathBuf,
            previous_default: String,
            _dir: tempfile::TempDir,
        }

        impl ThrowawayKeychain {
            pub(super) fn create() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("dbcrust-test.keychain-db");
                let previous_default = security(&["default-keychain", "-d", "user"])
                    .trim_matches(|c| c == '"' || c == ' ')
                    .to_string();
                let path_str = path.to_str().unwrap();
                security(&["create-keychain", "-p", "dbcrust-test", path_str]);
                security(&["unlock-keychain", "-p", "dbcrust-test", path_str]);
                security(&["default-keychain", "-d", "user", "-s", path_str]);
                Self {
                    path,
                    previous_default,
                    _dir: dir,
                }
            }
        }

        impl Drop for ThrowawayKeychain {
            fn drop(&mut self) {
                if !self.previous_default.is_empty() {
                    security(&[
                        "default-keychain",
                        "-d",
                        "user",
                        "-s",
                        &self.previous_default,
                    ]);
                }
                security(&["delete-keychain", self.path.to_str().unwrap()]);
            }
        }
    }
}
