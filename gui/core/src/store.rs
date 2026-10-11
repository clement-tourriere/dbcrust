//! Single owner of DBCrust's config and user-data files for the desktop app.
//!
//! `dbcrust::config::Config::load()` is not safe to run concurrently: a load
//! that overlaps another one silently returns default settings
//! (`config.rs`, `CONFIG_LOADING_IN_PROGRESS`), so every load runs under
//! [`config_load_lock`]. The CLI's `Database` constructors call it
//! internally; the GUI never lets them: connecting builds the `Database`
//! from a snapshot of this store (`Database::from_*_with_config`), so no
//! handshake ever happens under this lock (it is held for a file read only).
//! Mutations go through one shared `Config` so the GUI never writes two
//! divergent copies.

use crate::error::{GuiError, GuiResult};
use chrono::{DateTime, Utc};
use dbcrust::config::{Config, NamedQueriesStorage, NamedQueryScope, RecentConnection};
use dbcrust::database::{DatabaseType, DatabaseTypeExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

static CONFIG_LOAD_LOCK: Mutex<()> = Mutex::new(());

/// Serialises every code path that may call `Config::load()`. Hold it only
/// around the load itself, never across a network operation. Poison-tolerant:
/// a panic elsewhere must not brick the app.
pub fn config_load_lock() -> MutexGuard<'static, ()> {
    CONFIG_LOAD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Shared handle to the app's `Config`. Cheap to clone.
#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<Mutex<Config>>,
    /// Bumped by every [`ConfigStore::update`]; lets a reload detect that
    /// the store changed while it was reading files.
    revision: Arc<AtomicU64>,
    /// Orders `config.toml` writes without holding `inner` during I/O.
    save_lock: Arc<Mutex<()>>,
    /// Revision of the newest settings change not yet written to
    /// `config.toml` (0 = everything saved). Reload never replaces those.
    unsaved_settings: Arc<AtomicU64>,
}

impl ConfigStore {
    /// Load from disk (honours `DBCRUST_CONFIG_DIR`). Blocking file I/O:
    /// call off the UI thread when possible.
    pub fn load() -> Self {
        let config = {
            let _guard = config_load_lock();
            Config::load()
        };
        Self::from_config(config)
    }

