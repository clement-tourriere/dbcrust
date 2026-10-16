//! Client tests against an in-process fake server (RESP2 over TCP, canned
//! replies), plus opt-in integration tests against a real server
//! (`REDIS_URL`, skipped when unset).

use super::*;
use dbcrust_core::options::OPT_HARDEN_READ_ONLY;
use rstest::rstest;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Handler = dyn Fn(&[String]) -> Vec<u8> + Send + Sync;

/// A RESP2 server answering every command with what `handler` returns (raw
/// RESP bytes) and recording the commands it received.
struct FakeRedis {
    addr: SocketAddr,
    commands: Arc<Mutex<Vec<Vec<Vec<u8>>>>>,
}

impl FakeRedis {
    async fn start(handler: impl Fn(&[String]) -> Vec<u8> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let commands: Arc<Mutex<Vec<Vec<Vec<u8>>>>> = Arc::default();
        let handler: Arc<Handler> = Arc::new(handler);
        let log = commands.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buffer: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        while let Some((args, used)) = parse_command(&buffer) {
                            buffer.drain(..used);
                            let text: Vec<String> = args
                                .iter()
                                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                                .collect();
                            log.lock().unwrap().push(args);
                            let reply = handler(&text);
                            if socket.write_all(&reply).await.is_err() {
                                return;
                            }
                        }
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                        }
                    }
                });
            }
        });
        Self { addr, commands }
    }

    fn info(&self) -> ConnectionInfo {
        ConnectionInfo {
            database_type: DatabaseType::Redis,
            host: Some("127.0.0.1".to_string()),
            port: Some(self.addr.port()),
            ..Default::default()
        }
    }

    /// Commands received so far as text, in order.
    fn received(&self) -> Vec<Vec<String>> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .map(|args| {
                args.iter()
                    .map(|arg| String::from_utf8_lossy(arg).into_owned())
                    .collect()
            })
            .collect()
    }

    fn received_raw(&self) -> Vec<Vec<Vec<u8>>> {
        self.commands.lock().unwrap().clone()
    }
}

/// One `*N\r\n$len\r\n…` command from the front of `buffer`, with the bytes it
/// used, or `None` when incomplete.
fn parse_command(buffer: &[u8]) -> Option<(Vec<Vec<u8>>, usize)> {
    fn line(buffer: &[u8], from: usize) -> Option<(&[u8], usize)> {
        let end = buffer[from..].windows(2).position(|pair| pair == b"\r\n")?;
        Some((&buffer[from..from + end], from + end + 2))
    }
    let (header, mut at) = line(buffer, 0)?;
    let count: usize = std::str::from_utf8(header.strip_prefix(b"*")?)
        .ok()?
        .parse()
        .ok()?;
    let mut args = Vec::new();
    for _ in 0..count {
        let (len_line, next) = line(buffer, at)?;
        let len: usize = std::str::from_utf8(len_line.strip_prefix(b"$")?)
            .ok()?
            .parse()
            .ok()?;
        if buffer.len() < next + len + 2 {
            return None;
        }
        args.push(buffer[next..next + len].to_vec());
        at = next + len + 2;
    }
    Some((args, at))
}

fn ok() -> Vec<u8> {
    b"+OK\r\n".to_vec()
}

fn bulk(text: &str) -> Vec<u8> {
    format!("${}\r\n{text}\r\n", text.len()).into_bytes()
}

