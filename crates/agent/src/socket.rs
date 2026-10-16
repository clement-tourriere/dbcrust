//! Broker socket discovery.
//!
//! An explicit location is used alone, never with a silent fallback:
//! `--socket PATH`, else `$DBCRUST_BROKER_SOCKET` (a path only — never a URL).
//! Without one, the first existing socket wins:
//!
//! 1. the service socket: `/run/dbcrust/broker.sock` on Linux,
//!    `/var/run/dbcrust/broker.sock` on macOS (where `dbcrust broker
//!    install-service` puts it; `/var/run` is a symlink to `/run` on modern Linux)
//! 2. the user socket: `$XDG_RUNTIME_DIR/dbcrust/broker.sock` when
//!    `XDG_RUNTIME_DIR` is set, else `<config dir>/broker/broker.sock`, where
//!    config dir is `$DBCRUST_CONFIG_DIR` or `$HOME/.config/dbcrust` (the same
//!    resolution as the main `dbcrust` binary, mirrored here on purpose so this
//!    crate needs no config code)
//!
//! This is the only place that reads the process environment, and it reads
//! exactly four variables.

use std::fmt;
use std::path::{Path, PathBuf};

/// The system-wide service socket for this platform.
pub const SERVICE_SOCKET: &str = if cfg!(target_os = "linux") {
    "/run/dbcrust/broker.sock"
} else {
    "/var/run/dbcrust/broker.sock"
};

/// The four environment variables the client is allowed to look at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub broker_socket: Option<PathBuf>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

impl Env {
    /// Read `DBCRUST_BROKER_SOCKET`, `XDG_RUNTIME_DIR`, `DBCRUST_CONFIG_DIR`
    /// and `HOME` — nothing else.
    pub fn from_process() -> Self {
        fn var(name: &str) -> Option<PathBuf> {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        }
        Self {
            broker_socket: var("DBCRUST_BROKER_SOCKET"),
            xdg_runtime_dir: var("XDG_RUNTIME_DIR"),
            config_dir: var("DBCRUST_CONFIG_DIR"),
            home: var("HOME"),
        }
    }
}

/// Where to look for the broker.
#[derive(Debug, Clone)]
pub struct SocketSearch {
    pub flag: Option<PathBuf>,
    pub env: Env,
    pub service_socket: PathBuf,
}

impl SocketSearch {
    pub fn new(flag: Option<PathBuf>, env: Env) -> Self {
        Self {
            flag,
            env,
            service_socket: PathBuf::from(SERVICE_SOCKET),
        }
    }

    /// Every candidate path, in priority order, without duplicates. An
    /// explicit `--socket` or `$DBCRUST_BROKER_SOCKET` is the only candidate:
    /// falling back would talk to a different broker than the one asked for.
    pub fn candidates(&self) -> Vec<PathBuf> {
        if let Some(explicit) = self.flag.as_ref().or(self.env.broker_socket.as_ref()) {
            return vec![explicit.clone()];
        }
        let mut out: Vec<PathBuf> = Vec::new();
        let mut push = |path: PathBuf| {
            if !out.contains(&path) {
                out.push(path);
            }
        };
        push(self.service_socket.clone());
        if let Some(xdg) = &self.env.xdg_runtime_dir {
            push(xdg.join("dbcrust").join("broker.sock"));
        }
        // Also the fallback when XDG_RUNTIME_DIR is set: a broker started from
        // a shell without it listens here, and a stale candidate is harmless
        let config_dir = self.env.config_dir.clone().or_else(|| {
            self.env
                .home
                .as_ref()
                .map(|h| h.join(".config").join("dbcrust"))
        });
        if let Some(dir) = config_dir {
            push(dir.join("broker").join("broker.sock"));
        }
        out
    }

    /// The first candidate that exists and is a socket.
    pub fn discover(&self) -> Result<PathBuf, SocketNotFound> {
        let candidates = self.candidates();
        match candidates.iter().find(|path| is_socket(path)) {
            Some(path) => Ok(path.clone()),
            None => Err(SocketNotFound { tried: candidates }),
        }
    }
}

#[cfg(unix)]
fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}

#[cfg(not(unix))]
fn is_socket(path: &Path) -> bool {
    path.exists()
}

/// No candidate socket exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketNotFound {
    pub tried: Vec<PathBuf>,
}

