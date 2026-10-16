//! `secrets_index.toml`: the non-secret inventory of what the store holds.
//!
//! Keychains cannot enumerate their items, so DBCrust keeps ids and
//! non-secret metadata (engine/host/port/db/user, timestamps, revision,
//! whether an agent policy exists) in a plain TOML file next to the config.
//! It never contains a secret. It may drift from the backend (a crash between
//! the two writes, a manual keychain edit); `status` and `audit` report it.

use crate::SecretError;
use crate::fsutil;
use crate::record::{RecordId, RecordKind, SecretRecord};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// File name inside the config directory.
pub const SECRETS_INDEX_FILE_NAME: &str = "secrets_index.toml";
const INDEX_VERSION: u32 = 1;
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

/// One indexed record. Every field is non-secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: String,
    pub kind: RecordKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub has_agent_policy: bool,
}

impl IndexEntry {
    /// Entry for a non-connection id (`ai:`, `oauth:`, `broker:`).
    pub fn for_named(id: &str) -> Self {
        Self {
            id: id.to_string(),
            kind: RecordId::parse(id).map_or(RecordKind::Oauth, |parsed| parsed.kind),
            engine: None,
            host: None,
            port: None,
            database: None,
            user: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            revision: 1,
            has_agent_policy: false,
        }
    }

    /// Entry describing a connection (or broker) record.
    pub fn for_record(id: &str, record: &SecretRecord) -> Self {
        let target = &record.bound_target;
        Self {
            id: id.to_string(),
            kind: RecordId::parse(id).map_or(RecordKind::Connection, |parsed| parsed.kind),
            engine: Some(target.engine.clone()),
            host: Some(target.host.clone()),
            port: target.port,
            database: Some(target.database.clone()),
            user: Some(target.user.clone()),
            created_at: Some(record.created_at.clone()),
            revision: record.revision,
            has_agent_policy: record.agent_policy.is_some(),
        }
    }
}

/// The parsed file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsIndex {
    #[serde(default)]
    pub version: u32,
    #[serde(default, rename = "records")]
    pub records: Vec<IndexEntry>,
}

impl SecretsIndex {
    pub fn ids(&self) -> Vec<String> {
        self.records.iter().map(|entry| entry.id.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<&IndexEntry> {
        self.records.iter().find(|entry| entry.id == id)
    }
}

/// Read/write access to one index file.
#[derive(Debug, Clone)]
pub struct IndexFile {
    path: PathBuf,
}

impl IndexFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `<dir>/secrets_index.toml`.
    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join(SECRETS_INDEX_FILE_NAME))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing file = empty index. A damaged file is an error (the caller
    /// decides whether drift is tolerable).
    pub fn load(&self) -> Result<SecretsIndex, SecretError> {
        let Some(bytes) = fsutil::read_bounded(&self.path, MAX_INDEX_BYTES)? else {
            return Ok(SecretsIndex::default());
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| SecretError::Corrupt("secrets index is not valid text".to_string()))?;
        toml::from_str(&text)
            .map_err(|_| SecretError::Corrupt("secrets index could not be parsed".to_string()))
    }

    fn save(&self, index: &SecretsIndex) -> Result<(), SecretError> {
        let mut index = index.clone();
        index.version = INDEX_VERSION;
        index.records.sort_by(|a, b| a.id.cmp(&b.id));
        let body = toml::to_string_pretty(&index)
            .map_err(|_| SecretError::Io("secrets index could not be serialized".to_string()))?;
        let text = format!(
            "# DBCrust secret inventory — ids and non-secret metadata only.\n\
             # Secret values live in the OS keychain or secrets.enc, never here.\n{body}"
        );
        fsutil::write_private_atomic(&self.path, text.as_bytes())
    }

    /// Insert or replace one entry.
    pub fn upsert(&self, entry: IndexEntry) -> Result<(), SecretError> {
        let _lock = fsutil::FileLock::acquire(&self.path)?;
        let mut index = self.load().unwrap_or_default();
        index.records.retain(|existing| existing.id != entry.id);
        index.records.push(entry);
        self.save(&index)
    }

    /// Remove one entry (missing is fine).
    pub fn remove(&self, id: &str) -> Result<(), SecretError> {
        let _lock = fsutil::FileLock::acquire(&self.path)?;
        let mut index = self.load().unwrap_or_default();
        let before = index.records.len();
        index.records.retain(|existing| existing.id != id);
        if index.records.len() != before {
            self.save(&index)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetBinding;
    use std::collections::BTreeMap;

    #[test]
    fn upsert_remove_and_sorted_round_trip_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let index = IndexFile::in_dir(dir.path());
        assert!(index.load().unwrap().records.is_empty());

        let binding = TargetBinding::new("postgres", "db.example.com", None, "app", "alice", None);
        let record = SecretRecord::new("CANARY-PW", BTreeMap::new(), binding.clone());
        index
            .upsert(IndexEntry::for_record(&binding.record_id(), &record))
            .unwrap();
        index.upsert(IndexEntry::for_named("ai:anthropic")).unwrap();
        index.upsert(IndexEntry::for_named("ai:anthropic")).unwrap();

        let loaded = index.load().unwrap();
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0].id, "ai:anthropic");
        let conn = loaded.get(&binding.record_id()).unwrap();
        assert_eq!(conn.host.as_deref(), Some("db.example.com"));
        assert_eq!(conn.port, Some(5432));
        assert_eq!(conn.kind, RecordKind::Connection);

        let text = std::fs::read_to_string(index.path()).unwrap();
        assert!(!text.contains("CANARY"));

        index.remove("ai:anthropic").unwrap();
        index.remove("ai:anthropic").unwrap();
        assert_eq!(index.load().unwrap().records.len(), 1);
    }

    #[test]
    fn damaged_index_is_corrupt_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let index = IndexFile::in_dir(dir.path());
        std::fs::write(index.path(), "[[records]\nbroken").unwrap();
        assert!(matches!(index.load(), Err(SecretError::Corrupt(_))));
    }
}
