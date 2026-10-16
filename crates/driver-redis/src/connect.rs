//! `ConnectionInfo` → the `redis` crate's connection description.
//!
//! Pure: no I/O, so the URL/option mapping (ACL user, database index, TLS,
//! IPv6, unix sockets) is testable without a server.

use dbcrust_core::database::{ConnectionInfo, DatabaseError};
use redis::{ConnectionAddr, IntoConnectionInfo, RedisConnectionInfo};
use std::time::Duration;

pub(crate) const DEFAULT_HOST: &str = "127.0.0.1";
pub(crate) const DEFAULT_PORT: u16 = 6379;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Connection option: seconds to wait for the TCP/TLS connection.
pub(crate) const OPT_CONNECT_TIMEOUT: &str = "connect_timeout";

/// The logical database index of a connection (`/N` in the URL), 0 by default.
pub(crate) fn database_index(info: &ConnectionInfo) -> Result<i64, DatabaseError> {
    match info.database.as_deref().map(str::trim) {
        None | Some("") => Ok(0),
        Some(text) => text
            .strip_prefix("db")
            .unwrap_or(text)
            .parse::<i64>()
            .ok()
            .filter(|index| *index >= 0)
            .ok_or_else(|| {
                DatabaseError::InvalidUrl(format!(
                    "the Redis database must be a number (redis://host/0), got '{text}'"
                ))
            }),
    }
}

/// Seconds → timeout from the `connect_timeout` option.
pub(crate) fn connect_timeout(info: &ConnectionInfo) -> Duration {
    info.options
        .get(OPT_CONNECT_TIMEOUT)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs)
}

/// Build the `redis` crate's connection info for `info` at database `db`.
pub(crate) fn build_connection_info(
    info: &ConnectionInfo,
    db: i64,
) -> Result<redis::ConnectionInfo, DatabaseError> {
    let addr = if let Some(socket) = &info.file_path {
        ConnectionAddr::Unix(socket.into())
    } else {
        let host = info
            .host
            .as_deref()
            .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
            .filter(|host| !host.is_empty())
            .unwrap_or(DEFAULT_HOST)
            .to_string();
        let port = info.port.unwrap_or(DEFAULT_PORT);
        if info.use_tls {
            ConnectionAddr::TcpTls {
                host,
                port,
                // Certificate and hostname verification are never disabled
                insecure: false,
                tls_params: None,
            }
        } else {
            ConnectionAddr::Tcp(host, port)
        }
    };

    let mut redis_settings = RedisConnectionInfo::default()
        .set_db(db)
        .set_lib_name("dbcrust", env!("CARGO_PKG_VERSION"));
    if let Some(user) = info.username.as_deref().filter(|user| !user.is_empty()) {
        redis_settings = redis_settings.set_username(user);
    }
    if let Some(password) = info.password.as_deref().filter(|pw| !pw.is_empty()) {
        redis_settings = redis_settings.set_password(password);
    }
    addr.into_connection_info()
        .map(|connection| connection.set_redis_settings(redis_settings))
        .map_err(|error| DatabaseError::InvalidUrl(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcrust_core::database::DatabaseType;
    use rstest::rstest;

    fn info(url: &str) -> ConnectionInfo {
        ConnectionInfo::parse_url(url).unwrap()
    }

    #[rstest]
    #[case("redis://localhost", 0)]
    #[case("redis://localhost/0", 0)]
    #[case("redis://localhost/7", 7)]
    #[case("redis://localhost/db3", 3)]
    fn database_index_from_the_path(#[case] url: &str, #[case] expected: i64) {
        assert_eq!(database_index(&info(url)).unwrap(), expected);
    }

    #[rstest]
    #[case("redis://localhost/main")]
    #[case("redis://localhost/-1")]
    fn database_index_must_be_a_number(#[case] url: &str) {
        assert!(matches!(
            database_index(&info(url)),
            Err(DatabaseError::InvalidUrl(_))
        ));
    }

    #[test]
    fn tcp_target_with_acl_user_and_database() {
        let connection = build_connection_info(&info("redis://app:pw@cache:6380/2"), 2).unwrap();
        assert_eq!(
            connection.addr(),
            &ConnectionAddr::Tcp("cache".to_string(), 6380)
        );
        assert_eq!(connection.redis_settings().db(), 2);
        assert_eq!(connection.redis_settings().username(), Some("app"));
        assert_eq!(connection.redis_settings().password(), Some("pw"));
    }

    #[test]
    fn defaults_to_localhost_and_6379() {
        let blank = ConnectionInfo {
            database_type: DatabaseType::Redis,
            ..Default::default()
        };
        let connection = build_connection_info(&blank, 0).unwrap();
        assert_eq!(
            connection.addr(),
            &ConnectionAddr::Tcp(DEFAULT_HOST.to_string(), DEFAULT_PORT)
        );
        assert_eq!(connection.redis_settings().username(), None);
        assert_eq!(connection.redis_settings().password(), None);
    }

    #[test]
    fn rediss_verifies_the_certificate() {
        let connection = build_connection_info(&info("rediss://cache:6390"), 0).unwrap();
        match connection.addr() {
            ConnectionAddr::TcpTls {
                host,
                port,
                insecure,
                ..
            } => {
                assert_eq!((host.as_str(), *port, *insecure), ("cache", 6390, false));
            }
            other => panic!("expected TLS, got {other:?}"),
        }
    }

    #[test]
    fn ipv6_literals_lose_their_brackets() {
        let connection = build_connection_info(&info("redis://[::1]:6379"), 0).unwrap();
        assert_eq!(
            connection.addr(),
            &ConnectionAddr::Tcp("::1".to_string(), 6379)
        );
    }

    #[test]
    fn unix_socket_target() {
        let connection = build_connection_info(&info("redis+unix:///run/redis.sock"), 0).unwrap();
        assert_eq!(
            connection.addr(),
            &ConnectionAddr::Unix("/run/redis.sock".into())
        );
    }

    #[test]
    fn the_password_never_shows_in_debug_output() {
        let connection = build_connection_info(&info("redis://app:Cnry-7f3a9@cache/0"), 0).unwrap();
        assert!(!format!("{connection:?}").contains("Cnry-7f3a9"));
    }

    #[rstest]
    #[case(None, 10)]
    #[case(Some("3"), 3)]
    #[case(Some("0"), 10)]
    #[case(Some("soon"), 10)]
    fn connect_timeout_option(#[case] value: Option<&str>, #[case] expected_secs: u64) {
        let mut connection = info("redis://localhost");
        if let Some(value) = value {
            connection
                .options
                .insert(OPT_CONNECT_TIMEOUT.to_string(), value.to_string());
        }
        assert_eq!(
            connect_timeout(&connection),
            Duration::from_secs(expected_secs)
        );
    }
}
