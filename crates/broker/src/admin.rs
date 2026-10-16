//! Owner-side operations: enroll, revoke, tighten, list.
//!
//! Admin commands write the broker's state directly (no admin socket in v1):
//! in service mode the file permissions are the gate. Enrollment validates the
//! URL (no password, no helper commands, no tunnels, v1 engines only),
//! test-connects through the gateway's own connect path, and stores a
//! [`SecretRecord`] whose payload — target, credential, policy — is the only
//! thing the broker ever acts on.

use super::BrokerError;
use super::gateway::{connect_for_record, database_type_for};
use super::registry::{AliasEntry, AliasFile, ResolvedAlias, is_valid_alias};
use dbcrust_core::database::{ConnectionInfo, DatabaseError, DatabaseType};
use dbcrust_secrets::{
    AgentAccess, AgentPolicy, RecordId, SecretError, SecretRecord, SecretStore, TargetBinding,
    normalize_engine,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

pub const DEFAULT_MAX_ROWS: u32 = 500;
pub const DEFAULT_TIMEOUT_SECS: u32 = 30;
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 256;

/// What a validated enrollment URL pins down.
#[derive(Debug, Clone)]
pub struct EnrollTarget {
    pub binding: TargetBinding,
    /// Non-secret connection options that must travel with the record
    /// (none today besides secret-looking ones such as `sslpassword`).
    pub secret_options: BTreeMap<String, String>,
    pub db_type: DatabaseType,
    /// False for SQLite (a file has no password).
    pub needs_password: bool,
}

fn invalid(message: impl Into<String>) -> BrokerError {
    BrokerError::Invalid(message.into())
}

fn forbidden(message: impl Into<String>) -> BrokerError {
    BrokerError::Forbidden(message.into())
}

fn tls_mode_for(db_type: &DatabaseType, raw: &str) -> Result<String, BrokerError> {
    let value = raw.trim().to_lowercase();
    let mapped = match db_type {
        DatabaseType::PostgreSQL => match value.as_str() {
            "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full" => value,
            _ => {
                return Err(invalid(format!(
                    "sslmode must be one of disable, allow, prefer, require, verify-ca, verify-full (got {raw:?})"
                )));
            }
        },
        DatabaseType::MySQL => match value.as_str() {
            "disabled" | "disable" => "disabled".to_string(),
            "preferred" | "prefer" => "preferred".to_string(),
            "required" | "require" => "required".to_string(),
            "verify_ca" | "verify-ca" => "verify_ca".to_string(),
            "verify_identity" | "verify-full" | "verify-identity" => "verify_identity".to_string(),
            _ => {
                return Err(invalid(format!(
                    "ssl-mode must be one of disabled, preferred, required, verify_ca, verify_identity (got {raw:?})"
                )));
            }
        },
        _ => return Err(invalid("TLS options are not supported for this engine")),
    };
    Ok(mapped)
}

fn truthy(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Validate an enrollment URL and compute the target it binds.
///
/// Pure (no prompting, no network): the SQLite file must exist, nothing else
/// is touched.
pub fn validate_enrollment_url(url: &str) -> Result<EnrollTarget, BrokerError> {
    let url = url.trim();
    if url.is_empty() || url.len() > 2048 || url.chars().any(|c| c.is_control() || c == ' ') {
        return Err(invalid(
            "the connection URL is empty, too long or contains spaces",
        ));
    }
    let Some((scheme, _)) = url.split_once("://") else {
        return Err(invalid(
            "expected a URL such as postgres://user@host:5432/db or sqlite:///abs/path/file.db",
        ));
    };
    let scheme = scheme.to_lowercase();
    match scheme.as_str() {
        "docker" => {
            return Err(forbidden(
                "docker:// targets resolve on the machine running the command and cannot be bound; use the container's host and port",
            ));
        }
        "vault" => {
            return Err(forbidden(
                "vault:// credentials are fetched at connect time; broker aliases hold a fixed credential",
            ));
        }
        "session" | "recent" => {
            return Err(forbidden(
                "session:// and recent:// resolve through the user's own configuration; enroll the explicit URL",
            ));
        }
        "file" => {
            return Err(forbidden(
                "file:// is a picker, not a target; give the explicit sqlite:// URL",
            ));
        }
        _ => {}
    }
    let db_type = DatabaseType::from_scheme(&scheme)
        .map_err(|_| BrokerError::UnsupportedEngine(format!("unknown URL scheme {scheme}://")))?;
    if db_type == DatabaseType::Redis {
        // Agent access needs a Redis command classifier of the broker's own
        // (default-deny, lexer-based, subcommand-aware) before it can be bound
        return Err(BrokerError::UnsupportedEngine(
            "Redis enrollment for agents is not supported yet".to_string(),
        ));
    }
    if db_type == DatabaseType::SqlServer {
        // SQL Server has no session-level read-only: until enrollment can probe
        // the login's permissions, only the login itself limits an agent
        return Err(BrokerError::UnsupportedEngine(
            "SQL Server enrollment for agents is not supported yet: use a read-only login and wait for the read-only probe"
                .to_string(),
        ));
    }
    if db_type == DatabaseType::Redshift {
        // `BEGIN READ ONLY` is a transaction mode the client sets per statement
        // and there is no session-level default; nothing a connection can
        // prove about the user's privileges either. Until a read-only probe
        // exists, the user's grants are the only server-side limit
        return Err(BrokerError::UnsupportedEngine(
            "Redshift enrollment for agents is not supported yet: the server has no session-level read-only mode to enforce"
                .to_string(),
        ));
    }
    if db_type == DatabaseType::LibSql {
        // A read-only token is the only server-side limit, and a client cannot
        // prove what a token allows without writing: until a read-only probe
        // exists, an agent gets no libSQL alias
        return Err(BrokerError::UnsupportedEngine(
            "libSQL enrollment for agents is not supported yet: a read-only token is the only server-enforced limit and cannot be proven from the client"
                .to_string(),
        ));
    }
    if db_type == DatabaseType::InfluxDB {
        // The broker's classifier and row-limit wrapper are SQL-only, and what
        // an InfluxDB credential may do cannot be proven from the client: v1
        // does not enforce read-only for GET (it only warns), v2/v3 depend on
        // the token's scope
        return Err(BrokerError::UnsupportedEngine(
            "InfluxDB enrollment for agents is not supported yet: use a read-only API token or a v1 user limited to reads"
                .to_string(),
        ));
    }
    if !matches!(
        db_type,
        DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::ClickHouse
    ) {
        return Err(BrokerError::UnsupportedEngine(format!(
            "{db_type} is not supported by the broker in this version \
             (PostgreSQL, MySQL/MariaDB, SQLite and ClickHouse are)"
        )));
    }
    if !dbcrust_drivers::is_compiled(&db_type) {
        // Same wording the client factory uses: it names the cargo feature
        let missing = DatabaseError::ConnectorNotBuilt {
            feature: dbcrust_drivers::required_feature(&db_type),
            engine: db_type,
        };
        return Err(BrokerError::UnsupportedEngine(missing.to_string()));
    }

    // `user:@host` has an (empty) password slot the parsed form may drop
    let authority = url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    let userinfo_has_password = authority
        .rsplit_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains(':'));

    let info = ConnectionInfo::parse_url(url).map_err(|e| invalid(e.to_string()))?;
    if info.password.is_some() || userinfo_has_password {
        return Err(forbidden(
            "the URL must not contain a password; you will be asked for it interactively",
        ));
    }

    let mut tls_raw: Option<(String, String)> = None;
    let mut tls_bool: Option<bool> = None;
    let mut secret_options: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in &info.options {
        let lower = key.to_lowercase();
        if dbcrust_core::options::is_password_command_option_key(key) {
            return Err(forbidden(
                "password_command is not allowed: the broker must hold the credential itself",
            ));
        }
        if matches!(lower.as_str(), "password" | "pass" | "pwd" | "passwd") {
            return Err(forbidden(
                "the URL must not contain a password; you will be asked for it interactively",
            ));
        }
        if lower.starts_with("ssh") {
            return Err(forbidden(
                "SSH tunnel options are not allowed for broker aliases",
            ));
        }
        if lower.starts_with("vault") {
            return Err(forbidden(
                "Vault options are not allowed for broker aliases",
            ));
        }
        match lower.as_str() {
            "sslmode" | "ssl-mode" | "ssl_mode" => tls_raw = Some((key.clone(), value.clone())),
            "ssl" | "tls" | "secure" => {
                tls_bool = Some(
                    truthy(value).ok_or_else(|| invalid(format!("{key} must be true or false")))?,
                );
            }
            _ if dbcrust_core::password_sanitizer::is_secret_option_key(key) => {
                secret_options.insert(key.clone(), value.clone());
            }
            _ => {
                return Err(invalid(format!(
                    "option {key:?} is not supported for broker aliases \
                     (allowed: sslmode / ssl-mode, ssl, tls, secure)"
                )));
            }
        }
    }

    if db_type == DatabaseType::SQLite {
        if tls_raw.is_some() || tls_bool.is_some() || !secret_options.is_empty() {
            return Err(invalid("SQLite aliases take no connection options"));
        }
        let raw_path = info
            .file_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| invalid("sqlite:// needs a file path"))?;
        let path = std::fs::canonicalize(raw_path)
            .map_err(|e| invalid(format!("SQLite file {raw_path:?} is not accessible: {e}")))?;
        if !path.is_file() {
            return Err(invalid(format!("{} is not a file", path.display())));
        }
        let binding = TargetBinding::new("sqlite", "", None, &path.to_string_lossy(), "", None);
        return Ok(EnrollTarget {
            binding,
            secret_options,
            db_type,
            needs_password: false,
        });
    }

    let host = info
        .host
        .clone()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| invalid("the URL needs a host"))?;
    let user = info
        .username
        .clone()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| invalid("the URL needs a user (postgres://user@host/db)"))?;
    let database = match (&info.database, &db_type) {
        (Some(db), _) if !db.is_empty() => db.clone(),
        (_, DatabaseType::PostgreSQL) => "postgres".to_string(),
        (_, DatabaseType::MySQL) => "mysql".to_string(),
        (_, DatabaseType::ClickHouse) => "default".to_string(),
        _ => return Err(invalid("the URL needs a database name")),
    };

    let tls_mode = match db_type {
        DatabaseType::PostgreSQL | DatabaseType::MySQL => Some(match (&tls_raw, tls_bool) {
            (Some((_, raw)), _) => tls_mode_for(&db_type, raw)?,
            (None, Some(true)) => tls_mode_for(&db_type, "require")?,
            (None, Some(false)) => tls_mode_for(&db_type, "disable")?,
            // sqlx's default, recorded so it can never drift
            (None, None) => tls_mode_for(&db_type, "prefer")?,
        }),
        DatabaseType::ClickHouse => {
            if let Some((_, raw)) = &tls_raw {
                return Err(invalid(format!(
                    "sslmode {raw:?} does not apply to ClickHouse; use ssl=true"
                )));
            }
            // An explicit ssl=false wins; otherwise the parser's inference
            // (ssl=true, or the HTTPS port 8443) decides
            let tls = tls_bool.unwrap_or(info.use_tls);
            Some(if tls { "tls" } else { "plain" }.to_string())
        }
        _ => None,
    };

    let engine = match db_type {
        DatabaseType::PostgreSQL => "postgresql",
        DatabaseType::MySQL => "mysql",
        _ => "clickhouse",
    };
    let binding = TargetBinding::new(
        engine,
        &host,
        info.port,
        &database,
        &user,
        tls_mode.as_deref(),
    );
    Ok(EnrollTarget {
        binding,
        secret_options,
        db_type,
        needs_password: true,
    })
}