fn array(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = format!("*{}\r\n", items.len()).into_bytes();
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

fn integer(number: i64) -> Vec<u8> {
    format!(":{number}\r\n").into_bytes()
}

fn error(text: &str) -> Vec<u8> {
    format!("-{text}\r\n").into_bytes()
}

/// Default behaviour of a server with nothing special to say.
fn quiet(args: &[String]) -> Vec<u8> {
    match args[0].to_ascii_uppercase().as_str() {
        "PING" => b"+PONG\r\n".to_vec(),
        _ => ok(),
    }
}

async fn connect(server: &FakeRedis) -> RedisClient {
    RedisClient::new(server.info()).await.expect("fake server")
}

#[tokio::test]
async fn connect_authenticates_with_the_acl_user_and_selects_the_database() {
    let server = FakeRedis::start(quiet).await;
    let mut info = server.info();
    info.username = Some("app".into());
    info.password = Some("s3cret".into());
    info.database = Some("3".into());
    let client = RedisClient::new(info).await.unwrap();
    assert_eq!(client.get_current_database(), "3");

    let received = server.received();
    assert!(
        received.contains(&vec!["AUTH".into(), "app".into(), "s3cret".into()]),
        "{received:?}"
    );
    assert!(
        received.contains(&vec!["SELECT".into(), "3".into()]),
        "{received:?}"
    );
}

#[tokio::test]
async fn a_rejected_login_is_a_connection_error_without_the_password() {
    let server = FakeRedis::start(|args| match args[0].to_ascii_uppercase().as_str() {
        "AUTH" => error("WRONGPASS invalid username-password pair or user is disabled."),
        _ => ok(),
    })
    .await;
    let mut info = server.info();
    info.username = Some("app".into());
    info.password = Some("Cnry-7f3a9".into());
    let Err(error) = RedisClient::new(info).await else {
        panic!("login must fail");
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(
        text.to_lowercase().contains("authentication failed"),
        "{text}"
    );
    assert!(!text.contains("Cnry-7f3a9"), "{text}");
}

#[tokio::test]
async fn a_closed_port_is_a_connection_error_that_keeps_the_password_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let info = ConnectionInfo {
        database_type: DatabaseType::Redis,
        host: Some("127.0.0.1".into()),
        port: Some(port),
        username: Some("app".into()),
        password: Some("Cnry-7f3a9".into()),
        ..Default::default()
    };
    let Err(error) = RedisClient::new(info).await else {
        panic!("nothing listens there");
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );
    assert!(!error.to_string().contains("Cnry-7f3a9"), "{error}");
    assert!(!format!("{error:?}").contains("Cnry-7f3a9"), "{error:?}");
}

#[tokio::test]
async fn rediss_against_a_plain_server_fails_the_handshake_instead_of_panicking() {
    // No TLS server here: the point is that the rustls provider is installed
    // (a missing process-default provider panics in `ClientConfig::builder`)
    // and that the failed handshake surfaces as a connection error
    let server = FakeRedis::start(quiet).await;
    let mut info = server.info();
    info.use_tls = true;
    info.options.insert("connect_timeout".into(), "1".into());
    let Err(error) = RedisClient::new(info).await else {
        panic!("a plain server cannot complete a TLS handshake");
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn get_returns_one_value_cell() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "GET" => bulk("hello"),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let result = client
        .execute_query_structured("GET greeting")
        .await
        .unwrap();
    assert_eq!(result.columns, ["value"]);
    assert_eq!(result.rows, [vec![Some("hello".to_string())]]);
    assert!(
        server
            .received()
            .contains(&vec!["GET".into(), "greeting".into()])
    );
}

#[tokio::test]
async fn a_missing_key_is_null() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "GET" => b"$-1\r\n".to_vec(),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let result = client.execute_query_structured("GET nope").await.unwrap();
    assert_eq!(result.rows, [vec![None]]);
    // the legacy shape renders NULL like the other engines
    let rows = client.execute_query("GET nope").await.unwrap();
    assert_eq!(rows, [vec!["value".to_string()], vec!["NULL".to_string()]]);
}

#[tokio::test]
async fn hgetall_is_field_value_rows() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "HGETALL" => array(&[bulk("name"), bulk("ada"), bulk("age"), bulk("36")]),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let result = client
        .execute_query_structured("HGETALL user:1")
        .await
        .unwrap();
    assert_eq!(result.columns, ["field", "value"]);
    assert_eq!(
        result.rows,
        [
            vec![Some("name".to_string()), Some("ada".to_string())],
            vec![Some("age".to_string()), Some("36".to_string())],
        ]
    );
}

