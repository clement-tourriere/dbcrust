//! Client tests against an in-process fake Hrana server (HTTP/1.1 over a TCP
//! listener, canned JSON pipelines), plus an opt-in integration test against a
//! real `sqld` / Turso database (`LIBSQL_URL`, skipped when unset).

use super::*;
use dbcrust_core::database::{DatabaseType, set_thread_interrupt_flag};
use dbcrust_core::options::{OPT_HARDEN_READ_ONLY, OPT_HARDEN_TIMEOUT_SECS};
use rstest::rstest;
use serde_json::{Value as Json, json};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A JWT-shaped token (`{"alg":"EdDSA"}.{"a":"ro","id":"t1"}.signature`), built from
/// pieces so the file holds no JWT literal for a secret scanner to flag.
const TOKEN: &str = concat!(
    "eyJhbGciOiJFZERTQSJ9",
    ".",
    "eyJhIjoicm8iLCJpZCI6InQxIn0",
    ".",
    "c2lnbmF0dXJl-SECRET"
);

// ----- fake server -----------------------------------------------------------------

/// One request the fake received.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    authorization: Option<String>,
    namespace: Option<String>,
    body: Json,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay: std::time::Duration,
}

impl Reply {
    fn json(body: Json) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.to_string(),
            delay: std::time::Duration::ZERO,
        }
    }
    fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
            delay: std::time::Duration::ZERO,
        }
    }
}

type Handler = dyn Fn(&Seen) -> Reply + Send + Sync;

