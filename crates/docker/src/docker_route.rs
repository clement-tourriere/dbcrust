//! How a container's database port is reached from this machine: ordered
//! candidate addresses, bounded concurrent probing, the route that was
//! finally used, and the error that explains a failure.
//!
//! Order of preference: published binding → OrbStack custom/compose domain →
//! container network address → OrbStack automatic domain → `docker exec`
//! forwarding ([`crate::docker_tunnel`]). A TCP answer only proves
//! reachability: the caller still has to complete the database handshake, and
//! may fall through to the next route when that fails at the transport level
//! (see [`is_transport_failure`]).

use crate::docker::DockerContainerInfo;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::time::Duration;

/// How long one DNS + TCP probe may take. A swallowed address (VPN) never
/// answers, so this bounds the wait before the next strategy.
pub const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Upper bound for the whole direct-probe phase.
pub const PROBE_BUDGET: Duration = Duration::from_millis(2000);
/// The preferred candidate runs alone this long before the others start.
pub const PROBE_HEAD_START: Duration = Duration::from_millis(250);
/// Probes in flight at once.
pub const PROBE_MAX_CONCURRENT: usize = 3;
/// Upper bound for the whole connection setup: it starts before the container
/// is inspected and covers probing, forwarding and every handshake.
pub const SETUP_BUDGET: Duration = Duration::from_secs(10);
/// How long the database handshake on one route may take. A TCP answer from
/// something that never speaks the protocol (a proxy forwarding to nothing)
/// falls through to the next route after this.
pub const HANDSHAKE_BUDGET: Duration = Duration::from_secs(4);
/// Time kept back for forwarding (helper probe, relay, its handshake) when
/// direct routes burn their handshake budgets.
pub const FORWARDING_RESERVE: Duration = Duration::from_secs(3);

/// How a container's database port is reached from this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// Published port on the host.
    PublishedPort,
    /// OrbStack custom (`dev.orbstack.domains`) or compose domain.
    OrbstackDomain,
    /// The container's address on its Docker network.
    ContainerIp,
    /// OrbStack's `<container>.orb.local` domain.
    OrbstackAutoDomain,
    /// A `docker exec` relay (works while the daemon does).
    ExecTunnel,
}

impl RouteKind {
    /// Short label for UIs, e.g. "Published port".
    pub fn label(self) -> &'static str {
        match self {
            RouteKind::PublishedPort => "Published port",
            RouteKind::OrbstackDomain | RouteKind::OrbstackAutoDomain => "OrbStack domain",
            RouteKind::ContainerIp => "Container address",
            RouteKind::ExecTunnel => "Through Docker",
        }
    }
}

/// One address to try, in priority order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachCandidate {
    pub route: RouteKind,
    pub host: String,
    pub port: u16,
}

impl ReachCandidate {
    pub fn new(route: RouteKind, host: impl Into<String>, port: u16) -> Self {
        Self {
            route,
            host: host.into(),
            port,
        }
    }

