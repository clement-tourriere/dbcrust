//! The Unix-socket server: accepts connections, authenticates the peer,
//! performs the handshake and hands each request to the [`Gateway`].
//!
//! Conversation (see `crates/agent-protocol/PROTOCOL.md`): the first frame
//! must be `hello` (10 s deadline) and is answered with `welcome`, or with
//! `unsupported_version` followed by a close. After that the client sends one
//! request at a time and receives the answer with the same `id`.

use super::BrokerError;
use super::gateway::{Gateway, RequestContext};
use super::paths::{BrokerMode, BrokerSettings};
use super::privileges::group_id;
use dbcrust_agent_protocol::aio::{read_frame, write_message};
use dbcrust_agent_protocol::{
    ClientMessage, ErrorCode, FrameError, PROTOCOL_VERSION, ServerMessage, decode,
    versions_compatible,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_MAX_CONNECTIONS: usize = 32;
pub const MAX_CLIENT_LABEL_CHARS: usize = 128;

/// Free-form client labels are audit-only: strip control characters and cap
/// the length so a label cannot forge log lines.
pub fn sanitize_label(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_CLIENT_LABEL_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Peer credentials taken from the socket (`SO_PEERCRED`/`LOCAL_PEERCRED`).
#[derive(Debug, Clone, Copy)]
pub struct Peer {
    pub uid: u32,
    pub gid: u32,
    pub pid: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub mode: BrokerMode,
    /// The uid the broker runs as (user mode accepts only this uid).
    pub own_uid: u32,
    pub settings: BrokerSettings,
    pub max_connections: usize,
}

impl ServerConfig {
    fn admits(&self, peer: &Peer) -> bool {
        match self.mode {
            BrokerMode::User => peer.uid == self.own_uid,
            BrokerMode::Service => self.settings.peer_allowed(peer.uid, peer.gid),
        }
    }
}

pub struct Server {
    gateway: Arc<Gateway>,
    config: ServerConfig,
    permits: Arc<Semaphore>,
}

impl Server {
    pub fn new(gateway: Arc<Gateway>, config: ServerConfig) -> Arc<Self> {
        let permits = Arc::new(Semaphore::new(config.max_connections.max(1)));
        Arc::new(Self {
            gateway,
            config,
            permits,
        })
    }

    /// Accept connections until `shutdown` resolves.
    pub async fn serve(
        self: &Arc<Self>,
        listener: UnixListener,
        shutdown: impl std::future::Future<Output = ()>,
    ) {
        tokio::pin!(shutdown);
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => {
                    let stream = match accepted {
                        Ok((stream, _)) => stream,
                        Err(e) => {
                            tracing::warn!("accept failed: {e}");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            continue;
                        }
                    };
                    let server = self.clone();
                    tasks.spawn(async move { server.handle_connection(stream).await });
                    // Reap finished tasks so the set stays small
                    while tasks.try_join_next().is_some() {}
                }
            }
        }
        tasks.shutdown().await;
    }

    async fn handle_connection(self: Arc<Self>, mut stream: UnixStream) {
        let peer = match stream.peer_cred() {
            Ok(cred) => Peer {
                uid: cred.uid(),
                gid: cred.gid(),
                pid: cred.pid(),
            },
            Err(e) => {
                tracing::warn!("could not read peer credentials, closing: {e}");
                return;
            }
        };
        if !self.config.admits(&peer) {
            // Not even a protocol error: a refused peer learns nothing
            tracing::warn!(
                "refused connection from uid {} (pid {:?}): not allowed",
                peer.uid,
                peer.pid
            );
            return;
        }
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            let _ = write_message(
                &mut stream,
                &ServerMessage::Error {
                    id: None,
                    code: ErrorCode::LimitExceeded,
                    message: "the broker has too many open connections".to_string(),
                },
            )
            .await;
            return;
        };
        let _permit = permit;

        let (mut reader, mut writer) = stream.into_split();

        // Handshake
        let first = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader)).await;
        let body = match first {
            Err(_) => return,
            Ok(Ok(body)) => body,
            Ok(Err(FrameError::TooLarge { .. })) => {
                send_error(
                    &mut writer,
                    None,
                    ErrorCode::ProtocolError,
                    "frame too large",
                )
                .await;
                return;
            }
            Ok(Err(_)) => return,
        };
        let label = match decode::<ClientMessage>(&body) {
            Ok(ClientMessage::Hello { protocol, client }) => {
                if !versions_compatible(protocol, PROTOCOL_VERSION) {
                    send_error(
                        &mut writer,
                        None,
                        ErrorCode::UnsupportedVersion,
                        &format!(
                            "protocol {protocol} is not supported; this broker speaks {PROTOCOL_VERSION}"
                        ),
                    )
                    .await;
                    return;
                }
                sanitize_label(&client)
            }
            Ok(_) | Err(_) => {
                send_error(
                    &mut writer,
                    None,
                    ErrorCode::ProtocolError,
                    "the first message must be hello",
                )
                .await;
                return;
            }
        };
        if write_with_timeout(
            &mut writer,
            &ServerMessage::Welcome {
                protocol: PROTOCOL_VERSION,
                broker_version: env!("CARGO_PKG_VERSION").to_string(),
            },
        )
        .await
        .is_err()
        {
            return;
        }

        let ctx = RequestContext {
            client_label: label,
            peer_uid: Some(peer.uid),
            peer_pid: peer.pid,
        };

        // Requests, one at a time, answered in order
        loop {
            let body = match tokio::time::timeout(IDLE_TIMEOUT, read_frame(&mut reader)).await {
                Err(_) => return,
                Ok(Ok(body)) => body,
                Ok(Err(FrameError::TooLarge { len, max })) => {
                    // The body was not consumed: the stream cannot be
                    // resynchronized, so answer and close
                    send_error(
                        &mut writer,
                        None,
                        ErrorCode::ProtocolError,
                        &format!("frame of {len} bytes exceeds the {max}-byte limit"),
                    )
                    .await;
                    return;
                }
                Ok(Err(_)) => return,
            };
            let response = match decode::<ClientMessage>(&body) {
                Ok(request) => self.gateway.handle(&ctx, request).await,
                Err(_) => {
                    // Echo the id when the body was JSON with a numeric id
                    let id = serde_json::from_slice::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64));
                    ServerMessage::Error {
                        id,
                        code: ErrorCode::ProtocolError,
                        message: "the message is not a valid request".to_string(),
                    }
                }
            };
            if write_with_timeout(&mut writer, &response).await.is_err() {
                // An oversize or unwritable answer: try a short error, then stop
                send_error(
                    &mut writer,
                    response.id(),
                    ErrorCode::Internal,
                    "the response could not be delivered",
                )
                .await;
                return;
            }
        }
    }
}

