//! The [`SecretStore`] facade: backend selection, the connection-secret
//! lookup with target-binding verification, and the non-secret index.

use super::file::{KdfParams, KeySource, PassphraseFileBackend, platform_unlock_cache};
use super::index::{IndexEntry, IndexFile, SecretsIndex};
use super::keychain::KeychainBackend;
use super::memory::MemoryBackend;
use super::record::{RecordId, RecordKind, SecretRecord, TargetBinding};
use super::{BackendKind, BackendState, BackendStatus, SecretBackend, SecretError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;

/// `[secrets] backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendChoice {
    /// Keychain on macOS/Windows, encrypted file elsewhere.
    #[default]
    Auto,
    Keychain,
    File,
}

impl BackendChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            BackendChoice::Auto => "auto",
            BackendChoice::Keychain => "keychain",
            BackendChoice::File => "file",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "auto" => Some(BackendChoice::Auto),
            "keychain" => Some(BackendChoice::Keychain),
            "file" => Some(BackendChoice::File),
            _ => None,
        }
    }

    /// What `auto` resolves to on this platform.
    pub fn resolved(&self) -> BackendChoice {
        match self {
            BackendChoice::Auto => {
                if cfg!(any(target_os = "macos", target_os = "windows")) {
                    BackendChoice::Keychain
                } else {
                    BackendChoice::File
                }
            }
            explicit => *explicit,
        }
    }
}

fn default_true() -> bool {
    true
}

/// The `[secrets]` section of `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsConfig {
    /// Where secrets live (`auto`, `keychain`, `file`).
    #[serde(default)]
    pub backend: BackendChoice,
    /// Cache the unlocked file key for the login session (Linux keyutils
    /// session keyring; process memory elsewhere).
    #[serde(default = "default_true")]
    pub file_unlock_cache: bool,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            backend: BackendChoice::Auto,
            file_unlock_cache: true,
        }
    }
}

/// Everything needed to open the regular store.
#[derive(Debug, Clone)]
pub struct StoreSettings {
    pub config: SecretsConfig,
    /// DBCrust config directory (holds `secrets.enc` and the index).
    pub dir: PathBuf,
    /// KDF cost for a newly created file.
    pub kdf: KdfParams,
}

impl StoreSettings {
    pub fn new(config: SecretsConfig, dir: PathBuf) -> Self {
        Self {
            config,
            dir,
            kdf: KdfParams::DEFAULT,
        }
    }
}

/// A backend that fails every operation with the reason it is unusable, so a
/// misconfiguration is an error rather than a silent downgrade.
struct UnavailableBackend {
    kind: BackendKind,
    reason: String,
}

impl SecretBackend for UnavailableBackend {
    fn kind(&self) -> BackendKind {
        self.kind
    }
    fn get(&self, _id: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        Err(SecretError::Unavailable(self.reason.clone()))
    }
    fn put(&self, _id: &str, _value: &str) -> Result<(), SecretError> {
        Err(SecretError::Unavailable(self.reason.clone()))
    }
    fn delete(&self, _id: &str) -> Result<bool, SecretError> {
        Err(SecretError::Unavailable(self.reason.clone()))
    }
    fn status(&self) -> BackendStatus {
        BackendStatus {
            kind: self.kind,
            state: BackendState::Unavailable,
            detail: self.reason.clone(),
        }
    }
}

/// Record counts by kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RecordCounts {
    pub connection: usize,
    pub ai: usize,
    pub oauth: usize,
    pub broker: usize,
    pub other: usize,
}

impl RecordCounts {
    fn add(&mut self, id: &str) {
        match RecordId::parse(id).map(|parsed| parsed.kind) {
            Some(RecordKind::Connection) => self.connection += 1,
            Some(RecordKind::Ai) => self.ai += 1,
            Some(RecordKind::Oauth) => self.oauth += 1,
            Some(RecordKind::Broker) => self.broker += 1,
            None => self.other += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.connection + self.ai + self.oauth + self.broker + self.other
    }
}

/// Differences between the backend's contents and the index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IndexDrift {
    /// False when the backend cannot enumerate (keychains), so only the
    /// index is known.
    pub checked: bool,
    pub missing_in_backend: Vec<String>,
    pub missing_in_index: Vec<String>,
    pub index_error: Option<String>,
}

