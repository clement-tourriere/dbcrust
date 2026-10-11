//! Docker database container discovery, and how a container is reached.
//!
//! Discovery ([`discover`]) lists containers without probing anything.
//! Connecting is `ConnectionHandle::start_connect` in [`crate::worker`]: the
//! phases, the chosen [`DockerRoute`] and a failure's [`DockerFailure`] come
//! back through that API.
//!
//! Which daemon: listing and connecting use the current docker context. A
//! list that will be clicked on should come from [`discover_snapshot`] and
//! connect through `snapshot.daemon.connect_request(name)`, so a context
//! switch between the two can't send a container's credentials to a
//! same-named container on another daemon; the siblings of an open Docker
//! connection do this automatically. Saved sessions and recents are not
//! pinned: they always use the context that is current when they are opened.
//!
//! How a connect error surfaces: [`crate::GuiError::Connection`] carries the
//! full multi-line text (headline, reason, `Tried:` list, compose hint), and
//! `ConnectFailure::docker` carries the same facts typed for a
//! "Connection details" view.

use crate::error::{GuiError, GuiResult};
use crate::rt::run_detached;
use crate::types::ConnectRequest;
use dbcrust::database::{ConnectionInfo, DatabaseType};
use dbcrust::docker::{
    DockerClient, DockerEndpoint, DockerError, PINNED_DAEMON_OPTION, Unreachable,
};
use std::future::Future;
use std::sync::OnceLock;

pub use dbcrust::docker::{DockerPhase, DockerRoute, RouteAttempt, RouteKind};

static DISCOVERY_OVERRIDE: OnceLock<DockerDiscovery> = OnceLock::new();

/// Serve a fixed discovery result instead of querying the Docker daemon
/// (screenshot fixtures and tests). First call wins.
pub fn set_discovery_override(result: DockerDiscovery) {
    let _ = DISCOVERY_OVERRIDE.set(result);
}

/// A database container found on the local Docker daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub db_type: Option<DatabaseType>,
    pub host_port: Option<u16>,
    pub container_port: Option<u16>,
    pub running: bool,
}

impl DockerContainer {
    /// URL that connects to this container (credentials come from its
    /// environment variables).
    pub fn connection_url(&self) -> String {
        format!("docker://{}", self.name)
    }

    /// What is known about reaching this container without trying anything:
    /// a published port connects directly; otherwise DBCrust picks a route
    /// when connecting (container address, OrbStack domain, or forwarding
    /// through Docker). "Through Docker" is only true once a connection
    /// established it: see [`DockerRoute`].
    pub fn reach_hint(&self) -> ReachHint {
        match self.host_port {
            Some(port) => ReachHint::Published { port },
            None => ReachHint::NotPublished,
        }
    }
}

/// Probe-free answer to "how will this container be reached?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachHint {
    /// The database port is published on the host: direct connection.
    Published { port: u16 },
    /// Not published: the route is chosen when connecting.
    NotPublished,
}

impl ReachHint {
    /// Short row text: `Published · 32768` / `Port not published · Automatic`.
    pub fn label(&self) -> String {
        match self {
            ReachHint::Published { port } => format!("Published · {port}"),
            ReachHint::NotPublished => "Port not published · Automatic".to_string(),
        }
    }

    /// One sentence for a details pane.
    pub fn detail(&self) -> String {
        match self {
            ReachHint::Published { port } => {
                format!("Port {port} is published on this computer: connects directly.")
            }
            ReachHint::NotPublished => "The port isn't published. DBCrust tries the container's \
                network address and falls back to forwarding through Docker."
                .to_string(),
        }
    }

    pub fn is_published(&self) -> bool {
        matches!(self, ReachHint::Published { .. })
    }
}

/// Discovery outcome. "No containers" is a normal state, not an error.
#[derive(Debug, Clone, PartialEq)]
pub enum DockerDiscovery {
    /// Docker is reachable; possibly an empty list.
    Containers(Vec<DockerContainer>),
    /// The Docker daemon is not running, not reachable, or the selected
    /// context can't be used. The message names the endpoint.
    Unavailable(String),
}

