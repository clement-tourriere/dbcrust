//! Append-only JSON-lines audit log: `<state>/audit.jsonl`.
//!
//! One record per decision. Requests that will touch the database write a
//! `phase: "start"` line *before* executing — if that write fails the request
//! is refused (`internal`, fail closed) — and a `phase: "end"` line with rows,
//! bytes, truncation and duration afterwards. Requests refused before
//! execution (unknown alias, classifier refusal, …) write a single `end`
//! line. The log never contains SQL text, parameters or result data: only a
//! SHA-256 of the statement and its length.
//!
//! Rotation is size based: `audit.jsonl` plus four rotated files
//! (`audit.jsonl.1` … `.4`), 10 MiB each, all created 0600.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const KEEP_FILES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Start,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allowed,
    Refused,
    Error,
}

/// One audit line. Field names are the on-disk schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRecord {
    pub ts: String,
    /// Monotonic per broker run; ties a `start` line to its `end` line.
    pub seq: u64,
    pub phase: Phase,
    pub client_label: String,
    pub peer_uid: Option<u32>,
    pub peer_pid: Option<i32>,
    pub alias: Option<String>,
    pub op: String,
    pub decision: Decision,
    pub code: Option<String>,
    pub sql_sha256: Option<String>,
    pub sql_len: Option<usize>,
    pub rows: Option<u64>,
    pub bytes: Option<u64>,
    pub truncated: Option<bool>,
    pub duration_ms: Option<u64>,
}

/// SHA-256 (hex) of the statement text.
pub fn sql_digest(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

pub struct AuditLog {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    state: Mutex<State>,
    seq: AtomicU64,
}

struct State {
    file: Option<File>,
    size: u64,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_limits(path, MAX_FILE_BYTES, KEEP_FILES)
    }

    pub fn with_limits(path: impl Into<PathBuf>, max_bytes: u64, keep: usize) -> Self {
        Self {
            path: path.into(),
            max_bytes,
            keep: keep.max(1),
            state: Mutex::new(State {
                file: None,
                size: 0,
            }),
            seq: AtomicU64::new(1),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The next record sequence number.
    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn open(&self) -> io::Result<(File, u64)> {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            if !parent.exists() {
                dbcrust_secrets::fsutil::create_private_dir(parent)
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&self.path)?;
        let size = file.metadata()?.len();
        Ok((file, size))
    }

    fn rotate(&self, state: &mut State) -> io::Result<()> {
        state.file = None;
        // audit.jsonl.(keep-1) is dropped; the rest shift up by one
        let last = self.keep - 1;
        if last >= 1 {
            let _ = std::fs::remove_file(self.rotated(last));
            for n in (1..last).rev() {
                let from = self.rotated(n);
                if from.exists() {
                    std::fs::rename(&from, self.rotated(n + 1))?;
                }
            }
            if self.path.exists() {
                std::fs::rename(&self.path, self.rotated(1))?;
            }
        } else if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Append one record. `durable` additionally syncs the data to disk
    /// (used for `start` lines, which gate execution).
    pub fn append(&self, record: &AuditRecord, durable: bool) -> io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.file.is_none() {
            let (file, size) = self.open()?;
            state.file = Some(file);
            state.size = size;
        }
        if state.size > 0 && state.size + line.len() as u64 > self.max_bytes {
            self.rotate(&mut state)?;
            let (file, size) = self.open()?;
            state.file = Some(file);
            state.size = size;
        }
        let write = {
            let file = state
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("audit file not open"))?;
            file.write_all(&line).and_then(|_| {
                if durable {
                    file.sync_data()
                } else {
                    file.flush()
                }
            })
        };
        match write {
            Ok(()) => {
                state.size += line.len() as u64;
                Ok(())
            }
            Err(e) => {
                // Reopen next time: the handle may be stale or the disk full
                state.file = None;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(seq: u64, phase: Phase) -> AuditRecord {
        AuditRecord {
            ts: "2026-01-01T00:00:00Z".to_string(),
            seq,
            phase,
            client_label: "tests".to_string(),
            peer_uid: Some(501),
            peer_pid: Some(42),
            alias: Some("prod".to_string()),
            op: "query".to_string(),
            decision: Decision::Allowed,
            code: None,
            sql_sha256: Some(sql_digest("SELECT 1")),
            sql_len: Some(8),
            rows: None,
            bytes: None,
            truncated: None,
            duration_ms: None,
        }
    }

    #[test]
    fn lines_are_json_with_the_documented_fields_and_no_sql() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        log.append(&record(1, Phase::Start), true).unwrap();
        let mut end = record(1, Phase::End);
        end.rows = Some(3);
        end.bytes = Some(120);
        end.truncated = Some(false);
        end.duration_ms = Some(5);
        log.append(&end, false).unwrap();

        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        for key in [
            "ts",
            "client_label",
            "peer_uid",
            "peer_pid",
            "alias",
            "op",
            "decision",
            "code",
            "sql_sha256",
            "sql_len",
            "rows",
            "bytes",
            "truncated",
            "duration_ms",
            "phase",
            "seq",
        ] {
            assert!(first.get(key).is_some(), "missing {key}");
        }
        assert_eq!(first["phase"], "start");
        assert_eq!(first["decision"], "allowed");
        assert!(!text.contains("SELECT"), "raw SQL must never be logged");
        let back: AuditRecord = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(back, end);
    }

    #[test]
    fn digest_is_a_plain_sha256_of_the_statement() {
        assert_eq!(
            sql_digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn rotation_keeps_five_files_and_drops_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::with_limits(dir.path().join("audit.jsonl"), 600, 5);
        let probe = serde_json::to_vec(&record(1, Phase::End)).unwrap().len() as u64 + 1;
        let per_file = 600 / probe;
        assert!(per_file >= 1);
        // enough lines to force more than five rotations
        for seq in 0..(per_file * 8) {
            log.append(&record(seq, Phase::End), false).unwrap();
        }
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "audit.jsonl",
                "audit.jsonl.1",
                "audit.jsonl.2",
                "audit.jsonl.3",
                "audit.jsonl.4"
            ]
        );
        for name in names {
            let len = std::fs::metadata(dir.path().join(&name)).unwrap().len();
            assert!(len <= 600, "{name} is {len} bytes");
        }
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        log.append(&record(1, Phase::End), false).unwrap();
        let mode = std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn unwritable_location_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // The parent of the log path is a regular file
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let log = AuditLog::new(blocker.join("audit.jsonl"));
        assert!(log.append(&record(1, Phase::Start), true).is_err());
    }
}
