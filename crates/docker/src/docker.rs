//! Docker database containers: discovery, and connecting to them from this
//! machine however the container is (not) published.
//!
//! * Endpoint: [`DockerEndpoint`] picks the daemon like the `docker` CLI.
//! * Reachability: [`DockerClient::connect_via_routes`] tries published port →
//!   OrbStack domains → container address with short concurrent probes, then
//!   a `docker exec` relay ([`crate::docker_tunnel`]), and only reports
//!   success once the caller's database handshake succeeded on that route.
//!   When nothing works it fails within seconds with [`DockerError::Unreachable`].

use crate::docker_route::{
    FORWARDING_RESERVE, HANDSHAKE_BUDGET, ProbeConfig, ReachCandidate, ReachContext, SETUP_BUDGET,
    is_transport_failure, probe_candidates, published_endpoints, reach_candidates, tcp_connect,
};
use crate::docker_tunnel::{DockerTunnel, RelayTarget};
use bollard::Docker;
use bollard::models::{ContainerInspectResponse, ContainerSummary};
use bollard::query_parameters::{InspectContainerOptions, ListContainersOptions};
use dbcrust_core::database::{DatabaseType, DatabaseTypeExt};
use std::collections::HashMap;
use std::error::Error as StdError;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::debug;

// `DockerTarget` is pure string parsing (no bollard) and lives in
// `dbcrust_core::docker_target`; re-exported so `crate::docker::DockerTarget` and
// `dbcrust::docker::{DockerTarget, PINNED_DAEMON_OPTION}` keep resolving.
pub use crate::docker_endpoint::{
    DockerEndpoint, EndpointInputs, EndpointSource, is_orbstack_daemon,
};
pub use crate::docker_route::{
    DockerPhase, DockerRoute, PROBE_ATTEMPT_TIMEOUT, PROBE_BUDGET, PROBE_HEAD_START,
    PROBE_MAX_CONCURRENT, RouteAttempt, RouteKind, Unreachable,
};
pub use dbcrust_core::docker_target::{DockerTarget, PINNED_DAEMON_OPTION};

/// Time limits of a container connection. The defaults are the product
/// behaviour; tests shrink them.
#[derive(Debug, Clone, Copy)]
pub struct RouteBudgets {
    /// The whole setup, starting before the container is inspected.
    pub setup: Duration,
    /// One database handshake on one route.
    pub handshake: Duration,
    /// Kept back for forwarding while direct routes are tried.
    pub forwarding_reserve: Duration,
    /// Below this a direct route isn't worth starting a handshake on.
    pub min_handshake: Duration,
}

impl Default for RouteBudgets {
    fn default() -> Self {
        Self {
            setup: SETUP_BUDGET,
            handshake: HANDSHAKE_BUDGET,
            forwarding_reserve: FORWARDING_RESERVE,
            min_handshake: Duration::from_millis(500),
        }
    }
}

/// How long a single Docker API call may take before the daemon is declared
/// unresponsive (bollard's own default is 120 s).
const DOCKER_API_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Error, Debug)]
pub enum DockerError {
    #[error("Docker connection failed: {0}")]
    ConnectionError(#[from] bollard::errors::Error),
    #[error("Container '{0}' not found")]
    ContainerNotFound(String),
    #[error("No database containers found")]
    NoDatabaseContainers,
    #[error("Container '{0}' is not running")]
    ContainerNotRunning(String),
    #[error("No exposed ports found for container '{0}'")]
    NoExposedPorts(String),
    #[error("Database type detection failed for container '{0}'")]
    DatabaseTypeDetectionFailed(String),
    #[error("Missing required environment variable: {0}")]
    MissingEnvironmentVariable(String),
    /// The selected Docker context / `DOCKER_HOST` can't be used.
    #[error("{0}")]
    Endpoint(String),
    /// The daemon can't be reached at all: not running, socket missing,
    /// permission denied.
    #[error("Docker isn't reachable at {endpoint}: {reason}")]
    DaemonUnavailable { endpoint: String, reason: String },
    /// The daemon accepted the connection but did not answer in time.
    #[error("Docker timed out: no answer within {seconds} s ({action})")]
    DaemonTimeout { action: &'static str, seconds: u64 },
    /// Nothing answered: the port is not published, the container's network
    /// address is not routable from this machine and `docker exec`
    /// forwarding is not available either. Lists what was tried and how to
    /// fix it.
    #[error("{0}")]
    Unreachable(Unreachable),
    /// The whole setup (inspect, probes, forwarding, handshake) overran
    /// [`SETUP_BUDGET`].
    #[error(
        "Connecting to '{container}' took longer than {seconds} s (last step: {})",
        .phase.label().to_lowercase()
    )]
    SetupTimeout {
        container: String,
        seconds: u64,
        phase: DockerPhase,
    },
}

impl From<DockerError> for dbcrust_core::database::DatabaseError {
    fn from(error: DockerError) -> Self {
        dbcrust_core::database::DatabaseError::Docker(Box::new(error))
    }
}

/// One host binding of a container port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedBinding {
    /// Address the daemon listens on (`0.0.0.0`, `::`, `127.0.0.1`, ...);
    /// empty when the daemon doesn't say.
    pub host_ip: String,
    pub host_port: u16,
}

#[derive(Debug, Clone)]
pub struct DockerContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub database_type: Option<DatabaseType>,
    /// The preferred published host port of the database port.
    pub host_port: Option<u16>,
    /// Address `host_port` is bound to (`0.0.0.0`, `127.0.0.1`...).
    pub host_ip: Option<String>,
    /// Every published binding of the database port (IPv4 and IPv6).
    pub bindings: Vec<PublishedBinding>,
    pub container_port: Option<u16>,
    pub actual_database_port: u16,
    /// The container's IPv4 address on its first (by name) network with one.
    pub ip_address: Option<String>,
    /// Its global IPv6 address, when the network has one.
    pub ipv6_address: Option<String>,
    pub environment: HashMap<String, String>,
    pub labels: HashMap<String, String>,
}

#[derive(Clone)]
pub struct DockerDatabaseConnection {
    pub container_name: String,
    pub database_type: DatabaseType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database_name: Option<String>,
}

impl std::fmt::Debug for DockerDatabaseConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerDatabaseConnection")
            .field("container_name", &self.container_name)
            .field("database_type", &self.database_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field(
                "password",
                &dbcrust_core::password_sanitizer::mask_secret(&self.password),
            )
            .field("database_name", &self.database_name)
            .finish()
    }
}

