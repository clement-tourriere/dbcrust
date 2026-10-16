//! `dbcrust secrets audit`: a read-only scan of the places DBCrust (and the
//! tools it reads from) may leave credentials behind.
//!
//! Guarantees, each covered by a test:
//!
//! * Findings carry a rule id, a path, an optional line number, a severity
//!   and a remediation — **never** the matched value. Messages are fixed
//!   text per rule.
//! * Only known locations are read; symlinks are never followed; files over
//!   [`MAX_SCAN_BYTES`] are skipped (and reported).
//! * Nothing is executed: password commands, shells and helpers are never
//!   run, and `enc:` values are classified by their prefix, not decrypted.

use crate::secrets::StoreStatus;
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Largest file the audit will read.
pub const MAX_SCAN_BYTES: u64 = 10 * 1024 * 1024;
/// Most history files scanned (they can pile up per session).
const MAX_HISTORY_FILES: usize = 200;

/// How urgent a finding is. Declared most severe first, so sorting puts the
/// worst findings at the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
    Info,
}

impl Severity {
    fn label(&self) -> &'static str {
        match self {
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
            Severity::Info => "INFO",
        }
    }
}

/// One audit result. Contains no secret value.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: Severity,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub message: String,
    pub remediation: String,
}

/// Every location the audit may look at, injectable for tests.
#[derive(Debug, Clone)]
pub struct AuditContext {
    pub config_dir: PathBuf,
    /// `~/.dbcrust` (or `$DBCRUST_PASSFILE`).
    pub dbcrust_pass: Option<PathBuf>,
    pub pgpass: Option<PathBuf>,
    pub mysql_config: Option<PathBuf>,
    pub vault_token: Option<PathBuf>,
    /// Probe the old `dbcrust` keychain entries (existence only).
    pub probe_legacy_keychain: bool,
}

impl AuditContext {
    /// The real locations of the current user.
    pub fn from_environment() -> Option<Self> {
        let config_dir = crate::config::Config::get_config_directory().ok()?;
        let home = dirs::home_dir();
        Some(Self {
            config_dir,
            dbcrust_pass: crate::dbcrust_pass::get_dbcrust_pass_path(),
            pgpass: crate::pgpass::get_pgpass_path(),
            mysql_config: home.as_ref().map(|h| h.join(".my.cnf")),
            vault_token: home.map(|h| h.join(".vault-token")),
            probe_legacy_keychain: !crate::config::is_test_mode(),
        })
    }
}

/// The complete result of one audit run.
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub findings: Vec<Finding>,
    /// Paths that were read (for transparency; no contents).
    pub scanned: Vec<String>,
}

impl AuditReport {
    pub fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }

    /// Exit code: 0 when nothing high was found, 1 otherwise.
    pub fn exit_code(&self) -> i32 {
        i32::from(self.count(Severity::High) > 0)
    }

    pub fn render_json(&self) -> String {
        #[derive(Serialize)]
        struct Summary {
            high: usize,
            medium: usize,
            low: usize,
            info: usize,
        }
        #[derive(Serialize)]
        struct Out<'a> {
            ok: bool,
            summary: Summary,
            findings: &'a [Finding],
            scanned: &'a [String],
        }
        serde_json::to_string_pretty(&Out {
            ok: self.exit_code() == 0,
            summary: Summary {
                high: self.count(Severity::High),
                medium: self.count(Severity::Medium),
                low: self.count(Severity::Low),
                info: self.count(Severity::Info),
            },
            findings: &self.findings,
            scanned: &self.scanned,
        })
        .unwrap_or_else(|_| "{}".to_string())
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        if self.findings.is_empty() {
            out.push_str("No credential findings in the locations DBCrust knows about.\n");
        }
        for finding in &self.findings {
            let location = match finding.line {
                Some(line) => format!("{}:{line}", finding.path),
                None => finding.path.clone(),
            };
            out.push_str(&format!(
                "{:<6} {}  {}\n       {}\n       fix: {}\n",
                finding.severity.label(),
                finding.rule,
                location,
                finding.message,
                finding.remediation
            ));
        }
        out.push_str(&format!(
            "\n{} high, {} medium, {} low, {} info ({} location(s) scanned)\n",
            self.count(Severity::High),
            self.count(Severity::Medium),
            self.count(Severity::Low),
            self.count(Severity::Info),
            self.scanned.len()
        ));
        out
    }
}