/// Enrollment parameters (everything but the password).
#[derive(Debug, Clone, Default)]
pub struct EnrollRequest {
    pub alias: String,
    pub url: String,
    pub allow_writes: bool,
    pub max_rows: Option<u32>,
    pub timeout_secs: Option<u32>,
    pub max_bytes: Option<u64>,
    pub description: Option<String>,
}

impl EnrollRequest {
    /// Check the alias, description and limits (no I/O).
    pub fn validate(&self) -> Result<AgentPolicy, BrokerError> {
        if !is_valid_alias(&self.alias) {
            return Err(invalid(
                "aliases are lowercase letters, digits, '_' and '-' (1-63 characters, starting with a letter or digit)",
            ));
        }
        if let Some(description) = &self.description {
            if description.chars().count() > MAX_DESCRIPTION_CHARS
                || description.chars().any(char::is_control)
            {
                return Err(invalid(
                    "the description is limited to 256 characters without control characters",
                ));
            }
        }
        for (name, value) in [
            ("--max-rows", self.max_rows.map(u64::from)),
            ("--timeout", self.timeout_secs.map(u64::from)),
            ("--max-bytes", self.max_bytes),
        ] {
            if value == Some(0) {
                return Err(invalid(format!("{name} must be at least 1")));
            }
        }
        Ok(AgentPolicy {
            access: if self.allow_writes {
                AgentAccess::ReadWrite
            } else {
                AgentAccess::ReadOnly
            },
            max_rows: self.max_rows.unwrap_or(DEFAULT_MAX_ROWS),
            timeout_secs: self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
            max_bytes: self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        }
        .clamped())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollOutcome {
    pub alias: String,
    pub engine: String,
    pub policy: AgentPolicy,
    pub revision: u64,
    /// True when an existing alias was replaced.
    pub replaced: bool,
    pub target: String,
}

/// What `broker list` shows for one alias.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AliasStatus {
    pub alias: String,
    pub engine: String,
    pub description: Option<String>,
    pub enrolled_at: String,
    /// `ok`, or why the alias cannot be used.
    pub state: String,
    pub access: Option<String>,
    pub max_rows: Option<u32>,
    pub timeout_secs: Option<u32>,
    pub max_bytes: Option<u64>,
    pub revision: Option<u64>,
    /// `user@host:port/db` (never a credential).
    pub target: Option<String>,
}

/// Limits to lower with `broker policy`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TightenRequest {
    pub max_rows: Option<u32>,
    pub timeout_secs: Option<u32>,
    pub max_bytes: Option<u64>,
    pub read_only: bool,
}