struct FakeHrana {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeHrana {
    async fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let handler: Arc<Handler> = Arc::new(handler);
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let (handler, log) = (handler.clone(), log.clone());
                tokio::spawn(async move {
                    let mut buffer: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        // A full request: head, then Content-Length bytes
                        let (head, body) = loop {
                            if let Some(end) = find(&buffer, b"\r\n\r\n") {
                                let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
                                let length = head
                                    .lines()
                                    .find_map(|line| {
                                        let (name, value) = line.split_once(':')?;
                                        name.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse::<usize>().ok())
                                            .flatten()
                                    })
                                    .unwrap_or(0);
                                if buffer.len() >= end + 4 + length {
                                    let body = buffer[end + 4..end + 4 + length].to_vec();
                                    buffer.drain(..end + 4 + length);
                                    break (head, body);
                                }
                            }
                            match socket.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                            }
                        };
                        let header = |wanted: &str| {
                            head.lines().find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case(wanted)
                                    .then(|| value.trim().to_string())
                            })
                        };
                        let request = Seen {
                            path: head
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or_default()
                                .to_string(),
                            authorization: header("authorization"),
                            namespace: header("x-namespace"),
                            body: serde_json::from_slice(&body).unwrap_or(Json::Null),
                        };
                        log.lock().unwrap().push(request.clone());
                        let reply = handler(&request);
                        tokio::time::sleep(reply.delay).await;
                        let mut out = format!(
                            "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n",
                            reply.status,
                            reply.body.len()
                        );
                        for (name, value) in &reply.headers {
                            out.push_str(&format!("{name}: {value}\r\n"));
                        }
                        out.push_str("\r\n");
                        out.push_str(&reply.body);
                        if socket.write_all(out.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { addr, seen }
    }

    fn info(&self) -> ConnectionInfo {
        ConnectionInfo::parse_url(&format!("libsql+http://127.0.0.1:{}", self.addr.port())).unwrap()
    }

    fn info_with_token(&self) -> ConnectionInfo {
        let mut info = self.info();
        info.password = Some(TOKEN.to_string());
        info
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The SQL texts of the `execute` requests, in order.
    fn executed(&self) -> Vec<String> {
        self.requests()
            .iter()
            .flat_map(|seen| statements(&seen.body))
            .collect()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `execute` / `describe` SQL texts of a pipeline body.
fn statements(body: &Json) -> Vec<String> {
    body["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|request| match request["type"].as_str()? {
            "execute" => request["stmt"]["sql"].as_str().map(str::to_string),
            "describe" => request["sql"]
                .as_str()
                .map(|sql| format!("describe: {sql}")),
            _ => None,
        })
        .collect()
}

fn request_types(body: &Json) -> Vec<String> {
    body["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|request| request["type"].as_str().map(str::to_string))
        .collect()
}

// ----- canned Hrana answers --------------------------------------------------------

fn int(value: i64) -> Json {
    json!({"type": "integer", "value": value.to_string()})
}
fn text(value: &str) -> Json {
    json!({"type": "text", "value": value})
}
fn null() -> Json {
    json!({"type": "null"})
}

fn executed(names: &[&str], rows: Vec<Vec<Json>>) -> Json {
    json!({"type": "ok", "response": {"type": "execute", "result": {
        "cols": names.iter().map(|name| json!({"name": name, "decltype": null})).collect::<Vec<_>>(),
        "rows": rows,
        "affected_row_count": 0,
        "last_insert_rowid": null,
        "rows_read": 3,
        "rows_written": 0,
        "query_duration_ms": 0.5
    }}})
}

fn closed() -> Json {
    json!({"type": "ok", "response": {"type": "close"}})
}

fn failure(message: &str, code: &str) -> Json {
    json!({"type": "error", "error": {"message": message, "code": code}})
}

fn answer(baton: Option<&str>, results: Vec<Json>) -> Reply {
    Reply::json(json!({"baton": baton, "base_url": null, "results": results}))
}

/// `SELECT 1` ok, everything else through `inner` (which gets the SQL).
fn server(inner: impl Fn(&str) -> Json + Send + Sync + 'static) -> impl Fn(&Seen) -> Reply {
    move |seen| {
        let mut results = Vec::new();
        for request in seen.body["requests"].as_array().into_iter().flatten() {
            match request["type"].as_str() {
                Some("execute") => {
                    let sql = request["stmt"]["sql"].as_str().unwrap_or_default();
                    results.push(if sql == "SELECT 1" {
                        executed(&["1"], vec![vec![int(1)]])
                    } else {
                        inner(sql)
                    });
                }
                Some("describe") => results.push(inner(&format!(
                    "describe: {}",
                    request["sql"].as_str().unwrap_or_default()
                ))),
                _ => results.push(closed()),
            }
        }
        answer(None, results)
    }
}

// ----- tests -------------------------------------------------------------------------

#[tokio::test]
async fn a_query_is_one_stateless_pipeline_with_the_bearer_token() {
    let fake = FakeHrana::start(server(|sql| {
        assert_eq!(sql, "SELECT id, name FROM users");
        executed(
            &["id", "name"],
            vec![vec![int(1), text("ada")], vec![int(2), null()]],
        )
    }))
    .await;
    let client = LibSqlClient::new(fake.info_with_token()).await.unwrap();

    let result = client
        .execute_query_structured("SELECT id, name FROM users")
        .await
        .unwrap();
    assert_eq!(result.columns, ["id", "name"]);
    assert_eq!(
        result.rows,
        vec![
            vec![Some("1".to_string()), Some("ada".to_string())],
            vec![Some("2".to_string()), None],
        ]
    );

    let requests = fake.requests();
    // connect probe, then the query
    assert_eq!(statements(&requests[0].body), ["SELECT 1"]);
    let query = &requests[1];
    assert_eq!(query.path, "/v2/pipeline");
    assert_eq!(
        query.authorization.as_deref(),
        Some(&*format!("Bearer {TOKEN}"))
    );
    assert_eq!(query.namespace, None);
    assert_eq!(
        query.body,
        json!({
            "baton": null,
            "requests": [
                {"type": "execute", "stmt": {"sql": "SELECT id, name FROM users", "want_rows": true}},
                {"type": "close"}
            ]
        })
    );
}

#[tokio::test]
async fn every_hrana_value_type_decodes_to_the_cell_text() {
    let fake = FakeHrana::start(server(|_| {
        executed(
            &["i", "f", "whole", "t", "b", "padded", "n", "empty", "big"],
            vec![vec![
                int(-42),
                json!({"type": "float", "value": 2.5}),
                json!({"type": "float", "value": 3.0}),
                text("héllo \"quoted\" \n second line"),
                json!({"type": "blob", "base64": "3q2+7w"}),
                json!({"type": "blob", "base64": "3q2+7w=="}),
                null(),
                text(""),
                int(i64::MAX),
            ]],
        )
    }))
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    let result = client.execute_query_structured("SELECT *").await.unwrap();
    let row: Vec<Option<&str>> = result.rows[0].iter().map(|c| c.as_deref()).collect();
    assert_eq!(
        row,
        [
            Some("-42"),
            Some("2.5"),
            Some("3.0"),
            Some("héllo \"quoted\" \n second line"),
            Some("\\xdeadbeef"),
            Some("\\xdeadbeef"),
            None,
            Some(""),
            Some("9223372036854775807"),
        ]
    );
    // the legacy shape renders NULL as ""
    let legacy = client.execute_query("SELECT *").await.unwrap();
    assert_eq!(legacy[1][6], "");
}

#[tokio::test]
async fn a_statement_without_rows_is_the_empty_result() {
    let fake = FakeHrana::start(server(|_| {
        json!({"type": "ok", "response": {"type": "execute", "result": {
            "cols": [], "rows": [], "affected_row_count": 3, "last_insert_rowid": "9"
        }}})
    }))
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    assert!(
        client
            .execute_query("UPDATE t SET a = 1")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn statement_errors_carry_the_server_message_and_code() {
    let fake = FakeHrana::start(server(|_| {
        failure("SQLite error: no such table: nope", "SQLITE_UNKNOWN")
    }))
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    let error = client
        .execute_query("SELECT * FROM nope")
        .await
        .unwrap_err();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains("no such table: nope"), "{text}");
    assert!(text.contains("SQLITE_UNKNOWN"), "{text}");
}

#[rstest]
#[case(401, "{\"error\":\"The JWT is invalid\"}", "authentication failed")]
#[case(403, "forbidden", "authentication failed")]
fn bad_credentials_say_so_and_never_the_token(
    #[case] status: u16,
    #[case] body: &'static str,
    #[case] expected: &str,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // The server even echoes the credential it was sent
        let fake = FakeHrana::start(move |seen| {
            let echoed = format!(
                "{body} (got {})",
                seen.authorization.clone().unwrap_or_default()
            );
            Reply::status(status, &echoed)
        })
        .await;
        let error = match LibSqlClient::new(fake.info_with_token()).await {
            Ok(_) => panic!("the server refuses the token"),
            Err(error) => error,
        };
        assert!(
            matches!(error, DatabaseError::ConnectionError(_)),
            "{error}"
        );
        let text = error.to_string();
        assert!(text.to_lowercase().contains(expected), "{text}");
        assert!(text.contains(&status.to_string()), "{text}");
        assert!(!text.contains("SECRET") && !text.contains(TOKEN), "{text}");
    });
}

#[rstest]
#[case(404, "", None, "does not serve /v2/pipeline")]
#[case(429, "slow down", Some("7"), "rate limiting")]
#[case(500, "{\"error\":\"disk I/O error\"}", None, "disk I/O error")]
#[case(502, "bad gateway", None, "HTTP 502")]
fn http_failures_are_mapped(
    #[case] status: u16,
    #[case] body: &'static str,
    #[case] retry_after: Option<&'static str>,
    #[case] expected: &str,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // Healthy for the connect probe, failing afterwards
        let fake = FakeHrana::start(move |seen| {
            if statements(&seen.body) == ["SELECT 1"] {
                return answer(None, vec![executed(&["1"], vec![vec![int(1)]]), closed()]);
            }
            let mut reply = Reply::status(status, body);
            if let Some(after) = retry_after {
                reply.headers.push(("retry-after", after.to_string()));
            }
            reply
        })
        .await;
        let client = LibSqlClient::new(fake.info_with_token()).await.unwrap();
        let error = client.execute_query("SELECT 2").await.unwrap_err();
        let text = error.to_string();
        assert!(text.contains(expected), "{text}");
        if let Some(after) = retry_after {
            assert!(text.contains(after), "{text}");
        }
        assert!(!text.contains(TOKEN), "{text}");
    });
}

