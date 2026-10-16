//! The CloudWatch Logs client against an in-process HTTP server that speaks just
//! enough of the JSON 1.1 protocol (`X-Amz-Target: Logs_20140328.*`): the real
//! SDK, the ring HTTP client and the request signing run for real, nothing
//! leaves the machine and no AWS account is needed.
//!
//! There is no live test: the only free emulator (LocalStack community) does not
//! run Logs Insights, which is the point of this connector, so the
//! StartQuery → GetQueryResults → StopQuery state machine is covered here and a
//! manual smoke test against AWS belongs to the release checklist.

use super::*;
use dbcrust_core::database::{DatabaseClient, set_thread_interrupt_flag};
use rstest::rstest;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const ACCESS_KEY: &str = "AKIATESTKEY0123456AB";
const SECRET_KEY: &str = "test-secret-key-value/CANARY+12345";

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    operation: String,
    body: Value,
    headers: HashMap<String, String>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn ok(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
        }
    }

    fn error(status: u16, kind: &str, message: &str) -> Self {
        Self {
            status,
            body: json!({"__type": kind, "message": message}).to_string(),
        }
    }
}

struct FakeLogs {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeLogs {
    async fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests: Arc<Mutex<Vec<Request>>> = Arc::default();
        let handler = Arc::new(handler);
        let log = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buffer: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let head_end = loop {
                            if let Some(index) =
                                buffer.windows(4).position(|window| window == b"\r\n\r\n")
                            {
                                break index;
                            }
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
                        buffer.drain(..head_end + 4);
                        let mut lines = head.lines();
                        let request_line = lines.next().unwrap_or_default();
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or_default().to_string();
                        let path = parts.next().unwrap_or_default().to_string();
                        let headers: HashMap<String, String> = lines
                            .filter_map(|line| line.split_once(':'))
                            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                            .collect();
                        let length: usize = headers
                            .get("content-length")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        while buffer.len() < length {
                            match stream.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let body: Vec<u8> = buffer.drain(..length).collect();
                        let operation = headers
                            .get("x-amz-target")
                            .and_then(|target| target.rsplit('.').next())
                            .unwrap_or_default()
                            .to_string();
                        let request = Request {
                            method,
                            path,
                            operation,
                            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                            headers,
                        };
                        log.lock().unwrap().push(request.clone());
                        let reply = handler(&request);
                        let reason = if reply.status == 200 {
                            "OK"
                        } else {
                            "Bad Request"
                        };
                        let out = format!(
                            "HTTP/1.1 {} {reason}\r\nconnection: keep-alive\r\n\
                             content-type: application/x-amz-json-1.1\r\n\
                             x-amzn-requestid: REQ123\r\ncontent-length: {}\r\n\r\n{}",
                            reply.status,
                            reply.body.len(),
                            reply.body
                        );
                        if stream.write_all(out.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { addr, requests }
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn of(&self, operation: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|request| request.operation == operation)
            .collect()
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn info(&self, group: Option<&str>, options: &[(&str, &str)]) -> ConnectionInfo {
        let mut info = ConnectionInfo {
            database_type: DatabaseType::CloudWatchLogs,
            host: Some("127.0.0.1".to_string()),
            port: Some(self.addr.port()),
            use_tls: false,
            username: Some(ACCESS_KEY.to_string()),
            password: Some(SECRET_KEY.to_string()),
            database: group.map(str::to_string),
            ..Default::default()
        };
        for (key, value) in [
            ("region", "us-east-1"),
            ("tls", "false"),
            ("max_attempts", "1"),
            ("connect_timeout", "5"),
            ("query_timeout", "30"),
        ] {
            info.options.insert(key.to_string(), value.to_string());
        }
        for (key, value) in options {
            info.options.insert(key.to_string(), value.to_string());
        }
        info
    }

    async fn client(&self, group: Option<&str>, options: &[(&str, &str)]) -> CloudWatchLogsClient {
        CloudWatchLogsClient::new(self.info(group, options))
            .await
            .unwrap()
    }
}

fn text_of(result: &LimitedResult, row: usize, column: usize) -> String {
    result.result.rows[row][column].clone().unwrap_or_default()
}

fn group(name: &str) -> Value {
    json!({
        "logGroupName": name,
        "retentionInDays": 14,
        "storedBytes": 123456,
        "metricFilterCount": 2,
        "creationTime": 1_792_137_600_000i64,
        "logGroupClass": "STANDARD",
        "kmsKeyId": "arn:aws:kms:eu-west-1:123456789012:key/abc",
        "logGroupArn": format!("arn:aws:logs:eu-west-1:123456789012:log-group:{name}:*"),
    })
}

fn complete(rows: Value, scanned_bytes: f64) -> Reply {
    Reply::ok(json!({
        "status": "Complete",
        "results": rows,
        "statistics": {"recordsMatched": 2.0, "recordsScanned": 100.0, "bytesScanned": scanned_bytes},
    }))
}

fn two_rows() -> Value {
    json!([
        [
            {"field": "@timestamp", "value": "2026-10-16 08:00:01.000"},
            {"field": "@message", "value": "boom"},
            {"field": "@ptr", "value": "CmUKJQo"}
        ],
        [
            {"field": "@timestamp", "value": "2026-10-16 08:00:00.000"},
            {"field": "@message", "value": "hello"},
            {"field": "@ptr", "value": "CmUKJQp"}
        ]
    ])
}

/// Probes and group lookups answer; everything else is the test's business.
fn with_groups(
    handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    move |request| match request.operation.as_str() {
        "DescribeLogGroups" => match request.body["logGroupNamePrefix"].as_str() {
            Some(prefix) => {
                let all = [
                    "/aws/lambda/orders",
                    "/aws/lambda/orders-v2",
                    "/aws/lambda/users",
                ];
                let matching: Vec<Value> = all
                    .iter()
                    .filter(|name| name.starts_with(prefix))
                    .map(|name| group(name))
                    .collect();
                Reply::ok(json!({"logGroups": matching}))
            }
            None => Reply::ok(json!({"logGroups": [group("/aws/lambda/orders")]})),
        },
        _ => handler(request),
    }
}

#[tokio::test]
async fn connecting_probes_the_service_and_signs_requests() {
    let server = FakeLogs::start(|_| Reply::ok(json!({"logGroups": []}))).await;
    let client = server.client(None, &[]).await;
    assert_eq!(client.get_current_database(), "");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/");
    assert_eq!(requests[0].operation, "DescribeLogGroups");
    assert_eq!(requests[0].body["limit"], 1);
    assert_eq!(
        requests[0].header("x-amz-target"),
        Some("Logs_20140328.DescribeLogGroups")
    );
    let authorization = requests[0].header("authorization").expect("signed");
    assert!(
        authorization.starts_with(&format!("AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/")),
        "{authorization}"
    );
    assert!(
        authorization.contains("/logs/aws4_request"),
        "{authorization}"
    );
    for request in &requests {
        for value in request.headers.values() {
            assert!(!value.contains(SECRET_KEY), "{value}");
        }
    }
}

#[tokio::test]
async fn the_url_path_selects_a_group_or_a_prefix() {
    let server = FakeLogs::start(with_groups(|_| Reply::ok(json!({})))).await;

    // an exact name wins over longer names sharing it as a prefix
    let exact = server.client(Some("/aws/lambda/orders"), &[]).await;
    assert_eq!(exact.get_current_database(), "/aws/lambda/orders");

    // a prefix selects every group under it
    let prefix = server.client(Some("/aws/lambda/"), &[]).await;
    assert_eq!(prefix.get_current_database(), "/aws/lambda/orders (+2)");

    let error = CloudWatchLogsClient::new(server.info(Some("/nope"), &[]))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("no log group is named or starts with '/nope'"),
        "{error}"
    );
}

#[tokio::test]
async fn the_groups_option_names_groups_and_prefixes() {
    let server = FakeLogs::start(with_groups(|_| Reply::ok(json!({})))).await;
    let client = server
        .client(None, &[("groups", "/extra/one, /aws/lambda/u*")])
        .await;
    assert_eq!(
        client.get_server_info().await.unwrap().additional_info["log_groups"],
        "/extra/one, /aws/lambda/users"
    );
}

#[tokio::test]
async fn too_many_groups_for_one_query_are_refused() {
    let server = FakeLogs::start(|request| match request.operation.as_str() {
        "DescribeLogGroups" if request.body.get("logGroupNamePrefix").is_some() => {
            let groups: Vec<Value> = (0..50).map(|i| group(&format!("/many/g{i:02}"))).collect();
            Reply::ok(json!({"logGroups": groups, "nextToken": "more"}))
        }
        _ => Reply::ok(json!({"logGroups": []})),
    })
    .await;
    let error = CloudWatchLogsClient::new(server.info(Some("/many/"), &[]))
        .await
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("more than 50 log groups"),
        "{error}"
    );
}

#[tokio::test]
async fn a_path_that_cannot_be_listed_is_taken_as_the_group_itself() {
    let server = FakeLogs::start(|request| match request.operation.as_str() {
        "DescribeLogGroups" if request.body.get("logGroupNamePrefix").is_some() => Reply::error(
            400,
            "AccessDeniedException",
            "not authorized to perform: logs:DescribeLogGroups",
        ),
        _ => Reply::ok(json!({"logGroups": []})),
    })
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    assert_eq!(client.get_current_database(), "/aws/lambda/orders");
}

#[rstest]
#[case(&[("since", "soon")], "since")]
#[case(&[("max_window", "1x")], "max_window")]
#[case(&[("since", "2d"), ("max_window", "1d")], "wider than max_window")]
#[case(&[("max_scan_gb", "lots")], "max_scan_gb")]
#[case(&[("max_scan_gb", "-1")], "max_scan_gb")]
#[case(&[("query_timeout", "soon")], "query_timeout")]
#[tokio::test]
async fn bad_options_fail_the_connection_before_any_request(
    #[case] options: &[(&str, &str)],
    #[case] expected: &str,
) {
    let server = FakeLogs::start(|_| Reply::ok(json!({"logGroups": []}))).await;
    let error = CloudWatchLogsClient::new(server.info(None, options))
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains(expected), "{error}");
    assert_eq!(server.count(), 0);
}

#[tokio::test]
async fn bad_credentials_are_explained_and_the_secret_never_shown() {
    let server = FakeLogs::start(|_| {
        Reply::error(
            400,
            "UnrecognizedClientException",
            &format!("The security token included in the request is invalid: {SECRET_KEY}"),
        )
    })
    .await;
    let error = CloudWatchLogsClient::new(server.info(None, &[]))
        .await
        .err()
        .unwrap();
    let text = error.to_string();
    assert!(text.contains("UnrecognizedClientException"), "{text}");
    assert!(text.contains("access key id is unknown"), "{text}");
    assert!(!text.contains(SECRET_KEY), "{text}");
    assert!(!format!("{error:?}").contains(SECRET_KEY));
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_connection_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let server = FakeLogs::start(|_| Reply::ok(json!({}))).await;
    let mut info = server.info(None, &[]);
    info.port = Some(port);
    let error = CloudWatchLogsClient::new(info).await.err().unwrap();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::ConnectionError(_)), "{text}");
    assert!(!text.contains(SECRET_KEY), "{text}");
}