    pub fn address(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// What is known about the daemon when ordering candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachContext {
    /// The daemon really is OrbStack (so `*.orb.local` domains exist).
    pub orbstack: bool,
    /// Container addresses can be routed from this machine (Linux host, or
    /// OrbStack); false for Docker Desktop/Colima VMs and remote daemons.
    pub container_ip_routable: bool,
    /// Host that wildcard-published ports are reachable on when the daemon is
    /// remote; `None` means this machine (loopback).
    pub publish_host: Option<String>,
}

/// Ordered candidates. Pure.
pub fn reach_candidates(info: &DockerContainerInfo, ctx: &ReachContext) -> Vec<ReachCandidate> {
    let port = info.actual_database_port;
    let mut candidates: Vec<ReachCandidate> = Vec::new();
    let mut push = |candidate: ReachCandidate| {
        if !candidates
            .iter()
            .any(|c| c.host == candidate.host && c.port == candidate.port)
        {
            candidates.push(candidate);
        }
    };

    for (host, host_port) in published_endpoints(info, ctx.publish_host.as_deref()) {
        push(ReachCandidate::new(
            RouteKind::PublishedPort,
            host,
            host_port,
        ));
    }
    if ctx.orbstack {
        for domain in orbstack_domains(info) {
            push(ReachCandidate::new(RouteKind::OrbstackDomain, domain, port));
        }
    }
    if ctx.container_ip_routable {
        let address = [info.ip_address.as_deref(), info.ipv6_address.as_deref()]
            .into_iter()
            .flatten()
            .find(|ip| !ip.is_empty());
        if let Some(ip) = address {
            push(ReachCandidate::new(RouteKind::ContainerIp, ip, port));
        }
    }
    if ctx.orbstack && !info.name.is_empty() {
        push(ReachCandidate::new(
            RouteKind::OrbstackAutoDomain,
            format!("{}.orb.local", info.name),
            port,
        ));
    }
    candidates
}

/// How a published binding's address reads.
enum BindingAddress {
    /// Every interface (`0.0.0.0`, `::`, or unspecified by an older daemon).
    Wildcard { ipv6: bool },
    /// The host's own loopback: any `127.0.0.0/8` address, or `::1`.
    Loopback(IpAddr),
    /// One specific address.
    Specific(IpAddr),
}

fn classify_binding(host_ip: &str) -> Option<BindingAddress> {
    let host_ip = host_ip.trim();
    if host_ip.is_empty() {
        return Some(BindingAddress::Wildcard { ipv6: false });
    }
    // Zone ids (`fe80::1%en0`) aren't dialable here
    let ip: IpAddr = host_ip.parse().ok()?;
    Some(if ip.is_unspecified() {
        BindingAddress::Wildcard { ipv6: ip.is_ipv6() }
    } else if ip.is_loopback() {
        BindingAddress::Loopback(ip)
    } else {
        BindingAddress::Specific(ip)
    })
}

/// Where each published binding of the database port can be dialled.
///
/// * Local daemon: a wildcard binding is reached on `127.0.0.1` (`::1` when
///   only an IPv6 wildcard exists); a loopback binding on that very address;
///   a specific address only there.
/// * Remote daemon: its loopback is not this computer's, so every loopback
///   binding (all of `127.0.0.0/8`, `::1`) is unusable; a wildcard binding
///   means "all interfaces of the daemon's host" and is dialled on that
///   host; a specific address is dialled as is.
///
/// Bindings whose address can't be parsed are ignored. IPv4 comes first.
pub fn published_endpoints(
    info: &DockerContainerInfo,
    remote_host: Option<&str>,
) -> Vec<(String, u16)> {
    let bindings: Vec<(BindingAddress, u16)> = info
        .bindings
        .iter()
        .filter_map(|b| Some((classify_binding(&b.host_ip)?, b.host_port)))
        .collect();
    let covered_by_ipv4 = bindings.iter().any(|(address, _)| match address {
        BindingAddress::Wildcard { ipv6 } => !ipv6,
        BindingAddress::Loopback(ip) => ip.is_ipv4(),
        BindingAddress::Specific(_) => false,
    });

    let mut endpoints: Vec<(String, u16)> = Vec::new();
    let mut push = |host: String, port: u16| {
        if !endpoints.contains(&(host.clone(), port)) {
            endpoints.push((host, port));
        }
    };
    let is_ipv6 = |address: &BindingAddress| match address {
        BindingAddress::Wildcard { ipv6 } => *ipv6,
        BindingAddress::Loopback(ip) | BindingAddress::Specific(ip) => ip.is_ipv6(),
    };
    for ipv6_pass in [false, true] {
        for (address, port) in &bindings {
            if is_ipv6(address) != ipv6_pass {
                continue;
            }
            match (address, remote_host) {
                (BindingAddress::Loopback(_), Some(_)) => {}
                (BindingAddress::Wildcard { .. }, Some(host)) => push(host.to_string(), *port),
                // The IPv4 binding already covers this interface
                (BindingAddress::Wildcard { ipv6: true }, None) if covered_by_ipv4 => {}
                (BindingAddress::Wildcard { ipv6: true }, None) => push("::1".into(), *port),
                (BindingAddress::Wildcard { ipv6: false }, None) => push("127.0.0.1".into(), *port),
                (BindingAddress::Loopback(ip) | BindingAddress::Specific(ip), _) => {
                    push(ip.to_string(), *port)
                }
            }
        }
    }
    endpoints
}

/// OrbStack domains for a container: every non-wildcard `dev.orbstack.domains`
/// entry, then `<service>.<project>.orb.local` for compose containers. Names
/// are not guessed: whatever answers a probe is a real registration.
pub fn orbstack_domains(info: &DockerContainerInfo) -> Vec<String> {
    let mut domains: Vec<String> = info
        .labels
        .get("dev.orbstack.domains")
        .map(|labels| {
            labels
                .split(',')
                .map(str::trim)
                .filter(|d| !d.is_empty() && !d.contains('*'))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(domain) = compose_orbstack_domain(info) {
        domains.push(domain);
    }
    domains
}

/// Compose domain (`service.project.orb.local`), from the compose labels.
pub fn compose_orbstack_domain(info: &DockerContainerInfo) -> Option<String> {
    let project = info.labels.get("com.docker.compose.project")?;
    Some(match info.labels.get("com.docker.compose.service") {
        Some(service) => format!("{service}.{project}.orb.local"),
        None => format!("{project}.orb.local"),
    })
}

// ---------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------

/// Timing knobs of [`probe_candidates`].
#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    pub per_attempt: Duration,
    pub budget: Duration,
    pub head_start: Duration,
    pub max_concurrent: usize,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            per_attempt: PROBE_ATTEMPT_TIMEOUT,
            budget: PROBE_BUDGET,
            head_start: PROBE_HEAD_START,
            max_concurrent: PROBE_MAX_CONCURRENT,
        }
    }
}

