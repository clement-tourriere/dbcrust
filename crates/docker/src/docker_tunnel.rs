//! Reach a TCP port inside a container through the Docker API alone.
//!
//! When a container's database port is not published and its network address
//! cannot be routed from the host (a full-tunnel VPN, Docker Desktop, a remote
//! daemon...), the only path that always works is the one `docker exec` uses.
//! A [`DockerTunnel`] listens on `127.0.0.1:<ephemeral>`; every accepted TCP
//! connection becomes one exec running a small byte relay (`socat`, `perl` or
//! `nc`) towards the database port *inside* the container, with the exec's
//! stdin/stdout attached to the local socket. No image pull, no extra
//! container, no published port, no package install.
//!
//! * The relay targets the container's own network address (IPv4, else
//!   IPv6), so the server applies the same authentication rules as for any
//!   other client (the official postgres image trusts `127.0.0.1`!). There
//!   is deliberately no loopback fallback: a container without a usable
//!   address (host or `none` network mode) is an error, not a way around
//!   password authentication.
//! * One exec per accepted socket, at most [`MAX_RELAYS`] at a time.
//! * Client EOF half-closes the relay's write side while the reply drains;
//!   the relay ending (the database closed the connection) closes the client.
//! * The tunnel lives exactly as long as the value that owns it (the
//!   `Database` stores an `Arc<DockerTunnel>`): dropping it aborts the accept
//!   loop, which drops its `JoinSet`, which aborts every relay task, which
//!   drops every exec stream. The listener is on loopback only, so other
//!   local processes can reach it while it lives (like a published
//!   `127.0.0.1` port); the database still authenticates them.

use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures_util::{Stream, StreamExt};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, warn};

/// How long the capability probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
/// How long creating and starting one relay exec may take.
const EXEC_SETUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Read buffer for exec output: the default (8 KiB) throttles bulk results.
const EXEC_OUTPUT_CAPACITY: usize = 64 * 1024;
/// Relays (= database sockets) open at once through one tunnel.
pub const MAX_RELAYS: usize = 32;
/// Stderr kept for diagnostics, per relay.
const STDERR_LIMIT: usize = 1024;

/// Seconds `socat` waits for the reply after the client half-closed.
const SOCAT_DRAIN_SECS: &str = "30";

/// The byte-relay program, passed with its address and port as `argv`.
const PERL_RELAY: &str = include_str!("docker_relay.pl");

/// Why a tunnel could not be set up (or why one connection through it failed).
#[derive(Error, Debug)]
pub enum DockerTunnelError {
    /// `sh` itself could not run: distroless/scratch image, or exec refused.
    #[error("the container has no shell, so no forwarding helper can be looked up ({0})")]
    NoShell(String),
    /// A shell exists but none of the supported helpers does.
    #[error("this image has no supported forwarding helper (needs socat, perl or nc)")]
    NoHelper,
    /// The container has no network address to relay to.
    #[error(
        "the container has no usable network address to forward to \
         (is it using host or none network mode?)"
    )]
    NoContainerAddress,
    /// The Docker API rejected or failed an exec call.
    #[error("Docker exec failed: {0}")]
    Exec(#[from] bollard::errors::Error),
    /// Could not open the local listener.
    #[error("could not open a local listener: {0}")]
    Listen(#[source] io::Error),
    /// The daemon did not answer in time.
    #[error("Docker did not answer within {0} s")]
    Timeout(u64),
    /// Docker returned something that is not an attachable exec stream.
    #[error("Docker did not attach the exec session")]
    NotAttached,
    /// Copying bytes between the client and the exec failed.
    #[error("tunnel connection failed: {0}")]
    Io(#[from] io::Error),
}

/// Where the relay connects *inside* the container: the container's own
/// address, never loopback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayTarget {
    pub addr: IpAddr,
    pub port: u16,
}

impl RelayTarget {
    /// The container's address: IPv4 if it has one, else global IPv6. A
    /// container with neither (host/none network mode, link-local only) is
    /// refused rather than reached over loopback, where some images skip
    /// password authentication.
    pub fn for_container(
        ipv4: Option<&str>,
        ipv6: Option<&str>,
        port: u16,
    ) -> Result<Self, DockerTunnelError> {
        let v4 = ipv4
            .and_then(|ip| ip.trim().parse::<Ipv4Addr>().ok())
            .filter(|ip| !ip.is_unspecified() && !ip.is_loopback() && !ip.is_link_local());
        let v6 = ipv6
            .and_then(|ip| ip.trim().parse::<Ipv6Addr>().ok())
            .filter(|ip| {
                !ip.is_unspecified() && !ip.is_loopback() && (ip.segments()[0] & 0xffc0) != 0xfe80
            });
        let addr = v4
            .map(IpAddr::V4)
            .or(v6.map(IpAddr::V6))
            .ok_or(DockerTunnelError::NoContainerAddress)?;
        Ok(Self { addr, port })
    }

    pub fn describe(&self) -> String {
        match self.addr {
            IpAddr::V4(addr) => format!("{addr}:{}", self.port),
            IpAddr::V6(addr) => format!("[{addr}]:{}", self.port),
        }
    }

    pub fn is_ipv6(&self) -> bool {
        self.addr.is_ipv6()
    }
}

/// Which `nc` is installed: only variants with well-defined EOF behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcFlavor {
    /// Half-closes on stdin EOF and drains.
    Busybox,
    /// Needs `-N` to shut down the socket on stdin EOF.
    OpenBsd,
    /// Half-closes by default.
    Ncat,
}