static URL_USERINFO_PASSWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"[A-Za-z][A-Za-z0-9+.\-]*://[^/\s:@'"]+:[^/\s@'"]+@"#).expect("valid regex")
});
static TOML_STRING_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*"?([A-Za-z0-9_.\-]+)"?\s*=\s*"(.*)"\s*(#.*)?$"#).expect("valid regex")
});
static SQL_PASSWORD_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bpassword\s*(=|\s)\s*'[^']+'"#).expect("valid regex"));
static TUNNEL_USERINFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"=\s*"[^":@\s]+:[^"@\s]+@"#).expect("valid regex"));

struct Auditor {
    findings: Vec<Finding>,
    scanned: Vec<String>,
}

impl Auditor {
    fn push(
        &mut self,
        rule: &'static str,
        severity: Severity,
        path: &Path,
        line: Option<usize>,
        message: &str,
        remediation: &str,
    ) {
        self.findings.push(Finding {
            rule,
            severity,
            path: path.display().to_string(),
            line,
            message: message.to_string(),
            remediation: remediation.to_string(),
        });
    }

    /// Read a text file for scanning: absent → `None`; symlink or oversized
    /// → reported and skipped.
    fn read_text(&mut self, path: &Path) -> Option<String> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(_) => return None,
        };
        if meta.file_type().is_symlink() {
            self.push(
                "audit.skipped-symlink",
                Severity::Info,
                path,
                None,
                "Skipped: the path is a symbolic link and audit never follows links.",
                "Audit the link target directly if it is a DBCrust file.",
            );
            return None;
        }
        if !meta.is_file() {
            return None;
        }
        if meta.len() > MAX_SCAN_BYTES {
            self.push(
                "audit.skipped-large",
                Severity::Info,
                path,
                None,
                "Skipped: the file is larger than the 10 MiB scan limit.",
                "Inspect it manually.",
            );
            return None;
        }
        let bytes = fs::read(path).ok()?;
        self.scanned.push(path.display().to_string());
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[cfg(unix)]
    fn check_permissions(
        &mut self,
        path: &Path,
        rule: &'static str,
        severity: Severity,
        what: &str,
    ) {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = fs::symlink_metadata(path) else {
            return;
        };
        if meta.file_type().is_symlink() {
            return;
        }
        if meta.permissions().mode() & 0o077 != 0 {
            self.push(
                rule,
                severity,
                path,
                None,
                &format!(
                    "{what} is readable by other users (mode {:03o}).",
                    meta.permissions().mode() & 0o777
                ),
                "chmod 600 the file.",
            );
        }
    }

    #[cfg(not(unix))]
    fn check_permissions(&mut self, _: &Path, _: &'static str, _: Severity, _: &str) {}

    fn exists_no_follow(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    // --- DBCrust-owned legacy files -------------------------------------

    fn audit_dbcrust_pass(&mut self, path: &Path) {
        use crate::dbcrust_pass::{LineShape, scan_line_shapes};
        if !Self::exists_no_follow(path) {
            return;
        }
        let shapes = match scan_line_shapes(path) {
            Ok(shapes) => shapes,
            Err(_) => {
                self.push(
                    "audit.skipped-symlink",
                    Severity::Info,
                    path,
                    None,
                    "Skipped: the legacy password file is a symbolic link or could not be read.",
                    "Audit the link target directly.",
                );
                return;
            }
        };
        self.scanned.push(path.display().to_string());
        self.check_permissions(
            path,
            "legacy.dbcrust.permissions",
            Severity::High,
            "The legacy password file",
        );
        for (line, shape) in shapes {
            match shape {
                LineShape::Plaintext { wildcard } => {
                    self.push(
                        "legacy.dbcrust.plaintext",
                        Severity::High,
                        path,
                        Some(line),
                        "A password is stored in plain text in the legacy ~/.dbcrust file.",
                        "Run `dbcrust secrets migrate` to move it into the secure store.",
                    );
                    if wildcard {
                        self.wildcard(path, line);
                    }
                }
                LineShape::MachineKey { wildcard } => {
                    self.push(
                        "legacy.dbcrust.machine-key",
                        Severity::Medium,
                        path,
                        Some(line),
                        "A password is \"encrypted\" with a machine-derived key that any process of your user can re-derive.",
                        "Run `dbcrust secrets migrate` to move it into the secure store.",
                    );
                    if wildcard {
                        self.wildcard(path, line);
                    }
                }
                LineShape::Malformed => self.push(
                    "legacy.dbcrust.unreadable",
                    Severity::Low,
                    path,
                    Some(line),
                    "A line of the legacy password file is not a usable entry.",
                    "Fix or delete the line; migrate keeps it untouched.",
                ),
                LineShape::Ignorable => {}
            }
        }
    }

    fn wildcard(&mut self, path: &Path, line: usize) {
        self.push(
            "legacy.dbcrust.wildcard",
            Severity::Medium,
            path,
            Some(line),
            "A wildcard entry hands this password to every matching host, port, database or user.",
            "Bind it to a concrete target with `dbcrust secrets migrate`.",
        );
    }

    fn audit_ai_keys(&mut self, path: &Path) {
        let Some(text) = self.read_text(path) else {
            return;
        };
        self.check_permissions(
            path,
            "legacy.ai_keys.permissions",
            Severity::High,
            "The legacy AI key file",
        );
        for (index, line) in text.lines().enumerate() {
            let Some((_, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if value.starts_with("enc:") {
                self.push(
                    "legacy.ai_keys.machine-key",
                    Severity::Medium,
                    path,
                    Some(index + 1),
                    "An AI key is \"encrypted\" with a machine-derived key that any process of your user can re-derive.",
                    "Run `dbcrust secrets migrate`.",
                );
            } else {
                self.push(
                    "legacy.ai_keys.plaintext",
                    Severity::High,
                    path,
                    Some(index + 1),
                    "An AI key is stored in plain text.",
                    "Run `dbcrust secrets migrate`.",
                );
            }
        }
    }

    fn audit_legacy_keychain(&mut self) {
        for name in crate::ai::key_storage::legacy::keychain_account_names() {
            if crate::ai::key_storage::legacy::keychain_entry_exists(&name) {
                self.findings.push(Finding {
                    rule: "legacy.keychain.ai",
                    severity: Severity::Low,
                    path: format!("keychain:dbcrust/{name}"),
                    line: None,
                    message: "An AI key or token sits in the old, unscoped keychain entry."
                        .to_string(),
                    remediation: "Run `dbcrust secrets migrate`.".to_string(),
                });
            }
        }
    }

    // --- TOML / text stores ------------------------------------------------

    /// `sessions.toml` / `recent.toml`: URL passwords, secret-looking option
    /// values and SSH passwords.
    fn audit_connection_toml(&mut self, path: &Path) {
        let Some(text) = self.read_text(path) else {
            return;
        };
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            if URL_USERINFO_PASSWORD.is_match(line) {
                self.push(
                    "storage.url-password",
                    Severity::High,
                    path,
                    Some(number),
                    "A connection URL with an embedded password is stored.",
                    "Re-save the connection (passwords are kept in the secure store) and run `dbcrust secrets migrate`.",
                );
            }
            if let Some(captures) = TOML_STRING_ASSIGN.captures(line) {
                let key = &captures[1];
                let value = &captures[2];
                if key == "ssh_password" {
                    if !value.is_empty() {
                        self.push(
                            "storage.ssh-password",
                            Severity::High,
                            path,
                            Some(number),
                            "An SSH password is stored in plain text.",
                            "Run `dbcrust secrets migrate`; use SSH keys instead.",
                        );
                    }
                } else if crate::password_sanitizer::is_secret_option_key(key) && !value.is_empty()
                {
                    self.push(
                        "storage.secret-option",
                        Severity::High,
                        path,
                        Some(number),
                        "A secret-looking connection option value is stored in plain text.",
                        "Run `dbcrust secrets migrate`.",
                    );
                }
            }
        }
    }

    fn audit_config_toml(&mut self, path: &Path) {
        let Some(text) = self.read_text(path) else {
            return;
        };
        let mut in_tunnels = false;
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_tunnels = trimmed == "[ssh_tunnel_patterns]";
                continue;
            }
            if in_tunnels && TUNNEL_USERINFO.is_match(line) {
                self.push(
                    "config.ssh-tunnel-password",
                    Severity::High,
                    path,
                    Some(index + 1),
                    "An ssh_tunnel_patterns entry embeds a password (user:pass@host).",
                    "Use an SSH key or agent and remove the password from the pattern.",
                );
            }
        }
    }

    /// History files and named queries: pasted URLs and SQL password literals.
    fn audit_query_text(&mut self, path: &Path, source: &'static str) {
        let Some(text) = self.read_text(path) else {
            return;
        };
        let (url_rule, sql_rule) = if source == "history" {
            ("history.url-password", "history.sql-password")
        } else {
            ("named-queries.url-password", "named-queries.sql-password")
        };
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            if URL_USERINFO_PASSWORD.is_match(line) {
                self.push(
                    url_rule,
                    Severity::Medium,
                    path,
                    Some(number),
                    "A connection URL with an embedded password appears in saved input.",
                    "Delete the entry and rotate the password if the file was ever shared or backed up.",
                );
            }
            if SQL_PASSWORD_LITERAL.is_match(line) {
                self.push(
                    sql_rule,
                    Severity::Medium,
                    path,
                    Some(number),
                    "A SQL statement with a password literal appears in saved input.",
                    "Delete the entry and rotate the password if the file was ever shared or backed up.",
                );
            }
        }
    }

    // --- third-party files -------------------------------------------------

    fn audit_third_party(
        &mut self,
        path: &Path,
        present_rule: &'static str,
        permission_rule: &'static str,
        what: &str,
    ) {
        if !Self::exists_no_follow(path) {
            return;
        }
        self.scanned.push(path.display().to_string());
        self.push(
            present_rule,
            Severity::Info,
            path,
            None,
            &format!("{what} exists. DBCrust reads it but never writes or migrates it."),
            "Keep it at mode 600, or remove it if you moved to the secure store.",
        );
        self.check_permissions(path, permission_rule, Severity::High, what);
    }

    fn audit_store(&mut self, status: &StoreStatus, config_dir: &Path) {
        use crate::secrets::BackendState;
        let path = config_dir.join("secrets_index.toml");
        if status.backend.state == BackendState::Unavailable {
            self.push(
                "store.unavailable",
                Severity::Medium,
                &path,
                None,
                "The secret store is unavailable, so saved passwords cannot be read or written.",
                "Run `dbcrust secrets status`.",
            );
        }
        if !status.drift.is_clean() {
            self.push(
                "store.index-drift",
                Severity::Low,
                &path,
                None,
                "The secrets index and the store disagree about which records exist.",
                "Run `dbcrust secrets status` for the ids involved.",
            );
        }
    }
}