#[tokio::test]
async fn something_that_is_not_hrana_is_reported_as_such() {
    let fake = FakeHrana::start(|_| Reply::json(json!({"hello": "world"}))).await;
    let error = match LibSqlClient::new(fake.info()).await {
        Ok(_) => panic!("not a Hrana server"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("not a Hrana response"),
        "{error}"
    );
}

#[tokio::test]
async fn a_namespace_travels_in_the_x_namespace_header() {
    let fake = FakeHrana::start(server(|_| executed(&["x"], vec![vec![int(1)]]))).await;
    let mut info = fake.info();
    info.database = Some("tenant1".to_string());
    let client = LibSqlClient::new(info).await.unwrap();
    assert_eq!(client.get_current_database(), "tenant1");
    client.execute_query("SELECT 5").await.unwrap();
    assert!(
        fake.requests()
            .iter()
            .all(|seen| seen.namespace.as_deref() == Some("tenant1"))
    );
}

#[tokio::test]
async fn a_transaction_carries_its_baton_until_it_ends() {
    // The fake mints batons: a pipeline without `close` keeps the stream
    let counter = Arc::new(Mutex::new(0u32));
    let fake = FakeHrana::start({
        let counter = counter.clone();
        move |seen| {
            let types = request_types(&seen.body);
            let mut results = Vec::new();
            for request in seen.body["requests"].as_array().into_iter().flatten() {
                match request["type"].as_str() {
                    Some("execute") => results.push(executed(&["n"], vec![vec![int(1)]])),
                    _ => results.push(closed()),
                }
            }
            let baton = if types.contains(&"close".to_string()) {
                None
            } else {
                let mut n = counter.lock().unwrap();
                *n += 1;
                Some(format!("baton-{n}"))
            };
            answer(baton.as_deref(), results)
        }
    })
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();

    client.execute_query("SELECT 'before'").await.unwrap();
    client
        .execute_query("  /* open */ begin immediate transaction")
        .await
        .unwrap();
    client
        .execute_query("INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    client.execute_query("SAVEPOINT s").await.unwrap();
    client.execute_query("ROLLBACK TO s").await.unwrap(); // does not end it
    client.execute_query("COMMIT").await.unwrap();
    client.execute_query("SELECT 'after'").await.unwrap();

    let calls: Vec<(String, Option<String>, Vec<String>)> = fake
        .requests()
        .iter()
        .skip(1) // the connect probe
        .map(|seen| {
            (
                statements(&seen.body).join(";"),
                seen.body["baton"].as_str().map(str::to_string),
                request_types(&seen.body),
            )
        })
        .collect();
    let exec = || vec!["execute".to_string()];
    let exec_close = || vec!["execute".to_string(), "close".to_string()];
    assert_eq!(
        calls,
        [
            ("SELECT 'before'".into(), None, exec_close()),
            (
                "  /* open */ begin immediate transaction".into(),
                None,
                exec()
            ),
            (
                "INSERT INTO t VALUES (1)".into(),
                Some("baton-1".into()),
                exec()
            ),
            ("SAVEPOINT s".into(), Some("baton-2".into()), exec()),
            ("ROLLBACK TO s".into(), Some("baton-3".into()), exec()),
            ("COMMIT".into(), Some("baton-4".into()), exec_close()),
            ("SELECT 'after'".into(), None, exec_close()),
        ]
    );
}

#[tokio::test]
async fn an_expired_stream_ends_the_transaction_with_an_explanation() {
    let fake = FakeHrana::start(|seen| {
        let types = request_types(&seen.body);
        if seen.body["baton"].is_string() {
            return Reply::status(400, "{\"message\":\"The stream has expired due to inactivity\",\"code\":\"STREAM_EXPIRED\"}");
        }
        let results = types
            .iter()
            .map(|t| if t == "execute" { executed(&["n"], vec![vec![int(1)]]) } else { closed() })
            .collect();
        answer(if types.contains(&"close".to_string()) { None } else { Some("b1") }, results)
    })
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    client.execute_query("BEGIN").await.unwrap();
    let error = client
        .execute_query("INSERT INTO t VALUES (1)")
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("stream has expired"), "{text}");
    assert!(text.contains("transaction was lost"), "{text}");
    // and the next statement starts fresh (no baton)
    client.execute_query("SELECT 1 + 1").await.unwrap();
    assert!(fake.requests().last().unwrap().body["baton"].is_null());
}

#[tokio::test]
async fn a_failed_begin_does_not_leave_a_stream_open() {
    let fake = FakeHrana::start(|seen| {
        let mut results = Vec::new();
        for request in seen.body["requests"].as_array().into_iter().flatten() {
            results.push(match request["type"].as_str() {
                Some("execute") if request["stmt"]["sql"] == "BEGIN" => failure(
                    "cannot start a transaction within a transaction",
                    "SQLITE_ERROR",
                ),
                Some("execute") => executed(&["n"], vec![vec![int(1)]]),
                _ => closed(),
            });
        }
        let has_close = request_types(&seen.body).contains(&"close".to_string());
        answer(if has_close { None } else { Some("b1") }, results)
    })
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    assert!(client.execute_query("BEGIN").await.is_err());
    // the stream the failed BEGIN opened was closed, and nothing is carried over
    let requests = fake.requests();
    let closing = &requests[requests.len() - 1];
    assert_eq!(request_types(&closing.body), ["close"]);
    assert_eq!(closing.body["baton"], "b1");
    client.execute_query("SELECT 2").await.unwrap();
    assert!(fake.requests().last().unwrap().body["baton"].is_null());
}

#[tokio::test]
async fn the_server_may_pin_a_stream_to_another_node_on_the_same_scheme() {
    let other = FakeHrana::start(server(|_| {
        executed(&["where"], vec![vec![text("other node")]])
    }))
    .await;
    let other_url = format!("http://127.0.0.1:{}", other.addr.port());
    let main = FakeHrana::start({
        let other_url = other_url.clone();
        move |seen| {
            let types = request_types(&seen.body);
            let results = types
                .iter()
                .map(|t| {
                    if t == "execute" {
                        executed(&["where"], vec![vec![text("main")]])
                    } else {
                        closed()
                    }
                })
                .collect();
            let mut reply = answer(
                if types.contains(&"close".to_string()) {
                    None
                } else {
                    Some("b1")
                },
                results,
            );
            // a stream-pinning answer for BEGIN only
            if !types.contains(&"close".to_string()) {
                reply.body = reply.body.replace(
                    "\"base_url\":null",
                    &format!("\"base_url\":\"{other_url}\""),
                );
            }
            reply
        }
    })
    .await;
    let client = LibSqlClient::new(main.info()).await.unwrap();
    client.execute_query("BEGIN").await.unwrap();
    let result = client.execute_query("SELECT where_am_i").await.unwrap();
    assert_eq!(result[1][0], "other node");
    // the stream-pinned statement did not hit the first server again
    assert_eq!(main.requests().len(), 2, "probe + BEGIN only");
    client.execute_query("COMMIT").await.unwrap();
}

#[tokio::test]
async fn a_base_url_that_changes_the_scheme_is_ignored() {
    let main = FakeHrana::start(|seen| {
        let types = request_types(&seen.body);
        let results = types
            .iter()
            .map(|t| {
                if t == "execute" {
                    executed(&["a"], vec![vec![int(1)]])
                } else {
                    closed()
                }
            })
            .collect();
        let mut reply = answer(
            if types.contains(&"close".to_string()) {
                None
            } else {
                Some("b1")
            },
            results,
        );
        // a stream-pinning answer to a host that would get the token over TLS
        reply.body = reply
            .body
            .replace("\"base_url\":null", "\"base_url\":\"https://127.0.0.1:1\"");
        reply
    })
    .await;
    let client = LibSqlClient::new(main.info_with_token()).await.unwrap();
    client.execute_query("BEGIN").await.unwrap();
    client.execute_query("SELECT 1 + 1").await.unwrap();
    client.execute_query("COMMIT").await.unwrap();
    // every request stayed on the configured plain endpoint
    assert_eq!(main.requests().len(), 4);
}

#[tokio::test]
async fn endpoint_urls_follow_the_transport_and_default_ports() {
    let endpoint =
        Endpoint::from_info(&ConnectionInfo::parse_url("libsql://db.example").unwrap()).unwrap();
    assert!(endpoint.is_tls());
    assert_eq!(endpoint.base_url, "https://db.example:443");
    let plain = Endpoint::from_info(&ConnectionInfo::parse_url("libsql+http://localhost").unwrap())
        .unwrap();
    assert!(!plain.is_tls());
    assert_eq!(plain.base_url, "http://localhost:8080");
    let ipv6 = Endpoint::from_info(&ConnectionInfo::parse_url("libsql+http://[::1]:9000").unwrap())
        .unwrap();
    assert_eq!(ipv6.base_url, "http://[::1]:9000");
}

#[tokio::test]
async fn read_only_asks_the_server_before_running_anything() {
    let fake = FakeHrana::start(server(|sql| match sql {
        "describe: SELECT * FROM t" => {
            json!({"type": "ok", "response": {"type": "describe", "result": {
            "params": [], "cols": [], "is_explain": false, "is_readonly": true}}})
        }
        "describe: INSERT INTO t VALUES (1)" => {
            json!({"type": "ok", "response": {"type": "describe", "result": {
            "params": [], "cols": [], "is_explain": false, "is_readonly": false}}})
        }
        "describe: WITH x AS (SELECT 1) SELECT 1" => {
            json!({"type": "ok", "response": {"type": "describe", "result": {
            "params": [], "cols": [], "is_explain": false}}})
        }
        "describe: SELEC oops" => {
            failure("SQLite error: near \"SELEC\": syntax error", "SQLITE_ERROR")
        }
        _ => executed(&["a"], vec![vec![int(1)]]),
    }))
    .await;
    let mut info = fake.info();
    info.options
        .insert(OPT_HARDEN_READ_ONLY.to_string(), "1".to_string());
    let client = LibSqlClient::new(info).await.unwrap();

    assert!(client.execute_query("SELECT * FROM t").await.is_ok());

    let refused = client
        .execute_query("INSERT INTO t VALUES (1)")
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains(READ_ONLY_VIOLATION_MARKER),
        "{refused}"
    );
    let unknown = client
        .execute_query("WITH x AS (SELECT 1) SELECT 1")
        .await
        .unwrap_err();
    assert!(unknown.to_string().contains("did not say"), "{unknown}");
    let syntax = client.execute_query("SELEC oops").await.unwrap_err();
    assert!(syntax.to_string().contains("syntax error"), "{syntax}");

    // refused statements were never executed
    let executed = fake.executed();
    assert!(executed.contains(&"SELECT * FROM t".to_string()));
    assert!(!executed.contains(&"INSERT INTO t VALUES (1)".to_string()));
    assert!(!executed.contains(&"WITH x AS (SELECT 1) SELECT 1".to_string()));
}