/// The Docker context discovery and `docker://` connections talk to, for
/// messages such as "No database containers found in ‘orbstack’":
/// `Some(name)` for a docker context, `Some("DOCKER_HOST")` when that
/// variable selects the daemon, `None` for the platform default (or an
/// endpoint that can't be resolved).
pub fn endpoint_label() -> Option<String> {
    endpoint_label_of(&DockerEndpoint::from_environment().ok()?)
}

fn endpoint_label_of(endpoint: &DockerEndpoint) -> Option<String> {
    match &endpoint.source {
        dbcrust::docker::EndpointSource::Context(name) => Some(name.clone()),
        dbcrust::docker::EndpointSource::DockerHostEnv => Some("DOCKER_HOST".to_string()),
        _ => None,
    }
}

/// The Docker daemon a listing came from. Connect requests made through it
/// ([`DockerDaemon::connect_request`]) reach that very daemon, even if the
/// user switched docker contexts between the listing and the click: without
/// this, the credentials of one daemon's container would be sent to a
/// same-named container on another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerDaemon {
    pin: String,
    label: Option<String>,
}

impl DockerDaemon {
    /// The daemon `endpoint` selects (discovery builds it from the current
    /// context; screenshots and tests from an explicit endpoint).
    pub fn of(endpoint: &DockerEndpoint) -> Self {
        Self {
            pin: endpoint.pinned_value(),
            label: endpoint_label_of(endpoint),
        }
    }

    /// The context name (or `DOCKER_HOST`) to show: see [`endpoint_label`].
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// A request to connect to `container` on this daemon. Credentials come
    /// from the container's environment, as for `docker://<container>`.
    pub fn connect_request(&self, container: &str) -> ConnectRequest {
        ConnectRequest::info(ConnectionInfo {
            database_type: DatabaseType::PostgreSQL, // replaced by the container's engine
            host: None,
            port: None,
            username: None,
            password: None,
            database: None,
            file_path: None,
            options: [(PINNED_DAEMON_OPTION.to_string(), self.pin.clone())].into(),
            docker_container: Some(container.to_string()),
            use_tls: false,
        })
    }
}

/// A discovery result with the daemon it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct DockerSnapshot {
    pub discovery: DockerDiscovery,
    /// `None` when the daemon could not be determined (or for a fixed
    /// result served by [`set_discovery_override`]): requests then use
    /// whatever daemon is current.
    pub daemon: Option<DockerDaemon>,
}

/// List database containers (running first). Never blocks the caller. Use
/// [`discover_snapshot`] to also learn which daemon answered.
pub fn discover() -> impl Future<Output = GuiResult<DockerDiscovery>> + Send + 'static {
    let snapshot = discover_snapshot();
    async move { snapshot.await.map(|snapshot| snapshot.discovery) }
}

/// [`discover`], with the daemon the list came from. Connect with
/// `snapshot.daemon.connect_request(name)` so the connection goes to that
/// daemon (persisted recents and sessions keep using the current context).
pub fn discover_snapshot() -> impl Future<Output = GuiResult<DockerSnapshot>> + Send + 'static {
    let fixed = DISCOVERY_OVERRIDE.get().cloned();
    run_detached("docker", || async {
        if let Some(discovery) = fixed {
            return Ok(DockerSnapshot {
                discovery,
                daemon: None,
            });
        }
        let unavailable = |e: DockerError, daemon| {
            Ok(DockerSnapshot {
                discovery: DockerDiscovery::Unavailable(e.to_string()),
                daemon,
            })
        };
        let endpoint = match DockerEndpoint::from_environment() {
            Ok(endpoint) => endpoint,
            Err(e) => return unavailable(e, None),
        };
        let daemon = Some(DockerDaemon::of(&endpoint));
        let client = match DockerClient::with_endpoint(endpoint) {
            Ok(client) => client,
            Err(e) => return unavailable(e, daemon),
        };
        let discovery = match client.list_database_containers().await {
            Ok(containers) => DockerDiscovery::Containers(
                containers
                    .into_iter()
                    .map(|container| DockerContainer {
                        running: container.status.contains("running")
                            || container.status.contains("Up"),
                        id: container.id,
                        name: container.name,
                        image: container.image,
                        status: container.status,
                        db_type: container.database_type,
                        host_port: container.host_port,
                        container_port: container.container_port,
                    })
                    .collect(),
            ),
            Err(DockerError::NoDatabaseContainers) => DockerDiscovery::Containers(Vec::new()),
            Err(
                e @ (DockerError::ConnectionError(_)
                | DockerError::DaemonUnavailable { .. }
                | DockerError::DaemonTimeout { .. }
                | DockerError::Endpoint(_)),
            ) => DockerDiscovery::Unavailable(e.to_string()),
            Err(e) => return Err(GuiError::Docker(e.to_string())),
        };
        Ok(DockerSnapshot { discovery, daemon })
    })
}

