//! Opening a `Database` for a [`ConnectRequest`]. Runs on the connection's
//! own worker thread (the sqlx pools must live on the runtime that created
//! them).

use crate::docker::{DockerFailure, DockerPhase, DockerRoute};
use crate::error::{GuiError, GuiResult};
use crate::ids::ConnectionId;
use crate::store::ConfigStore;
use crate::types::{ConnectRequest, ConnectTarget, ConnectionSummary};
use crate::vault::format_vault_error;
use dbcrust::config::Config;
use dbcrust::database::{ConnectionInfo, DatabaseType, DatabaseTypeExt};
use dbcrust::db::{Database, FrontendMode};
use dbcrust::docker::DockerTarget;
use dbcrust::password_sanitizer::sanitize_connection_url;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::RwLock;

/// An opened database plus what the UI needs to know about it.
pub(crate) struct Opened {
    pub database: Database,
    pub summary: ConnectionSummary,
    pub default_limit: usize,
    /// How a Docker container was reached.
    pub route: Option<DockerRoute>,
}

/// What an attempt is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectMode {
    /// Open a live connection and remember it in the recent list.
    Open,
    /// Same authentication, TLS and read-only setup, then close: nothing is
    /// recorded.
    Test,
}

/// A step of connecting, as the UI shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectPhase {
    /// Preparing the attempt (reading the target, Vault credentials...).
    Starting,
    /// Docker: asking the daemon about the container.
    ResolvingDocker,
    /// Docker: probing the published port, OrbStack domains, container address.
    CheckingDirectAccess,
    /// Docker: setting up forwarding through `docker exec`.
    OpeningForwarding,
    /// The database handshake (authentication, TLS, read-only setup).
    Authenticating,
}

impl ConnectPhase {
    pub fn label(self) -> &'static str {
        match self {
            ConnectPhase::Starting => "Starting",
            ConnectPhase::ResolvingDocker => DockerPhase::ResolvingDocker.label(),
            ConnectPhase::CheckingDirectAccess => DockerPhase::CheckingDirectAccess.label(),
            ConnectPhase::OpeningForwarding => DockerPhase::OpeningForwarding.label(),
            ConnectPhase::Authenticating => DockerPhase::Authenticating.label(),
        }
    }
}

impl From<DockerPhase> for ConnectPhase {
    fn from(phase: DockerPhase) -> Self {
        match phase {
            DockerPhase::ResolvingDocker => ConnectPhase::ResolvingDocker,
            DockerPhase::CheckingDirectAccess => ConnectPhase::CheckingDirectAccess,
            DockerPhase::OpeningForwarding => ConnectPhase::OpeningForwarding,
            DockerPhase::Authenticating => ConnectPhase::Authenticating,
        }
    }
}

/// Why a connect failed: the message for the user, plus the typed details of
/// a Docker "no route" failure.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConnectError {
    pub error: GuiError,
    pub docker: Option<Box<DockerFailure>>,
}

impl From<GuiError> for ConnectError {
    fn from(error: GuiError) -> Self {
        Self {
            error,
            docker: None,
        }
    }
}

/// A failed database construction. A Docker "no route" failure keeps its
/// own text (it already says what happened); everything else is prefixed.
fn connection_error(error: Box<dyn std::error::Error>, prefix: &str) -> ConnectError {
    match DockerFailure::from_error(error.as_ref()) {
        Some(failure) => ConnectError {
            error: GuiError::Connection(failure.message.clone()),
            docker: Some(Box::new(failure)),
        },
        None => GuiError::Connection(format!(
            "{prefix}{}",
            dbcrust::password_sanitizer::redact_error(&error)
        ))
        .into(),
    }
}