#[tokio::test]
async fn without_read_only_nothing_is_described() {
    let fake = FakeHrana::start(server(|_| executed(&["a"], vec![vec![int(1)]]))).await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    client
        .execute_query("INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    assert!(
        !fake
            .executed()
            .iter()
            .any(|sql| sql.starts_with("describe:"))
    );
}

#[tokio::test]
async fn interrupt_drops_the_request() {
    let fake = FakeHrana::start(|seen| {
        let slow = statements(&seen.body) == ["SELECT slow"];
        let types = request_types(&seen.body);
        let results = types
            .iter()
            .map(|t| {
                if t == "execute" {
                    executed(&["a"], vec![vec![int(1)]])
                } else {
                    closed()
                }
            })
            .collect();
        let mut reply = answer(None, results);
        if slow {
            reply.delay = std::time::Duration::from_secs(10);
        }
        reply
    })
    .await;
    let flag = Arc::new(AtomicBool::new(false));
    set_thread_interrupt_flag(Some(flag.clone()));
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    let trigger = flag.clone();
    let interrupt = async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        trigger.store(true, Ordering::Relaxed);
    };
    let started = std::time::Instant::now();
    let (result, ()) = tokio::join!(client.execute_query("SELECT slow"), interrupt);
    set_thread_interrupt_flag(None);
    assert!(result.unwrap_err().to_string().contains("Query cancelled"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn the_broker_timeout_ends_a_slow_request() {
    let fake = FakeHrana::start(|seen| {
        let slow = statements(&seen.body) == ["SELECT slow"];
        let results = request_types(&seen.body)
            .iter()
            .map(|t| {
                if t == "execute" {
                    executed(&["a"], vec![vec![int(1)]])
                } else {
                    closed()
                }
            })
            .collect();
        let mut reply = answer(None, results);
        if slow {
            reply.delay = std::time::Duration::from_secs(10);
        }
        reply
    })
    .await;
    let mut info = fake.info();
    info.options
        .insert(OPT_HARDEN_TIMEOUT_SECS.to_string(), "1".to_string());
    let client = LibSqlClient::new(info).await.unwrap();
    let error = client.execute_query("SELECT slow").await.unwrap_err();
    assert!(
        error.to_string().contains("timed out after 1 seconds"),
        "{error}"
    );
}

#[tokio::test]
async fn a_refused_connection_is_a_connection_error_without_the_token() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut info = ConnectionInfo::parse_url(&format!("libsql+http://127.0.0.1:{port}")).unwrap();
    info.password = Some(TOKEN.to_string());
    let error = match LibSqlClient::new(info).await {
        Ok(_) => panic!("nothing is listening"),
        Err(error) => error,
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error}"
    );
    assert!(!error.to_string().contains("SECRET"), "{error}");
}