/// StartQuery → Scheduled → Running → Complete.
fn polling_server(
    statuses: &'static [&'static str],
) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    let polls = Arc::new(AtomicUsize::new(0));
    with_groups(move |request| match request.operation.as_str() {
        "StartQuery" => Reply::ok(json!({"queryId": "q-1"})),
        "GetQueryResults" => {
            assert_eq!(request.body["queryId"], "q-1");
            let n = polls.fetch_add(1, Ordering::SeqCst);
            match statuses.get(n) {
                Some(status) if *status != "Complete" => Reply::ok(json!({
                    "status": status,
                    "results": [],
                    "statistics": {"recordsMatched": 0.0, "recordsScanned": 10.0 * n as f64, "bytesScanned": 100.0 * n as f64},
                })),
                _ => complete(two_rows(), 2048.0),
            }
        }
        "StopQuery" => Reply::ok(json!({"success": true})),
        other => Reply::error(400, "ValidationException", other),
    })
}

#[tokio::test]
async fn a_query_is_started_polled_to_completion_and_mapped() {
    let server = FakeLogs::start(polling_server(&["Scheduled", "Running", "Complete"])).await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let before = now_epoch_for_tests();
    let out = client
        .execute_limited("fields @timestamp, @message | sort @timestamp desc | limit 50")
        .await
        .unwrap();
    let after = now_epoch_for_tests();

    assert_eq!(out.result.columns, ["@timestamp", "@message"]);
    assert_eq!(out.result.rows.len(), 2);
    assert_eq!(text_of(&out, 0, 1), "boom");
    assert!(!out.truncated);
    assert!(
        out.notices[0].contains("matched 2 records, scanned 100 records (2.00 KiB) in 1 log group"),
        "{:?}",
        out.notices
    );

    let starts = server.of("StartQuery");
    assert_eq!(starts.len(), 1);
    let body = &starts[0].body;
    assert_eq!(body["logGroupNames"], json!(["/aws/lambda/orders"]));
    assert_eq!(
        body["queryString"],
        "fields @timestamp, @message | sort @timestamp desc | limit 50"
    );
    assert_eq!(body["limit"], 1000);
    // the default window is the last hour
    let (start, end) = (
        body["startTime"].as_i64().unwrap(),
        body["endTime"].as_i64().unwrap(),
    );
    assert_eq!(end - start, 3600);
    assert!(
        (before..=after).contains(&end),
        "{end} not in {before}..={after}"
    );

    assert_eq!(
        server.of("GetQueryResults").len(),
        3,
        "polled until Complete"
    );
    assert!(
        server.of("StopQuery").is_empty(),
        "a finished query is not stopped"
    );
}