/// The `docker://` target of a request: the URL itself, or fully resolved
/// connection details of a Docker connection (a sibling database of an open
/// container connection), whose host and port belong to a relay that is gone
/// by now. A sibling's info also carries the daemon pin of its parent.
fn docker_template(target: &ConnectTarget, url: &str) -> Option<ConnectionInfo> {
    match target {
        ConnectTarget::Info(info)
            if info
                .docker_container
                .as_deref()
                .is_some_and(|name| !name.is_empty()) =>
        {
            Some((**info).clone())
        }
        ConnectTarget::Info(_) => None,
        _ if url.starts_with("docker://") => {
            DockerTarget::parse(url).map(|target| target.to_connection_info())
        }
        _ => None,
    }
}

/// How long a new connection may wait for others to finish being built.
const CONSTRUCTION_WAIT: Duration = Duration::from_secs(30);

/// Serialises the part of connecting that depends on the process-wide
/// read-only request: the engines read it while building their connect
/// options (SQLite even after connecting), so a read-only construction runs
/// alone with the flag set, and ordinary ones run side by side with it
/// clear. An async lock, so waiting never blocks a worker's runtime and a
/// cancelled or timed-out attempt just stops waiting.
///
/// The handshake has to stay inside: the flag is read until the pool is up
/// (a per-thread flag in the core would make this gate unnecessary). Config
/// files are *not* touched (constructors get the store's settings), so the
/// UI's own config reads and writes never wait on a handshake.
#[derive(Default)]
struct ConstructionGate(RwLock<()>);

