//! What an AWS connection asks for, read from a [`ConnectionInfo`], and which
//! credentials that selects.
//!
//! The mapping (shared by every AWS connector):
//!
//! | `ConnectionInfo`            | Meaning                                              |
//! |-----------------------------|------------------------------------------------------|
//! | `host`, `port`, `use_tls`   | custom endpoint (MinIO, R2, LocalStack, VPC endpoint); none = AWS |
//! | `username` + `password`     | static `access_key_id` + `secret_access_key`         |
//! | option `session_token`      | session token of static temporary credentials (secret) |
//! | option `profile`            | named profile of `~/.aws/config` (SSO, role, keys)   |
//! | option `region`             | region (else profile, else environment)              |
//! | option `path_style`         | force path-style addressing (default with a custom endpoint) |
//! | option `anonymous`          | unsigned requests (public buckets)                   |
//! | options `role_arn`, `external_id`, `session_name` | assume a role on top of the credentials |
//! | option `ca_cert`            | PEM file trusted in addition to the system roots     |
//! | options `connect_timeout`, `operation_timeout`, `max_attempts` | limits (seconds / attempts) |

use crate::AwsError;
use dbcrust_core::database::ConnectionInfo;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

const OPT_REGION: &str = "region";
const OPT_PROFILE: &str = "profile";
const OPT_PATH_STYLE: [&str; 2] = ["path_style", "force_path_style"];
const OPT_ANONYMOUS: &str = "anonymous";
const OPT_SESSION_TOKEN: &str = "session_token";
const OPT_ROLE_ARN: &str = "role_arn";
const OPT_EXTERNAL_ID: &str = "external_id";
const OPT_SESSION_NAME: &str = "session_name";
const OPT_CA_CERT: &str = "ca_cert";
const OPT_CONNECT_TIMEOUT: &str = "connect_timeout";
const OPT_OPERATION_TIMEOUT: &str = "operation_timeout";
const OPT_MAX_ATTEMPTS: &str = "max_attempts";

/// Seconds to wait for a TCP + TLS connection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Attempts per operation (the first included) before the error is reported.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Static AWS keys. The secret key and session token never appear in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct StaticCredentials {
    pub access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) session_token: Option<String>,
}

impl StaticCredentials {
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
    ) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token: session_token.filter(|token| !token.is_empty()),
        }
    }

    /// Every secret value, for scrubbing error text.
    pub(crate) fn secrets(&self) -> Vec<&str> {
        let mut secrets = vec![self.secret_access_key.as_str()];
        if let Some(token) = &self.session_token {
            secrets.push(token);
        }
        secrets
    }
}

impl fmt::Debug for StaticCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"[REDACTED]")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Which credentials a connection uses, in decreasing priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// Requests are not signed (public buckets).
    Anonymous,
    /// The keys given in the URL / secret store.
    Static,
    /// A named profile of the shared AWS config, whatever the environment says
    /// (`AWS_ACCESS_KEY_ID` does not override an explicit `?profile=`).
    Profile(String),
    /// The SDK default chain: environment variables → shared config (`default`
    /// or `AWS_PROFILE`, SSO included) → web identity → ECS → EC2 IMDS.
    DefaultChain,
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anonymous => f.write_str("anonymous (unsigned requests)"),
            Self::Static => f.write_str("static access key"),
            Self::Profile(name) => write!(f, "profile '{name}'"),
            Self::DefaultChain => f.write_str("the AWS default credential chain"),
        }
    }
}

/// The AWS part of a connection.
#[derive(Debug, Clone)]
pub struct AwsSettings {
    pub region: Option<String>,
    pub profile: Option<String>,
    /// `scheme://host[:port]` of a non-AWS (or VPC) endpoint.
    pub endpoint_url: Option<String>,
    pub force_path_style: bool,
    pub anonymous: bool,
    pub credentials: Option<StaticCredentials>,
    pub role_arn: Option<String>,
    pub external_id: Option<String>,
    pub session_name: Option<String>,
    pub ca_cert: Option<PathBuf>,
    pub connect_timeout: Duration,
    /// Whole-operation deadline enforced by the SDK; `None` leaves it to the
    /// caller (the drivers also race every call against the query timeout).
    pub operation_timeout: Option<Duration>,
    pub max_attempts: u32,
}

