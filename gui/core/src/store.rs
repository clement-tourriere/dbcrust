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
use dbcrust::config::{
    Config, NamedQueriesStorage, NamedQuery, NamedQueryScope, RecentConnection, StorageFile,
};
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
    /// One saved-query change at a time, from the in-memory change to the
    /// end of its `named_queries.toml` write (`inner` is not held during
    /// the write), so a failed write can be rolled back exactly.
    queries_save_lock: Arc<Mutex<()>>,
    /// Saved-query changes applied in memory whose file write has not
    /// finished. Reload never replaces those.
    unsaved_queries: Arc<AtomicU64>,
    /// Every saved query, republished after each change, so readers (menus,
    /// the palette, Settings) never wait for the store lock.
    catalogue: Arc<Mutex<Arc<Vec<SavedQuery>>>>,
    /// Serializes changes to sessions, recents and the Vault cache, each
    /// file on its own ([`ConfigStore::update_storage`]).
    file_locks: Arc<FileLocks>,
    /// Writes `config.toml` (tests substitute failures).
    settings_writer: Arc<SettingsWriter>,
    /// Writes `named_queries.toml` (tests substitute failures).
    queries_writer: Arc<QueriesWriter>,
}

type SettingsWriter = dyn Fn(&Config) -> std::io::Result<()> + Send + Sync;

/// One lock per user-data file the `Config` helpers write.
#[derive(Default)]
struct FileLocks {
    sessions: Mutex<()>,
    recents: Mutex<()>,
    vault: Mutex<()>,
}

impl FileLocks {
    fn of(&self, file: StorageFile) -> &Mutex<()> {
        match file {
            StorageFile::SavedSessions => &self.sessions,
            StorageFile::RecentConnections => &self.recents,
            StorageFile::VaultCredentials => &self.vault,
        }
    }
}
type QueriesWriter = dyn Fn(&NamedQueriesStorage) -> Result<(), String> + Send + Sync;

impl ConfigStore {
    /// Load from disk (honours `DBCRUST_CONFIG_DIR`). Blocking file I/O:
    /// call off the UI thread when possible.
    pub fn load() -> Self {
        #[cfg(test)]
        crate::test_support::isolate();
        let config = {
            let _guard = config_load_lock();
            Config::load()
        };
        Self::from_config(config)
    }