#[tokio::test]
async fn integer_replies_are_text() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "DBSIZE" => integer(42),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let result = client.execute_query_structured("DBSIZE").await.unwrap();
    assert_eq!(result.rows, [vec![Some("42".to_string())]]);
}

#[tokio::test]
async fn quoted_and_binary_arguments_reach_the_server_intact() {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    client
        .execute_query(r#"SET "a b" "\xff\x00end" 'it\'s'"#)
        .await
        .unwrap();
    let raw = server.received_raw();
    let set = raw
        .iter()
        .find(|args| args[0] == b"SET")
        .expect("SET reached the server");
    assert_eq!(set[1], b"a b");
    assert_eq!(set[2], [0xff, 0x00, b'e', b'n', b'd']);
    assert_eq!(set[3], b"it's");
}

#[tokio::test]
async fn a_server_error_is_a_query_error() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "FOO" => error("ERR unknown command 'FOO', with args beginning with: "),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let error = client.execute_query("FOO").await.unwrap_err();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{error:?}");
    assert!(error.to_string().contains("unknown command"), "{error}");
}

#[tokio::test]
async fn a_malformed_line_never_reaches_the_server() {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    let before = server.received().len();
    let error = client.execute_query("GET \"open").await.unwrap_err();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{error:?}");
    assert_eq!(server.received().len(), before);
}

#[rstest]
#[case("SUBSCRIBE news")]
#[case("MONITOR")]
#[case("MULTI")]
#[case("SELECT 2")]
#[case("AUTH pw")]
#[case("CLIENT REPLY OFF")]
#[tokio::test]
async fn stateful_commands_are_not_supported_and_not_sent(#[case] line: &str) {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    let before = server.received().len();
    let error = client.execute_query(line).await.unwrap_err();
    assert!(
        matches!(
            error,
            DatabaseError::FeatureNotSupported {
                database_type: DatabaseType::Redis,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(server.received().len(), before, "{line} must not be sent");
}

#[tokio::test]
async fn a_read_only_connection_refuses_writes_before_sending_them() {
    let server = FakeRedis::start(quiet).await;
    let mut info = server.info();
    info.options.insert(OPT_HARDEN_READ_ONLY.into(), "1".into());
    let client = RedisClient::new(info).await.unwrap();
    let before = server.received().len();
    for line in ["SET k v", "FLUSHALL", "KEYS *", "EVAL \"return 1\" 0"] {
        let error = client.execute_query(line).await.unwrap_err();
        assert!(
            error.to_string().contains("read-only mode"),
            "{line}: {error}"
        );
    }
    assert_eq!(server.received().len(), before);
    client.execute_query("GET k").await.unwrap();
}

#[tokio::test]
async fn max_rows_caps_the_reply_and_says_so() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "LRANGE" => array(&(0..10).map(|n| bulk(&n.to_string())).collect::<Vec<_>>()),
        _ => ok(),
    })
    .await;
    let mut info = server.info();
    info.options.insert("max_rows".into(), "4".into());
    let client = RedisClient::new(info).await.unwrap();
    let limited = client.execute_limited("LRANGE l 0 -1").await.unwrap();
    assert_eq!(limited.result.rows.len(), 4);
    assert!(limited.truncated);
    let exact = client.execute_limited("LRANGE l 0 3").await.unwrap();
    assert_eq!(exact.result.rows.len(), 4);
    assert!(
        exact.truncated,
        "the fake answers 10 rows whatever the range says"
    );
}

#[tokio::test]
async fn connect_to_database_opens_a_connection_on_that_database() {
    let server = FakeRedis::start(quiet).await;
    let mut client = connect(&server).await;
    client.connect_to_database("db5").await.unwrap();
    assert_eq!(client.get_current_database(), "5");
    assert_eq!(client.get_connection_info().database.as_deref(), Some("5"));
    assert!(
        server
            .received()
            .contains(&vec!["SELECT".into(), "5".into()])
    );
    assert_eq!(
        client.get_metadata_provider().default_schema().as_deref(),
        Some("db5")
    );
    let error = client.connect_to_database("main").await.unwrap_err();
    assert!(matches!(error, DatabaseError::InvalidUrl(_)), "{error:?}");
}

