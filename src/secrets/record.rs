//! The versioned payload stored for every connection secret, its target
//! binding and the agent policy ceiling that travels with it.

use super::SecretError;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

/// Current payload version.
pub const RECORD_VERSION: u32 = 1;

/// Characters left as-is in record ids (everything else is percent-encoded
/// so `:`, `@` and `/` stay unambiguous separators).
const ID_SAFE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn encode_component(value: &str) -> String {
    utf8_percent_encode(value, ID_SAFE).to_string()
}

/// Canonical engine name: lowercase, aliases folded (`postgres` →
/// `postgresql`). Unknown names are only lowercased and trimmed.
pub fn normalize_engine(engine: &str) -> String {
    match engine.trim().to_lowercase().as_str() {
        "postgresql" | "postgres" | "pg" => "postgresql".to_string(),
        "mysql" | "mariadb" => "mysql".to_string(),
        "mongodb" | "mongo" | "mongodb+srv" => "mongodb".to_string(),
        "elasticsearch" | "elastic" => "elasticsearch".to_string(),
        "clickhouse" | "ch" => "clickhouse".to_string(),
        "sqlite" | "sqlite3" => "sqlite".to_string(),
        other => other.to_string(),
    }
}

/// The default TCP port of a (normalized) engine, when it has one.
pub fn default_port_for_engine(engine: &str) -> Option<u16> {
    match normalize_engine(engine).as_str() {
        "postgresql" => Some(5432),
        "mysql" => Some(3306),
        "mongodb" => Some(27017),
        "elasticsearch" => Some(9200),
        "clickhouse" => Some(8123),
        _ => None,
    }
}

/// The exact target a secret may be used against.
///
/// A secret is only ever handed out for a target that [`matches`] the one it
/// was stored for: redirecting a saved connection to another host fails
/// closed instead of sending the password there.
///
/// [`matches`]: TargetBinding::matches
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetBinding {
    pub engine: String,
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub tls_mode: Option<String>,
}

impl TargetBinding {
    /// Build a normalized binding.
    pub fn new(
        engine: &str,
        host: &str,
        port: Option<u16>,
        database: &str,
        user: &str,
        tls_mode: Option<&str>,
    ) -> Self {
        Self {
            engine: engine.to_string(),
            host: host.to_string(),
            port,
            database: database.to_string(),
            user: user.to_string(),
            tls_mode: tls_mode.map(str::to_string),
        }
        .normalized()
    }

    /// Lowercase engine/host/TLS, fold engine aliases, apply the engine's
    /// default port, trim everything. Idempotent.
    pub fn normalized(&self) -> Self {
        let engine = normalize_engine(&self.engine);
        let host = self
            .host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.')
            .to_lowercase();
        let port = self.port.or_else(|| default_port_for_engine(&engine));
        let tls_mode = self
            .tls_mode
            .as_deref()
            .map(|mode| mode.trim().to_lowercase())
            .filter(|mode| !mode.is_empty());
        Self {
            engine,
            host,
            port,
            database: self.database.trim().to_string(),
            user: self.user.trim().to_string(),
            tls_mode,
        }
    }

    /// True when `requested` is the same target. The TLS mode is compared
    /// only when both sides know it (callers resolving a password from a bare
    /// host/port/db/user cannot know it).
    pub fn matches(&self, requested: &TargetBinding) -> bool {
        let stored = self.normalized();
        let wanted = requested.normalized();
        let tls_matches = match (&stored.tls_mode, &wanted.tls_mode) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        stored.engine == wanted.engine
            && stored.host == wanted.host
            && stored.port == wanted.port
            && stored.database == wanted.database
            && stored.user == wanted.user
            && tls_matches
    }

    /// `user@host:port/db` for messages (never contains a secret).
    pub fn describe(&self) -> String {
        let normalized = self.normalized();
        format!(
            "{}:{}@{}:{}/{}",
            normalized.engine,
            normalized.user,
            normalized.host,
            normalized
                .port
                .map_or_else(|| "-".to_string(), |p| p.to_string()),
            normalized.database
        )
    }

    /// The store id of the connection record for this target.
    pub fn record_id(&self) -> String {
        let normalized = self.normalized();
        format!(
            "conn:{}:{}@{}:{}/{}",
            encode_component(&normalized.engine),
            encode_component(&normalized.user),
            encode_component(&normalized.host),
            normalized.port.unwrap_or(0),
            encode_component(&normalized.database)
        )
    }
}

/// What an agent may do with a connection. Ordered: `ReadOnly < ReadWrite`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAccess {
    ReadOnly,
    ReadWrite,
}