/// Held while one `Database` is being built.
#[allow(dead_code)]
enum GateGuard {
    Shared(tokio::sync::RwLockReadGuard<'static, ()>),
    Exclusive(tokio::sync::RwLockWriteGuard<'static, ()>),
}

impl ConstructionGate {
    /// Wait for the gate: shared for ordinary constructions, exclusive for
    /// read-only ones.
    async fn acquire(&'static self, exclusive: bool) -> GateGuard {
        if exclusive {
            GateGuard::Exclusive(self.0.write().await)
        } else {
            GateGuard::Shared(self.0.read().await)
        }
    }
}

fn global_gate() -> &'static ConstructionGate {
    static GATE: OnceLock<ConstructionGate> = OnceLock::new();
    GATE.get_or_init(ConstructionGate::default)
}

/// A construction slot of the global gate, with the read-only request set
/// for as long as it is held.
struct ConstructionSlot {
    // Field order is drop order: clear the flag, then let others in.
    _flag: Option<ReadOnlyRequest>,
    _guard: GateGuard,
}

async fn construction_slot(read_only: bool) -> ConstructionSlot {
    let guard = global_gate().acquire(read_only).await;
    ConstructionSlot {
        _flag: read_only.then(|| ReadOnlyRequest::set(true)),
        _guard: guard,
    }
}

/// [`construction_slot`], giving up after [`CONSTRUCTION_WAIT`].
async fn construction_slot_within(read_only: bool) -> Result<ConstructionSlot, ConnectError> {
    tokio::time::timeout(CONSTRUCTION_WAIT, construction_slot(read_only))
        .await
        .map_err(|_| {
            GuiError::Connection(
                "Another connection is still being set up. Try again in a moment.".into(),
            )
            .into()
        })
}

/// Resolve the request to a URL, construct the `Database`, and (for
/// [`ConnectMode::Open`]) record the connection in the recent list.
/// `report` is told each [`ConnectPhase`] as it starts.
pub(crate) async fn open(
    id: ConnectionId,
    request: &ConnectRequest,
    store: &ConfigStore,
    mode: ConnectMode,
    report: &dyn Fn(ConnectPhase),
) -> Result<Opened, ConnectError> {
    report(ConnectPhase::Starting);
    let (url, session_name) = resolve_url(request, store)?;
    // One snapshot of the settings for the whole attempt: constructors get it
    // instead of re-reading the config files under a global lock
    let config = Arc::new(store.snapshot());
    let (limit, expanded, credential_process, read_only_default) = (
        config.default_limit,
        config.expanded_display_default,
        config.vault_credential_process.clone(),
        config.read_only_default,
    );
    // `read_only_default` (Settings) makes every connection read-only; an
    // explicit request can only add protection, never remove it
    let read_only = request.read_only || read_only_default;

    // Vault authentication and credential issuance can be slow (or wait on
    // a credential process): resolve them before taking a construction slot
    // so they never hold up other connections
    let vault = if url.starts_with("vault://") {
        Some(
            resolve_vault(
                &url,
                request.vault_addr.as_deref(),
                &credential_process,
                store,
                mode,
            )
            .await?,
        )
    } else {
        None
    };
    let docker = if vault.is_none() {
        docker_template(&request.target, &url)
    } else {
        None
    };
    let ssh_tunnel = match (&request.target, &docker) {
        // A Docker container is reached through Docker, never an SSH jump
        (_, Some(_)) => None,
        // May run shell substitutions: `config` is already our own copy
        (ConnectTarget::Info(info), None) => info
            .host
            .as_ref()
            .and_then(|host| config.get_ssh_tunnel_for_host(host)),
        _ => None,
    };

    let mut route = None;
    let (mut database, connection_info) = if let Some(template) = docker {
        // Container lookup, probing and forwarding are bounded and take no
        // lock: an unreachable container must not stall other connections.
        // Only the database handshake itself takes a construction slot.
        let build_config = Arc::clone(&config);
        let connected = Database::connect_docker_container(
            &template,
            |phase| report(phase.into()),
            |info| {
                let config = Arc::clone(&build_config);
                async move {
                    let _slot = construction_slot(read_only).await;
                    Database::from_connection_info_with_config(
                        info,
                        Some(limit),
                        Some(expanded),
                        None,
                        FrontendMode::Gui,
                        &config,
                    )
                    .await
                }
            },
        )
        .await
        .map_err(|e| connection_error(e, "Connection failed: "))?;
        route = Some(connected.route);
        (connected.database, Some(connected.connection_info))
    } else {
        report(ConnectPhase::Authenticating);
        let _slot = construction_slot_within(read_only).await?;
        let connected = if let Some(vault) = vault {
            Database::from_url_with_config(
                &vault.postgres_url,
                &config,
                Some(limit),
                Some(expanded),
                FrontendMode::Gui,
            )
            .await
            .map(|mut database| {
                database.set_connection_info_override(vault.info.clone());
                (database, Some(vault.info))
            })
            .map_err(|e| connection_error(e, "Could not connect with Vault credentials: "))
        } else if let ConnectTarget::Info(info) = &request.target {
            Database::from_connection_info_with_config(
                (**info).clone(),
                Some(limit),
                Some(expanded),
                ssh_tunnel,
                FrontendMode::Gui,
                &config,
            )
            .await
            .map(|database| (database, None))
            .map_err(|e| connection_error(e, "Connection failed: "))
        } else {
            Database::from_url_with_config(
                &url,
                &config,
                Some(limit),
                Some(expanded),
                FrontendMode::Gui,
            )
            .await
            .map(|database| (database, None))
            .map_err(|e| connection_error(e, "Connection failed: "))
        };
        connected?
    };

    database.set_read_only(read_only);
    // The GUI has a dedicated Explain action; never silently swap results
    // for plans because of the CLI's `explain_mode_default`.
    if database.is_explain_mode() {
        database.toggle_explain_mode();
    }

    let current_url = match (&connection_info, url.starts_with("vault://")) {
        (Some(info), true) => complete_vault_url(info, &url),
        _ => url.clone(),
    };
    if mode == ConnectMode::Open {
        record_recent(store, &url, &database, connection_info.as_ref());
    }

    let display_url = sanitize_connection_url(&current_url);
    let database_name = database.get_current_db();
    // A container connection is identified by the container, not by the
    // address it happened to be reached at (a relay's port changes per run)
    let docker_container = connection_info
        .as_ref()
        .and_then(|info| info.docker_container.clone())
        .filter(|name| !name.is_empty());
    let (host, port) = match (&docker_container, &connection_info) {
        (Some(container), Some(info)) => (
            container.clone(),
            info.port.unwrap_or_else(|| database.get_port()),
        ),
        _ => (database.get_host(), database.get_port()),
    };
    let label = session_name
        .clone()
        .unwrap_or_else(|| default_label(&database.get_database_type(), &database_name, &host));
    let summary = ConnectionSummary {
        id,
        db_type: database.get_database_type(),
        database: database_name,
        host,
        port,
        user: database.get_username(),
        display_url,
        label,
        session_name,
        read_only,
    };
    Ok(Opened {
        database,
        summary,
        default_limit: limit,
        route,
    })
}

/// Sets DBCrust's process-wide read-only request for the duration of one
/// `Database` construction, and always clears it — even if construction
/// fails or panics.
struct ReadOnlyRequest;

impl ReadOnlyRequest {
    fn set(read_only: bool) -> Self {
        dbcrust::database::set_read_only_requested(read_only);
        Self
    }
}

impl Drop for ReadOnlyRequest {
    fn drop(&mut self) {
        dbcrust::database::set_read_only_requested(false);
    }
}

/// A short label: the database (file name for files), plus the host when
/// it is not local.
pub(crate) fn default_label(db_type: &DatabaseType, database: &str, host: &str) -> String {
    let database = if matches!(
        db_type,
        DatabaseType::SQLite
            | DatabaseType::Parquet
            | DatabaseType::CSV
            | DatabaseType::JSON
            | DatabaseType::DuckDB
    ) {
        std::path::Path::new(database)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| database.to_string())
    } else {
        database.to_string()
    };
    let local = matches!(host, "" | "unknown" | "localhost" | "127.0.0.1" | "::1");
    match (database.is_empty(), local) {
        (true, true) => db_type.display_name().to_string(),
        (true, false) => host.to_string(),
        (false, true) => database,
        (false, false) => format!("{database} ({host})"),
    }
}