impl IndexDrift {
    pub fn is_clean(&self) -> bool {
        self.missing_in_backend.is_empty()
            && self.missing_in_index.is_empty()
            && self.index_error.is_none()
    }
}

/// Result of [`SecretStore::status`]. Contains ids, never values.
#[derive(Debug, Clone)]
pub struct StoreStatus {
    pub backend: BackendStatus,
    pub counts: RecordCounts,
    pub index_path: Option<PathBuf>,
    pub drift: IndexDrift,
}

/// Facade over one backend plus the non-secret index.
///
/// Cheap to share (`Arc`). Reads never prompt in non-interactive contexts:
/// a locked file backend returns [`SecretError::Locked`].
pub struct SecretStore {
    backend: Arc<dyn SecretBackend>,
    index: Option<IndexFile>,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretStore")
            .field("backend", &self.backend.kind())
            .field(
                "index",
                &self.index.as_ref().map(|i| i.path().to_path_buf()),
            )
            .finish()
    }
}

impl SecretStore {
    /// Wrap an explicit backend. `index_path` is where the non-secret
    /// index lives (`None` = no index; counts then come from the backend).
    pub fn with_backend(backend: Arc<dyn SecretBackend>, index_path: Option<PathBuf>) -> Self {
        Self {
            backend,
            index: index_path.map(IndexFile::new),
        }
    }

    /// A process-local store with no persistence (tests, tooling).
    pub fn in_memory() -> Self {
        Self::with_backend(Arc::new(MemoryBackend::new()), None)
    }

    /// Open the store selected by `settings`. An unusable selection yields
    /// a store whose operations fail with the reason, not a weaker store.
    pub fn open(settings: &StoreSettings) -> Self {
        let index = Some(IndexFile::in_dir(&settings.dir));
        let backend: Arc<dyn SecretBackend> = match settings.config.backend.resolved() {
            BackendChoice::Keychain => {
                if cfg!(any(target_os = "macos", target_os = "windows")) {
                    Arc::new(KeychainBackend::new())
                } else {
                    Arc::new(UnavailableBackend {
                        kind: BackendKind::Keychain,
                        reason: "this platform has no persistent OS keychain DBCrust can use; set secrets.backend = \"file\"".to_string(),
                    })
                }
            }
            BackendChoice::File | BackendChoice::Auto => {
                let cache = settings
                    .config
                    .file_unlock_cache
                    .then(platform_unlock_cache)
                    .flatten();
                Arc::new(
                    PassphraseFileBackend::in_dir(&settings.dir, KeySource::prompt())
                        .with_kdf_params(settings.kdf)
                        .with_unlock_cache(cache),
                )
            }
        };
        Self { backend, index }
    }

    pub fn backend(&self) -> &Arc<dyn SecretBackend> {
        &self.backend
    }

    pub fn backend_kind(&self) -> BackendKind {
        self.backend.kind()
    }

    pub fn index(&self) -> Option<&IndexFile> {
        self.index.as_ref()
    }

    // ---- connection secrets -------------------------------------------------

    /// The record for `binding`, verified against it.
    ///
    /// `Ok(None)` when nothing is stored; [`SecretError::TargetMismatch`]
    /// when a record exists under this id but is bound to another target.
    pub fn get_connection_secret(
        &self,
        binding: &TargetBinding,
    ) -> Result<Option<SecretRecord>, SecretError> {
        let id = binding.record_id();
        let Some(json) = self.backend.get(&id)? else {
            return Ok(None);
        };
        let record = SecretRecord::from_json(&json)?;
        if !record.bound_target.matches(binding) {
            return Err(SecretError::TargetMismatch(format!(
                "stored for {}, requested {}",
                record.bound_target.describe(),
                binding.describe()
            )));
        }
        Ok(Some(record))
    }

    /// Store (or update) the secret for `binding`. Existing
    /// `secret_options` not named in the argument are kept; the agent policy
    /// and creation time survive while the target stays the same.
    pub fn put_connection_secret(
        &self,
        binding: &TargetBinding,
        secret: &str,
        secret_options: BTreeMap<String, Zeroizing<String>>,
    ) -> Result<SecretRecord, SecretError> {
        self.write_connection(binding, |record| {
            record.secret = Zeroizing::new(secret.to_string());
            record.secret_options.extend(secret_options);
        })
    }