/// A successful [`DockerClient::connect_via_routes`].
#[derive(Debug)]
pub struct DockerConnected<T> {
    /// What the caller's `build` produced (the `Database`).
    pub value: T,
    /// The address that was dialled (the local relay endpoint when forwarded).
    pub connection: DockerDatabaseConnection,
    pub route: DockerRoute,
    /// Keeps the relay alive: store it next to `value` for as long as the
    /// connection lives.
    pub tunnel: Option<Arc<DockerTunnel>>,
    /// The container's name and the database port inside it: the stable
    /// address of the connection, whatever route carried it.
    pub container_name: String,
    pub database_port: u16,
}

/// A transport-level bollard error as "the daemon is not reachable"; HTTP
/// answers from the daemon (404...) stay [`DockerError::ConnectionError`].
fn daemon_error(endpoint: &DockerEndpoint, error: bollard::errors::Error) -> DockerError {
    use bollard::errors::Error;
    if matches!(
        error,
        Error::DockerResponseServerError { .. } | Error::JsonDataError { .. }
    ) {
        return DockerError::ConnectionError(error);
    }
    let mut reason = match &error {
        Error::SocketNotFoundError(path) => {
            format!("the socket {path} doesn't exist (is Docker running?)")
        }
        other => other.to_string(),
    };
    if reason.to_ascii_lowercase().contains("permission denied") {
        reason.push_str(" (is your user allowed to use the Docker socket?)");
    }
    DockerError::DaemonUnavailable {
        endpoint: endpoint.describe(),
        reason,
    }
}

/// Reports phases to the caller and remembers the current one for timeouts.
struct Phases<P> {
    progress: P,
    current: Arc<Mutex<DockerPhase>>,
}

impl<P: FnMut(DockerPhase)> Phases<P> {
    fn enter(&mut self, phase: DockerPhase) {
        if let Ok(mut current) = self.current.lock() {
            *current = phase;
        }
        (self.progress)(phase);
    }
}

pub struct DockerClient {
    docker: Docker,
    endpoint: DockerEndpoint,
    orbstack: tokio::sync::OnceCell<bool>,
    budgets: RouteBudgets,
}

impl DockerClient {
    /// Create a Docker client for the endpoint the `docker` CLI would use:
    /// `DOCKER_CONTEXT`, `DOCKER_HOST`, the current context, then the
    /// platform default. An endpoint DBCrust can't speak is an error.
    pub fn new() -> Result<Self, DockerError> {
        Self::with_endpoint(DockerEndpoint::from_environment()?)
    }

    /// Create a Docker client for an explicit endpoint.
    pub fn with_endpoint(endpoint: DockerEndpoint) -> Result<Self, DockerError> {
        let docker = match &endpoint.host {
            Some(host) => Docker::connect_with_host(host),
            None => Docker::connect_with_defaults(),
        }
        .map_err(|e| daemon_error(&endpoint, e))?;
        debug!("[docker] endpoint: {}", endpoint.describe());
        Ok(DockerClient {
            docker,
            endpoint,
            orbstack: tokio::sync::OnceCell::new(),
            budgets: RouteBudgets::default(),
        })
    }

    /// Create a Docker client for a socket path (or `unix://`/`tcp://` URL).
    pub fn with_socket(socket_path: &str) -> Result<Self, DockerError> {
        Self::with_endpoint(DockerEndpoint::explicit(socket_path)?)
    }

    /// Replace the time limits (for tests).
    pub fn with_budgets(mut self, budgets: RouteBudgets) -> Self {
        self.budgets = budgets;
        self
    }

    /// The daemon endpoint in use.
    pub fn endpoint(&self) -> &DockerEndpoint {
        &self.endpoint
    }

    /// The underlying bollard client (cheap to clone).
    pub fn bollard(&self) -> &Docker {
        &self.docker
    }

    /// Run a Docker API call under [`DOCKER_API_TIMEOUT`].
    async fn api<T>(
        &self,
        action: &'static str,
        call: impl Future<Output = Result<T, bollard::errors::Error>>,
    ) -> Result<T, DockerError> {
        match tokio::time::timeout(DOCKER_API_TIMEOUT, call).await {
            Ok(result) => result.map_err(|e| daemon_error(&self.endpoint, e)),
            Err(_) => Err(DockerError::DaemonTimeout {
                action,
                seconds: DOCKER_API_TIMEOUT.as_secs(),
            }),
        }
    }

    /// Whether the daemon is OrbStack: its socket lives under `~/.orbstack/`,
    /// or `docker info` says so. Never inferred from the host OS.
    pub async fn is_orbstack(&self) -> bool {
        *self
            .orbstack
            .get_or_init(|| async {
                if self.endpoint.mentions_orbstack() {
                    return true;
                }
                match self.api("reading daemon info", self.docker.info()).await {
                    Ok(info) => {
                        is_orbstack_daemon(info.operating_system.as_deref(), info.name.as_deref())
                    }
                    Err(_) => false,
                }
            })
            .await
    }

    async fn reach_context(&self) -> ReachContext {
        let local = self.endpoint.is_local();
        let orbstack = local && self.is_orbstack().await;
        ReachContext {
            orbstack,
            // macOS and Windows daemons run in a VM whose container network
            // is not routed to the host; only OrbStack bridges it.
            container_ip_routable: local && (orbstack || cfg!(target_os = "linux")),
            publish_host: self.endpoint.remote_host(),
        }
    }

    /// List all running containers
    pub async fn list_containers(&self) -> Result<Vec<ContainerSummary>, DockerError> {
        let mut filters = HashMap::new();
        filters.insert("status".to_string(), vec!["running".to_string()]);

        let options = ListContainersOptions {
            all: false,
            filters: Some(filters),
            ..Default::default()
        };

        self.api(
            "listing containers",
            self.docker.list_containers(Some(options)),
        )
        .await
    }