/// The program run inside the container, in order of preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayHelper {
    /// `socat - TCP:ADDR:PORT`
    Socat,
    /// A small `perl` relay (core modules only): present in the official
    /// Debian-based database images, where `socat`/`nc` usually are not.
    Perl,
    /// A supported `nc`.
    Netcat(NcFlavor),
}

impl RelayHelper {
    pub fn name(self) -> &'static str {
        match self {
            RelayHelper::Socat => "socat",
            RelayHelper::Perl => "perl",
            RelayHelper::Netcat(_) => "nc",
        }
    }

    /// The exec command line forwarding stdin/stdout to `target`. Address and
    /// port are separate `argv` entries (typed, never code), the script is a
    /// constant.
    pub fn argv(self, target: &RelayTarget) -> Vec<String> {
        let (addr, port) = (target.addr.to_string(), target.port.to_string());
        match self {
            // `-t`: how long socat keeps draining the reply after the client
            // half-closed (its default is 0.5 s, which cuts off any server
            // that answers after reading to EOF)
            RelayHelper::Socat => {
                let endpoint = if target.is_ipv6() {
                    format!("TCP6:[{addr}]:{port}")
                } else {
                    format!("TCP:{addr}:{port}")
                };
                vec![
                    "socat".into(),
                    "-t".into(),
                    SOCAT_DRAIN_SECS.into(),
                    "-".into(),
                    endpoint,
                ]
            }
            RelayHelper::Perl => vec!["perl".into(), "-e".into(), PERL_RELAY.into(), addr, port],
            RelayHelper::Netcat(NcFlavor::OpenBsd) => {
                vec!["nc".into(), "-N".into(), addr, port]
            }
            RelayHelper::Netcat(_) => vec!["nc".into(), addr, port],
        }
    }
}

/// Shell snippet run through `sh -c` that prints one token per usable helper:
/// `socat`, `perl` (IPv4), `perl:ip` (IPv4 and IPv6), `nc:busybox`,
/// `nc:openbsd`, `nc:ncat`.
const PROBE_SCRIPT: &str = r#"
command -v socat >/dev/null 2>&1 && echo socat
if command -v perl >/dev/null 2>&1 && perl -MIO::Socket::INET -MIO::Select -e1 >/dev/null 2>&1; then
  echo perl
  perl -MIO::Socket::IP -e1 >/dev/null 2>&1 && echo perl:ip
fi
if command -v nc >/dev/null 2>&1; then
  h=$(nc -h 2>&1 </dev/null | head -n 5)
  case "$h" in
    *[Bb]usy[Bb]ox*) echo nc:busybox ;;
    *[Oo]pen[Bb][Ss][Dd]*) echo nc:openbsd ;;
    *[Nn]cat*) echo nc:ncat ;;
  esac
fi
exit 0
"#;

/// Pick the best helper from the probe's stdout (one token per line) for a
/// target of the given address family.
pub fn choose_helper(probe_stdout: &str, ipv6: bool) -> Option<RelayHelper> {
    let has = |token: &str| probe_stdout.lines().any(|line| line.trim() == token);
    if has("socat") {
        Some(RelayHelper::Socat)
    } else if has("perl:ip") || (has("perl") && !ipv6) {
        Some(RelayHelper::Perl)
    } else if has("nc:busybox") {
        Some(RelayHelper::Netcat(NcFlavor::Busybox))
    } else if has("nc:openbsd") {
        Some(RelayHelper::Netcat(NcFlavor::OpenBsd))
    } else if has("nc:ncat") {
        Some(RelayHelper::Netcat(NcFlavor::Ncat))
    } else {
        None
    }
}

/// State shared between the tunnel handle and its relay tasks.
#[derive(Debug, Default)]
struct Shared {
    /// Last line a relay wrote to stderr (e.g. `connection refused`).
    last_error: Mutex<Option<String>>,
}

impl Shared {
    fn record_stderr(&self, bytes: &[u8]) {
        let text = sanitize_diagnostic(bytes);
        let Some(line) = text.lines().rev().map(str::trim).find(|l| !l.is_empty()) else {
            return;
        };
        debug!("[docker tunnel] relay stderr: {line}");
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(line.to_string());
        }
    }
}

/// Printable, bounded text from relay stderr: it may be arbitrary bytes.
fn sanitize_diagnostic(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(STDERR_LIMIT)])
        .chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

/// A local `127.0.0.1:<port>` endpoint forwarding to a port inside a container.
#[derive(Debug)]
pub struct DockerTunnel {
    local_addr: SocketAddr,
    helper: RelayHelper,
    container: String,
    target: RelayTarget,
    shared: Arc<Shared>,
    accept_task: JoinHandle<()>,
}

impl DockerTunnel {
    /// Probe the container for a helper and start listening.
    ///
    /// Must run inside a tokio runtime (the accept loop is spawned on it and
    /// keeps running as long as that runtime is driven).
    pub async fn open(
        docker: Docker,
        container: &str,
        target: RelayTarget,
    ) -> Result<Self, DockerTunnelError> {
        let helper = tokio::time::timeout(
            PROBE_TIMEOUT,
            detect_helper(&docker, container, target.is_ipv6()),
        )
        .await
        .map_err(|_| DockerTunnelError::Timeout(PROBE_TIMEOUT.as_secs()))??;
        Self::open_with_helper(docker, container, target, helper).await
    }