    /// Wrap an in-memory config (tests, screenshot fixtures).
    pub fn from_config(config: Config) -> Self {
        // A store's helpers may run on any thread: keep all of them, not
        // just the test's own, out of the real config directory
        #[cfg(test)]
        crate::test_support::isolate();
        let catalogue = Arc::new(catalogue_of(&config));
        Self {
            inner: Arc::new(Mutex::new(config)),
            revision: Arc::new(AtomicU64::new(0)),
            save_lock: Arc::new(Mutex::new(())),
            unsaved_settings: Arc::new(AtomicU64::new(0)),
            queries_save_lock: Arc::new(Mutex::new(())),
            unsaved_queries: Arc::new(AtomicU64::new(0)),
            catalogue: Arc::new(Mutex::new(catalogue)),
            file_locks: Arc::default(),
            settings_writer: Arc::new(|config: &Config| config.save_with_documentation()),
            queries_writer: Arc::new(|storage: &NamedQueriesStorage| {
                storage
                    .save_to_disk()
                    .map_err(|e| format!("Could not write named_queries.toml: {e}"))
            }),
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
            if self.has_unsaved_changes() {
                tracing::debug!("config reload skipped: changes are not saved yet");
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
        if self.revision.load(Ordering::SeqCst) != before || self.has_unsaved_changes() {
            tracing::debug!("config reload skipped: the store changed meanwhile");
            return false;
        }
        *current = fresh;
        self.publish(&current);
        true
    }

    /// Settings or saved queries changed in memory and not written yet.
    fn has_unsaved_changes(&self) -> bool {
        self.unsaved_settings.load(Ordering::SeqCst) != 0
            || self.unsaved_queries.load(Ordering::SeqCst) != 0
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
    /// [`ConfigStore::save_settings`] for `config.toml` fields). Never call
    /// a `Config` helper that writes a file here: the lock would be held for
    /// the write. Use [`ConfigStore::update_sessions`] /
    /// [`ConfigStore::update_recents`] or the saved-query methods.
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

    /// [`ConfigStore::update_settings`] for a change that may be refused:
    /// only an `Ok` counts as unsaved, and an `Err` must leave the config
    /// untouched. In memory only (the lock is never held for I/O), so the
    /// UI thread may apply a Settings edit with it and every later reader
    /// (a connection being opened) sees the new value.
    pub fn try_update_settings<R, E>(
        &self,
        f: impl FnOnce(&mut Config) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut config = self.lock();
        let result = f(&mut config)?;
        let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
        self.unsaved_settings.store(revision, Ordering::SeqCst);
        Ok(result)
    }

    /// Owned copy of the current config.
    pub fn snapshot(&self) -> Config {
        self.lock().clone()
    }

    /// Persist `config.toml`, keeping the documentation comments (never use
    /// plain `Config::save`, which strips them).
    pub fn save_settings(&self) -> GuiResult<()> {
        let _saving = self.saving_settings();
        self.write_settings()
    }

    /// Settings changed in memory that `config.toml` does not hold yet
    /// (including after a failed save).
    pub fn has_unsaved_settings(&self) -> bool {
        self.unsaved_settings.load(Ordering::SeqCst) != 0
    }

    /// Save `config.toml` if settings changed since the last successful
    /// save; true when it wrote. Waits for a save in progress first, so on
    /// return everything is on disk, or the error says why not (the change
    /// stays unsaved, and is written by the next save). For the quit path.
    pub fn save_unsaved_settings(&self) -> GuiResult<bool> {
        let _saving = self.saving_settings();
        if !self.has_unsaved_settings() {
            return Ok(false);
        }
        self.write_settings().map(|()| true)
    }

    /// Save `config.toml` until nothing is unsaved: a change made while a
    /// write runs (Settings stays usable while the app quits) is written by
    /// another pass. Gives up once `deadline` has passed. For the quit path.
    pub fn save_settings_until_clean(&self, deadline: std::time::Instant) -> GuiResult<()> {
        loop {
            let _saving = self.saving_settings();
            if !self.has_unsaved_settings() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(GuiError::Storage(
                    "Settings kept changing while they were saved.".into(),
                ));
            }
            self.write_settings()?;
        }
    }

    fn saving_settings(&self) -> MutexGuard<'_, ()> {
        self.save_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Snapshot under the store lock, write outside it: UI reads never wait
    /// for disk I/O; `save_lock` (held by the caller) keeps saves ordered.
    fn write_settings(&self) -> GuiResult<()> {
        let (snapshot, revision) = {
            let config = self.lock();
            (config.clone(), self.revision.load(Ordering::SeqCst))
        };
        (self.settings_writer)(&snapshot)
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
        self.update_sessions(|config| config.delete_session(name))
            .map_err(|e| GuiError::Storage(format!("Could not delete session \"{name}\": {e}")))
    }

    /// Change saved sessions with a `Config` helper that writes
    /// `sessions.toml` (save, rename, delete). See
    /// [`ConfigStore::update_storage`]. Blocking: call it on a worker.
    pub fn update_sessions<T, E>(
        &self,
        f: impl FnOnce(&mut Config) -> Result<T, E>,
    ) -> Result<T, E> {
        self.update_storage(StorageFile::SavedSessions, f)
    }

    /// Change recent connections with a `Config` helper that writes
    /// `recent.toml` (record, remove, clear). See
    /// [`ConfigStore::update_storage`]. Blocking: call it on a worker.
    pub fn update_recents<T, E>(
        &self,
        f: impl FnOnce(&mut Config) -> Result<T, E>,
    ) -> Result<T, E> {
        self.update_storage(StorageFile::RecentConnections, f)
    }

    /// Run `f`, a `Config` helper that writes `file`'s user data, on a copy
    /// of the config, outside the store lock: the lock is never held for
    /// that disk write, so Settings edits (applied on the UI thread) and
    /// readers never wait for it. On success the changed data is published
    /// to the store; on failure nothing changes in memory (an atomic write
    /// leaves the previous file). Changes to one file are serialized by
    /// its own lock, from the copy to the publication, so they never
    /// interleave or undo each other.
    fn update_storage<T, E>(
        &self,
        file: StorageFile,
        f: impl FnOnce(&mut Config) -> Result<T, E>,
    ) -> Result<T, E> {
        let _file = self
            .file_locks
            .of(file)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut scratch = self.snapshot();
        let result = f(&mut scratch)?;
        let mut config = self.lock();
        config.copy_storage_from(&scratch, file);
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(result)
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
    /// [`crate::launcher::recent_key`]). Serialized with recording a recent
    /// entry ([`ConfigStore::update_recents`]): a removal can never
    /// interleave with (or be undone by) a connection recording its own.
    pub fn remove_recent(&self, key: &str) -> GuiResult<bool> {
        self.update_recents(|config| -> Result<bool, Box<dyn std::error::Error>> {
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
    //
    // Saved queries change in memory under the store lock, then
    // `named_queries.toml` is written after the lock is released (writes are
    // ordered by `queries_save_lock`, each one snapshotting the newest
    // state). Readers use the published catalogue and never wait for the
    // config lock. Change saved queries only through these methods: they
    // keep the catalogue in step.

    /// Every saved query (all scopes, plus legacy unscoped entries that a
    /// scoped global query does not shadow), sorted by name. Never blocks on
    /// the store lock: safe from render paths.
    pub fn all_named_queries(&self) -> Arc<Vec<SavedQuery>> {
        self.catalogue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The saved query with storage identity `key`, in any scope.
    pub fn saved_query(&self, key: &str) -> Option<SavedQuery> {
        self.all_named_queries()
            .iter()
            .find(|entry| entry.key == key)
            .cloned()
    }

    /// Named queries visible for `db_type` (global + that engine's scope),
    /// plus legacy unscoped entries not shadowed by a scoped one.
    pub fn named_queries(&self, db_type: Option<&DatabaseType>) -> Vec<NamedQueryEntry> {
        let catalogue = self.all_named_queries();
        let mut entries: Vec<NamedQueryEntry> = catalogue
            .iter()
            .filter(|entry| !entry.is_legacy())
            .filter(|entry| match &entry.scope {
                NamedQueryScope::Global => true,
                NamedQueryScope::DatabaseType(engine) => Some(engine) == db_type,
                NamedQueryScope::Session(_) => false,
            })
            .map(SavedQuery::entry)
            .collect();
        for legacy in catalogue.iter().filter(|entry| entry.is_legacy()) {
            if !entries.iter().any(|entry| entry.name == legacy.name) {
                entries.push(legacy.entry());
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.scope_label.cmp(&b.scope_label)));
        entries
    }

    /// The text of the saved query `name` saved for `engine` (`None`: the
    /// one available to every connection).
    pub fn named_query(&self, name: &str, engine: Option<&DatabaseType>) -> Option<String> {
        let scope = engine.map_or(NamedQueryScope::Global, |engine| {
            NamedQueryScope::DatabaseType(engine.clone())
        });
        self.all_named_queries()
            .iter()
            .find(|entry| !entry.is_legacy() && entry.name == name && entry.scope == scope)
            .map(|entry| entry.query.clone())
    }

    /// Save (or overwrite) a named query. `db_type: None` saves it globally.
    /// Overwriting keeps the query's creation date and description.
    pub fn save_named_query(
        &self,
        name: &str,
        query: &str,
        db_type: Option<DatabaseType>,
    ) -> GuiResult<()> {
        let name = name.trim();
        let scope = match db_type {
            Some(db_type) => NamedQueryScope::DatabaseType(db_type),
            None => NamedQueryScope::Global,
        };
        self.change_queries(|config| {
            let key = NamedQueriesStorage::generate_key(name, &scope);
            let old = config.named_queries_storage().queries.get(&key).cloned();
            let entry = NamedQuery {
                query: query.to_string(),
                scope: scope.clone(),
                created_at: old.as_ref().map_or_else(Utc::now, |old| old.created_at),
                description: old.as_ref().and_then(|old| old.description.clone()),
            };
            config.named_queries_storage_mut().replace(
                old.is_some().then_some(key.as_str()),
                name,
                entry,
            )?;
            // A legacy entry of that name is superseded by the global query
            let legacy =
                scope == NamedQueryScope::Global && config.named_queries.contains_key(name);
            let touched = Touched::queries();
            Ok((
                (),
                if legacy {
                    touched.dropping_legacy(name)
                } else {
                    touched
                },
            ))
        })
        .map_err(|e| GuiError::Storage(format!("Could not save query \"{name}\": {e}")))
    }

    /// Create (`old_key: None`) or edit a saved query in one mutation:
    /// SQL, description, and a rename or scope change (the old key is
    /// removed and the new one inserted together, keeping `created_at`).
    /// Refuses to overwrite another query. Returns the new key. Blocking
    /// file I/O: call it on a worker.
    pub fn update_named_query(
        &self,
        old_key: Option<&str>,
        name: &str,
        query: &str,
        scope: NamedQueryScope,
        description: Option<String>,
    ) -> GuiResult<String> {
        self.change_queries(|config| {
            let entry = NamedQuery {
                query: query.to_string(),
                scope,
                created_at: Utc::now(),
                description: description
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty()),
            };
            match old_key.and_then(|key| key.strip_prefix(LEGACY_PREFIX)) {
                // A legacy entry becomes a scoped query (it had no date)
                Some(legacy) => {
                    if !config.named_queries.contains_key(legacy) {
                        return Err(GONE.to_string());
                    }
                    let key = config
                        .named_queries_storage_mut()
                        .replace(None, name, entry)?;
                    Ok((key, Touched::queries().dropping_legacy(legacy)))
                }
                None => {
                    let key = config
                        .named_queries_storage_mut()
                        .replace(old_key, name, entry)?;
                    Ok((key, Touched::queries()))
                }
            }
        })
        .map_err(|e| GuiError::Storage(e.to_string()))
    }

    /// Replace the SQL of the saved query `key`, keeping its name, scope,
    /// description and creation date. Returns its key afterwards (a legacy
    /// entry moves to the global scope). Blocking: call it on a worker.
    pub fn save_named_query_by_key(&self, key: &str, query: &str) -> GuiResult<String> {
        self.change_queries(|config| {
            if let Some(name) = key.strip_prefix(LEGACY_PREFIX) {
                if !config.named_queries.contains_key(name) {
                    return Err(GONE_SAVE_AS.to_string());
                }
                let entry = NamedQuery {
                    query: query.to_string(),
                    scope: NamedQueryScope::Global,
                    created_at: Utc::now(),
                    description: None,
                };
                let key = config
                    .named_queries_storage_mut()
                    .replace(None, name, entry)?;
                return Ok((key, Touched::queries().dropping_legacy(name)));
            }
            let storage = config.named_queries_storage_mut();
            let mut entry = storage
                .queries
                .get(key)
                .cloned()
                .ok_or_else(|| GONE_SAVE_AS.to_string())?;
            entry.query = query.to_string();
            let name = NamedQueriesStorage::name_in_key(key, &entry.scope).to_string();
            let key = storage.replace(Some(key), &name, entry)?;
            Ok((key, Touched::queries()))
        })
        .map_err(|e| GuiError::Storage(e.to_string()))
    }

    /// Update only a query's description. Blocking: call it on a worker.
    pub fn set_named_query_description(
        &self,
        key: &str,
        description: Option<String>,
    ) -> GuiResult<()> {
        self.change_queries(|config| {
            let storage = config.named_queries_storage_mut();
            let mut entry = storage
                .queries
                .get(key)
                .cloned()
                .ok_or_else(|| GONE.to_string())?;
            entry.description = description
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty());
            let name = NamedQueriesStorage::name_in_key(key, &entry.scope).to_string();
            storage.replace(Some(key), &name, entry)?;
            Ok(((), Touched::queries()))
        })
        .map_err(|e| GuiError::Storage(e.to_string()))
    }

    /// Delete by the `key` of a [`SavedQuery`] / [`NamedQueryEntry`].
    /// Blocking: call it on a worker.
    pub fn delete_named_query(&self, key: &str) -> GuiResult<bool> {
        self.change_queries(|config| {
            if let Some(name) = key.strip_prefix(LEGACY_PREFIX) {
                // Only in config.toml: removed once the change is done
                let existed = config.named_queries.contains_key(name);
                let touched = Touched::default();
                return Ok((
                    existed,
                    if existed {
                        touched.dropping_legacy(name)
                    } else {
                        touched
                    },
                ));
            }
            let existed = config
                .named_queries_storage_mut()
                .queries
                .remove(key)
                .is_some();
            Ok((
                existed,
                Touched {
                    queries: existed,
                    drop_legacy: None,
                },
            ))
        })
        .map_err(|e| GuiError::Storage(format!("Could not delete query: {e}")))
    }

    /// Apply a saved-query change in memory (nothing changes when `change`
    /// fails) and publish the catalogue, then write `named_queries.toml`
    /// outside the store lock. If that write fails, the change is rolled
    /// back: memory, the catalogue and the file agree again. A legacy entry
    /// (`config.toml`) is removed only after the dedicated file holds its
    /// replacement, then `config.toml` is saved.
    fn change_queries<R>(
        &self,
        change: impl FnOnce(&mut Config) -> Result<(R, Touched), String>,
    ) -> Result<R, String> {
        // Held until the write is done: no other saved-query change can
        // interleave, so restoring `before` undoes exactly this one
        let _changing = self
            .queries_save_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (result, touched, before) = {
            let mut config = self.lock();
            let before = config.named_queries_storage().clone();
            let (result, touched) = change(&mut config)?;
            if touched.queries {
                self.revision.fetch_add(1, Ordering::SeqCst);
                // Reload never replaces a change whose write is pending
                self.unsaved_queries.fetch_add(1, Ordering::SeqCst);
                self.publish(&config);
            }
            (result, touched, before)
        };
        let written = if touched.queries {
            let storage = self.read(|config| config.named_queries_storage().clone());
            (self.queries_writer)(&storage)
        } else {
            Ok(())
        };
        // One lock for the outcome: the reload protection passes from the
        // pending query write to the unsaved legacy removal with no moment
        // where both are clear (a reload then would bring the legacy entry
        // back from config.toml next to its replacement)
        let save_config = {
            let mut config = self.lock();
            let (rolled_back, legacy_dropped) = match &written {
                Err(_) => {
                    *config.named_queries_storage_mut() = before;
                    (true, false)
                }
                Ok(()) => (
                    false,
                    touched
                        .drop_legacy
                        .as_ref()
                        .is_some_and(|name| config.named_queries.remove(name).is_some()),
                ),
            };
            if rolled_back || legacy_dropped {
                let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
                if legacy_dropped {
                    self.unsaved_settings.store(revision, Ordering::SeqCst);
                }
                self.publish(&config);
            }
            if touched.queries {
                self.unsaved_queries.fetch_sub(1, Ordering::SeqCst);
            }
            legacy_dropped
        };
        written?;
        if save_config {
            self.save_settings().map_err(|e| e.to_string())?;
        }
        Ok(result)
    }

    /// Rebuild the published catalogue. Call with the store lock held.
    fn publish(&self, config: &Config) {
        *self
            .catalogue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(catalogue_of(config));
    }

    // ── Recents and Vault cache (Settings) ──────────────────────────────

    /// Clear recents in memory and in `recent.toml`, like
    /// [`ConfigStore::remove_recent`]. Blocking: call it on a worker.
    pub fn clear_recent_connections(&self) -> GuiResult<()> {
        self.update_recents(|config| config.clear_recent_connections())
            .map_err(|e| GuiError::Storage(format!("Could not clear recent connections: {e}")))
    }

    /// Remove the encrypted Vault credential cache (no token needed).
    /// Blocking: call it on a worker.
    pub fn clear_vault_credentials(&self) -> GuiResult<()> {
        self.update_storage(StorageFile::VaultCredentials, |config| {
            config.clear_vault_credentials()
        })
        .map_err(|e| GuiError::Storage(format!("Could not clear cached credentials: {e}")))
    }

    /// What the Vault credential cache on disk holds, without usernames,
    /// passwords or leases. Reads and decrypts the file with the ambient
    /// Vault token, never changing it (a cache it cannot decrypt is
    /// `Locked`, not deleted): call it on a worker.
    pub fn vault_cache() -> VaultCache {
        let storage = Config::inspect_vault_credential_cache();
        if storage.is_locked() {
            return VaultCache::Locked;
        }
        let mut entries: Vec<VaultCacheEntry> = storage
            .cached_credentials
            .values()
            .map(|cached| VaultCacheEntry {
                mount: cached.mount_path.clone(),
                database: cached.database_name.clone(),
                role: cached.role_name.clone(),
                expires_at: cached.expire_time,
            })
            .collect();
        entries.sort_by(|a, b| {
            (&a.mount, &a.database, &a.role).cmp(&(&b.mount, &b.database, &b.role))
        });
        VaultCache::Entries(entries)
    }
}

/// What a saved-query change touched (see [`ConfigStore::change_queries`]).
#[derive(Default)]
struct Touched {
    /// `named_queries.toml` must be written.
    queries: bool,
    /// A legacy entry of `config.toml` to remove once that is done.
    drop_legacy: Option<String>,
}

impl Touched {
    fn queries() -> Self {
        Self {
            queries: true,
            drop_legacy: None,
        }
    }

    fn dropping_legacy(self, name: &str) -> Self {
        Self {
            drop_legacy: Some(name.to_string()),
            ..self
        }
    }
}

const GONE: &str = "This saved query no longer exists.";
const GONE_SAVE_AS: &str = "This saved query no longer exists. Use Save as… to keep the query.";

/// Every saved query of `config`, sorted by name (case-insensitive), then scope.
fn catalogue_of(config: &Config) -> Vec<SavedQuery> {
    let storage = config.named_queries_storage();
    let mut entries: Vec<SavedQuery> = storage
        .queries
        .iter()
        .map(|(key, entry)| SavedQuery {
            key: key.clone(),
            name: NamedQueriesStorage::name_in_key(key, &entry.scope).to_string(),
            query: entry.query.clone(),
            scope: entry.scope.clone(),
            description: entry.description.clone(),
            created_at: Some(entry.created_at),
        })
        .collect();
    for (name, query) in &config.named_queries {
        let global = NamedQueriesStorage::generate_key(name, &NamedQueryScope::Global);
        if !storage.queries.contains_key(&global) {
            entries.push(SavedQuery {
                key: format!("{LEGACY_PREFIX}{name}"),
                name: name.clone(),
                query: query.clone(),
                scope: NamedQueryScope::Global,
                description: None,
                created_at: None,
            });
        }
    }
    entries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| scope_label(&a.scope).cmp(&scope_label(&b.scope)))
    });
    entries
}

const LEGACY_PREFIX: &str = "legacy::";

/// Human-readable saved query scope, without credentials.
pub fn scope_label(scope: &NamedQueryScope) -> String {
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

/// A saved query in any scope, with its stable key and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedQuery {
    /// Storage identity (`NamedQueriesStorage::generate_key`, or
    /// `legacy::<name>` for an unscoped entry still in `config.toml`).
    pub key: String,
    pub name: String,
    pub query: String,
    pub scope: NamedQueryScope,
    pub description: Option<String>,
    /// `None` for legacy entries, which never recorded one.
    pub created_at: Option<DateTime<Utc>>,
}

impl SavedQuery {
    /// An unscoped entry from an older `config.toml`.
    pub fn is_legacy(&self) -> bool {
        self.key.starts_with(LEGACY_PREFIX)
    }

    fn entry(&self) -> NamedQueryEntry {
        NamedQueryEntry {
            key: self.key.clone(),
            name: self.name.clone(),
            query: self.query.clone(),
            scope_label: scope_label(&self.scope),
        }
    }
}

/// The Vault credential cache as Settings shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum VaultCache {
    /// Encrypted, and no Vault token is available to read it.
    Locked,
    Entries(Vec<VaultCacheEntry>),
}

/// One cached Vault credential: where it came from and when it expires.
/// Never carries the username, password or lease.
#[derive(Debug, Clone, PartialEq)]
pub struct VaultCacheEntry {
    pub mount: String,
    pub database: String,
    pub role: String,
    pub expires_at: DateTime<Utc>,
}

impl VaultCacheEntry {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }

