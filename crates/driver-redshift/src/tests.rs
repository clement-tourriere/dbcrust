//! Client tests against an in-process fake server (PostgreSQL wire protocol,
//! simple queries only, canned replies), plus an opt-in integration test
//! against a real cluster (`REDSHIFT_URL`, skipped when unset).
//!
//! The fake answers `Parse`/`Bind` (the extended protocol) with an error and
//! records the fact, so every test also proves that the driver stays on simple
//! queries, and it never serves `pg_type`, so an unknown type OID (`SUPER`,
//! `VARBYTE`, `GEOMETRY`) decoding at all proves no catalog lookup happens.

use super::*;
use dbcrust_core::database::{DatabaseType, set_thread_interrupt_flag};
use dbcrust_core::options::{OPT_HARDEN_READ_ONLY, OPT_HARDEN_TIMEOUT_SECS};
use rstest::rstest;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

// ----- fake server -----------------------------------------------------------

#[derive(Clone)]
enum Reply {
    /// Column (name, type OID) and text rows.
    Rows(Vec<(&'static str, u32)>, Vec<Vec<Option<&'static str>>>),
    Done(&'static str),
    Error(&'static str, &'static str),
    /// Block until a `pg_cancel_backend` arrives, then fail with 57014.
    UntilCancelled,
}

const BOOL: u32 = 16;
const INT4: u32 = 23;
const TEXT: u32 = 25;
const SUPER: u32 = 4000;
const GEOMETRY: u32 = 3000;
const VARBYTE: u32 = 6551;

type Handler = dyn Fn(&str) -> Vec<Reply> + Send + Sync;

struct FakeRedshift {
    addr: SocketAddr,
    queries: Arc<Mutex<Vec<String>>>,
    startups: Arc<Mutex<Vec<HashMap<String, String>>>>,
    extended_protocol_used: Arc<AtomicBool>,
}

fn put_i32(buffer: &mut Vec<u8>, value: i32) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn put_i16(buffer: &mut Vec<u8>, value: i16) {
    buffer.extend_from_slice(&value.to_be_bytes());
}

fn message(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    put_i32(&mut out, body.len() as i32 + 4);
    out.extend_from_slice(body);
    out
}

fn error_message(code: &str, text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for (field, value) in [(b'S', "ERROR"), (b'C', code), (b'M', text)] {
        body.push(field);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);
    message(b'E', &body)
}

fn encode(reply: &Reply) -> Vec<u8> {
    match reply {
        Reply::Rows(columns, rows) => {
            let mut out = Vec::new();
            let mut body = Vec::new();
            put_i16(&mut body, columns.len() as i16);
            for (name, oid) in columns {
                body.extend_from_slice(name.as_bytes());
                body.push(0);
                put_i32(&mut body, 0); // table oid
                put_i16(&mut body, 0); // attribute number
                put_i32(&mut body, *oid as i32);
                put_i16(&mut body, -1); // type size
                put_i32(&mut body, -1); // type modifier
                put_i16(&mut body, 0); // text format
            }
            out.extend(message(b'T', &body));
            for row in rows {
                let mut body = Vec::new();
                put_i16(&mut body, row.len() as i16);
                for cell in row {
                    match cell {
                        Some(text) => {
                            put_i32(&mut body, text.len() as i32);
                            body.extend_from_slice(text.as_bytes());
                        }
                        None => put_i32(&mut body, -1),
                    }
                }
                out.extend(message(b'D', &body));
            }
            out.extend(message(b'C', format!("SELECT {}\0", rows.len()).as_bytes()));
            out
        }
        Reply::Done(tag) => message(b'C', format!("{tag}\0").as_bytes()),
        Reply::Error(code, text) => error_message(code, text),
        Reply::UntilCancelled => unreachable!("handled by the connection loop"),
    }
}

async fn read_message(socket: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut kind = [0u8; 1];
    socket.read_exact(&mut kind).await.ok()?;
    let mut length = [0u8; 4];
    socket.read_exact(&mut length).await.ok()?;
    let mut body = vec![0u8; (i32::from_be_bytes(length) - 4) as usize];
    socket.read_exact(&mut body).await.ok()?;
    Some((kind[0], body))
}

impl FakeRedshift {
    async fn start(handler: impl Fn(&str) -> Vec<Reply> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let queries: Arc<Mutex<Vec<String>>> = Arc::default();
        let startups: Arc<Mutex<Vec<HashMap<String, String>>>> = Arc::default();
        let extended = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(Notify::new());
        let handler: Arc<Handler> = Arc::new(handler);
        let server = Self {
            addr,
            queries: queries.clone(),
            startups: startups.clone(),
            extended_protocol_used: extended.clone(),
        };
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let (handler, queries, startups, extended, cancelled) = (
                    handler.clone(),
                    queries.clone(),
                    startups.clone(),
                    extended.clone(),
                    cancelled.clone(),
                );
                tokio::spawn(async move {
                    // Startup message: length, protocol, key\0value\0 … \0
                    let mut length = [0u8; 4];
                    if socket.read_exact(&mut length).await.is_err() {
                        return;
                    }
                    let mut body = vec![0u8; (i32::from_be_bytes(length) - 4) as usize];
                    if socket.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let parts: Vec<String> = body[4..]
                        .split(|byte| *byte == 0)
                        .map(|part| String::from_utf8_lossy(part).into_owned())
                        .collect();
                    startups.lock().unwrap().push(
                        parts
                            .chunks(2)
                            .filter(|pair| pair.len() == 2 && !pair[0].is_empty())
                            .map(|pair| (pair[0].clone(), pair[1].clone()))
                            .collect(),
                    );
                    let mut hello = message(b'R', &0i32.to_be_bytes());
                    hello.extend(message(b'S', b"server_version\x008.0.2\x00"));
                    let mut key = Vec::new();
                    put_i32(&mut key, 4242);
                    put_i32(&mut key, 7);
                    hello.extend(message(b'K', &key));
                    hello.extend(message(b'Z', b"I"));
                    if socket.write_all(&hello).await.is_err() {
                        return;
                    }
                    while let Some((kind, body)) = read_message(&mut socket).await {
                        match kind {
                            b'Q' => {
                                let sql =
                                    String::from_utf8_lossy(&body[..body.len() - 1]).into_owned();
                                queries.lock().unwrap().push(sql.clone());
                                let mut out = Vec::new();
                                for reply in handler(&sql) {
                                    if let Reply::UntilCancelled = reply {
                                        let _ = socket.write_all(&out).await;
                                        out.clear();
                                        cancelled.notified().await;
                                        out.extend(error_message(
                                            "57014",
                                            "Query cancelled on user's request",
                                        ));
                                    } else {
                                        out.extend(encode(&reply));
                                    }
                                }
                                if sql.starts_with("SELECT pg_cancel_backend(") {
                                    cancelled.notify_waiters();
                                }
                                out.extend(message(b'Z', b"I"));
                                if socket.write_all(&out).await.is_err() {
                                    return;
                                }
                            }
                            b'X' => return,
                            b'P' | b'B' | b'D' | b'E' | b'S' => {
                                extended.store(true, Ordering::SeqCst);
                                let mut out =
                                    error_message("0A000", "extended protocol is not supported");
                                out.extend(message(b'Z', b"I"));
                                let _ = socket.write_all(&out).await;
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        server
    }

    fn info(&self) -> ConnectionInfo {
        let mut info = ConnectionInfo {
            database_type: DatabaseType::Redshift,
            host: Some("127.0.0.1".to_string()),
            port: Some(self.addr.port()),
            username: Some("awsuser".to_string()),
            password: Some("s3cret".to_string()),
            ..Default::default()
        };
        // The fake has no TLS
        info.options
            .insert("sslmode".to_string(), "disable".to_string());
        info
    }

    fn queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }

    /// Statements the user caused (not the driver's bookkeeping).
    fn user_queries(&self) -> Vec<String> {
        self.queries()
            .into_iter()
            .filter(|sql| !sql.starts_with("SELECT pg_backend_pid") && !sql.starts_with("SET "))
            .collect()
    }
}

fn pid_reply() -> Vec<Reply> {
    vec![Reply::Rows(
        vec![("pg_backend_pid", INT4)],
        vec![vec![Some("4242")]],
    )]
}

// ----- tests -----------------------------------------------------------------

#[tokio::test]
async fn simple_queries_return_every_redshift_type_as_text() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        sql if sql.starts_with("SELECT * FROM types") => vec![Reply::Rows(
            vec![
                ("id", INT4),
                ("flag", BOOL),
                ("doc", SUPER),
                ("geo", GEOMETRY),
                ("raw", VARBYTE),
                ("gone", TEXT),
                ("empty", TEXT),
            ],
            vec![vec![
                Some("7"),
                Some("t"),
                Some("{\"a\":[1,2,{\"b\":null}]}"),
                Some("0101000000000000000000F03F0000000000000040"),
                Some("deadbeef"),
                None,
                Some(""),
            ]],
        )],
        _ => vec![Reply::Error("42601", "unexpected statement")],
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();

    let result = client
        .execute_query_structured("SELECT * FROM types")
        .await
        .unwrap();
    assert_eq!(
        result.columns,
        ["id", "flag", "doc", "geo", "raw", "gone", "empty"]
    );
    assert_eq!(
        result.rows,
        vec![vec![
            Some("7".to_string()),
            Some("true".to_string()),
            Some("{\"a\":[1,2,{\"b\":null}]}".to_string()),
            Some("0101000000000000000000F03F0000000000000040".to_string()),
            Some("deadbeef".to_string()),
            None,                // NULL
            Some(String::new()), // empty string, not NULL
        ]]
    );
    // The legacy shape renders NULL as the empty string
    let legacy = client.execute_query("SELECT * FROM types").await.unwrap();
    assert_eq!(legacy[0][0], "id");
    assert_eq!(legacy[1][5], "");

    assert!(
        !server.extended_protocol_used.load(Ordering::SeqCst),
        "{:?}",
        server.queries()
    );
    assert!(
        server.queries().iter().all(|sql| !sql.contains("pg_type")),
        "no type catalog lookup: {:?}",
        server.queries()
    );
}

#[tokio::test]
async fn startup_sends_no_options_and_defaults_the_database() {
    let server = FakeRedshift::start(|_| pid_reply()).await;
    let mut info = server.info();
    info.options
        .insert(OPT_HARDEN_TIMEOUT_SECS.to_string(), "30".to_string());
    let client = RedshiftClient::new(info).await.unwrap();
    assert_eq!(client.get_current_database(), "dev");

    let startups = server.startups.lock().unwrap().clone();
    let startup = &startups[0];
    assert_eq!(startup.get("user").map(String::as_str), Some("awsuser"));
    assert_eq!(startup.get("database").map(String::as_str), Some("dev"));
    assert_eq!(
        startup.get("application_name").map(String::as_str),
        Some("dbcrust")
    );
    assert!(
        !startup.contains_key("options"),
        "Redshift does not take `options=-c …`: {startup:?}"
    );
    // The statement timeout is a SET on every new connection, in milliseconds
    assert!(
        server
            .queries()
            .contains(&"SET statement_timeout TO 30000".to_string()),
        "{:?}",
        server.queries()
    );
}

#[tokio::test]
async fn read_only_wraps_each_statement_in_a_read_only_transaction() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        "SELECT 1" => vec![Reply::Rows(vec![("?column?", INT4)], vec![vec![Some("1")]])],
        "SELECT broken" => vec![Reply::Error("42703", "column \"broken\" does not exist")],
        _ => vec![Reply::Done("BEGIN")],
    })
    .await;
    let mut info = server.info();
    info.options
        .insert(OPT_HARDEN_READ_ONLY.to_string(), "1".to_string());
    let client = RedshiftClient::new(info).await.unwrap();

    client.execute_query("SELECT 1").await.unwrap();
    let error = client.execute_query("SELECT broken").await.unwrap_err();
    assert!(error.to_string().contains("does not exist"), "{error}");
    // The session is still usable after the error
    client.execute_query("SELECT 1").await.unwrap();

    assert_eq!(
        server.user_queries(),
        [
            "BEGIN READ ONLY",
            "SELECT 1",
            "ROLLBACK",
            "BEGIN READ ONLY",
            "SELECT broken",
            "ROLLBACK", // an error does not leave the transaction open
            "BEGIN READ ONLY",
            "SELECT 1",
            "ROLLBACK",
        ]
    );
}

#[tokio::test]
async fn statements_run_as_is_when_not_read_only() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        _ => vec![Reply::Done("INSERT 0 1")],
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let result = client
        .execute_query("INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(server.user_queries(), ["INSERT INTO t VALUES (1)"]);
}

#[tokio::test]
async fn only_the_first_result_set_with_rows_is_returned() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        _ => vec![
            Reply::Done("SET"),
            Reply::Rows(vec![("a", INT4)], vec![vec![Some("1")], vec![Some("2")]]),
            Reply::Rows(vec![("b", TEXT)], vec![vec![Some("x")]]),
        ],
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let result = client
        .execute_query_structured("SET x TO 1; SELECT a; SELECT b")
        .await
        .unwrap();
    assert_eq!(result.columns, ["a"]);
    assert_eq!(result.rows.len(), 2);
}

#[tokio::test]
async fn server_errors_carry_the_server_message_and_keep_the_session() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        "SELECT oops" => vec![Reply::Error("42703", "column \"oops\" does not exist")],
        _ => vec![Reply::Rows(vec![("n", INT4)], vec![vec![Some("1")]])],
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let error = client.execute_query("SELECT oops").await.unwrap_err();
    assert!(
        error.to_string().contains("column \"oops\" does not exist"),
        "{error}"
    );
    assert_eq!(client.execute_query("SELECT 1").await.unwrap()[1][0], "1");
    // One pinned session for both statements: only one pid lookup
    assert_eq!(
        server
            .queries()
            .iter()
            .filter(|sql| sql.starts_with("SELECT pg_backend_pid"))
            .count(),
        1
    );
}

