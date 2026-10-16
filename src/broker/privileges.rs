//! Who the broker runs as: the trusted-binary rule and verified privilege drops.
//!
//! **Trusted binary.** A service-mode broker (or `sudo dbcrust broker …` admin
//! command) runs with more authority than the user who installed it. If it
//! were launched from a user-writable path such as `~/.cargo/bin/dbcrust`, any
//! same-user process — an AI agent included — could replace that file and be
//! executed as root. So in service mode the running executable and *every
//! ancestor directory* must be owned by root and not writable by group or
//! other. `dbcrust broker install-service` puts a copy of the binary at
//! [`INSTALLED_BINARY`] for exactly this reason.
//!
//! **Verified drop.** After `setgroups → setgid → setuid` the process proves it
//! cannot become root again and that its ids are what was asked for; anything
//! else is fatal.

use super::BrokerError;
use std::path::{Path, PathBuf};

/// Where `install-service` puts the binary the daemon and `sudo` admin
/// commands run.
pub const INSTALLED_BINARY: &str = "/usr/local/libexec/dbcrust-broker";

// ---- trusted binary ------------------------------------------------------------

/// Ownership and mode of one path component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathOwnership {
    pub path: PathBuf,
    pub uid: u32,
    /// Permission bits (`st_mode & 0o7777`).
    pub mode: u32,
}

/// Violations of "owned by root, not group/world-writable" in a chain of
/// path components. Group *ownership* is not checked: `root:wheel` and
/// `root:admin` are fine as long as the group cannot write.
pub fn ownership_violations(chain: &[PathOwnership]) -> Vec<String> {
    let mut violations = Vec::new();
    for entry in chain {
        if entry.uid != 0 {
            violations.push(format!(
                "{} is owned by uid {}, not root",
                entry.path.display(),
                entry.uid
            ));
        }
        if entry.mode & 0o020 != 0 {
            violations.push(format!(
                "{} is group-writable (mode {:o})",
                entry.path.display(),
                entry.mode & 0o777
            ));
        }
        if entry.mode & 0o002 != 0 {
            violations.push(format!(
                "{} is world-writable (mode {:o})",
                entry.path.display(),
                entry.mode & 0o777
            ));
        }
    }
    violations
}

/// The file and each of its ancestors, with ownership read through `stat`
/// (`path → (uid, mode)`). The path must already be canonical.
pub fn ownership_chain(
    path: &Path,
    stat: impl Fn(&Path) -> std::io::Result<(u32, u32)>,
) -> std::io::Result<Vec<PathOwnership>> {
    path.ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| {
            let (uid, mode) = stat(p)?;
            Ok(PathOwnership {
                path: p.to_path_buf(),
                uid,
                mode,
            })
        })
        .collect()
}

fn stat_real(path: &Path) -> std::io::Result<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path)?;
    Ok((meta.uid(), meta.mode() & 0o7777))
}

/// Trust violations of an executable path, using the real file system.
/// The path is canonicalized first, so symlinks are judged by their target.
pub fn binary_trust_violations(exe: &Path) -> Result<Vec<String>, BrokerError> {
    let canonical = std::fs::canonicalize(exe)
        .map_err(|e| BrokerError::State(format!("{}: {e}", exe.display())))?;
    let chain = ownership_chain(&canonical, stat_real)
        .map_err(|e| BrokerError::State(format!("{}: {e}", canonical.display())))?;
    Ok(ownership_violations(&chain))
}

/// At most three violations for a one-screen message, then a count.
fn summarize(violations: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut text = violations
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if violations.len() > SHOWN {
        text.push_str(&format!(" (and {} more)", violations.len() - SHOWN));
    }
    text
}

/// The refusal message shown for an untrusted binary.
pub fn untrusted_binary_message(exe: &Path, violations: &[String]) -> String {
    format!(
        "refusing to run the service-mode broker from an untrusted binary ({}): {}. \
         A binary that a normal user can modify would run with the service's authority. \
         Use the installed copy instead — `sudo {INSTALLED_BINARY} broker …` (put there by \
         `sudo dbcrust broker install-service`). For development only, \
         --allow-untrusted-binary skips this check.",
        exe.display(),
        summarize(violations)
    )
}

