//! Process environment for a desktop launch.
//!
//! Apps started from Finder/Dock/launchers inherit a minimal `PATH`, so SSH
//! tunnel patterns using command substitution (`jq`, `owl`, …), password
//! commands, and Vault credential processes fail to find their tools. The
//! login shell's `PATH` is imported once at startup, with a timeout so a slow
//! or interactive profile cannot hang the launch.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Run `command`, returning its stdout if it exits successfully within
/// `timeout`. The child is killed on timeout. Stdin is closed so nothing can
/// prompt.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    spawn_with_timeout(command, timeout)?.wait()
}

/// A command started by [`spawn_with_timeout`], running while the caller
/// does something else.
pub struct PendingOutput {
    child: Child,
    reader: JoinHandle<Vec<u8>>,
    deadline: Instant,
}

/// Start `command` (stdin closed, stdout captured) without waiting for it;
/// [`PendingOutput::wait`] collects its stdout. `timeout` counts from now.
pub fn spawn_with_timeout(command: &mut Command, timeout: Duration) -> Option<PendingOutput> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    Some(PendingOutput {
        child,
        reader,
        deadline,
    })
}

impl PendingOutput {
    /// The command's stdout if it exited successfully before the deadline;
    /// otherwise it is killed and `None` is returned.
    pub fn wait(mut self) -> Option<Vec<u8>> {
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < self.deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Ok(None) | Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break None;
                }
            }
        };
        // On timeout/failure the reader is left detached: a grandchild may still
        // hold the pipe open, and joining would block on it.
        match status {
            Some(status) if status.success() => self.reader.join().ok(),
            _ => None,
        }
    }
}

/// Import the login shell's `PATH` (macOS/Linux). Must run before any other
/// thread is spawned: it mutates the process environment.
pub fn import_login_shell_path() {
    start_login_shell_path().apply();
}

/// The login shell's `PATH`, being read (see [`start_login_shell_path`]).
pub struct LoginShellPath {
    shell: String,
    pending: Option<PendingOutput>,
}

/// Start reading the login shell's `PATH` and return at once, so the caller
/// can do its own startup work (reading config files) while the shell runs
/// its profile. [`LoginShellPath::apply`] waits for it (bounded) and sets it.
pub fn start_login_shell_path() -> LoginShellPath {
    if cfg!(windows) {
        return LoginShellPath {
            shell: String::new(),
            pending: None,
        };
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| default_shell().to_string());
    // fish keeps PATH as a list
    let print_path = if shell.contains("fish") {
        "string join : $PATH"
    } else {
        "printf '%s' \"$PATH\""
    };
    let pending = spawn_with_timeout(
        Command::new(&shell).args(["-l", "-c", print_path]),
        Duration::from_secs(3),
    );
    LoginShellPath { shell, pending }
}

impl LoginShellPath {
    /// Wait for the shell (until 3 s after it started) and import its
    /// `PATH`. Must run before any other thread that reads the environment
    /// exists: it mutates the process environment.
    pub fn apply(self) {
        if cfg!(windows) {
            return;
        }
        let Some(output) = self.pending.and_then(PendingOutput::wait) else {
            tracing::warn!("could not read PATH from login shell {}", self.shell);
            return;
        };
        let path = String::from_utf8_lossy(&output).trim().to_string();
        if !path.is_empty() {
            // SAFETY: documented precondition — called at startup before any
            // other thread that reads the environment exists (the only one
            // alive reads the shell's stdout).
            unsafe { std::env::set_var("PATH", path) };
        }
    }
}

fn default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/sh"
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout_and_enforces_timeout() {
        let out = output_with_timeout(
            Command::new("/bin/sh").args(["-c", "printf hello"]),
            Duration::from_secs(5),
        );
        assert_eq!(out.as_deref(), Some(&b"hello"[..]));

        let started = Instant::now();
        let slow = output_with_timeout(
            Command::new("/bin/sh").args(["-c", "sleep 5"]),
            Duration::from_millis(100),
        );
        assert!(slow.is_none());
        assert!(started.elapsed() < Duration::from_secs(3));

        let failing = output_with_timeout(
            Command::new("/bin/sh").args(["-c", "exit 3"]),
            Duration::from_secs(5),
        );
        assert!(failing.is_none());
    }

    #[test]
    fn a_started_command_runs_while_the_caller_works() {
        let started = Instant::now();
        let pending = spawn_with_timeout(
            Command::new("/bin/sh").args(["-c", "sleep 0.3; printf done"]),
            Duration::from_secs(5),
        )
        .expect("spawned");
        // Spawning does not wait for the command
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(pending.wait().as_deref(), Some(&b"done"[..]));

        // The deadline counts from the start, not from `wait`
        let pending = spawn_with_timeout(
            Command::new("/bin/sh").args(["-c", "sleep 5"]),
            Duration::from_millis(100),
        )
        .expect("spawned");
        std::thread::sleep(Duration::from_millis(150));
        let waited = Instant::now();
        assert!(pending.wait().is_none());
        assert!(waited.elapsed() < Duration::from_secs(1));
    }
}