    /// Like [`DockerTunnel::open`] with the helper already chosen.
    pub async fn open_with_helper(
        docker: Docker,
        container: &str,
        target: RelayTarget,
        helper: RelayHelper,
    ) -> Result<Self, DockerTunnelError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(DockerTunnelError::Listen)?;
        let local_addr = listener.local_addr().map_err(DockerTunnelError::Listen)?;
        let shared = Arc::new(Shared::default());
        let accept_task = tokio::spawn(accept_loop(
            listener,
            docker,
            container.to_string(),
            helper.argv(&target),
            Arc::clone(&shared),
        ));
        debug!(
            "[docker tunnel] {local_addr} -> {container} {} via {}",
            target.describe(),
            helper.name()
        );
        Ok(Self {
            local_addr,
            helper,
            container: container.to_string(),
            target,
            shared,
            accept_task,
        })
    }

    /// The loopback port clients should connect to.
    pub fn local_port(&self) -> u16 {
        self.local_addr.port()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn helper(&self) -> RelayHelper {
        self.helper
    }

    pub fn target(&self) -> RelayTarget {
        self.target
    }

    /// Human-readable description for route details.
    pub fn describe(&self) -> String {
        format!(
            "local forwarding on {} through `docker exec` ({} relay to {})",
            self.local_addr,
            self.helper.name(),
            self.target.describe()
        )
    }

    /// The last error line a relay printed, if any: the best hint when a
    /// connection through the tunnel is closed immediately (nothing is
    /// listening on the port inside the container, for instance).
    pub fn last_error(&self) -> Option<String> {
        self.shared.last_error.lock().ok().and_then(|e| e.clone())
    }

    /// The container this tunnel enters.
    pub fn container(&self) -> &str {
        &self.container
    }
}

impl Drop for DockerTunnel {
    fn drop(&mut self) {
        // Dropping the accept loop drops its `JoinSet`, which aborts every
        // relay task, which drops every exec stream.
        self.accept_task.abort();
    }
}

/// Run the capability probe exec and choose a helper.
async fn detect_helper(
    docker: &Docker,
    container: &str,
    ipv6: bool,
) -> Result<RelayHelper, DockerTunnelError> {
    let exec = docker
        .create_exec(
            container,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                cmd: Some(vec!["sh".to_string(), "-c".into(), PROBE_SCRIPT.into()]),
                ..Default::default()
            },
        )
        .await?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    match docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                tty: false,
                output_capacity: None,
            }),
        )
        .await
    {
        Ok(StartExecResults::Attached { mut output, .. }) => {
            while let Some(frame) = output.next().await {
                match frame? {
                    LogOutput::StdOut { message } | LogOutput::Console { message } => {
                        stdout.extend_from_slice(&message)
                    }
                    LogOutput::StdErr { message } => stderr.extend_from_slice(&message),
                    LogOutput::StdIn { .. } => {}
                }
            }
        }
        Ok(StartExecResults::Detached) => return Err(DockerTunnelError::NotAttached),
        // The daemon refuses to start an exec whose binary does not exist.
        Err(e) => return Err(DockerTunnelError::NoShell(e.to_string())),
    }

    let stdout = String::from_utf8_lossy(&stdout);
    if let Some(helper) = choose_helper(&stdout, ipv6) {
        return Ok(helper);
    }
    let stderr = sanitize_diagnostic(&stderr);
    if stdout.trim().is_empty() && !stderr.trim().is_empty() {
        // `sh` could not run (OCI error text arrives as stderr).
        let first = stderr.lines().next().unwrap_or("").trim().to_string();
        return Err(DockerTunnelError::NoShell(first));
    }
    Err(DockerTunnelError::NoHelper)
}