fn now_epoch_for_tests() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn the_window_prefix_and_group_flag_apply_to_that_query_only() {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;

    client
        .execute_limited("since 30m fields @message")
        .await
        .unwrap();
    client
        .execute_limited(
            "between 2026-10-16 08:00 and 2026-10-16T09:30:00Z --group /aws/lambda/users,/other fields @message",
        )
        .await
        .unwrap();
    client
        .execute_limited("--group /aws/lambda/u* fields @message")
        .await
        .unwrap();
    client.execute_limited("fields @message").await.unwrap();

    let starts = server.of("StartQuery");
    let width = |i: usize| {
        starts[i].body["endTime"].as_i64().unwrap() - starts[i].body["startTime"].as_i64().unwrap()
    };
    assert_eq!(width(0), 1800);
    assert_eq!(starts[1].body["startTime"], 1_792_137_600i64);
    assert_eq!(starts[1].body["endTime"], 1_792_143_000i64);
    assert_eq!(
        starts[1].body["logGroupNames"],
        json!(["/aws/lambda/users", "/other"])
    );
    assert_eq!(starts[1].body["queryString"], "fields @message");
    assert_eq!(
        starts[2].body["logGroupNames"],
        json!(["/aws/lambda/users"])
    );
    assert_eq!(width(3), 3600, "the default window is back");
    assert_eq!(
        starts[3].body["logGroupNames"],
        json!(["/aws/lambda/orders"])
    );
}

