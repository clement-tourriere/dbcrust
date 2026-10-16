//! In-memory backend for tests and ephemeral tooling. Nothing is persisted.

use super::{BackendKind, BackendState, BackendStatus, SecretBackend, SecretError};
use std::collections::BTreeMap;
use std::sync::Mutex;
use zeroize::Zeroizing;

/// A `BTreeMap` behind a mutex.
#[derive(Default)]
pub struct MemoryBackend {
    values: Mutex<BTreeMap<String, Zeroizing<String>>>,
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    fn values(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Zeroizing<String>>> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl std::fmt::Debug for MemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryBackend")
            .field("records", &self.values().len())
            .finish()
    }
}

impl SecretBackend for MemoryBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Memory
    }

    fn get(&self, id: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        Ok(self.values().get(id).cloned())
    }

    fn put(&self, id: &str, value: &str) -> Result<(), SecretError> {
        self.values()
            .insert(id.to_string(), Zeroizing::new(value.to_string()));
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<bool, SecretError> {
        Ok(self.values().remove(id).is_some())
    }

    fn status(&self) -> BackendStatus {
        BackendStatus {
            kind: BackendKind::Memory,
            state: BackendState::Ready,
            detail: format!("{} record(s) held in process memory", self.values().len()),
        }
    }

    fn list_ids(&self) -> Result<Option<Vec<String>>, SecretError> {
        Ok(Some(self.values().keys().cloned().collect()))
    }
}
