//! Test isolation for this crate: no test may read or write the
//! developer's real `~/.config/dbcrust`.
//!
//! DBCrust core puts its files in a temp dir when it believes it runs a
//! test, but that heuristic (`cfg!(test)` in *its* crate, or a thread name
//! containing "test") misses threads a test spawns with
//! `std::thread::spawn`: they used to resolve to the real config
//! directory. [`isolate`] pins the directory for the whole process with the
//! core's override, which wins over the heuristic on every thread, without
//! mutating the environment. It points at the very directory the heuristic
//! picks, so paths resolve the same before and after it runs.

use std::path::PathBuf;

/// The per-process directory every test's config files live in.
pub(crate) fn config_dir() -> PathBuf {
    std::env::temp_dir().join(format!("dbcrust_test_{}", std::process::id()))
}

/// Pin DBCrust's config directory to [`config_dir`] for the whole process,
/// on every thread. Idempotent. `ConfigStore::from_config` / `load` call it
/// in test builds; tests that write user files call it first anyway.
pub(crate) fn isolate() {
    dbcrust_app::config::set_config_dir_override(config_dir());
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcrust_app::config::Config;
    use std::process::Command;

    /// Set in the subprocess that runs the guard's checks.
    const CHILD: &str = "DBCRUST_GUI_CORE_ISOLATION_GUARD_CHILD";
    const GUARD: &str =
        "test_support::tests::spawned_threads_never_resolve_the_real_config_directory";

    /// The guard. Isolation is process-wide and other tests set it up, so
    /// the checks run in a fresh copy of this test binary where nothing has:
    /// only `ConfigStore::from_config` may isolate it. `HOME` points at a
    /// temp dir there, so even a regression cannot reach the real config.
    #[test]
    fn spawned_threads_never_resolve_the_real_config_directory() {
        if std::env::var_os(CHILD).is_some() {
            return check_in_child();
        }
        let home = tempfile::tempdir().expect("temp HOME");
        let output = Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", GUARD, "--nocapture", "--test-threads=1"])
            .env_remove("DBCRUST_CONFIG_DIR")
            .env_remove("RUST_TEST_MODE")
            .env("HOME", home.path())
            .env(CHILD, "1")
            .output()
            .expect("run the guard in a subprocess");
        assert!(
            output.status.success(),
            "the isolation guard failed in its subprocess:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the guard did not run in its subprocess:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// In the subprocess: a store exists, so a thread with no name (which
    /// the core's heuristic does not recognise as a test) still resolves
    /// every user file inside the test directory.
    fn check_in_child() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let _store = crate::ConfigStore::from_config(Config::default_settings());
        let paths = std::thread::spawn(|| {
            assert!(std::thread::current().name().is_none());
            [
                Config::get_config_directory().unwrap(),
                Config::get_config_file_path().unwrap(),
                Config::get_recent_connections_path().unwrap(),
                Config::get_saved_sessions_path().unwrap(),
                Config::get_named_queries_path().unwrap(),
                Config::get_vault_credentials_path().unwrap(),
                crate::gui_state::GuiState::path().unwrap(),
            ]
        })
        .join()
        .unwrap();
        let isolated = config_dir();
        for path in paths {
            assert!(
                path.starts_with(&isolated),
                "{} is outside the test directory {}",
                path.display(),
                isolated.display()
            );
            assert!(!path.starts_with(home.join(".config")));
        }
    }
}