/// Why no route reached a container, typed for a "Connection details" view.
/// The same facts are in the text of the error ([`DockerFailure::message`]).
#[derive(Debug, Clone, PartialEq)]
pub struct DockerFailure {
    pub container: String,
    /// The database port inside the container.
    pub port: u16,
    /// `Couldn't reach 'name'`.
    pub headline: String,
    /// Why, in a few sentences (published or not, VPN hint, helper missing).
    pub explanation: String,
    /// Every route that was tried, in order, with why it failed.
    pub attempts: Vec<RouteAttempt>,
    /// A copyable compose line publishing the port on loopback only.
    pub compose_example: String,
    /// What applying it does ("recreates the container").
    pub compose_note: String,
    /// The complete message.
    pub message: String,
}

impl DockerFailure {
    /// The typed details of a connect error, if it is a "no route" failure.
    pub fn from_error(error: &(dyn std::error::Error + 'static)) -> Option<Self> {
        match error.downcast_ref::<DockerError>()? {
            DockerError::Unreachable(details) => Some(Self::from_unreachable(details)),
            _ => None,
        }
    }

    fn from_unreachable(details: &Unreachable) -> Self {
        Self {
            container: details.container.clone(),
            port: details.port,
            headline: details.headline(),
            explanation: details.explanation(),
            attempts: details.attempts.clone(),
            compose_example: details.compose_example(),
            compose_note: Unreachable::COMPOSE_NOTE.to_string(),
            message: details.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn container(host_port: Option<u16>) -> DockerContainer {
        DockerContainer {
            id: "abc".into(),
            name: "tt2-postgres-1".into(),
            image: "postgres:16".into(),
            status: "Up 2 hours".into(),
            db_type: Some(DatabaseType::PostgreSQL),
            host_port,
            container_port: Some(5432),
            running: true,
        }
    }

    #[rstest]
    #[case(Some(32768), ReachHint::Published { port: 32768 }, "Published · 32768", true)]
    #[case(None, ReachHint::NotPublished, "Port not published · Automatic", false)]
    fn reach_hint_is_derived_without_probing(
        #[case] host_port: Option<u16>,
        #[case] hint: ReachHint,
        #[case] label: &str,
        #[case] published: bool,
    ) {
        let container = container(host_port);
        assert_eq!(container.reach_hint(), hint);
        assert_eq!(hint.label(), label);
        assert_eq!(hint.is_published(), published);
        assert!(!hint.detail().is_empty());
        assert_eq!(container.connection_url(), "docker://tt2-postgres-1");
    }

    #[test]
    fn unreachable_errors_are_exposed_typed() {
        let error: Box<dyn std::error::Error> = DockerError::Unreachable(Unreachable {
            container: "tt2-postgres-1".into(),
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
        let failure = DockerFailure::from_error(error.as_ref()).expect("typed");
        assert_eq!(failure.headline, "Couldn't reach 'tt2-postgres-1'");
        assert_eq!(failure.compose_example, r#"ports: ["127.0.0.1::5432"]"#);
        assert!(failure.compose_note.contains("recreates the container"));
        assert_eq!(failure.attempts.len(), 1);
        assert_eq!(failure.message, error.to_string());

        let other: Box<dyn std::error::Error> =
            DockerError::ContainerNotFound("nope".into()).into();
        assert!(DockerFailure::from_error(other.as_ref()).is_none());
    }
}