    /// List all database containers (running or not)
    pub async fn list_database_containers(&self) -> Result<Vec<DockerContainerInfo>, DockerError> {
        let options = ListContainersOptions {
            all: true,
            ..Default::default()
        };

        let containers = self
            .api(
                "listing containers",
                self.docker.list_containers(Some(options)),
            )
            .await?;
        let mut database_containers = Vec::new();

        for container in containers {
            if let Some(image) = &container.image {
                let database_type = Self::detect_database_type_from_image(image);
                if let Some(db_type) = database_type {
                    // Port information from the container summary (the
                    // default port of the engine; inspect refines it)
                    let bindings = Self::summary_bindings(&container, &db_type);
                    let (host_port, host_ip) = preferred_binding(&bindings);

                    let container_info = DockerContainerInfo {
                        id: container.id.unwrap_or_default(),
                        name: container
                            .names
                            .unwrap_or_default()
                            .first()
                            .map(|n| n.trim_start_matches('/').to_string())
                            .unwrap_or_default(),
                        image: image.to_string(),
                        status: container.status.unwrap_or_default(),
                        database_type: Some(db_type.clone()),
                        host_port,
                        host_ip,
                        bindings,
                        container_port: Some(Self::get_default_port(&db_type)),
                        actual_database_port: Self::get_default_port(&db_type), // Will be detected in inspect_container
                        ip_address: None, // Will be populated by inspect_container if needed
                        ipv6_address: None,
                        environment: HashMap::new(), // Will be populated by inspect_container if needed
                        labels: HashMap::new(), // Will be populated by inspect_container if needed
                    };
                    database_containers.push(container_info);
                }
            }
        }

        if database_containers.is_empty() {
            return Err(DockerError::NoDatabaseContainers);
        }

        // Sort containers: running containers first, then others
        database_containers.sort_by(|a, b| {
            let a_running = a.status.contains("running") || a.status.contains("Up");
            let b_running = b.status.contains("running") || b.status.contains("Up");

            match (a_running, b_running) {
                (true, false) => std::cmp::Ordering::Less, // a is running, b is not - a comes first
                (false, true) => std::cmp::Ordering::Greater, // b is running, a is not - b comes first
                _ => a.name.cmp(&b.name), // Both same status - sort by name alphabetically
            }
        });

        Ok(database_containers)
    }

    /// Get detailed information about a specific container
    pub async fn inspect_container(
        &self,
        container_id: &str,
    ) -> Result<DockerContainerInfo, DockerError> {
        let options = InspectContainerOptions { size: false };
        let container = self
            .api(
                "inspecting the container",
                self.docker.inspect_container(container_id, Some(options)),
            )
            .await
            .map_err(|e| match e {
                DockerError::ConnectionError(
                    bollard::errors::Error::DockerResponseServerError {
                        status_code: 404, ..
                    },
                ) => DockerError::ContainerNotFound(container_id.to_string()),
                other => other,
            })?;

        let config = container
            .config
            .as_ref()
            .ok_or_else(|| DockerError::ContainerNotFound(container_id.to_string()))?;
        let state = container
            .state
            .as_ref()
            .ok_or_else(|| DockerError::ContainerNotFound(container_id.to_string()))?;

        // Check if container is running
        if !state.running.unwrap_or(false) {
            return Err(DockerError::ContainerNotRunning(container_id.to_string()));
        }

        let image = config.image.as_ref().unwrap_or(&String::new()).clone();
        let database_type = Self::detect_database_type_from_image(&image)
            .ok_or_else(|| DockerError::DatabaseTypeDetectionFailed(container_id.to_string()))?;

        // Extract environment variables
        let mut environment = HashMap::new();
        if let Some(env_vars) = &config.env {
            for env_var in env_vars {
                if let Some((key, value)) = env_var.split_once('=') {
                    environment.insert(key.to_string(), value.to_string());
                }
            }
        }

        // Extract labels
        let mut labels = HashMap::new();
        if let Some(container_labels) = &config.labels {
            for (key, value) in container_labels {
                labels.insert(key.clone(), value.clone());
            }
        }

        // The port the database listens on (never "the first exposed port")
        let actual_database_port = Self::database_port(&database_type, &environment);

        // Every host binding of that port
        let bindings = Self::inspect_bindings(&container, actual_database_port);
        let (host_port, host_ip) = preferred_binding(&bindings);

        // Extract IP addresses from network settings
        let (ip_address, ipv6_address) = Self::extract_ip_addresses(&container);

        Ok(DockerContainerInfo {
            id: container.id.unwrap_or_default(),
            name: container
                .name
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string(),
            image,
            status: state
                .status
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            database_type: Some(database_type),
            host_port,
            host_ip,
            bindings,
            container_port: Some(actual_database_port),
            actual_database_port,
            ip_address,
            ipv6_address,
            environment,
            labels,
        })
    }

    /// The container's IPv4 and global IPv6 addresses. Networks are visited
    /// by name so the choice is the same on every call.
    fn extract_ip_addresses(
        container: &ContainerInspectResponse,
    ) -> (Option<String>, Option<String>) {
        let Some(networks) = container
            .network_settings
            .as_ref()
            .and_then(|settings| settings.networks.as_ref())
        else {
            return (None, None);
        };
        let mut names: Vec<_> = networks.keys().collect();
        names.sort();
        let first = |pick: fn(&bollard::models::EndpointSettings) -> Option<&String>| {
            names
                .iter()
                .filter_map(|name| pick(&networks[*name]))
                .find(|address| !address.is_empty())
                .cloned()
        };
        (
            first(|network| network.ip_address.as_ref()),
            first(|network| network.global_ipv6_address.as_ref()),
        )
    }

    /// Published bindings of the engine's default port in a container summary.
    fn summary_bindings(
        container: &ContainerSummary,
        database_type: &DatabaseType,
    ) -> Vec<PublishedBinding> {
        let default_port = Self::get_default_port(database_type);
        let mut bindings: Vec<PublishedBinding> = Vec::new();
        for port in container.ports.iter().flatten() {
            if port.private_port != default_port {
                continue;
            }
            // UDP publications of the same number are not the database
            if port
                .typ
                .as_ref()
                .is_some_and(|typ| typ.to_string().eq_ignore_ascii_case("udp"))
            {
                continue;
            }
            if let Some(host_port) = port.public_port {
                let binding = PublishedBinding {
                    host_ip: port.ip.clone().unwrap_or_default(),
                    host_port,
                };
                if !bindings.contains(&binding) {
                    bindings.push(binding);
                }
            }
        }
        bindings
    }

    /// Published bindings of `port/tcp` in an inspected container.
    fn inspect_bindings(container: &ContainerInspectResponse, port: u16) -> Vec<PublishedBinding> {
        let mut bindings: Vec<PublishedBinding> = Vec::new();
        let published = container
            .network_settings
            .as_ref()
            .and_then(|settings| settings.ports.as_ref())
            .and_then(|ports| ports.get(&format!("{port}/tcp")))
            .and_then(|bindings| bindings.as_ref());
        for binding in published.into_iter().flatten() {
            let Some(host_port) = binding
                .host_port
                .as_deref()
                .and_then(|p| p.parse::<u16>().ok())
            else {
                continue;
            };
            let binding = PublishedBinding {
                host_ip: binding.host_ip.clone().unwrap_or_default(),
                host_port,
            };
            if !bindings.contains(&binding) {
                bindings.push(binding);
            }
        }
        bindings
    }

