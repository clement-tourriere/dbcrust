//! What Settings reads, and how its changes reach the store and the disk.
//!
//! Field getters read [`SettingsCache::config`], a snapshot of the store's
//! config refreshed in the background, never the store lock itself. An
//! edit is applied to the store at once, in memory (`try_update_settings`:
//! connections opened right after read the new value — "Open connections
//! read-only", SSH tunnels), then to the snapshot so the control shows it.
//! Only the disk write is deferred: one background save at a time,
//! [`SAVE_DELAY`] after the first edit of a burst (text and number fields
//! change the value on every keystroke), again if more edits arrived
//! meanwhile. A failed save stays pending in the store for the next save
//! and for the coordinated quit ([`flush_on_quit`]), which also waits for a
//! save in progress.

use super::*;
use dbcrust_gui_core::dbcrust::config_editor::FieldSpec;
use dbcrust_gui_core::store::VaultCache;
use gpui_kit::component::button::ButtonVariant;
use std::time::Duration;

/// How long a burst of edits may grow before `config.toml` is written.
pub(super) const SAVE_DELAY: Duration = Duration::from_millis(400);

pub(crate) struct SettingsCache {
    /// What the controls show: the store's config, edits included.
    pub config: Rc<Config>,
    /// `None` until the first background read of the cache file.
    pub vault: Option<VaultCache>,
    pub directory: Option<PathBuf>,
    /// A save is scheduled or running.
    saving: bool,
    /// A refresh came back older than an edit: refresh again after the save.
    refresh_after_save: bool,
    /// Bumped by each edit and refresh: a snapshot read before an edit is
    /// older than the cache and must not replace it.
    generation: u64,
}

impl Global for SettingsCache {}

impl SettingsCache {
    pub fn new(config: Config) -> Self {
        Self {
            config: Rc::new(config),
            vault: None,
            directory: None,
            saving: false,
            refresh_after_save: false,
            generation: 0,
        }
    }
}

/// One change made in Settings.
#[derive(Clone, Debug)]
pub(super) enum Edit {
    /// A schema field (`config_editor::apply_value`).
    Value(&'static str, String),
    /// Add (`old: None`) or replace an SSH tunnel pattern. Renaming the
    /// pattern removes the old key and inserts the new one together.
    Tunnel {
        old: Option<String>,
        pattern: String,
        target: String,
    },
    DeleteTunnel(String),
}

impl Edit {
    /// Validate and apply; nothing changes on error.
    pub(super) fn apply(&self, config: &mut Config) -> Result<(), String> {
        match self {
            Self::Value(path, value) => {
                config_editor::apply_value(config, path, value)?;
            }
            Self::Tunnel {
                old,
                pattern,
                target,
            } => {
                let (pattern, target) = (pattern.trim(), target.trim());
                config_editor::validate_tunnel_pattern(pattern)?;
                config_editor::validate_tunnel_target(config, target)?;
                if old.as_deref() != Some(pattern)
                    && config.ssh_tunnel_patterns.contains_key(pattern)
                {
                    return Err(tunnels::DUPLICATE_PATTERN.to_string());
                }
                if let Some(old) = old {
                    config.ssh_tunnel_patterns.remove(old);
                }
                config
                    .ssh_tunnel_patterns
                    .insert(pattern.to_string(), target.to_string());
            }
            Self::DeleteTunnel(pattern) => {
                config.ssh_tunnel_patterns.remove(pattern);
            }
        }
        Ok(())
    }
}

pub(super) fn cache(cx: &App) -> &SettingsCache {
    cx.global::<SettingsCache>()
}

/// Make sure the cache exists: a synchronous copy of the store when its
/// lock is free (once, when Settings first opens), else defaults until the
/// background refresh lands.
pub(super) fn ensure(cx: &mut App) {
    if !cx.has_global::<SettingsCache>() {
        let config = store(cx)
            .try_read(Clone::clone)
            .unwrap_or_else(Config::default_settings);
        cx.set_global(SettingsCache::new(config));
    }
}

pub(super) fn read_value(cx: &App, spec: &'static FieldSpec) -> String {
    (spec.get)(&cache(cx).config)
}

/// Apply a schema value, or notify with the schema's message.
pub(super) fn write_value(cx: &mut App, spec: &'static FieldSpec, value: String) {
    if let Err(error) = enqueue(Edit::Value(spec.path, value), cx) {
        notify(
            cx,
            Notification::error(error).title(format!("Couldn’t change {}", spec.label)),
        );
    }
}

/// Apply `edit` to the store (validated by the schema; nothing changes on
/// error), then to the snapshot the controls read, and schedule the save.
/// Runtime state that depends on the field (value formatting, query
/// timeout) is updated immediately.
pub(super) fn enqueue(edit: Edit, cx: &mut App) -> Result<(), String> {
    // The store first: it is what connections read. In memory only; the
    // lock is never held for file I/O by the settings or saved-query paths
    store(cx).try_update_settings(|config| edit.apply(config))?;
    cx.update_global::<SettingsCache, _>(|cache, _| {
        let config = Rc::make_mut(&mut cache.config);
        if edit.apply(config).is_err() {
            // The snapshot had drifted from the store: re-read it
            cache.refresh_after_save = true;
        }
        if let Edit::Value(path, _) = &edit {
            config_editor::reapply_runtime_setting(config, path);
        }
        cache.generation += 1;
    });
    schedule_save(cx);
    Ok(())
}

fn schedule_save(cx: &mut App) {
    let cache = cx.global_mut::<SettingsCache>();
    if cache.saving {
        return;
    }
    cache.saving = true;
    let store = store(cx);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let saving = store.clone();
        let result = cx
            .background_spawn(async move { saving.save_unsaved_settings() })
            .await;
        cx.update(|cx| {
            let cache = cx.global_mut::<SettingsCache>();
            cache.saving = false;
            let refresh_after = std::mem::take(&mut cache.refresh_after_save);
            match result {
                // Edits made during the save get the next one
                Ok(_) if store.has_unsaved_settings() => schedule_save(cx),
                Ok(_) => {}
                // Still pending in the store: the next edit or the quit
                // flush retries it
                Err(error) => notify(
                    cx,
                    Notification::error(error.to_string()).title("Couldn’t save settings"),
                ),
            }
            if refresh_after {
                refresh(cx);
            }
        });
    })
    .detach();
}

