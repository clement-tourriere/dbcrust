//! `dbcrust secrets …`: inspect, unlock, audit, migrate and delete stored
//! secrets. No database connection is needed. Output never contains secret
//! values; `-o json` is supported where noted.
//!
//! Exit codes: `0` success, `1` an audit found high-severity items or an
//! operation failed, `2` the command cannot run here (non-interactive, bad
//! id, locked store).

pub mod audit;
pub mod migrate;

use crate::cli::{OutputFormat, SecretsAction};
use crate::config::Config;
use crate::secrets::{BackendKind, BackendState, RecordId, SecretStore};
use serde_json::json;
use std::sync::Arc;

/// Run one `dbcrust secrets` command and return the process exit code.
pub fn run(
    action: Option<&SecretsAction>,
    root_format: Option<OutputFormat>,
    config: &Config,
    interactive: bool,
) -> i32 {
    let store = crate::secrets::default_store();
    let wants_json = |own: &Option<OutputFormat>| {
        matches!(
            own.or(root_format),
            Some(OutputFormat::Json | OutputFormat::Jsonl)
        )
    };
    match action {
        None | Some(SecretsAction::Status { .. }) => {
            let json = match action {
                Some(SecretsAction::Status { format }) => wants_json(format),
                _ => wants_json(&None),
            };
            status(&store, config, json)
        }
        Some(SecretsAction::Unlock) => unlock(&store),
        Some(SecretsAction::Lock) => lock(&store),
        Some(SecretsAction::Audit { format }) => audit(&store, wants_json(format)),
        Some(SecretsAction::Migrate { dry_run, yes }) => {
            migrate(store, *dry_run, *yes, interactive)
        }
        Some(SecretsAction::Delete { id, yes }) => delete(&store, id, *yes, interactive),
    }
}