    /// The port the database listens on inside the container: the engine's
    /// own environment override if the image documents one, else its default
    /// port. `EXPOSE`d or published ports prove nothing about what listens
    /// where (a postgres container may also publish an admin UI).
    pub fn database_port(
        database_type: &DatabaseType,
        environment: &HashMap<String, String>,
    ) -> u16 {
        let override_var = match database_type {
            DatabaseType::PostgreSQL => Some("PGPORT"),
            DatabaseType::MySQL => Some("MYSQL_TCP_PORT"),
            _ => None,
        };
        override_var
            .and_then(|var| environment.get(var))
            .and_then(|value| value.trim().parse::<u16>().ok())
            .filter(|port| *port != 0)
            .unwrap_or_else(|| Self::get_default_port(database_type))
    }

    /// Detect database type from Docker image name
    fn detect_database_type_from_image(image: &str) -> Option<DatabaseType> {
        let image_lower = image.to_lowercase();

        if image_lower.contains("postgres")
            || image_lower.contains("pgvector")
            || (image_lower.contains("pg")
                && (image_lower.contains(":") || image_lower.contains("/")))
        {
            Some(DatabaseType::PostgreSQL)
        } else if image_lower.contains("mysql") || image_lower.contains("mariadb") {
            Some(DatabaseType::MySQL)
        } else if image_lower.contains("sqlite") {
            Some(DatabaseType::SQLite)
        } else if image_lower.contains("clickhouse")
            || image_lower.contains("clickhouse-server")
            || image_lower.contains("yandex/clickhouse")
            || image_lower.contains("clickhouse/clickhouse")
        {
            Some(DatabaseType::ClickHouse)
        } else if image_lower.contains("mongo")
            || image_lower.contains("mongodb")
            || image_lower.contains("mongo:")
        {
            Some(DatabaseType::MongoDB)
        } else if image_lower.contains("elasticsearch")
            || image_lower.contains("elastic/elasticsearch")
            || image_lower.contains("docker.elastic.co/elasticsearch")
            || (image_lower.contains("elastic")
                && !image_lower.contains("kibana")  // Exclude Kibana containers
                && (image_lower.contains(":") || image_lower.contains("/")))
        {
            Some(DatabaseType::Elasticsearch)
        } else {
            None
        }
    }

    /// Get default port for database type
    fn get_default_port(database_type: &DatabaseType) -> u16 {
        database_type.default_port().unwrap_or(0)
    }

    /// Pick the database credentials and name out of the container's environment.
    fn database_credentials(
        container_info: &DockerContainerInfo,
    ) -> Result<(DatabaseType, Option<String>, Option<String>, Option<String>), DockerError> {
        let database_type = container_info
            .database_type
            .clone()
            .ok_or_else(|| DockerError::DatabaseTypeDetectionFailed(container_info.name.clone()))?;

        if database_type.is_file_based() {
            // SQLite doesn't use network connections
            return Ok((database_type, None, None, None));
        }

        // Extract username from environment variables
        let username = database_type
            .docker_username_env_vars()
            .iter()
            .find_map(|var| container_info.environment.get(*var))
            .cloned()
            .unwrap_or_else(|| database_type.default_username().to_string());

        // Extract password from environment variables
        let password = database_type
            .docker_password_env_vars()
            .iter()
            .find_map(|var| container_info.environment.get(*var))
            .cloned();

        // Extract database name from environment variables
        let database_name = database_type
            .docker_database_env_vars()
            .iter()
            .find_map(|var| container_info.environment.get(*var))
            .cloned()
            .unwrap_or_else(|| username.clone());

        // Special handling for ClickHouse with CLICKHOUSE_SKIP_USER_SETUP=1:
        // the default user is available without a password.
        let skips_user_setup = database_type == DatabaseType::ClickHouse
            && container_info
                .environment
                .get("CLICKHOUSE_SKIP_USER_SETUP")
                .is_some_and(|skip| skip == "1");
        let password = if skips_user_setup {
            tracing::debug!(
                "[DockerClient::build_connection_info] ClickHouse SKIP_USER_SETUP=1 detected, using default user without password"
            );
            None
        } else {
            password
        };

        Ok((database_type, Some(username), password, Some(database_name)))
    }

    fn connection_to(
        container_info: &DockerContainerInfo,
        host: &str,
        port: u16,
    ) -> Result<DockerDatabaseConnection, DockerError> {
        let (database_type, username, password, database_name) =
            Self::database_credentials(container_info)?;
        Ok(DockerDatabaseConnection {
            container_name: container_info.name.clone(),
            database_type,
            host: host.to_string(),
            port,
            username,
            password,
            database_name,
        })
    }

    /// Inspect `container` and connect to its database by the first route
    /// that works. The whole thing, inspection included, runs under one
    /// [`SETUP_BUDGET`] deadline. See [`DockerClient::connect_via_routes`].
    pub async fn connect_container<T, P, B, Fut>(
        &self,
        container: &str,
        progress: P,
        build: B,
    ) -> Result<DockerConnected<T>, Box<dyn StdError>>
    where
        P: FnMut(DockerPhase),
        B: FnMut(DockerDatabaseConnection) -> Fut,
        Fut: Future<Output = Result<T, Box<dyn StdError>>>,
    {
        self.within_budget(container, progress, |deadline, mut phases| async move {
            phases.enter(DockerPhase::ResolvingDocker);
            let info = self.inspect_container(container).await?;
            self.try_routes(&info, deadline, phases, build).await
        })
        .await
    }

    /// Connect to the container's database by the first route that works,
    /// bounded by [`SETUP_BUDGET`].
    ///
    /// `build` turns a resolved address into a connected value (the caller's
    /// `Database`, including authentication/TLS setup). Routes are tried in
    /// order: published binding, OrbStack domains, container address (probed
    /// concurrently with short TCP connects), then a `docker exec` relay.
    /// Each handshake gets at most [`HANDSHAKE_BUDGET`] and direct routes
    /// leave [`FORWARDING_RESERVE`] for forwarding, so an endpoint that
    /// accepts TCP but never speaks the protocol cannot eat the whole budget.
    /// A route whose handshake fails at the transport level (or times out)
    /// falls through to the next; an authentication/TLS/unknown-database
    /// failure stops immediately so credentials are never replayed
    /// elsewhere. `progress` is told each [`DockerPhase`] as it starts.
    ///
    /// Dropping the returned future cancels everything, including a relay
    /// that was just opened.
    pub async fn connect_via_routes<T, P, B, Fut>(
        &self,
        container_info: &DockerContainerInfo,
        progress: P,
        build: B,
    ) -> Result<DockerConnected<T>, Box<dyn StdError>>
    where
        P: FnMut(DockerPhase),
        B: FnMut(DockerDatabaseConnection) -> Fut,
        Fut: Future<Output = Result<T, Box<dyn StdError>>>,
    {
        self.within_budget(&container_info.name, progress, |deadline, phases| {
            self.try_routes(container_info, deadline, phases, build)
        })
        .await
    }

