//! Process environment for a desktop launch.
//!
//! Apps started from Finder/Dock/launchers inherit a minimal `PATH`, so SSH
//! tunnel patterns using command substitution (`jq`, `owl`, …), password
//! commands, and Vault credential processes fail to find their tools. The
//! login shell's `PATH` is imported once at startup, with a timeout so a slow
//! or interactive profile cannot hang the launch.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `command`, returning its stdout if it exits successfully within
/// `timeout`. The child is killed on timeout. Stdin is closed so nothing can
/// prompt.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // On timeout/failure the reader is left detached: a grandchild may still
    // hold the pipe open, and joining would block on it.
    match status {
        Some(status) if status.success() => reader.join().ok(),
        _ => None,
    }
}

/// Import the login shell's `PATH` (macOS/Linux). Must run before any other
/// thread is spawned: it mutates the process environment.
pub fn import_login_shell_path() {
    if cfg!(windows) {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| default_shell().to_string());
    // fish keeps PATH as a list
    let print_path = if shell.contains("fish") {
        "string join : $PATH"
    } else {
        "printf '%s' \"$PATH\""
    };
    let Some(output) = output_with_timeout(
        Command::new(&shell).args(["-l", "-c", print_path]),
        Duration::from_secs(3),
    ) else {
        tracing::warn!("could not read PATH from login shell {shell}");
        return;
    };
    let path = String::from_utf8_lossy(&output).trim().to_string();
    if !path.is_empty() {
        // SAFETY: documented precondition — called at startup before any
        // other thread exists.
        unsafe { std::env::set_var("PATH", path) };
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
}