pub struct Admin {
    pub aliases_path: std::path::PathBuf,
    pub store: Arc<SecretStore>,
}

/// `user@host:port/db` (or the file path for SQLite): non-secret, for humans.
pub fn describe_target(binding: &TargetBinding) -> String {
    let t = binding.normalized();
    if t.engine == "sqlite" {
        return format!("sqlite:{}", t.database);
    }
    format!(
        "{}://{}@{}{}/{}",
        t.engine,
        t.user,
        t.host,
        t.port.map(|p| format!(":{p}")).unwrap_or_default(),
        t.database
    )
}

fn access_label(access: AgentAccess) -> &'static str {
    match access {
        AgentAccess::ReadOnly => "read_only",
        AgentAccess::ReadWrite => "read_write",
    }
}

impl Admin {
    pub fn new(aliases_path: impl AsRef<Path>, store: Arc<SecretStore>) -> Self {
        Self {
            aliases_path: aliases_path.as_ref().to_path_buf(),
            store,
        }
    }

    fn lock(&self) -> Result<dbcrust_secrets::fsutil::FileLock, BrokerError> {
        dbcrust_secrets::fsutil::FileLock::acquire(&self.aliases_path)
            .map_err(|e| BrokerError::State(e.to_string()))
    }

    fn existing_record(&self, alias: &str) -> Result<Option<SecretRecord>, BrokerError> {
        Ok(self.store.get_record(&RecordId::broker(alias))?)
    }

