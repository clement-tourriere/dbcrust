//! Process hardening applied when `broker serve` starts.
//!
//! * `umask 077` — every file the broker creates is private;
//! * core dumps off (`RLIMIT_CORE = 0`) — a crash must not write secrets
//!   to disk;
//! * debugger attach blocked: Linux `prctl(PR_SET_DUMPABLE, 0)` (also blocks
//!   same-user `ptrace` and `/proc/<pid>/mem`), macOS
//!   `ptrace(PT_DENY_ATTACH)` (skippable with `--allow-debugger` for
//!   development);
//! * environment scrubbed of variables that would redirect a connection or
//!   its trust roots (`PG*`, `MYSQL*`, `CLICKHOUSE*`, `SSL_CERT_*`, proxy
//!   variables, stray `DBCRUST_*`) before any client is created.

/// What `apply` did, for the startup log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HardeningReport {
    pub umask_set: bool,
    pub core_dumps_disabled: bool,
    pub debugger_blocked: bool,
    pub scrubbed_env: Vec<String>,
}

/// Whether an environment variable must not reach database drivers.
///
/// `DBCRUST_CONFIG_DIR` and the broker's own `DBCRUST_BROKER_*` variables
/// are kept; every other `DBCRUST_*` is dropped.
pub fn should_scrub_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper.starts_with("PG") || upper.starts_with("MYSQL") || upper.starts_with("CLICKHOUSE") {
        return true;
    }
    if matches!(
        upper.as_str(),
        "SSL_CERT_FILE" | "SSL_CERT_DIR" | "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY"
    ) {
        return true;
    }
    if upper.starts_with("DBCRUST_") {
        return !(upper == "DBCRUST_CONFIG_DIR" || upper.starts_with("DBCRUST_BROKER_"));
    }
    false
}

static SCRUBBED: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Remove scrub-listed variables from this process's environment, once.
///
/// `main()` calls this *before the tokio runtime exists* when argv is
/// `broker serve` (`remove_var` is only sound while no other thread can read
/// the environment); [`apply`] calls it again as a safety net for argv shapes
/// the early check does not recognize, where it finds nothing left to remove
/// or, at worst, removes them with idle runtime workers. Returns the names
/// removed by the first call.
pub fn scrub_environment() -> &'static [String] {
    SCRUBBED.get_or_init(|| {
        let names: Vec<String> = std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .filter(|name| should_scrub_env(name))
            .collect();
        for name in &names {
            // SAFETY: see the function documentation: single-threaded when
            // called from `main`; otherwise runs at startup before the broker
            // creates any client, while the runtime's workers are parked.
            unsafe { std::env::remove_var(name) };
        }
        names
    })
}

/// Whether argv (program name first) is `dbcrust … broker serve …`: the
/// first two non-flag words. Used before the argument parser and runtime run.
pub fn argv_is_broker_serve<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut words = args
        .into_iter()
        .skip(1)
        .filter(|a| !a.as_ref().to_string_lossy().starts_with('-'));
    let first = words.next();
    let second = words.next();
    matches!(
        (first, second),
        (Some(a), Some(b)) if a.as_ref() == "broker" && b.as_ref() == "serve"
    )
}

#[cfg(target_os = "linux")]
fn block_debugger() -> bool {
    // SAFETY: PR_SET_DUMPABLE with a plain integer argument.
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) == 0 }
}

#[cfg(target_os = "macos")]
fn block_debugger() -> bool {
    const PT_DENY_ATTACH: libc::c_int = 31;
    // SAFETY: PT_DENY_ATTACH takes no pid/address/data.
    unsafe { libc::ptrace(PT_DENY_ATTACH, 0, std::ptr::null_mut(), 0) == 0 }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn block_debugger() -> bool {
    false
}

/// Apply every hardening step. Failures are reported, not fatal: the broker
/// logs what it could not do.
///
/// Call this **after** any privilege drop: Linux resets `PR_SET_DUMPABLE` on
/// credential changes, so a debugger-attach block applied while root would be
/// undone by the `setuid`.
pub fn apply(allow_debugger: bool) -> HardeningReport {
    let mut report = HardeningReport::default();

    // SAFETY: umask cannot fail.
    unsafe { libc::umask(0o077) };
    report.umask_set = true;

    let no_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: valid pointer to an initialized rlimit.
    report.core_dumps_disabled = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &no_core) } == 0;

    if !allow_debugger {
        report.debugger_blocked = block_debugger();
    }

    report.scrubbed_env = scrub_environment().to_vec();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(&["dbcrust", "broker", "serve"], true)]
    #[case(&["dbcrust", "broker", "serve", "--mode", "service"], true)]
    #[case(&["dbcrust", "--no-input", "broker", "serve"], true)]
    #[case(&["dbcrust", "broker", "status"], false)]
    #[case(&["dbcrust", "broker"], false)]
    #[case(&["dbcrust", "postgres://h/db", "-c", "serve"], false)]
    #[case(&["dbcrust", "serve", "broker"], false)]
    fn broker_serve_argv_detection(#[case] argv: &[&str], #[case] expected: bool) {
        assert_eq!(argv_is_broker_serve(argv), expected);
    }

    #[rstest]
    #[case("PGHOST", true)]
    #[case("PGPASSWORD", true)]
    #[case("PGSSLMODE", true)]
    #[case("pgservice", true)]
    #[case("MYSQL_HOST", true)]
    #[case("MYSQL_PWD", true)]
    #[case("CLICKHOUSE_HOST", true)]
    #[case("CLICKHOUSE_PASSWORD", true)]
    #[case("SSL_CERT_FILE", true)]
    #[case("SSL_CERT_DIR", true)]
    #[case("HTTP_PROXY", true)]
    #[case("https_proxy", true)]
    #[case("All_Proxy", true)]
    #[case("no_proxy", true)]
    #[case("DBCRUST_SOMETHING", true)]
    #[case("DBCRUST_CONFIG_DIR", false)]
    #[case("DBCRUST_BROKER_SOCKET", false)]
    #[case("HOME", false)]
    #[case("PATH", false)]
    #[case("XDG_RUNTIME_DIR", false)]
    #[case("RUST_LOG", false)]
    #[case("PAGER", false)]
    fn env_scrub_list(#[case] name: &str, #[case] scrubbed: bool) {
        assert_eq!(should_scrub_env(name), scrubbed, "{name}");
    }
}