impl fmt::Display for SocketNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no DBCrust broker socket found; tried: ")?;
        for (i, path) in self.tried.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", path.display())?;
        }
        f.write_str(". Ask the database owner to start the broker with `dbcrust broker serve`, or point to it with --socket / DBCRUST_BROKER_SOCKET")
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn search(dir: &Path, flag: Option<&str>, env: Env) -> SocketSearch {
        SocketSearch {
            flag: flag.map(|f| dir.join(f)),
            env,
            service_socket: dir.join("service.sock"),
        }
    }

    fn listen(path: &Path) -> UnixListener {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        UnixListener::bind(path).unwrap()
    }

    #[test]
    fn default_service_socket_is_platform_specific() {
        let s = SocketSearch::new(None, Env::default());
        let expected = if cfg!(target_os = "linux") {
            "/run/dbcrust/broker.sock"
        } else {
            "/var/run/dbcrust/broker.sock"
        };
        assert_eq!(s.candidates().first().unwrap(), Path::new(expected));
    }

    #[test]
    fn explicit_location_is_the_only_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            broker_socket: Some(dir.path().join("env.sock")),
            xdg_runtime_dir: Some(dir.path().join("xdg")),
            config_dir: Some(dir.path().join("cfg")),
            home: Some(dir.path().join("home")),
        };
        let s = search(dir.path(), Some("flag.sock"), env.clone());
        assert_eq!(s.candidates(), vec![dir.path().join("flag.sock")]);
        let s = search(dir.path(), None, env);
        assert_eq!(s.candidates(), vec![dir.path().join("env.sock")]);
    }

    #[test]
    fn candidate_order_is_service_xdg_config() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            xdg_runtime_dir: Some(dir.path().join("xdg")),
            config_dir: Some(dir.path().join("cfg")),
            home: Some(dir.path().join("home")),
            ..Env::default()
        };
        let s = search(dir.path(), None, env);
        assert_eq!(
            s.candidates(),
            vec![
                dir.path().join("service.sock"),
                dir.path().join("xdg/dbcrust/broker.sock"),
                dir.path().join("cfg/broker/broker.sock"),
            ]
        );
    }

    #[test]
    fn config_dir_falls_back_to_home_dot_config() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            home: Some(dir.path().join("home")),
            ..Env::default()
        };
        let s = search(dir.path(), None, env);
        assert_eq!(
            s.candidates().last().unwrap(),
            &dir.path().join("home/.config/dbcrust/broker/broker.sock")
        );
    }

    #[test]
    fn first_existing_socket_wins() {
        let dir = tempfile::tempdir().unwrap();
        let _service = listen(&dir.path().join("service.sock"));
        let _user = listen(&dir.path().join("cfg/broker/broker.sock"));
        let env = Env {
            config_dir: Some(dir.path().join("cfg")),
            ..Env::default()
        };
        // flag points nowhere: an error, never a fallback to another broker
        let s = search(dir.path(), Some("missing.sock"), env.clone());
        assert_eq!(
            s.discover().unwrap_err().tried,
            vec![dir.path().join("missing.sock")]
        );

        // an existing flag path beats everything
        let _flag = listen(&dir.path().join("flag.sock"));
        let s = search(dir.path(), Some("flag.sock"), env.clone());
        assert_eq!(s.discover().unwrap(), dir.path().join("flag.sock"));

        // env beats service and user sockets
        let _env_sock = listen(&dir.path().join("env.sock"));
        let env_with = Env {
            broker_socket: Some(dir.path().join("env.sock")),
            ..env
        };
        let s = search(dir.path(), None, env_with);
        assert_eq!(s.discover().unwrap(), dir.path().join("env.sock"));
    }

    #[test]
    fn user_socket_is_found_when_nothing_else_exists() {
        let dir = tempfile::tempdir().unwrap();
        let _user = listen(&dir.path().join("xdg/dbcrust/broker.sock"));
        let env = Env {
            xdg_runtime_dir: Some(dir.path().join("xdg")),
            ..Env::default()
        };
        let s = search(dir.path(), None, env);
        assert_eq!(
            s.discover().unwrap(),
            dir.path().join("xdg/dbcrust/broker.sock")
        );
    }

    #[test]
    fn regular_files_are_not_sockets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("service.sock"), b"").unwrap();
        let s = search(dir.path(), None, Env::default());
        let err = s.discover().unwrap_err();
        assert!(err.tried.contains(&dir.path().join("service.sock")));
    }

    #[test]
    fn not_found_lists_paths_and_the_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            config_dir: Some(dir.path().join("cfg")),
            ..Env::default()
        };
        let s = search(dir.path(), None, env);
        let message = s.discover().unwrap_err().to_string();
        assert!(message.contains("service.sock"));
        assert!(message.contains("cfg/broker/broker.sock"));
        assert!(message.contains("dbcrust broker serve"));
    }

    #[test]
    fn duplicates_are_collapsed() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = search(
            dir.path(),
            None,
            Env {
                config_dir: Some(dir.path().join("cfg")),
                ..Env::default()
            },
        );
        s.service_socket = dir.path().join("cfg/broker/broker.sock");
        assert_eq!(
            s.candidates(),
            vec![dir.path().join("cfg/broker/broker.sock")]
        );
    }
}
