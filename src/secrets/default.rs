//! The process-wide store: the single place the secrets module reads
//! DBCrust's config. Everything else takes an explicit store.

use super::store::{SecretStore, SecretsConfig, StoreSettings};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

static DEFAULT: Mutex<Option<Arc<SecretStore>>> = Mutex::new(None);

thread_local! {
    static OVERRIDE: RefCell<Vec<Arc<SecretStore>>> = const { RefCell::new(Vec::new()) };
}

#[derive(serde::Deserialize, Default)]
struct PartialConfig {
    #[serde(default)]
    secrets: SecretsConfig,
}

fn read_settings() -> Option<StoreSettings> {
    let dir = crate::config::Config::get_config_directory().ok()?;
    let config = std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<PartialConfig>(&text).ok())
        .map(|partial| partial.secrets)
        .unwrap_or_default();
    Some(StoreSettings::new(config, dir))
}

/// The store every DBCrust code path uses.
///
/// Test builds (and `RUST_TEST_MODE`) get an in-memory store, so no test
/// can touch the real keychain, keyutils or `$HOME`. A thread-local override
/// ([`with_store_override`]) wins over everything.
pub fn default_store() -> Arc<SecretStore> {
    if let Some(store) = OVERRIDE.with(|stack| stack.borrow().last().cloned()) {
        return store;
    }
    let mut slot = DEFAULT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(store) = slot.as_ref() {
        return store.clone();
    }
    let store = Arc::new(if crate::config::is_test_mode() {
        SecretStore::in_memory()
    } else {
        match read_settings() {
            Some(settings) => SecretStore::open(&settings),
            None => SecretStore::open(&StoreSettings::new(
                SecretsConfig::default(),
                std::env::temp_dir().join("dbcrust-no-config-dir"),
            )),
        }
    });
    *slot = Some(store.clone());
    store
}

/// Forget the cached store so the next use re-reads `[secrets]` (after
/// `dbcrust config set secrets.backend …`).
pub fn reset_default_store() {
    *DEFAULT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Run `f` with `store` as this thread's default store (tests, tooling).
pub fn with_store_override<R>(store: Arc<SecretStore>, f: impl FnOnce() -> R) -> R {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            OVERRIDE.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
    }
    OVERRIDE.with(|stack| stack.borrow_mut().push(store));
    let _pop = Pop;
    f()
}