#[tokio::test]
async fn the_since_option_sets_the_default_window() {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server
        .client(
            Some("/aws/lambda/orders"),
            &[("since", "15m"), ("max_rows", "50")],
        )
        .await;
    client.execute_limited("fields @message").await.unwrap();
    let body = &server.of("StartQuery")[0].body;
    assert_eq!(
        body["endTime"].as_i64().unwrap() - body["startTime"].as_i64().unwrap(),
        900
    );
    assert_eq!(body["limit"], 50);
}

#[rstest]
#[case("since 30d fields @message", "wider than max_window (1w)")]
#[case(
    "between 2026-10-17 and 2026-10-16 fields @message",
    "ends before it starts"
)]
#[tokio::test]
async fn a_window_that_is_too_wide_or_backwards_is_refused_before_any_request(
    #[case] line: &str,
    #[case] expected: &str,
) {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let before = server.count();
    let error = client.execute_limited(line).await.unwrap_err().to_string();
    assert!(error.contains(expected), "{error}");
    assert_eq!(server.count(), before, "nothing was started");
    assert!(server.of("StartQuery").is_empty());
}

#[tokio::test]
async fn a_query_without_a_group_says_how_to_choose_one() {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server.client(None, &[]).await;
    let error = client
        .execute_limited("fields @message")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no log group selected"), "{error}");
    assert!(server.of("StartQuery").is_empty());
}