    /// Wrap an in-memory config (tests, screenshot fixtures).
    pub fn from_config(config: Config) -> Self {
        Self {
            inner: Arc::new(Mutex::new(config)),
            revision: Arc::new(AtomicU64::new(0)),
            save_lock: Arc::new(Mutex::new(())),
            unsaved_settings: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Re-read every file from disk so CLI-side changes (new sessions,
    /// recents, named queries) become visible. Blocking file I/O.
    ///
    /// If the store was modified while the files were being read, the fresh
    /// copy may predate that change: it is discarded (the next reload picks
    /// up external changes) rather than overwriting it. Returns whether the
    /// store was replaced.
    pub fn reload(&self) -> bool {
        // Read the revision and the unsaved marker together, under the store
        // lock that every update holds while changing them
        let before = {
            let _config = self.lock();
            if self.unsaved_settings.load(Ordering::SeqCst) != 0 {
                tracing::debug!("config reload skipped: settings changes are not saved yet");
                return false;
            }
            self.revision.load(Ordering::SeqCst)
        };
        let fresh = {
            let _guard = config_load_lock();
            Config::load()
        };
        let mut current = self.lock();
        // Any update since `before` (saved or not) may be newer than what
        // was just read: keep the in-memory state
        if self.revision.load(Ordering::SeqCst) != before
            || self.unsaved_settings.load(Ordering::SeqCst) != 0
        {
            tracing::debug!("config reload skipped: the store changed meanwhile");
            return false;
        }
        *current = fresh;
        true
    }

    fn lock(&self) -> MutexGuard<'_, Config> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Read access. Keep the closure short: it holds the store lock.
    pub fn read<R>(&self, f: impl FnOnce(&Config) -> R) -> R {
        f(&self.lock())
    }

    /// Like [`ConfigStore::read`], but never waits: `None` while another
    /// thread holds the store (e.g. a storage helper writing a file). For
    /// render paths, which must not block the UI thread.
    pub fn try_read<R>(&self, f: impl FnOnce(&Config) -> R) -> Option<R> {
        match self.inner.try_lock() {
            Ok(config) => Some(f(&config)),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(f(&poisoned.into_inner())),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// Mutate the in-memory config. Persisting is the caller's job (use
    /// [`ConfigStore::save_settings`] for `config.toml` fields; the storage
    /// helpers on `Config` persist their own files).
    pub fn update<R>(&self, f: impl FnOnce(&mut Config) -> R) -> R {
        let mut config = self.lock();
        let result = f(&mut config);
        self.revision.fetch_add(1, Ordering::SeqCst);
        result
    }

    /// Change `config.toml` settings in memory; they count as unsaved (and
    /// are protected from [`ConfigStore::reload`]) until
    /// [`ConfigStore::save_settings`] succeeds.
    pub fn update_settings<R>(&self, f: impl FnOnce(&mut Config) -> R) -> R {
        let mut config = self.lock();
        let result = f(&mut config);
        let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
        self.unsaved_settings.store(revision, Ordering::SeqCst);
        result
    }

    /// Owned copy of the current config.
    pub fn snapshot(&self) -> Config {
        self.lock().clone()
    }

    /// Persist `config.toml`, keeping the documentation comments (never use
    /// plain `Config::save`, which strips them).
    pub fn save_settings(&self) -> GuiResult<()> {
        // Snapshot under the store lock, write outside it: UI reads never
        // wait for disk I/O; `save_lock` keeps concurrent saves ordered.
        let _saving = self
            .save_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (snapshot, revision) = {
            let config = self.lock();
            (config.clone(), self.revision.load(Ordering::SeqCst))
        };
        snapshot
            .save_with_documentation()
            .map_err(|e| GuiError::Storage(format!("Could not save settings: {e}")))?;
        // Saved up to `revision`: clear the marker unless a newer settings
        // change arrived meanwhile
        let mut unsaved = self.unsaved_settings.load(Ordering::SeqCst);
        while unsaved != 0 && unsaved <= revision {
            match self.unsaved_settings.compare_exchange(
                unsaved,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(current) => unsaved = current,
            }
        }
        Ok(())
    }

    // ── Saved sessions ──────────────────────────────────────────────────

    pub fn sessions(&self) -> Vec<SessionEntry> {
        self.read(|config| {
            let mut sessions: Vec<SessionEntry> = config
                .list_sessions()
                .into_iter()
                .map(|(name, session)| SessionEntry {
                    target: session.reconnect_target(),
                    db_type: session.database_type.clone(),
                    user: session.user.clone(),
                    database: session.dbname.clone(),
                    name,
                })
                .collect();
            sessions.sort_by_key(|s| s.name.to_lowercase());
            sessions
        })
    }

    pub fn delete_session(&self, name: &str) -> GuiResult<bool> {
        self.update(|config| config.delete_session(name))
            .map_err(|e| GuiError::Storage(format!("Could not delete session \"{name}\": {e}")))
    }

    // ── Recent connections ──────────────────────────────────────────────

    pub fn recent_connections(&self) -> Vec<RecentEntry> {
        self.read(|config| {
            config
                .get_recent_connections()
                .iter()
                .map(|recent| RecentEntry {
                    display_name: recent.display_name.clone(),
                    url: recent.connection_url.clone(),
                    db_type: recent.database_type.clone(),
                    timestamp: recent.timestamp,
                    connection: recent.clone(),
                })
                .collect()
        })
    }

    /// Remove the recent connection with identity `key` (see
    /// [`crate::launcher::recent_key`]). One store mutation, like recording a
    /// recent entry: memory and `recent.toml` change together under the
    /// store's lock, so a removal can never interleave with (or be undone
    /// by) a connection recording its own entry.
    pub fn remove_recent(&self, key: &str) -> GuiResult<bool> {
        self.update(|config| -> Result<bool, Box<dyn std::error::Error>> {
            let urls: Vec<String> = config
                .get_recent_connections()
                .iter()
                .filter(|recent| crate::launcher::recent_key(&recent.connection_url) == key)
                .map(|recent| recent.connection_url.clone())
                .collect();
            let mut removed = false;
            for url in urls {
                removed |= config.remove_recent_connection(&url)?;
            }
            Ok(removed)
        })
        .map_err(|e| GuiError::Storage(format!("Could not remove the recent connection: {e}")))
    }

    // ── Named queries ───────────────────────────────────────────────────

    /// Named queries visible for `db_type` (global + that engine's scope),
    /// plus legacy unscoped entries not shadowed by a scoped one.
    pub fn named_queries(&self, db_type: Option<&DatabaseType>) -> Vec<NamedQueryEntry> {
        self.read(|config| {
            let mut entries: Vec<NamedQueryEntry> = config
                .list_available_named_queries(db_type, None)
                .into_iter()
                .map(|(name, query, scope)| NamedQueryEntry {
                    key: NamedQueriesStorage::generate_key(&name, &scope),
                    scope_label: scope_label(&scope),
                    name,
                    query,
                })
                .collect();
            for (name, query) in config.list_named_queries() {
                if !entries.iter().any(|entry| entry.name == name) {
                    entries.push(NamedQueryEntry {
                        key: format!("{LEGACY_PREFIX}{name}"),
                        scope_label: "global".to_string(),
                        name,
                        query,
                    });
                }
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.scope_label.cmp(&b.scope_label)));
            entries
        })
    }

    /// The text of the saved query `name` saved for `engine` (`None`: the
    /// one available to every connection).
    pub fn named_query(&self, name: &str, engine: Option<&DatabaseType>) -> Option<String> {
        let scope = engine.map_or(NamedQueryScope::Global, |engine| {
            NamedQueryScope::DatabaseType(engine.clone())
        });
        self.read(|config| {
            config
                .list_available_named_queries(engine, None)
                .into_iter()
                .find(|(candidate, _, candidate_scope)| {
                    candidate == name && *candidate_scope == scope
                })
                .map(|(_, query, _)| query)
        })
    }

    /// Save (or overwrite) a named query. `db_type: None` saves it globally.
    pub fn save_named_query(
        &self,
        name: &str,
        query: &str,
        db_type: Option<DatabaseType>,
    ) -> GuiResult<()> {
        let scope = match db_type {
            Some(db_type) => NamedQueryScope::DatabaseType(db_type),
            None => NamedQueryScope::Global,
        };
        self.update(|config| config.add_named_query_with_scope(name, query, scope))
            .map_err(|e| GuiError::Storage(format!("Could not save query \"{name}\": {e}")))
    }

    /// Delete by the `key` of a [`NamedQueryEntry`].
    pub fn delete_named_query(&self, key: &str) -> GuiResult<bool> {
        self.update(|config| {
            if let Some(name) = key.strip_prefix(LEGACY_PREFIX) {
                return config.delete_named_query(name).map_err(|e| e.to_string());
            }
            let Some(entry) = config.get_named_query_with_scope(key) else {
                return Ok(false);
            };
            let scope = entry.scope.clone();
            // Keys are `<scope prefix>::<name>`; the scope prefix itself never
            // contains the name, so strip it instead of splitting on `::`
            // (names may contain `::`).
            let prefix_len = NamedQueriesStorage::generate_key("", &scope).len();
            let name = &key[prefix_len.min(key.len())..];
            config
                .delete_named_query_with_scope(name, &scope)
                .map_err(|e| e.to_string())
        })
        .map_err(|e| GuiError::Storage(format!("Could not delete query: {e}")))
    }
}

const LEGACY_PREFIX: &str = "legacy::";

fn scope_label(scope: &NamedQueryScope) -> String {
    match scope {
        NamedQueryScope::Global => "global".to_string(),
        NamedQueryScope::DatabaseType(db_type) => db_type.display_name().to_string(),
        NamedQueryScope::Session(session) => format!("session {session}"),
    }
}

/// A saved session (`sessions.toml`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntry {
    pub name: String,
    /// Human-readable reconnect target (`user@host:port/db`, a file path,
    /// `Docker: name`, `vault://…`). Never contains a password.
    pub target: String,
    pub db_type: DatabaseType,
    pub user: String,
    pub database: String,
}

/// A recent connection (`recent.toml`). Passwords are never stored.
#[derive(Debug, Clone)]
pub struct RecentEntry {
    pub display_name: String,
    /// Sanitized URL as stored (the identity of the entry).
    pub url: String,
    pub db_type: DatabaseType,
    pub timestamp: DateTime<Utc>,
    /// The stored record, used to rebuild a connectable URL on a worker.
    pub connection: RecentConnection,
}

/// A named query visible to the current connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedQueryEntry {
    /// Stable storage key (pass back to [`ConfigStore::delete_named_query`]).
    pub key: String,
    pub name: String,
    pub query: String,
    /// "global", an engine name, or "session …".
    pub scope_label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_never_overwrites_a_concurrent_update() {
        let store = ConfigStore::from_config(Config::default());
        let before = store.revision.load(Ordering::SeqCst);
        store.update(|config| config.default_limit = 4242);
        assert_eq!(store.revision.load(Ordering::SeqCst), before + 1);

        // Simulate a reload that started before the update: it must back off
        let fresh = Config::default();
        let mut current = store.lock();
        let replaced = store.revision.load(Ordering::SeqCst) == before;
        if replaced {
            *current = fresh;
        }
        drop(current);
        assert!(!replaced);
        assert_eq!(store.read(|config| config.default_limit), 4242);
    }

    #[test]
    fn unsaved_settings_block_reload() {
        let store = ConfigStore::from_config(Config::default());
        store.update_settings(|config| config.default_limit = 7);
        assert_ne!(store.unsaved_settings.load(Ordering::SeqCst), 0);
        assert!(!store.reload(), "reload must not drop unsaved settings");
        assert_eq!(store.read(|config| config.default_limit), 7);
    }

    #[test]
    fn removing_a_recent_changes_memory_and_disk_together() {
        let store = ConfigStore::from_config(Config::default());
        let add = |url: &str| {
            store
                .update(|config| {
                    config.add_recent_connection_with_options(
                        url.to_string(),
                        DatabaseType::PostgreSQL,
                        true,
                        Default::default(),
                    )
                })
                .expect("record");
        };
        add("postgres://a@removal-one:5432/x");
        add("postgres://b@removal-two:5432/y");
        assert_eq!(store.recent_connections().len(), 2);

        let key = crate::launcher::recent_key("postgres://a@removal-one:5432/x");
        assert!(store.remove_recent(&key).expect("remove"));
        let left: Vec<String> = store
            .recent_connections()
            .into_iter()
            .map(|recent| recent.url)
            .collect();
        assert_eq!(left, ["postgres://b@removal-two:5432/y"]);
        // ...and the file no longer has it: a later reload cannot bring it
        // back. (Only the removed entry is checked: tests running in
        // parallel share this process's recent.toml and may rewrite it.)
        let path = Config::get_recent_connections_path().expect("path");
        let on_disk = std::fs::read_to_string(path).expect("recent.toml");
        assert!(!on_disk.contains("@removal-one:5432"), "{on_disk}");
        // Nothing left to remove
        assert!(!store.remove_recent(&key).expect("again"));
        // A connection recording itself afterwards does not resurrect it
        add("postgres://c@removal-three:5432/z");
        assert_eq!(store.recent_connections().len(), 2);
        assert!(
            !store
                .recent_connections()
                .iter()
                .any(|recent| recent.url.contains("@removal-one"))
        );
    }

    #[test]
    fn scope_labels_are_readable() {
        assert_eq!(scope_label(&NamedQueryScope::Global), "global");
        assert_eq!(
            scope_label(&NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL)),
            "PostgreSQL"
        );
    }

    #[test]
    fn named_query_key_prefix_strips_cleanly() {
        let scope = NamedQueryScope::DatabaseType(DatabaseType::SQLite);
        let key = NamedQueriesStorage::generate_key("a::b", &scope);
        let prefix_len = NamedQueriesStorage::generate_key("", &scope).len();
        assert_eq!(&key[prefix_len..], "a::b");
    }
}