fn status(store: &SecretStore, config: &Config, json: bool) -> i32 {
    let status = store.status();
    let state = match status.backend.state {
        BackendState::Ready => "ready",
        BackendState::Locked => "locked",
        BackendState::Unavailable => "unavailable",
        BackendState::Empty => "empty",
    };
    let configured = config.secrets.backend;
    if json {
        let value = json!({
            "backend": {
                "configured": configured.as_str(),
                "resolved": configured.resolved().as_str(),
                "kind": status.backend.kind,
                "label": status.backend.kind.label(),
            },
            "state": state,
            "detail": status.backend.detail,
            "records": {
                "connection": status.counts.connection,
                "ai": status.counts.ai,
                "oauth": status.counts.oauth,
                "broker": status.counts.broker,
                "other": status.counts.other,
                "total": status.counts.total(),
            },
            "index": {
                "path": status.index_path.as_ref().map(|p| p.display().to_string()),
                "drift": {
                    "checked": status.drift.checked,
                    "missing_in_backend": status.drift.missing_in_backend,
                    "missing_in_index": status.drift.missing_in_index,
                    "error": status.drift.index_error,
                },
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return 0;
    }

    println!("Secret storage");
    println!(
        "  backend:  {} (config: {}{})",
        status.backend.kind.label(),
        configured.as_str(),
        if configured.as_str() == configured.resolved().as_str() {
            String::new()
        } else {
            format!(" → {}", configured.resolved().as_str())
        }
    );
    println!("  state:    {state}");
    println!("  detail:   {}", status.backend.detail);
    println!(
        "  records:  {} (connections {}, ai {}, oauth {}, broker {})",
        status.counts.total(),
        status.counts.connection,
        status.counts.ai,
        status.counts.oauth,
        status.counts.broker
    );
    if let Some(path) = &status.index_path {
        println!("  index:    {}", path.display());
    }
    if let Some(error) = &status.drift.index_error {
        println!("  drift:    index unreadable ({error})");
    } else if !status.drift.checked {
        println!("  drift:    not checked (this backend cannot list its records)");
    } else if status.drift.is_clean() {
        println!("  drift:    none");
    } else {
        for id in &status.drift.missing_in_backend {
            println!("  drift:    {id} is indexed but missing from the store");
        }
        for id in &status.drift.missing_in_index {
            println!("  drift:    {id} is in the store but not indexed");
        }
    }
    match status.backend.state {
        BackendState::Locked => println!("\nRun `dbcrust secrets unlock` to unlock it."),
        BackendState::Unavailable => {
            println!("\nThe store cannot be used; saved passwords cannot be read or written.")
        }
        _ => {}
    }
    0
}

fn unlock(store: &SecretStore) -> i32 {
    if store.backend_kind() != BackendKind::EncryptedFile {
        eprintln!(
            "The {} backend has no passphrase to unlock.",
            store.backend_kind().label()
        );
        return 2;
    }
    if !crate::secrets::is_interactive() {
        eprintln!("unlock needs an interactive terminal: it asks for the passphrase.");
        return 2;
    }
    let empty = store.status().backend.state == BackendState::Empty;
    match store.unlock() {
        Ok(()) if empty => {
            println!(
                "Nothing is stored yet; a passphrase is chosen the first time a secret is saved."
            );
            0
        }
        Ok(()) => {
            println!(
                "Secrets unlocked for this login session. `dbcrust secrets lock` forgets the key."
            );
            0
        }
        Err(e) => {
            eprintln!("Could not unlock: {e}");
            2
        }
    }
}

fn lock(store: &SecretStore) -> i32 {
    if store.backend_kind() != BackendKind::EncryptedFile {
        eprintln!(
            "The {} backend has no passphrase lock; the OS controls access.",
            store.backend_kind().label()
        );
        return 2;
    }
    match store.lock() {
        Ok(()) => {
            println!("Cached unlock key cleared; the next use asks for the passphrase.");
            0
        }
        Err(e) => {
            eprintln!("Could not lock: {e}");
            1
        }
    }
}

fn audit(store: &SecretStore, json: bool) -> i32 {
    let Some(context) = audit::AuditContext::from_environment() else {
        eprintln!("Could not determine the DBCrust config directory.");
        return 1;
    };
    let status = store.status();
    let report = audit::run_audit(&context, Some(&status));
    if json {
        println!("{}", report.render_json());
    } else {
        print!("{}", report.render_text());
    }
    report.exit_code()
}

fn migrate(store: Arc<SecretStore>, dry_run: bool, yes: bool, interactive: bool) -> i32 {
    let Some(paths) = migrate::MigrationPaths::from_environment() else {
        eprintln!("Could not determine the DBCrust config directory.");
        return 1;
    };
    let migrator = migrate::Migrator::new(store, paths);
    let options = migrate::MigrationOptions {
        dry_run,
        assume_yes: yes,
    };
    match migrator.run(options, &migrate::InquirePrompter, interactive) {
        Ok(report) => {
            print!("{}", report.render_text());
            report.exit_code()
        }
        Err(message) => {
            eprintln!("{message}");
            2
        }
    }
}

fn delete(store: &SecretStore, id: &str, yes: bool, interactive: bool) -> i32 {
    if RecordId::parse(id).is_none() {
        eprintln!(
            "\"{id}\" is not a secret id. Ids look like conn:<engine>:<user>@<host>:<port>/<db>, ai:<provider> or oauth:<name>; `dbcrust secrets status -o json` lists what exists."
        );
        return 2;
    }
    if !yes {
        if !interactive {
            eprintln!("delete asks for confirmation; pass --yes to confirm non-interactively.");
            return 2;
        }
        let confirmed = inquire::Confirm::new(&format!("Delete the stored secret {id}?"))
            .with_default(false)
            .prompt();
        match confirmed {
            Ok(true) => {}
            Ok(false) => {
                println!("Cancelled.");
                return 0;
            }
            Err(e) => {
                eprintln!("Cancelled: {e}");
                return 2;
            }
        }
    }
    match store.delete(id) {
        Ok(true) => {
            println!("Deleted {id}.");
            0
        }
        Ok(false) => {
            println!("No stored secret with id {id}.");
            0
        }
        Err(e) => {
            eprintln!("Could not delete {id}: {e}");
            if e.is_locked() { 2 } else { 1 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{TargetBinding, with_store_override};

    #[test]
    fn delete_requires_a_known_id_and_confirmation_without_a_tty() {
        let store = Arc::new(SecretStore::in_memory());
        let binding = TargetBinding::new("postgres", "h", None, "d", "u", None);
        store
            .put_connection_secret(&binding, "pw", Default::default())
            .unwrap();
        let id = binding.record_id();
        with_store_override(store.clone(), || {
            // Unknown id shape
            assert_eq!(delete(&store, "nonsense", true, false), 2);
            // Non-interactive and no --yes: refused, nothing deleted
            assert_eq!(delete(&store, &id, false, false), 2);
            assert!(store.get_connection_secret(&binding).unwrap().is_some());
            // --yes
            assert_eq!(delete(&store, &id, true, false), 0);
            assert!(store.get_connection_secret(&binding).unwrap().is_none());
            // Deleting again is not an error
            assert_eq!(delete(&store, &id, true, false), 0);
        });
    }

    #[test]
    fn lock_and_unlock_refuse_backends_without_a_passphrase() {
        let store = SecretStore::in_memory();
        assert_eq!(lock(&store), 2);
        assert_eq!(unlock(&store), 2);
    }
}
