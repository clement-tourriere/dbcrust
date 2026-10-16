//! `dbcrust broker …` front end: argument handling, prompts and output.

use super::BrokerError;
use super::admin::{Admin, AliasStatus, EnrollRequest, TightenRequest, validate_enrollment_url};
use super::audit::AuditLog;
use super::gateway::Gateway;
use super::hardening;
use super::paths::{
    BrokerMode, BrokerPaths, BrokerSettings, PathEnv, effective_uid, ensure_user_state_dir,
    service_permission_violations,
};
use super::privileges::{drop_privileges, enforce_binary_trust, user_ids};
use super::registry::Registry;
use super::server::{
    DEFAULT_MAX_CONNECTIONS, Server, ServerConfig, bind_socket_with, probe_socket,
};
use super::service::{self, InstallRequest, ServicePrompts, UninstallRequest};
use crate::cli::{BrokerAction, BrokerModeArg, OutputFormat};
use crate::secrets::{AgentAccess, KeySource, PassphraseFileBackend, SecretStore};
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;

/// The terminal questions of `install-service` / `uninstall-service`: the
/// broker core asks through [`ServicePrompts`], this is the `inquire` side.
struct TerminalPrompts;

impl ServicePrompts for TerminalPrompts {
    fn confirm(&mut self, question: &str, default: bool) -> Option<bool> {
        inquire::Confirm::new(question)
            .with_default(default)
            .prompt()
            .ok()
    }
}

fn mode_of(arg: Option<BrokerModeArg>) -> BrokerMode {
    match arg.unwrap_or_default() {
        BrokerModeArg::User => BrokerMode::User,
        BrokerModeArg::Service => BrokerMode::Service,
    }
}

fn resolve_paths(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    socket: &Option<String>,
) -> Result<BrokerPaths, BrokerError> {
    let mode = mode_of(mode);
    // Service mode never touches the invoking user's config directory
    let env = if mode == BrokerMode::User {
        PathEnv::from_process()
    } else {
        PathEnv {
            xdg_runtime_dir: None,
            config_dir: None,
        }
    };
    BrokerPaths::resolve(
        mode,
        state_dir.as_ref().map(PathBuf::from),
        socket.as_ref().map(PathBuf::from),
        &env,
    )
}

/// The secret store of a broker: the owner's regular store (user mode) or a
/// key-file-encrypted store inside the state directory (service mode).
pub fn open_store(paths: &BrokerPaths, create_key: bool) -> Arc<SecretStore> {
    match paths.mode {
        BrokerMode::User => crate::secrets::default_store(),
        BrokerMode::Service => {
            let backend = PassphraseFileBackend::new(
                paths.secrets_file(),
                KeySource::key_file(paths.key_file(), create_key),
            );
            Arc::new(SecretStore::with_backend(Arc::new(backend), None))
        }
    }
}

/// Prepare the state directory for an admin command.
fn prepare_state(paths: &BrokerPaths, create: bool) -> Result<(), BrokerError> {
    match paths.mode {
        BrokerMode::User => ensure_user_state_dir(&paths.state_dir),
        BrokerMode::Service => {
            if !paths.state_dir.exists() {
                if effective_uid() == 0 {
                    return Err(BrokerError::State(format!(
                        "state directory {} does not exist; create it owned by the service user first",
                        paths.state_dir.display()
                    )));
                }
                if create {
                    crate::secrets::fsutil::create_private_dir(&paths.state_dir)
                        .map_err(|e| BrokerError::State(e.to_string()))?;
                }
            }
            // After a privilege drop the files belong to the service user;
            // before it (root) the directory's owner is the expected one
            let expected = if effective_uid() == 0 {
                std::fs::metadata(&paths.state_dir)
                    .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
                    .map_err(|e| BrokerError::State(e.to_string()))?
            } else {
                effective_uid()
            };
            let violations = service_permission_violations(paths, expected)?;
            if violations.is_empty() {
                Ok(())
            } else {
                Err(BrokerError::State(format!(
                    "refusing to use the service state: {}",
                    violations.join("; ")
                )))
            }
        }
    }
}