    /// "Expires in 40 min", "Expires in 5 h", "Expires 2026-11-02", "Expired".
    pub fn expiry_label(&self, now: DateTime<Utc>) -> String {
        let left = self.expires_at - now;
        if self.is_expired(now) {
            "Expired".to_string()
        } else if left < chrono::TimeDelta::hours(1) {
            format!("Expires in {} min", left.num_minutes().max(1))
        } else if left < chrono::TimeDelta::hours(48) {
            format!("Expires in {} h", left.num_hours())
        } else {
            format!("Expires {}", self.expires_at.format("%Y-%m-%d"))
        }
    }
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

impl ConfigStore {
    /// Tests: write `config.toml` with `writer` instead (count, fail, stall).
    #[doc(hidden)]
    pub fn with_settings_writer(
        mut self,
        writer: impl Fn(&Config) -> std::io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.settings_writer = Arc::new(writer);
        self
    }
}

#[cfg(test)]
impl ConfigStore {
    fn with_queries_writer(
        mut self,
        writer: impl Fn(&NamedQueriesStorage) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.queries_writer = Arc::new(writer);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    fn failing_queries(_: &NamedQueriesStorage) -> Result<(), String> {
        Err("disk full".into())
    }

    #[test]
    fn a_failed_query_write_rolls_the_change_back() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let old = store
            .update_named_query(None, "kept", "SELECT 1", NamedQueryScope::Global, None)
            .unwrap();
        let fails = Arc::new(AtomicBool::new(true));
        let toggle = fails.clone();
        let store = store.with_queries_writer(move |storage| {
            if toggle.load(Ordering::SeqCst) {
                failing_queries(storage)
            } else {
                Ok(())
            }
        });
        let error = store
            .update_named_query(
                Some(&old),
                "renamed",
                "SELECT 2",
                NamedQueryScope::Global,
                None,
            )
            .unwrap_err();
        assert!(error.to_string().contains("disk full"), "{error}");
        // Memory, the catalogue and the file agree: nothing was renamed
        let kept = store.saved_query(&old).expect("the original is back");
        assert_eq!(kept.query, "SELECT 1");
        assert!(store.saved_query("global::renamed").is_none());
        assert!(store.read(|config| {
            config.named_queries_storage().queries.contains_key(&old)
                && config.named_queries_storage().queries.len() == 1
        }));
        // A failed delete is rolled back too
        assert!(store.delete_named_query(&old).is_err());
        assert!(store.saved_query(&old).is_some());
        // ...and a retry once the disk is back works on the same key
        fails.store(false, Ordering::SeqCst);
        let renamed = store
            .update_named_query(
                Some(&old),
                "renamed",
                "SELECT 2",
                NamedQueryScope::Global,
                None,
            )
            .unwrap();
        assert_eq!(renamed, "global::renamed");
        assert_eq!(store.unsaved_queries.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_legacy_query_is_dropped_only_after_its_replacement_is_written() {
        crate::test_support::isolate();
        let mut config = Config::default_settings();
        config
            .named_queries
            .insert("legacy-failing".into(), "SELECT 1".into());
        let saves = Arc::new(AtomicU64::new(0));
        let counted = saves.clone();
        let store = ConfigStore::from_config(config)
            .with_queries_writer(failing_queries)
            .with_settings_writer(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
        assert!(
            store
                .update_named_query(
                    Some("legacy::legacy-failing"),
                    "moved",
                    "SELECT 1",
                    NamedQueryScope::Global,
                    None,
                )
                .is_err()
        );
        // Still in memory, so a settings save keeps it in config.toml
        assert!(store.read(|config| config.named_queries.contains_key("legacy-failing")));
        assert!(store.saved_query("legacy::legacy-failing").is_some());
        assert!(!store.has_unsaved_settings());
        assert_eq!(saves.load(Ordering::SeqCst), 0, "config.toml untouched");
        store.update_settings(|config| config.default_limit = 7);
        let rendered = store.read(Config::render_documented_config);
        assert!(rendered.contains("legacy-failing"), "{rendered}");
    }

    #[test]
    fn the_quit_save_writes_an_edit_made_during_the_final_write() {
        crate::test_support::isolate();
        // The writer is called with a snapshot; the user changes a setting
        // (Settings stays usable while the app quits) before it returns
        let store_cell: Arc<std::sync::OnceLock<ConfigStore>> = Arc::default();
        let writes: Arc<Mutex<Vec<usize>>> = Arc::default();
        let (cell, written) = (store_cell.clone(), writes.clone());
        let store = ConfigStore::from_config(Config::default_settings()).with_settings_writer(
            move |snapshot| {
                let mut written = written.lock().unwrap();
                written.push(snapshot.default_limit);
                if written.len() == 1 {
                    cell.get()
                        .unwrap()
                        .update_settings(|config| config.default_limit = 2);
                }
                Ok(())
            },
        );
        store_cell.set(store.clone()).ok().unwrap();
        store.update_settings(|config| config.default_limit = 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        store.save_settings_until_clean(deadline).unwrap();
        assert!(!store.has_unsaved_settings(), "the second value is on disk");
        assert_eq!(*writes.lock().unwrap(), [1, 2]);
    }

    #[test]
    fn a_slow_recents_write_never_blocks_a_settings_edit() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let (entered_tx, entered) = mpsc::channel();
        let (release_tx, release) = mpsc::channel::<()>();
        // A connection records its recent entry and the write is slow. The
        // closure stands in for the helper: it touches no file at all, so
        // this test cannot notice a thread escaping the test directory;
        // `test_support`'s subprocess guard covers that
        let recording = {
            let store = store.clone();
            std::thread::spawn(move || {
                store.update_recents(|_config| {
                    entered_tx.send(()).unwrap();
                    release.recv().unwrap();
                    Ok::<_, String>(())
                })
            })
        };
        entered.recv().unwrap();
        // Meanwhile the UI thread applies a Settings edit: it must not wait
        let (done_tx, done) = mpsc::channel();
        {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .try_update_settings(|config| {
                        config.read_only_default = true;
                        Ok::<_, String>(())
                    })
                    .unwrap();
                done_tx.send(()).unwrap();
            });
        }
        let edited = done.recv_timeout(std::time::Duration::from_secs(2));
        release_tx.send(()).unwrap();
        recording.join().unwrap().unwrap();
        assert!(edited.is_ok(), "the settings edit waited for recent.toml");
        assert!(store.read(|config| config.read_only_default));
    }

    #[test]
    fn a_failed_recents_write_changes_nothing_in_memory() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let result: Result<(), String> = store.update_recents(|config| {
            config
                .add_recent_connection_with_options(
                    "postgres://app@never-written:5432/x".into(),
                    DatabaseType::PostgreSQL,
                    true,
                    Default::default(),
                )
                .unwrap();
            Err("disk full".into())
        });
        assert!(result.is_err());
        assert!(
            !store
                .recent_connections()
                .iter()
                .any(|recent| recent.url.contains("@never-written"))
        );
    }

