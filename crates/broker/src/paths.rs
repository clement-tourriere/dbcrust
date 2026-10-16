//! Broker modes, state-directory layout and the permission rules around it.
//!
//! * **user** (default): state in `<dbcrust config dir>/broker/`, secrets in
//!   the owner's regular secret store (`broker:<alias>` records), socket
//!   reachable only by the owner's uid.
//! * **service**: state in a directory owned by a dedicated service user
//!   (`/Library/Application Support/dbcrust-broker`, `/var/lib/dbcrust-broker`)
//!   holding an encrypted `secrets.enc` keyed by a 0600 `store.key`. File
//!   permissions are the admin boundary: only the service user (or root, via
//!   `sudo`) can enroll, revoke or tighten.

use super::BrokerError;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// File names inside a state directory.
pub const ALIASES_FILE: &str = "aliases.toml";
pub const AUDIT_FILE: &str = "audit.jsonl";
pub const BROKER_CONFIG_FILE: &str = "broker.toml";
pub const KEY_FILE: &str = "store.key";
pub const SOCKET_FILE: &str = "broker.sock";

/// The system-wide socket the thin client looks at first: `/run/dbcrust/…` on
/// Linux (where `/var/run` is a symlink to `/run`), `/var/run/dbcrust/…` on
/// macOS and other Unixes.
pub const SERVICE_SOCKET: &str = if cfg!(target_os = "linux") {
    "/run/dbcrust/broker.sock"
} else {
    "/var/run/dbcrust/broker.sock"
};
/// Group allowed to reach the service socket.
pub const DEFAULT_SOCKET_GROUP: &str = "dbcrust-agents";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrokerMode {
    #[default]
    User,
    Service,
}

impl BrokerMode {
    pub fn label(self) -> &'static str {
        match self {
            BrokerMode::User => "user",
            BrokerMode::Service => "service",
        }
    }
}

/// Everything the broker reads or writes, resolved once.
#[derive(Debug, Clone)]
pub struct BrokerPaths {
    pub mode: BrokerMode,
    pub state_dir: PathBuf,
    pub socket: PathBuf,
}

/// Inputs to [`BrokerPaths::resolve`] that come from the process environment,
/// injectable for tests.
#[derive(Debug, Clone, Default)]
pub struct PathEnv {
    pub xdg_runtime_dir: Option<PathBuf>,
    /// The DBCrust config directory (user mode state lives under it).
    pub config_dir: Option<PathBuf>,
}

impl PathEnv {
    pub fn from_process() -> Self {
        Self {
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            config_dir: dbcrust_secrets::paths::config_directory().ok(),
        }
    }
}

/// Default service state directory for this platform.
pub fn default_service_state_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/dbcrust-broker")
    } else {
        PathBuf::from("/var/lib/dbcrust-broker")
    }
}

impl BrokerPaths {
    pub fn resolve(
        mode: BrokerMode,
        state_dir: Option<PathBuf>,
        socket: Option<PathBuf>,
        env: &PathEnv,
    ) -> Result<Self, BrokerError> {
        let state_dir = match (state_dir, mode) {
            (Some(dir), _) => dir,
            (None, BrokerMode::Service) => default_service_state_dir(),
            (None, BrokerMode::User) => env
                .config_dir
                .clone()
                .ok_or_else(|| BrokerError::Config("no DBCrust config directory".to_string()))?
                .join("broker"),
        };
        let socket = match (socket, mode) {
            (Some(path), _) => path,
            (None, BrokerMode::Service) => PathBuf::from(SERVICE_SOCKET),
            (None, BrokerMode::User) => match &env.xdg_runtime_dir {
                Some(xdg) => xdg.join("dbcrust").join(SOCKET_FILE),
                None => state_dir.join(SOCKET_FILE),
            },
        };
        Ok(Self {
            mode,
            state_dir,
            socket,
        })
    }

    pub fn aliases_file(&self) -> PathBuf {
        self.state_dir.join(ALIASES_FILE)
    }

    pub fn audit_file(&self) -> PathBuf {
        self.state_dir.join(AUDIT_FILE)
    }

    pub fn broker_config_file(&self) -> PathBuf {
        self.state_dir.join(BROKER_CONFIG_FILE)
    }

    pub fn key_file(&self) -> PathBuf {
        self.state_dir.join(KEY_FILE)
    }

    pub fn secrets_file(&self) -> PathBuf {
        self.state_dir.join(dbcrust_secrets::SECRETS_FILE_NAME)
    }
}

/// `broker.toml` (service mode): who may connect and which group owns the
/// socket. Unknown keys are an error: a typo must not silently widen access.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BrokerSettings {
    /// Group given the socket (service mode), default `dbcrust-agents`.
    #[serde(default)]
    pub socket_group: Option<String>,
    /// When set, only these peer uids may connect (in addition to the file
    /// permissions on the socket).
    #[serde(default)]
    pub allowed_uids: Vec<u32>,
    /// When set, peers in these primary gids may connect.
    #[serde(default)]
    pub allowed_gids: Vec<u32>,
    /// Concurrent client connections (default 32).
    #[serde(default)]
    pub max_connections: Option<usize>,
}