/// Result of one probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Never started: a better candidate answered first.
    NotTried,
    Reachable,
    /// Why it failed, e.g. `timed out after 1.5 s`.
    Failed(String),
}

/// What probing found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// Index into `attempts` of the chosen (highest-priority reachable) one.
    pub chosen: Option<usize>,
    pub attempts: Vec<(ReachCandidate, ProbeOutcome)>,
}

impl ProbeReport {
    /// The attempts that failed, as records for the error message.
    pub fn failures(&self) -> Vec<RouteAttempt> {
        self.attempts
            .iter()
            .filter_map(|(candidate, outcome)| match outcome {
                ProbeOutcome::Failed(reason) => Some(RouteAttempt {
                    route: candidate.route,
                    target: candidate.address(),
                    reason: reason.clone(),
                }),
                ProbeOutcome::Reachable | ProbeOutcome::NotTried => None,
            })
            .collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// Highest-priority reachable candidate, and nothing above it is pending.
    Chosen(usize),
    /// A higher-priority candidate has not answered yet (or not started).
    Wait,
    /// Every candidate failed.
    NoneReachable,
}

/// Decide among partial results (`None` = still pending), by priority.
pub(crate) fn pick(results: &[Option<bool>]) -> Pick {
    for (index, result) in results.iter().enumerate() {
        match result {
            Some(true) => return Pick::Chosen(index),
            None => return Pick::Wait,
            Some(false) => {}
        }
    }
    Pick::NoneReachable
}

fn describe_connect_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => "connection refused".to_string(),
        io::ErrorKind::TimedOut => "timed out".to_string(),
        _ => error.to_string(),
    }
}

/// Probe candidates and return the highest-priority one that answers.
///
/// The preferred (first) candidate runs alone for `head_start`, so the common
/// case (a published port) never fans out; the rest then race with at most
/// `max_concurrent` probes in flight. A failed candidate releases the next
/// immediately. Candidates unanswered when `budget` ends count as failures,
/// but a reachable one that already answered is still used.
pub async fn probe_candidates<F, Fut>(
    candidates: &[ReachCandidate],
    config: &ProbeConfig,
    connect: F,
) -> ProbeReport
where
    F: Fn(ReachCandidate) -> Fut,
    Fut: Future<Output = io::Result<()>>,
{
    let total = candidates.len();
    let mut results: Vec<Option<bool>> = vec![None; total];
    let mut outcomes: Vec<ProbeOutcome> = vec![ProbeOutcome::NotTried; total];
    let unanswered = ProbeOutcome::Failed(format!(
        "no answer within {:.1} s",
        config.budget.as_secs_f32()
    ));

    let per_attempt = config.per_attempt;
    let mut pending = FuturesUnordered::new();
    let mut started = 0usize;
    let mut head_start_over = config.head_start.is_zero();
    let head_start = tokio::time::sleep(config.head_start);
    let deadline = tokio::time::sleep(config.budget);
    tokio::pin!(head_start, deadline);

    let mut chosen = None;
    loop {
        // Decide first: once a candidate is chosen, nothing below it starts.
        match pick(&results) {
            Pick::Chosen(index) => {
                chosen = Some(index);
                break;
            }
            Pick::NoneReachable => break,
            Pick::Wait => {}
        }
        // Start more probes: always one if none is running, more once the
        // preferred candidate had its head start.
        while started < total
            && pending.len() < config.max_concurrent.max(1)
            && (head_start_over || pending.is_empty())
        {
            let index = started;
            // Until it answers, a started probe counts as unanswered.
            outcomes[index] = unanswered.clone();
            let attempt = connect(candidates[index].clone());
            pending.push(async move {
                let outcome = match tokio::time::timeout(per_attempt, attempt).await {
                    Ok(Ok(())) => ProbeOutcome::Reachable,
                    Ok(Err(e)) => ProbeOutcome::Failed(describe_connect_error(&e)),
                    Err(_) => ProbeOutcome::Failed(format!(
                        "timed out after {:.1} s",
                        per_attempt.as_secs_f32()
                    )),
                };
                (index, outcome)
            });
            started += 1;
        }
        if pending.is_empty() {
            break;
        }
        tokio::select! {
            Some((index, outcome)) = pending.next() => {
                results[index] = Some(outcome == ProbeOutcome::Reachable);
                outcomes[index] = outcome;
            }
            _ = &mut head_start, if !head_start_over => head_start_over = true,
            _ = &mut deadline => {
                // Out of time: take the best candidate that did answer.
                chosen = results.iter().position(|r| *r == Some(true));
                break;
            }
        }
    }

    ProbeReport {
        chosen,
        attempts: candidates.iter().cloned().zip(outcomes).collect(),
    }
}

