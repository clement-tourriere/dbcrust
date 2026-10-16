//! `dbcrust secrets migrate`: move DBCrust-owned legacy secrets into the
//! secret store.
//!
//! The operation is two-phase and safe to interrupt at any point:
//!
//! 1. **Inventory** the DBCrust-owned legacy secrets (`~/.dbcrust` entries,
//!    `ai_keys.enc`, the old keychain AI entries, secret options and SSH
//!    passwords still in `sessions.toml`).
//! 2. **Stage** each into the store, **read it back and compare** (verify),
//!    and only then **clean up** the legacy copy.
//!
//! Every step is idempotent, so re-running after a crash simply finishes the
//! job: a secret exists in the store *before* its legacy copy is removed,
//! and legacy lines are removed individually, atomically and only when their
//! value is the one that was verified. A small journal (`secrets_migration.toml`)
//! records ids and states — never values — so an interrupted run can say what
//! it is resuming.
//!
//! Wildcard `.dbcrust` entries are never bound automatically; third-party
//! files (`.pgpass`, `.my.cnf`) are reported and never edited.

use crate::dbcrust_pass::{self, LegacyLineKind};
use crate::secrets::legacy::ai_keys as legacy;
use crate::secrets::{RecordId, SecretError, SecretStore, TargetBinding};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Journal file name inside the config directory.
pub const JOURNAL_FILE_NAME: &str = "secrets_migration.toml";

/// Where the legacy data lives (injectable for tests).
#[derive(Debug, Clone)]
pub struct MigrationPaths {
    pub dbcrust_pass: Option<PathBuf>,
    pub ai_keys: Option<PathBuf>,
    pub sessions: PathBuf,
    pub journal: PathBuf,
    /// Include the old `dbcrust` keychain AI entries.
    pub legacy_keychain: bool,
}

impl MigrationPaths {
    pub fn from_environment() -> Option<Self> {
        let dir = crate::config::Config::get_config_directory().ok()?;
        Some(Self {
            dbcrust_pass: dbcrust_pass::get_dbcrust_pass_path(),
            ai_keys: Some(dir.join("ai_keys.enc")),
            sessions: dir.join("sessions.toml"),
            journal: dir.join(JOURNAL_FILE_NAME),
            legacy_keychain: !crate::config::is_test_mode(),
        })
    }
}

/// What the user decided / how a run should behave.
#[derive(Debug, Clone, Copy, Default)]
pub struct MigrationOptions {
    /// Print the plan, change nothing.
    pub dry_run: bool,
    /// Migrate every non-wildcard DBCrust-owned item without asking.
    pub assume_yes: bool,
}

/// Interaction hooks (`inquire` in production, scripted in tests).
pub trait Prompter {
    /// Choose which of `labels` to migrate; returns the chosen indexes.
    fn select_items(&self, labels: &[String]) -> Result<Vec<usize>, String>;
    /// Final confirmation.
    fn confirm(&self, question: &str) -> Result<bool, String>;
    /// Bind a wildcard entry to a concrete target, or skip it (`None`).
    fn resolve_wildcard(&self, entry: &WildcardEntry) -> Result<Option<WildcardChoice>, String>;
}

/// A wildcard `.dbcrust` entry awaiting a decision (no password inside).
#[derive(Debug, Clone)]
pub struct WildcardEntry {
    pub engine: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub user: String,
    pub line: usize,
}

/// The concrete target chosen for a wildcard entry.
#[derive(Debug, Clone)]
pub struct WildcardChoice {
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    /// Remove the (shared) wildcard line from `.dbcrust` afterwards.
    pub remove_wildcard_line: bool,
}

/// `inquire`-backed prompter.
pub struct InquirePrompter;

impl Prompter for InquirePrompter {
    fn select_items(&self, labels: &[String]) -> Result<Vec<usize>, String> {
        if labels.is_empty() {
            return Ok(Vec::new());
        }
        let all: Vec<usize> = (0..labels.len()).collect();
        let chosen = inquire::MultiSelect::new(
            "Select the secrets to move into the secure store:",
            labels.to_vec(),
        )
        .with_default(&all)
        .raw_prompt()
        .map_err(|e| e.to_string())?;
        Ok(chosen.into_iter().map(|option| option.index).collect())
    }

    fn confirm(&self, question: &str) -> Result<bool, String> {
        inquire::Confirm::new(question)
            .with_default(true)
            .prompt()
            .map_err(|e| e.to_string())
    }