#[tokio::test]
async fn metadata_reads_the_sqlite_catalog_in_few_round_trips() {
    let fake = FakeHrana::start(server(|sql| {
        if sql.contains(
            "FROM \"main\".sqlite_schema WHERE type IN ('table', 'view') AND name = 'orders'",
        ) {
            executed(&["name"], vec![vec![text("orders")]])
        } else if sql.starts_with("SELECT name, type, \"notnull\"") {
            executed(
                &["name", "type", "notnull", "dflt_value", "pk"],
                vec![
                    vec![text("id"), text("INTEGER"), int(0), null(), int(1)],
                    vec![
                        text("customer"),
                        text("TEXT"),
                        int(1),
                        text("'n/a'"),
                        int(0),
                    ],
                ],
            )
        } else if sql.starts_with("SELECT name, \"unique\", origin") {
            executed(
                &["name", "unique", "origin"],
                vec![
                    vec![text("orders_customer"), int(0), text("c")],
                    vec![text("sqlite_autoindex_orders_1"), int(1), text("u")],
                ],
            )
        } else if sql.contains("pragma_index_info('orders_customer'") {
            executed(&["name"], vec![vec![text("customer")]])
        } else if sql.contains("pragma_index_info('sqlite_autoindex_orders_1'") {
            executed(&["name"], vec![vec![text("customer")], vec![text("id")]])
        } else if sql.starts_with("SELECT \"from\", \"table\", \"to\"") {
            executed(
                &["from", "table", "to"],
                vec![vec![text("customer"), text("customers"), text("id")]],
            )
        } else if sql.starts_with("SELECT m.name") {
            executed(
                &["name", "from", "to"],
                vec![vec![text("lines"), text("order_id"), text("id")]],
            )
        } else if sql.contains("pragma_database_list") {
            executed(&["name"], vec![vec![text("main")], vec![text("aux")]])
        } else if sql.contains("sqlite_schema WHERE type IN ('table', 'view') AND substr") {
            executed(
                &["name"],
                vec![vec![text("customers")], vec![text("orders")]],
            )
        } else if sql.starts_with("SELECT name FROM pragma_table_info") {
            executed(&["name"], vec![vec![text("id")], vec![text("customer")]])
        } else {
            failure(&format!("unexpected statement {sql}"), "SQLITE_ERROR")
        }
    }))
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    let provider = client.get_metadata_provider();

    assert_eq!(provider.get_schemas().await.unwrap(), ["main", "aux"]);
    assert_eq!(
        provider.get_tables(None).await.unwrap(),
        ["customers", "orders"]
    );
    assert_eq!(
        provider.get_columns("orders", None).await.unwrap(),
        ["id", "customer"]
    );
    assert_eq!(provider.default_schema().as_deref(), Some("main"));

    let before = fake.requests().len();
    let details = provider.get_table_details("orders", None).await.unwrap();
    assert_eq!(
        fake.requests().len() - before,
        2,
        "the table, its columns, indexes and keys in one pipeline, the index columns in a second"
    );
    let columns: Vec<_> = details
        .columns
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.data_type.as_str(),
                c.nullable,
                c.default_value.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        columns,
        [
            ("id", "INTEGER", false, None), // INTEGER PRIMARY KEY is never NULL
            ("customer", "TEXT", false, Some("'n/a'")),
        ]
    );
    let indexes: Vec<_> = details
        .indexes
        .iter()
        .map(|i| {
            (
                i.name.as_str(),
                i.definition.as_str(),
                i.is_unique,
                i.is_primary,
            )
        })
        .collect();
    assert_eq!(
        indexes,
        [
            ("orders_pkey", "(id)", true, true), // the rowid alias has no index of its own
            ("orders_customer", "(customer)", false, false),
            ("sqlite_autoindex_orders_1", "(customer, id)", true, false),
        ]
    );
    assert_eq!(
        details.foreign_keys[0].definition,
        "FOREIGN KEY (customer) REFERENCES customers(id)"
    );
    assert_eq!(details.referenced_by[0].table, "lines");

    let missing = FakeHrana::start(server(|_| executed(&["name"], vec![]))).await;
    let client = LibSqlClient::new(missing.info()).await.unwrap();
    let error = client
        .get_metadata_provider()
        .get_table_details("ghost", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("'ghost' does not exist"),
        "{error}"
    );
}

