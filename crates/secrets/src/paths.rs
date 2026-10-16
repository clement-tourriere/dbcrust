//! Where DBCrust keeps its files, and whether this process is a test.
//!
//! Every user file of DBCrust (config, sessions, recents, saved queries, the
//! secret store index and file, history…) resolves its directory through
//! [`config_directory`]. Test isolation lives here too, because the same
//! decision ("is this a test? then use a throwaway directory and an
//! in-memory secret store") must be shared by the config layer and the secret
//! store and must never be taken twice with different answers.
//!
//! # Test mode
//!
//! [`is_test_mode`] is true when any of these holds:
//!
//! * the crate is compiled for its own unit tests (`cfg!(test)`) or with the
//!   `test-mode` cargo feature. **`cfg!(test)` is only true in the crate
//!   being tested**: the root crate's tests link `dbcrust-secrets` compiled
//!   *without* `cfg(test)`, so every crate whose tests can reach config or
//!   secrets must enable the `test-mode` feature of this crate in its
//!   `[dev-dependencies]` (dev-dependency features are not unified into
//!   normal or release builds);
//! * [`force_test_mode`] was called (a runtime switch for harnesses that
//!   cannot depend on a feature: a custom test runner, a Python test
//!   fixture, `gui-core`'s test support);
//! * the `RUST_TEST_MODE` environment variable is set;
//! * the current thread's name contains `test` (the libtest default).
//!
//! The last two are heuristics; they miss threads a test spawns without a
//! name (`std::thread::spawn`, tokio workers), which is exactly how a test
//! once overwrote the real `~/.config/dbcrust/recent.toml`. The first two are
//! process-wide and exact. [`set_config_dir_override`] is the other
//! process-wide pin: it fixes the directory itself.

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Environment variable that forces the configuration directory (useful when
/// a Python/Django process runs with a different `HOME`: Docker, systemd,
/// IDE-launched servers).
pub const CONFIG_DIR_ENV: &str = "DBCRUST_CONFIG_DIR";

/// Environment variable that puts the process in test mode.
pub const TEST_MODE_ENV: &str = "RUST_TEST_MODE";

/// Set by [`force_test_mode`]; never reset.
static FORCED_TEST_MODE: AtomicBool = AtomicBool::new(false);

/// Put the whole process in test mode, on every thread, for good.
///
/// Call it from the first line of a test harness that cannot enable the
/// `test-mode` cargo feature. It cannot be undone: a process that has been
/// told it is a test must not later reach the real config directory or
/// keychain.
#[doc(hidden)]
pub fn force_test_mode() {
    FORCED_TEST_MODE.store(true, Ordering::SeqCst);
}

/// Whether this process must stay away from the real config directory, the
/// real keychain and `$HOME`. See the module docs for what triggers it.
#[doc(hidden)]
pub fn is_test_mode() -> bool {
    resolve_test_mode(
        cfg!(any(test, feature = "test-mode")),
        FORCED_TEST_MODE.load(Ordering::SeqCst),
        std::env::var_os(TEST_MODE_ENV).is_some(),
        std::thread::current().name(),
    )
}

/// The decision of [`is_test_mode`] as a pure function of its inputs, so the
/// cross-crate case (`compiled == false`) can be tested from inside the
/// crate.
fn resolve_test_mode(
    compiled: bool,
    forced: bool,
    env_set: bool,
    thread_name: Option<&str>,
) -> bool {
    compiled || forced || env_set || thread_name.is_some_and(|name| name.contains("test"))
}

/// The config directory pinned for this whole process by
/// [`set_config_dir_override`]. Nothing sets it in the CLI or the app.
static CONFIG_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Pin every DBCrust user file of this process (config.toml, sessions,
/// recents, saved queries, the Vault cache, history, the AI key file and
/// the desktop app's state) under `path`, on every thread at once, without
/// touching the environment (which other threads may read concurrently).
/// It wins over `DBCRUST_CONFIG_DIR`, the test heuristic and the home
/// directory. Set it before any file is read; the first call wins, and a
/// later call returns whether `path` is the one in effect. For test
/// harnesses.
#[doc(hidden)]
pub fn set_config_dir_override(path: PathBuf) -> bool {
    let in_effect = CONFIG_DIR_OVERRIDE.get_or_init(|| path.clone());
    *in_effect == path
}

/// The one place the config directory is chosen from: the process override,
/// then `DBCRUST_CONFIG_DIR`.
fn config_dir_override() -> Option<PathBuf> {
    if let Some(path) = CONFIG_DIR_OVERRIDE.get() {
        return Some(path.clone());
    }
    let raw = std::env::var_os(CONFIG_DIR_ENV)?;
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// `~/.config/dbcrust`, ignoring every override and the test heuristic.
fn home_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config").join("dbcrust"))
}

/// The throwaway directory test mode resolves to.
fn test_config_dir() -> PathBuf {
    std::env::temp_dir().join(format!("dbcrust_test_{}", std::process::id()))
}

/// The configuration directory: single source of truth for all config files.
///
/// Order: [`set_config_dir_override`], then `DBCRUST_CONFIG_DIR`, then (in
/// test mode) `<tmp>/dbcrust_test_<pid>`, then `~/.config/dbcrust`. The
/// directory is created if it does not exist.
pub fn config_directory() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(config_dir) = config_dir_override() {
        ensure_directory(&config_dir)?;
        return Ok(config_dir);
    }

    if is_test_mode() {
        let test_dir = test_config_dir();
        ensure_directory(&test_dir)?;
        Ok(test_dir)
    } else if let Some(config_dir) = home_config_dir() {
        ensure_directory(&config_dir)?;
        Ok(config_dir)
    } else {
        Err("Failed to get configuration directory".into())
    }
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    if path.exists() {
        if path.is_dir() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "Config path exists but is not a directory: {}",
                path.display()
            )))
        }
    } else {
        fs::create_dir_all(path)
    }
}

