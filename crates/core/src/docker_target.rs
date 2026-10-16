//! `docker://[user[:password]@]container[/database][?options]` targets: the
//! pure string side of Docker support (parsing, rendering, converting to and
//! from [`ConnectionInfo`]). No Docker daemon access lives here; see
//! the root crate's `docker` module for that, which re-exports these types.

use crate::database::{ConnectionInfo, DatabaseType};
use std::collections::BTreeMap;

/// Option key under which a connection request pins the Docker daemon (the
/// value of `DockerEndpoint::pinned_value` in the Docker layer). It travels in
/// `ConnectionInfo::options` between a live connection and its siblings and
/// is consumed by `Database::connect_docker_container`; it is
/// never handed to a database engine and never persisted.
pub const PINNED_DAEMON_OPTION: &str = "docker_daemon";

/// A parsed `docker://[user[:password]@]container[/database][?options]` URL.
///
/// Parts are taken verbatim (no percent-decoding, as `docker://` URLs always
/// were) except option values, which are percent-encoded.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DockerTarget {
    pub user: Option<String>,
    pub password: Option<String>,
    /// Empty for `docker://` (pick interactively).
    pub container: String,
    pub database: Option<String>,
    /// Connection options (`sslmode=require`, ...). `ssl=true` is kept as
    /// [`DockerTarget::use_tls`].
    pub options: BTreeMap<String, String>,
    pub use_tls: bool,
}

/// Everything except unreserved characters is percent-encoded in option
/// values.
const OPTION_ENCODE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

impl std::fmt::Debug for DockerTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerTarget")
            .field("user", &self.user)
            .field(
                "password",
                &crate::password_sanitizer::mask_secret(&self.password),
            )
            .field("container", &self.container)
            .field("database", &self.database)
            .field(
                "options",
                &crate::password_sanitizer::redact_options(&self.options),
            )
            .field("use_tls", &self.use_tls)
            .finish()
    }
}