/// The real probe: DNS + TCP connect.
pub async fn tcp_connect(candidate: ReachCandidate) -> io::Result<()> {
    tokio::net::TcpStream::connect((candidate.host.as_str(), candidate.port))
        .await
        .map(drop)
}

// ---------------------------------------------------------------------------
// Phases, routes and the failure report
// ---------------------------------------------------------------------------

/// Steps of connecting to a container, in the order they are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerPhase {
    /// Asking the daemon about the container.
    ResolvingDocker,
    /// Probing the published port / OrbStack domain / container address.
    CheckingDirectAccess,
    /// Setting up `docker exec` forwarding.
    OpeningForwarding,
    /// The database handshake (authentication, TLS, read-only setup).
    Authenticating,
}

impl DockerPhase {
    pub fn label(self) -> &'static str {
        match self {
            DockerPhase::ResolvingDocker => "Resolving Docker",
            DockerPhase::CheckingDirectAccess => "Checking direct access",
            DockerPhase::OpeningForwarding => "Opening forwarding",
            DockerPhase::Authenticating => "Authenticating",
        }
    }
}

/// The route a connection ended up using.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerRoute {
    pub kind: RouteKind,
    /// The endpoint the database client dials.
    pub host: String,
    pub port: u16,
    /// The relay program for [`RouteKind::ExecTunnel`] (`perl`, `socat`, `nc`).
    pub helper: Option<String>,
    /// One line for a "Connection details" view.
    pub detail: String,
}

impl DockerRoute {
    pub fn is_forwarded(&self) -> bool {
        self.kind == RouteKind::ExecTunnel
    }

    pub fn is_direct(&self) -> bool {
        !self.is_forwarded()
    }
}

/// One route that was tried and failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteAttempt {
    pub route: RouteKind,
    /// `host:port`, or the relay description for forwarding.
    pub target: String,
    pub reason: String,
}

/// Why the database port of a container could not be reached by any route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreachable {
    pub container: String,
    /// The database port inside the container.
    pub port: u16,
    /// A published binding of that port (`127.0.0.1:32768`), if any.
    pub published: Option<String>,
    /// Everything that was tried, forwarding last.
    pub attempts: Vec<RouteAttempt>,
}

impl Unreachable {
    /// `Couldn't reach 'name'`.
    pub fn headline(&self) -> String {
        format!("Couldn't reach '{}'", self.container)
    }

    /// The reason in a few sentences.
    pub fn explanation(&self) -> String {
        let port = self.port;
        let mut text = match &self.published {
            Some(published) => {
                format!("Port {port} is published on {published}, but nothing answered there")
            }
            None => format!("Port {port} isn't published"),
        };
        let direct_tried = self
            .attempts
            .iter()
            .any(|a| a.route != RouteKind::ExecTunnel && a.route != RouteKind::PublishedPort);
        if direct_tried {
            text.push_str(
                ", and the container address didn't respond. A VPN may be blocking that route.",
            );
        } else {
            text.push_str(", and the container address can't be reached from this computer.");
        }
        if let Some(forwarding) = self
            .attempts
            .iter()
            .rev()
            .find(|a| a.route == RouteKind::ExecTunnel)
        {
            let mut reason = forwarding.reason.clone();
            if let Some(first) = reason.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            text.push(' ');
            text.push_str(&reason);
            if !reason.ends_with('.') {
                text.push('.');
            }
        }
        text
    }

    /// A localhost-only publish example for a compose file.
    pub fn compose_example(&self) -> String {
        format!("ports: [\"127.0.0.1::{}\"]", self.port)
    }

    /// What applying [`Unreachable::compose_example`] does.
    pub const COMPOSE_NOTE: &'static str =
        "Applying it recreates the container; the port is then reachable on this computer only.";
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.\n{}", self.headline(), self.explanation())?;
        if !self.attempts.is_empty() {
            write!(f, "\nTried:")?;
            for attempt in &self.attempts {
                write!(f, "\n  - {}: {}", attempt.target, attempt.reason)?;
            }
        }
        write!(
            f,
            "\nTo publish the port on this computer only, add to the service in compose.yaml \
             (recreates the container):\n  {}",
            self.compose_example()
        )
    }
}