/// `sudo dbcrust broker …` in service mode: become the owner of the state
/// directory so every file stays owned by the service user.
///
/// The drop is verified (ids re-read, `setuid(0)` must fail) and followed by
/// the process hardening: Linux resets the dumpable flag on credential
/// changes, so hardening has to come after.
fn drop_privileges_to_state_owner(paths: &BrokerPaths) -> Result<(), BrokerError> {
    use std::os::unix::fs::MetadataExt;
    if paths.mode != BrokerMode::Service || effective_uid() != 0 {
        return Ok(());
    }
    let meta = std::fs::metadata(&paths.state_dir)
        .map_err(|e| BrokerError::State(format!("{}: {e}", paths.state_dir.display())))?;
    let (uid, gid) = (meta.uid(), meta.gid());
    if uid == 0 {
        return Err(BrokerError::State(format!(
            "{} is owned by root; the broker's state must belong to an unprivileged service user",
            paths.state_dir.display()
        )));
    }
    drop_privileges(uid, gid, None)?;
    hardening::apply(false);
    Ok(())
}

/// Service-mode commands that run with the service's authority refuse to
/// start from a binary a normal user could have replaced.
fn require_trusted_binary(paths: &BrokerPaths, allow_untrusted: bool) -> Result<(), BrokerError> {
    if paths.mode == BrokerMode::Service {
        enforce_binary_trust(allow_untrusted)?;
    }
    Ok(())
}

fn wants_json(own: Option<OutputFormat>, root: Option<OutputFormat>) -> bool {
    matches!(own.or(root), Some(OutputFormat::Json | OutputFormat::Jsonl))
}

fn report(error: &BrokerError) -> i32 {
    eprintln!("dbcrust broker: {error}");
    error.exit_code()
}

fn init_logging() {
    // stderr only: the service manager captures it. No SQL text and no
    // secrets are logged by default: sqlx's own statement logging (slow
    // statement warnings carry the SQL) is switched off unless RUST_LOG
    // says otherwise.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=off"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .compact()
        .try_init();
}

/// Run one `dbcrust broker` command and return the process exit code.
pub async fn run(
    action: Option<&BrokerAction>,
    root_format: Option<OutputFormat>,
    interactive: bool,
) -> i32 {
    let result = match action {
        None => {
            println!(
                "dbcrust broker: serve | enroll | list | revoke | policy | status | install-service | uninstall-service (see `dbcrust broker --help`)"
            );
            return 0;
        }
        Some(BrokerAction::Serve {
            mode,
            state_dir,
            socket,
            drop_to,
            allow_root,
            allow_debugger,
            allow_untrusted_binary,
        }) => {
            serve(
                *mode,
                state_dir,
                socket,
                drop_to,
                *allow_root,
                *allow_debugger,
                *allow_untrusted_binary,
            )
            .await
        }
        Some(BrokerAction::Enroll {
            alias,
            url,
            allow_writes,
            max_rows,
            timeout,
            max_bytes,
            description,
            mode,
            state_dir,
            allow_untrusted_binary,
        }) => {
            let request = EnrollRequest {
                alias: alias.clone(),
                url: url.clone(),
                allow_writes: *allow_writes,
                max_rows: *max_rows,
                timeout_secs: *timeout,
                max_bytes: *max_bytes,
                description: description.clone(),
            };
            enroll(
                *mode,
                state_dir,
                request,
                interactive,
                *allow_untrusted_binary,
            )
            .await
        }
        Some(BrokerAction::List {
            format,
            mode,
            state_dir,
            allow_untrusted_binary,
        }) => list(
            *mode,
            state_dir,
            wants_json(*format, root_format),
            *allow_untrusted_binary,
        ),
        Some(BrokerAction::Revoke {
            alias,
            mode,
            state_dir,
            allow_untrusted_binary,
        }) => revoke(*mode, state_dir, alias, *allow_untrusted_binary),
        Some(BrokerAction::Policy {
            alias,
            max_rows,
            timeout,
            max_bytes,
            read_only,
            allow_writes,
            mode,
            state_dir,
            allow_untrusted_binary,
        }) => {
            if *allow_writes {
                Err(BrokerError::Forbidden(
                    "granting writes loosens the policy; re-enroll the alias with `dbcrust broker enroll … --allow-writes` (it needs the password again)"
                        .to_string(),
                ))
            } else {
                let request = TightenRequest {
                    max_rows: *max_rows,
                    timeout_secs: *timeout,
                    max_bytes: *max_bytes,
                    read_only: *read_only,
                };
                policy(*mode, state_dir, alias, request, *allow_untrusted_binary)
            }
        }
        Some(BrokerAction::Status {
            format,
            mode,
            state_dir,
            socket,
        }) => status(*mode, state_dir, socket, wants_json(*format, root_format)),
        Some(BrokerAction::InstallService {
            dry_run,
            add_user,
            no_add_user,
            target_os,
        }) => {
            service::install(
                &InstallRequest {
                    dry_run: *dry_run,
                    add_user: add_user.clone(),
                    no_add_user: *no_add_user,
                    target_os: target_os.clone(),
                },
                interactive,
                &mut TerminalPrompts,
            )
            .await
        }
        Some(BrokerAction::UninstallService {
            purge,
            dry_run,
            target_os,
        }) => {
            service::uninstall(
                &UninstallRequest {
                    purge: *purge,
                    dry_run: *dry_run,
                    target_os: target_os.clone(),
                },
                &mut TerminalPrompts,
            )
            .await
        }
    };
    match result {
        Ok(code) => code,
        Err(error) => report(&error),
    }
}

