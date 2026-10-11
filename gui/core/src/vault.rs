//! HashiCorp Vault discovery for the connection wizard: address detection,
//! listing database configs the token can use, and their roles.

use crate::error::{GuiError, GuiResult};
use crate::rt::run_detached;
use crate::store::ConfigStore;
use dbcrust::vault_client as vault;
use std::future::Future;
use std::process::Command;
use std::time::Duration;

/// What the wizard knows before asking anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultEnvironment {
    /// Detected `VAULT_ADDR`, if any.
    pub addr: Option<String>,
    /// Where the address came from ("process environment (VAULT_ADDR)", …).
    pub source: Option<String>,
    /// A token (`VAULT_TOKEN`, `~/.vault-token`) or a credential process is
    /// configured.
    pub token_available: bool,
}

/// Detect the Vault address and token. Blocking (may run `launchctl` /
/// `systemctl` with a short timeout): call off the UI thread.
pub fn detect_environment(store: &ConfigStore) -> VaultEnvironment {
    let detected = vault::detect_vault_addr()
        .map(|detected| (detected.addr, detected.source))
        .or_else(detect_platform_vault_addr);
    let credential_process =
        store.read(|config| !config.vault_credential_process.trim().is_empty());
    let (addr, source) = match detected {
        Some((addr, source)) => (Some(addr), Some(source)),
        None => (None, None),
    };
    VaultEnvironment {
        addr,
        source,
        token_available: credential_process || vault::get_vault_token().is_ok(),
    }
}

/// Database configs under `mount` that expose at least one role the token
/// can read.
pub fn list_databases(
    store: &ConfigStore,
    mount: String,
    addr: Option<String>,
) -> impl Future<Output = GuiResult<Vec<String>>> + Send + 'static {
    let credential_process = store.read(|config| config.vault_credential_process.clone());
    run_detached("vault", move || async move {
        let auth = resolve_auth(addr.as_deref(), &credential_process).await?;
        let all = vault::list_vault_databases_with_auth(&mount, &auth)
            .await
            .map_err(|e| {
                GuiError::Vault(format_vault_error(
                    "Vault discovery failed",
                    format!("Failed to list Vault databases: {e}"),
                ))
            })?;
        let total = all.len();
        let usable = vault::filter_databases_with_available_roles_with_auth(&mount, all, &auth)
            .await
            .map_err(|e| {
                GuiError::Vault(format_vault_error(
                    "Vault discovery failed",
                    format!("Failed to filter accessible databases: {e}"),
                ))
            })?;
        if usable.is_empty() && total > 0 {
            return Err(GuiError::Vault(
                "This mount has database configs, but none expose a role your token can read."
                    .into(),
            ));
        }
        Ok(usable)
    })
}

/// Roles of `database` under `mount` the token can read.
pub fn list_roles(
    store: &ConfigStore,
    mount: String,
    database: String,
    addr: Option<String>,
) -> impl Future<Output = GuiResult<Vec<String>>> + Send + 'static {
    let credential_process = store.read(|config| config.vault_credential_process.clone());
    run_detached("vault", move || async move {
        let auth = resolve_auth(addr.as_deref(), &credential_process).await?;
        vault::get_available_roles_for_user_with_auth(&mount, &database, &auth)
            .await
            .map_err(|e| {
                GuiError::Vault(format_vault_error(
                    "Vault role lookup failed",
                    format!("Failed to list Vault roles: {e}"),
                ))
            })
    })
}

async fn resolve_auth(addr: Option<&str>, credential_process: &str) -> GuiResult<vault::VaultAuth> {
    let addr = addr.map(str::trim).filter(|addr| !addr.is_empty());
    let credential_process = Some(credential_process.trim()).filter(|p| !p.is_empty());
    vault::resolve_vault_auth(addr, credential_process)
        .await
        .map_err(|e| {
            GuiError::Vault(format_vault_error(
                "Vault authentication failed",
                e.to_string(),
            ))
        })
}

/// Turn Vault client errors into actionable messages.
pub fn format_vault_error(context: &str, error: impl Into<String>) -> String {
    let error = error.into();
    if error.contains("Vault address not set") {
        return format!(
            "{context}: no Vault address is configured.\n\nEnter it in the Vault address field, or export VAULT_ADDR before launching DBCrust."
        );
    }
    if error.contains("Vault token not found") || error.contains("Failed to read token file") {
        return format!(
            "{context}: no Vault token is available.\n\nSet VAULT_TOKEN, run `vault login`, or configure vault_credential_process, then try again."
        );
    }
    if error.contains("Failed to retrieve ACL permissions")
        || error.contains("Failed to retrieve path capabilities")
    {
        return format!(
            "{context}: this token cannot introspect its capabilities.\n\nDBCrust uses capability checks to hide databases and roles you cannot read. Ask for access to sys/capabilities-self, or connect with a full vault://role@mount/database URL.\n\nDetails: {error}"
        );
    }
    if error.contains("403 Forbidden") {
        return format!(
            "{context}: Vault returned 403 Forbidden.\n\nCheck that your token is valid and can list the mount and read the role paths.\n\nDetails: {error}"
        );
    }
    if error.contains("config not found") || error.contains("mounted at") {
        return format!(
            "{context}: the mount path was not found.\n\nCheck the secrets engine mount path and that your token can list it.\n\nDetails: {error}"
        );
    }
    error
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output =
        crate::env::output_with_timeout(Command::new(program).args(args), Duration::from_secs(2))?;
    let stdout = String::from_utf8_lossy(&output).trim().to_string();
    (!stdout.is_empty()).then_some(stdout)
}

#[cfg(target_os = "macos")]
fn detect_platform_vault_addr() -> Option<(String, String)> {
    command_stdout("/bin/launchctl", &["getenv", "VAULT_ADDR"])
        .map(|addr| (addr, "macOS launchctl user environment".to_string()))
}

#[cfg(target_os = "linux")]
fn detect_platform_vault_addr() -> Option<(String, String)> {
    let environment = command_stdout("systemctl", &["--user", "show-environment"])?;
    environment.lines().find_map(|line| {
        let addr = line.strip_prefix("VAULT_ADDR=")?.trim();
        (!addr.is_empty()).then(|| (addr.to_string(), "systemd user environment".to_string()))
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn detect_platform_vault_addr() -> Option<(String, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_errors_get_actionable_messages() {
        let message = format_vault_error("Vault failed", "Vault token not found anywhere");
        assert!(message.contains("VAULT_TOKEN"));
        assert_eq!(format_vault_error("x", "something else"), "something else");
    }
}