/// Hard ceilings no policy may exceed.
pub const HARD_MAX_ROWS: u32 = 10_000;
pub const HARD_MAX_TIMEOUT_SECS: u32 = 600;
pub const HARD_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Per-connection ceiling for agent queries, stored inside the secret
/// payload so it cannot be edited out of band without the secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPolicy {
    pub access: AgentAccess,
    pub max_rows: u32,
    pub timeout_secs: u32,
    pub max_bytes: u64,
}

impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            access: AgentAccess::ReadOnly,
            max_rows: 500,
            timeout_secs: 30,
            max_bytes: 8 * 1024 * 1024,
        }
    }
}

/// A client's (possibly partial) request to narrow a policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PolicyRequest {
    pub access: Option<AgentAccess>,
    pub max_rows: Option<u32>,
    pub timeout_secs: Option<u32>,
    pub max_bytes: Option<u64>,
}

impl AgentPolicy {
    /// This policy with every limit capped at the hard ceilings (and at
    /// least 1, so a policy can never mean "unlimited" through zero).
    pub fn clamped(&self) -> Self {
        Self {
            access: self.access,
            max_rows: self.max_rows.clamp(1, HARD_MAX_ROWS),
            timeout_secs: self.timeout_secs.clamp(1, HARD_MAX_TIMEOUT_SECS),
            max_bytes: self.max_bytes.clamp(1, HARD_MAX_BYTES),
        }
    }

    /// The effective policy for a request: it can only lower limits and
    /// downgrade access, never raise either.
    pub fn tighten(&self, requested: &AgentPolicy) -> AgentPolicy {
        let ceiling = self.clamped();
        AgentPolicy {
            access: ceiling.access.min(requested.access),
            max_rows: ceiling.max_rows.min(requested.max_rows),
            timeout_secs: ceiling.timeout_secs.min(requested.timeout_secs),
            max_bytes: ceiling.max_bytes.min(requested.max_bytes),
        }
        .clamped()
    }

    /// [`tighten`](Self::tighten) for requests that name only some limits.
    pub fn tighten_with(&self, request: PolicyRequest) -> AgentPolicy {
        let ceiling = self.clamped();
        self.tighten(&AgentPolicy {
            access: request.access.unwrap_or(ceiling.access),
            max_rows: request.max_rows.unwrap_or(ceiling.max_rows),
            timeout_secs: request.timeout_secs.unwrap_or(ceiling.timeout_secs),
            max_bytes: request.max_bytes.unwrap_or(ceiling.max_bytes),
        })
    }
}

/// What kind of thing a record id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Connection,
    Ai,
    Oauth,
    /// Reserved for the credential broker (`broker:<alias>`).
    Broker,
}

/// A parsed record id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordId {
    pub kind: RecordKind,
    raw: String,
}