    #[test]
    fn a_legacy_rename_is_protected_from_reload_throughout() {
        crate::test_support::isolate();
        let mut config = Config::default_settings();
        config
            .named_queries
            .insert("legacy-window".into(), "SELECT 1".into());
        let store_cell: Arc<std::sync::OnceLock<ConfigStore>> = Arc::default();
        let observer: Arc<Mutex<Option<std::thread::JoinHandle<bool>>>> = Arc::default();
        let (held_tx, held_rx) = mpsc::channel::<()>();
        let held_rx = Mutex::new(held_rx);
        let (cell, slot) = (store_cell.clone(), observer.clone());
        let store = ConfigStore::from_config(config)
            .with_settings_writer(|_| Ok(()))
            .with_queries_writer(move |_| {
                // The dedicated file is written. A reload now takes the
                // store lock: from then until the change is complete on the
                // other side of the lock, the store must stay protected
                let store = cell.get().unwrap().clone();
                let held = held_tx.clone();
                *slot.lock().unwrap() = Some(std::thread::spawn(move || {
                    store.read(|config| {
                        held.send(()).unwrap();
                        let until =
                            std::time::Instant::now() + std::time::Duration::from_millis(250);
                        while std::time::Instant::now() < until {
                            let unprotected = store.unsaved_queries.load(Ordering::SeqCst) == 0
                                && store.unsaved_settings.load(Ordering::SeqCst) == 0;
                            if unprotected && config.named_queries.contains_key("legacy-window") {
                                return true;
                            }
                            std::thread::yield_now();
                        }
                        false
                    })
                }));
                // Return only once the reload holds the lock
                held_rx.lock().unwrap().recv().unwrap();
                Ok(())
            });
        store_cell.set(store.clone()).ok().unwrap();
        store
            .update_named_query(
                Some("legacy::legacy-window"),
                "renamed-window",
                "SELECT 1",
                NamedQueryScope::Global,
                None,
            )
            .unwrap();
        let observer = observer.lock().unwrap().take().expect("the reload ran");
        let unprotected = observer.join().unwrap();
        assert!(
            !unprotected,
            "a reload could have brought the legacy query back from config.toml"
        );
        assert!(store.read(|config| config.named_queries.is_empty()));
        assert_eq!(store.all_named_queries().len(), 1);
    }