async fn accept_loop(
    listener: TcpListener,
    docker: Docker,
    container: String,
    argv: Vec<String>,
    shared: Arc<Shared>,
) {
    let permits = Arc::new(Semaphore::new(MAX_RELAYS));
    let mut relays = JoinSet::new();
    loop {
        // Reap finished relays so the set doesn't grow with every connection.
        while relays.try_join_next().is_some() {}
        // Backpressure: past the cap, new sockets wait in the listen backlog.
        let Ok(permit) = Arc::clone(&permits).acquire_owned().await else {
            return;
        };
        match listener.accept().await {
            Ok((stream, peer)) => {
                debug!("[docker tunnel] accepted {peer}");
                // Latency matters for request/response database protocols.
                let _ = stream.set_nodelay(true);
                let docker = docker.clone();
                let container = container.clone();
                let argv = argv.clone();
                let shared = Arc::clone(&shared);
                relays.spawn(async move {
                    let _permit = permit;
                    if let Err(e) =
                        serve_connection(docker, &container, argv, stream, &shared).await
                    {
                        warn!("[docker tunnel] relay to {container} ended: {e}");
                    }
                });
            }
            Err(e) => {
                // EMFILE and friends: back off instead of spinning.
                warn!("[docker tunnel] accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// One accepted client: start an attached exec and pump bytes both ways.
async fn serve_connection(
    docker: Docker,
    container: &str,
    argv: Vec<String>,
    client: TcpStream,
    shared: &Shared,
) -> Result<(), DockerTunnelError> {
    let setup = async {
        let exec = docker
            .create_exec(
                container,
                CreateExecOptions {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    // A tty would translate bytes and merge stderr into stdout.
                    tty: Some(false),
                    cmd: Some(argv),
                    ..Default::default()
                },
            )
            .await?;
        docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: false,
                    output_capacity: Some(EXEC_OUTPUT_CAPACITY),
                }),
            )
            .await
    };
    let started = tokio::time::timeout(EXEC_SETUP_TIMEOUT, setup)
        .await
        .map_err(|_| DockerTunnelError::Timeout(EXEC_SETUP_TIMEOUT.as_secs()))??;
    let StartExecResults::Attached { output, input } = started else {
        return Err(DockerTunnelError::NotAttached);
    };

    pump(client, input, output, |bytes| shared.record_stderr(bytes))
        .await
        .map_err(DockerTunnelError::Io)
}

/// Copy bytes between a client socket and an attached exec until either side
/// is done.
///
/// * client EOF (half-close) closes the exec's stdin, then the exec's answer
///   keeps flowing back until the process ends;
/// * the exec ending (database closed the connection, relay exited) closes
///   the client socket and cancels the upload;
/// * stderr frames never reach the client; they go to `on_stderr`.
async fn pump<C, I, S, E>(
    client: C,
    mut exec_stdin: I,
    mut exec_output: S,
    on_stderr: impl Fn(&[u8]),
) -> io::Result<()>
where
    C: tokio::io::AsyncRead + AsyncWrite + Unpin,
    I: AsyncWrite + Unpin,
    S: Stream<Item = Result<LogOutput, E>> + Unpin,
    E: std::fmt::Display,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);

    let upload = async {
        tokio::io::copy(&mut client_read, &mut exec_stdin).await?;
        exec_stdin.shutdown().await
    };
    let download = async {
        while let Some(frame) = exec_output.next().await {
            match frame.map_err(|e| io::Error::other(e.to_string()))? {
                LogOutput::StdOut { message } | LogOutput::Console { message } => {
                    client_write.write_all(&message).await?;
                }
                LogOutput::StdErr { message } => on_stderr(&message),
                LogOutput::StdIn { .. } => {}
            }
        }
        client_write.shutdown().await
    };
    tokio::pin!(upload, download);

    tokio::select! {
        uploaded = &mut upload => {
            uploaded?;
            // The client is done sending; let the reply drain.
            download.await
        }
        downloaded = &mut download => downloaded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use tokio::io::{AsyncReadExt, DuplexStream};

    #[rstest]
    #[case("socat\nperl\nperl:ip\nnc:busybox\n", false, Some(RelayHelper::Socat))]
    #[case("perl\nperl:ip\nnc:busybox\n", false, Some(RelayHelper::Perl))]
    #[case("perl\nnc:busybox\n", false, Some(RelayHelper::Perl))]
    // IPv6 needs IO::Socket::IP: plain perl can't, nc can
    #[case("perl\nperl:ip\n", true, Some(RelayHelper::Perl))]
    #[case(
        "perl\nnc:busybox\n",
        true,
        Some(RelayHelper::Netcat(NcFlavor::Busybox))
    )]
    #[case("perl\n", true, None)]
    #[case("nc:busybox\n", false, Some(RelayHelper::Netcat(NcFlavor::Busybox)))]
    #[case("nc:openbsd\n", false, Some(RelayHelper::Netcat(NcFlavor::OpenBsd)))]
    #[case("nc:ncat\n", false, Some(RelayHelper::Netcat(NcFlavor::Ncat)))]
    #[case(
        "nc:busybox\nnc:ncat\n",
        false,
        Some(RelayHelper::Netcat(NcFlavor::Busybox))
    )]
    #[case("  socat \r\n", true, Some(RelayHelper::Socat))]
    // bash alone is deliberately not a helper
    #[case("bash\n", false, None)]
    // unsupported nc variants print nothing
    #[case("", false, None)]
    #[case("ncat\nperl5\nsocat2\n", false, None)]
    fn picks_the_best_available_helper(
        #[case] probe: &str,
        #[case] ipv6: bool,
        #[case] expected: Option<RelayHelper>,
    ) {
        assert_eq!(choose_helper(probe, ipv6), expected);
    }

    fn target(ip: &str) -> RelayTarget {
        RelayTarget::for_container(Some(ip), None, 5432).expect("address")
    }

    #[test]
    fn helpers_relay_to_the_container_address_with_validated_argv() {
        let target = target("172.18.0.3");
        assert_eq!(
            RelayHelper::Socat.argv(&target),
            ["socat", "-t", "30", "-", "TCP:172.18.0.3:5432"]
        );
        assert_eq!(
            RelayHelper::Netcat(NcFlavor::Busybox).argv(&target),
            ["nc", "172.18.0.3", "5432"]
        );
        assert_eq!(
            RelayHelper::Netcat(NcFlavor::OpenBsd).argv(&target),
            ["nc", "-N", "172.18.0.3", "5432"]
        );
        let perl = RelayHelper::Perl.argv(&target);
        assert_eq!(&perl[..2], ["perl", "-e"]);
        // The script is a constant; address and port travel as argv.
        assert_eq!(&perl[3..], ["172.18.0.3", "5432"]);
        assert!(!perl[2].contains("172.18.0.3") && !perl[2].contains("5432"));
        assert!(perl[2].contains("shutdown($sock, 1)"));
        assert!(perl[2].contains("IO::Socket::IP"));
    }

    #[test]
    fn ipv6_targets_use_the_right_address_syntax() {
        let target = RelayTarget::for_container(None, Some("fd00:dead:beef::2"), 5432).unwrap();
        assert!(target.is_ipv6());
        assert_eq!(target.describe(), "[fd00:dead:beef::2]:5432");
        assert_eq!(
            RelayHelper::Socat.argv(&target),
            ["socat", "-t", "30", "-", "TCP6:[fd00:dead:beef::2]:5432"]
        );
        assert_eq!(
            RelayHelper::Netcat(NcFlavor::OpenBsd).argv(&target),
            ["nc", "-N", "fd00:dead:beef::2", "5432"]
        );
        let perl = RelayHelper::Perl.argv(&target);
        assert_eq!(&perl[3..], ["fd00:dead:beef::2", "5432"]);
    }

    #[rstest]
    #[case(Some("192.168.107.3"), None, "192.168.107.3")]
    #[case(Some(" 10.0.0.9 "), None, "10.0.0.9")]
    // IPv4 wins when both exist
    #[case(Some("10.0.0.9"), Some("fd00::5"), "10.0.0.9")]
    #[case(None, Some("fd00::5"), "fd00::5")]
    #[case(Some(""), Some("2001:db8::7"), "2001:db8::7")]
    fn relay_target_is_the_container_address(
        #[case] v4: Option<&str>,
        #[case] v6: Option<&str>,
        #[case] expected: &str,
    ) {
        let target = RelayTarget::for_container(v4, v6, 3306).unwrap();
        assert_eq!(target.addr.to_string(), expected);
        assert_eq!(target.port, 3306);
    }

    /// No address is an error: loopback would bypass the password rules some
    /// images only apply to non-loopback clients.
    #[rstest]
    #[case(None, None)]
    #[case(Some(""), Some(""))]
    #[case(Some("0.0.0.0"), None)]
    #[case(Some("127.0.0.1"), Some("::1"))]
    #[case(Some("169.254.1.1"), None)]
    #[case(None, Some("fe80::1"))]
    #[case(Some("not-an-ip"), Some("::"))]
    fn a_container_without_a_usable_address_is_refused(
        #[case] v4: Option<&str>,
        #[case] v6: Option<&str>,
    ) {
        let error = RelayTarget::for_container(v4, v6, 5432).unwrap_err();
        assert!(matches!(error, DockerTunnelError::NoContainerAddress));
        assert!(error.to_string().contains("no usable network address"));
    }

    /// An exec that echoes stdin to stdout and exits at stdin EOF, as a
    /// stand-in for `socat - TCP:...` against an echo server.
    fn fake_exec(
        reply_prefix: &'static [u8],
    ) -> (
        DuplexStream,
        impl Stream<Item = Result<LogOutput, io::Error>> + Unpin,
    ) {
        let (stdin_write, mut stdin_read) = tokio::io::duplex(16 * 1024);
        let (mut stdout_write, stdout_read) = tokio::io::duplex(16 * 1024);
        tokio::spawn(async move {
            let _ = stdout_write.write_all(reply_prefix).await;
            let _ = tokio::io::copy(&mut stdin_read, &mut stdout_write).await;
            let _ = stdout_write.shutdown().await;
        });
        let frames = futures_util::stream::unfold(stdout_read, |mut reader| async move {
            let mut chunk = vec![0u8; 4096];
            match reader.read(&mut chunk).await {
                Ok(0) => None,
                Ok(n) => {
                    chunk.truncate(n);
                    let frame = LogOutput::StdOut {
                        message: chunk.into(),
                    };
                    Some((Ok(frame), reader))
                }
                Err(e) => Some((Err(e), reader)),
            }
        });
        (stdin_write, Box::pin(frames))
    }

    #[tokio::test]
    async fn pump_relays_bytes_both_ways_and_honours_half_close() {
        let (mut app, tunnel_side) = tokio::io::duplex(64 * 1024);
        let (exec_stdin, exec_output) = fake_exec(b"hello:");
        let pumped = tokio::spawn(pump(tunnel_side, exec_stdin, exec_output, |_| {}));

        let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let to_send = payload.clone();
        let (mut app_read, mut app_write) = tokio::io::split(&mut app);
        let writer = async {
            app_write.write_all(&to_send).await.unwrap();
            // Half-close: the pipe must still deliver the whole reply.
            app_write.shutdown().await.unwrap();
        };
        let reader = async {
            let mut received = Vec::new();
            app_read.read_to_end(&mut received).await.unwrap();
            received
        };
        let (_, received) = tokio::join!(writer, reader);

        assert_eq!(&received[..6], b"hello:");
        assert_eq!(&received[6..], &payload[..]);
        assert!(pumped.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn pump_ends_when_the_exec_ends_and_keeps_stderr_out_of_the_stream() {
        let (mut app, tunnel_side) = tokio::io::duplex(1024);
        let (exec_stdin, _unused) = tokio::io::duplex(1024);
        let frames = futures_util::stream::iter(vec![
            Ok::<_, io::Error>(LogOutput::StdErr {
                message: "nc: connection refused\n".into(),
            }),
            Ok(LogOutput::StdOut {
                message: "bye".into(),
            }),
        ]);
        let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = Arc::clone(&seen);
        let result = pump(tunnel_side, exec_stdin, frames, move |bytes| {
            sink.lock().unwrap().extend_from_slice(bytes)
        })
        .await;
        assert!(result.is_ok());

        let mut received = Vec::new();
        app.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, b"bye");
        assert_eq!(&*seen.lock().unwrap(), b"nc: connection refused\n");
    }

    #[test]
    fn stderr_diagnostics_are_bounded_and_printable() {
        let shared = Shared::default();
        shared.record_stderr(b"warning\nnc: connect to 10.0.0.2 port 5432 (tcp) failed\n\n");
        assert_eq!(
            shared.last_error.lock().unwrap().as_deref(),
            Some("nc: connect to 10.0.0.2 port 5432 (tcp) failed")
        );
        shared.record_stderr(b"  \n");
        assert!(shared.last_error.lock().unwrap().is_some());

        let noisy = sanitize_diagnostic(b"bad\x1b[31m\x00bytes\xff");
        assert!(!noisy.chars().any(|c| c.is_control()));
        assert!(sanitize_diagnostic(&vec![b'x'; 10_000]).len() <= STDERR_LIMIT);
    }

    #[test]
    fn error_messages_name_the_cause() {
        assert!(
            DockerTunnelError::NoHelper
                .to_string()
                .contains("socat, perl or nc")
        );
        assert!(
            DockerTunnelError::NoShell("exec: \"sh\": executable file not found".into())
                .to_string()
                .contains("executable file not found")
        );
    }
}