/// Scan the locations described by `context` (plus the store health, when
/// given). Read-only.
pub fn run_audit(context: &AuditContext, store: Option<&StoreStatus>) -> AuditReport {
    let mut auditor = Auditor {
        findings: Vec::new(),
        scanned: Vec::new(),
    };

    if let Some(path) = &context.dbcrust_pass {
        auditor.audit_dbcrust_pass(path);
    }
    let config = &context.config_dir;
    auditor.audit_ai_keys(&config.join("ai_keys.enc"));
    if context.probe_legacy_keychain {
        auditor.audit_legacy_keychain();
    }
    auditor.audit_connection_toml(&config.join("sessions.toml"));
    auditor.audit_connection_toml(&config.join("recent.toml"));
    auditor.audit_config_toml(&config.join("config.toml"));
    auditor.audit_query_text(&config.join("named_queries.toml"), "named");

    if let Ok(entries) = fs::read_dir(config) {
        let mut history: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == "history" || name.starts_with("history_"))
            })
            .collect();
        history.sort();
        for path in history.into_iter().take(MAX_HISTORY_FILES) {
            auditor.audit_query_text(&path, "history");
        }
    }

    if let Some(path) = &context.pgpass {
        auditor.audit_third_party(
            path,
            "third-party.pgpass.present",
            "third-party.pgpass.permissions",
            "The PostgreSQL password file",
        );
    }
    if let Some(path) = &context.mysql_config {
        auditor.audit_third_party(
            path,
            "third-party.mycnf.present",
            "third-party.mycnf.permissions",
            "The MySQL option file",
        );
    }
    if let Some(path) = &context.vault_token {
        auditor.audit_third_party(
            path,
            "third-party.vault-token.present",
            "third-party.vault-token.permissions",
            "The Vault token file",
        );
    }
    if let Some(status) = store {
        auditor.audit_store(status, config);
    }

    auditor.findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.rule.cmp(b.rule))
    });
    AuditReport {
        findings: auditor.findings,
        scanned: auditor.scanned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANARIES: [&str; 6] = [
        "CANARY-DBCRUST-PLAIN",
        "CANARY-SESSION-OPT",
        "CANARY-URL-PW",
        "CANARY-SSH-PW",
        "CANARY-TUNNEL-PW",
        "CANARY-HISTORY-PW",
    ];

    fn fixture() -> (tempfile::TempDir, AuditContext) {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        fs::create_dir_all(&config).unwrap();
        let legacy_enc = crate::password_encryption::legacy_encrypt_for_tests("CANARY-ENC-VALUE");

        let dbcrust_pass = dir.path().join(".dbcrust");
        fs::write(
            &dbcrust_pass,
            format!(
                "# comment\npostgresql:db.example.com:5432:app:alice:CANARY-DBCRUST-PLAIN\n\
                 mysql\\:x:1:2:3:4\nmysql:*:3306:app:bob:{}\ngarbage\n",
                legacy_enc.replace(':', "\\:")
            ),
        )
        .unwrap();
        fs::write(
            config.join("sessions.toml"),
            "[sessions.prod]\nhost = \"db\"\n[sessions.prod.options]\nsslpassword = \"CANARY-SESSION-OPT\"\nsslmode = \"require\"\n\
             [sessions.prod.ssh_tunnel]\nssh_password = \"CANARY-SSH-PW\"\n",
        )
        .unwrap();
        fs::write(
            config.join("recent.toml"),
            "[[connections]]\nconnection_url = \"postgres://u:CANARY-URL-PW@h:5432/d\"\n",
        )
        .unwrap();
        fs::write(
            config.join("config.toml"),
            "default_limit = 100\n[ssh_tunnel_patterns]\n\"^db\\\\..*$\" = \"user:CANARY-TUNNEL-PW@jump.example.com:22\"\n[logging]\nlevel = \"info\"\n",
        )
        .unwrap();
        fs::write(
            config.join("history_default"),
            "select 1;\nALTER USER bob PASSWORD 'CANARY-HISTORY-PW';\n\\c postgres://u:CANARY-HISTORY-PW@h/d\n",
        )
        .unwrap();
        fs::write(
            config.join("named_queries.toml"),
            "[queries.\"global::q\"]\nquery = \"CREATE ROLE x PASSWORD 'CANARY-HISTORY-PW'\"\n",
        )
        .unwrap();
        fs::write(
            config.join("ai_keys.enc"),
            "openai_api_key=sk-CANARY-PLAIN-AI\nanthropic_api_key=enc:00ff\n",
        )
        .unwrap();
        let pgpass = dir.path().join(".pgpass");
        fs::write(&pgpass, "h:5432:d:u:CANARY-PGPASS\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&pgpass, fs::Permissions::from_mode(0o644)).unwrap();
            fs::set_permissions(&dbcrust_pass, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let context = AuditContext {
            config_dir: config,
            dbcrust_pass: Some(dbcrust_pass),
            pgpass: Some(pgpass),
            mysql_config: Some(dir.path().join(".my.cnf")),
            vault_token: Some(dir.path().join(".vault-token")),
            probe_legacy_keychain: false,
        };
        (dir, context)
    }

    #[test]
    fn findings_carry_rule_path_line_and_remediation() {
        let (_dir, context) = fixture();
        let report = run_audit(&context, None);
        let rules: Vec<&str> = report.findings.iter().map(|f| f.rule).collect();
        for expected in [
            "legacy.dbcrust.plaintext",
            "legacy.dbcrust.machine-key",
            "legacy.dbcrust.wildcard",
            "legacy.dbcrust.unreadable",
            "legacy.ai_keys.plaintext",
            "legacy.ai_keys.machine-key",
            "storage.secret-option",
            "storage.ssh-password",
            "storage.url-password",
            "config.ssh-tunnel-password",
            "history.sql-password",
            "history.url-password",
            "named-queries.sql-password",
            "third-party.pgpass.present",
        ] {
            assert!(rules.contains(&expected), "missing {expected}: {rules:?}");
        }
        #[cfg(unix)]
        assert!(rules.contains(&"third-party.pgpass.permissions"));

        let plaintext = report
            .findings
            .iter()
            .find(|f| f.rule == "legacy.dbcrust.plaintext")
            .unwrap();
        assert_eq!(plaintext.severity, Severity::High);
        assert_eq!(plaintext.line, Some(2));
        assert!(plaintext.path.ends_with(".dbcrust"));
        assert!(plaintext.remediation.contains("secrets migrate"));
        assert_eq!(report.exit_code(), 1);
        // Worst first
        assert_eq!(report.findings[0].severity, Severity::High);
    }

    #[test]
    fn canary_values_never_appear_in_text_or_json() {
        let (_dir, context) = fixture();
        let report = run_audit(&context, None);
        let text = report.render_text();
        let json = report.render_json();
        for canary in CANARIES
            .iter()
            .chain(["CANARY-ENC-VALUE", "CANARY-PGPASS", "CANARY-PLAIN-AI"].iter())
        {
            assert!(!text.contains(canary), "{canary} leaked into text output");
            assert!(!json.contains(canary), "{canary} leaked into JSON output");
        }
        // Both formats are real and parse
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["findings"].as_array().unwrap().len() >= 10);
        assert_eq!(parsed["ok"], false);
        assert!(text.contains("high"));
    }

    #[test]
    fn clean_tree_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let context = AuditContext {
            config_dir: dir.path().to_path_buf(),
            dbcrust_pass: Some(dir.path().join(".dbcrust")),
            pgpass: None,
            mysql_config: None,
            vault_token: None,
            probe_legacy_keychain: false,
        };
        let report = run_audit(&context, None);
        assert!(report.findings.is_empty());
        assert_eq!(report.exit_code(), 0);
        assert!(report.render_text().contains("No credential findings"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed_and_oversized_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real_sessions");
        fs::write(&real, "sslpassword = \"CANARY-LINKED\"\n").unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("sessions.toml")).unwrap();
        fs::write(
            dir.path().join("recent.toml"),
            vec![b'a'; (MAX_SCAN_BYTES + 1) as usize],
        )
        .unwrap();
        let context = AuditContext {
            config_dir: dir.path().to_path_buf(),
            dbcrust_pass: None,
            pgpass: None,
            mysql_config: None,
            vault_token: None,
            probe_legacy_keychain: false,
        };
        let report = run_audit(&context, None);
        let rules: Vec<&str> = report.findings.iter().map(|f| f.rule).collect();
        assert!(rules.contains(&"audit.skipped-symlink"));
        assert!(rules.contains(&"audit.skipped-large"));
        assert!(!rules.contains(&"storage.secret-option"));
        assert!(!report.render_json().contains("CANARY-LINKED"));
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn audit_never_runs_password_commands() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("executed");
        fs::write(
            dir.path().join("sessions.toml"),
            format!(
                "[sessions.p.options]\npassword_command = \"touch {}\"\n",
                marker.display()
            ),
        )
        .unwrap();
        let context = AuditContext {
            config_dir: dir.path().to_path_buf(),
            dbcrust_pass: None,
            pgpass: None,
            mysql_config: None,
            vault_token: None,
            probe_legacy_keychain: false,
        };
        let report = run_audit(&context, None);
        assert!(!marker.exists());
        // password_command is a reference, not a secret value
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.rule != "storage.secret-option")
        );
    }
}