fn resolve_url(
    request: &ConnectRequest,
    store: &ConfigStore,
) -> GuiResult<(String, Option<String>)> {
    match &request.target {
        ConnectTarget::Url(url) => {
            let url = url.trim();
            if url.is_empty() {
                return Err(GuiError::Connection("Enter a connection URL".into()));
            }
            Ok((url.to_string(), None))
        }
        ConnectTarget::Session(name) => {
            let session = store
                .read(|config| config.get_session(name).cloned())
                .ok_or_else(|| {
                    GuiError::Connection(format!("Saved session \"{name}\" not found"))
                })?;
            let url = session
                .reconstruct_connection_url()
                .map_err(|e| GuiError::Connection(format!("Could not open \"{name}\": {e}")))?;
            Ok((url, Some(name.clone())))
        }
        ConnectTarget::Info(info) => Ok((
            // A container connection is identified by its canonical target
            // (never the relay port it happened to use, never a password)
            DockerTarget::from_connection_info(info)
                .map(|target| target.render())
                .unwrap_or_else(|| info.to_url()),
            None,
        )),
        ConnectTarget::Recent(entry) => {
            let url = entry.connection.reconstruct_connection_url().map_err(|e| {
                GuiError::Connection(format!("Could not reopen {}: {e}", entry.display_name))
            })?;
            Ok((url, None))
        }
    }
}