    fn resolve_wildcard(&self, entry: &WildcardEntry) -> Result<Option<WildcardChoice>, String> {
        let describe = format!(
            "{}:{}@{}:{}/{} (line {})",
            entry.engine, entry.user, entry.host, entry.port, entry.database, entry.line
        );
        let bind = "Bind it to one concrete target";
        let skip = "Skip (leave it in ~/.dbcrust)";
        let choice = inquire::Select::new(
            &format!("Wildcard entry {describe} matches many targets:"),
            vec![bind, skip],
        )
        .prompt()
        .map_err(|e| e.to_string())?;
        if choice == skip {
            return Ok(None);
        }
        let ask = |label: &str, current: &str| -> Result<String, String> {
            if current == "*" {
                inquire::Text::new(label)
                    .with_validator(|input: &str| {
                        if input.trim().is_empty() {
                            Ok(inquire::validator::Validation::Invalid(
                                "A value is required.".into(),
                            ))
                        } else {
                            Ok(inquire::validator::Validation::Valid)
                        }
                    })
                    .prompt()
                    .map_err(|e| e.to_string())
            } else {
                Ok(current.to_string())
            }
        };
        let host = ask("Host:", &entry.host)?;
        let port_text = ask("Port:", &entry.port)?;
        let database = ask("Database:", &entry.database)?;
        let user = ask("User:", &entry.user)?;
        let port = port_text
            .parse::<u16>()
            .map_err(|_| "the port must be a number".to_string())?;
        let remove_wildcard_line = inquire::Confirm::new(
            "Remove the wildcard line afterwards? (other targets relying on it will lose the password)",
        )
        .with_default(false)
        .prompt()
        .map_err(|e| e.to_string())?;
        Ok(Some(WildcardChoice {
            host,
            port: Some(port),
            database,
            user,
            remove_wildcard_line,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ItemState {
    Planned,
    Staged,
    Verified,
    Cleaned,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Journal {
    #[serde(default)]
    items: Vec<JournalItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalItem {
    /// Store id (or `session:<name>`): never a value.
    key: String,
    state: ItemState,
}

enum Source {
    DbcrustLine { raw: String },
    AiKeysLine { raw: String },
    LegacyKeychain { account: String },
    SessionSecrets { session: String },
}

enum Payload {
    Connection {
        binding: TargetBinding,
        password: Zeroizing<String>,
    },
    Named {
        id: String,
        value: Zeroizing<String>,
    },
    SessionOptions {
        binding: TargetBinding,
        options: BTreeMap<String, Zeroizing<String>>,
    },
}

struct Item {
    key: String,
    description: String,
    source: Source,
    payload: Payload,
    /// The source line is a shared wildcard entry bound to one target: it
    /// is removed only when `also_remove_wildcard` is set.
    shared_source: bool,
    /// Raw wildcard line to drop after a bound item verified.
    also_remove_wildcard: Option<String>,
    state: ItemState,
}

/// Everything found in the legacy locations.
#[derive(Default)]
struct Inventory {
    items: Vec<Item>,
    wildcards: Vec<(WildcardEntry, String)>,
    /// Extra raw legacy lines whose value equals an already-planned item's,
    /// keyed by that item's key (removed together with it).
    duplicates: BTreeMap<String, Vec<String>>,
    /// Old keychain accounts holding the same value as a planned item.
    duplicate_keychain: Vec<(String, String)>,
}

/// What a run did (ids and counts only).
#[derive(Debug, Default, Clone, Serialize)]
pub struct MigrationReport {
    pub migrated: Vec<String>,
    /// Legacy items left in place, with the reason.
    pub retained: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
    /// Wildcard entries left alone.
    pub wildcards: Vec<String>,
    /// Third-party files reported but never edited.
    pub third_party: Vec<String>,
    pub resumed_items: usize,
    pub dry_run: bool,
}

impl MigrationReport {
    pub fn exit_code(&self) -> i32 {
        i32::from(!self.failed.is_empty())
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        if self.dry_run {
            out.push_str("Dry run — nothing was changed.\n");
        }
        if self.resumed_items > 0 {
            out.push_str(&format!(
                "Resumed an interrupted migration ({} item(s) were in progress).\n",
                self.resumed_items
            ));
        }
        let section = |out: &mut String, title: &str, lines: Vec<String>| {
            if !lines.is_empty() {
                out.push_str(&format!("{title}:\n"));
                for line in lines {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        };
        section(
            &mut out,
            if self.dry_run {
                "Would migrate"
            } else {
                "Migrated into the secure store"
            },
            self.migrated.clone(),
        );
        section(
            &mut out,
            "Left in place",
            self.retained
                .iter()
                .map(|(what, why)| format!("{what} — {why}"))
                .collect(),
        );
        section(
            &mut out,
            "Failed (nothing was lost; legacy copies are untouched)",
            self.failed
                .iter()
                .map(|(what, why)| format!("{what} — {why}"))
                .collect(),
        );
        section(
            &mut out,
            "Wildcard entries (not migrated automatically)",
            self.wildcards.clone(),
        );
        section(
            &mut out,
            "Third-party files (reported, never edited)",
            self.third_party.clone(),
        );
        if out.is_empty() || out == "Dry run — nothing was changed.\n" {
            out.push_str("Nothing to migrate.\n");
        }
        out
    }
}

/// Runs one migration against a store and a set of legacy paths.
pub struct Migrator {
    store: Arc<SecretStore>,
    paths: MigrationPaths,
    #[cfg(test)]
    pub(crate) fail_cleanup: bool,
}

impl Migrator {
    pub fn new(store: Arc<SecretStore>, paths: MigrationPaths) -> Self {
        Self {
            store,
            paths,
            #[cfg(test)]
            fail_cleanup: false,
        }
    }

    fn engine_port(port: &str) -> Option<u16> {
        port.parse::<u16>().ok().filter(|port| *port != 0)
    }

    /// Build the inventory. Reads legacy files; never writes.
    fn inventory(&self, report: &mut MigrationReport) -> Inventory {
        let mut items: Vec<Item> = Vec::new();
        let mut wildcards: Vec<(WildcardEntry, String)> = Vec::new();
        let mut duplicates: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut duplicate_keychain: Vec<(String, String)> = Vec::new();
        let mut seen_keys: BTreeMap<String, usize> = BTreeMap::new();

        // --- ~/.dbcrust ---
        if let Some(path) = &self.paths.dbcrust_pass {
            match dbcrust_pass::read_legacy_lines(path) {
                Ok(lines) => {
                    for line in lines {
                        let location = format!("{}:{}", path.display(), line.number);
                        match line.kind {
                            LegacyLineKind::Entry { entry, .. } => {
                                if entry.is_wildcard() {
                                    wildcards.push((
                                        WildcardEntry {
                                            engine: entry.database_type.as_str().to_string(),
                                            host: entry.hostname.clone(),
                                            port: entry.port.clone(),
                                            database: entry.database.clone(),
                                            user: entry.username.clone(),
                                            line: line.number,
                                        },
                                        line.raw,
                                    ));
                                    // password is dropped with `entry` — it is
                                    // re-read from the line if the user binds it
                                    continue;
                                }
                                let binding = TargetBinding::new(
                                    entry.database_type.as_str(),
                                    &entry.hostname,
                                    Self::engine_port(&entry.port),
                                    &entry.database,
                                    &entry.username,
                                    None,
                                );
                                let key = binding.record_id();
                                if let Some(&first) = seen_keys.get(&key) {
                                    // Same target twice: the first line wins
                                    // (it is what lookups used). The second
                                    // goes with it only if its value is equal.
                                    let same = matches!(
                                        &items[first].payload,
                                        Payload::Connection { password, .. }
                                            if password.as_str() == entry.password
                                    );
                                    if same {
                                        duplicates.entry(key).or_default().push(line.raw);
                                    } else {
                                        report.retained.push((
                                            location,
                                            "another entry for the same target holds a different value; kept"
                                                .to_string(),
                                        ));
                                    }
                                    continue;
                                }
                                seen_keys.insert(key.clone(), items.len());
                                items.push(Item {
                                    description: format!("{key} (from {location})"),
                                    key,
                                    source: Source::DbcrustLine { raw: line.raw },
                                    payload: Payload::Connection {
                                        binding,
                                        password: Zeroizing::new(entry.password.clone()),
                                    },
                                    shared_source: false,
                                    also_remove_wildcard: None,
                                    state: ItemState::Planned,
                                });
                            }
                            LegacyLineKind::Unreadable(reason) => {
                                report.retained.push((location, reason));
                            }
                            LegacyLineKind::Blank | LegacyLineKind::Comment => {}
                        }
                    }
                }
                Err(e) => report
                    .retained
                    .push((path.display().to_string(), format!("cannot be read: {e}"))),
            }
        }

        // --- ai_keys.enc and the old keychain entries ---
        let mut named: Vec<(String, String, Source, Zeroizing<String>)> = Vec::new();
        if self.paths.legacy_keychain {
            for account in legacy::keychain_account_names(&legacy::suggested_provider_names()) {
                if let Some(value) = legacy::keychain_get(&account) {
                    named.push((
                        account.clone(),
                        format!("old keychain entry \"{account}\""),
                        Source::LegacyKeychain { account },
                        value,
                    ));
                }
            }
        }
        if let Some(path) = &self.paths.ai_keys {
            for entry in legacy::read_file_entries(path) {
                let location = path.display().to_string();
                match entry.value {
                    Ok(value) => named.push((
                        entry.name.clone(),
                        format!("{location} entry \"{}\"", entry.name),
                        Source::AiKeysLine {
                            raw: entry.raw_line,
                        },
                        value,
                    )),
                    Err(reason) => report
                        .retained
                        .push((format!("{location} entry \"{}\"", entry.name), reason)),
                }
            }
        }
        for (name, origin, source, value) in named {
            let id = named_store_id(&name);
            if let Some(&first) = seen_keys.get(&id) {
                let same = matches!(
                    &items[first].payload,
                    Payload::Named { value: existing, .. } if existing.as_str() == value.as_str()
                );
                if same {
                    match &source {
                        Source::AiKeysLine { raw } => {
                            duplicates.entry(id.clone()).or_default().push(raw.clone());
                        }
                        Source::LegacyKeychain { account } => {
                            duplicate_keychain.push((id.clone(), account.clone()));
                        }
                        _ => {}
                    }
                } else {
                    report.retained.push((
                        origin,
                        "another legacy copy holds a different value; kept".to_string(),
                    ));
                }
                continue;
            }
            seen_keys.insert(id.clone(), items.len());
            items.push(Item {
                description: format!("{id} (from {origin})"),
                key: id.clone(),
                source,
                payload: Payload::Named { id, value },
                shared_source: false,
                also_remove_wildcard: None,
                state: ItemState::Planned,
            });
        }

        // --- sessions.toml: secret options and ssh_password ---
        if let Ok(Some(bytes)) =
            crate::secrets::fsutil::read_bounded(&self.paths.sessions, 10 * 1024 * 1024)
        {
            if let Ok(text) = String::from_utf8(bytes) {
                if let Ok(storage) = toml::from_str::<crate::config::SavedSessionsStorage>(&text) {
                    let mut names: Vec<&String> = storage.sessions.keys().collect();
                    names.sort();
                    for name in names {
                        let session = &storage.sessions[name];
                        let mut options: BTreeMap<String, Zeroizing<String>> = session
                            .options
                            .iter()
                            .filter(|(key, value)| {
                                crate::password_sanitizer::is_secret_option_key(key)
                                    && !value.is_empty()
                            })
                            .map(|(key, value)| (key.clone(), Zeroizing::new(value.clone())))
                            .collect();
                        if let Some(password) = session
                            .ssh_tunnel
                            .as_ref()
                            .and_then(|tunnel| tunnel.ssh_password.as_ref())
                            .filter(|password| !password.is_empty())
                        {
                            options.insert(
                                crate::config::SSH_PASSWORD_SECRET_OPTION.to_string(),
                                Zeroizing::new(password.clone()),
                            );
                        }
                        if options.is_empty() {
                            continue;
                        }
                        let tls = crate::config::tls_mode_from_options(&session.options);
                        let binding = if session.host.starts_with("DOCKER:") {
                            None
                        } else {
                            crate::config::connection_binding(
                                &session.database_type,
                                &session.host,
                                session.port,
                                &session.dbname,
                                &session.user,
                                tls.as_deref(),
                            )
                        };
                        let location =
                            format!("session \"{name}\" in {}", self.paths.sessions.display());
                        match binding {
                            Some(binding) => {
                                let key = format!("session:{name}");
                                items.push(Item {
                                    description: format!(
                                        "{} secret option(s) of {} (target {})",
                                        options.len(),
                                        location,
                                        binding.record_id()
                                    ),
                                    key,
                                    source: Source::SessionSecrets {
                                        session: name.clone(),
                                    },
                                    payload: Payload::SessionOptions { binding, options },
                                    shared_source: false,
                                    also_remove_wildcard: None,
                                    state: ItemState::Planned,
                                });
                            }
                            None => report.retained.push((
                                location,
                                "secret options on a file-based or Docker session have no target to bind to".to_string(),
                            )),
                        }
                    }
                }
            }
        }

        Inventory {
            items,
            wildcards,
            duplicates,
            duplicate_keychain,
        }
    }

    fn load_journal(&self) -> Journal {
        std::fs::read_to_string(&self.paths.journal)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save_journal(&self, items: &[Item]) {
        let journal = Journal {
            items: items
                .iter()
                .map(|item| JournalItem {
                    key: item.key.clone(),
                    state: item.state,
                })
                .collect(),
        };
        if let Ok(text) = toml::to_string_pretty(&journal) {
            let header =
                "# DBCrust secrets migration journal — ids and states only, never values.\n";
            let _ = crate::secrets::fsutil::write_private_atomic(
                &self.paths.journal,
                format!("{header}{text}").as_bytes(),
            );
        }
    }

    fn stage(&self, item: &Item) -> Result<(), SecretError> {
        match &item.payload {
            Payload::Connection { binding, password } => self
                .store
                .put_connection_secret(binding, password, BTreeMap::new())
                .map(|_| ()),
            Payload::Named { id, value } => self.store.put_secret(id, value),
            Payload::SessionOptions { binding, options } => self
                .store
                .merge_connection_options(binding, options.clone())
                .map(|_| ()),
        }
    }

    /// Read the staged value back and compare it with the legacy value.
    fn verify(&self, item: &Item) -> Result<(), SecretError> {
        let mismatch =
            || SecretError::Corrupt("read-back did not match the legacy value".to_string());
        match &item.payload {
            Payload::Connection { binding, password } => {
                let record = self
                    .store
                    .get_connection_secret(binding)?
                    .ok_or_else(|| SecretError::NotFound(binding.record_id()))?;
                if record.secret.as_str() == password.as_str() {
                    Ok(())
                } else {
                    Err(mismatch())
                }
            }
            Payload::Named { id, value } => {
                let stored = self
                    .store
                    .get_secret(id)?
                    .ok_or_else(|| SecretError::NotFound(id.clone()))?;
                if stored.as_str() == value.as_str() {
                    Ok(())
                } else {
                    Err(mismatch())
                }
            }
            Payload::SessionOptions { binding, options } => {
                let record = self
                    .store
                    .get_connection_secret(binding)?
                    .ok_or_else(|| SecretError::NotFound(binding.record_id()))?;
                for (key, value) in options {
                    match record.secret_options.get(key) {
                        Some(stored) if stored.as_str() == value.as_str() => {}
                        _ => return Err(mismatch()),
                    }
                }
                Ok(())
            }
        }
    }

    /// Run the migration.
    pub fn run(
        &self,
        options: MigrationOptions,
        prompter: &dyn Prompter,
        interactive: bool,
    ) -> Result<MigrationReport, String> {
        let mut report = MigrationReport {
            dry_run: options.dry_run,
            ..Default::default()
        };
        let previous = self.load_journal();
        report.resumed_items = previous
            .items
            .iter()
            .filter(|item| item.state != ItemState::Cleaned)
            .count();

        let Inventory {
            mut items,
            wildcards,
            duplicates,
            duplicate_keychain,
        } = self.inventory(&mut report);

        for (entry, _) in &wildcards {
            report.wildcards.push(format!(
                "{}:{}@{}:{}/{} (line {})",
                entry.engine, entry.user, entry.host, entry.port, entry.database, entry.line
            ));
        }
        for (label, path) in [
            ("PostgreSQL password file", crate::pgpass::get_pgpass_path()),
            ("MySQL option file", crate::myconf::get_mysql_config_path()),
        ] {
            if !crate::config::is_test_mode() {
                if let Some(path) = path.filter(|p| std::fs::symlink_metadata(p).is_ok()) {
                    report
                        .third_party
                        .push(format!("{label} {} (left as is)", path.display()));
                }
            }
        }

        // --- selection ---
        if options.dry_run {
            for item in &items {
                report.migrated.push(item.description.clone());
            }
            return Ok(report);
        }
        if items.is_empty() && wildcards.is_empty() {
            self.finish_journal(&items);
            return Ok(report);
        }

        let mut selected: Vec<usize> = if options.assume_yes {
            (0..items.len()).collect()
        } else if !interactive {
            return Err(
                "migrate needs a terminal to ask for confirmation; re-run interactively, or pass --yes for non-wildcard items (or --dry-run to see the plan)"
                    .to_string(),
            );
        } else {
            let labels: Vec<String> = items.iter().map(|item| item.description.clone()).collect();
            prompter.select_items(&labels)?
        };

        // Wildcards are only ever resolved interactively, one by one, and
        // never by --yes.
        if !options.assume_yes && interactive {
            for (entry, raw) in &wildcards {
                let Some(choice) = prompter.resolve_wildcard(entry)? else {
                    continue;
                };
                // The entry's password is re-read from its own line, only now
                let password = self
                    .paths
                    .dbcrust_pass
                    .as_deref()
                    .and_then(|path| dbcrust_pass::read_legacy_lines(path).ok())
                    .and_then(|lines| {
                        lines.into_iter().find_map(|line| match line.kind {
                            LegacyLineKind::Entry { entry, .. } if line.raw == *raw => {
                                Some(entry.password)
                            }
                            _ => None,
                        })
                    });
                let Some(password) = password else { continue };
                let binding = TargetBinding::new(
                    &entry.engine,
                    &choice.host,
                    choice.port,
                    &choice.database,
                    &choice.user,
                    None,
                );
                let key = binding.record_id();
                items.push(Item {
                    description: format!("{key} (bound from wildcard line {})", entry.line),
                    key,
                    source: Source::DbcrustLine { raw: raw.clone() },
                    payload: Payload::Connection {
                        binding,
                        password: Zeroizing::new(password),
                    },
                    shared_source: true,
                    also_remove_wildcard: choice.remove_wildcard_line.then(|| raw.clone()),
                    state: ItemState::Planned,
                });
                selected.push(items.len() - 1);
                report
                    .wildcards
                    .retain(|w| !w.ends_with(&format!("(line {})", entry.line)));
            }
        }

        if selected.is_empty() {
            self.finish_journal(&items);
            return Ok(report);
        }
        if !options.assume_yes
            && !prompter.confirm(&format!(
                "Move {} secret(s) into the secure store and remove the legacy copies?",
                selected.len()
            ))?
        {
            report.retained.push((
                "all selected items".to_string(),
                "migration was declined".to_string(),
            ));
            return Ok(report);
        }

        // --- stage + verify ---
        let mut abort: Option<String> = None;
        for &index in &selected {
            let item = &mut items[index];
            if abort.is_some() {
                report
                    .retained
                    .push((item.description.clone(), "not attempted".to_string()));
                continue;
            }
            // Re-read the state recorded by an earlier run for reporting only;
            // every step below is idempotent regardless.
            match self.stage_and_verify(item) {
                Ok(()) => {}
                Err(e) => {
                    if matches!(
                        e,
                        SecretError::Locked(_)
                            | SecretError::Unavailable(_)
                            | SecretError::Denied(_)
                    ) {
                        abort = Some(e.to_string());
                    }
                    report
                        .failed
                        .push((item.description.clone(), e.to_string()));
                    continue;
                }
            }
            self.save_journal_partial(&selected, &items);
        }
        if let Some(reason) = abort {
            report
                .failed
                .push(("remaining items".to_string(), format!("stopped: {reason}")));
        }

        // --- cleanup of verified items ---
        self.cleanup(
            &mut items,
            &selected,
            &duplicates,
            &duplicate_keychain,
            &mut report,
        );
        self.finish_journal(&items);
        Ok(report)
    }

    fn stage_and_verify(&self, item: &mut Item) -> Result<(), SecretError> {
        self.stage(item)?;
        item.state = ItemState::Staged;
        self.verify(item)?;
        item.state = ItemState::Verified;
        Ok(())
    }

    fn save_journal_partial(&self, selected: &[usize], items: &[Item]) {
        let subset: Vec<&Item> = selected.iter().map(|&i| &items[i]).collect();
        let journal = Journal {
            items: subset
                .iter()
                .map(|item| JournalItem {
                    key: item.key.clone(),
                    state: item.state,
                })
                .collect(),
        };
        if let Ok(text) = toml::to_string_pretty(&journal) {
            let _ =
                crate::secrets::fsutil::write_private_atomic(&self.paths.journal, text.as_bytes());
        }
    }

    fn cleanup(
        &self,
        items: &mut [Item],
        selected: &[usize],
        duplicates: &BTreeMap<String, Vec<String>>,
        duplicate_keychain: &[(String, String)],
        report: &mut MigrationReport,
    ) {
        let mut dbcrust_lines: HashSet<String> = HashSet::new();
        let mut ai_lines: HashSet<String> = HashSet::new();
        let mut keychain_accounts: Vec<String> = Vec::new();
        let mut sessions: Vec<String> = Vec::new();
        let mut verified: Vec<usize> = Vec::new();

        for &index in selected {
            let item = &items[index];
            if item.state != ItemState::Verified {
                continue;
            }
            verified.push(index);
            match &item.source {
                Source::DbcrustLine { raw } => {
                    if item.shared_source {
                        // A wildcard line serves many targets: dropped only
                        // when the user asked for it.
                        if let Some(wild) = &item.also_remove_wildcard {
                            dbcrust_lines.insert(wild.clone());
                        }
                    } else {
                        dbcrust_lines.insert(raw.clone());
                    }
                }
                Source::AiKeysLine { raw } => {
                    ai_lines.insert(raw.clone());
                }
                Source::LegacyKeychain { account } => keychain_accounts.push(account.clone()),
                Source::SessionSecrets { session } => sessions.push(session.clone()),
            }
            // Equal-valued duplicates of the same target go with it.
            if let Some(extra) = duplicates.get(&item.key) {
                for raw in extra {
                    match &item.source {
                        Source::DbcrustLine { .. } => dbcrust_lines.insert(raw.clone()),
                        _ => ai_lines.insert(raw.clone()),
                    };
                }
            }
            for (key, account) in duplicate_keychain {
                if *key == item.key {
                    keychain_accounts.push(account.clone());
                }
            }
        }

        #[cfg(test)]
        let fail_cleanup = self.fail_cleanup;
        #[cfg(not(test))]
        let fail_cleanup = false;

        let mut cleanup_error: Option<String> = None;
        if fail_cleanup {
            cleanup_error = Some("injected cleanup failure".to_string());
        } else {
            if !dbcrust_lines.is_empty() {
                if let Some(path) = &self.paths.dbcrust_pass {
                    if let Err(e) = dbcrust_pass::remove_legacy_lines(path, &dbcrust_lines) {
                        cleanup_error = Some(format!("{}: {e}", path.display()));
                    }
                }
            }
            if !ai_lines.is_empty() {
                if let Some(path) = &self.paths.ai_keys {
                    if let Err(e) = legacy::remove_file_lines(path, &ai_lines) {
                        cleanup_error = Some(format!("{}: {e}", path.display()));
                    }
                }
            }
            for account in &keychain_accounts {
                if !legacy::keychain_delete(account) {
                    // Already gone is fine; nothing else to do.
                }
            }
            if !sessions.is_empty() {
                if let Err(e) = self.scrub_sessions(&sessions) {
                    cleanup_error = Some(format!("{}: {e}", self.paths.sessions.display()));
                }
            }
        }

        match cleanup_error {
            None => {
                for &index in &verified {
                    items[index].state = ItemState::Cleaned;
                    report.migrated.push(items[index].description.clone());
                }
            }
            Some(reason) => {
                for &index in &verified {
                    report.failed.push((
                        items[index].description.clone(),
                        format!(
                            "stored and verified, but the legacy copy could not be removed ({reason}); re-run `dbcrust secrets migrate` to retry"
                        ),
                    ));
                }
            }
        }
    }

    /// Rewrite `sessions.toml` with the migrated sessions scrubbed.
    fn scrub_sessions(&self, names: &[String]) -> Result<(), String> {
        let text = std::fs::read_to_string(&self.paths.sessions).map_err(|e| e.to_string())?;
        let mut storage: crate::config::SavedSessionsStorage =
            toml::from_str(&text).map_err(|_| "sessions file could not be parsed".to_string())?;
        for name in names {
            if let Some(session) = storage.sessions.get_mut(name) {
                *session = session.scrubbed();
            }
        }
        let content = toml::to_string_pretty(&storage).map_err(|e| e.to_string())?;
        crate::secrets::fsutil::write_private_atomic(&self.paths.sessions, content.as_bytes())
            .map_err(|e| e.to_string())
    }

    fn finish_journal(&self, items: &[Item]) {
        if items
            .iter()
            .all(|item| item.state == ItemState::Cleaned || item.state == ItemState::Planned)
        {
            let _ = std::fs::remove_file(&self.paths.journal);
        } else {
            self.save_journal(items);
        }
    }
}

/// Store id for a legacy AI/OAuth name: `<provider>_api_key` →
/// `ai:<provider>`, anything else → `oauth:<name>`.
fn named_store_id(name: &str) -> String {
    match name.strip_suffix("_api_key") {
        Some(provider) if !provider.is_empty() => RecordId::ai(provider),
        _ => RecordId::oauth(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::testing::FaultyBackend;
    use crate::secrets::{BackendKind, MemoryBackend};
    use std::cell::RefCell;
    use std::path::Path;

    struct ScriptedPrompter {
        wildcard: RefCell<Option<WildcardChoice>>,
        prompted: RefCell<usize>,
    }

    impl ScriptedPrompter {
        fn skip() -> Self {
            Self {
                wildcard: RefCell::new(None),
                prompted: RefCell::new(0),
            }
        }
    }

    impl Prompter for ScriptedPrompter {
        fn select_items(&self, labels: &[String]) -> Result<Vec<usize>, String> {
            *self.prompted.borrow_mut() += 1;
            Ok((0..labels.len()).collect())
        }
        fn confirm(&self, _question: &str) -> Result<bool, String> {
            *self.prompted.borrow_mut() += 1;
            Ok(true)
        }
        fn resolve_wildcard(&self, _e: &WildcardEntry) -> Result<Option<WildcardChoice>, String> {
            *self.prompted.borrow_mut() += 1;
            Ok(self.wildcard.borrow_mut().take())
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        paths: MigrationPaths,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let legacy_enc = crate::password_encryption::legacy_encrypt_for_tests("enc-pw");
        let enc_line = format!(
            "mysql:db2.example.com:3306:shop:bob:{}",
            legacy_enc.replace(':', "\\:")
        );
        let dbcrust_pass = dir.path().join(".dbcrust");
        fs_write(
            &dbcrust_pass,
            &format!(
                "# my passwords\npostgresql:db.example.com:5432:app:alice:plain-pw\n{enc_line}\n\
                 postgresql:*:5432:app:carol:wild-pw\nunreadable line\n"
            ),
        );
        let ai_keys = dir.path().join("ai_keys.enc");
        let ai_enc = crate::password_encryption::legacy_encrypt_for_tests("sk-legacy");
        fs_write(&ai_keys, &format!("openai_api_key={ai_enc}\n"));
        let sessions = dir.path().join("sessions.toml");
        fs_write(
            &sessions,
            "[sessions.prod]\nhost = \"db.example.com\"\nport = 5432\nuser = \"alice\"\ndbname = \"app\"\ndatabase_type = \"PostgreSQL\"\n\
             [sessions.prod.options]\nsslpassword = \"cert-pw\"\nsslmode = \"require\"\n",
        );
        Fixture {
            paths: MigrationPaths {
                dbcrust_pass: Some(dbcrust_pass),
                ai_keys: Some(ai_keys),
                sessions,
                journal: dir.path().join(JOURNAL_FILE_NAME),
                legacy_keychain: false,
            },
            _dir: dir,
        }
    }

    fn fs_write(path: &Path, content: &str) {
        std::fs::write(path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn yes() -> MigrationOptions {
        MigrationOptions {
            dry_run: false,
            assume_yes: true,
        }
    }

    fn alice() -> TargetBinding {
        TargetBinding::new("postgresql", "db.example.com", None, "app", "alice", None)
    }

    #[test]
    fn full_migration_moves_secrets_and_removes_only_migrated_legacy_entries() {
        let fx = fixture();
        let store = Arc::new(SecretStore::in_memory());
        let migrator = Migrator::new(store.clone(), fx.paths.clone());
        let report = migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(report.failed.is_empty(), "{report:?}");

        // Store holds everything
        let alice_rec = store.get_connection_secret(&alice()).unwrap().unwrap();
        assert_eq!(&*alice_rec.secret, "plain-pw");
        assert_eq!(&*alice_rec.secret_options["sslpassword"], "cert-pw");
        let bob = TargetBinding::new("mysql", "db2.example.com", None, "shop", "bob", None);
        assert_eq!(
            &*store.get_connection_secret(&bob).unwrap().unwrap().secret,
            "enc-pw"
        );
        assert_eq!(
            &*store.get_secret("ai:openai").unwrap().unwrap(),
            "sk-legacy"
        );

        // Legacy: migrated lines gone, wildcard / unreadable / comments kept
        let left = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        assert!(left.contains("# my passwords"));
        assert!(left.contains("postgresql:*:5432:app:carol:wild-pw"));
        assert!(left.contains("unreadable line"));
        assert!(!left.contains("plain-pw"));
        assert!(!left.contains("enc\\:"));
        assert!(!fx.paths.ai_keys.as_ref().unwrap().exists());
        let sessions = std::fs::read_to_string(&fx.paths.sessions).unwrap();
        assert!(!sessions.contains("cert-pw"));
        assert!(sessions.contains("sslmode"));
        assert!(!fx.paths.journal.exists(), "journal removed when complete");

        // Wildcard never auto-migrated
        assert_eq!(report.wildcards.len(), 1);
        let carol = TargetBinding::new("postgresql", "*", Some(5432), "app", "carol", None);
        assert!(store.get_connection_secret(&carol).unwrap().is_none());
        assert!(
            report.retained.iter().any(
                |(_, why)| why.contains("encrypted value") || why.contains("expected 6 fields")
            )
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let fx = fixture();
        let store = Arc::new(SecretStore::in_memory());
        let migrator = Migrator::new(store.clone(), fx.paths.clone());
        migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        let revision_after_first = store
            .get_connection_secret(&alice())
            .unwrap()
            .unwrap()
            .revision;
        let second = migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(second.migrated.is_empty(), "{second:?}");
        assert!(second.failed.is_empty());
        let rec = store.get_connection_secret(&alice()).unwrap().unwrap();
        assert_eq!(&*rec.secret, "plain-pw");
        assert_eq!(
            rec.revision, revision_after_first,
            "second run must not rewrite records"
        );
    }

    #[test]
    fn dry_run_changes_nothing_and_needs_no_store() {
        let fx = fixture();
        let faulty = Arc::new(FaultyBackend::new(MemoryBackend::new()));
        faulty.fail_get_with(SecretError::Locked("locked".to_string()));
        faulty.fail_put_with(SecretError::Locked("locked".to_string()));
        let store = Arc::new(SecretStore::with_backend(faulty, None));
        let before = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        let report = Migrator::new(store, fx.paths.clone())
            .run(
                MigrationOptions {
                    dry_run: true,
                    assume_yes: false,
                },
                &ScriptedPrompter::skip(),
                false,
            )
            .unwrap();
        assert!(report.dry_run);
        assert!(report.migrated.len() >= 4);
        let after = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        assert_eq!(before, after);
        assert!(fx.paths.ai_keys.as_ref().unwrap().exists());
        assert!(!report.render_text().contains("plain-pw"));
    }

    /// Stage failing at every put, verify failing at every id, cleanup
    /// failing: the secret is never lost and a clean re-run converges.
    #[test]
    fn crash_at_each_step_never_loses_a_secret_and_is_resumable() {
        // 4 migratable items (alice, bob, openai, session options)
        for failing_put in 1..=4usize {
            let fx = fixture();
            let faulty = Arc::new(FaultyBackend::new(MemoryBackend::new()));
            faulty.fail_nth_put(failing_put);
            let store = Arc::new(SecretStore::with_backend(faulty.clone(), None));
            let migrator = Migrator::new(store.clone(), fx.paths.clone());
            let report = migrator
                .run(yes(), &ScriptedPrompter::skip(), false)
                .unwrap();
            assert!(!report.failed.is_empty(), "put #{failing_put}: {report:?}");
            assert_secrets_recoverable(&store, &fx);

            // Heal and re-run: everything ends in the store
            faulty.clear_faults();
            let rerun = migrator
                .run(yes(), &ScriptedPrompter::skip(), false)
                .unwrap();
            assert!(rerun.failed.is_empty(), "{rerun:?}");
            assert_eq!(
                &*store
                    .get_connection_secret(&alice())
                    .unwrap()
                    .unwrap()
                    .secret,
                "plain-pw"
            );
            assert!(store.get_secret("ai:openai").unwrap().is_some());
            assert!(!fx.paths.ai_keys.as_ref().unwrap().exists());
        }

        // Verification (read-back) failure for one record
        let fx = fixture();
        let faulty = Arc::new(FaultyBackend::new(MemoryBackend::new()));
        faulty.fail_get_of("ai:openai");
        let store = Arc::new(SecretStore::with_backend(faulty.clone(), None));
        let migrator = Migrator::new(store.clone(), fx.paths.clone());
        let report = migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(!report.failed.is_empty());
        assert!(
            fx.paths.ai_keys.as_ref().unwrap().exists(),
            "an unverified secret keeps its legacy copy"
        );
        faulty.clear_faults();
        let rerun = migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(rerun.failed.is_empty(), "{rerun:?}");
        assert!(!fx.paths.ai_keys.as_ref().unwrap().exists());

        // Cleanup failure: stored + verified, legacy untouched, retry works
        let fx = fixture();
        let store = Arc::new(SecretStore::in_memory());
        let mut migrator = Migrator::new(store.clone(), fx.paths.clone());
        migrator.fail_cleanup = true;
        let report = migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(!report.failed.is_empty());
        assert!(fx.paths.ai_keys.as_ref().unwrap().exists());
        assert!(fx.paths.journal.exists(), "journal kept for the retry");
        let journal = std::fs::read_to_string(&fx.paths.journal).unwrap();
        for value in ["plain-pw", "enc-pw", "sk-legacy", "cert-pw"] {
            assert!(!journal.contains(value), "journal must never hold values");
        }
        assert_eq!(
            &*store
                .get_connection_secret(&alice())
                .unwrap()
                .unwrap()
                .secret,
            "plain-pw"
        );
        migrator.fail_cleanup = false;
        let rerun = migrator
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(rerun.failed.is_empty(), "{rerun:?}");
        assert!(!fx.paths.journal.exists());
    }

    /// Every secret is in the store or still in its legacy file.
    fn assert_secrets_recoverable(store: &SecretStore, fx: &Fixture) {
        let legacy_pass = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        let in_store_or_legacy = |stored: bool, legacy_contains: bool| stored || legacy_contains;
        let alice_stored = store
            .get_connection_secret(&alice())
            .ok()
            .flatten()
            .is_some_and(|r| &*r.secret == "plain-pw");
        assert!(in_store_or_legacy(
            alice_stored,
            legacy_pass.contains("plain-pw")
        ));
        let bob = TargetBinding::new("mysql", "db2.example.com", None, "shop", "bob", None);
        let bob_stored = store
            .get_connection_secret(&bob)
            .ok()
            .flatten()
            .is_some_and(|r| &*r.secret == "enc-pw");
        assert!(in_store_or_legacy(
            bob_stored,
            legacy_pass.contains("mysql:db2")
        ));
        let ai_stored = store.get_secret("ai:openai").ok().flatten().is_some();
        assert!(in_store_or_legacy(
            ai_stored,
            fx.paths.ai_keys.as_ref().unwrap().exists()
        ));
        let sessions = std::fs::read_to_string(&fx.paths.sessions).unwrap();
        let opt_stored = store
            .get_connection_secret(&alice())
            .ok()
            .flatten()
            .is_some_and(|r| r.secret_options.contains_key("sslpassword"));
        assert!(in_store_or_legacy(opt_stored, sessions.contains("cert-pw")));
    }

    #[test]
    fn wildcard_entries_bind_only_through_an_explicit_choice() {
        let fx = fixture();
        let store = Arc::new(SecretStore::in_memory());
        let migrator = Migrator::new(store.clone(), fx.paths.clone());
        let prompter = ScriptedPrompter {
            wildcard: RefCell::new(Some(WildcardChoice {
                host: "reports.example.com".to_string(),
                port: Some(5432),
                database: "app".to_string(),
                user: "carol".to_string(),
                remove_wildcard_line: false,
            })),
            prompted: RefCell::new(0),
        };
        let report = migrator
            .run(
                MigrationOptions {
                    dry_run: false,
                    assume_yes: false,
                },
                &prompter,
                true,
            )
            .unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        let carol = TargetBinding::new(
            "postgresql",
            "reports.example.com",
            Some(5432),
            "app",
            "carol",
            None,
        );
        let record = store.get_connection_secret(&carol).unwrap().unwrap();
        assert_eq!(&*record.secret, "wild-pw");
        assert_eq!(record.bound_target.host, "reports.example.com");
        // The shared wildcard line stays unless explicitly removed
        let left = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        assert!(left.contains("postgresql:*:5432:app:carol:wild-pw"));
    }

    #[test]
    fn yes_never_touches_wildcards_and_non_interactive_without_yes_is_refused() {
        let fx = fixture();
        let store = Arc::new(SecretStore::in_memory());
        let migrator = Migrator::new(store.clone(), fx.paths.clone());
        let prompter = ScriptedPrompter::skip();
        let err = migrator
            .run(MigrationOptions::default(), &prompter, false)
            .unwrap_err();
        assert!(err.contains("--yes"));
        assert_eq!(
            *prompter.prompted.borrow(),
            0,
            "non-interactive never prompts"
        );

        migrator.run(yes(), &prompter, false).unwrap();
        assert_eq!(*prompter.prompted.borrow(), 0, "--yes never prompts");
        let left = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        assert!(left.contains("wild-pw"));
    }

    #[test]
    fn locked_store_aborts_without_touching_legacy_files() {
        let fx = fixture();
        let faulty = Arc::new(FaultyBackend::new(MemoryBackend::new()));
        faulty.fail_put_with(SecretError::Locked("locked".to_string()));
        let store = Arc::new(SecretStore::with_backend(faulty, None));
        let before = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        let report = Migrator::new(store, fx.paths.clone())
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(!report.failed.is_empty());
        assert!(report.migrated.is_empty());
        let after = std::fs::read_to_string(fx.paths.dbcrust_pass.as_ref().unwrap()).unwrap();
        assert_eq!(before, after);
        assert!(fx.paths.ai_keys.as_ref().unwrap().exists());
        let _ = BackendKind::Memory;
    }

    #[test]
    fn duplicate_entries_with_the_same_value_go_together_and_conflicts_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dbcrust");
        fs_write(
            &path,
            "postgresql:h:5432:d:u:same\npostgresql:H:5432:d:u:same\npostgresql:h:5432:d:u:other\n",
        );
        let paths = MigrationPaths {
            dbcrust_pass: Some(path.clone()),
            ai_keys: None,
            sessions: dir.path().join("sessions.toml"),
            journal: dir.path().join(JOURNAL_FILE_NAME),
            legacy_keychain: false,
        };
        let store = Arc::new(SecretStore::in_memory());
        let report = Migrator::new(store.clone(), paths)
            .run(yes(), &ScriptedPrompter::skip(), false)
            .unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        let left = std::fs::read_to_string(&path).unwrap();
        // the conflicting value is never dropped
        assert!(left.contains(":other"), "{left}");
        assert!(!left.contains(":same"), "{left}");
    }
}