// ---- serve ---------------------------------------------------------------------

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        // Without signal handlers fall back to waiting forever; the process
        // is then stopped by SIGKILL
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

async fn serve(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    socket: &Option<String>,
    drop_to: &Option<String>,
    allow_root: bool,
    allow_debugger: bool,
    allow_untrusted_binary: bool,
) -> Result<i32, BrokerError> {
    init_logging();
    let paths = resolve_paths(mode, state_dir, socket)?;
    let euid = effective_uid();
    require_trusted_binary(&paths, allow_untrusted_binary)?;

    // `--drop-to USER`: started as root so the socket directory can be
    // created, then the broker becomes USER
    let drop_target = match drop_to {
        None => None,
        Some(_) if paths.mode != BrokerMode::Service => {
            return Err(BrokerError::Invalid(
                "--drop-to is for service mode (--mode service)".to_string(),
            ));
        }
        Some(_) if euid != 0 => {
            return Err(BrokerError::Invalid(
                "--drop-to needs the broker to be started as root".to_string(),
            ));
        }
        Some(name) => {
            let (uid, gid) = user_ids(name)
                .ok_or_else(|| BrokerError::Invalid(format!("no such user: {name}")))?;
            if uid == 0 {
                return Err(BrokerError::Invalid(
                    "--drop-to must name an unprivileged user".to_string(),
                ));
            }
            Some((name.clone(), uid, gid))
        }
    };

    let settings = match paths.mode {
        BrokerMode::Service => {
            if euid == 0 && !allow_root && drop_target.is_none() {
                return Err(BrokerError::Forbidden(
                    "refusing to run the service-mode broker as root; start it as the dedicated service user, as root with --drop-to USER, or (debugging only) pass --allow-root"
                        .to_string(),
                ));
            }
            // The state belongs to the user the broker will be
            let expected_uid = drop_target.as_ref().map_or(euid, |(_, uid, _)| *uid);
            let violations = service_permission_violations(&paths, expected_uid)?;
            if !violations.is_empty() {
                return Err(BrokerError::State(format!(
                    "refusing to start: {}",
                    violations.join("; ")
                )));
            }
            BrokerSettings::load(&paths.broker_config_file())?
        }
        BrokerMode::User => {
            ensure_user_state_dir(&paths.state_dir)?;
            BrokerSettings::default()
        }
    };

    // As root: bind the socket in its root-owned directory, give it to the
    // service user and the agents group, then become the service user and
    // prove it. Nothing secret has been opened yet.
    let mut listener = None;
    let mut own_uid = euid;
    if let Some((name, uid, gid)) = &drop_target {
        let bound = service::bind_socket_for_drop(&paths.socket, *uid, &settings, 0)?;
        drop_privileges(*uid, *gid, Some(name))?;
        own_uid = *uid;
        listener = Some(bound);
    }

    // Hardening after any privilege drop (the kernel resets the dumpable
    // flag on credential changes) and before any client or store exists
    let hardening = hardening::apply(allow_debugger);
    tracing::info!(
        "broker {} starting in {} mode as uid {} (umask set: {}, core dumps off: {}, debugger blocked: {}, scrubbed {} environment variables)",
        env!("CARGO_PKG_VERSION"),
        paths.mode.label(),
        own_uid,
        hardening.umask_set,
        hardening.core_dumps_disabled,
        hardening.debugger_blocked,
        hardening.scrubbed_env.len()
    );
    if !hardening.core_dumps_disabled {
        tracing::warn!("could not disable core dumps");
    }
    if !allow_debugger && !hardening.debugger_blocked {
        tracing::warn!("could not block debugger attach on this platform");
    }

    // The gateway enforces per-request deadlines; the CLI-wide timeout the
    // drivers read must not cut statements short
    crate::database::set_query_timeout_seconds(0);

    let store = open_store(&paths, false);
    // A locked passphrase store would otherwise prompt in the middle of the
    // first agent request: ask once, up front, when there is a terminal
    if paths.mode == BrokerMode::User
        && store.status().backend.state == crate::secrets::BackendState::Locked
    {
        if crate::secrets::is_interactive() {
            if let Err(e) = store.unlock() {
                tracing::warn!("secret store stays locked: {e}");
            }
        } else {
            tracing::warn!(
                "secret store is locked and there is no terminal to unlock it; agents get broker_locked until it is unlocked (dbcrust secrets unlock)"
            );
        }
    }
    let registry = Arc::new(Registry::new(paths.aliases_file(), store));
    let audit = Arc::new(AuditLog::new(paths.audit_file()));
    let gateway = Arc::new(Gateway::new(registry, audit));

    let listener = match listener {
        Some(listener) => listener,
        None => bind_socket_with(
            &paths.socket,
            paths.mode,
            &settings,
            // systemd's RuntimeDirectory is owned by the service user's own
            // group: hand it to the socket group so agents can traverse it
            paths.mode == BrokerMode::Service && is_managed_socket_dir(&paths.socket),
        )?,
    };
    let server = Server::new(
        gateway.clone(),
        ServerConfig {
            mode: paths.mode,
            own_uid,
            max_connections: settings.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS),
            settings,
        },
    );
    tracing::info!("listening on {}", paths.socket.display());
    server.serve(listener, shutdown_signal()).await;

    tracing::info!("shutting down");
    gateway.close_all().await;
    // After a privilege drop the socket directory is root's: leaving the
    // file is fine, the next start replaces the stale one
    let _ = std::fs::remove_file(&paths.socket);
    Ok(0)
}