fn record_recent(
    store: &ConfigStore,
    url: &str,
    database: &Database,
    connection_info: Option<&ConnectionInfo>,
) {
    let result = store.update_recents(|config: &mut Config| {
        if url.starts_with("vault://") {
            match connection_info {
                Some(info) => config.add_recent_connection_with_options(
                    complete_vault_url(info, url),
                    DatabaseType::PostgreSQL,
                    true,
                    info.options.clone(),
                ),
                None => Ok(()),
            }
        } else if let Some(target) = connection_info.and_then(DockerTarget::from_connection_info) {
            // The canonical container target (user, database, options): never
            // the password, never the address a relay happened to use
            let database_type = connection_info
                .map(|info| info.database_type.clone())
                .unwrap_or(DatabaseType::PostgreSQL);
            config.add_recent_connection(
                target.render(),
                docker_display_name(&target),
                database_type,
                true,
            )
        } else if let Some(info) = connection_info {
            config.add_recent_connection_auto_display(
                sanitize_connection_url(&info.to_url()),
                info.database_type.clone(),
                true,
            )
        } else {
            config.add_recent_connection_auto_display(
                sanitize_connection_url(url),
                database.get_database_type(),
                true,
            )
        }
    });
    if let Err(e) = result {
        tracing::warn!("could not record recent connection: {e}");
    }
}

/// `user@container/database (Docker: container)`: the name a Docker recent
/// is listed under.
fn docker_display_name(target: &DockerTarget) -> String {
    let mut name = String::new();
    if let Some(user) = target.user.as_deref().filter(|u| !u.is_empty()) {
        name.push_str(user);
        name.push('@');
    }
    name.push_str(&target.container);
    if let Some(database) = target.database.as_deref().filter(|d| !d.is_empty()) {
        name.push('/');
        name.push_str(database);
    }
    format!("{name} (Docker: {})", target.container)
}

fn complete_vault_url(info: &ConnectionInfo, fallback: &str) -> String {
    match (
        info.options.get("vault_mount"),
        info.options.get("vault_database"),
        info.options.get("vault_role"),
    ) {
        (Some(mount), Some(database), Some(role)) if role.is_empty() => {
            format!("vault://{mount}/{database}")
        }
        (Some(mount), Some(database), Some(role)) => format!("vault://{role}@{mount}/{database}"),
        _ => fallback.to_string(),
    }
}

/// The settings Vault credential handling runs with: a test connection reads
/// the credential cache but never writes to it.
fn vault_config_for(mode: ConnectMode, mut config: Config) -> Config {
    if mode == ConnectMode::Test {
        config.vault_credential_cache_enabled = false;
    }
    config
}

/// A Vault-issued PostgreSQL login, resolved before connecting.
struct VaultLogin {
    postgres_url: String,
    info: ConnectionInfo,
}