impl DockerTarget {
    /// `None` unless `url` starts with `docker://`.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.strip_prefix("docker://")?;
        let (credentials, rest) = match rest.find('@') {
            Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
            None => (None, rest),
        };
        let (user, password) = match credentials {
            Some(credentials) => match credentials.split_once(':') {
                Some((user, password)) => (Some(user.to_string()), Some(password.to_string())),
                None => (Some(credentials.to_string()), None),
            },
            None => (None, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((rest, query)) => (rest, Some(query)),
            None => (rest, None),
        };
        let (container, database) = match rest.split_once('/') {
            Some((container, database)) => (container, Some(database.to_string())),
            None => (rest, None),
        };
        let mut options = BTreeMap::new();
        for pair in query.into_iter().flat_map(|q| q.split('&')) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if key.is_empty() {
                continue;
            }
            let decode = |text: &str| {
                percent_encoding::percent_decode_str(text)
                    .decode_utf8_lossy()
                    .into_owned()
            };
            options.insert(decode(key), decode(value));
        }
        let use_tls = options
            .remove("ssl")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"));
        Some(Self {
            user,
            password,
            container: container.to_string(),
            database,
            options,
            use_tls,
        })
    }

    /// The target of an open (or resolved) container connection. The password
    /// is kept only so callers can reuse it in memory; [`DockerTarget::render`]
    /// never writes it.
    pub fn from_connection_info(info: &ConnectionInfo) -> Option<Self> {
        let container = info.docker_container.as_deref().filter(|c| !c.is_empty())?;
        Some(Self {
            user: info.username.clone().filter(|u| !u.is_empty()),
            password: info.password.clone().filter(|p| !p.is_empty()),
            container: container.to_string(),
            database: info.database.clone().filter(|d| !d.is_empty()),
            options: info
                .options
                .iter()
                .filter(|(key, _)| key.as_str() != PINNED_DAEMON_OPTION)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            use_tls: info.use_tls,
        })
    }

    /// Connection details to resolve against the container: the parts the URL
    /// named (the engine, host and port are filled in from the container).
    pub fn to_connection_info(&self) -> ConnectionInfo {
        ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            username: self.user.clone(),
            password: self.password.clone(),
            database: self.database.clone(),
            options: self
                .options
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            docker_container: Some(self.container.clone()),
            use_tls: self.use_tls,
            ..Default::default()
        }
    }

    /// The canonical URL to persist (recents, sessions): container, user,
    /// database and options. **Never the password** and never an address (a
    /// relay's port changes per connection). Parts that could not survive a
    /// round trip through the verbatim URL syntax are left out (the
    /// container's own environment then supplies the default on reconnect),
    /// and so are options that look like secrets.
    pub fn render(&self) -> String {
        let mut url = String::from("docker://");
        if let Some(user) = self
            .user
            .as_deref()
            .filter(|u| !u.is_empty() && !u.contains(['@', ':', '/', '?', '#']))
        {
            url.push_str(user);
            url.push('@');
        }
        url.push_str(&self.container);
        if let Some(database) = self
            .database
            .as_deref()
            .filter(|d| !d.is_empty() && !d.contains(['?', '#']))
        {
            url.push('/');
            url.push_str(database);
        }
        let mut query: Vec<String> = self
            .options
            .iter()
            .filter(|(key, _)| {
                let key = key.to_ascii_lowercase();
                !(key.contains("password") || key.contains("secret") || key.contains("token"))
            })
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    percent_encoding::utf8_percent_encode(key, OPTION_ENCODE),
                    percent_encoding::utf8_percent_encode(value, OPTION_ENCODE)
                )
            })
            .collect();
        if self.use_tls {
            query.push("ssl=true".to_string());
        }
        query.sort();
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn the_canonical_target_round_trips_without_password_or_address() {
        let target = DockerTarget::parse(
            "docker://app:p%40ss@tt2-postgres-1/shop?sslmode=require&ssl=true&search_path=a%20b",
        )
        .unwrap();
        assert!(target.use_tls);
        let rendered = target.render();
        assert_eq!(
            rendered,
            "docker://app@tt2-postgres-1/shop?search_path=a%20b&ssl=true&sslmode=require"
        );
        assert!(!rendered.contains("p%40ss") && !rendered.contains("pw"));
        let again = DockerTarget::parse(&rendered).unwrap();
        assert_eq!(again.user.as_deref(), Some("app"));
        assert_eq!(again.password, None);
        assert_eq!(again.database.as_deref(), Some("shop"));
        assert_eq!(again.options, target.options);
        assert!(again.use_tls);
        assert_eq!(again.render(), rendered);
    }

    #[rstest]
    // user names that the verbatim syntax can't carry are left out
    #[case(Some("a@b"), Some("db"), "docker://c/db")]
    #[case(Some("a:b"), Some("db"), "docker://c/db")]
    #[case(Some("app"), Some("we?ird"), "docker://app@c")]
    #[case(None, None, "docker://c")]
    fn what_cannot_round_trip_is_left_to_the_container_environment(
        #[case] user: Option<&str>,
        #[case] database: Option<&str>,
        #[case] expected: &str,
    ) {
        let target = DockerTarget {
            user: user.map(str::to_string),
            container: "c".into(),
            database: database.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(target.render(), expected);
    }

    #[test]
    fn secrets_never_reach_the_persisted_target() {
        let mut info = DockerTarget::parse("docker://app@c/db")
            .unwrap()
            .to_connection_info();
        info.password = Some("hunter2".into());
        info.options
            .insert("password_command".into(), "pass show db".into());
        info.options.insert("API_TOKEN".into(), "abc".into());
        info.options.insert("sslmode".into(), "require".into());
        info.options
            .insert(PINNED_DAEMON_OPTION.into(), "unix:///x.sock".into());
        let rendered = DockerTarget::from_connection_info(&info).unwrap().render();
        assert_eq!(rendered, "docker://app@c/db?sslmode=require");
    }
}