/// The directories `install-service` manages for the socket
/// (`/run/dbcrust`, `/var/run/dbcrust`).
fn is_managed_socket_dir(socket: &std::path::Path) -> bool {
    socket.parent().is_some_and(|dir| {
        dir == std::path::Path::new("/run/dbcrust")
            || dir == std::path::Path::new("/var/run/dbcrust")
    })
}

// ---- enroll --------------------------------------------------------------------

fn prompt_password(user: &str, host: &str) -> Result<Zeroizing<String>, BrokerError> {
    let prompt = if host.is_empty() {
        format!("Database password for {user}:")
    } else {
        format!("Database password for {user}@{host}:")
    };
    let value = inquire::Password::new(&prompt)
        .without_confirmation()
        .with_help_message("stored by the broker; agents never see it")
        .prompt()
        .map_err(|e| match e {
            inquire::InquireError::OperationCanceled
            | inquire::InquireError::OperationInterrupted => {
                BrokerError::Interactive("enrollment cancelled".to_string())
            }
            other => BrokerError::Interactive(format!("could not read the password: {other}")),
        })?;
    Ok(Zeroizing::new(value))
}

async fn enroll(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    request: EnrollRequest,
    interactive: bool,
    allow_untrusted_binary: bool,
) -> Result<i32, BrokerError> {
    // Validate everything that needs no prompt first
    request.validate()?;
    let target = validate_enrollment_url(&request.url)?;
    let paths = resolve_paths(mode, state_dir, &None)?;
    // As root (`sudo`) this binary must be one an unprivileged user cannot swap
    if effective_uid() == 0 {
        require_trusted_binary(&paths, allow_untrusted_binary)?;
    }
    prepare_state(&paths, true)?;

    let existing = Admin::new(
        paths.aliases_file(),
        // listing needs the store; open lazily for the existence check only
        open_store(&paths, false),
    )
    .list()
    .ok()
    .map(|all| all.iter().any(|a| a.alias == request.alias))
    .unwrap_or(false);

    // The password is never taken from argv, the environment or a pipe
    if (target.needs_password || existing) && !interactive {
        return Err(BrokerError::Interactive(
            "enrollment needs a terminal: the password is read interactively and never from arguments, the environment or a pipe"
                .to_string(),
        ));
    }
    if existing {
        let proceed = inquire::Confirm::new(&format!(
            "Alias {:?} already exists. Re-enroll it with the policy given here?",
            request.alias
        ))
        .with_default(false)
        .prompt()
        .map_err(|_| BrokerError::Interactive("enrollment cancelled".to_string()))?;
        if !proceed {
            println!("Nothing changed.");
            return Ok(0);
        }
    }
    let password = if target.needs_password {
        Some(prompt_password(&target.binding.user, &target.binding.host)?)
    } else {
        None
    };

    // Terminal work is done: from here on act as the state directory's owner
    drop_privileges_to_state_owner(&paths)?;
    let store = open_store(&paths, true);
    let admin = Admin::new(paths.aliases_file(), store);
    let outcome = admin.enroll(&request, password).await?;

    println!(
        "Enrolled {:?} → {} ({} access, max {} rows, {}s timeout, revision {}{})",
        outcome.alias,
        outcome.target,
        match outcome.policy.access {
            AgentAccess::ReadOnly => "read-only",
            AgentAccess::ReadWrite => "read-write",
        },
        outcome.policy.max_rows,
        outcome.policy.timeout_secs,
        outcome.revision,
        if outcome.replaced { ", replaced" } else { "" },
    );
    if paths.mode == BrokerMode::Service {
        println!("The service picks the change up on its own; agents use `dbcrust-agent`.");
    } else {
        println!("Start the broker with `dbcrust broker serve`; agents use `dbcrust-agent`.");
    }
    Ok(0)
}