    /// Enroll (or re-enroll) `request.alias`. `password` is the owner's proof
    /// and the credential; SQLite takes none. Replacing an existing alias
    /// bumps its revision.
    pub async fn enroll(
        &self,
        request: &EnrollRequest,
        password: Option<Zeroizing<String>>,
    ) -> Result<EnrollOutcome, BrokerError> {
        let policy = request.validate()?;
        let target = validate_enrollment_url(&request.url)?;
        let secret = match (target.needs_password, password) {
            (true, Some(p)) if !p.is_empty() => p,
            (true, _) => {
                return Err(invalid(
                    "a password is required (it is read interactively and stored by the broker)",
                ));
            }
            (false, _) => Zeroizing::new(String::new()),
        };

        let record_id = RecordId::broker(&request.alias);
        let previous = self.existing_record(&request.alias)?;
        let mut record = SecretRecord::new(
            &secret,
            target
                .secret_options
                .iter()
                .map(|(k, v)| (k.clone(), Zeroizing::new(v.clone())))
                .collect(),
            target.binding.clone(),
        );
        record.agent_policy = Some(policy);
        if let Some(previous) = &previous {
            record.revision = previous.revision.saturating_add(1);
            record.created_at = previous.created_at.clone();
        }

        // Test the exact connect path the gateway will use, with read-only
        // hardening on, before anything is stored
        let engine = normalize_engine(&record.bound_target.engine);
        let entry = AliasEntry {
            alias: request.alias.clone(),
            record_id: record_id.clone(),
            engine: engine.clone(),
            description: request.description.clone(),
            enrolled_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            revision: record.revision,
        };
        let probe = ResolvedAlias {
            alias: request.alias.clone(),
            entry: entry.clone(),
            record: record.clone(),
            policy,
        };
        debug_assert!(database_type_for(&engine).is_some());
        let client = connect_for_record(&probe, true, policy.timeout_secs)
            .await
            .map_err(|e| BrokerError::Connect(e.message))?;
        let check = client.execute_query_structured("SELECT 1").await;
        // Dropped, not `close()`d: the PostgreSQL client pins a pooled
        // connection, so a graceful pool close would wait for it forever
        drop(client);
        match check {
            Ok(result) if !result.rows.is_empty() => {}
            Ok(_) => {
                return Err(BrokerError::Connect(
                    "SELECT 1 returned no rows".to_string(),
                ));
            }
            Err(e) => {
                return Err(BrokerError::Connect(
                    dbcrust_core::password_sanitizer::redact_text(&e.to_string()),
                ));
            }
        }

        let _lock = self.lock()?;
        self.store.put_record(&record_id, &record)?;
        let mut file = AliasFile::load(&self.aliases_path)?;
        file.upsert(entry);
        file.save(&self.aliases_path)?;

        Ok(EnrollOutcome {
            alias: request.alias.clone(),
            engine,
            policy,
            revision: record.revision,
            replaced: previous.is_some(),
            target: describe_target(&record.bound_target),
        })
    }