#[tokio::test]
async fn list_databases_reads_the_keyspace() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "INFO" => bulk(
            "# Keyspace\r\ndb0:keys=3,expires=1,avg_ttl=0\r\ndb2:keys=9,expires=0,avg_ttl=0\r\n",
        ),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let rows = client.list_databases().await.unwrap();
    assert_eq!(
        rows,
        [
            vec!["Database".to_string(), "Keys".to_string()],
            vec!["0".to_string(), "3".to_string()],
            vec!["2".to_string(), "9".to_string()],
        ]
    );
    let schemas = client.get_metadata_provider().get_schemas().await.unwrap();
    assert_eq!(schemas, ["db0", "db2"]);
}

#[rstest]
#[case(
    "# Server\r\nredis_version:7.2.4\r\nredis_mode:standalone\r\nos:Linux\r\n",
    "Redis",
    "7.2.4"
)]
#[case(
    "# Server\r\nredis_version:7.2.4\r\nvalkey_version:8.0.1\r\n",
    "Valkey",
    "8.0.1"
)]
#[tokio::test]
async fn server_info_names_the_server(
    #[case] info_text: &'static str,
    #[case] server_type: &str,
    #[case] version: &str,
) {
    let server = FakeRedis::start(move |args| match args[0].as_str() {
        "INFO" => bulk(info_text),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let info = client.get_server_info().await.unwrap();
    assert_eq!(info.server_type, server_type);
    assert_eq!(info.server_version, version);
    assert!(!info.supports_transactions);
}

#[tokio::test]
async fn key_listing_is_a_bounded_scan_never_keys() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "SCAN" if args[1] == "0" => array(&[bulk("7"), array(&[bulk("user:1"), bulk("user:2")])]),
        "SCAN" => array(&[bulk("0"), array(&[bulk("user:2"), bulk("session:9")])]),
        "TYPE" => b"+hash\r\n".to_vec(),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let provider = client.get_metadata_provider();
    let tables = provider.get_tables(None).await.unwrap();
    // the duplicate `user:2` from the second page is listed once
    assert_eq!(tables, ["user:1", "user:2", "session:9"]);
    let received = server.received();
    assert!(
        received.iter().all(|args| args[0] != "KEYS"),
        "{received:?}"
    );
    assert!(
        received
            .iter()
            .any(|args| args[0] == "SCAN" && args[2] == "COUNT")
    );

    let kinds = provider.get_table_kinds(&tables).await.unwrap();
    assert_eq!(kinds.get("user:1").map(String::as_str), Some("hash"));
    assert_eq!(kinds.len(), 3);
}

#[tokio::test]
async fn key_listing_stops_at_the_scan_limit() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        // an endless keyspace: every page has fresh keys and a non-zero cursor
        "SCAN" => array(&[
            bulk(&(args[1].parse::<u64>().unwrap() + 1).to_string()),
            array(&[
                bulk(&format!("k{}a", args[1])),
                bulk(&format!("k{}b", args[1])),
            ]),
        ]),
        _ => ok(),
    })
    .await;
    let mut info = server.info();
    info.options.insert("scan_limit".into(), "5".into());
    let client = RedisClient::new(info).await.unwrap();
    let keys = client
        .get_metadata_provider()
        .get_tables(Some("db0"))
        .await
        .unwrap();
    assert_eq!(keys, ["k0a", "k0b", "k1a", "k1b", "k2a"]);
}