#[tokio::test]
async fn interrupt_cancels_server_side_through_a_second_connection() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        "SELECT slow" => vec![Reply::UntilCancelled],
        _ => vec![Reply::Done("SELECT 0")],
    })
    .await;
    let flag = Arc::new(AtomicBool::new(false));
    set_thread_interrupt_flag(Some(flag.clone()));
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let trigger = flag.clone();
    let interrupt = async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        trigger.store(true, Ordering::Relaxed);
    };
    let (result, ()) = tokio::join!(client.execute_query("SELECT slow"), interrupt);
    set_thread_interrupt_flag(None);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Query cancelled"), "{error}");
    assert!(
        server
            .queries()
            .contains(&"SELECT pg_cancel_backend(4242)".to_string()),
        "{:?}",
        server.queries()
    );
}

#[tokio::test]
async fn explain_is_the_text_plan_one_line_per_row() {
    let server = FakeRedshift::start(|sql| match sql {
        "SELECT pg_backend_pid()" => pid_reply(),
        sql if sql.starts_with("EXPLAIN ") => vec![Reply::Rows(
            vec![("QUERY PLAN", TEXT)],
            vec![
                vec![Some(
                    "XN Seq Scan on orders  (cost=0.00..0.10 rows=10 width=4)",
                )],
                vec![Some("  Filter: (id > 1)")],
            ],
        )],
        _ => vec![Reply::Error("42601", "unexpected")],
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let plan = client
        .explain_query("SELECT id FROM orders WHERE id > 1")
        .await
        .unwrap();
    assert_eq!(plan[0], ["QUERY PLAN"]);
    assert_eq!(plan.len(), 3);
    assert!(plan[1][0].starts_with("XN Seq Scan"));
    // Already-prefixed input is not prefixed twice, and never asks for JSON
    client.explain_query("explain SELECT 1").await.unwrap();
    client.explain_query_raw("SELECT 2").await.unwrap();
    let queries = server.user_queries();
    assert_eq!(
        queries,
        [
            "EXPLAIN SELECT id FROM orders WHERE id > 1",
            "EXPLAIN SELECT 1",
            "EXPLAIN SELECT 2"
        ]
    );
    assert!(client.explain_analyze_query_raw("SELECT 1").await.is_err());
}

#[tokio::test]
async fn metadata_uses_the_svv_views_with_escaped_literals() {
    let server = FakeRedshift::start(|sql| {
        if sql.contains("FROM svv_tables") {
            vec![Reply::Rows(
                vec![("table_schema", TEXT), ("table_name", TEXT)],
                vec![
                    vec![Some("public"), Some("orders")],
                    vec![Some("sales"), Some("daily.totals")],
                    vec![Some("spectrum"), Some("events")],
                ],
            )]
        } else if sql.contains("svv_external_schemas") {
            vec![Reply::Rows(
                vec![("schema_name", TEXT)],
                vec![vec![Some("public")], vec![Some("spectrum")]],
            )]
        } else if sql.contains("FROM svv_columns") && sql.contains("column_default") {
            vec![Reply::Rows(
                vec![
                    ("column_name", TEXT),
                    ("data_type", TEXT),
                    ("character_maximum_length", INT4),
                    ("numeric_precision", INT4),
                    ("numeric_scale", INT4),
                    ("is_nullable", TEXT),
                    ("column_default", TEXT),
                ],
                vec![
                    vec![
                        Some("id"),
                        Some("integer"),
                        None,
                        Some("32"),
                        Some("0"),
                        Some("NO"),
                        None,
                    ],
                    vec![
                        Some("name"),
                        Some("character varying"),
                        Some("256"),
                        None,
                        None,
                        Some("YES"),
                        Some("'n/a'::character varying"),
                    ],
                    vec![
                        Some("amount"),
                        Some("numeric"),
                        None,
                        Some("12"),
                        Some("2"),
                        Some("YES"),
                        None,
                    ],
                    vec![
                        Some("payload"),
                        Some("super"),
                        None,
                        None,
                        None,
                        Some("YES"),
                        None,
                    ],
                ],
            )]
        } else if sql.contains("FROM svv_columns") {
            vec![Reply::Rows(
                vec![("column_name", TEXT)],
                vec![vec![Some("id")], vec![Some("name")]],
            )]
        } else if sql.contains("FROM pg_table_def") {
            vec![Reply::Rows(
                vec![
                    ("column", TEXT),
                    ("encoding", TEXT),
                    ("distkey", BOOL),
                    ("sortkey", INT4),
                ],
                vec![
                    vec![Some("id"), Some("az64"), Some("t"), Some("2")],
                    vec![Some("name"), Some("lzo"), Some("f"), Some("1")],
                    vec![Some("amount"), Some("none"), Some("f"), Some("0")],
                ],
            )]
        } else if sql.contains("FROM svv_table_info") {
            vec![Reply::Rows(
                vec![
                    ("diststyle", TEXT),
                    ("sortkey1", TEXT),
                    ("sortkey_num", INT4),
                    ("size", INT4),
                    ("tbl_rows", INT4),
                ],
                vec![vec![
                    Some("KEY(id)"),
                    Some("name"),
                    Some("2"),
                    Some("12"),
                    Some("99"),
                ]],
            )]
        } else if sql.contains("pg_get_constraintdef") && sql.contains("con.contype = 'f'") {
            vec![Reply::Rows(
                vec![("a", TEXT), ("b", TEXT), ("c", TEXT), ("d", TEXT)],
                vec![vec![
                    Some("sales"),
                    Some("lines"),
                    Some("lines_order_fk"),
                    Some("FOREIGN KEY (order_id) REFERENCES orders(id)"),
                ]],
            )]
        } else if sql.contains("pg_get_constraintdef") {
            vec![Reply::Rows(
                vec![("conname", TEXT), ("contype", TEXT), ("def", TEXT)],
                vec![
                    vec![Some("orders_pkey"), Some("p"), Some("PRIMARY KEY (id)")],
                    vec![
                        Some("orders_cust_fk"),
                        Some("f"),
                        Some("FOREIGN KEY (cust) REFERENCES customers(id)"),
                    ],
                ],
            )]
        } else if sql.contains("FROM pg_proc") {
            vec![Reply::Rows(
                vec![("nspname", TEXT), ("proname", TEXT)],
                vec![vec![Some("public"), Some("f_clean")]],
            )]
        } else if sql.starts_with("SELECT pg_backend_pid") {
            pid_reply()
        } else {
            vec![Reply::Error("42601", "unexpected statement")]
        }
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let provider = client.get_metadata_provider();

    assert_eq!(
        provider.get_schemas().await.unwrap(),
        ["public", "spectrum"]
    );
    assert_eq!(
        provider.get_tables(None).await.unwrap(),
        ["orders", "sales.\"daily.totals\"", "spectrum.events"]
    );
    assert_eq!(
        provider.get_columns("orders", None).await.unwrap(),
        ["id", "name"]
    );
    assert_eq!(provider.get_functions(None).await.unwrap(), ["f_clean"]);
    assert_eq!(provider.default_schema().as_deref(), Some("public"));

    let details = provider.get_table_details("orders", None).await.unwrap();
    let columns: Vec<_> = details
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.data_type.as_str(), c.nullable))
        .collect();
    assert_eq!(
        columns,
        [
            ("id", "integer ENCODE az64", false),
            ("name", "character varying(256) ENCODE lzo", true),
            ("amount", "numeric(12,2)", true),
            ("payload", "super", true),
        ]
    );
    assert_eq!(
        details.columns[1].default_value.as_deref(),
        Some("'n/a'::character varying")
    );
    let indexes: Vec<_> = details
        .indexes
        .iter()
        .map(|i| (i.name.as_str(), i.definition.as_str(), i.is_primary))
        .collect();
    assert_eq!(
        indexes,
        [
            ("DISTSTYLE", "KEY(id)", false),
            ("SORTKEY", "name, id", false),
            ("orders_pkey", "PRIMARY KEY (id)", true),
        ]
    );
    assert_eq!(details.foreign_keys.len(), 1);
    assert_eq!(details.foreign_keys[0].name, "orders_cust_fk");
    assert_eq!(details.referenced_by[0].table, "lines");

    // An explicit schema narrows the listing and escapes the literal
    provider.get_tables(Some("o'brien\\")).await.ok();
    assert!(
        server
            .queries()
            .iter()
            .any(|sql| sql.contains("table_schema = 'o''brien\\\\'")),
        "{:?}",
        server.queries()
    );
}

#[tokio::test]
async fn enrichment_failures_do_not_fail_the_table_description() {
    let server = FakeRedshift::start(|sql| {
        if sql.starts_with("SELECT pg_backend_pid") {
            pid_reply()
        } else if sql.contains("FROM svv_columns") {
            vec![Reply::Rows(
                vec![
                    ("column_name", TEXT),
                    ("data_type", TEXT),
                    ("character_maximum_length", INT4),
                    ("numeric_precision", INT4),
                    ("numeric_scale", INT4),
                    ("is_nullable", TEXT),
                    ("column_default", TEXT),
                ],
                vec![vec![
                    Some("id"),
                    Some("integer"),
                    None,
                    None,
                    None,
                    Some("YES"),
                    None,
                ]],
            )]
        } else {
            vec![Reply::Error("42P01", "relation does not exist")]
        }
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let details = client
        .get_metadata_provider()
        .get_table_details("t", Some("s"))
        .await
        .unwrap();
    assert_eq!(details.columns.len(), 1);
    assert!(details.indexes.is_empty());
}

#[tokio::test]
async fn server_info_reports_the_redshift_build() {
    let server = FakeRedshift::start(|sql| {
        if sql.starts_with("SELECT version()") {
            vec![Reply::Rows(
                vec![("version", TEXT), ("current_user", TEXT)],
                vec![vec![
                    Some(
                        "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by gcc (GCC) 3.4.2 20041017 (Red Hat 3.4.2-6.fc3), Redshift 1.0.94871",
                    ),
                    Some("awsuser"),
                ]],
            )]
        } else if sql.starts_with("SELECT 1") {
            vec![Reply::Rows(vec![("?column?", INT4)], vec![vec![Some("1")]])]
        } else {
            pid_reply()
        }
    })
    .await;
    let client = RedshiftClient::new(server.info()).await.unwrap();
    let info = client.get_server_info().await.unwrap();
    assert_eq!(info.server_type, "Redshift");
    assert_eq!(info.server_version, "1.0.94871");
    assert_eq!(info.version_major, Some(1));
    assert_eq!(
        info.additional_info.get("current_user").map(String::as_str),
        Some("awsuser")
    );
    assert!(client.is_connected().await);
}

#[tokio::test]
async fn a_refused_connection_is_a_connection_error_without_the_password() {
    // Nothing listens on this port
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut info = ConnectionInfo {
        database_type: DatabaseType::Redshift,
        host: Some("127.0.0.1".to_string()),
        port: Some(port),
        username: Some("awsuser".to_string()),
        password: Some("hunter2-canary".to_string()),
        ..Default::default()
    };
    info.options
        .insert("sslmode".to_string(), "disable".to_string());
    let error = match RedshiftClient::new(info).await {
        Ok(_) => panic!("nothing is listening"),
        Err(error) => error,
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error}"
    );
    assert!(!error.to_string().contains("hunter2-canary"), "{error}");
}

// ----- pure pieces -----------------------------------------------------------

#[rstest]
#[case("require", sqlx::postgres::PgSslMode::Require)]
#[case("verify-full", sqlx::postgres::PgSslMode::VerifyFull)]
#[case("verify-ca", sqlx::postgres::PgSslMode::VerifyCa)]
#[case("disable", sqlx::postgres::PgSslMode::Disable)]
#[case("prefer", sqlx::postgres::PgSslMode::Prefer)]
fn sslmode_maps_to_the_sqlx_mode(#[case] mode: &str, #[case] expected: sqlx::postgres::PgSslMode) {
    let mut info = ConnectionInfo::default();
    info.options.insert("sslmode".to_string(), mode.to_string());
    assert!(
        std::mem::discriminant(&conn::ssl_mode(&info)) == std::mem::discriminant(&expected),
        "{mode}"
    );
}

#[test]
fn no_sslmode_means_require_and_the_default_port_is_5439() {
    let mut info = ConnectionInfo::parse_url("redshift://awsuser:pw@c.example/dev").unwrap();
    // hand-built infos without the normalised option still require TLS
    info.options.clear();
    assert!(matches!(
        conn::ssl_mode(&info),
        sqlx::postgres::PgSslMode::Require
    ));
    let options = conn::connect_options(&info);
    assert_eq!(options.get_port(), 5439);
    assert_eq!(options.get_host(), "c.example");
    assert_eq!(options.get_database(), Some("dev"));
    assert_eq!(options.get_username(), "awsuser");
    assert_eq!(options.get_options(), None);
}

#[rstest]
#[case("explain select 1", "select 1")]
#[case("  EXPLAIN   select 1  ", "select 1")]
#[case("select 1", "select 1")]
#[case("explanation_of_things", "explanation_of_things")]
fn explain_target_strips_one_leading_explain(#[case] input: &str, #[case] expected: &str) {
    assert_eq!(explain_target(input), expected);
}

#[test]
fn the_redshift_build_is_pulled_out_of_the_banner() {
    assert_eq!(
        redshift_version(
            "PostgreSQL 8.0.2 on i686-pc-linux-gnu, compiled by gcc, Redshift 1.0.94871"
        ),
        Some("1.0.94871")
    );
    assert_eq!(redshift_version("PostgreSQL 16.2"), None);
}

#[rstest]
#[case("plain", "'plain'")]
#[case("o'brien", "'o''brien'")]
#[case("back\\slash", "'back\\\\slash'")]
#[case("trailing\\", "'trailing\\\\'")]
#[case("\\' OR 1=1 --", "'\\\\'' OR 1=1 --'")]
fn literals_cannot_break_out_of_their_quotes(#[case] value: &str, #[case] expected: &str) {
    assert_eq!(metadata::lit(value), expected);
}

#[rstest]
#[case("character varying", Some("256"), None, None, "character varying(256)")]
#[case("character", Some("10"), None, None, "character(10)")]
#[case("numeric", None, Some("18"), Some("4"), "numeric(18,4)")]
#[case("integer", None, Some("32"), Some("0"), "integer")]
#[case("super", None, None, None, "super")]
#[case("character varying", None, None, None, "character varying")]
fn types_display_like_ddl(
    #[case] data_type: &str,
    #[case] length: Option<&str>,
    #[case] precision: Option<&str>,
    #[case] scale: Option<&str>,
    #[case] expected: &str,
) {
    assert_eq!(
        metadata::display_type(data_type, length, precision, scale),
        expected
    );
}

#[test]
fn sort_keys_come_in_declared_order_descending_ones_included() {
    let keyed = vec![
        (2, "b".to_string()),
        (0, "none".to_string()),
        (-1, "a".to_string()),
        (3, "c".to_string()),
    ];
    assert_eq!(metadata::sort_key_columns(keyed), ["a", "b", "c"]);
}

#[test]
fn catalog_queries_name_the_redshift_views_and_nothing_newer() {
    for sql in [
        metadata::schemas_sql(),
        metadata::tables_sql(None),
        metadata::tables_sql(Some("s")),
        metadata::columns_sql("s", "t"),
        metadata::functions_sql(None),
        metadata::detail_columns_sql("s", "t"),
        metadata::table_def_sql("s", "t"),
        metadata::table_info_sql("s", "t"),
        metadata::constraints_sql("s", "t"),
        metadata::referenced_by_sql("s", "t"),
    ] {
        // Not available on PostgreSQL 8.0.2, and no bind parameters
        for newer in [
            "prokind",
            "pg_get_serial_sequence",
            "pg_range",
            "$1",
            "information_schema.",
        ] {
            assert!(!sql.contains(newer), "{newer} in {sql}");
        }
    }
    assert!(metadata::schemas_sql().contains("svv_external_schemas"));
    assert!(metadata::tables_sql(None).contains("FROM svv_tables"));
    assert!(metadata::table_info_sql("s", "t").contains("\"schema\" = 's'"));
}

#[test]
fn text_values_render_as_the_server_printed_them() {
    assert_eq!(value::render("BOOL", b"t"), "true");
    assert_eq!(value::render("BOOL", b"f"), "false");
    assert_eq!(value::render("TEXT", b"t"), "t");
    assert_eq!(value::render("?", b"{\"a\": 1}"), "{\"a\": 1}");
    assert_eq!(value::render("?", &[0xde, 0xad, 0xff]), "\\xdeadff");
}

// ----- opt-in integration ------------------------------------------------------

/// `REDSHIFT_URL=redshift://user:pass@cluster:5439/dev`, a real cluster or
/// Serverless workgroup. Read-only statements only.
#[tokio::test]
async fn live_cluster_round_trip() {
    let Ok(url) = std::env::var("REDSHIFT_URL") else {
        return;
    };
    let info = ConnectionInfo::parse_url(&url).expect("REDSHIFT_URL parses");
    let client = RedshiftClient::new(info).await.expect("connects");

    let result = client
        .execute_query_structured("SELECT 1 AS one, 'a'::varchar AS s, NULL::int AS n, true AS b, JSON_PARSE('{\"k\":[1,2]}') AS doc")
        .await
        .unwrap();
    assert_eq!(result.columns, ["one", "s", "n", "b", "doc"]);
    assert_eq!(result.rows[0][0].as_deref(), Some("1"));
    assert_eq!(result.rows[0][2], None);
    assert_eq!(result.rows[0][3].as_deref(), Some("true"));
    assert_eq!(result.rows[0][4].as_deref(), Some("{\"k\":[1,2]}"));

    let provider = client.get_metadata_provider();
    provider.get_schemas().await.expect("schemas");
    provider.get_tables(None).await.expect("tables");
    client.list_databases().await.expect("databases");
    client.explain_query("SELECT 1").await.expect("explain");
    let info = client.get_server_info().await.expect("server info");
    assert_eq!(info.server_type, "Redshift");
}