#[tokio::test]
async fn catalog_literals_escape_quotes() {
    let fake = FakeHrana::start(server(|_| executed(&["name"], vec![]))).await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    client
        .get_metadata_provider()
        .get_columns("it's \"odd\"", Some("a'b"))
        .await
        .unwrap();
    assert!(
        fake.executed()
            .iter()
            .any(|sql| sql.contains("pragma_table_info('it''s \"odd\"', 'a''b')")),
        "{:?}",
        fake.executed()
    );
}

#[tokio::test]
async fn explain_prints_the_plan_tree_and_never_doubles_the_prefix() {
    let fake = FakeHrana::start(server(|sql| {
        assert!(sql.starts_with("EXPLAIN QUERY PLAN "), "{sql}");
        assert!(!sql.contains("EXPLAIN QUERY PLAN EXPLAIN"), "{sql}");
        executed(
            &["id", "parent", "notused", "detail"],
            vec![
                vec![int(2), int(0), int(0), text("SCAN orders")],
                vec![int(5), int(2), int(0), text("USING INDEX orders_customer")],
            ],
        )
    }))
    .await;
    let client = LibSqlClient::new(fake.info()).await.unwrap();
    let plan = client
        .explain_query("explain query plan SELECT * FROM orders")
        .await
        .unwrap();
    assert_eq!(plan[0], ["libSQL Query Plan"]);
    assert_eq!(plan[2], ["SCAN orders"]);
    assert_eq!(plan[3], ["  USING INDEX orders_customer"]);
    let raw = client.explain_query_raw("SELECT 1").await.unwrap();
    assert_eq!(raw[0], ["id", "parent", "notused", "detail"]);
}