// ---- list / revoke / policy ----------------------------------------------------

fn admin_for(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    allow_untrusted_binary: bool,
) -> Result<(BrokerPaths, Admin), BrokerError> {
    let paths = resolve_paths(mode, state_dir, &None)?;
    if effective_uid() == 0 {
        require_trusted_binary(&paths, allow_untrusted_binary)?;
    }
    prepare_state(&paths, false)?;
    drop_privileges_to_state_owner(&paths)?;
    let store = open_store(&paths, false);
    let admin = Admin::new(paths.aliases_file(), store);
    Ok((paths, admin))
}

fn list(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    json: bool,
    allow_untrusted_binary: bool,
) -> Result<i32, BrokerError> {
    let (paths, admin) = admin_for(mode, state_dir, allow_untrusted_binary)?;
    let aliases = admin.list()?;
    if json {
        let value = serde_json::json!({
            "mode": paths.mode.label(),
            "state_dir": paths.state_dir,
            "aliases": aliases,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return Ok(0);
    }
    if aliases.is_empty() {
        println!(
            "No aliases enrolled ({} mode, {}).",
            paths.mode.label(),
            paths.state_dir.display()
        );
        println!("Enroll one with `dbcrust broker enroll <alias> <url>`.");
        return Ok(0);
    }
    print_aliases(&aliases);
    Ok(0)
}

fn print_aliases(aliases: &[AliasStatus]) {
    for a in aliases {
        println!(
            "{}  [{}]  {}",
            a.alias,
            a.engine,
            a.target.as_deref().unwrap_or("-")
        );
        println!(
            "    access: {}   max rows: {}   timeout: {}s   max bytes: {}   revision: {}   state: {}",
            a.access.as_deref().unwrap_or("-"),
            a.max_rows.map_or_else(|| "-".into(), |v| v.to_string()),
            a.timeout_secs.map_or_else(|| "-".into(), |v| v.to_string()),
            a.max_bytes.map_or_else(|| "-".into(), |v| v.to_string()),
            a.revision.map_or_else(|| "-".into(), |v| v.to_string()),
            a.state
        );
        if let Some(description) = &a.description {
            println!("    {description}");
        }
    }
}

fn revoke(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    alias: &str,
    allow_untrusted_binary: bool,
) -> Result<i32, BrokerError> {
    let (_, admin) = admin_for(mode, state_dir, allow_untrusted_binary)?;
    if admin.revoke(alias)? {
        println!("Revoked {alias:?}: its credential was deleted and agents can no longer use it.");
        Ok(0)
    } else {
        eprintln!("dbcrust broker: no alias named {alias:?}");
        Ok(1)
    }
}

fn policy(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    alias: &str,
    request: TightenRequest,
    allow_untrusted_binary: bool,
) -> Result<i32, BrokerError> {
    if request.max_rows.is_none()
        && request.timeout_secs.is_none()
        && request.max_bytes.is_none()
        && !request.read_only
    {
        return Err(BrokerError::Invalid(
            "nothing to change: give --max-rows, --timeout, --max-bytes or --read-only".to_string(),
        ));
    }
    let (_, admin) = admin_for(mode, state_dir, allow_untrusted_binary)?;
    let policy = admin.tighten(alias, request)?;
    println!(
        "Policy for {alias:?}: {} access, max {} rows, {}s timeout, {} bytes.",
        match policy.access {
            AgentAccess::ReadOnly => "read-only",
            AgentAccess::ReadWrite => "read-write",
        },
        policy.max_rows,
        policy.timeout_secs,
        policy.max_bytes
    );
    Ok(0)
}

// ---- status --------------------------------------------------------------------

fn status(
    mode: Option<BrokerModeArg>,
    state_dir: &Option<String>,
    socket: &Option<String>,
    json: bool,
) -> Result<i32, BrokerError> {
    let paths = resolve_paths(mode, state_dir, socket)?;
    let state_exists = paths.state_dir.exists();
    let (aliases, store_state, store_detail) = if state_exists {
        let store = open_store(&paths, false);
        let status = store.status();
        let listing = Admin::new(paths.aliases_file(), store).list().ok();
        (
            listing,
            format!("{:?}", status.backend.state).to_lowercase(),
            status.backend.detail,
        )
    } else {
        (None, "no state directory".to_string(), String::new())
    };
    let permissions = if paths.mode == BrokerMode::Service && state_exists {
        service_permission_violations(&paths, effective_uid()).ok()
    } else {
        None
    };
    let running = probe_socket(&paths.socket);
    let service_status = (paths.mode == BrokerMode::Service)
        .then(|| service::Platform::current().map(service::service_status))
        .flatten();
    let audit_bytes = std::fs::metadata(paths.audit_file()).map(|m| m.len()).ok();

    if json {
        let value = serde_json::json!({
            "mode": paths.mode.label(),
            "state_dir": paths.state_dir,
            "state_dir_exists": state_exists,
            "socket": paths.socket,
            "running": running.is_some(),
            "broker_version": running,
            "store": { "state": store_state, "detail": store_detail },
            "aliases": aliases.as_ref().map(Vec::len),
            "audit_bytes": audit_bytes,
            "permission_violations": permissions,
            "service": service_status,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return Ok(0);
    }

    println!("Broker status");
    println!("  mode:       {}", paths.mode.label());
    println!(
        "  state dir:  {}{}",
        paths.state_dir.display(),
        if state_exists { "" } else { " (missing)" }
    );
    println!("  socket:     {}", paths.socket.display());
    match &running {
        Some(version) => println!("  running:    yes (broker {version})"),
        None => println!("  running:    no (nothing answers on the socket)"),
    }
    println!("  store:      {store_state} {store_detail}");
    match &aliases {
        Some(list) => println!("  aliases:    {}", list.len()),
        None => println!("  aliases:    unknown"),
    }
    if let Some(bytes) = audit_bytes {
        println!("  audit log:  {bytes} bytes");
    }
    if let Some(violations) = permissions {
        if violations.is_empty() {
            println!("  perms:      ok");
        } else {
            for v in violations {
                println!("  perms:      PROBLEM {v}");
            }
        }
    }
    if let Some(service_status) = &service_status {
        service_status.print();
    }
    Ok(0)
}