    /// Run `work` under the single setup deadline, which starts now. `work`
    /// receives the deadline and a phase reporter; if the deadline passes it
    /// is dropped and the error names the step it was in.
    async fn within_budget<T, P, W, Fut>(
        &self,
        container: &str,
        progress: P,
        work: W,
    ) -> Result<DockerConnected<T>, Box<dyn StdError>>
    where
        P: FnMut(DockerPhase),
        W: FnOnce(Instant, Phases<P>) -> Fut,
        Fut: Future<Output = Result<DockerConnected<T>, Box<dyn StdError>>>,
    {
        let deadline = Instant::now() + self.budgets.setup;
        let current = Arc::new(Mutex::new(DockerPhase::ResolvingDocker));
        let phases = Phases {
            progress,
            current: Arc::clone(&current),
        };
        match tokio::time::timeout(self.budgets.setup, work(deadline, phases)).await {
            Ok(result) => result,
            Err(_) => Err(DockerError::SetupTimeout {
                container: container.to_string(),
                seconds: self.budgets.setup.as_secs(),
                phase: current
                    .lock()
                    .map(|p| *p)
                    .unwrap_or(DockerPhase::CheckingDirectAccess),
            }
            .into()),
        }
    }

    async fn try_routes<T, P, B, Fut>(
        &self,
        info: &DockerContainerInfo,
        deadline: Instant,
        mut phases: Phases<P>,
        mut build: B,
    ) -> Result<DockerConnected<T>, Box<dyn StdError>>
    where
        P: FnMut(DockerPhase),
        B: FnMut(DockerDatabaseConnection) -> Fut,
        Fut: Future<Output = Result<T, Box<dyn StdError>>>,
    {
        // Fail early on a missing type/credentials before any network work.
        Self::database_credentials(info)?;
        let port = info.actual_database_port;
        if port == 0 {
            return Err(DockerError::NoExposedPorts(info.name.clone()).into());
        }
        let budgets = self.budgets;
        let time_left = || deadline.saturating_duration_since(Instant::now());

        phases.enter(DockerPhase::CheckingDirectAccess);
        let ctx = self.reach_context().await;
        let mut remaining: Vec<ReachCandidate> = reach_candidates(info, &ctx);
        debug!("[docker] {}: candidates {:?}", info.name, remaining);
        let mut attempts: Vec<RouteAttempt> = Vec::new();

        while !remaining.is_empty() {
            // Keep time for forwarding: direct routes may not spend it all.
            let handshake_budget = budgets
                .handshake
                .min(time_left().saturating_sub(budgets.forwarding_reserve));
            if handshake_budget < budgets.min_handshake {
                debug!("[docker] {}: out of time for more direct routes", info.name);
                break;
            }
            phases.enter(DockerPhase::CheckingDirectAccess);
            let report = probe_candidates(&remaining, &ProbeConfig::default(), tcp_connect).await;
            attempts.extend(report.failures());
            let Some(index) = report.chosen else { break };
            let candidate = remaining[index].clone();

            // A TCP answer is only reachability: complete the handshake,
            // within its own budget.
            phases.enter(DockerPhase::Authenticating);
            let connection = Self::connection_to(info, &candidate.host, candidate.port)?;
            let handshake = tokio::time::timeout(
                handshake_budget.min(time_left().saturating_sub(budgets.forwarding_reserve)),
                build(connection.clone()),
            )
            .await;
            match handshake {
                Ok(Ok(value)) => {
                    debug!(
                        "[docker] {}: connected via {}",
                        info.name,
                        candidate.address()
                    );
                    return Ok(DockerConnected {
                        value,
                        connection,
                        route: DockerRoute {
                            kind: candidate.route,
                            host: candidate.host.clone(),
                            port: candidate.port,
                            helper: None,
                            detail: format!("{} {}", candidate.route.label(), candidate.address()),
                        },
                        tunnel: None,
                        container_name: info.name.clone(),
                        database_port: port,
                    });
                }
                Ok(Err(e)) if is_transport_failure(&e.to_string()) => {
                    attempts.push(RouteAttempt {
                        route: candidate.route,
                        target: candidate.address(),
                        reason: format!("answered, but the database connection failed ({e})"),
                    });
                    remaining.drain(..=index);
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    attempts.push(RouteAttempt {
                        route: candidate.route,
                        target: candidate.address(),
                        reason: format!(
                            "accepted the connection, but the database handshake did not finish within {} s",
                            handshake_budget.as_secs().max(1)
                        ),
                    });
                    remaining.drain(..=index);
                }
            }
        }

        // No direct route: forward through the Docker API.
        phases.enter(DockerPhase::OpeningForwarding);
        let unreachable = |attempts: Vec<RouteAttempt>| {
            let published = published_endpoints(info, self.endpoint.remote_host().as_deref())
                .into_iter()
                .next()
                .map(|(host, port)| ReachCandidate::new(RouteKind::PublishedPort, host, port))
                .map(|c| c.address());
            Box::<dyn StdError>::from(DockerError::Unreachable(Unreachable {
                container: info.name.clone(),
                port,
                published,
                attempts,
            }))
        };
        let forwarding_failed = |attempts: &mut Vec<RouteAttempt>, reason: String| {
            attempts.push(RouteAttempt {
                route: RouteKind::ExecTunnel,
                target: "through Docker (docker exec)".to_string(),
                reason,
            });
        };
        let relay_target = match RelayTarget::for_container(
            info.ip_address.as_deref(),
            info.ipv6_address.as_deref(),
            port,
        ) {
            Ok(target) => target,
            Err(e) => {
                forwarding_failed(&mut attempts, e.to_string());
                return Err(unreachable(attempts));
            }
        };
        let exec_id = if info.id.is_empty() {
            &info.name
        } else {
            &info.id
        };
        let tunnel = match DockerTunnel::open(self.docker.clone(), exec_id, relay_target).await {
            Ok(tunnel) => Arc::new(tunnel),
            Err(e) => {
                forwarding_failed(&mut attempts, e.to_string());
                return Err(unreachable(attempts));
            }
        };

        phases.enter(DockerPhase::Authenticating);
        let connection = Self::connection_to(
            info,
            &tunnel.local_addr().ip().to_string(),
            tunnel.local_port(),
        )?;
        let handshake_budget = budgets.handshake.min(time_left());
        let handshake = tokio::time::timeout(handshake_budget, build(connection.clone())).await;
        let through_relay = |reason: String| {
            let hint = tunnel
                .last_error()
                .map(|line| format!(": {line}"))
                .unwrap_or_default();
            Box::<dyn StdError>::from(format!(
                "{reason}\nThe connection went through a `docker exec` relay to {}{hint}",
                relay_target.describe()
            ))
        };
        match handshake {
            Ok(Ok(value)) => Ok(DockerConnected {
                value,
                connection,
                route: DockerRoute {
                    kind: RouteKind::ExecTunnel,
                    host: tunnel.local_addr().ip().to_string(),
                    port: tunnel.local_port(),
                    helper: Some(tunnel.helper().name().to_string()),
                    detail: tunnel.describe(),
                },
                tunnel: Some(tunnel),
                container_name: info.name.clone(),
                database_port: port,
            }),
            Ok(Err(e)) if is_transport_failure(&e.to_string()) => Err(through_relay(e.to_string())),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(through_relay(format!(
                "the database handshake did not finish within {} s",
                handshake_budget.as_secs().max(1)
            ))),
        }
    }