#[tokio::test]
async fn server_info_and_the_token_scope_claim() {
    let fake = FakeHrana::start(server(|sql| {
        assert_eq!(sql, "SELECT sqlite_version()");
        executed(&["v"], vec![vec![text("3.45.1")]])
    }))
    .await;
    let client = LibSqlClient::new(fake.info_with_token()).await.unwrap();
    let info = client.get_server_info().await.unwrap();
    assert_eq!(info.server_type, "libSQL");
    assert_eq!(info.server_version, "3.45.1");
    assert_eq!(info.version_major, Some(3));
    // what the token says about itself (not verified), never the token
    assert_eq!(
        info.additional_info
            .get("token_access_claim")
            .map(String::as_str),
        Some("ro")
    );
    assert!(!format!("{info:?}").contains("SECRET"));
    assert!(client.is_connected().await);
    assert!(!format!("{client:?}").contains("SECRET"));
}

// ----- pure pieces --------------------------------------------------------------------

#[rstest]
#[case("BEGIN", true, false)]
#[case("begin deferred transaction", true, false)]
#[case("  -- start\n BEGIN IMMEDIATE", true, false)]
#[case("/* c */ Begin", true, false)]
#[case("COMMIT", false, true)]
#[case("end transaction", false, true)]
#[case("ROLLBACK", false, true)]
#[case("ROLLBACK TRANSACTION", false, true)]
#[case("ROLLBACK TO SAVEPOINT s", false, false)]
#[case("rollback to s", false, false)]
#[case("SELECT 'begin'", false, false)]
#[case("CREATE TRIGGER t AFTER INSERT ON x BEGIN SELECT 1; END", false, false)]
#[case("SAVEPOINT s", false, false)]
#[case("", false, false)]
fn transaction_boundaries(#[case] sql: &str, #[case] begins: bool, #[case] ends: bool) {
    assert_eq!(begins_transaction(sql), begins, "{sql:?}");
    assert_eq!(ends_transaction(sql), ends, "{sql:?}");
}

