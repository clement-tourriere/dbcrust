//! Alias registry: `aliases.toml` descriptors + the authoritative records.
//!
//! `aliases.toml` holds only non-secret *descriptors* (alias, record id,
//! engine, description, enrollment time). Everything the broker acts on — the
//! target, the credential and the policy ceiling — comes from the
//! [`SecretRecord`] stored under `broker:<alias>`. If a descriptor disagrees
//! with its record (someone edited the file) the alias is refused.
//!
//! The registry re-reads `aliases.toml` when its mtime/size changes and
//! re-reads records at most once a minute (and whenever the file changed), so
//! `enroll`/`revoke`/`policy` run from another process take effect without a
//! restart.

use dbcrust_secrets::{AgentPolicy, RecordId, SecretError, SecretRecord, SecretStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::BrokerError;

/// How long a record read from the store is trusted without a file change.
const RECORD_TTL: Duration = Duration::from_secs(60);
const MAX_ALIAS_FILE_BYTES: u64 = 1024 * 1024;

/// True for `^[a-z0-9][a-z0-9_-]{0,62}$`.
pub fn is_valid_alias(alias: &str) -> bool {
    let mut chars = alias.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    alias.len() <= 63
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A non-secret alias descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasEntry {
    pub alias: String,
    pub record_id: String,
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub enrolled_at: String,
    /// Display only; the record's own revision is authoritative.
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasFile {
    #[serde(default, rename = "alias")]
    pub aliases: Vec<AliasEntry>,
}

impl AliasFile {
    pub fn load(path: &Path) -> Result<Self, BrokerError> {
        let bytes = dbcrust_secrets::fsutil::read_bounded(path, MAX_ALIAS_FILE_BYTES)
            .map_err(|e| BrokerError::State(e.to_string()))?;
        let Some(bytes) = bytes else {
            return Ok(Self::default());
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| BrokerError::State(format!("{} is not UTF-8", path.display())))?;
        toml::from_str(&text).map_err(|e| BrokerError::State(format!("{}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<(), BrokerError> {
        let body = toml::to_string(self)
            .map_err(|e| BrokerError::State(format!("could not serialize aliases: {e}")))?;
        let text = format!(
            "# Managed by `dbcrust broker`. Non-secret descriptors only: targets,\n\
             # credentials and policy live in the secret store and cannot be\n\
             # changed by editing this file.\n\n{body}"
        );
        dbcrust_secrets::fsutil::write_private_atomic(path, text.as_bytes())
            .map_err(|e| BrokerError::State(e.to_string()))
    }

    pub fn get(&self, alias: &str) -> Option<&AliasEntry> {
        self.aliases.iter().find(|entry| entry.alias == alias)
    }

    pub fn upsert(&mut self, entry: AliasEntry) {
        match self.aliases.iter_mut().find(|e| e.alias == entry.alias) {
            Some(existing) => *existing = entry,
            None => self.aliases.push(entry),
        }
        self.aliases.sort_by(|a, b| a.alias.cmp(&b.alias));
    }

    pub fn remove(&mut self, alias: &str) -> bool {
        let before = self.aliases.len();
        self.aliases.retain(|entry| entry.alias != alias);
        self.aliases.len() != before
    }
}

/// A usable alias: validated descriptor + record + clamped policy.
pub struct ResolvedAlias {
    pub alias: String,
    pub entry: AliasEntry,
    pub record: SecretRecord,
    /// The policy ceiling from the record, clamped to the hard limits.
    pub policy: AgentPolicy,
}

impl std::fmt::Debug for ResolvedAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAlias")
            .field("alias", &self.alias)
            .field("target", &self.record.bound_target.describe())
            .field("policy", &self.policy)
            .field("revision", &self.record.revision)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// Not enrolled, revoked, or no record behind the descriptor.
    Unknown,
    /// Present but unusable: tampered descriptor, no agent policy, bad engine.
    Denied(String),
    /// The secret store is locked.
    Locked(String),
    /// The store or the alias file failed.
    Unavailable(String),
}

struct Cache {
    stamp: Option<(SystemTime, u64)>,
    file: AliasFile,
    resolved: HashMap<String, (Instant, Arc<ResolvedAlias>)>,
}

pub struct Registry {
    aliases_path: PathBuf,
    store: Arc<SecretStore>,
    cache: Mutex<Cache>,
}

fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The engines a broker alias may use in v1.
pub fn engine_is_supported(engine: &str) -> bool {
    matches!(
        dbcrust_secrets::normalize_engine(engine).as_str(),
        "postgresql" | "mysql" | "sqlite" | "clickhouse" | "duckdb"
    )
}

impl Registry {
    pub fn new(aliases_path: PathBuf, store: Arc<SecretStore>) -> Self {
        Self {
            aliases_path,
            store,
            cache: Mutex::new(Cache {
                stamp: None,
                file: AliasFile::default(),
                resolved: HashMap::new(),
            }),
        }
    }

    pub fn store(&self) -> &Arc<SecretStore> {
        &self.store
    }

    pub fn aliases_path(&self) -> &Path {
        &self.aliases_path
    }

    fn refresh(&self, cache: &mut Cache) -> Result<(), ResolveError> {
        let stamp = file_stamp(&self.aliases_path);
        if cache.stamp.is_some() && cache.stamp == stamp {
            return Ok(());
        }
        let file = AliasFile::load(&self.aliases_path)
            .map_err(|e| ResolveError::Unavailable(e.to_string()))?;
        cache.file = file;
        cache.stamp = stamp;
        cache.resolved.clear();
        Ok(())
    }

    fn load_record(&self, entry: &AliasEntry) -> Result<ResolvedAlias, ResolveError> {
        let expected_id = RecordId::broker(&entry.alias);
        if entry.record_id != expected_id {
            tracing::warn!(
                "alias {} refused: descriptor names a different record than broker:<alias>",
                entry.alias
            );
            return Err(ResolveError::Denied(
                "alias descriptor does not match its secret record".to_string(),
            ));
        }
        let record = match self.store.get_record(&expected_id) {
            Ok(Some(record)) => record,
            Ok(None) => return Err(ResolveError::Unknown),
            Err(SecretError::Locked(m)) => return Err(ResolveError::Locked(m)),
            Err(e) => return Err(ResolveError::Unavailable(e.to_string())),
        };
        let Some(policy) = record.agent_policy else {
            return Err(ResolveError::Denied(
                "this connection is not enrolled for agent use".to_string(),
            ));
        };
        let record_engine = dbcrust_secrets::normalize_engine(&record.bound_target.engine);
        if record_engine != dbcrust_secrets::normalize_engine(&entry.engine) {
            tracing::warn!(
                "alias {} refused: aliases.toml engine disagrees with the stored record",
                entry.alias
            );
            return Err(ResolveError::Denied(
                "alias descriptor does not match its secret record".to_string(),
            ));
        }
        if !engine_is_supported(&record_engine) {
            return Err(ResolveError::Denied(format!(
                "engine {record_engine} is not supported by the broker"
            )));
        }
        Ok(ResolvedAlias {
            alias: entry.alias.clone(),
            entry: entry.clone(),
            policy: policy.clamped(),
            record,
        })
    }

    /// Resolve one alias (blocking: may touch the keychain).
    pub fn resolve(&self, alias: &str) -> Result<Arc<ResolvedAlias>, ResolveError> {
        if !is_valid_alias(alias) {
            return Err(ResolveError::Unknown);
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        self.refresh(&mut cache)?;
        if let Some((at, resolved)) = cache.resolved.get(alias) {
            if at.elapsed() < RECORD_TTL {
                return Ok(resolved.clone());
            }
        }
        let entry = cache
            .file
            .get(alias)
            .cloned()
            .ok_or(ResolveError::Unknown)?;
        match self.load_record(&entry) {
            Ok(resolved) => {
                let resolved = Arc::new(resolved);
                cache
                    .resolved
                    .insert(alias.to_string(), (Instant::now(), resolved.clone()));
                Ok(resolved)
            }
            Err(e) => {
                cache.resolved.remove(alias);
                Err(e)
            }
        }
    }

    /// Every alias that currently resolves, sorted by name. Unusable aliases
    /// are omitted (they are reported by `broker status`).
    pub fn list(&self) -> Result<Vec<Arc<ResolvedAlias>>, ResolveError> {
        let names: Vec<String> = {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            self.refresh(&mut cache)?;
            cache.file.aliases.iter().map(|e| e.alias.clone()).collect()
        };
        let mut out = Vec::new();
        for name in names {
            match self.resolve(&name) {
                Ok(resolved) => out.push(resolved),
                Err(ResolveError::Locked(m)) => return Err(ResolveError::Locked(m)),
                Err(_) => {}
            }
        }
        out.sort_by(|a, b| a.alias.cmp(&b.alias));
        Ok(out)
    }

    /// Descriptors as written in `aliases.toml` (no record access).
    pub fn descriptors(&self) -> Result<Vec<AliasEntry>, ResolveError> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        self.refresh(&mut cache)?;
        Ok(cache.file.aliases.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcrust_secrets::{AgentAccess, TargetBinding};
    use rstest::rstest;
    use std::collections::BTreeMap;

    #[rstest]
    #[case("prod", true)]
    #[case("a", true)]
    #[case("0day", true)]
    #[case("my-db_2", true)]
    #[case(&"a".repeat(63), true)]
    #[case(&"a".repeat(64), false)]
    #[case("", false)]
    #[case("Prod", false)]
    #[case("-db", false)]
    #[case("_db", false)]
    #[case("a b", false)]
    #[case("a/b", false)]
    #[case("a.b", false)]
    #[case("../x", false)]
    #[case("pr\u{f6}d", false)]
    fn alias_syntax(#[case] alias: &str, #[case] valid: bool) {
        assert_eq!(is_valid_alias(alias), valid, "{alias:?}");
    }

    fn record(engine: &str, access: Option<AgentAccess>) -> SecretRecord {
        let mut record = SecretRecord::new(
            "pw",
            BTreeMap::new(),
            TargetBinding::new(engine, "db.example", None, "app", "agent", None),
        );
        record.agent_policy = access.map(|access| AgentPolicy {
            access,
            ..AgentPolicy::default()
        });
        record
    }

    fn entry(alias: &str, engine: &str) -> AliasEntry {
        AliasEntry {
            alias: alias.to_string(),
            record_id: RecordId::broker(alias),
            engine: engine.to_string(),
            description: Some("test".to_string()),
            enrolled_at: "2026-01-01T00:00:00Z".to_string(),
            revision: 1,
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        store: Arc<SecretStore>,
        registry: Registry,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aliases.toml");
        let store = Arc::new(SecretStore::in_memory());
        let registry = Registry::new(path.clone(), store.clone());
        Fixture {
            _dir: dir,
            path,
            store,
            registry,
        }
    }

    fn enroll(f: &Fixture, alias: &str, engine: &str, access: Option<AgentAccess>) {
        f.store
            .put_record(&RecordId::broker(alias), &record(engine, access))
            .unwrap();
        let mut file = AliasFile::load(&f.path).unwrap();
        file.upsert(entry(alias, engine));
        file.save(&f.path).unwrap();
    }

    #[test]
    fn resolves_enrolled_alias_with_policy_from_the_record() {
        let f = fixture();
        enroll(&f, "prod", "postgres", Some(AgentAccess::ReadOnly));
        let resolved = f.registry.resolve("prod").unwrap();
        assert_eq!(resolved.policy.access, AgentAccess::ReadOnly);
        assert_eq!(resolved.record.bound_target.host, "db.example");
        assert!(matches!(
            f.registry.resolve("nope"),
            Err(ResolveError::Unknown)
        ));
        assert!(matches!(
            f.registry.resolve("../x"),
            Err(ResolveError::Unknown)
        ));
    }

    #[test]
    fn revoked_alias_is_unknown_even_if_the_descriptor_remains() {
        let f = fixture();
        enroll(&f, "prod", "postgres", Some(AgentAccess::ReadOnly));
        f.registry.resolve("prod").unwrap();
        f.store.delete(&RecordId::broker("prod")).unwrap();
        // Force a cache refresh the way a CLI write would
        let mut file = AliasFile::load(&f.path).unwrap();
        file.upsert(AliasEntry {
            revision: 2,
            ..entry("prod", "postgres")
        });
        file.save(&f.path).unwrap();
        assert!(matches!(
            f.registry.resolve("prod"),
            Err(ResolveError::Unknown)
        ));
    }

    #[test]
    fn editing_the_descriptor_cannot_change_engine_or_policy() {
        let f = fixture();
        enroll(&f, "prod", "postgres", Some(AgentAccess::ReadOnly));
        // Tamper: claim another engine in aliases.toml
        let mut file = AliasFile::load(&f.path).unwrap();
        file.upsert(entry("prod", "mysql"));
        file.save(&f.path).unwrap();
        assert!(matches!(
            f.registry.resolve("prod"),
            Err(ResolveError::Denied(_))
        ));
    }

    #[test]
    fn descriptor_pointing_at_another_record_is_denied() {
        let f = fixture();
        enroll(&f, "prod", "postgres", Some(AgentAccess::ReadOnly));
        enroll(&f, "other", "postgres", Some(AgentAccess::ReadWrite));
        let mut file = AliasFile::load(&f.path).unwrap();
        let mut stolen = entry("prod", "postgres");
        stolen.record_id = RecordId::broker("other");
        file.upsert(stolen);
        file.save(&f.path).unwrap();
        assert!(matches!(
            f.registry.resolve("prod"),
            Err(ResolveError::Denied(_))
        ));
    }

    #[test]
    fn extra_descriptor_keys_are_ignored() {
        let f = fixture();
        enroll(&f, "prod", "postgres", Some(AgentAccess::ReadOnly));
        let text = std::fs::read_to_string(&f.path).unwrap();
        std::fs::write(
            &f.path,
            text.replace(
                "engine =",
                "host = \"evil.example\"\naccess = \"read_write\"\nengine =",
            ),
        )
        .unwrap();
        let resolved = f.registry.resolve("prod").unwrap();
        assert_eq!(resolved.record.bound_target.host, "db.example");
        assert_eq!(resolved.policy.access, AgentAccess::ReadOnly);
    }

    #[test]
    fn record_without_agent_policy_is_denied() {
        let f = fixture();
        enroll(&f, "prod", "postgres", None);
        assert!(matches!(
            f.registry.resolve("prod"),
            Err(ResolveError::Denied(_))
        ));
    }

    #[test]
    fn unsupported_engine_is_denied() {
        let f = fixture();
        enroll(&f, "docs", "mongodb", Some(AgentAccess::ReadOnly));
        assert!(matches!(
            f.registry.resolve("docs"),
            Err(ResolveError::Denied(_))
        ));
    }

    #[test]
    fn list_returns_only_usable_aliases_sorted() {
        let f = fixture();
        enroll(&f, "b", "mysql", Some(AgentAccess::ReadOnly));
        enroll(&f, "a", "sqlite", Some(AgentAccess::ReadWrite));
        enroll(&f, "c", "postgres", None);
        let names: Vec<String> = f
            .registry
            .list()
            .unwrap()
            .iter()
            .map(|r| r.alias.clone())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn alias_file_round_trips_without_secrets() {
        let f = fixture();
        enroll(&f, "prod", "postgres", Some(AgentAccess::ReadOnly));
        let text = std::fs::read_to_string(&f.path).unwrap();
        assert!(!text.contains("pw"));
        let file = AliasFile::load(&f.path).unwrap();
        assert_eq!(file.get("prod").unwrap().engine, "postgres");
        let mut file = file;
        assert!(file.remove("prod"));
        assert!(!file.remove("prod"));
    }
}