impl Default for AwsSettings {
    fn default() -> Self {
        Self {
            region: None,
            profile: None,
            endpoint_url: None,
            force_path_style: false,
            anonymous: false,
            credentials: None,
            role_arn: None,
            external_id: None,
            session_name: None,
            ca_cert: None,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            operation_timeout: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }
}

fn option<'a>(info: &'a ConnectionInfo, key: &str) -> Option<&'a str> {
    info.options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn flag(info: &ConnectionInfo, keys: &[&str]) -> Result<Option<bool>, AwsError> {
    for key in keys {
        if let Some(value) = option(info, key) {
            return match value.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Some(true)),
                "false" | "0" | "no" | "off" => Ok(Some(false)),
                other => Err(AwsError::InvalidOption(format!(
                    "{key} must be true or false, got '{other}'"
                ))),
            };
        }
    }
    Ok(None)
}

fn bounded_number(
    info: &ConnectionInfo,
    key: &str,
    range: std::ops::RangeInclusive<u64>,
) -> Result<Option<u64>, AwsError> {
    let Some(value) = option(info, key) else {
        return Ok(None);
    };
    match value.parse::<u64>() {
        Ok(number) if range.contains(&number) => Ok(Some(number)),
        _ => Err(AwsError::InvalidOption(format!(
            "{key} must be a whole number between {} and {}, got '{value}'",
            range.start(),
            range.end()
        ))),
    }
}

/// `eu-west-1`, `us-gov-east-1`, `auto` (R2): lowercase letters, digits, dashes.
fn valid_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn valid_profile(profile: &str) -> bool {
    !profile.is_empty()
        && profile.len() <= 128
        && !profile
            .chars()
            .any(|c| c.is_control() || matches!(c, '[' | ']'))
}

fn endpoint_url(info: &ConnectionInfo) -> Result<Option<String>, AwsError> {
    let Some(host) = info
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    else {
        return Ok(None);
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.is_empty()
        || bare
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '@' | '?' | '#' | '\\'))
    {
        return Err(AwsError::InvalidOption(format!(
            "'{host}' is not a valid endpoint host"
        )));
    }
    let scheme = if info.use_tls { "https" } else { "http" };
    let host = if bare.contains(':') {
        format!("[{bare}]")
    } else {
        bare.to_string()
    };
    Ok(Some(match info.port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }))
}