/// Replace `path` atomically: write a temp file next to it, then rename it
/// over the original. A concurrent reader (another DBCrust process, or the
/// desktop app's connection workers) sees either the old or the new file,
/// never a truncated one. Symlinks are followed (the link itself is kept).
///
/// The temp file is created owner-only (0600 on Unix) before any content is
/// written; afterwards it takes the existing file's permissions, so a file
/// the user restricted stays restricted and new files stay private.
pub fn write_file_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "dbcrust".to_string());
    // Unique per write: concurrent writers (threads, processes) never share
    // a temp file, and `create_new` guarantees it starts with our mode
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let temp = target.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Ok(metadata) = fs::metadata(&target) {
            fs::set_permissions(&temp, metadata.permissions())?;
        }
        fs::rename(&temp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendKind, default_store};
    use rstest::rstest;

    #[test]
    fn atomic_write_replaces_content_and_keeps_symlinks_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.toml");
        fs::write(&real, "old = 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        }

        write_file_atomically(&real, b"new = 2\n").unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), "new = 2\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&real).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);

            let link = dir.path().join("link.toml");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            write_file_atomically(&link, b"via = 3\n").unwrap();
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read_to_string(&real).unwrap(), "via = 3\n");
        }
        // New files are private
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let fresh = dir.path().join("fresh.toml");
            write_file_atomically(&fresh, b"a = 1\n").unwrap();
            let mode = fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // No temp files left behind
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    /// `compiled` is `cfg!(any(test, feature = "test-mode"))`: false is what
    /// this module sees when another crate's tests link it. The explicit
    /// switch and the environment must carry the decision on their own.
    #[rstest]
    #[case::compiled_in(true, false, false, None, true)]
    #[case::forced_without_cfg_test(false, true, false, None, true)]
    #[case::env_without_cfg_test(false, false, true, None, true)]
    #[case::libtest_thread_name(false, false, false, Some("tests::my_test"), true)]
    #[case::unnamed_thread_is_not_detected(false, false, false, None, false)]
    #[case::other_name_is_not_detected(false, false, false, Some("tokio-runtime-worker"), false)]
    fn test_mode_decision(
        #[case] compiled: bool,
        #[case] forced: bool,
        #[case] env_set: bool,
        #[case] thread_name: Option<&str>,
        #[case] expected: bool,
    ) {
        assert_eq!(
            resolve_test_mode(compiled, forced, env_set, thread_name),
            expected
        );
    }

    /// Canary for the incident in the `test-config-isolation` memory: a
    /// thread a test spawns with `std::thread::spawn` has no name, so the
    /// thread-name heuristic is blind to it. It must still resolve to a
    /// throwaway config directory and an in-memory secret store, never to
    /// `~/.config/dbcrust` or the keychain.
    ///
    /// Here `cfg!(test)` makes this trivially true; the root crate's
    /// `test_isolation` canaries guard the `test-mode` feature wiring.
    #[test]
    fn unnamed_threads_never_reach_the_real_config_dir_or_keychain() {
        let (in_test_mode, dir, backend) = std::thread::spawn(|| {
            assert!(
                std::thread::current().name().is_none(),
                "the canary must run on a bare, unnamed thread"
            );
            let backend: BackendKind = default_store().backend_kind();
            (
                is_test_mode(),
                config_directory().map_err(|e| e.to_string()),
                backend,
            )
        })
        .join()
        .expect("canary thread panicked");

        assert!(in_test_mode, "an unnamed thread must still be in test mode");
        assert_eq!(
            backend,
            BackendKind::Memory,
            "the default secret store must be in-memory under test"
        );
        let dir = dir.expect("config directory resolves");
        if let Some(real) = home_config_dir() {
            assert!(
                !dir.starts_with(&real),
                "{} is under the real config dir {}",
                dir.display(),
                real.display()
            );
        }
        // The same directory the main test thread sees: no thread has its own
        assert_eq!(dir, config_directory().unwrap());
        if std::env::var_os(CONFIG_DIR_ENV).is_none() && CONFIG_DIR_OVERRIDE.get().is_none() {
            assert_eq!(dir, test_config_dir());
        }
    }

    /// Same guard for the async runtimes the app and the broker use: tokio
    /// worker and `spawn_blocking` threads are named `tokio-runtime-worker`
    /// / `tokio-rt-worker`, not `*test*`.
    #[test]
    fn runtime_worker_threads_resolve_the_isolated_directory() {
        let expected = config_directory().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap();
        let (worker, blocking) = runtime.block_on(async {
            let worker = tokio::spawn(async { (is_test_mode(), config_directory().unwrap()) })
                .await
                .unwrap();
            let blocking =
                tokio::task::spawn_blocking(|| (is_test_mode(), config_directory().unwrap()))
                    .await
                    .unwrap();
            (worker, blocking)
        });
        for (in_test_mode, dir) in [worker, blocking] {
            assert!(in_test_mode);
            assert_eq!(dir, expected);
        }
    }
}