    #[test]
    fn a_refused_settings_change_is_not_unsaved() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let refused: Result<(), String> = store.try_update_settings(|_| Err("invalid".into()));
        assert!(refused.is_err());
        assert!(!store.has_unsaved_settings());
        store
            .try_update_settings(|config| {
                config.read_only_default = true;
                Ok::<_, String>(())
            })
            .unwrap();
        assert!(store.has_unsaved_settings());
        assert!(store.read(|config| config.read_only_default));
    }

    #[test]
    fn a_failed_settings_save_stays_unsaved_until_a_save_succeeds() {
        crate::test_support::isolate();
        let fails = Arc::new(AtomicBool::new(true));
        let toggle = fails.clone();
        let store =
            ConfigStore::from_config(Config::default_settings()).with_settings_writer(move |_| {
                if toggle.load(Ordering::SeqCst) {
                    Err(std::io::Error::other("disk full"))
                } else {
                    Ok(())
                }
            });
        assert!(!store.save_unsaved_settings().unwrap(), "nothing to save");
        store.update_settings(|config| config.default_limit = 9);
        assert!(store.save_unsaved_settings().is_err());
        assert!(store.has_unsaved_settings(), "kept for a retry");
        assert!(!store.reload(), "and protected from reload");
        fails.store(false, Ordering::SeqCst);
        // The retry (next save, or quit) writes it without a new edit
        assert!(store.save_unsaved_settings().unwrap());
        assert!(!store.has_unsaved_settings());
    }