    /// Parse Docker URL format: docker://user:password@container_name/database
    /// (a `?query` after the database is accepted and ignored here; see
    /// [`DockerTarget`] for the options).
    pub fn parse_docker_url(
        url: &str,
    ) -> Option<(Option<String>, Option<String>, String, Option<String>)> {
        let target = DockerTarget::parse(url)?;
        Some((
            target.user,
            target.password,
            target.container,
            target.database,
        ))
    }
}

/// The preferred published host port among bindings: IPv4 first.
fn preferred_binding(bindings: &[PublishedBinding]) -> (Option<u16>, Option<String>) {
    bindings
        .iter()
        .find(|b| !b.host_ip.contains(':'))
        .or_else(|| bindings.first())
        .map(|b| (Some(b.host_port), Some(b.host_ip.clone())))
        .unwrap_or((None, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn test_database_error_keeps_its_text_and_type() {
        let error = dbcrust_core::database::DatabaseError::from(DockerError::ContainerNotFound(
            "pg-1".to_string(),
        ));
        assert_eq!(
            error.to_string(),
            "Docker error: Container 'pg-1' not found"
        );
        let source = std::error::Error::source(&error).expect("boxed DockerError");
        assert!(source.downcast_ref::<DockerError>().is_some());
    }

    #[test]
    fn test_detect_database_type_from_image() {
        assert_eq!(
            DockerClient::detect_database_type_from_image("postgres:13"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("pgvector/pgvector:pg16"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("pg:14"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("mysql:8.0"),
            Some(DatabaseType::MySQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("mariadb:10.5"),
            Some(DatabaseType::MySQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("clickhouse:latest"),
            Some(DatabaseType::ClickHouse)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("clickhouse/clickhouse-server:23.8"),
            Some(DatabaseType::ClickHouse)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("yandex/clickhouse-server:latest"),
            Some(DatabaseType::ClickHouse)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("mongo:7.0"),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image(
                "mongodb/mongodb-community-server:latest"
            ),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("bitnami/mongodb:latest"),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("elasticsearch:8.15.0"),
            Some(DatabaseType::Elasticsearch)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("elastic/elasticsearch:8.15.0"),
            Some(DatabaseType::Elasticsearch)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image(
                "docker.elastic.co/elasticsearch/elasticsearch:8.15.0"
            ),
            Some(DatabaseType::Elasticsearch)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("bitnami/elasticsearch:latest"),
            Some(DatabaseType::Elasticsearch)
        );
        // Test Kibana containers are NOT detected as Elasticsearch
        assert_eq!(
            DockerClient::detect_database_type_from_image("elastic/kibana:8.15.0"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("docker.elastic.co/kibana/kibana:8.15.0"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("kibana:8.15.0"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("bitnami/kibana:latest"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("nginx:latest"),
            None
        );
    }

    #[test]
    fn test_parse_docker_url() {
        // Test empty URL
        assert_eq!(
            DockerClient::parse_docker_url("docker://"),
            Some((None, None, String::new(), None))
        );

        // Test container name only
        assert_eq!(
            DockerClient::parse_docker_url("docker://postgres-db"),
            Some((None, None, "postgres-db".to_string(), None))
        );

        // Test container name with database
        assert_eq!(
            DockerClient::parse_docker_url("docker://postgres-db/mydb"),
            Some((
                None,
                None,
                "postgres-db".to_string(),
                Some("mydb".to_string())
            ))
        );

        // Test with username
        assert_eq!(
            DockerClient::parse_docker_url("docker://user@postgres-db"),
            Some((
                Some("user".to_string()),
                None,
                "postgres-db".to_string(),
                None
            ))
        );

        // Test with username and password
        assert_eq!(
            DockerClient::parse_docker_url("docker://user:pass@postgres-db"),
            Some((
                Some("user".to_string()),
                Some("pass".to_string()),
                "postgres-db".to_string(),
                None
            ))
        );

        // Test full URL
        assert_eq!(
            DockerClient::parse_docker_url("docker://user:pass@postgres-db/mydb"),
            Some((
                Some("user".to_string()),
                Some("pass".to_string()),
                "postgres-db".to_string(),
                Some("mydb".to_string())
            ))
        );

        // Test invalid URL
        assert_eq!(
            DockerClient::parse_docker_url("postgres://user:pass@host/db"),
            None
        );
    }

    #[test]
    fn test_get_default_port() {
        assert_eq!(
            DockerClient::get_default_port(&DatabaseType::PostgreSQL),
            5432
        );
        assert_eq!(DockerClient::get_default_port(&DatabaseType::MySQL), 3306);
        assert_eq!(DockerClient::get_default_port(&DatabaseType::SQLite), 0);
    }

    // -- container info -------------------------------------------------------

    fn container(name: &str) -> DockerContainerInfo {
        DockerContainerInfo {
            id: format!("id-{name}"),
            name: name.to_string(),
            image: "postgres:17".to_string(),
            status: "running".to_string(),
            database_type: Some(DatabaseType::PostgreSQL),
            host_port: None,
            host_ip: None,
            bindings: Vec::new(),
            container_port: Some(5432),
            actual_database_port: 5432,
            ip_address: None,
            ipv6_address: None,
            environment: HashMap::new(),
            labels: HashMap::new(),
        }
    }

    fn with_bindings(mut info: DockerContainerInfo, ports: &[(&str, u16)]) -> DockerContainerInfo {
        info.bindings = ports
            .iter()
            .map(|(ip, port)| PublishedBinding {
                host_ip: ip.to_string(),
                host_port: *port,
            })
            .collect();
        (info.host_port, info.host_ip) = preferred_binding(&info.bindings);
        info
    }

    #[rstest]
    #[case(DatabaseType::PostgreSQL, &[], 5432)]
    #[case(DatabaseType::PostgreSQL, &[("PGPORT", "5433")], 5433)]
    #[case(DatabaseType::PostgreSQL, &[("PGPORT", "not-a-port")], 5432)]
    #[case(DatabaseType::PostgreSQL, &[("PGPORT", "0")], 5432)]
    #[case(DatabaseType::MySQL, &[("MYSQL_TCP_PORT", "3307")], 3307)]
    // PGPORT means nothing to MySQL
    #[case(DatabaseType::MySQL, &[("PGPORT", "5433")], 3306)]
    fn database_port_comes_from_the_engine_not_from_exposed_ports(
        #[case] db_type: DatabaseType,
        #[case] env: &[(&str, &str)],
        #[case] expected: u16,
    ) {
        let env = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(DockerClient::database_port(&db_type, &env), expected);
    }

    #[test]
    fn inspect_bindings_keep_every_host_binding_of_the_database_port() {
        use bollard::models::{NetworkSettings, PortBinding};
        let binding = |ip: &str, port: &str| PortBinding {
            host_ip: Some(ip.into()),
            host_port: Some(port.into()),
        };
        let mut ports = HashMap::new();
        ports.insert(
            "5432/tcp".to_string(),
            Some(vec![
                binding("0.0.0.0", "32768"),
                binding("::", "32768"),
                binding("0.0.0.0", "32768"),
            ]),
        );
        // An unrelated published service must not matter.
        ports.insert(
            "8080/tcp".to_string(),
            Some(vec![binding("0.0.0.0", "8080")]),
        );
        ports.insert("5432/udp".to_string(), Some(vec![binding("0.0.0.0", "9")]));
        let inspect = ContainerInspectResponse {
            network_settings: Some(NetworkSettings {
                ports: Some(ports),
                ..Default::default()
            }),
            ..Default::default()
        };
        let bindings = DockerClient::inspect_bindings(&inspect, 5432);
        assert_eq!(
            bindings,
            vec![
                PublishedBinding {
                    host_ip: "0.0.0.0".into(),
                    host_port: 32768
                },
                PublishedBinding {
                    host_ip: "::".into(),
                    host_port: 32768
                },
            ]
        );
        assert_eq!(
            preferred_binding(&bindings),
            (Some(32768), Some("0.0.0.0".into()))
        );
        // Nothing published (null binding list) → no bindings.
        let mut unpublished = HashMap::new();
        unpublished.insert("5432/tcp".to_string(), None);
        let inspect = ContainerInspectResponse {
            network_settings: Some(NetworkSettings {
                ports: Some(unpublished),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(DockerClient::inspect_bindings(&inspect, 5432).is_empty());
        assert_eq!(preferred_binding(&[]), (None, None));
    }

    #[test]
    fn credentials_come_from_the_container_environment() {
        let mut info = container("db");
        info.environment
            .insert("POSTGRES_USER".into(), "app".into());
        info.environment
            .insert("POSTGRES_PASSWORD".into(), "secret".into());
        let (db_type, user, password, name) = DockerClient::database_credentials(&info).unwrap();
        assert_eq!(db_type, DatabaseType::PostgreSQL);
        assert_eq!(user.as_deref(), Some("app"));
        assert_eq!(password.as_deref(), Some("secret"));
        // POSTGRES_DB defaults to the user name.
        assert_eq!(name.as_deref(), Some("app"));
    }

    #[test]
    fn clickhouse_without_user_setup_uses_the_default_user_without_password() {
        let mut info = container("ch");
        info.database_type = Some(DatabaseType::ClickHouse);
        info.environment
            .insert("CLICKHOUSE_PASSWORD".into(), "secret".into());
        info.environment
            .insert("CLICKHOUSE_SKIP_USER_SETUP".into(), "1".into());
        let (_, _, password, _) = DockerClient::database_credentials(&info).unwrap();
        assert_eq!(password, None);
    }

    // -- connecting through routes -----------------------------------------------

    /// A client whose daemon is a closed local port: every API call fails
    /// fast, which is what the route logic must cope with.
    fn offline_client() -> DockerClient {
        DockerClient::with_endpoint(DockerEndpoint::explicit("tcp://127.0.0.1:1").unwrap())
            .expect("tcp endpoints connect lazily")
    }

    async fn listening_port() -> (tokio::net::TcpListener, u16) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    async fn closed_port() -> u16 {
        listening_port().await.1
    }

    type BuildResult = Result<String, Box<dyn StdError>>;

    #[tokio::test]
    async fn connects_on_the_published_port_and_reports_the_phases() {
        let (_listener, port) = listening_port().await;
        let info = with_bindings(container("db"), &[("0.0.0.0", port)]);
        let mut phases = Vec::new();
        let connected = offline_client()
            .connect_via_routes(
                &info,
                |phase| phases.push(phase),
                |connection| async move {
                    BuildResult::Ok(format!("{}:{}", connection.host, connection.port))
                },
            )
            .await
            .unwrap();
        assert_eq!(connected.value, format!("127.0.0.1:{port}"));
        assert_eq!(connected.route.kind, RouteKind::PublishedPort);
        assert!(connected.route.is_direct());
        assert!(connected.tunnel.is_none());
        assert_eq!(
            phases,
            [
                DockerPhase::CheckingDirectAccess,
                DockerPhase::CheckingDirectAccess,
                DockerPhase::Authenticating
            ]
        );
    }

    #[tokio::test]
    async fn a_route_that_answers_but_fails_at_transport_level_falls_through() {
        let (_a, first) = listening_port().await;
        let (_b, second) = listening_port().await;
        let info = with_bindings(
            container("db"),
            &[("127.0.0.1", first), ("127.0.0.1", second)],
        );
        let mut calls = 0;
        let connected = offline_client()
            .connect_via_routes(
                &info,
                |_| {},
                |connection| {
                    calls += 1;
                    let first_call = calls == 1;
                    async move {
                        if first_call {
                            BuildResult::Err(
                                "error communicating with database: Connection reset by peer"
                                    .into(),
                            )
                        } else {
                            Ok(format!("{}", connection.port))
                        }
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(connected.value, second.to_string());
        assert_eq!(connected.route.port, second);
    }

    #[tokio::test]
    async fn an_authentication_failure_stops_without_trying_other_routes() {
        let (_a, first) = listening_port().await;
        let (_b, second) = listening_port().await;
        let info = with_bindings(
            container("db"),
            &[("127.0.0.1", first), ("127.0.0.1", second)],
        );
        let mut calls = 0;
        let error = offline_client()
            .connect_via_routes(&info, |_| {}, |_| {
                calls += 1;
                async {
                    BuildResult::Err(
                        "error returned from database: password authentication failed for user \"app\""
                            .into(),
                    )
                }
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("password authentication failed"));
        assert_eq!(
            calls, 1,
            "credentials must not be replayed on another route"
        );
    }

    #[tokio::test]
    async fn nothing_reachable_and_no_forwarding_fails_fast_with_the_attempts() {
        let closed = closed_port().await;
        let info = with_bindings(container("tt2-postgres-1"), &[("0.0.0.0", closed)]);
        let started = std::time::Instant::now();
        let error = offline_client()
            .connect_via_routes(&info, |_| {}, |_| async { BuildResult::Ok(String::new()) })
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let docker_error = error.downcast_ref::<DockerError>().expect("typed error");
        let DockerError::Unreachable(details) = docker_error else {
            panic!("expected Unreachable, got {docker_error:?}");
        };
        assert_eq!(details.container, "tt2-postgres-1");
        assert_eq!(details.port, 5432);
        assert_eq!(
            details.published.as_deref(),
            Some(format!("127.0.0.1:{closed}").as_str())
        );
        let routes: Vec<_> = details.attempts.iter().map(|a| a.route).collect();
        assert_eq!(routes, [RouteKind::PublishedPort, RouteKind::ExecTunnel]);
        let text = error.to_string();
        assert!(text.contains("Couldn't reach 'tt2-postgres-1'"), "{text}");
        assert!(text.contains("connection refused"), "{text}");
        assert!(text.contains(r#"ports: ["127.0.0.1::5432"]"#), "{text}");
    }

    #[tokio::test]
    async fn containers_without_a_database_port_are_rejected_early() {
        let mut info = container("lite");
        info.database_type = Some(DatabaseType::SQLite);
        info.actual_database_port = 0;
        let error = offline_client()
            .connect_via_routes(&info, |_| {}, |_| async { BuildResult::Ok(String::new()) })
            .await
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<DockerError>(),
            Some(DockerError::NoExposedPorts(_))
        ));
    }

    #[test]
    fn setup_timeout_names_the_step() {
        let text = DockerError::SetupTimeout {
            container: "db".into(),
            seconds: 10,
            phase: DockerPhase::OpeningForwarding,
        }
        .to_string();
        assert_eq!(
            text,
            "Connecting to 'db' took longer than 10 s (last step: opening forwarding)"
        );
    }
    // -- daemon identity, targets ---------------------------------------------

    #[rstest]
    #[case("docker://", "", None, None)]
    #[case("docker://db", "db", None, None)]
    #[case("docker://app@db/shop", "db", Some("app"), Some("shop"))]
    #[case(
        "docker://app:pw@db/shop?sslmode=require",
        "db",
        Some("app"),
        Some("shop")
    )]
    #[case("docker://db?sslmode=require", "db", None, None)]
    fn docker_targets_parse(
        #[case] url: &str,
        #[case] container: &str,
        #[case] user: Option<&str>,
        #[case] database: Option<&str>,
    ) {
        let target = DockerTarget::parse(url).unwrap();
        assert_eq!(target.container, container);
        assert_eq!(target.user.as_deref(), user);
        assert_eq!(target.database.as_deref(), database);
        // the old tuple API agrees (and ignores the query)
        let (u, _, c, d) = DockerClient::parse_docker_url(url).unwrap();
        assert_eq!(
            (u.as_deref(), c.as_str(), d.as_deref()),
            (user, container, database)
        );
        assert!(DockerTarget::parse("postgres://x/y").is_none());
    }

    // -- time budgets ------------------------------------------------------------

    fn tight(setup_ms: u64, handshake_ms: u64, reserve_ms: u64) -> RouteBudgets {
        RouteBudgets {
            setup: Duration::from_millis(setup_ms),
            handshake: Duration::from_millis(handshake_ms),
            forwarding_reserve: Duration::from_millis(reserve_ms),
            min_handshake: Duration::from_millis(50),
        }
    }

    fn unreachable_details<'a>(error: &'a (dyn StdError + 'static)) -> &'a Unreachable {
        match error.downcast_ref::<DockerError>() {
            Some(DockerError::Unreachable(details)) => details,
            other => panic!("expected Unreachable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_endpoint_that_never_finishes_the_handshake_falls_through() {
        // Accepts TCP (the kernel does), never speaks: a proxy to nothing.
        let (_silent, port) = listening_port().await;
        let info = with_bindings(container("tt2-postgres-1"), &[("0.0.0.0", port)]);
        let client = offline_client().with_budgets(tight(5000, 300, 200));
        let started = Instant::now();
        let error = client
            .connect_via_routes(
                &info,
                |_| {},
                |_| async { std::future::pending::<BuildResult>().await },
            )
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        let details = unreachable_details(error.as_ref());
        assert_eq!(details.attempts[0].route, RouteKind::PublishedPort);
        assert!(
            details.attempts[0]
                .reason
                .contains("handshake did not finish"),
            "{:?}",
            details.attempts
        );
        // ...and forwarding was still tried afterwards
        assert_eq!(
            details.attempts.last().unwrap().route,
            RouteKind::ExecTunnel
        );
    }

    #[tokio::test]
    async fn direct_routes_leave_time_for_forwarding() {
        let (_a, first) = listening_port().await;
        let (_b, second) = listening_port().await;
        let info = with_bindings(
            container("db"),
            &[("127.0.0.1", first), ("127.0.0.1", second)],
        );
        // 2 s in all, 1.2 s reserved: the first silent route may take 0.8 s,
        // which leaves nothing for a second one.
        let client = offline_client().with_budgets(tight(2000, 1500, 1200));
        let mut handshakes = 0;
        let started = Instant::now();
        let error = client
            .connect_via_routes(
                &info,
                |_| {},
                |_| {
                    handshakes += 1;
                    async { std::future::pending::<BuildResult>().await }
                },
            )
            .await
            .unwrap_err();
        assert_eq!(handshakes, 1, "the second direct route must not start");
        assert!(
            started.elapsed() < Duration::from_millis(1900),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            unreachable_details(error.as_ref())
                .attempts
                .last()
                .unwrap()
                .route,
            RouteKind::ExecTunnel
        );
    }

    #[tokio::test]
    async fn one_deadline_covers_the_container_lookup_too() {
        // A daemon that accepts connections and never answers the inspect.
        let (_silent, port) = listening_port().await;
        let client = DockerClient::with_endpoint(
            DockerEndpoint::explicit(&format!("tcp://127.0.0.1:{port}")).unwrap(),
        )
        .unwrap()
        .with_budgets(tight(400, 300, 100));
        let started = Instant::now();
        let error = client
            .connect_container(
                "tt2-postgres-1",
                |_| {},
                |_| async { BuildResult::Ok(String::new()) },
            )
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        match error.downcast_ref::<DockerError>() {
            Some(DockerError::SetupTimeout {
                phase, container, ..
            }) => {
                assert_eq!(*phase, DockerPhase::ResolvingDocker);
                assert_eq!(container, "tt2-postgres-1");
            }
            other => panic!("expected the overall timeout, got {other:?}"),
        }
    }
}