async fn write_with_timeout<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    message: &ServerMessage,
) -> Result<(), ()> {
    match tokio::time::timeout(WRITE_TIMEOUT, write_message(writer, message)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

async fn send_error<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    id: Option<u64>,
    code: ErrorCode,
    message: &str,
) {
    let _ = write_with_timeout(
        writer,
        &ServerMessage::Error {
            id,
            code,
            message: message.to_string(),
        },
    )
    .await;
}

// ---- socket setup -------------------------------------------------------------

/// Remove a stale socket file, but only if it is a socket nobody answers on.
pub(super) fn clear_stale_socket(path: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::FileTypeExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(BrokerError::State(format!("{}: {e}", path.display()))),
    };
    if !meta.file_type().is_socket() {
        return Err(BrokerError::State(format!(
            "{} exists and is not a socket; refusing to remove it",
            path.display()
        )));
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => Err(BrokerError::State(format!(
            "another broker is already listening on {}",
            path.display()
        ))),
        Err(_) => std::fs::remove_file(path)
            .map_err(|e| BrokerError::State(format!("could not remove stale socket: {e}"))),
    }
}

/// Create the listening socket with the permissions of the mode: 0600 for
/// the owner (user mode); 0660 with the configured group in service mode
/// (falling back to 0600, with a warning, when the group does not exist).
pub fn bind_socket(
    path: &Path,
    mode: BrokerMode,
    settings: &BrokerSettings,
) -> Result<UnixListener, BrokerError> {
    bind_socket_with(path, mode, settings, false)
}