    #[test]
    fn saving_unsaved_settings_waits_for_a_save_in_progress() {
        crate::test_support::isolate();
        let (entered_tx, entered) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<bool>();
        let release_rx = Mutex::new(release_rx);
        let store =
            ConfigStore::from_config(Config::default_settings()).with_settings_writer(move |_| {
                let _ = entered_tx.send(());
                // The first save blocks until released, then fails
                match release_rx.lock().unwrap().recv() {
                    Ok(true) => Err(std::io::Error::other("disk full")),
                    _ => Ok(()),
                }
            });
        store.update_settings(|config| config.default_limit = 11);
        let in_flight = {
            let store = store.clone();
            std::thread::spawn(move || store.save_settings())
        };
        entered.recv().unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        let quit = {
            let (store, finished) = (store.clone(), finished.clone());
            std::thread::spawn(move || {
                let saved = store.save_unsaved_settings();
                finished.store(true, Ordering::SeqCst);
                saved
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !finished.load(Ordering::SeqCst),
            "the quit flush waits for the write in progress"
        );
        release.send(true).unwrap(); // the in-flight save fails…
        assert!(in_flight.join().unwrap().is_err());
        entered.recv().unwrap();
        release.send(false).unwrap(); // …so the quit flush writes it
        assert!(quit.join().unwrap().unwrap());
        assert!(!store.has_unsaved_settings());
    }

    #[test]
    fn manager_lists_every_scope_and_preserves_metadata_on_rename() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let key = store
            .update_named_query(
                None,
                "manager::original",
                "SELECT $1",
                NamedQueryScope::Session("session::id".into()),
                Some("  Useful description ".into()),
            )
            .unwrap();
        let before = store.saved_query(&key).unwrap();
        assert_eq!(before.name, "manager::original");
        assert_eq!(before.description.as_deref(), Some("Useful description"));
        assert!(before.created_at.is_some());
        let renamed = store
            .update_named_query(
                Some(&key),
                "manager::renamed",
                "SELECT $2",
                NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL),
                before.description.clone(),
            )
            .unwrap();
        assert!(store.saved_query(&key).is_none());
        let after = store.saved_query(&renamed).unwrap();
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(after.description, before.description);
        assert_eq!(after.query, "SELECT $2");
        assert!(store.named_queries(Some(&DatabaseType::MySQL)).is_empty());
        assert_eq!(
            store.named_queries(Some(&DatabaseType::PostgreSQL)).len(),
            1
        );
        store
            .set_named_query_description(&renamed, Some("Updated".into()))
            .unwrap();
        // Saving SQL from a query tab keeps the manager's metadata
        store
            .save_named_query(
                "manager::renamed",
                "SELECT 3",
                Some(DatabaseType::PostgreSQL),
            )
            .unwrap();
        let saved = store.saved_query(&renamed).unwrap();
        assert_eq!(saved.query, "SELECT 3");
        assert_eq!(saved.description.as_deref(), Some("Updated"));
        assert_eq!(saved.created_at, before.created_at);
        assert_eq!(
            store.save_named_query_by_key(&renamed, "SELECT 4").unwrap(),
            renamed
        );
        assert_eq!(store.saved_query(&renamed).unwrap().query, "SELECT 4");
        assert!(store.delete_named_query(&renamed).unwrap());
        assert!(!store.delete_named_query(&renamed).unwrap());
        assert!(store.all_named_queries().is_empty());
    }