/// Tests against a real Docker daemon. Opt in with `DBCRUST_DOCKER_IT=1`:
///
/// ```text
/// DBCRUST_DOCKER_IT=1 cargo test --lib docker_tunnel::integration -- --test-threads=2
/// ```
///
/// They only use images already present locally (`alpine/socat`, `postgres:17`,
/// `alpine`), never pull, and always remove the containers they create.
#[cfg(test)]
mod integration {
    use super::*;
    use crate::docker::DockerClient;
    use bollard::models::ContainerCreateBody;
    use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn enabled() -> bool {
        std::env::var("DBCRUST_DOCKER_IT").is_ok_and(|v| v == "1")
    }

    struct Fixture {
        docker: Docker,
        id: String,
        ip: String,
        ipv6: String,
    }

    impl Fixture {
        fn target(&self, port: u16) -> RelayTarget {
            RelayTarget::for_container(Some(&self.ip), Some(&self.ipv6), port)
                .expect("container address")
        }

        /// Run `script` with `sh -c` inside the container; stdout as text.
        async fn sh(&self, script: &str) -> String {
            let exec = self
                .docker
                .create_exec(
                    &self.id,
                    CreateExecOptions {
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        cmd: Some(vec!["sh".to_string(), "-c".into(), script.into()]),
                        ..Default::default()
                    },
                )
                .await
                .expect("create exec");
            let mut text = String::new();
            if let Ok(StartExecResults::Attached { mut output, .. }) =
                self.docker.start_exec(&exec.id, None).await
            {
                while let Some(Ok(frame)) = output.next().await {
                    text.push_str(&frame.to_string());
                }
            }
            text
        }
    }