impl AwsSettings {
    /// Read the settings from a parsed connection. Fails on contradictions
    /// (anonymous with credentials, keys with a profile) and malformed values,
    /// never on a missing credential: the chain reports that when it is used.
    pub fn from_connection_info(info: &ConnectionInfo) -> Result<Self, AwsError> {
        let mut settings = Self::default();

        let user = info.username.as_deref().filter(|u| !u.is_empty());
        let secret = info.password.as_deref().filter(|p| !p.is_empty());
        match (user, secret) {
            (Some(id), Some(secret)) => {
                settings.credentials = Some(StaticCredentials::new(
                    id,
                    secret,
                    option(info, OPT_SESSION_TOKEN).map(str::to_string),
                ));
            }
            (Some(_), None) => {
                return Err(AwsError::InvalidOption(
                    "authentication required: an access key id needs its secret access key \
                     (ACCESS_KEY:SECRET@, or a saved one); for ambient credentials leave the \
                     user out or use ?profile="
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(AwsError::InvalidOption(
                    "a secret access key needs its access key id (ACCESS_KEY:SECRET@)".to_string(),
                ));
            }
            (None, None) => {
                if option(info, OPT_SESSION_TOKEN).is_some() {
                    return Err(AwsError::InvalidOption(
                        "session_token goes with an access key id and secret access key"
                            .to_string(),
                    ));
                }
            }
        }

        if let Some(region) = option(info, OPT_REGION) {
            if !valid_region(region) {
                return Err(AwsError::InvalidOption(format!(
                    "'{region}' is not a valid AWS region (e.g. eu-west-1; auto for R2)"
                )));
            }
            settings.region = Some(region.to_string());
        }
        if let Some(profile) = option(info, OPT_PROFILE) {
            if !valid_profile(profile) {
                return Err(AwsError::InvalidOption(format!(
                    "'{profile}' is not a valid profile name"
                )));
            }
            settings.profile = Some(profile.to_string());
        }
        settings.endpoint_url = endpoint_url(info)?;
        // Path style by default with a custom endpoint (MinIO has no
        // bucket.host DNS); AWS' own hosts keep virtual-hosted addressing
        let aws_host = info.host.as_deref().is_some_and(|host| {
            host.ends_with(".amazonaws.com") || host.ends_with(".amazonaws.com.cn")
        });
        settings.force_path_style =
            flag(info, &OPT_PATH_STYLE)?.unwrap_or(settings.endpoint_url.is_some() && !aws_host);
        settings.anonymous = flag(info, &[OPT_ANONYMOUS])?.unwrap_or(false);

        settings.role_arn = option(info, OPT_ROLE_ARN).map(str::to_string);
        if let Some(arn) = &settings.role_arn {
            if !arn.starts_with("arn:") {
                return Err(AwsError::InvalidOption(format!(
                    "role_arn must be an ARN (arn:aws:iam::123456789012:role/name), got '{arn}'"
                )));
            }
        }
        settings.external_id = option(info, OPT_EXTERNAL_ID).map(str::to_string);
        settings.session_name = option(info, OPT_SESSION_NAME).map(str::to_string);
        settings.ca_cert = option(info, OPT_CA_CERT).map(PathBuf::from);

        if let Some(seconds) = bounded_number(info, OPT_CONNECT_TIMEOUT, 1..=600)? {
            settings.connect_timeout = Duration::from_secs(seconds);
        }
        settings.operation_timeout = bounded_number(info, OPT_OPERATION_TIMEOUT, 1..=86_400)?
            .map(Duration::from_secs)
            .or_else(dbcrust_core::database::query_timeout);
        if let Some(attempts) = bounded_number(info, OPT_MAX_ATTEMPTS, 1..=10)? {
            settings.max_attempts = attempts as u32;
        }

        settings.validate()?;
        Ok(settings)
    }

    /// Contradictions that no amount of environment can resolve.
    pub fn validate(&self) -> Result<(), AwsError> {
        if self.anonymous {
            if self.credentials.is_some() {
                return Err(AwsError::InvalidOption(
                    "anonymous=true contradicts the access key in the URL".to_string(),
                ));
            }
            if self.profile.is_some() || self.role_arn.is_some() {
                return Err(AwsError::InvalidOption(
                    "anonymous=true contradicts profile / role_arn".to_string(),
                ));
            }
        }
        if self.credentials.is_some() && self.profile.is_some() {
            return Err(AwsError::InvalidOption(
                "an access key in the URL and ?profile= are exclusive: pick one".to_string(),
            ));
        }
        if (self.external_id.is_some() || self.session_name.is_some()) && self.role_arn.is_none() {
            return Err(AwsError::InvalidOption(
                "external_id / session_name only apply with role_arn".to_string(),
            ));
        }
        Ok(())
    }

    /// The credentials this connection uses: anonymous, then static keys, then
    /// an explicit profile, then the SDK default chain.
    pub fn credential_source(&self) -> CredentialSource {
        if self.anonymous {
            CredentialSource::Anonymous
        } else if self.credentials.is_some() {
            CredentialSource::Static
        } else if let Some(profile) = &self.profile {
            CredentialSource::Profile(profile.clone())
        } else {
            CredentialSource::DefaultChain
        }
    }

    /// Secret values to scrub from any text shown to the user.
    pub fn secrets(&self) -> Vec<&str> {
        self.credentials
            .as_ref()
            .map(StaticCredentials::secrets)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