    #[rstest::rstest]
    #[case(
        NamedQueryScope::Global,
        "A global saved query named “manager-occupied”"
    )]
    #[case(
        NamedQueryScope::DatabaseType(DatabaseType::SQLite),
        "already exists for SQLite"
    )]
    #[case(
        NamedQueryScope::Session("manager-session".into()),
        "already exists for session “manager-session”"
    )]
    fn manager_refuses_collisions_without_losing_the_original(
        #[case] scope: NamedQueryScope,
        #[case] message: &str,
    ) {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let old = store
            .update_named_query(None, "manager-old", "SELECT 1", scope.clone(), None)
            .unwrap();
        store
            .update_named_query(None, "manager-occupied", "SELECT 2", scope.clone(), None)
            .unwrap();
        let error = store
            .update_named_query(Some(&old), "manager-occupied", "SELECT 3", scope, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error}");
        assert_eq!(store.saved_query(&old).unwrap().query, "SELECT 1");
        assert_eq!(store.all_named_queries().len(), 2);
    }

    #[rstest::rstest]
    #[case("", "SELECT 1", "Enter a name")]
    #[case("   ", "SELECT 1", "Enter a name")]
    #[case("empty-sql", "  ", "Write SQL")]
    fn manager_rejects_incomplete_queries(
        #[case] name: &str,
        #[case] sql: &str,
        #[case] message: &str,
    ) {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        let error = store
            .update_named_query(None, name, sql, NamedQueryScope::Global, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error}");
        assert!(store.all_named_queries().is_empty());
    }

    #[test]
    fn manager_handles_legacy_keys_still_in_memory() {
        crate::test_support::isolate();
        let mut config = Config::default_settings();
        config
            .named_queries
            .insert("legacy-manager".into(), "SELECT 1".into());
        config
            .named_queries
            .insert("legacy-saved".into(), "SELECT 1".into());
        let store = ConfigStore::from_config(config);
        let legacy = store.saved_query("legacy::legacy-manager").unwrap();
        assert!(legacy.is_legacy());
        assert_eq!(legacy.created_at, None);
        // Visible to every connection, like a global query
        assert_eq!(store.named_queries(Some(&DatabaseType::MySQL)).len(), 2);
        let key = store
            .update_named_query(
                Some("legacy::legacy-manager"),
                "legacy-updated",
                "SELECT 2",
                NamedQueryScope::Global,
                Some("Migrated".into()),
            )
            .unwrap();
        assert!(store.saved_query("legacy::legacy-manager").is_none());
        assert!(store.read(|config| !config.named_queries.contains_key("legacy-manager")));
        assert_eq!(
            store.saved_query(&key).unwrap().description.as_deref(),
            Some("Migrated")
        );
        // Saving a tab bound to a legacy entry moves it to the global scope
        let moved = store
            .save_named_query_by_key("legacy::legacy-saved", "SELECT 3")
            .unwrap();
        assert_eq!(moved, "global::legacy-saved");
        assert_eq!(store.saved_query(&moved).unwrap().query, "SELECT 3");
        assert!(
            store
                .save_named_query_by_key("legacy::legacy-saved", "SELECT 4")
                .is_err()
        );
    }

    #[test]
    fn catalogue_reads_never_wait_for_the_store_lock() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        store
            .update_named_query(None, "catalogue", "SELECT 1", NamedQueryScope::Global, None)
            .unwrap();
        // A worker holding the lock (e.g. writing sessions.toml) does not
        // block menus, the palette or Settings
        let _held = store.lock();
        assert_eq!(store.all_named_queries().len(), 1);
        assert_eq!(store.named_queries(None).len(), 1);
        assert_eq!(
            store.named_query("catalogue", None).as_deref(),
            Some("SELECT 1")
        );
    }

    #[rstest::rstest]
    #[case(-5, "Expired")]
    #[case(0, "Expired")]
    #[case(20, "Expires in 20 min")]
    #[case(5 * 60, "Expires in 5 h")]
    fn vault_cache_expiry_reads_naturally(#[case] minutes: i64, #[case] label: &str) {
        crate::test_support::isolate();
        let now = Utc::now();
        let entry = VaultCacheEntry {
            mount: "database".into(),
            database: "orders".into(),
            role: "reader".into(),
            expires_at: now + chrono::TimeDelta::minutes(minutes),
        };
        assert_eq!(entry.expiry_label(now), label);
    }

    #[test]
    fn reload_waits_for_saved_query_writes() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default_settings());
        store.unsaved_queries.fetch_add(1, Ordering::SeqCst);
        assert!(!store.reload(), "reload must not drop a pending write");
        store.unsaved_queries.fetch_sub(1, Ordering::SeqCst);
    }

    #[test]
    fn reload_never_overwrites_a_concurrent_update() {
        crate::test_support::isolate();
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
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default());
        store.update_settings(|config| config.default_limit = 7);
        assert_ne!(store.unsaved_settings.load(Ordering::SeqCst), 0);
        assert!(!store.reload(), "reload must not drop unsaved settings");
        assert_eq!(store.read(|config| config.default_limit), 7);
    }

    #[test]
    fn removing_a_recent_changes_memory_and_disk_together() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default());
        let add = |url: &str| {
            store
                .update_recents(|config| {
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
        crate::test_support::isolate();
        assert_eq!(scope_label(&NamedQueryScope::Global), "global");
        assert_eq!(
            scope_label(&NamedQueryScope::DatabaseType(DatabaseType::PostgreSQL)),
            "PostgreSQL"
        );
    }

    #[test]
    fn named_query_key_prefix_strips_cleanly() {
        crate::test_support::isolate();
        let scope = NamedQueryScope::DatabaseType(DatabaseType::SQLite);
        let key = NamedQueriesStorage::generate_key("a::b", &scope);
        let prefix_len = NamedQueriesStorage::generate_key("", &scope).len();
        assert_eq!(&key[prefix_len..], "a::b");
    }
}