/// Enforce the trusted-binary rule for the current executable.
///
/// With `allow_untrusted` a violation becomes a loud warning on stderr.
pub fn enforce_binary_trust(allow_untrusted: bool) -> Result<(), BrokerError> {
    let exe = std::env::current_exe()
        .map_err(|e| BrokerError::State(format!("cannot locate the running binary: {e}")))?;
    let violations = binary_trust_violations(&exe)?;
    if violations.is_empty() {
        return Ok(());
    }
    if allow_untrusted {
        eprintln!(
            "WARNING: --allow-untrusted-binary: running the service-mode broker from {} ({}). \
             Anything that can modify that file runs with this process's authority. Development only.",
            exe.display(),
            summarize(&violations)
        );
        return Ok(());
    }
    Err(BrokerError::Forbidden(untrusted_binary_message(
        &exe,
        &violations,
    )))
}

// ---- users and groups ----------------------------------------------------------

/// Resolve a group name to its gid.
pub fn group_id(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    // SAFETY: getgrnam takes a NUL-terminated name; the result is read
    // immediately, before any other call can reuse the static buffer.
    unsafe {
        let group = libc::getgrnam(c_name.as_ptr());
        if group.is_null() {
            None
        } else {
            Some((*group).gr_gid)
        }
    }
}

/// Resolve a user name to `(uid, primary gid)`.
pub fn user_ids(name: &str) -> Option<(u32, u32)> {
    let c_name = std::ffi::CString::new(name).ok()?;
    // SAFETY: as for getgrnam above.
    unsafe {
        let passwd = libc::getpwnam(c_name.as_ptr());
        if passwd.is_null() {
            None
        } else {
            Some(((*passwd).pw_uid, (*passwd).pw_gid))
        }
    }
}

/// Whether `user` is listed as a member of group `group` in the directory
/// (independent of the current session's credentials).
pub fn user_in_group(user: &str, group: &str) -> bool {
    let Ok(c_group) = std::ffi::CString::new(group) else {
        return false;
    };
    // SAFETY: getgrnam result is read immediately; gr_mem is a NULL-terminated
    // array of NUL-terminated strings.
    unsafe {
        let entry = libc::getgrnam(c_group.as_ptr());
        if entry.is_null() {
            return false;
        }
        let mut member = (*entry).gr_mem;
        if member.is_null() {
            return false;
        }
        while !(*member).is_null() {
            if std::ffi::CStr::from_ptr(*member).to_bytes() == user.as_bytes() {
                return true;
            }
            member = member.add(1);
        }
    }
    false
}

// ---- privilege drop ------------------------------------------------------------

/// The process's credentials, for verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub uid: u32,
    pub euid: u32,
    pub gid: u32,
    pub egid: u32,
    /// Supplementary groups.
    pub groups: Vec<u32>,
}

impl Credentials {
    pub fn current() -> Self {
        // SAFETY: plain credential getters; getgroups gets a buffer sized by
        // a first call with zero length.
        unsafe {
            let count = libc::getgroups(0, std::ptr::null_mut());
            let mut buffer = vec![0 as libc::gid_t; count.max(0) as usize];
            let got = if buffer.is_empty() {
                0
            } else {
                libc::getgroups(buffer.len() as libc::c_int, buffer.as_mut_ptr())
            };
            buffer.truncate(got.max(0) as usize);
            Self {
                uid: libc::getuid(),
                euid: libc::geteuid(),
                gid: libc::getgid(),
                egid: libc::getegid(),
                groups: buffer,
            }
        }
    }
}

/// Check that `creds` is exactly "unprivileged user `uid`, primary group
/// `gid`", without root among the supplementary groups.
pub fn verify_dropped(creds: &Credentials, uid: u32, gid: u32) -> Result<(), String> {
    if creds.uid != uid || creds.euid != uid {
        return Err(format!(
            "uid is {}/{} (real/effective), expected {uid}",
            creds.uid, creds.euid
        ));
    }
    if creds.gid != gid || creds.egid != gid {
        return Err(format!(
            "gid is {}/{} (real/effective), expected {gid}",
            creds.gid, creds.egid
        ));
    }
    if gid != 0 && creds.groups.contains(&0) {
        return Err("still a member of group 0 (wheel/root)".to_string());
    }
    Ok(())
}