#[tokio::test]
async fn another_database_is_not_listed_from_this_connection() {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    let error = client
        .get_metadata_provider()
        .get_tables(Some("db9"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, DatabaseError::MetadataError(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("\\c"), "{error}");
}

#[tokio::test]
async fn describing_a_hash_gives_properties_and_a_member_sample() {
    let server =
        FakeRedis::start(
            |args| match (args[0].as_str(), args.get(1).map(String::as_str)) {
                ("TYPE", _) => b"+hash\r\n".to_vec(),
                ("PTTL", _) => integer(-1),
                ("MEMORY", Some("USAGE")) => integer(312),
                ("HLEN", _) => integer(2),
                ("OBJECT", Some("ENCODING")) => bulk("listpack"),
                ("HSCAN", _) => array(&[
                    bulk("0"),
                    array(&[bulk("name"), bulk("ada"), bulk("age"), bulk("36")]),
                ]),
                _ => ok(),
            },
        )
        .await;
    let client = connect(&server).await;
    let details = client
        .get_metadata_provider()
        .get_table_details("user:1", None)
        .await
        .unwrap();
    assert_eq!(details.name, "user:1");
    assert_eq!(details.schema, "db0");
    let rows: Vec<(String, String, Option<String>)> = details
        .columns
        .iter()
        .map(|c| (c.name.clone(), c.data_type.clone(), c.default_value.clone()))
        .collect();
    assert_eq!(
        rows,
        [
            ("type".into(), "hash".into(), None),
            ("ttl".into(), "no expiry".into(), None),
            ("memory".into(), "312 bytes".into(), None),
            ("length".into(), "2 fields".into(), None),
            ("encoding".into(), "listpack".into(), None),
            ("name".into(), "string".into(), Some("ada".into())),
            ("age".into(), "string".into(), Some("36".into())),
        ]
    );
    let columns = client
        .get_metadata_provider()
        .get_columns("user:1", None)
        .await
        .unwrap();
    assert_eq!(columns, ["name", "age"]);
}

#[tokio::test]
async fn describing_a_missing_key_is_a_metadata_error() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "TYPE" => b"+none\r\n".to_vec(),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let error = client
        .get_metadata_provider()
        .get_table_details("ghost", None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, DatabaseError::MetadataError(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("does not exist"), "{error}");
}

#[tokio::test]
async fn completion_commands_fall_back_when_command_list_is_blocked() {
    let server = FakeRedis::start(|args| match args[0].as_str() {
        "COMMAND" => error("NOPERM this user has no permissions to run the 'command' command"),
        _ => ok(),
    })
    .await;
    let client = connect(&server).await;
    let commands = client
        .get_metadata_provider()
        .get_functions(None)
        .await
        .unwrap();
    assert!(commands.iter().any(|name| name == "HGETALL"));
    assert!(commands.iter().any(|name| name == "SCAN"));
}

#[tokio::test]
async fn is_connected_pings() {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    assert!(client.is_connected().await);
    assert!(server.received().iter().any(|args| args[0] == "PING"));
}

#[tokio::test]
async fn explain_is_not_supported() {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    let error = client.explain_query("GET k").await.unwrap_err();
    assert!(
        matches!(error, DatabaseError::FeatureNotSupported { .. }),
        "{error:?}"
    );
    assert!(!client.get_metadata_provider().supports_explain());
}

#[tokio::test]
async fn test_query_parses_and_only_runs_reads() {
    let server = FakeRedis::start(quiet).await;
    let client = connect(&server).await;
    let before = server.received().len();
    client.test_query("SET k v").await.unwrap(); // parsed, never sent
    assert_eq!(server.received().len(), before);
    client.test_query("GET k").await.unwrap();
    assert_eq!(server.received().len(), before + 1);
    assert!(client.test_query("GET \"open").await.is_err());
}

#[rstest]
#[case(
    "# Server\r\nredis_version:7.2.4\r\n\r\n# Clients\r\nconnected_clients:1\r\n",
    "connected_clients",
    "1"
)]
#[case("redis_version:7.2.4\r\n", "redis_version", "7.2.4")]
fn parse_info_reads_key_value_lines(#[case] text: &str, #[case] key: &str, #[case] expected: &str) {
    assert_eq!(
        parse_info(text).get(key).map(String::as_str),
        Some(expected)
    );
}

