//! Shared setup of the in-process UI tests (`workspace/tests.rs`,
//! `query/tests.rs`).

use dbcrust_gui_core::docker::{self, DockerDiscovery};
use std::sync::Once;

/// Make the tests hermetic. Call first thing in every test that builds an app.
///
/// - The config directory is a per-process temp directory. DBCrust core only
///   isolates it by itself on threads named `*test*`, which the connection
///   workers are not, and not at all with `--test-threads=1` (tests then run
///   on `main`).
/// - Home never queries the Docker daemon of the machine running the tests.
pub(crate) fn isolate() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir =
            std::env::temp_dir().join(format!("dbcrust_gui_test_config_{}", std::process::id()));
        // SAFETY: runs once, and before the calling test reads the environment
        unsafe { std::env::set_var("DBCRUST_CONFIG_DIR", dir) };
        docker::set_discovery_override(DockerDiscovery::Unavailable(
            "Docker is disabled in tests".into(),
        ));
    });
}