impl BrokerSettings {
    pub fn load(path: &Path) -> Result<Self, BrokerError> {
        let bytes = dbcrust_secrets::fsutil::read_bounded(path, 64 * 1024)
            .map_err(|e| BrokerError::Config(e.to_string()))?;
        match bytes {
            None => Ok(Self::default()),
            Some(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|_| BrokerError::Config(format!("{} is not UTF-8", path.display())))?;
                toml::from_str(&text)
                    .map_err(|e| BrokerError::Config(format!("{}: {e}", path.display())))
            }
        }
    }

    pub fn socket_group(&self) -> &str {
        self.socket_group.as_deref().unwrap_or(DEFAULT_SOCKET_GROUP)
    }

    /// Whether a peer with these credentials may talk to the broker.
    pub fn peer_allowed(&self, uid: u32, gid: u32) -> bool {
        if self.allowed_uids.is_empty() && self.allowed_gids.is_empty() {
            return true;
        }
        self.allowed_uids.contains(&uid) || self.allowed_gids.contains(&gid)
    }
}

/// The effective uid of this process.
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Check that a service-mode state directory is private to its owner.
///
/// Every violation is reported; a non-empty list means "refuse to start".
/// `expected_uid` is the uid the broker runs as.
pub fn service_permission_violations(
    paths: &BrokerPaths,
    expected_uid: u32,
) -> Result<Vec<String>, BrokerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let mut violations = Vec::new();
    let dir = &paths.state_dir;
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(BrokerError::State(format!(
                "state directory {} does not exist",
                dir.display()
            )));
        }
        Err(e) => return Err(BrokerError::State(format!("{}: {e}", dir.display()))),
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                violations.push(format!("{} is a symbolic link", dir.display()));
            } else if !meta.is_dir() {
                violations.push(format!("{} is not a directory", dir.display()));
            } else {
                if meta.permissions().mode() & 0o077 != 0 {
                    violations.push(format!(
                        "{} is accessible by group/other (mode {:o}); it must be 0700",
                        dir.display(),
                        meta.permissions().mode() & 0o777
                    ));
                }
                if meta.uid() != expected_uid {
                    violations.push(format!(
                        "{} is owned by uid {}, not the broker's uid {expected_uid}",
                        dir.display(),
                        meta.uid()
                    ));
                }
            }
        }
    }

    for file in [
        paths.key_file(),
        paths.secrets_file(),
        paths.aliases_file(),
        paths.broker_config_file(),
        paths.audit_file(),
    ] {
        match std::fs::symlink_metadata(&file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => violations.push(format!("{}: {e}", file.display())),
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    violations.push(format!("{} is a symbolic link", file.display()));
                    continue;
                }
                if meta.permissions().mode() & 0o077 != 0 {
                    violations.push(format!(
                        "{} is accessible by group/other (mode {:o}); it must be 0600",
                        file.display(),
                        meta.permissions().mode() & 0o777
                    ));
                }
                if meta.uid() != expected_uid {
                    violations.push(format!(
                        "{} is owned by uid {}, not the broker's uid {expected_uid}",
                        file.display(),
                        meta.uid()
                    ));
                }
            }
        }
    }
    Ok(violations)
}