/// Write the settings that are not on disk yet, after any save in
/// progress, until nothing is unsaved (Settings stays usable while the app
/// quits) or `deadline` passes. For the coordinated quit: the returned
/// closure blocks on the disk, run it off the UI thread.
pub(crate) fn flush_on_quit(
    cx: &App,
) -> impl FnOnce(std::time::Instant) -> Result<(), String> + Send + use<> {
    let store = store(cx);
    move |deadline| {
        store
            .save_settings_until_clean(deadline)
            .map_err(|error| error.to_string())
    }
}

/// Settings changed and not on disk yet (the quit checks it last, on the
/// UI thread, so no edit slips in between).
pub(crate) fn has_unsaved_settings(cx: &App) -> bool {
    store(cx).has_unsaved_settings()
}

/// Re-read the store (and the Vault cache file) in the background. A
/// snapshot taken before an edit never replaces the control's value.
pub(super) fn refresh(cx: &mut App) {
    let cache = cx.global_mut::<SettingsCache>();
    cache.generation += 1;
    let generation = cache.generation;
    let store = store(cx);
    cx.spawn(async move |cx| {
        let (config, vault, directory) = cx
            .background_spawn(async move {
                (
                    store.snapshot(),
                    ConfigStore::vault_cache(),
                    Config::get_config_directory().ok(),
                )
            })
            .await;
        cx.update(|cx| {
            cx.update_global::<SettingsCache, _>(|cache, _| {
                cache.vault = Some(vault);
                cache.directory = directory;
                if cache.generation == generation {
                    cache.config = Rc::new(config);
                } else {
                    // An edit landed meanwhile: read again once it is saved
                    cache.refresh_after_save = true;
                }
            });
        });
    })
    .detach();
}

/// Settings → Files → Reload from disk: re-read every file, then let every
/// view re-read the store. Refused while a change is not saved yet: saving
/// it first would write this app's copy over the edits made outside, which
/// are what a reload is for.
pub(super) fn reload(cx: &mut App) {
    let store = store(cx);
    cx.spawn(async move |cx| {
        let reloaded = cx
            .background_spawn(async move {
                let reloaded = store.reload();
                if reloaded {
                    config_editor::reapply_runtime_settings(&store.snapshot());
                }
                reloaded
            })
            .await;
        cx.update(|cx| {
            if reloaded {
                AppState::global(cx).update(cx, |state, cx| state.storage_changed(cx));
            }
            notify(
                cx,
                if reloaded {
                    Notification::info("Settings, saved queries and connections were reloaded.")
                        .title("Reloaded from disk")
                } else {
                    Notification::warning(
                        "A change made here is not saved yet, so nothing was reloaded. Try \
                         again in a moment.",
                    )
                    .title("Nothing reloaded")
                },
            );
        });
    })
    .detach();
}

/// What a "Clear …" button removes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Clearable {
    RecentConnections,
    VaultCache,
}

/// Confirm, then clear on a worker and let every view re-read the store.
pub(super) fn confirm_clear(what: Clearable, window: &mut Window, cx: &mut App) {
    let (title, description, ok) = match what {
        Clearable::RecentConnections => (
            "Clear recent connections?",
            "Every entry in Recent is removed. Saved connections stay on the Connections page.",
            "Clear recent connections",
        ),
        Clearable::VaultCache => (
            "Clear cached Vault credentials?",
            "The encrypted cache is deleted from this computer. The next Vault connection \
             asks Vault for new credentials.",
            "Clear cached credentials",
        ),
    };
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .confirm()
            .title(title)
            .description(description)
            .ok_text(ok)
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                let store = store(cx);
                cx.spawn(async move |cx| {
                    let result = cx
                        .background_spawn(async move {
                            match what {
                                Clearable::RecentConnections => store.clear_recent_connections(),
                                Clearable::VaultCache => store.clear_vault_credentials(),
                            }
                        })
                        .await;
                    cx.update(|cx| match result {
                        Ok(()) => {
                            AppState::global(cx).update(cx, |state, cx| state.storage_changed(cx))
                        }
                        Err(error) => notify(cx, Notification::error(error.to_string())),
                    });
                })
                .detach();
                true
            })
    });
}