/// Whether this process can still become root.
fn can_regain_root() -> bool {
    // SAFETY: plain credential syscalls; an unprivileged process gets EPERM.
    unsafe { libc::setuid(0) == 0 || libc::seteuid(0) == 0 }
}

/// Drop from root to `uid:gid` and prove it.
///
/// `user` (when known) makes the supplementary groups that user's own
/// (`initgroups`); otherwise they are cleared. Order is the only safe one:
/// groups → gid → uid. Afterwards the ids are re-read and `setuid(0)` must
/// fail. Any failure is fatal to the caller: the process may still be root.
pub fn drop_privileges(uid: u32, gid: u32, user: Option<&str>) -> Result<(), BrokerError> {
    if uid == 0 {
        return Err(BrokerError::State(
            "refusing to \"drop\" to uid 0".to_string(),
        ));
    }
    let c_user = match user {
        Some(name) => Some(
            std::ffi::CString::new(name)
                .map_err(|_| BrokerError::State("user name contains NUL".to_string()))?,
        ),
        None => None,
    };
    // SAFETY: plain credential syscalls; every failure is checked and fatal.
    let applied = unsafe {
        let groups_ok = match &c_user {
            Some(name) => libc::initgroups(name.as_ptr(), gid as _) == 0,
            None => libc::setgroups(0, std::ptr::null()) == 0,
        };
        groups_ok && libc::setgid(gid as libc::gid_t) == 0 && libc::setuid(uid as libc::uid_t) == 0
    };
    if !applied {
        return Err(BrokerError::State(format!(
            "could not drop privileges to uid {uid}: {}",
            std::io::Error::last_os_error()
        )));
    }
    verify_dropped(&Credentials::current(), uid, gid).map_err(|why| {
        BrokerError::State(format!(
            "privilege drop to uid {uid} did not take effect ({why}); aborting"
        ))
    })?;
    if can_regain_root() {
        return Err(BrokerError::State(format!(
            "privilege drop to uid {uid} is reversible (setuid(0) succeeded); aborting"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use std::collections::HashMap;

    fn fake_stat(table: &[(&str, u32, u32)]) -> impl Fn(&Path) -> std::io::Result<(u32, u32)> {
        let map: HashMap<PathBuf, (u32, u32)> = table
            .iter()
            .map(|(p, uid, mode)| (PathBuf::from(p), (*uid, *mode)))
            .collect();
        move |path: &Path| {
            map.get(path).copied().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, path.display().to_string())
            })
        }
    }

    fn violations(table: &[(&str, u32, u32)], exe: &str) -> Vec<String> {
        let chain = ownership_chain(Path::new(exe), fake_stat(table)).unwrap();
        ownership_violations(&chain)
    }

    const INSTALLED: &[(&str, u32, u32)] = &[
        ("/usr/local/libexec/dbcrust-broker", 0, 0o755),
        ("/usr/local/libexec", 0, 0o755),
        ("/usr/local", 0, 0o755),
        ("/usr", 0, 0o755),
        ("/", 0, 0o755),
    ];

    #[test]
    fn a_root_owned_chain_is_trusted() {
        assert!(violations(INSTALLED, "/usr/local/libexec/dbcrust-broker").is_empty());
    }

    #[test]
    fn root_wheel_group_ownership_is_fine_when_not_group_writable() {
        // group ownership is not part of the check at all; 0o755 vs 0o775 is
        let mut table = INSTALLED.to_vec();
        table[1] = ("/usr/local/libexec", 0, 0o750);
        assert!(violations(&table, "/usr/local/libexec/dbcrust-broker").is_empty());
    }

    #[rstest]
    #[case::user_owned_file(
        "/usr/local/libexec/dbcrust-broker",
        Some(501),
        None,
        "owned by uid 501"
    )]
    #[case::user_owned_parent("/usr/local/libexec", Some(501), None, "owned by uid 501")]
    #[case::homebrew_style_usr_local("/usr/local", Some(501), None, "owned by uid 501")]
    #[case::group_writable_file(
        "/usr/local/libexec/dbcrust-broker",
        None,
        Some(0o775),
        "group-writable"
    )]
    #[case::world_writable_parent("/usr/local/libexec", None, Some(0o757), "world-writable")]
    #[case::group_writable_root("/", None, Some(0o775), "group-writable")]
    fn untrusted_chains_are_refused(
        #[case] target: &str,
        #[case] uid: Option<u32>,
        #[case] mode: Option<u32>,
        #[case] expect: &str,
    ) {
        let table: Vec<(&str, u32, u32)> = INSTALLED
            .iter()
            .map(|&(path, u, m)| {
                if path == target {
                    (path, uid.unwrap_or(u), mode.unwrap_or(m))
                } else {
                    (path, u, m)
                }
            })
            .collect();
        let found = violations(&table, "/usr/local/libexec/dbcrust-broker");
        assert!(
            found
                .iter()
                .any(|v| v.contains(expect) && v.contains(target)),
            "{found:?}"
        );
    }

    #[test]
    fn cargo_bin_in_a_home_directory_is_refused_with_the_remedy() {
        let table = [
            ("/Users/me/.cargo/bin/dbcrust", 501, 0o755),
            ("/Users/me/.cargo/bin", 501, 0o755),
            ("/Users/me/.cargo", 501, 0o755),
            ("/Users/me", 501, 0o750),
            ("/Users", 0, 0o755),
            ("/", 0, 0o755),
        ];
        let found = violations(&table, "/Users/me/.cargo/bin/dbcrust");
        assert_eq!(found.len(), 4);
        let message = untrusted_binary_message(Path::new("/Users/me/.cargo/bin/dbcrust"), &found);
        assert!(message.contains("(and 1 more)"), "{message}");
        assert!(message.contains(INSTALLED_BINARY));
        assert!(message.contains("--allow-untrusted-binary"));
    }

    #[test]
    fn a_missing_component_is_an_error_not_a_pass() {
        let table = [("/a/b", 0, 0o755)];
        assert!(ownership_chain(Path::new("/a/b"), fake_stat(&table)).is_err());
    }

    #[test]
    fn real_file_system_check_flags_a_user_owned_temp_binary() {
        // Never root in tests: a temp binary is user-owned, hence untrusted
        if effective_uid_is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("dbcrust");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        let found = binary_trust_violations(&exe).unwrap();
        assert!(found.iter().any(|v| v.contains("not root")), "{found:?}");
    }

    fn effective_uid_is_root() -> bool {
        // SAFETY: geteuid cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    fn creds(uid: u32, euid: u32, gid: u32, egid: u32, groups: &[u32]) -> Credentials {
        Credentials {
            uid,
            euid,
            gid,
            egid,
            groups: groups.to_vec(),
        }
    }

    #[rstest]
    #[case::clean(creds(300, 300, 300, 300, &[300]), true)]
    #[case::empty_groups(creds(300, 300, 300, 300, &[]), true)]
    #[case::saved_euid_root(creds(300, 0, 300, 300, &[]), false)]
    #[case::real_uid_root(creds(0, 300, 300, 300, &[]), false)]
    #[case::wrong_uid(creds(301, 301, 300, 300, &[]), false)]
    #[case::egid_root(creds(300, 300, 300, 0, &[]), false)]
    #[case::gid_root(creds(300, 300, 0, 300, &[]), false)]
    #[case::wheel_left_over(creds(300, 300, 300, 300, &[300, 0]), false)]
    fn dropped_credentials_are_verified(#[case] actual: Credentials, #[case] ok: bool) {
        assert_eq!(verify_dropped(&actual, 300, 300).is_ok(), ok);
    }

    #[test]
    fn dropping_to_root_is_refused() {
        assert!(drop_privileges(0, 0, None).is_err());
    }

    #[test]
    fn current_credentials_are_self_consistent() {
        let now = Credentials::current();
        // SAFETY: getuid cannot fail.
        assert_eq!(now.uid, unsafe { libc::getuid() });
    }
}