/// Make sure a user-mode state directory exists and is private to the owner.
pub fn ensure_user_state_dir(dir: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    dbcrust_secrets::fsutil::create_private_dir(dir)
        .map_err(|e| BrokerError::State(e.to_string()))?;
    let meta = std::fs::symlink_metadata(dir)
        .map_err(|e| BrokerError::State(format!("{}: {e}", dir.display())))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(BrokerError::State(format!(
            "{} is not a plain directory",
            dir.display()
        )));
    }
    if meta.uid() != effective_uid() {
        return Err(BrokerError::State(format!(
            "{} is not owned by the current user",
            dir.display()
        )));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| BrokerError::State(format!("{}: {e}", dir.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::os::unix::fs::PermissionsExt;

    fn env(xdg: Option<&str>, config: Option<&str>) -> PathEnv {
        PathEnv {
            xdg_runtime_dir: xdg.map(PathBuf::from),
            config_dir: config.map(PathBuf::from),
        }
    }

    #[test]
    fn user_mode_paths_follow_the_client_discovery_rules() {
        // Same layout crates/agent/src/socket.rs searches
        let with_xdg = BrokerPaths::resolve(
            BrokerMode::User,
            None,
            None,
            &env(Some("/run/user/1"), Some("/h/.config/dbcrust")),
        )
        .unwrap();
        assert_eq!(
            with_xdg.state_dir,
            PathBuf::from("/h/.config/dbcrust/broker")
        );
        assert_eq!(
            with_xdg.socket,
            PathBuf::from("/run/user/1/dbcrust/broker.sock")
        );

        let without_xdg = BrokerPaths::resolve(
            BrokerMode::User,
            None,
            None,
            &env(None, Some("/h/.config/dbcrust")),
        )
        .unwrap();
        assert_eq!(
            without_xdg.socket,
            PathBuf::from("/h/.config/dbcrust/broker/broker.sock")
        );
    }

    #[test]
    fn service_mode_defaults_and_overrides() {
        let defaults =
            BrokerPaths::resolve(BrokerMode::Service, None, None, &env(None, None)).unwrap();
        assert_eq!(defaults.socket, PathBuf::from(SERVICE_SOCKET));
        assert_eq!(defaults.state_dir, default_service_state_dir());
        let custom = BrokerPaths::resolve(
            BrokerMode::Service,
            Some("/x/state".into()),
            Some("/x/s.sock".into()),
            &env(None, None),
        )
        .unwrap();
        assert_eq!(custom.state_dir, PathBuf::from("/x/state"));
        assert_eq!(custom.socket, PathBuf::from("/x/s.sock"));
        assert_eq!(custom.key_file(), PathBuf::from("/x/state/store.key"));
    }

    fn service_paths(dir: &Path) -> BrokerPaths {
        BrokerPaths {
            mode: BrokerMode::Service,
            state_dir: dir.to_path_buf(),
            socket: dir.join("s.sock"),
        }
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn private_service_state_has_no_violations() {
        let dir = tempfile::tempdir().unwrap();
        chmod(dir.path(), 0o700);
        let key = dir.path().join(KEY_FILE);
        std::fs::write(&key, [0u8; 32]).unwrap();
        chmod(&key, 0o600);
        let paths = service_paths(dir.path());
        assert!(
            service_permission_violations(&paths, effective_uid())
                .unwrap()
                .is_empty()
        );
    }

    #[rstest]
    #[case::group_readable_dir(0o750, 0o600)]
    #[case::world_readable_dir(0o755, 0o600)]
    #[case::group_readable_key(0o700, 0o640)]
    #[case::world_readable_key(0o700, 0o644)]
    fn broad_modes_are_refused(#[case] dir_mode: u32, #[case] key_mode: u32) {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join(KEY_FILE);
        std::fs::write(&key, [0u8; 32]).unwrap();
        chmod(&key, key_mode);
        chmod(dir.path(), dir_mode);
        let violations =
            service_permission_violations(&service_paths(dir.path()), effective_uid()).unwrap();
        assert!(!violations.is_empty(), "expected a violation");
    }

    #[test]
    fn wrong_owner_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        chmod(dir.path(), 0o700);
        let key = dir.path().join(KEY_FILE);
        std::fs::write(&key, [0u8; 32]).unwrap();
        chmod(&key, 0o600);
        // Pretend the broker runs as a different uid
        let violations =
            service_permission_violations(&service_paths(dir.path()), effective_uid() + 1).unwrap();
        assert!(violations.iter().any(|v| v.contains("owned by uid")));
    }

    #[test]
    fn missing_state_dir_is_an_error_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let paths = service_paths(&dir.path().join("absent"));
        assert!(service_permission_violations(&paths, effective_uid()).is_err());
    }

    #[test]
    fn symlinked_state_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        chmod(dir.path(), 0o700);
        let real = dir.path().join("real.key");
        std::fs::write(&real, [0u8; 32]).unwrap();
        chmod(&real, 0o600);
        std::os::unix::fs::symlink(&real, dir.path().join(KEY_FILE)).unwrap();
        let violations =
            service_permission_violations(&service_paths(dir.path()), effective_uid()).unwrap();
        assert!(violations.iter().any(|v| v.contains("symbolic link")));
    }

    #[test]
    fn user_state_dir_is_created_private_and_tightened() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("broker");
        ensure_user_state_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        chmod(&dir, 0o755);
        ensure_user_state_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn broker_settings_parse_and_reject_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BROKER_CONFIG_FILE);
        std::fs::write(
            &path,
            "socket_group = \"agents\"\nallowed_uids = [501]\nallowed_gids = [20]\n",
        )
        .unwrap();
        let settings = BrokerSettings::load(&path).unwrap();
        assert_eq!(settings.socket_group(), "agents");
        assert!(settings.peer_allowed(501, 0));
        assert!(settings.peer_allowed(0, 20));
        assert!(!settings.peer_allowed(502, 21));

        std::fs::write(&path, "allowed_uid = [1]\n").unwrap();
        assert!(BrokerSettings::load(&path).is_err());

        let missing = BrokerSettings::load(&dir.path().join("none.toml")).unwrap();
        assert_eq!(missing.socket_group(), DEFAULT_SOCKET_GROUP);
        assert!(missing.peer_allowed(1, 1));
    }
}