    /// Create and start a container from a local image, run `body` against
    /// it, and always remove it. Returns without running anything when the
    /// opt-in variable is unset, Docker is unavailable or the image is not
    /// present locally.
    async fn with_container<F, Fut>(
        image: &str,
        entrypoint: Option<Vec<String>>,
        cmd: Vec<String>,
        body: F,
    ) where
        F: FnOnce(Fixture) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        with_container_on(None, image, entrypoint, cmd, body).await;
    }

    /// [`with_container`] on an IPv6-only network created for the test.
    async fn with_container_on_ipv6_only<F, Fut>(
        image: &str,
        entrypoint: Option<Vec<String>>,
        cmd: Vec<String>,
        body: F,
    ) where
        F: FnOnce(Fixture) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        with_container_on(Some(Network::Ipv6Only), image, entrypoint, cmd, body).await;
    }

    enum Network {
        Ipv6Only,
    }

    async fn with_container_on<F, Fut>(
        network: Option<Network>,
        image: &str,
        entrypoint: Option<Vec<String>>,
        cmd: Vec<String>,
        body: F,
    ) where
        F: FnOnce(Fixture) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        if !enabled() {
            eprintln!("skipping {image} test: set DBCRUST_DOCKER_IT=1 to run it");
            return;
        }
        let docker = match DockerClient::new() {
            Ok(client) => client.bollard().clone(),
            Err(e) => {
                eprintln!("skipping: Docker unavailable ({e})");
                return;
            }
        };
        if docker.inspect_image(image).await.is_err() {
            eprintln!("skipping: image {image} is not present locally (never pulled)");
            return;
        }
        let name = format!(
            "dbcrust-it-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let network_name = match network {
            Some(Network::Ipv6Only) => {
                // A subnet per run: parallel runs and leftovers don't collide
                let subnet = format!(
                    "fd00:{:x}:{:x}::/64",
                    std::process::id() % 0xffff,
                    COUNTER.load(Ordering::SeqCst)
                );
                let request = bollard::models::NetworkCreateRequest {
                    name: format!("{name}-net"),
                    driver: Some("bridge".into()),
                    enable_ipv6: Some(true),
                    ipam: Some(bollard::models::Ipam {
                        config: Some(vec![bollard::models::IpamConfig {
                            subnet: Some(subnet),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }),
                    options: Some(
                        [(
                            "com.docker.network.enable_ipv4".to_string(),
                            "false".to_string(),
                        )]
                        .into(),
                    ),
                    ..Default::default()
                };
                match docker.create_network(request).await {
                    Ok(_) => Some(format!("{name}-net")),
                    Err(e) => {
                        eprintln!("skipping: this daemon can't make IPv6-only networks ({e})");
                        return;
                    }
                }
            }
            None => None,
        };
        let created = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(&name).build()),
                ContainerCreateBody {
                    image: Some(image.to_string()),
                    entrypoint,
                    cmd: Some(cmd),
                    host_config: network_name
                        .clone()
                        .map(|network| bollard::models::HostConfig {
                            network_mode: Some(network),
                            ..Default::default()
                        }),
                    ..Default::default()
                },
            )
            .await
            .expect("create container");
        let remove = || async {
            let options = RemoveContainerOptionsBuilder::new().force(true).build();
            let _ = docker.remove_container(&created.id, Some(options)).await;
            if let Some(network) = &network_name {
                let _ = docker.remove_network(network).await;
            }
        };
        if let Err(e) = docker.start_container(&created.id, None).await {
            remove().await;
            panic!("start container: {e}");
        }
        let networks = docker
            .inspect_container(&created.id, None)
            .await
            .ok()
            .and_then(|c| c.network_settings)
            .and_then(|n| n.networks)
            .unwrap_or_default();
        let address = |pick: fn(&bollard::models::EndpointSettings) -> &Option<String>| {
            networks
                .values()
                .find_map(|n| pick(n).clone().filter(|ip| !ip.is_empty()))
                .unwrap_or_default()
        };
        let fixture = Fixture {
            docker: docker.clone(),
            id: created.id.clone(),
            ip: address(|n| &n.ip_address),
            ipv6: address(|n| &n.global_ipv6_address),
        };
        // Leave time for the listener to come up.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let joined = tokio::spawn(body(fixture)).await;
        remove().await;
        if let Err(e) = joined {
            std::panic::resume_unwind(e.into_panic());
        }
    }

    /// Send `payload`, half-close, and read the whole reply.
    async fn roundtrip(port: u16, payload: &[u8]) -> Vec<u8> {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let (mut read, mut write) = stream.into_split();
        let send = async {
            write.write_all(payload).await.expect("write");
            write.shutdown().await.expect("half-close");
        };
        let receive = async {
            let mut reply = Vec::new();
            read.read_to_end(&mut reply).await.expect("read");
            reply
        };
        let (_, reply) = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(send, receive)
        })
        .await
        .expect("relay round trip timed out");
        reply
    }

    fn payload(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    /// Two servers in one `alpine/socat` container: an echo on 9000, and on
    /// 9001 one that answers a full second after the client half-closed.
    fn echo_entrypoint() -> Option<Vec<String>> {
        let script = "socat TCP-LISTEN:9000,reuseaddr,fork EXEC:cat & \
             socat -t 5 TCP-LISTEN:9001,reuseaddr,fork SYSTEM:'cat; sleep 1; echo LATE' & \
             wait";
        Some(vec!["sh".to_string(), "-c".into(), script.into()])
    }

    #[tokio::test]
    async fn socat_and_busybox_nc_relay_binary_payloads_with_half_close() {
        with_container(
            "alpine/socat:latest",
            echo_entrypoint(),
            Vec::new(),
            |fixture| async move {
                // The probe prefers socat in this image.
                let probed =
                    DockerTunnel::open(fixture.docker.clone(), &fixture.id, fixture.target(9000))
                        .await
                        .expect("tunnel");
                assert_eq!(probed.helper(), RelayHelper::Socat);
                drop(probed);

                for helper in [RelayHelper::Socat, RelayHelper::Netcat(NcFlavor::Busybox)] {
                    let tunnel = DockerTunnel::open_with_helper(
                        fixture.docker.clone(),
                        &fixture.id,
                        fixture.target(9000),
                        helper,
                    )
                    .await
                    .expect("tunnel");
                    let port = tunnel.local_port();

                    // One big binary exchange, then many at once (one exec each)
                    let big = payload(1 << 20, 7);
                    assert_eq!(roundtrip(port, &big).await, big, "{helper:?}");
                    let many = (0..24u32).map(|i| async move {
                        let data = payload(64 * 1024 + i as usize, i);
                        assert_eq!(roundtrip(port, &data).await, data, "connection {i}");
                    });
                    futures_util::future::join_all(many).await;

                    // The tunnel dies with its owner.
                    drop(tunnel);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    assert!(
                        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
                        "listener must close with the tunnel"
                    );
                }
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_reply_that_arrives_after_the_half_close_is_not_truncated() {
        with_container(
            "alpine/socat:latest",
            echo_entrypoint(),
            Vec::new(),
            |fixture| async move {
                for helper in [RelayHelper::Socat, RelayHelper::Netcat(NcFlavor::Busybox)] {
                    let tunnel = DockerTunnel::open_with_helper(
                        fixture.docker.clone(),
                        &fixture.id,
                        fixture.target(9001),
                        helper,
                    )
                    .await
                    .expect("tunnel");
                    let started = std::time::Instant::now();
                    let reply = roundtrip(tunnel.local_port(), b"question").await;
                    // Echoed at once, then the server waits 1 s (twice socat's
                    // default 0.5 s drain) before its last line.
                    assert_eq!(reply, b"questionLATE\n", "{helper:?}");
                    assert!(
                        started.elapsed() >= Duration::from_millis(900),
                        "{helper:?}"
                    );
                }
            },
        )
        .await;
    }

    #[tokio::test]
    async fn dropping_the_tunnel_ends_the_relay_processes() {
        with_container(
            "alpine/socat:latest",
            echo_entrypoint(),
            Vec::new(),
            |fixture| async move {
                let tunnel = DockerTunnel::open_with_helper(
                    fixture.docker.clone(),
                    &fixture.id,
                    fixture.target(9000),
                    RelayHelper::Netcat(NcFlavor::Busybox),
                )
                .await
                .expect("tunnel");
                // Hold three connections open without sending anything.
                let mut held = Vec::new();
                for _ in 0..3 {
                    held.push(
                        TcpStream::connect(("127.0.0.1", tunnel.local_port()))
                            .await
                            .expect("connect"),
                    );
                }
                let relays = || fixture.sh("ps | grep -c '[n]c [0-9]'");
                let mut running = 0;
                for _ in 0..30 {
                    running = relays().await.trim().parse::<u32>().unwrap_or(0);
                    if running == 3 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                assert_eq!(running, 3, "one relay exec per accepted socket");

                drop(tunnel);
                drop(held);
                let mut left = running;
                for _ in 0..50 {
                    left = relays().await.trim().parse::<u32>().unwrap_or(0);
                    if left == 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                assert_eq!(left, 0, "relay processes must exit with the tunnel");
            },
        )
        .await;
    }

    /// An echo server written in perl: replies `REPLY:` + everything the
    /// client sent, but only after the client half-closed.
    const PERL_SERVER: &str = r#"
use IO::Socket::IP;
$SIG{CHLD} = 'IGNORE';
my $l = IO::Socket::IP->new(Listen => 16, LocalPort => 9000, ReuseAddr => 1) or die $!;
while (my $c = $l->accept) {
    next if fork;
    binmode($c);
    local $/;
    my $d = <$c>;
    sleep 1; # answer well after the client half-closed
    print $c "REPLY:", $d;
    close $c;
    exit 0;
}
"#;

    #[tokio::test]
    async fn perl_relay_in_a_debian_image_without_socat_or_nc() {
        let entrypoint = Some(vec!["perl".to_string(), "-e".into(), PERL_SERVER.into()]);
        with_container(
            "postgres:17",
            entrypoint,
            Vec::new(),
            |fixture| async move {
                let tunnel =
                    DockerTunnel::open(fixture.docker.clone(), &fixture.id, fixture.target(9000))
                        .await
                        .expect("tunnel");
                assert_eq!(tunnel.helper(), RelayHelper::Perl);
                assert!(!tunnel.target().addr.is_loopback());
                let port = tunnel.local_port();

                let big = payload(2 << 20, 99);
                let started = std::time::Instant::now();
                let reply = roundtrip(port, &big).await;
                assert!(started.elapsed() >= Duration::from_millis(900));
                assert_eq!(&reply[..6], b"REPLY:");
                assert_eq!(&reply[6..], &big[..]);

                let many = (0..16u32).map(|i| async move {
                    let data = payload(10_000 + i as usize, i + 1000);
                    let reply = roundtrip(port, &data).await;
                    assert_eq!(&reply[6..], &data[..], "connection {i}");
                });
                futures_util::future::join_all(many).await;
            },
        )
        .await;
    }

    /// A container whose network has no IPv4 at all: the relay must go to
    /// its IPv6 address (and never fall back to loopback).
    #[tokio::test]
    async fn an_ipv6_only_container_is_relayed_over_ipv6() {
        let entrypoint = Some(vec!["perl".to_string(), "-e".into(), PERL_SERVER.into()]);
        with_container_on_ipv6_only(
            "postgres:17",
            entrypoint,
            Vec::new(),
            |fixture| async move {
                assert!(fixture.ip.is_empty(), "no IPv4: {:?}", fixture.ip);
                assert!(!fixture.ipv6.is_empty());
                let target = fixture.target(9000);
                assert!(target.is_ipv6());
                let tunnel = DockerTunnel::open(fixture.docker.clone(), &fixture.id, target)
                    .await
                    .expect("tunnel");
                assert_eq!(tunnel.helper(), RelayHelper::Perl);
                let data = payload(300_000, 5);
                let reply = roundtrip(tunnel.local_port(), &data).await;
                assert_eq!(&reply[..6], b"REPLY:");
                assert_eq!(&reply[6..], &data[..]);
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_refused_port_inside_the_container_closes_fast_with_the_reason() {
        with_container(
            "alpine/socat:latest",
            echo_entrypoint(),
            Vec::new(),
            |fixture| async move {
                let tunnel = DockerTunnel::open_with_helper(
                    fixture.docker.clone(),
                    &fixture.id,
                    fixture.target(9999), // nothing listens here
                    RelayHelper::Socat,
                )
                .await
                .expect("tunnel");
                let started = std::time::Instant::now();
                let reply = roundtrip(tunnel.local_port(), b"hello").await;
                assert!(reply.is_empty());
                assert!(started.elapsed() < Duration::from_secs(5));
                let reason = tunnel.last_error().unwrap_or_default();
                assert!(reason.to_lowercase().contains("refused"), "{reason:?}");
            },
        )
        .await;
    }

    #[tokio::test]
    async fn a_container_without_any_helper_fails_with_no_helper() {
        let sleep = vec!["sleep".to_string(), "120".into()];
        with_container("alpine:latest", None, sleep, |fixture| async move {
            fixture.sh("rm -f /usr/bin/nc /bin/nc").await;
            let error =
                DockerTunnel::open(fixture.docker.clone(), &fixture.id, fixture.target(5432))
                    .await
                    .expect_err("no helper left");
            assert!(matches!(error, DockerTunnelError::NoHelper), "{error}");
        })
        .await;
    }
}