impl RecordId {
    /// Classify an id by its prefix; `None` for unknown prefixes.
    pub fn parse(id: &str) -> Option<Self> {
        let kind = match id.split_once(':')?.0 {
            "conn" => RecordKind::Connection,
            "ai" => RecordKind::Ai,
            "oauth" => RecordKind::Oauth,
            "broker" => RecordKind::Broker,
            _ => return None,
        };
        (id.len() > 5 && id.len() <= 512).then(|| Self {
            kind,
            raw: id.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// `ai:<provider>` for an AI provider's API key.
    pub fn ai(provider: &str) -> String {
        format!("ai:{}", encode_component(&provider.trim().to_lowercase()))
    }

    /// `oauth:<name>` for a named OAuth/token secret.
    pub fn oauth(name: &str) -> String {
        format!("oauth:{}", encode_component(name.trim()))
    }

    /// `broker:<alias>` (reserved; no behaviour in the store itself).
    pub fn broker(alias: &str) -> String {
        format!("broker:{}", encode_component(alias.trim()))
    }
}

/// A stored connection secret and everything bound to it.
#[derive(Clone)]
pub struct SecretRecord {
    /// Payload version (always [`RECORD_VERSION`] for new records).
    pub v: u32,
    /// The password / token. Empty when the record only carries
    /// `secret_options`.
    pub secret: Zeroizing<String>,
    /// Secret-looking connection options (`sslpassword`, `ssh_password`…).
    pub secret_options: BTreeMap<String, Zeroizing<String>>,
    pub bound_target: TargetBinding,
    pub agent_policy: Option<AgentPolicy>,
    /// Incremented on every rewrite of the same record.
    pub revision: u64,
    /// RFC 3339 creation time.
    pub created_at: String,
}

impl std::fmt::Debug for SecretRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRecord")
            .field("v", &self.v)
            .field("secret", &"***")
            .field(
                "secret_options",
                &self.secret_options.keys().collect::<Vec<_>>(),
            )
            .field("bound_target", &self.bound_target)
            .field("agent_policy", &self.agent_policy)
            .field("revision", &self.revision)
            .field("created_at", &self.created_at)
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
struct WireRecord {
    v: u32,
    secret: Zeroizing<String>,
    #[serde(default)]
    secret_options: BTreeMap<String, Zeroizing<String>>,
    bound_target: TargetBinding,
    #[serde(default)]
    agent_policy: Option<AgentPolicy>,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    created_at: String,
}

impl SecretRecord {
    /// A fresh record (revision 1, created now).
    pub fn new(
        secret: &str,
        secret_options: BTreeMap<String, Zeroizing<String>>,
        bound_target: TargetBinding,
    ) -> Self {
        Self {
            v: RECORD_VERSION,
            secret: Zeroizing::new(secret.to_string()),
            secret_options,
            bound_target: bound_target.normalized(),
            agent_policy: None,
            revision: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Serialize to the JSON stored in the backend (zeroized on drop).
    pub fn to_json(&self) -> Result<Zeroizing<String>, SecretError> {
        let wire = WireRecord {
            v: self.v,
            secret: self.secret.clone(),
            secret_options: self.secret_options.clone(),
            bound_target: self.bound_target.clone(),
            agent_policy: self.agent_policy,
            revision: self.revision,
            created_at: self.created_at.clone(),
        };
        serde_json::to_string(&wire)
            .map(Zeroizing::new)
            .map_err(|_| SecretError::Corrupt("record could not be serialized".to_string()))
    }

    /// Parse a stored payload. Errors never echo the payload.
    pub fn from_json(json: &str) -> Result<Self, SecretError> {
        let wire: WireRecord = serde_json::from_str(json)
            .map_err(|_| SecretError::Corrupt("record payload is not valid".to_string()))?;
        if wire.v != RECORD_VERSION {
            return Err(SecretError::Corrupt(format!(
                "record version {} is not supported by this DBCrust",
                wire.v
            )));
        }
        Ok(Self {
            v: wire.v,
            secret: wire.secret,
            secret_options: wire.secret_options,
            bound_target: wire.bound_target.normalized(),
            agent_policy: wire.agent_policy,
            revision: wire.revision,
            created_at: wire.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn binding(engine: &str, host: &str, port: Option<u16>) -> TargetBinding {
        TargetBinding::new(engine, host, port, "app", "alice", None)
    }

    #[rstest]
    #[case(
        "Postgres",
        "DB.Example.COM ",
        None,
        "postgresql",
        "db.example.com",
        Some(5432)
    )]
    #[case("MariaDB", "[::1]", None, "mysql", "::1", Some(3306))]
    #[case(
        "mongodb+srv",
        "cluster.example.net",
        Some(27018),
        "mongodb",
        "cluster.example.net",
        Some(27018)
    )]
    #[case("sqlite", "localhost", None, "sqlite", "localhost", None)]
    fn binding_normalization(
        #[case] engine: &str,
        #[case] host: &str,
        #[case] port: Option<u16>,
        #[case] want_engine: &str,
        #[case] want_host: &str,
        #[case] want_port: Option<u16>,
    ) {
        let b = binding(engine, host, port);
        assert_eq!(b.engine, want_engine);
        assert_eq!(b.host, want_host);
        assert_eq!(b.port, want_port);
        assert_eq!(b, b.normalized(), "normalization is idempotent");
    }

    #[test]
    fn matches_ignores_case_aliases_and_default_ports() {
        let stored = binding("postgresql", "db.example.com", Some(5432));
        assert!(stored.matches(&binding("PG", "DB.example.com", None)));
        assert!(!stored.matches(&binding("postgresql", "evil.example.com", Some(5432))));
        assert!(!stored.matches(&binding("postgresql", "db.example.com", Some(5433))));
        assert!(!stored.matches(&TargetBinding::new(
            "postgresql",
            "db.example.com",
            None,
            "other",
            "alice",
            None
        )));
        assert!(!stored.matches(&TargetBinding::new(
            "postgresql",
            "db.example.com",
            None,
            "app",
            "bob",
            None
        )));
    }

    #[test]
    fn tls_mode_is_compared_only_when_both_sides_know_it() {
        let with_tls = TargetBinding::new("postgresql", "h", None, "d", "u", Some("Require"));
        let without = TargetBinding::new("postgresql", "h", None, "d", "u", None);
        let other = TargetBinding::new("postgresql", "h", None, "d", "u", Some("disable"));
        assert!(with_tls.matches(&without));
        assert!(without.matches(&with_tls));
        assert!(with_tls.matches(&TargetBinding::new(
            "postgresql",
            "h",
            None,
            "d",
            "u",
            Some("require")
        )));
        assert!(!with_tls.matches(&other));
    }

    #[test]
    fn record_ids_are_normalized_and_percent_encoded() {
        let b = TargetBinding::new("PG", "DB.example.com", None, "my/db:1", "al@ice", None);
        assert_eq!(
            b.record_id(),
            "conn:postgresql:al%40ice@db.example.com:5432/my%2Fdb%3A1"
        );
        assert_eq!(RecordId::ai("OpenAI"), "ai:openai");
        assert_eq!(RecordId::oauth("chatgpt tokens"), "oauth:chatgpt%20tokens");
        assert_eq!(RecordId::broker("prod"), "broker:prod");
        assert_eq!(
            RecordId::parse("broker:prod").map(|id| id.kind),
            Some(RecordKind::Broker)
        );
        assert!(RecordId::parse("nonsense:1").is_none());
    }

    #[rstest]
    #[case(AgentPolicy::default(), AgentPolicy { access: AgentAccess::ReadWrite, max_rows: 1_000_000, timeout_secs: 9_999, max_bytes: u64::MAX })]
    #[case(AgentPolicy { access: AgentAccess::ReadWrite, max_rows: 10_000, timeout_secs: 600, max_bytes: HARD_MAX_BYTES }, AgentPolicy { access: AgentAccess::ReadWrite, max_rows: 20_000, timeout_secs: 900, max_bytes: u64::MAX })]
    #[case(AgentPolicy { access: AgentAccess::ReadOnly, max_rows: 10, timeout_secs: 2, max_bytes: 100 }, AgentPolicy { access: AgentAccess::ReadWrite, max_rows: 500, timeout_secs: 30, max_bytes: 1 << 20 })]
    fn tighten_never_loosens(#[case] ceiling: AgentPolicy, #[case] requested: AgentPolicy) {
        let effective = ceiling.tighten(&requested);
        let cap = ceiling.clamped();
        assert!(effective.access <= cap.access);
        assert!(effective.access <= requested.access);
        assert!(effective.max_rows <= cap.max_rows.min(requested.max_rows));
        assert!(effective.timeout_secs <= cap.timeout_secs.min(requested.timeout_secs));
        assert!(effective.max_bytes <= cap.max_bytes.min(requested.max_bytes));
        assert!(effective.max_rows <= HARD_MAX_ROWS);
        assert!(effective.timeout_secs <= HARD_MAX_TIMEOUT_SECS);
        assert!(effective.max_bytes <= HARD_MAX_BYTES);
    }

    #[test]
    fn policy_defaults_and_partial_requests() {
        let policy = AgentPolicy::default();
        assert_eq!(policy.access, AgentAccess::ReadOnly);
        assert_eq!(policy.max_rows, 500);
        assert_eq!(policy.timeout_secs, 30);
        assert_eq!(policy.max_bytes, 8 * 1024 * 1024);

        let narrowed = policy.tighten_with(PolicyRequest {
            max_rows: Some(50),
            access: Some(AgentAccess::ReadWrite),
            ..Default::default()
        });
        assert_eq!(narrowed.max_rows, 50);
        assert_eq!(narrowed.access, AgentAccess::ReadOnly);
        assert_eq!(narrowed.timeout_secs, 30);

        let oversized = AgentPolicy {
            max_rows: u32::MAX,
            ..AgentPolicy::default()
        };
        assert_eq!(oversized.clamped().max_rows, HARD_MAX_ROWS);
    }

    #[test]
    fn record_json_round_trip_and_redacted_debug() {
        let mut options = BTreeMap::new();
        options.insert(
            "sslpassword".to_string(),
            Zeroizing::new("CANARY-OPT".to_string()),
        );
        let mut record = SecretRecord::new(
            "CANARY-PW",
            options,
            binding("postgres", "db.example.com", None),
        );
        record.agent_policy = Some(AgentPolicy::default());
        record.revision = 3;

        let json = record.to_json().unwrap();
        let back = SecretRecord::from_json(&json).unwrap();
        assert_eq!(&*back.secret, "CANARY-PW");
        assert_eq!(&*back.secret_options["sslpassword"], "CANARY-OPT");
        assert_eq!(back.bound_target, record.bound_target);
        assert_eq!(back.agent_policy, Some(AgentPolicy::default()));
        assert_eq!(back.revision, 3);

        let debug = format!("{record:?}");
        assert!(!debug.contains("CANARY"), "{debug}");
        assert!(debug.contains("sslpassword"));
    }

    #[test]
    fn unsupported_version_and_garbage_are_corrupt() {
        let err = SecretRecord::from_json(
            "{\"v\":99,\"secret\":\"x\",\"bound_target\":{\"engine\":\"a\",\"host\":\"b\"}}",
        )
        .unwrap_err();
        assert!(matches!(err, SecretError::Corrupt(_)));
        let err = SecretRecord::from_json("not json CANARY").unwrap_err();
        assert!(matches!(err, SecretError::Corrupt(_)));
        assert!(!err.to_string().contains("CANARY"));
    }
}