/// [`bind_socket`], optionally also handing the socket's *directory* to the
/// socket group (0750). Used for the managed service socket directory, which
/// systemd creates owned by the service user's primary group: members of
/// `dbcrust-agents` must be able to traverse it to reach the socket.
pub fn bind_socket_with(
    path: &Path,
    mode: BrokerMode,
    settings: &BrokerSettings,
    share_dir_with_group: bool,
) -> Result<UnixListener, BrokerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let group = (mode == BrokerMode::Service).then(|| settings.socket_group().to_string());
    let gid = group.as_deref().and_then(group_id);

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if !parent.exists() {
            crate::secrets::fsutil::create_private_dir(parent)
                .map_err(|e| BrokerError::State(e.to_string()))?;
            if let Some(gid) = gid {
                // Let the group traverse the socket directory
                if std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o750)).is_ok()
                {
                    let _ = chown(parent, None, Some(gid));
                }
            }
        } else if let (true, Some(gid)) = (share_dir_with_group, gid) {
            // Only a plain directory the broker itself owns
            let own = std::fs::symlink_metadata(parent)
                .map(|m| m.is_dir() && m.uid() == super::paths::effective_uid())
                .unwrap_or(false);
            if own {
                if let Err(e) = chown(parent, None, Some(gid)).and_then(|()| {
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o750))
                }) {
                    tracing::warn!(
                        "could not give {} to group {:?} ({e}); clients may not reach the socket",
                        parent.display(),
                        group.as_deref().unwrap_or_default()
                    );
                }
            }
        }
    }
    clear_stale_socket(path)?;
    let listener = UnixListener::bind(path)
        .map_err(|e| BrokerError::State(format!("cannot bind {}: {e}", path.display())))?;

    let mut final_mode = 0o600;
    if let (Some(name), None) = (&group, gid) {
        tracing::warn!(
            "group {name:?} does not exist; the socket stays 0600 (only the broker's own user can connect)"
        );
    }
    if let Some(gid) = gid {
        match chown(path, None, Some(gid)) {
            Ok(()) => final_mode = 0o660,
            Err(e) => tracing::warn!(
                "could not give the socket to group {:?} ({e}); it stays 0600",
                group.as_deref().unwrap_or_default()
            ),
        }
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(final_mode))
        .map_err(|e| BrokerError::State(format!("cannot set socket permissions: {e}")))?;
    Ok(listener)
}

/// `chown` with either id left unchanged when `None`.
pub(super) fn chown(path: &Path, uid: Option<u32>, gid: Option<u32>) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("path contains NUL"))?;
    // uid_t::MAX / gid_t::MAX (-1) leave the id unchanged
    // SAFETY: valid NUL-terminated path.
    let rc = unsafe {
        libc::chown(
            c_path.as_ptr(),
            uid.map_or(libc::uid_t::MAX, |u| u as libc::uid_t),
            gid.map_or(libc::gid_t::MAX, |g| g as libc::gid_t),
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::os::unix::fs::PermissionsExt;

    #[rstest]
    #[case("claude-code", "claude-code")]
    #[case("  padded  ", "padded")]
    #[case("", "unknown")]
    #[case("\u{1b}[31mred\u{7}", "[31mred")]
    #[case("line\nbreak\r\n{\"forged\":true}", "linebreak{\"forged\":true}")]
    fn labels_are_sanitized(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(sanitize_label(raw), expected);
    }

    #[test]
    fn long_labels_are_capped() {
        let label = sanitize_label(&"x".repeat(10_000));
        assert_eq!(label.chars().count(), MAX_CLIENT_LABEL_CHARS);
    }

    #[test]
    fn admission_rules_by_mode() {
        let own = 501;
        let peer = |uid, gid| Peer {
            uid,
            gid,
            pid: None,
        };
        let user = ServerConfig {
            mode: BrokerMode::User,
            own_uid: own,
            settings: BrokerSettings::default(),
            max_connections: 4,
        };
        assert!(user.admits(&peer(own, 20)));
        assert!(!user.admits(&peer(own + 1, 20)));
        // root is not special in user mode
        assert!(!user.admits(&peer(0, 0)));

        let open_service = ServerConfig {
            mode: BrokerMode::Service,
            own_uid: 300,
            settings: BrokerSettings::default(),
            max_connections: 4,
        };
        assert!(open_service.admits(&peer(1234, 1234)));

        let restricted = ServerConfig {
            settings: BrokerSettings {
                allowed_uids: vec![501],
                allowed_gids: vec![900],
                ..BrokerSettings::default()
            },
            ..open_service
        };
        assert!(restricted.admits(&peer(501, 1)));
        assert!(restricted.admits(&peer(7, 900)));
        assert!(!restricted.admits(&peer(7, 8)));
    }

    #[tokio::test]
    async fn bind_creates_a_private_socket_and_replaces_a_stale_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("broker.sock");
        let first = bind_socket(&path, BrokerMode::User, &BrokerSettings::default()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let parent_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(parent_mode, 0o700);

        // A live broker blocks a second one
        assert!(bind_socket(&path, BrokerMode::User, &BrokerSettings::default()).is_err());
        // Once it is gone the leftover socket file is stale and replaced
        drop(first);
        assert!(path.exists());
        bind_socket(&path, BrokerMode::User, &BrokerSettings::default()).unwrap();
    }

    #[tokio::test]
    async fn a_regular_file_at_the_socket_path_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.sock");
        std::fs::write(&path, b"precious").unwrap();
        assert!(bind_socket(&path, BrokerMode::User, &BrokerSettings::default()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"precious");
    }

    #[tokio::test]
    async fn service_socket_without_the_group_stays_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broker.sock");
        let settings = BrokerSettings {
            socket_group: Some("dbcrust-no-such-group-xyz".to_string()),
            ..BrokerSettings::default()
        };
        let _listener = bind_socket(&path, BrokerMode::Service, &settings).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
