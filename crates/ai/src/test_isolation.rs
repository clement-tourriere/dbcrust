//! Canaries for the test-isolation wiring of `dbcrust-secrets`.
//!
//! `cfg!(test)` is only true inside the crate under test, so this crate's
//! tests see `dbcrust_secrets` compiled as a normal dependency. What keeps a
//! test on a thread libtest did not name (`std::thread::spawn`, tokio
//! workers: the OAuth callback server and key probes run on them) away from
//! the real `~/.config/dbcrust` and keychain is the `test-mode` feature this
//! crate enables from `[dev-dependencies]`. If that
//! entry is dropped these tests fail — but only when `RUST_TEST_MODE` is not
//! set (it would mask the regression), so run them once with it unset:
//!
//! ```text
//! env -u RUST_TEST_MODE -u DBCRUST_CONFIG_DIR cargo test -p dbcrust-ai --lib test_isolation
//! ```

use dbcrust_secrets::{BackendKind, default_store, paths};

fn real_config_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|dir| dir.join("dbcrust"))
}

fn assert_isolated(label: &str, in_test_mode: bool, dir: std::path::PathBuf, backend: BackendKind) {
    assert!(in_test_mode, "{label}: not in test mode");
    assert_eq!(
        backend,
        BackendKind::Memory,
        "{label}: the default secret store must be in-memory under test"
    );
    if let Some(real) = real_config_dir() {
        assert!(
            !dir.starts_with(&real),
            "{label}: {} is under the real config dir {}",
            dir.display(),
            real.display()
        );
    }
}

/// A bare `std::thread` has no name: the thread-name heuristic cannot see it.
#[test]
fn unnamed_threads_stay_isolated() {
    let (in_test_mode, dir, backend) = std::thread::spawn(|| {
        assert!(std::thread::current().name().is_none());
        (
            paths::is_test_mode(),
            paths::config_directory().expect("config directory resolves"),
            default_store().backend_kind(),
        )
    })
    .join()
    .expect("canary thread panicked");
    assert_isolated("std::thread", in_test_mode, dir, backend);
}

/// Tokio worker and `spawn_blocking` threads are not named like tests either.
#[test]
fn runtime_threads_stay_isolated() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    let probe = || {
        (
            paths::is_test_mode(),
            paths::config_directory().expect("config directory resolves"),
            default_store().backend_kind(),
        )
    };
    let (worker, blocking) = runtime.block_on(async move {
        let worker = tokio::spawn(async move { probe() }).await.unwrap();
        let blocking = tokio::task::spawn_blocking(probe).await.unwrap();
        (worker, blocking)
    });
    assert_isolated("tokio worker", worker.0, worker.1, worker.2);
    assert_isolated("spawn_blocking", blocking.0, blocking.1, blocking.2);
}