    /// Add or replace secret options on the record for `binding`, creating
    /// a secret-less record when the target has none yet (the password stays
    /// untouched when there is one).
    pub fn merge_connection_options(
        &self,
        binding: &TargetBinding,
        secret_options: BTreeMap<String, Zeroizing<String>>,
    ) -> Result<SecretRecord, SecretError> {
        self.write_connection(binding, |record| {
            record.secret_options.extend(secret_options)
        })
    }

    fn write_connection(
        &self,
        binding: &TargetBinding,
        apply: impl FnOnce(&mut SecretRecord),
    ) -> Result<SecretRecord, SecretError> {
        let binding = binding.normalized();
        let id = binding.record_id();
        let existing = match self.backend.get(&id) {
            Ok(Some(json)) => match SecretRecord::from_json(&json) {
                Ok(record) => Some(record),
                // A damaged record is repaired by the write.
                Err(SecretError::Corrupt(_)) => None,
                Err(other) => return Err(other),
            },
            Ok(None) => None,
            Err(SecretError::Corrupt(_)) => None,
            Err(other) => return Err(other),
        };
        let mut record = match existing {
            Some(mut previous) => {
                previous.revision += 1;
                if !previous.bound_target.matches(&binding) {
                    // Re-pointed at a changed target: the old ceiling does
                    // not carry over.
                    previous.agent_policy = None;
                }
                previous.bound_target = binding.clone();
                previous
            }
            None => SecretRecord::new("", BTreeMap::new(), binding.clone()),
        };
        apply(&mut record);
        self.put_record(&id, &record)?;
        Ok(record)
    }

    // ---- generic records (broker ids, tooling) ------------------------------

    /// Read one record by id (no binding check: the caller applies
    /// [`TargetBinding::matches`] against the target it resolved).
    pub fn get_record(&self, id: &str) -> Result<Option<SecretRecord>, SecretError> {
        match self.backend.get(id)? {
            Some(json) => SecretRecord::from_json(&json).map(Some),
            None => Ok(None),
        }
    }

    /// Write one record by id. `conn:` ids must be the id of the record's
    /// own binding.
    pub fn put_record(&self, id: &str, record: &SecretRecord) -> Result<(), SecretError> {
        if matches!(
            RecordId::parse(id).map(|parsed| parsed.kind),
            Some(RecordKind::Connection)
        ) && record.bound_target.record_id() != id
        {
            return Err(SecretError::TargetMismatch(format!(
                "record id {id} does not name {}",
                record.bound_target.describe()
            )));
        }
        let json = record.to_json()?;
        self.backend.put(id, &json)?;
        self.index_upsert(IndexEntry::for_record(id, record));
        Ok(())
    }

    // ---- named secrets (AI keys, OAuth tokens) ------------------------------

    fn check_named(id: &str) -> Result<(), SecretError> {
        match RecordId::parse(id).map(|parsed| parsed.kind) {
            Some(RecordKind::Ai | RecordKind::Oauth) => Ok(()),
            _ => Err(SecretError::Io(format!(
                "{id} is not an ai:/oauth: secret id"
            ))),
        }
    }

    /// Read an `ai:<provider>` / `oauth:<name>` secret.
    pub fn get_secret(&self, id: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        Self::check_named(id)?;
        self.backend.get(id)
    }

    /// Store an `ai:<provider>` / `oauth:<name>` secret.
    pub fn put_secret(&self, id: &str, value: &str) -> Result<(), SecretError> {
        Self::check_named(id)?;
        self.backend.put(id, value)?;
        self.index_upsert(IndexEntry::for_named(id));
        Ok(())
    }

    // ---- shared -------------------------------------------------------------

    /// Delete any record by id; `Ok(false)` when it did not exist.
    pub fn delete(&self, id: &str) -> Result<bool, SecretError> {
        let existed = self.backend.delete(id)?;
        if let Some(index) = &self.index {
            if let Err(e) = index.remove(id) {
                tracing::warn!("secrets index could not be updated: {e}");
            }
        }
        Ok(existed)
    }

    /// Non-secret inventory from the index (empty without one).
    pub fn indexed(&self) -> SecretsIndex {
        self.index
            .as_ref()
            .and_then(|index| index.load().ok())
            .unwrap_or_default()
    }