#[test]
fn options_fall_back_to_the_defaults_when_invalid() {
    let mut info = ConnectionInfo::default();
    assert_eq!(usize_option(&info, OPT_MAX_ROWS, 7), 7);
    info.options.insert(OPT_MAX_ROWS.into(), "0".into());
    assert_eq!(usize_option(&info, OPT_MAX_ROWS, 7), 7);
    info.options.insert(OPT_MAX_ROWS.into(), "12".into());
    assert_eq!(usize_option(&info, OPT_MAX_ROWS, 7), 12);
}

// -- Opt-in integration tests against a real server ---------------------------
//
// `REDIS_URL=redis://localhost:6379/15 cargo test -p dbcrust-driver-redis`
// (`docker run --rm -d -p 6379:6379 redis:7`). They write under the
// `dbcrust:it:` prefix of whatever database the URL selects and remove it.

async fn real_client() -> Option<RedisClient> {
    let url = std::env::var("REDIS_URL").ok()?;
    let info = ConnectionInfo::parse_url(&url).expect("REDIS_URL parses");
    Some(RedisClient::new(info).await.expect("REDIS_URL connects"))
}

#[tokio::test]
async fn integration_round_trip() {
    let Some(client) = real_client().await else {
        eprintln!("skipping: REDIS_URL is not set");
        return;
    };
    client
        .execute_query("DEL dbcrust:it:str dbcrust:it:hash dbcrust:it:list dbcrust:it:zset")
        .await
        .unwrap();
    client
        .execute_query(r#"SET dbcrust:it:str "hello world""#)
        .await
        .unwrap();
    let got = client
        .execute_query_structured("GET dbcrust:it:str")
        .await
        .unwrap();
    assert_eq!(got.rows, [vec![Some("hello world".to_string())]]);

    client
        .execute_query("HSET dbcrust:it:hash name ada age 36")
        .await
        .unwrap();
    let hash = client
        .execute_query_structured("HGETALL dbcrust:it:hash")
        .await
        .unwrap();
    assert_eq!(hash.columns, ["field", "value"]);
    assert_eq!(hash.rows.len(), 2);

    client
        .execute_query("RPUSH dbcrust:it:list a b c")
        .await
        .unwrap();
    let list = client
        .execute_query_structured("LRANGE dbcrust:it:list 0 -1")
        .await
        .unwrap();
    assert_eq!(list.columns, ["index", "value"]);
    assert_eq!(list.rows.len(), 3);

    client
        .execute_query("ZADD dbcrust:it:zset 1 one 2 two")
        .await
        .unwrap();
    let zset = client
        .execute_query_structured("ZRANGE dbcrust:it:zset 0 -1 WITHSCORES")
        .await
        .unwrap();
    assert_eq!(zset.columns, ["member", "score"]);
    assert_eq!(zset.rows.len(), 2);

    let scan = client
        .execute_query_structured("SCAN 0 MATCH dbcrust:it:* COUNT 100")
        .await
        .unwrap();
    assert_eq!(scan.columns, ["cursor", "value"]);

    let keys = client
        .get_metadata_provider()
        .get_tables(None)
        .await
        .unwrap();
    assert!(keys.iter().any(|key| key == "dbcrust:it:hash"));
    let details = client
        .get_metadata_provider()
        .get_table_details("dbcrust:it:hash", None)
        .await
        .unwrap();
    assert_eq!(details.columns[0].data_type, "hash");

    let info = client.get_server_info().await.unwrap();
    assert!(info.server_type == "Redis" || info.server_type == "Valkey");
    assert!(client.is_connected().await);

    assert!(
        client.execute_query("KEYS *").await.is_ok(),
        "KEYS is allowed without --read-only"
    );
    client
        .execute_query("DEL dbcrust:it:str dbcrust:it:hash dbcrust:it:list dbcrust:it:zset")
        .await
        .unwrap();
}