    /// Remove an alias. Returns whether anything existed.
    pub fn revoke(&self, alias: &str) -> Result<bool, BrokerError> {
        if !is_valid_alias(alias) {
            return Err(invalid("not a valid alias name"));
        }
        let _lock = self.lock()?;
        // Record first: once it is gone the alias is unusable even if the
        // descriptor removal below fails
        let had_record = self.store.delete(&RecordId::broker(alias))?;
        let mut file = AliasFile::load(&self.aliases_path)?;
        let had_entry = file.remove(alias);
        if had_entry {
            file.save(&self.aliases_path)?;
        }
        Ok(had_record || had_entry)
    }

    /// Lower an alias's limits or downgrade it to read-only. Anything that
    /// would loosen the policy is refused ("re-enroll").
    pub fn tighten(
        &self,
        alias: &str,
        request: TightenRequest,
    ) -> Result<AgentPolicy, BrokerError> {
        if !is_valid_alias(alias) {
            return Err(invalid("not a valid alias name"));
        }
        let _lock = self.lock()?;
        let id = RecordId::broker(alias);
        let mut record = self
            .store
            .get_record(&id)?
            .ok_or_else(|| invalid(format!("no alias named {alias:?}")))?;
        let current = record
            .agent_policy
            .ok_or_else(|| invalid("this alias has no agent policy"))?
            .clamped();

        let reenroll = |what: &str| {
            forbidden(format!(
                "{what} would loosen the policy; re-enroll the alias (it needs the password again)"
            ))
        };
        let mut next = current;
        if let Some(rows) = request.max_rows {
            if rows == 0 {
                return Err(invalid("--max-rows must be at least 1"));
            }
            if rows > current.max_rows {
                return Err(reenroll("raising --max-rows"));
            }
            next.max_rows = rows;
        }
        if let Some(secs) = request.timeout_secs {
            if secs == 0 {
                return Err(invalid("--timeout must be at least 1"));
            }
            if secs > current.timeout_secs {
                return Err(reenroll("raising --timeout"));
            }
            next.timeout_secs = secs;
        }
        if let Some(bytes) = request.max_bytes {
            if bytes == 0 {
                return Err(invalid("--max-bytes must be at least 1"));
            }
            if bytes > current.max_bytes {
                return Err(reenroll("raising --max-bytes"));
            }
            next.max_bytes = bytes;
        }
        if request.read_only {
            next.access = AgentAccess::ReadOnly;
        }

        record.agent_policy = Some(next);
        record.revision = record.revision.saturating_add(1);
        self.store.put_record(&id, &record)?;

        let mut file = AliasFile::load(&self.aliases_path)?;
        if let Some(entry) = file.get(alias).cloned() {
            file.upsert(AliasEntry {
                revision: record.revision,
                ..entry
            });
            file.save(&self.aliases_path)?;
        }
        Ok(next)
    }