/// Whether a database connection error means "this route doesn't carry the
/// connection" (try the next one) rather than "the database answered and said
/// no" (authentication, certificates, unknown database: stop, and never
/// resubmit credentials elsewhere). Matches on the rendered message because
/// the engines wrap their errors as strings.
pub fn is_transport_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    const STOP: &[&str] = &[
        "password",
        "authentication",
        "access denied",
        "permission denied",
        "certificate",
        "tls",
        "ssl",
        "does not exist",
        "unknown database",
        "role ",
    ];
    const TRANSPORT: &[&str] = &[
        "connection refused",
        "connection reset",
        "connection closed",
        "connection aborted",
        "broken pipe",
        "unexpected eof",
        "unexpected end of file",
        "early eof",
        "timed out",
        "timeout",
        "no route to host",
        "network is unreachable",
        "host is unreachable",
        "error communicating with database",
        "failed to lookup address",
        "name or service not known",
    ];
    !STOP.iter().any(|needle| message.contains(needle))
        && TRANSPORT.iter().any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::PublishedBinding;
    use dbcrust_core::database::DatabaseType;
    use rstest::rstest;
    use std::collections::HashMap;

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

    fn published(mut info: DockerContainerInfo, bindings: &[(&str, u16)]) -> DockerContainerInfo {
        info.bindings = bindings
            .iter()
            .map(|(ip, port)| PublishedBinding {
                host_ip: ip.to_string(),
                host_port: *port,
            })
            .collect();
        info.host_port = info.bindings.first().map(|b| b.host_port);
        info
    }

    fn compose_container(project: &str, service: &str) -> DockerContainerInfo {
        let mut info = container(&format!("{project}-{service}-1"));
        info.labels
            .insert("com.docker.compose.project".into(), project.into());
        info.labels
            .insert("com.docker.compose.service".into(), service.into());
        info.ip_address = Some("192.168.107.3".into());
        info
    }

    fn ctx(orbstack: bool, ip_routable: bool) -> ReachContext {
        ReachContext {
            orbstack,
            container_ip_routable: ip_routable,
            publish_host: None,
        }
    }

    fn summary(candidates: &[ReachCandidate]) -> Vec<(RouteKind, String)> {
        candidates.iter().map(|c| (c.route, c.address())).collect()
    }

    #[test]
    fn orbstack_candidates_follow_the_priority_order() {
        let mut info = published(compose_container("tt2", "postgres"), &[("0.0.0.0", 32768)]);
        info.labels.insert(
            "dev.orbstack.domains".into(),
            "db.test, *.wild.test, db2.test,".into(),
        );
        assert_eq!(
            summary(&reach_candidates(&info, &ctx(true, true))),
            vec![
                (RouteKind::PublishedPort, "127.0.0.1:32768".to_string()),
                (RouteKind::OrbstackDomain, "db.test:5432".to_string()),
                (RouteKind::OrbstackDomain, "db2.test:5432".to_string()),
                (
                    RouteKind::OrbstackDomain,
                    "postgres.tt2.orb.local:5432".to_string()
                ),
                (RouteKind::ContainerIp, "192.168.107.3:5432".to_string()),
                (
                    RouteKind::OrbstackAutoDomain,
                    "tt2-postgres-1.orb.local:5432".to_string()
                ),
            ]
        );
    }

    #[test]
    fn unpublished_orbstack_container_has_no_published_candidate() {
        let info = compose_container("tt2", "postgres");
        let routes: Vec<_> = reach_candidates(&info, &ctx(true, true))
            .iter()
            .map(|c| c.route)
            .collect();
        assert_eq!(
            routes,
            [
                RouteKind::OrbstackDomain,
                RouteKind::ContainerIp,
                RouteKind::OrbstackAutoDomain
            ]
        );
    }

    #[test]
    fn orbstack_domains_are_never_tried_on_other_daemons() {
        // A Mac running Docker Desktop or Colima: OrbStack names don't exist
        // and container addresses aren't routed.
        let info = published(compose_container("tt2", "postgres"), &[("0.0.0.0", 5433)]);
        assert_eq!(
            summary(&reach_candidates(&info, &ctx(false, false))),
            vec![(RouteKind::PublishedPort, "127.0.0.1:5433".to_string())]
        );
        // ...and an unpublished container then goes straight to forwarding.
        let info = compose_container("tt2", "postgres");
        assert!(reach_candidates(&info, &ctx(false, false)).is_empty());
    }

    #[test]
    fn linux_hosts_try_the_container_ip() {
        let info = compose_container("app", "db");
        assert_eq!(
            summary(&reach_candidates(&info, &ctx(false, true))),
            vec![(RouteKind::ContainerIp, "192.168.107.3:5432".to_string())]
        );
    }

    #[test]
    fn an_ipv6_only_container_is_tried_on_its_ipv6_address() {
        let mut info = container("db");
        info.ipv6_address = Some("fd00:d1b:c0de::2".into());
        assert_eq!(
            summary(&reach_candidates(&info, &ctx(false, true))),
            vec![(
                RouteKind::ContainerIp,
                "[fd00:d1b:c0de::2]:5432".to_string()
            )]
        );
        // IPv4 is preferred when the container has both
        info.ip_address = Some("172.18.0.5".into());
        assert_eq!(
            summary(&reach_candidates(&info, &ctx(false, true))),
            vec![(RouteKind::ContainerIp, "172.18.0.5:5432".to_string())]
        );
    }

    #[test]
    fn a_container_name_is_not_turned_into_a_guessed_domain() {
        // No compose labels: only the automatic `<name>.orb.local` is tried.
        let mut info = container("myproject-postgres-1");
        info.ip_address = None;
        assert_eq!(compose_orbstack_domain(&info), None);
        assert_eq!(
            summary(&reach_candidates(&info, &ctx(true, true))),
            vec![(
                RouteKind::OrbstackAutoDomain,
                "myproject-postgres-1.orb.local:5432".to_string()
            )]
        );
    }

    #[rstest]
    // dual-stack wildcard: one loopback endpoint
    #[case(&[("0.0.0.0", 15432), ("::", 15432)], None, &["127.0.0.1:15432"])]
    // wildcard-less IPv6-only publication is reached on ::1
    #[case(&[("::", 15432)], None, &["::1:15432"])]
    #[case(&[("127.0.0.1", 15432)], None, &["127.0.0.1:15432"])]
    #[case(&[("::1", 15432)], None, &["::1:15432"])]
    // specific address: only there
    #[case(&[("192.168.1.20", 15432)], None, &["192.168.1.20:15432"])]
    // empty host ip (older daemons) behaves like the wildcard
    #[case(&[("", 15432)], None, &["127.0.0.1:15432"])]
    // IPv4 first, distinct ports kept
    #[case(&[("::1", 1), ("127.0.0.1", 2)], None, &["127.0.0.1:2", "::1:1"])]
    // a remote daemon's wildcard is the daemon host; its loopback is not ours
    #[case(&[("0.0.0.0", 15432), ("::", 15432)], Some("docker.lab"), &["docker.lab:15432"])]
    #[case(&[("127.0.0.1", 15432)], Some("docker.lab"), &[])]
    // every loopback address is ours, not the remote daemon's
    #[case(&[("127.0.0.2", 15432), ("127.1.2.3", 15433), ("::1", 15434)], Some("docker.lab"), &[])]
    // ...while a remote daemon's specific address is fine
    #[case(&[("127.0.0.2", 1), ("10.1.2.3", 2)], Some("docker.lab"), &["10.1.2.3:2"])]
    // locally the whole 127.0.0.0/8 range is dialled as is
    #[case(&[("127.0.0.2", 15432)], None, &["127.0.0.2:15432"])]
    // an IPv4 wildcard also covers the IPv6 one; a bound ::1 stays reachable
    #[case(&[("::", 1), ("0.0.0.0", 1), ("::1", 2)], None, &["127.0.0.1:1", "::1:2"])]
    // garbage and zone ids can't be dialled
    #[case(&[("not-an-ip", 1), ("fe80::1%en0", 2)], None, &[])]
    #[case(&[("not-an-ip", 1)], Some("docker.lab"), &[])]
    fn published_bindings_become_endpoints(
        #[case] bindings: &[(&str, u16)],
        #[case] remote: Option<&str>,
        #[case] expected: &[&str],
    ) {
        let info = published(container("db"), bindings);
        let got: Vec<String> = published_endpoints(&info, remote)
            .into_iter()
            .map(|(host, port)| format!("{host}:{port}"))
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn ipv6_endpoints_are_bracketed_in_messages() {
        let candidate = ReachCandidate::new(RouteKind::PublishedPort, "::1", 5432);
        assert_eq!(candidate.address(), "[::1]:5432");
    }

    #[test]
    fn candidates_use_the_port_the_database_listens_on() {
        let mut info = compose_container("app", "db");
        info.actual_database_port = 5433;
        assert_eq!(
            reach_candidates(&info, &ctx(false, true))[0].address(),
            "192.168.107.3:5433"
        );
    }

    // -- probing ------------------------------------------------------------

    fn candidate(host: &str) -> ReachCandidate {
        ReachCandidate::new(RouteKind::ContainerIp, host, 5432)
    }

    /// Fake connector: `host` is `<ok|err>-<delay ms>`.
    async fn fake_connect(candidate: ReachCandidate) -> io::Result<()> {
        let (verdict, delay) = candidate.host.split_once('-').unwrap();
        tokio::time::sleep(Duration::from_millis(delay.parse().unwrap())).await;
        match verdict {
            "ok" => Ok(()),
            _ => Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
        }
    }

    fn config(per_attempt_ms: u64, budget_ms: u64, head_start_ms: u64, max: usize) -> ProbeConfig {
        ProbeConfig {
            per_attempt: Duration::from_millis(per_attempt_ms),
            budget: Duration::from_millis(budget_ms),
            head_start: Duration::from_millis(head_start_ms),
            max_concurrent: max,
        }
    }

    async fn probe(hosts: &[&str], config: ProbeConfig) -> ProbeReport {
        let candidates: Vec<_> = hosts.iter().map(|h| candidate(h)).collect();
        probe_candidates(&candidates, &config, fake_connect).await
    }

    #[tokio::test]
    async fn priority_beats_speed_among_reachable_candidates() {
        // #0 answers late, #1 immediately: #0 still wins.
        let report = probe(&["ok-120", "ok-0"], config(1000, 2000, 30, 3)).await;
        assert_eq!(report.chosen, Some(0));
    }

    #[tokio::test]
    async fn the_preferred_candidate_gets_a_head_start() {
        // #0 answers within its head start: #1 is never even started, so its
        // (would-be) failure isn't reported.
        let report = probe(&["ok-20", "err-0"], config(1000, 2000, 200, 3)).await;
        assert_eq!(report.chosen, Some(0));
        assert!(report.failures().is_empty());
    }

    #[tokio::test]
    async fn a_failed_high_priority_candidate_releases_the_next_immediately() {
        let started = std::time::Instant::now();
        let report = probe(&["err-0", "ok-0", "ok-1500"], config(2000, 3000, 1000, 3)).await;
        assert_eq!(report.chosen, Some(1));
        // Neither the head start nor the slow #2 was waited for.
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn concurrency_is_bounded() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let candidates: Vec<_> = (0..8).map(|i| candidate(&format!("err-{i}"))).collect();
        let (r, p) = (running.clone(), peak.clone());
        let report = probe_candidates(&candidates, &config(1000, 3000, 0, 3), move |c| {
            let (r, p) = (r.clone(), p.clone());
            async move {
                let now = r.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(now, Ordering::SeqCst);
                let result = fake_connect(c).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
                r.fetch_sub(1, Ordering::SeqCst);
                result
            }
        })
        .await;
        assert_eq!(report.chosen, None);
        assert_eq!(report.attempts.len(), 8);
        assert!(
            peak.load(Ordering::SeqCst) <= 3,
            "{}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn unanswered_candidates_time_out_and_are_reported() {
        let started = std::time::Instant::now();
        let report = probe(&["err-0", "ok-5000"], config(200, 2000, 0, 3)).await;
        assert_eq!(report.chosen, None);
        assert!(started.elapsed() < Duration::from_millis(1500));
        let failures = report.failures();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].reason, "connection refused");
        assert_eq!(failures[0].target, "err-0:5432");
        assert!(
            failures[1].reason.starts_with("timed out after 0.2"),
            "{failures:?}"
        );
    }

    #[tokio::test]
    async fn the_budget_caps_the_phase_but_keeps_an_answer_already_in() {
        // #0 never finishes within the budget; #1 answered: take #1.
        let report = probe(&["ok-900", "ok-0"], config(5000, 200, 0, 3)).await;
        assert_eq!(report.chosen, Some(1));
        let pending = &report.attempts[0].1;
        assert!(
            matches!(pending, ProbeOutcome::Failed(r) if r.contains("no answer within 0.2")),
            "{pending:?}"
        );
    }

    #[tokio::test]
    async fn no_candidates_means_no_choice() {
        assert_eq!(probe(&[], ProbeConfig::default()).await.chosen, None);
    }

    #[tokio::test]
    async fn real_tcp_probe_prefers_the_listening_port() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let open = listener.local_addr().unwrap().port();
        // A port that is certainly closed: bind and drop another listener.
        let closed = {
            let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            l.local_addr().unwrap().port()
        };
        let candidates = vec![
            ReachCandidate::new(RouteKind::PublishedPort, "127.0.0.1", closed),
            ReachCandidate::new(RouteKind::ContainerIp, "127.0.0.1", open),
        ];
        let report = probe_candidates(&candidates, &ProbeConfig::default(), tcp_connect).await;
        assert_eq!(report.chosen, Some(1));
        assert_eq!(report.failures()[0].reason, "connection refused");
    }

    #[rstest]
    #[case(&[Some(true)], Pick::Chosen(0))]
    #[case(&[Some(false), Some(true), None], Pick::Chosen(1))]
    #[case(&[None, Some(true)], Pick::Wait)]
    #[case(&[Some(false), None], Pick::Wait)]
    #[case(&[Some(false), Some(false)], Pick::NoneReachable)]
    #[case(&[], Pick::NoneReachable)]
    fn picking_by_priority(#[case] results: &[Option<bool>], #[case] expected: Pick) {
        assert_eq!(pick(results), expected);
    }

    // -- error classification and copy -----------------------------------------

    #[rstest]
    #[case(
        "Failed to create database client: error communicating with database: Connection refused (os error 61)",
        true
    )]
    #[case(
        "error communicating with database: Connection reset by peer (os error 54)",
        true
    )]
    #[case("error communicating with database: unexpected end of file", true)]
    #[case("pool timed out while waiting for an open connection", true)]
    #[case("operation timed out", true)]
    #[case(
        "error returned from database: password authentication failed for user \"app\"",
        false
    )]
    #[case(
        "error returned from database: database \"nope\" does not exist",
        false
    )]
    #[case("error returned from database: role \"app\" does not exist", false)]
    #[case(
        "Access denied for user 'root'@'172.18.0.1' (using password: YES)",
        false
    )]
    #[case(
        "error occurred while attempting to establish a TLS connection: invalid peer certificate",
        false
    )]
    #[case("received fatal alert: handshake_failure (ssl)", false)]
    #[case("some unrelated failure", false)]
    fn transport_failures_are_told_from_server_answers(#[case] message: &str, #[case] retry: bool) {
        assert_eq!(is_transport_failure(message), retry);
    }

    fn unreachable(published: Option<&str>, attempts: Vec<RouteAttempt>) -> Unreachable {
        Unreachable {
            container: "tt2-postgres-1".into(),
            port: 5432,
            published: published.map(str::to_string),
            attempts,
        }
    }

    fn attempt(route: RouteKind, target: &str, reason: &str) -> RouteAttempt {
        RouteAttempt {
            route,
            target: target.into(),
            reason: reason.into(),
        }
    }

    #[test]
    fn unreachable_message_matches_the_agreed_copy() {
        let error = unreachable(
            None,
            vec![
                attempt(
                    RouteKind::ContainerIp,
                    "192.168.107.3:5432",
                    "timed out after 1.5 s",
                ),
                attempt(
                    RouteKind::ExecTunnel,
                    "docker exec",
                    "this image has no supported forwarding helper (needs socat, perl or nc)",
                ),
            ],
        );
        assert_eq!(error.headline(), "Couldn't reach 'tt2-postgres-1'");
        assert_eq!(
            error.explanation(),
            "Port 5432 isn't published, and the container address didn't respond. \
             A VPN may be blocking that route. \
             This image has no supported forwarding helper (needs socat, perl or nc)."
        );
        assert_eq!(error.compose_example(), r#"ports: ["127.0.0.1::5432"]"#);
        let text = error.to_string();
        assert!(
            text.starts_with("Couldn't reach 'tt2-postgres-1'.\nPort 5432 isn't published"),
            "{text}"
        );
        assert!(
            text.contains("  - 192.168.107.3:5432: timed out after 1.5 s"),
            "{text}"
        );
        assert!(
            text.contains("  - docker exec: this image has no supported"),
            "{text}"
        );
        assert!(text.contains(r#"ports: ["127.0.0.1::5432"]"#), "{text}");
        assert!(text.contains("recreates the container"), "{text}");
        // Not the primary suggestion.
        assert!(
            !text.to_lowercase().contains("disconnect the vpn"),
            "{text}"
        );
    }

    #[test]
    fn unreachable_message_without_direct_attempts_or_with_a_published_port() {
        let docker_desktop = unreachable(
            None,
            vec![attempt(
                RouteKind::ExecTunnel,
                "docker exec",
                "Docker exec failed: boom",
            )],
        );
        assert!(
            docker_desktop
                .explanation()
                .contains("the container address can't be reached from this computer"),
            "{}",
            docker_desktop.explanation()
        );
        assert!(!docker_desktop.explanation().contains("VPN"));

        let published = unreachable(Some("127.0.0.1:32768"), Vec::new());
        assert_eq!(
            published.explanation(),
            "Port 5432 is published on 127.0.0.1:32768, but nothing answered there, \
             and the container address can't be reached from this computer."
        );
    }

    #[test]
    fn phases_have_labels() {
        assert_eq!(
            DockerPhase::CheckingDirectAccess.label(),
            "Checking direct access"
        );
        assert_eq!(RouteKind::ExecTunnel.label(), "Through Docker");
    }

    use std::sync::Arc;
}