    /// Health, counts by kind and index drift. Never prompts, never reads
    /// secret values.
    pub fn status(&self) -> StoreStatus {
        let backend = self.backend.status();
        let (index_ids, index_error) = match &self.index {
            Some(index) => match index.load() {
                Ok(parsed) => (parsed.ids(), None),
                Err(e) => (Vec::new(), Some(e.to_string())),
            },
            None => (Vec::new(), None),
        };
        let backend_ids = match backend.state {
            BackendState::Unavailable => None,
            _ => self.backend.list_ids().ok().flatten(),
        };
        let mut counts = RecordCounts::default();
        let mut drift = IndexDrift {
            index_error,
            ..IndexDrift::default()
        };
        match &backend_ids {
            Some(ids) => {
                drift.checked = self.index.is_some();
                let in_backend: BTreeSet<&String> = ids.iter().collect();
                let in_index: BTreeSet<&String> = index_ids.iter().collect();
                for id in ids {
                    counts.add(id);
                }
                if self.index.is_some() && drift.index_error.is_none() {
                    drift.missing_in_index = in_backend
                        .difference(&in_index)
                        .map(|s| (*s).clone())
                        .collect();
                    drift.missing_in_backend = in_index
                        .difference(&in_backend)
                        .map(|s| (*s).clone())
                        .collect();
                }
            }
            None => {
                for id in &index_ids {
                    counts.add(id);
                }
            }
        }
        StoreStatus {
            backend,
            counts,
            index_path: self.index.as_ref().map(|i| i.path().to_path_buf()),
            drift,
        }
    }

    /// Unlock the backend interactively and cache the key.
    pub fn unlock(&self) -> Result<(), SecretError> {
        self.backend.unlock()
    }

    /// Forget any cached key.
    pub fn lock(&self) -> Result<(), SecretError> {
        self.backend.lock()
    }