    /// Every enrolled alias with its stored policy (never a credential).
    pub fn list(&self) -> Result<Vec<AliasStatus>, BrokerError> {
        let file = AliasFile::load(&self.aliases_path)?;
        let mut out = Vec::new();
        for entry in &file.aliases {
            let mut status = AliasStatus {
                alias: entry.alias.clone(),
                engine: entry.engine.clone(),
                description: entry.description.clone(),
                enrolled_at: entry.enrolled_at.clone(),
                state: "ok".to_string(),
                access: None,
                max_rows: None,
                timeout_secs: None,
                max_bytes: None,
                revision: None,
                target: None,
            };
            match self.store.get_record(&RecordId::broker(&entry.alias)) {
                Ok(Some(record)) => {
                    status.target = Some(describe_target(&record.bound_target));
                    status.revision = Some(record.revision);
                    match record.agent_policy {
                        Some(policy) => {
                            let policy = policy.clamped();
                            status.access = Some(access_label(policy.access).to_string());
                            status.max_rows = Some(policy.max_rows);
                            status.timeout_secs = Some(policy.timeout_secs);
                            status.max_bytes = Some(policy.max_bytes);
                        }
                        None => status.state = "no agent policy".to_string(),
                    }
                    if normalize_engine(&record.bound_target.engine)
                        != normalize_engine(&entry.engine)
                    {
                        status.state = "refused: descriptor does not match record".to_string();
                    }
                }
                Ok(None) => status.state = "record missing (revoked?)".to_string(),
                Err(SecretError::Locked(_)) => status.state = "store locked".to_string(),
                Err(e) => status.state = format!("store error: {e}"),
            }
            out.push(status);
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rstest::rstest;

    fn url(u: &str) -> Result<EnrollTarget, BrokerError> {
        validate_enrollment_url(u)
    }

    #[test]
    fn plain_postgres_url_binds_target_with_explicit_tls() {
        let t = url("postgres://agent@db.example.com:5433/app").unwrap();
        assert_eq!(t.binding.engine, "postgresql");
        assert_eq!(t.binding.host, "db.example.com");
        assert_eq!(t.binding.port, Some(5433));
        assert_eq!(t.binding.database, "app");
        assert_eq!(t.binding.user, "agent");
        assert_eq!(t.binding.tls_mode.as_deref(), Some("prefer"));
        assert!(t.needs_password);
    }

    #[rstest]
    #[case("postgres://agent@db/app?sslmode=verify-full", "verify-full")]
    #[case("postgres://agent@db/app?sslmode=require", "require")]
    #[case("postgres://agent@db/app?ssl=true", "require")]
    #[case("postgres://agent@db/app?ssl=false", "disable")]
    fn postgres_tls_modes(#[case] u: &str, #[case] expected: &str) {
        assert_eq!(url(u).unwrap().binding.tls_mode.as_deref(), Some(expected));
    }

    #[rstest]
    #[case("mysql://agent@db/app", "preferred")]
    #[case("mysql://agent@db/app?ssl-mode=VERIFY_IDENTITY", "verify_identity")]
    #[case("mysql://agent@db/app?sslmode=require", "required")]
    fn mysql_tls_modes(#[case] u: &str, #[case] expected: &str) {
        assert_eq!(url(u).unwrap().binding.tls_mode.as_deref(), Some(expected));
    }

    #[test]
    fn clickhouse_tls_is_plain_or_tls() {
        assert_eq!(
            url("clickhouse://agent@ch:8123/default")
                .unwrap()
                .binding
                .tls_mode
                .as_deref(),
            Some("plain")
        );
        assert_eq!(
            url("clickhouse://agent@ch:8443/default")
                .unwrap()
                .binding
                .tls_mode
                .as_deref(),
            Some("tls")
        );
        assert_eq!(
            url("clickhouse://agent@ch/default?ssl=true")
                .unwrap()
                .binding
                .tls_mode
                .as_deref(),
            Some("tls")
        );
    }

    #[test]
    fn default_database_names_are_recorded() {
        assert_eq!(url("postgres://a@h").unwrap().binding.database, "postgres");
        assert_eq!(url("mysql://a@h").unwrap().binding.database, "mysql");
        assert_eq!(url("clickhouse://a@h").unwrap().binding.database, "default");
    }

    #[rstest]
    #[case::password("postgres://alice:hunter2@db/app")]
    #[case::empty_password("postgres://alice:@db/app")]
    #[case::password_param("postgres://alice@db/app?password=x")]
    #[case::pass_param("postgres://alice@db/app?pwd=x")]
    #[case::password_command("postgres://alice@db/app?password_command=pass+show+db")]
    #[case::password_cmd("postgres://alice@db/app?password_cmd=true")]
    #[case::docker("docker://my-postgres/app")]
    #[case::vault("vault://role@database/mydb")]
    #[case::session("session://prod")]
    #[case::recent("recent://")]
    #[case::file_picker("file://")]
    #[case::ssh_option("postgres://alice@db/app?ssh_tunnel=jump@host")]
    #[case::ssh_host("postgres://alice@db/app?ssh_host=jump")]
    #[case::unknown_option("postgres://alice@db/app?search_path=evil")]
    #[case::no_scheme("db.example.com/app")]
    #[case::spaces("postgres://alice@db/app name")]
    #[case::empty("")]
    fn forbidden_or_invalid_urls_are_rejected(#[case] u: &str) {
        assert!(url(u).is_err(), "{u} should be rejected");
    }

    #[rstest]
    #[case("mongodb://alice@db/app")]
    #[case("elasticsearch://alice@es:9200")]
    #[case("white-dragon://host:7700")]
    #[case("parquet:///tmp/x.parquet")]
    #[case("csv:///tmp/x.csv")]
    #[case("duckdb:///tmp/x.duckdb")]
    #[case("redis://alice@r")]
    #[case("rediss://alice@r:6380/1")]
    #[case("valkey://alice@r")]
    #[case("redis+unix:///run/redis.sock")]
    #[case("mssql://sa@db.example/Sales")]
    #[case("sqlserver://sa@db.example:1433/Sales?encrypt=true")]
    #[case("mssql+tls://sa@db.example")]
    #[case("redshift://awsuser@c.abc.eu-west-1.redshift.amazonaws.com:5439/dev")]
    #[case("redshift+tls://awsuser@c.example/dev")]
    #[case("libsql://my-db-org.turso.io")]
    #[case("turso://my-db-org.turso.io?authToken=x")]
    #[case("libsql+http://localhost:8080")]
    #[case("influxdb://alice@db.example/telemetry")]
    #[case("influxdb2s://db.example/bucket?org=acme")]
    #[case("influxdb3://db.example/mydb")]
    fn unsupported_engines_are_rejected_as_such(#[case] u: &str) {
        assert!(
            matches!(url(u), Err(BrokerError::UnsupportedEngine(_))),
            "{u}: {:?}",
            url(u)
        );
    }

    #[test]
    fn sql_server_enrollment_points_at_a_read_only_login() {
        let err = url("mssql://alice@db.example/Sales").unwrap_err();
        assert!(
            matches!(&err, BrokerError::UnsupportedEngine(message)
                if message == "SQL Server enrollment for agents is not supported yet: use a read-only login and wait for the read-only probe"),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("unsupported_engine: SQL Server enrollment")
        );
    }

    #[test]
    fn redshift_enrollment_explains_the_missing_read_only_mode() {
        let err = url("redshift://alice@c.example:5439/dev").unwrap_err();
        assert!(
            matches!(&err, BrokerError::UnsupportedEngine(message)
                if message.starts_with("Redshift enrollment for agents is not supported yet")),
            "{err:?}"
        );
    }

    #[test]
    fn libsql_enrollment_explains_the_unprovable_token_scope() {
        let err = url("libsql://my-db-org.turso.io").unwrap_err();
        assert!(
            matches!(&err, BrokerError::UnsupportedEngine(message)
                if message.starts_with("libSQL enrollment for agents is not supported yet")),
            "{err:?}"
        );
    }

    #[test]
    fn influxdb_enrollment_points_at_a_read_only_credential() {
        let err = url("influxdb2://db.example/b?org=acme&token=canary-token-1").unwrap_err();
        assert!(
            matches!(&err, BrokerError::UnsupportedEngine(message)
                if message.starts_with("InfluxDB enrollment for agents is not supported yet")),
            "{err:?}"
        );
        assert!(!err.to_string().contains("canary-token-1"));
    }

    #[test]
    fn redis_enrollment_says_it_is_not_supported_yet() {
        let err = url("redis://alice@r").unwrap_err();
        assert!(
            matches!(&err, BrokerError::UnsupportedEngine(message)
                if message == "Redis enrollment for agents is not supported yet"),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .starts_with("unsupported_engine: Redis enrollment")
        );
    }

    #[test]
    fn rejection_messages_never_echo_the_password() {
        let err = url("postgres://alice:canary-secret-123@db/app").unwrap_err();
        assert!(!err.to_string().contains("canary-secret-123"));
        let err = url("postgres://alice@db/app?password=canary-secret-123").unwrap_err();
        assert!(!err.to_string().contains("canary-secret-123"));
        let err =
            url("postgres://alice@db/app?sslpassword2=canary-secret-123&bogus=1").unwrap_err();
        assert!(!err.to_string().contains("canary-secret-123"));
    }

    #[test]
    fn secret_looking_options_move_to_secret_options() {
        let t = url("postgres://alice@db/app?sslpassword=keypass").unwrap();
        assert_eq!(
            t.secret_options.get("sslpassword").map(String::as_str),
            Some("keypass")
        );
    }

    #[test]
    fn sqlite_binds_the_canonical_path_and_needs_no_password() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("x.db");
        std::fs::write(&db, b"").unwrap();
        let t = url(&format!("sqlite://{}", db.display())).unwrap();
        assert_eq!(t.binding.engine, "sqlite");
        assert!(!t.needs_password);
        assert_eq!(
            t.binding.database,
            std::fs::canonicalize(&db).unwrap().to_string_lossy()
        );
        // nonexistent file and directories are refused
        assert!(url(&format!("sqlite://{}/nope.db", dir.path().display())).is_err());
        assert!(url(&format!("sqlite://{}", dir.path().display())).is_err());
        // options make no sense for a file
        assert!(url(&format!("sqlite://{}?sslmode=require", db.display())).is_err());
    }

    #[rstest]
    #[case("", false)]
    #[case("Prod", false)]
    #[case("prod", true)]
    fn request_validation_checks_alias(#[case] alias: &str, #[case] ok: bool) {
        let request = EnrollRequest {
            alias: alias.to_string(),
            url: "sqlite:///x".to_string(),
            ..Default::default()
        };
        assert_eq!(request.validate().is_ok(), ok);
    }

    #[test]
    fn request_defaults_and_clamps() {
        let request = EnrollRequest {
            alias: "prod".into(),
            ..Default::default()
        };
        let policy = request.validate().unwrap();
        assert_eq!(policy.access, AgentAccess::ReadOnly);
        assert_eq!(policy.max_rows, 500);
        assert_eq!(policy.timeout_secs, 30);
        assert_eq!(policy.max_bytes, 8 * 1024 * 1024);

        let big = EnrollRequest {
            alias: "prod".into(),
            allow_writes: true,
            max_rows: Some(1_000_000),
            timeout_secs: Some(100_000),
            max_bytes: Some(u64::MAX),
            ..Default::default()
        };
        let policy = big.validate().unwrap();
        assert_eq!(policy.access, AgentAccess::ReadWrite);
        assert_eq!(policy.max_rows, 10_000);
        assert_eq!(policy.timeout_secs, 600);
        assert_eq!(policy.max_bytes, 64 * 1024 * 1024);

        let zero = EnrollRequest {
            alias: "prod".into(),
            max_rows: Some(0),
            ..Default::default()
        };
        assert!(zero.validate().is_err());
    }
}
