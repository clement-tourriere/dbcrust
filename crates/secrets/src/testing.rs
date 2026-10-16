//! Test doubles for the secret store (compiled only for tests or with the
//! `test-support` feature).

use crate::{BackendKind, BackendStatus, MemoryBackend, SecretBackend, SecretError};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// A [`MemoryBackend`] that can be told to fail specific operations, to
/// simulate a keychain that errors at a chosen migration step.
pub struct FaultyBackend {
    inner: MemoryBackend,
    fail_get: Mutex<Option<SecretError>>,
    fail_put: Mutex<Option<SecretError>>,
    /// Fail the n-th `put` (1-based) once, then recover.
    fail_nth_put: Mutex<Option<usize>>,
    puts: Mutex<usize>,
    /// Make `get` of one id fail (verification step).
    fail_get_id: Mutex<Option<String>>,
}

impl FaultyBackend {
    pub fn new(inner: MemoryBackend) -> Self {
        Self {
            inner,
            fail_get: Mutex::new(None),
            fail_put: Mutex::new(None),
            fail_nth_put: Mutex::new(None),
            puts: Mutex::new(0),
            fail_get_id: Mutex::new(None),
        }
    }

    pub fn fail_get_with(&self, error: SecretError) {
        *self.fail_get.lock().unwrap() = Some(error);
    }

    pub fn fail_put_with(&self, error: SecretError) {
        *self.fail_put.lock().unwrap() = Some(error);
    }

    pub fn clear_faults(&self) {
        *self.fail_get.lock().unwrap() = None;
        *self.fail_put.lock().unwrap() = None;
        *self.fail_nth_put.lock().unwrap() = None;
        *self.fail_get_id.lock().unwrap() = None;
    }

    pub fn fail_nth_put(&self, n: usize) {
        *self.puts.lock().unwrap() = 0;
        *self.fail_nth_put.lock().unwrap() = Some(n);
    }

    pub fn fail_get_of(&self, id: &str) {
        *self.fail_get_id.lock().unwrap() = Some(id.to_string());
    }

    fn clone_error(error: &SecretError) -> SecretError {
        match error {
            SecretError::Unavailable(m) => SecretError::Unavailable(m.clone()),
            SecretError::Locked(m) => SecretError::Locked(m.clone()),
            SecretError::Denied(m) => SecretError::Denied(m.clone()),
            SecretError::NotFound(m) => SecretError::NotFound(m.clone()),
            SecretError::Corrupt(m) => SecretError::Corrupt(m.clone()),
            SecretError::TargetMismatch(m) => SecretError::TargetMismatch(m.clone()),
            SecretError::Io(m) => SecretError::Io(m.clone()),
        }
    }
}

impl SecretBackend for FaultyBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Memory
    }

    fn get(&self, id: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        if let Some(error) = self.fail_get.lock().unwrap().as_ref() {
            return Err(Self::clone_error(error));
        }
        if self.fail_get_id.lock().unwrap().as_deref() == Some(id) {
            return Err(SecretError::Unavailable(
                "injected read failure".to_string(),
            ));
        }
        self.inner.get(id)
    }

    fn put(&self, id: &str, value: &str) -> Result<(), SecretError> {
        if let Some(error) = self.fail_put.lock().unwrap().as_ref() {
            return Err(Self::clone_error(error));
        }
        let mut puts = self.puts.lock().unwrap();
        *puts += 1;
        if *self.fail_nth_put.lock().unwrap() == Some(*puts) {
            return Err(SecretError::Unavailable(
                "injected write failure".to_string(),
            ));
        }
        drop(puts);
        self.inner.put(id, value)
    }

    fn delete(&self, id: &str) -> Result<bool, SecretError> {
        self.inner.delete(id)
    }

    fn status(&self) -> BackendStatus {
        self.inner.status()
    }

    fn list_ids(&self) -> Result<Option<Vec<String>>, SecretError> {
        self.inner.list_ids()
    }
}