#[tokio::test]
async fn use_chooses_the_groups_and_refuses_more_than_fifty() {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server.client(None, &[]).await;

    let out = client
        .execute_limited("use /aws/lambda/users,/aws/lambda/orders*")
        .await
        .unwrap();
    let names: Vec<String> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    assert_eq!(
        names,
        [
            "/aws/lambda/users",
            "/aws/lambda/orders",
            "/aws/lambda/orders-v2"
        ]
    );
    client.execute_limited("fields @message").await.unwrap();
    assert_eq!(
        server.of("StartQuery")[0].body["logGroupNames"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let shown = client.execute_limited("use").await.unwrap();
    assert_eq!(shown.result.rows.len(), 3);

    let many = (0..51)
        .map(|i| format!("/g{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let error = client
        .execute_limited(&format!("use {many}"))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("at most 50"), "{error}");
    assert_eq!(shown.result.rows.len(), 3);
    let error = client
        .execute_limited("use /nothing/*")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no log group starts with '/nothing/'"),
        "{error}"
    );
}

#[tokio::test]
async fn ctrl_c_stops_the_query_on_the_server() {
    let server = FakeLogs::start(polling_server(&["Running"; 1000])).await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let flag = Arc::new(AtomicBool::new(false));
    set_thread_interrupt_flag(Some(flag.clone()));
    let trigger = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        trigger.store(true, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let error = client.execute_limited("fields @message").await.unwrap_err();
    set_thread_interrupt_flag(None);
    let text = error.to_string();
    assert!(text.contains("interrupted"), "{text}");
    assert!(
        text.contains("the query was stopped on the server"),
        "{text}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let stops = server.of("StopQuery");
    assert_eq!(stops.len(), 1, "exactly one StopQuery");
    assert_eq!(stops[0].body["queryId"], "q-1");
}

#[tokio::test]
async fn the_timeout_stops_the_query_too() {
    let server = FakeLogs::start(polling_server(&["Running"; 1000])).await;
    let client = server
        .client(Some("/aws/lambda/orders"), &[("query_timeout", "1")])
        .await;
    let started = std::time::Instant::now();
    let error = client.execute_limited("fields @message").await.unwrap_err();
    let text = error.to_string();
    assert!(text.contains("timed out after 1s"), "{text}");
    assert!(
        text.contains("the query was stopped on the server"),
        "{text}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(server.of("StopQuery").len(), 1);
    assert!(
        server.of("GetQueryResults").len() >= 2,
        "it was polled with a backoff"
    );
}

#[tokio::test]
async fn a_scan_over_the_budget_is_stopped() {
    // 1 MiB budget, 2 MiB scanned while still running
    let polls = Arc::new(AtomicUsize::new(0));
    let counter = polls.clone();
    let server = FakeLogs::start(with_groups(move |request| match request.operation.as_str() {
        "StartQuery" => Reply::ok(json!({"queryId": "q-1"})),
        "GetQueryResults" => {
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::ok(json!({
                "status": "Running",
                "results": [],
                "statistics": {"recordsMatched": 5.0, "recordsScanned": 9000.0, "bytesScanned": 2.0 * 1024.0 * 1024.0},
            }))
        }
        _ => Reply::ok(json!({"success": true})),
    }))
    .await;
    let client = server
        .client(
            Some("/aws/lambda/orders"),
            &[("max_scan_gb", "0.0009765625")],
        )
        .await;
    let error = client
        .execute_limited("fields @message")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("already scanned 2.00 MiB"), "{error}");
    assert!(error.contains("max_scan_gb budget of 1.00 MiB"), "{error}");
    assert!(
        error.contains("the query was stopped on the server"),
        "{error}"
    );
    assert_eq!(server.of("StopQuery").len(), 1);
    assert_eq!(polls.load(Ordering::SeqCst), 1, "it did not keep polling");
}

#[rstest]
#[case("Failed", "the Logs Insights query failed")]
#[case("Cancelled", "was cancelled")]
#[case("Timeout", "timed out on the server")]
#[tokio::test]
async fn a_query_that_ended_badly_is_reported_and_not_stopped(
    #[case] status: &'static str,
    #[case] expected: &str,
) {
    let server = FakeLogs::start(with_groups(move |request| {
        match request.operation.as_str() {
            "StartQuery" => Reply::ok(json!({"queryId": "q-1"})),
            _ => Reply::ok(json!({"status": status, "results": []})),
        }
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let error = client
        .execute_limited("fields @message")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains(expected), "{error}");
    assert!(server.of("StopQuery").is_empty(), "it is over already");
}

#[tokio::test]
async fn a_failing_poll_still_stops_the_query() {
    let polls = Arc::new(AtomicUsize::new(0));
    let server = FakeLogs::start(with_groups(move |request| {
        match request.operation.as_str() {
            "StartQuery" => Reply::ok(json!({"queryId": "q-1"})),
            "GetQueryResults" => {
                if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Reply::ok(json!({"status": "Running", "results": []}))
                } else {
                    Reply::error(400, "ThrottlingException", "Rate exceeded")
                }
            }
            _ => Reply::ok(json!({"success": true})),
        }
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let error = client
        .execute_limited("fields @message")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ThrottlingException"), "{error}");
    assert!(error.contains("throttling"), "{error}");
    assert_eq!(server.of("StopQuery").len(), 1);
}

#[tokio::test]
async fn a_stop_that_fails_is_said_so() {
    let server = FakeLogs::start(with_groups(|request| match request.operation.as_str() {
        "StartQuery" => Reply::ok(json!({"queryId": "q-1"})),
        "GetQueryResults" => Reply::error(400, "ServiceUnavailableException", "try later"),
        _ => Reply::error(400, "ServiceUnavailableException", "try later"),
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let error = client
        .execute_limited("fields @message")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("could not confirm that query q-1 was stopped"),
        "{error}"
    );
}

#[tokio::test]
async fn a_full_result_is_reported_as_possibly_cut_short() {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server
        .client(Some("/aws/lambda/orders"), &[("max_rows", "2")])
        .await;
    let out = client.execute_limited("fields @message").await.unwrap();
    assert!(out.truncated);
    assert!(
        out.warnings[0].contains("reached its limit of 2 rows"),
        "{:?}",
        out.warnings
    );
    assert_eq!(server.of("StartQuery")[0].body["limit"], 2);
}

#[rstest]
#[case(
    "MalformedQueryException",
    "unexpected symbol found | at line 1 and position 9",
    "syntax error"
)]
#[case(
    "LimitExceededException",
    "Maximum concurrent queries",
    "30 concurrent"
)]
#[case(
    "AccessDeniedException",
    "User is not authorized to perform: logs:StartQuery",
    "logs:StartQuery"
)]
#[case(
    "ResourceNotFoundException",
    "The specified log group does not exist.",
    "`groups` lists them"
)]
#[tokio::test]
async fn start_errors_carry_the_code_a_hint_and_the_request_id(
    #[case] kind: &str,
    #[case] message: &str,
    #[case] hint: &str,
) {
    let (kind, message) = (kind.to_string(), message.to_string());
    let server = FakeLogs::start(with_groups(move |request| {
        match request.operation.as_str() {
            "StartQuery" => Reply::error(400, &kind, &message),
            _ => Reply::ok(json!({})),
        }
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let error = client.execute_limited("fields @message").await.unwrap_err();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{text}");
    assert!(text.contains(hint), "{text}");
    assert!(text.contains("[request id REQ123]"), "{text}");
    assert!(server.of("StopQuery").is_empty(), "nothing was started");
}

#[tokio::test]
async fn groups_lists_with_a_prefix_and_pages() {
    let server = FakeLogs::start(|request| {
        if request.body["limit"] == 1 {
            return Reply::ok(json!({"logGroups": []}));
        }
        match (
            request.body["logGroupNamePrefix"].as_str(),
            request.body["nextToken"].as_str(),
        ) {
            (Some("/aws/"), None) => Reply::ok(
                json!({"logGroups": [group("/aws/a"), group("/aws/b")], "nextToken": "t1"}),
            ),
            (Some("/aws/"), Some("t1")) => Reply::ok(json!({"logGroups": [group("/aws/c")]})),
            (None, None) => Reply::ok(
                json!({"logGroups": [group("/x"), group("/y"), group("/z")], "nextToken": "t9"}),
            ),
            (None, Some("t9")) => Reply::ok(json!({"logGroups": [group("/w")]})),
            _ => Reply::error(400, "InvalidParameterException", "unexpected"),
        }
    })
    .await;
    let client = server.client(None, &[]).await;

    let out = client.execute_limited("groups /aws/").await.unwrap();
    assert_eq!(
        out.result.columns,
        [
            "log_group",
            "retention",
            "stored_bytes",
            "class",
            "metric_filters",
            "created"
        ]
    );
    let names: Vec<String> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    assert_eq!(names, ["/aws/a", "/aws/b", "/aws/c"]);
    assert_eq!(text_of(&out, 0, 1), "14 days");
    assert_eq!(text_of(&out, 0, 2), "123456");
    assert_eq!(text_of(&out, 0, 3), "STANDARD");
    assert_eq!(text_of(&out, 0, 5), "2026-10-16T08:00:00.000Z");

    let limited = client.execute_limited("groups -n 2").await.unwrap();
    assert_eq!(limited.result.rows.len(), 2);
    assert!(limited.truncated);
    assert!(
        limited.warnings[0].contains("first 2 log groups"),
        "{:?}",
        limited.warnings
    );

    let databases = client.list_databases().await.unwrap();
    assert_eq!(databases[0], ["Log group"]);
    assert_eq!(
        databases.len(),
        5,
        "the header and four groups over two pages"
    );
}

#[tokio::test]
async fn streams_lists_the_most_recently_active_first() {
    let server = FakeLogs::start(with_groups(|request| match request.operation.as_str() {
        "DescribeLogStreams" => {
            assert_eq!(request.body["orderBy"], "LastEventTime");
            assert_eq!(request.body["descending"], true);
            Reply::ok(json!({"logStreams": [
                {"logStreamName": "2026/10/16/[$LATEST]aaa", "lastEventTimestamp": 1_792_137_600_000i64, "firstEventTimestamp": 1_792_130_000_000i64, "creationTime": 1_792_129_000_000i64},
                {"logStreamName": "2026/10/15/[$LATEST]bbb"}
            ]}))
        }
        _ => Reply::ok(json!({})),
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let out = client.execute_limited("streams").await.unwrap();
    assert_eq!(
        out.result.columns,
        ["log_stream", "last_event", "first_event", "created"]
    );
    assert_eq!(text_of(&out, 0, 1), "2026-10-16T08:00:00.000Z");
    assert_eq!(out.result.rows[1][1], None);
    assert_eq!(server.of("DescribeLogStreams")[0].body["limit"], 20);

    // a group named in the command wins; several selected groups need a name
    let out = client.execute_limited("streams /other -n 5").await.unwrap();
    assert_eq!(out.result.rows.len(), 2);
    assert_eq!(server.of("DescribeLogStreams")[1].body["limit"], 5);
    client.execute_limited("use /a,/b").await.unwrap();
    let error = client
        .execute_limited("streams")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("2 log groups are selected"), "{error}");
}

#[tokio::test]
async fn tail_reads_a_bounded_window_with_a_filter() {
    let server = FakeLogs::start(with_groups(|request| match request.operation.as_str() {
        "FilterLogEvents" => match request.body["nextToken"].as_str() {
            None => Reply::ok(json!({
                "events": [
                    {"logStreamName": "s1", "timestamp": 1_792_137_600_000i64, "message": "first\n"},
                    {"logStreamName": "s2", "timestamp": 1_792_137_601_500i64, "message": "second"}
                ],
                "nextToken": "n1"
            })),
            Some("n1") => Reply::ok(json!({"events": [
                {"logStreamName": "s1", "timestamp": 1_792_137_602_000i64, "message": "third"}
            ]})),
            Some(other) => Reply::error(400, "InvalidParameterException", other),
        },
        _ => Reply::ok(json!({})),
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let before = now_epoch_for_tests();
    let out = client
        .execute_limited("tail --since 2m --filter ERROR")
        .await
        .unwrap();
    assert_eq!(out.result.columns, ["timestamp", "log_stream", "message"]);
    assert_eq!(out.result.rows.len(), 3);
    assert_eq!(text_of(&out, 0, 0), "2026-10-16T08:00:00.000Z");
    assert_eq!(
        text_of(&out, 0, 2),
        "first",
        "the trailing newline is dropped"
    );
    assert!(!out.truncated);
    let calls = server.of("FilterLogEvents");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].body["logGroupName"], "/aws/lambda/orders");
    assert_eq!(calls[0].body["filterPattern"], "ERROR");
    assert_eq!(calls[0].body["limit"], 100);
    let start_ms = calls[0].body["startTime"].as_i64().unwrap();
    assert!(
        (before - 120) * 1000 <= start_ms && start_ms <= (before + 5 - 120) * 1000,
        "{start_ms}"
    );
    assert_eq!(calls[1].body["nextToken"], "n1");
}

#[tokio::test]
async fn tail_is_bounded_by_its_limit_and_its_window() {
    let server = FakeLogs::start(with_groups(|request| match request.operation.as_str() {
        "FilterLogEvents" => {
            let asked = request.body["limit"].as_u64().unwrap();
            let events: Vec<Value> = (0..asked)
                .map(|i| json!({"logStreamName": "s", "timestamp": 1_792_137_600_000i64 + i as i64, "message": format!("m{i}")}))
                .collect();
            Reply::ok(json!({"events": events, "nextToken": "more"}))
        }
        _ => Reply::ok(json!({})),
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let out = client.execute_limited("tail -n 7").await.unwrap();
    assert_eq!(out.result.rows.len(), 7);
    assert!(out.truncated);
    assert!(
        out.warnings[0].contains("oldest first"),
        "{:?}",
        out.warnings
    );
    assert_eq!(server.of("FilterLogEvents").len(), 1);
    assert_eq!(server.of("FilterLogEvents")[0].body["limit"], 7);

    let error = client
        .execute_limited("tail --since 30d")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("wider than max_window"), "{error}");
    assert_eq!(server.of("FilterLogEvents").len(), 1);
}

#[rstest]
#[case("put /aws/lambda/orders hello")]
#[case("delete /aws/lambda/orders")]
#[case("create log group /x")]
#[case("DROP TABLE x")]
#[case("since 5m")]
#[case("")]
#[tokio::test]
async fn writes_and_unknown_commands_are_refused_before_anything_is_sent(#[case] line: &str) {
    let server = FakeLogs::start(polling_server(&["Complete"])).await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let before = server.count();
    let error = client.execute_limited(line).await.unwrap_err().to_string();
    assert!(!error.is_empty());
    assert!(client.test_query(line).await.is_err());
    assert_eq!(server.count(), before, "nothing reached the endpoint");
}

#[tokio::test]
async fn only_read_operations_are_ever_sent() {
    let server = FakeLogs::start(with_groups(|request| match request.operation.as_str() {
        "StartQuery" => Reply::ok(json!({"queryId": "q-1"})),
        "GetQueryResults" => complete(two_rows(), 10.0),
        "DescribeLogStreams" => Reply::ok(json!({"logStreams": []})),
        "FilterLogEvents" => Reply::ok(json!({"events": []})),
        other => Reply::error(400, "ValidationException", other),
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    for line in [
        "groups",
        "streams",
        "tail",
        "use /aws/lambda/orders",
        "fields @message",
        "since 5m stats count(*) by bin(1m)",
        "help",
    ] {
        client.execute_limited(line).await.unwrap();
    }
    client
        .get_metadata_provider()
        .get_tables(None)
        .await
        .unwrap();
    client
        .get_metadata_provider()
        .get_table_details("/aws/lambda/orders", None)
        .await
        .unwrap();
    const READS: [&str; 6] = [
        "DescribeLogGroups",
        "DescribeLogStreams",
        "FilterLogEvents",
        "StartQuery",
        "GetQueryResults",
        "StopQuery",
    ];
    for request in server.requests() {
        assert_eq!(request.method, "POST");
        assert!(
            READS.contains(&request.operation.as_str()),
            "{}",
            request.operation
        );
    }
}

#[tokio::test]
async fn metadata_describes_groups_and_their_recent_streams() {
    let server = FakeLogs::start(with_groups(|request| match request.operation.as_str() {
        "DescribeLogStreams" => Reply::ok(json!({"logStreams": [
            {"logStreamName": "s-new", "lastEventTimestamp": 1_792_137_600_000i64},
            {"logStreamName": "s-old"}
        ]})),
        _ => Reply::ok(json!({})),
    }))
    .await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    let provider = client.get_metadata_provider();

    assert_eq!(provider.get_schemas().await.unwrap(), ["us-east-1"]);
    assert_eq!(
        provider.get_tables(None).await.unwrap(),
        ["/aws/lambda/orders"]
    );
    let columns = provider
        .get_columns("/aws/lambda/orders", None)
        .await
        .unwrap();
    assert_eq!(columns[..2], ["@timestamp", "@message"]);
    let functions = provider.get_functions(None).await.unwrap();
    for word in ["tail", "fields", "stats", "since"] {
        assert!(functions.contains(&word.to_string()), "{word}");
    }
    assert!(!provider.supports_explain());
    assert_eq!(
        provider.default_schema().as_deref(),
        Some("/aws/lambda/orders")
    );

    let details = provider
        .get_table_details("/aws/lambda/orders", None)
        .await
        .unwrap();
    let get = |name: &str| {
        details
            .columns
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .data_type
            .clone()
    };
    assert_eq!(get("retention"), "14 days");
    assert_eq!(get("stored bytes"), "123456");
    assert_eq!(get("metric filters"), "2");
    assert_eq!(get("class"), "STANDARD");
    assert_eq!(get("stream s-new"), "last event 2026-10-16T08:00:00.000Z");
    assert_eq!(get("stream s-old"), "no events");

    let error = provider.get_table_details("/nope", None).await.unwrap_err();
    assert!(
        error.to_string().contains("no log group is named '/nope'"),
        "{error}"
    );
}

#[tokio::test]
async fn switching_groups_checks_nothing_it_cannot_and_keeps_the_old_ones_on_failure() {
    let server = FakeLogs::start(with_groups(|_| Reply::ok(json!({})))).await;
    let mut client = server.client(Some("/aws/lambda/orders"), &[]).await;
    client
        .connect_to_database("/aws/lambda/users")
        .await
        .unwrap();
    assert_eq!(client.get_current_database(), "/aws/lambda/users");
    client.connect_to_database("/aws/lambda/o*").await.unwrap();
    assert_eq!(client.get_current_database(), "/aws/lambda/orders (+1)");
    assert_eq!(
        client.get_connection_info().database.as_deref(),
        Some("/aws/lambda/o")
    );
    assert!(client.connect_to_database("/nothing/*").await.is_err());
    assert_eq!(client.get_current_database(), "/aws/lambda/orders (+1)");
    assert!(client.connect_to_database("bad name").await.is_err());
    assert!(client.connect_to_database("").await.is_err());
}

#[tokio::test]
async fn is_connected_and_server_info_never_show_the_secret() {
    let server = FakeLogs::start(with_groups(|_| Reply::ok(json!({})))).await;
    let client = server.client(Some("/aws/lambda/orders"), &[]).await;
    assert!(client.is_connected().await);
    let info = client.get_server_info().await.unwrap();
    assert!(
        info.server_type
            .contains("CloudWatch Logs-compatible service at http://127.0.0.1"),
        "{}",
        info.server_type
    );
    assert_eq!(info.server_version, "us-east-1");
    assert_eq!(info.additional_info["credentials"], "static access key");
    assert_eq!(info.additional_info["default_window"], "1h");
    assert_eq!(info.additional_info["max_window"], "1w");
    assert!(!format!("{info:?}").contains(SECRET_KEY));
    assert!(client.explain_query("fields @message").await.is_err());
}

#[tokio::test]
async fn help_lists_every_command_and_the_query_form() {
    let server = FakeLogs::start(|_| Reply::ok(json!({"logGroups": []}))).await;
    let client = server.client(None, &[]).await;
    let out = client.execute_limited("help").await.unwrap();
    let commands: Vec<String> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    for verb in dbcrust_core::cloudwatch_command::command_names() {
        assert!(commands.iter().any(|c| c.starts_with(verb)), "{verb}");
    }
    assert!(commands.iter().any(|c| c.contains("fields")));
}