    fn index_upsert(&self, entry: IndexEntry) {
        if let Some(index) = &self.index {
            if let Err(e) = index.upsert(entry) {
                tracing::warn!("secrets index could not be updated: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::testing::FaultyBackend;
    use crate::secrets::{AgentPolicy, MemoryBackend};

    fn binding() -> TargetBinding {
        TargetBinding::new("postgres", "db.example.com", None, "app", "alice", None)
    }

    #[test]
    fn connection_secret_round_trip_keeps_policy_and_bumps_revision() {
        let store = SecretStore::in_memory();
        assert!(store.get_connection_secret(&binding()).unwrap().is_none());

        let first = store
            .put_connection_secret(&binding(), "pw-1", BTreeMap::new())
            .unwrap();
        assert_eq!(first.revision, 1);

        // Attach a policy (what the broker does), then change the password.
        let mut with_policy = first.clone();
        with_policy.agent_policy = Some(AgentPolicy::default());
        store
            .put_record(&binding().record_id(), &with_policy)
            .unwrap();

        let mut options = BTreeMap::new();
        options.insert(
            "sslpassword".to_string(),
            Zeroizing::new("cert-pw".to_string()),
        );
        let second = store
            .put_connection_secret(&binding(), "pw-2", options)
            .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.agent_policy, Some(AgentPolicy::default()));
        assert_eq!(second.created_at, first.created_at);

        let loaded = store.get_connection_secret(&binding()).unwrap().unwrap();
        assert_eq!(&*loaded.secret, "pw-2");
        assert_eq!(&*loaded.secret_options["sslpassword"], "cert-pw");

        // Options merge: re-saving the password keeps earlier options.
        store
            .put_connection_secret(&binding(), "pw-3", BTreeMap::new())
            .unwrap();
        let loaded = store.get_connection_secret(&binding()).unwrap().unwrap();
        assert_eq!(&*loaded.secret_options["sslpassword"], "cert-pw");
    }

    #[test]
    fn mismatched_binding_is_target_mismatch_never_used() {
        let backend = Arc::new(MemoryBackend::new());
        let store = SecretStore::with_backend(backend.clone(), None);
        // A record for another host planted under this target's id.
        let evil = TargetBinding::new("postgres", "evil.example.com", None, "app", "alice", None);
        let planted = SecretRecord::new("CANARY-PW", BTreeMap::new(), evil);
        backend
            .put(&binding().record_id(), &planted.to_json().unwrap())
            .unwrap();

        let err = store.get_connection_secret(&binding()).unwrap_err();
        assert!(matches!(err, SecretError::TargetMismatch(_)), "{err:?}");
        assert!(!err.to_string().contains("CANARY"));
    }

    #[test]
    fn put_record_rejects_a_conn_id_that_is_not_the_records_own() {
        let store = SecretStore::in_memory();
        let record = SecretRecord::new("pw", BTreeMap::new(), binding());
        let other = TargetBinding::new("postgres", "other", None, "app", "alice", None);
        let err = store.put_record(&other.record_id(), &record).unwrap_err();
        assert!(matches!(err, SecretError::TargetMismatch(_)));
        // broker ids are free-form
        store.put_record("broker:prod", &record).unwrap();
        assert!(store.get_record("broker:prod").unwrap().is_some());
    }

    #[test]
    fn named_secrets_only_accept_ai_and_oauth_ids() {
        let store = SecretStore::in_memory();
        store.put_secret("ai:anthropic", "sk-test").unwrap();
        assert_eq!(
            &*store.get_secret("ai:anthropic").unwrap().unwrap(),
            "sk-test"
        );
        assert!(store.put_secret("conn:x", "v").is_err());
        assert!(store.get_secret("broker:x").is_err());
        assert!(store.delete("ai:anthropic").unwrap());
        assert!(store.get_secret("ai:anthropic").unwrap().is_none());
    }

    #[test]
    fn index_stays_consistent_and_status_reports_drift() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemoryBackend::new());
        let store =
            SecretStore::with_backend(backend.clone(), Some(dir.path().join("secrets_index.toml")));
        store
            .put_connection_secret(&binding(), "pw", BTreeMap::new())
            .unwrap();
        store.put_secret("ai:openai", "sk").unwrap();

        let status = store.status();
        assert_eq!(status.counts.connection, 1);
        assert_eq!(status.counts.ai, 1);
        assert!(status.drift.checked);
        assert!(status.drift.is_clean(), "{:?}", status.drift);

        // Drift: a record vanishes from the backend, another appears.
        backend.delete("ai:openai").unwrap();
        backend.put("oauth:stray", "v").unwrap();
        let status = store.status();
        assert_eq!(
            status.drift.missing_in_backend,
            vec!["ai:openai".to_string()]
        );
        assert_eq!(
            status.drift.missing_in_index,
            vec!["oauth:stray".to_string()]
        );

        // Delete keeps the index in sync.
        assert!(store.delete(&binding().record_id()).unwrap());
        assert!(store.indexed().get(&binding().record_id()).is_none());

        let text = std::fs::read_to_string(dir.path().join("secrets_index.toml")).unwrap();
        assert!(!text.contains("pw\"") && !text.contains("sk"));
    }

    #[test]
    fn backend_failures_are_errors_not_misses() {
        let faulty = Arc::new(FaultyBackend::new(MemoryBackend::new()));
        let store = SecretStore::with_backend(faulty.clone(), None);
        store
            .put_connection_secret(&binding(), "pw", BTreeMap::new())
            .unwrap();
        faulty.fail_get_with(SecretError::Locked("locked".to_string()));
        let err = store.get_connection_secret(&binding()).unwrap_err();
        assert!(err.is_locked());
        // And a failed write does not look like success.
        faulty.fail_put_with(SecretError::Unavailable("down".to_string()));
        assert!(
            store
                .put_connection_secret(&binding(), "pw2", BTreeMap::new())
                .is_err()
        );
    }

    #[test]
    fn explicit_keychain_on_unsupported_platform_fails_closed() {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::open(&StoreSettings::new(
            SecretsConfig {
                backend: BackendChoice::Keychain,
                file_unlock_cache: false,
            },
            dir.path().to_path_buf(),
        ));
        let err = store.get_connection_secret(&binding()).unwrap_err();
        assert!(matches!(err, SecretError::Unavailable(_)));
        assert!(!dir.path().join("secrets.enc").exists());
    }

    #[test]
    fn config_serde_defaults() {
        let parsed: SecretsConfig = toml::from_str("").unwrap();
        assert_eq!(parsed, SecretsConfig::default());
        let parsed: SecretsConfig = toml::from_str("backend = \"file\"").unwrap();
        assert_eq!(parsed.backend, BackendChoice::File);
        assert!(parsed.file_unlock_cache);
        assert!(toml::from_str::<SecretsConfig>("backend = \"cloud\"").is_err());
    }
}