#[rstest]
#[case("SELECT 1", "SELECT 1")]
#[case("explain select 1", "select 1")]
#[case("EXPLAIN QUERY PLAN select 1", "select 1")]
#[case("  explain   query   plan   select 1 ", "select 1")]
fn explain_target_strips_the_prefix(#[case] input: &str, #[case] expected: &str) {
    assert_eq!(explain_target(input), expected);
}

#[test]
fn token_access_reads_the_claim_of_a_jwt_and_nothing_else() {
    assert_eq!(token_access(TOKEN).as_deref(), Some("ro"));
    assert_eq!(token_access("not-a-jwt"), None);
    assert_eq!(token_access("a.b.c"), None);
    assert_eq!(token_access(""), None);
}

#[test]
fn the_endpoint_hides_its_token_and_scrubs_text() {
    let mut info = ConnectionInfo::parse_url("libsql://db.example").unwrap();
    info.password = Some("tok-123".to_string());
    let endpoint = Endpoint::from_info(&info).unwrap();
    assert!(!format!("{endpoint:?}").contains("tok-123"));
    assert_eq!(
        endpoint.scrub("Bearer tok-123 was refused"),
        "Bearer [REDACTED] was refused"
    );
    assert_eq!(DatabaseType::LibSql.to_string(), "libSQL");
}

#[rstest]
#[case("{\"error\":\"nope\"}", "nope")]
#[case("{\"message\":\"also nope\"}", "also nope")]
#[case("plain text body", "plain text body")]
#[case("", "")]
fn server_messages_are_unwrapped(#[case] body: &str, #[case] expected: &str) {
    assert_eq!(hrana::server_message(body), expected);
}

#[test]
fn long_error_bodies_are_cut() {
    let body = "x".repeat(2000);
    let shown = hrana::server_message(&body);
    assert!(shown.chars().count() <= 301, "{}", shown.len());
}

// ----- opt-in integration ---------------------------------------------------------------

/// `LIBSQL_URL=libsql+http://127.0.0.1:8080` (a local `sqld`, e.g.
/// `docker run -p 8080:8080 ghcr.io/tursodatabase/libsql-server:latest`) or a
/// Turso URL with its token (`libsql://db-org.turso.io?authToken=…`).
#[tokio::test]
async fn live_server_round_trip() {
    let Ok(url) = std::env::var("LIBSQL_URL") else {
        return;
    };
    let info = ConnectionInfo::parse_url(&url).expect("LIBSQL_URL parses");
    let client = LibSqlClient::new(info).await.expect("connects");

    client
        .execute_query("DROP TABLE IF EXISTS dbcrust_it")
        .await
        .unwrap();
    client
        .execute_query("CREATE TABLE dbcrust_it (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL, data BLOB)")
        .await
        .unwrap();
    client
        .execute_query(
            "INSERT INTO dbcrust_it VALUES (1, 'ada', 1.5, x'deadbeef'), (2, 'bob', NULL, NULL)",
        )
        .await
        .unwrap();
    let result = client
        .execute_query_structured("SELECT id, name, score, data FROM dbcrust_it ORDER BY id")
        .await
        .unwrap();
    assert_eq!(result.columns, ["id", "name", "score", "data"]);
    assert_eq!(result.rows[0][2].as_deref(), Some("1.5"));
    assert_eq!(result.rows[0][3].as_deref(), Some("\\xdeadbeef"));
    assert_eq!(result.rows[1][2], None);

    // a transaction spans statements, and a rollback undoes it
    client.execute_query("BEGIN").await.unwrap();
    client
        .execute_query("DELETE FROM dbcrust_it")
        .await
        .unwrap();
    assert!(
        client
            .execute_query("SELECT * FROM dbcrust_it")
            .await
            .unwrap()
            .is_empty()
    );
    client.execute_query("ROLLBACK").await.unwrap();
    assert_eq!(
        client
            .execute_query("SELECT * FROM dbcrust_it")
            .await
            .unwrap()
            .len(),
        3
    );

    let provider = client.get_metadata_provider();
    assert!(
        provider
            .get_tables(None)
            .await
            .unwrap()
            .contains(&"dbcrust_it".to_string())
    );
    let details = provider
        .get_table_details("dbcrust_it", None)
        .await
        .unwrap();
    assert_eq!(details.columns.len(), 4);
    client
        .explain_query("SELECT * FROM dbcrust_it WHERE id = 1")
        .await
        .unwrap();
    client.test_query("SELECT * FROM dbcrust_it").await.unwrap();
    assert!(client.test_query("SELEC nope").await.is_err());
    let info = client.get_server_info().await.unwrap();
    assert_eq!(info.server_type, "libSQL");

    client.execute_query("DROP TABLE dbcrust_it").await.unwrap();
}