async fn resolve_vault(
    url: &str,
    vault_addr: Option<&str>,
    credential_process: &str,
    store: &ConfigStore,
    mode: ConnectMode,
) -> GuiResult<VaultLogin> {
    use dbcrust::vault_client as vault;

    let (role, mount, database) = vault::parse_vault_url(url).ok_or_else(|| {
        GuiError::Vault(format!(
            "Invalid Vault URL: {}",
            dbcrust::password_sanitizer::redact_url(url)
        ))
    })?;
    let database = database.ok_or_else(|| {
        GuiError::Vault("Vault connections need a database: vault://role@mount/database".into())
    })?;
    let role = role.ok_or_else(|| {
        GuiError::Vault("Vault connections need a role: vault://role@mount/database".into())
    })?;

    let vault_addr = vault_addr.map(str::trim).filter(|addr| !addr.is_empty());
    let credential_process = Some(credential_process.trim()).filter(|process| !process.is_empty());
    let auth = vault::resolve_vault_auth(vault_addr, credential_process)
        .await
        .map_err(|e| {
            GuiError::Vault(format_vault_error(
                "Vault authentication failed",
                e.to_string(),
            ))
        })?;

    // Credential caching mutates (and persists) the vault credential store:
    // a test connection must not leave credentials behind
    let mut config = vault_config_for(mode, store.snapshot());
    let (credentials, _lease) = vault::get_dynamic_credentials_with_caching_with_auth(
        &mount,
        &database,
        &role,
        &mut config,
        &auth,
    )
    .await
    .map_err(|e| {
        GuiError::Vault(format_vault_error(
            "Vault connection failed",
            format!("Failed to get Vault credentials: {e}"),
        ))
    })?;

    let db_config = vault::get_vault_database_config_with_auth(&mount, &database, &auth)
        .await
        .map_err(|e| {
            GuiError::Vault(format_vault_error(
                "Vault connection failed",
                format!("Failed to get database config from Vault: {e}"),
            ))
        })?;
    let template = db_config
        .connection_details
        .connection_url
        .as_ref()
        .ok_or_else(|| GuiError::Vault("Vault's database config has no connection URL".into()))?;
    let postgres_url =
        vault::construct_postgres_url(template, &credentials.username, &credentials.password)
            .map_err(|e| GuiError::Vault(format!("Could not build the connection URL: {e}")))?;

    let template_info = ConnectionInfo::parse_url(template)
        .map_err(|e| GuiError::Vault(format!("Could not parse Vault's connection URL: {e}")))?;
    // Keep the template's connection options (sslmode, …): siblings opened
    // from this connection must not lose TLS verification
    let mut options = template_info.options.clone();
    options.extend([
        ("vault_mount".to_string(), mount),
        ("vault_database".to_string(), database),
        ("vault_role".to_string(), role),
    ]);
    let info = ConnectionInfo {
        database_type: DatabaseType::PostgreSQL,
        host: template_info.host.clone(),
        port: template_info.port,
        username: Some(credentials.username.clone()),
        password: Some(credentials.password),
        database: template_info.database.clone(),
        file_path: None,
        options,
        docker_container: None,
        use_tls: template_info.use_tls,
    };
    Ok(VaultLogin { postgres_url, info })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(DatabaseType::PostgreSQL, "app", "localhost", "app")]
    #[case(
        DatabaseType::PostgreSQL,
        "app",
        "db.prod.internal",
        "app (db.prod.internal)"
    )]
    #[case(DatabaseType::SQLite, "/tmp/data/shop.db", "unknown", "shop.db")]
    #[case(DatabaseType::Elasticsearch, "", "search.local", "search.local")]
    fn labels(
        #[case] db_type: DatabaseType,
        #[case] database: &str,
        #[case] host: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(default_label(&db_type, database, host), expected);
    }

    #[test]
    fn vault_url_completion() {
        let info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: std::collections::HashMap::from([
                ("vault_mount".to_string(), "database".to_string()),
                ("vault_database".to_string(), "app".to_string()),
                ("vault_role".to_string(), "ro".to_string()),
            ]),
            docker_container: None,
            use_tls: false,
        };
        assert_eq!(complete_vault_url(&info, "x"), "vault://ro@database/app");
    }

    fn docker_info(container: &str, database: &str) -> ConnectionInfo {
        // What a sibling request carries: the details of an open container
        // connection, address of its (now gone) relay included.
        ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some("127.0.0.1".into()),
            port: Some(54321),
            username: Some("app".into()),
            password: Some("p@ss:w/rd".into()),
            database: Some(database.into()),
            file_path: None,
            options: std::collections::HashMap::new(),
            docker_container: Some(container.into()),
            use_tls: false,
        }
    }

    #[test]
    fn a_sibling_of_a_container_connection_resolves_the_container_again() {
        let mut info = docker_info("tt2-postgres-1", "analytics");
        info.options.insert(
            dbcrust::docker::PINNED_DAEMON_OPTION.into(),
            "unix:///a.sock".into(),
        );
        let target = ConnectTarget::Info(Box::new(info.clone()));
        // Credentials with URL-special characters travel in the info, not in
        // a URL; the relay's address is ignored by the container flow.
        let template = docker_template(&target, "unused").expect("docker flow");
        assert_eq!(template.password.as_deref(), Some("p@ss:w/rd"));
        assert_eq!(template.docker_container.as_deref(), Some("tt2-postgres-1"));
        assert_eq!(template.database.as_deref(), Some("analytics"));
        // The daemon of the parent travels with it
        assert_eq!(
            template
                .options
                .get(dbcrust::docker::PINNED_DAEMON_OPTION)
                .map(String::as_str),
            Some("unix:///a.sock")
        );

        // Identity (summary, drafts) is the canonical container target: the
        // user and database, no password, no relay address, no daemon pin.
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default());
        let request = ConnectRequest::info(info);
        assert_eq!(
            resolve_url(&request, &store).unwrap().0,
            "docker://app@tt2-postgres-1/analytics"
        );
    }

    #[rstest]
    #[case(ConnectTarget::Url("docker://tt2-postgres-1/app".into()), "docker://tt2-postgres-1/app", true)]
    #[case(ConnectTarget::Url("postgres://localhost/app".into()), "postgres://localhost/app", false)]
    #[case(ConnectTarget::Url("vault://role@database/app".into()), "vault://role@database/app", false)]
    fn only_docker_targets_use_the_container_flow(
        #[case] target: ConnectTarget,
        #[case] url: &str,
        #[case] docker: bool,
    ) {
        assert_eq!(docker_template(&target, url).is_some(), docker);
    }

    #[test]
    fn a_docker_url_keeps_its_user_database_and_options_in_the_template() {
        let target = ConnectTarget::Url("docker://app:pw@db/shop?sslmode=require&ssl=true".into());
        let template = docker_template(&target, "docker://app:pw@db/shop?sslmode=require&ssl=true")
            .expect("docker flow");
        assert_eq!(template.username.as_deref(), Some("app"));
        assert_eq!(template.password.as_deref(), Some("pw"));
        assert_eq!(template.database.as_deref(), Some("shop"));
        assert_eq!(
            template.options.get("sslmode").map(String::as_str),
            Some("require")
        );
        assert!(template.use_tls);
    }

    #[test]
    fn a_plain_info_never_takes_the_container_flow() {
        let mut info = docker_info("x", "app");
        info.docker_container = None;
        assert!(docker_template(&ConnectTarget::Info(Box::new(info.clone())), "").is_none());
        info.docker_container = Some(String::new());
        assert!(docker_template(&ConnectTarget::Info(Box::new(info)), "").is_none());
    }

    #[test]
    fn docker_recents_keep_user_database_and_options_but_never_a_password_or_port() {
        crate::test_support::isolate();
        let store = ConfigStore::from_config(Config::default());
        let mut info = docker_info("tt2-postgres-1", "analytics");
        info.options.insert("sslmode".into(), "require".into());
        // A forwarded connection records the container, not the relay port
        info.host = Some("tt2-postgres-1".into());
        info.port = Some(5432);
        record_recent(
            &store,
            "docker://app:p@ss@tt2-postgres-1",
            &Database::new_for_test(),
            Some(&info),
        );

        let recents = store.read(|config| config.get_recent_connections().clone());
        let recent = recents
            .iter()
            .find(|r| r.connection_url.starts_with("docker://"))
            .expect("a docker recent");
        assert_eq!(
            recent.connection_url,
            "docker://app@tt2-postgres-1/analytics?sslmode=require"
        );
        assert!(!recent.connection_url.contains("p@ss"));
        assert_eq!(
            recent.display_name,
            "app@tt2-postgres-1/analytics (Docker: tt2-postgres-1)"
        );
        // ...and it reconstructs faithfully
        assert_eq!(
            recent.reconstruct_connection_url().unwrap(),
            recent.connection_url
        );
    }

    #[test]
    fn docker_sessions_reconstruct_with_their_user_and_database() {
        let session = dbcrust::config::SavedSession {
            host: "DOCKER:tt2-postgres-1".into(),
            port: 0,
            user: "app".into(),
            dbname: "analytics".into(),
            ssh_tunnel: None,
            database_type: DatabaseType::PostgreSQL,
            file_path: None,
            options: std::collections::HashMap::from([
                ("sslmode".to_string(), "require".to_string()),
                ("password_command".to_string(), "pass show db".to_string()),
            ]),
        };
        // What `resolve_url` opens for this session: user, database and
        // options survive, the password command does not leak
        assert_eq!(
            session.reconstruct_connection_url().unwrap(),
            "docker://app@tt2-postgres-1/analytics?sslmode=require"
        );
    }

    #[test]
    fn a_test_connection_never_writes_the_vault_credential_cache() {
        let mut config = Config::default();
        config.vault_credential_cache_enabled = true;
        assert!(vault_config_for(ConnectMode::Open, config.clone()).vault_credential_cache_enabled);
        assert!(!vault_config_for(ConnectMode::Test, config).vault_credential_cache_enabled);
    }

    #[tokio::test]
    async fn read_only_constructions_run_alone_and_waiting_never_blocks() {
        // A private gate: the real one is shared with every other test.
        let gate: &'static ConstructionGate = Box::leak(Box::default());

        // Ordinary constructions share it
        let a = gate.acquire(false).await;
        let b = gate.acquire(false).await;
        // ...a read-only one has to wait for them, without blocking the
        // runtime (this test would hang if waiting blocked its only thread)
        let waited = tokio::time::timeout(Duration::from_millis(100), gate.acquire(true)).await;
        assert!(waited.is_err(), "exclusive must wait for shared holders");
        drop((a, b));

        // Once alone it excludes everyone, whoever arrives
        let exclusive = gate.acquire(true).await;
        for exclusive_waiter in [false, true] {
            let waited =
                tokio::time::timeout(Duration::from_millis(100), gate.acquire(exclusive_waiter))
                    .await;
            assert!(waited.is_err(), "must still be waiting");
        }
        // Giving up (dropping the waiters above) left nothing behind
        drop(exclusive);
        tokio::time::timeout(Duration::from_secs(5), gate.acquire(false))
            .await
            .expect("free again");
    }

    #[test]
    fn a_no_route_failure_keeps_its_own_text_and_typed_details() {
        use dbcrust::docker::{DockerError, RouteAttempt, RouteKind, Unreachable};
        let error: Box<dyn std::error::Error> = DockerError::Unreachable(Unreachable {
            container: "db".into(),
            port: 5432,
            published: None,
            attempts: vec![RouteAttempt {
                route: RouteKind::ExecTunnel,
                target: "through Docker (docker exec)".into(),
                reason: "this image has no supported forwarding helper (needs socat, perl or nc)"
                    .into(),
            }],
        })
        .into();
        let failure = connection_error(error, "Connection failed: ");
        let GuiError::Connection(message) = &failure.error else {
            panic!("expected a connection error");
        };
        assert!(message.starts_with("Couldn't reach 'db'."), "{message}");
        assert!(!message.contains("Connection failed"), "{message}");
        assert_eq!(failure.docker.unwrap().port, 5432);

        // Anything else is prefixed and has no Docker details.
        let other = connection_error("boom".into(), "Connection failed: ");
        assert_eq!(
            other.error,
            GuiError::Connection("Connection failed: boom".into())
        );
        assert!(other.docker.is_none());
    }

    #[test]
    fn docker_phases_map_onto_connect_phases() {
        for (docker, connect, label) in [
            (
                DockerPhase::ResolvingDocker,
                ConnectPhase::ResolvingDocker,
                "Resolving Docker",
            ),
            (
                DockerPhase::CheckingDirectAccess,
                ConnectPhase::CheckingDirectAccess,
                "Checking direct access",
            ),
            (
                DockerPhase::OpeningForwarding,
                ConnectPhase::OpeningForwarding,
                "Opening forwarding",
            ),
            (
                DockerPhase::Authenticating,
                ConnectPhase::Authenticating,
                "Authenticating",
            ),
        ] {
            assert_eq!(ConnectPhase::from(docker), connect);
            assert_eq!(connect.label(), label);
        }
    }
}
