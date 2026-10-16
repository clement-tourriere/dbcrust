//! The DynamoDB client against an in-process HTTP server that speaks just enough
//! of the DynamoDB JSON 1.0 protocol (`X-Amz-Target: DynamoDB_20120810.*`): the
//! real SDK, the ring HTTP client and the request signing run for real, nothing
//! leaves the machine and no AWS account is needed.
//! A live run against DynamoDB Local is gated on `DYNAMODB_TEST_URL`.

use super::*;
use dbcrust_core::database::{DatabaseClient, set_thread_interrupt_flag};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const ACCESS_KEY: &str = "AKIATESTKEY0123456AB";
const SECRET_KEY: &str = "test-secret-key-value/CANARY+12345";

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    /// The operation: `ListTables`, `Scan`…
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
    delay: Option<Duration>,
}

impl Reply {
    fn ok(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            delay: None,
        }
    }

    fn error(status: u16, kind: &str, message: &str) -> Self {
        Self {
            status,
            body: json!({
                "__type": format!("com.amazonaws.dynamodb.v20120810#{kind}"),
                "message": message,
            })
            .to_string(),
            delay: None,
        }
    }
}

struct FakeDynamo {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeDynamo {
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
                        // one request (head + body) per iteration (keep-alive)
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
                        if let Some(delay) = reply.delay {
                            tokio::time::sleep(delay).await;
                        }
                        let reason = if reply.status == 200 {
                            "OK"
                        } else {
                            "Bad Request"
                        };
                        let out = format!(
                            "HTTP/1.1 {} {reason}\r\nconnection: keep-alive\r\n\
                             content-type: application/x-amz-json-1.0\r\n\
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

    fn info(&self, table: Option<&str>, options: &[(&str, &str)]) -> ConnectionInfo {
        let mut info = ConnectionInfo {
            database_type: DatabaseType::DynamoDB,
            host: Some("127.0.0.1".to_string()),
            port: Some(self.addr.port()),
            use_tls: false,
            username: Some(ACCESS_KEY.to_string()),
            password: Some(SECRET_KEY.to_string()),
            database: table.map(str::to_string),
            ..Default::default()
        };
        for (key, value) in [
            ("region", "us-east-1"),
            ("tls", "false"),
            ("max_attempts", "1"),
            ("connect_timeout", "5"),
        ] {
            info.options.insert(key.to_string(), value.to_string());
        }
        for (key, value) in options {
            info.options.insert(key.to_string(), value.to_string());
        }
        info
    }

    async fn client(&self, table: Option<&str>, options: &[(&str, &str)]) -> DynamoClient {
        DynamoClient::new(self.info(table, options)).await.unwrap()
    }
}

fn text_of(result: &LimitedResult, row: usize, column: usize) -> String {
    result.result.rows[row][column].clone().unwrap_or_default()
}

fn column_of(result: &LimitedResult, name: &str) -> usize {
    result
        .result
        .columns
        .iter()
        .position(|column| column == name)
        .unwrap_or_else(|| panic!("no column {name} in {:?}", result.result.columns))
}

/// A table with a string partition key `customer` and a number sort key `ts`,
/// and a global index on `status`.
fn orders_description() -> Value {
    json!({
        "Table": {
            "TableName": "Orders",
            "TableStatus": "ACTIVE",
            "KeySchema": [
                {"AttributeName": "customer", "KeyType": "HASH"},
                {"AttributeName": "ts", "KeyType": "RANGE"}
            ],
            "AttributeDefinitions": [
                {"AttributeName": "customer", "AttributeType": "S"},
                {"AttributeName": "ts", "AttributeType": "N"},
                {"AttributeName": "status", "AttributeType": "S"}
            ],
            "BillingModeSummary": {"BillingMode": "PAY_PER_REQUEST"},
            "ItemCount": 1234,
            "TableSizeBytes": 2048000,
            "GlobalSecondaryIndexes": [{
                "IndexName": "by_status",
                "KeySchema": [{"AttributeName": "status", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "IndexStatus": "ACTIVE"
            }],
            "TableArn": "arn:aws:dynamodb:us-east-1:123456789012:table/Orders"
        }
    })
}

/// DescribeTable → Orders, ListTables → the given names, everything else from
/// the test's closure.
fn with_orders(
    handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    move |request| match request.operation.as_str() {
        "DescribeTable" => Reply::ok(orders_description()),
        "ListTables" if request.body["Limit"] == 1 => Reply::ok(json!({"TableNames": ["Orders"]})),
        _ => handler(request),
    }
}

fn order(customer: &str, ts: i64, status: &str) -> Value {
    json!({
        "customer": {"S": customer},
        "ts": {"N": ts.to_string()},
        "status": {"S": status},
        "total": {"N": "12.50"},
    })
}

#[tokio::test]
async fn connecting_probes_the_account_and_signs_requests() {
    let server = FakeDynamo::start(|_| Reply::ok(json!({"TableNames": []}))).await;
    let client = server.client(None, &[]).await;
    assert_eq!(client.get_current_database(), "");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/");
    assert_eq!(requests[0].operation, "ListTables");
    assert_eq!(requests[0].body["Limit"], 1);
    assert_eq!(
        requests[0].header("x-amz-target"),
        Some("DynamoDB_20120810.ListTables")
    );
    let authorization = requests[0].header("authorization").expect("signed");
    assert!(
        authorization.starts_with(&format!("AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/")),
        "{authorization}"
    );
    assert!(
        authorization.contains("/dynamodb/aws4_request"),
        "{authorization}"
    );
    for request in &requests {
        for value in request.headers.values() {
            assert!(!value.contains(SECRET_KEY), "{value}");
        }
    }
}

#[tokio::test]
async fn connecting_to_a_table_describes_it() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({})))).await;
    let client = server.client(Some("Orders"), &[]).await;
    assert_eq!(client.get_current_database(), "Orders");
    assert_eq!(server.of("DescribeTable").len(), 1);
    assert_eq!(server.of("DescribeTable")[0].body["TableName"], "Orders");
}

#[tokio::test]
async fn an_unknown_table_fails_the_connection_with_a_hint() {
    let server = FakeDynamo::start(|_| {
        Reply::error(
            400,
            "ResourceNotFoundException",
            "Requested resource not found",
        )
    })
    .await;
    let error = DynamoClient::new(server.info(Some("Nope"), &[]))
        .await
        .err()
        .unwrap();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::ConnectionError(_)), "{text}");
    assert!(text.contains("cannot open table Nope"), "{text}");
    assert!(text.contains("ResourceNotFoundException"), "{text}");
    assert!(text.contains("`tables` lists them"), "{text}");
    assert!(text.contains("[request id REQ123]"), "{text}");
}

#[tokio::test]
async fn denied_probes_do_not_block_the_connection() {
    let server = FakeDynamo::start(|_| {
        Reply::error(
            400,
            "AccessDeniedException",
            "User is not authorized to perform: dynamodb:DescribeTable",
        )
    })
    .await;
    server.client(Some("Orders"), &[]).await;
    server.client(None, &[]).await;
}

#[tokio::test]
async fn bad_credentials_are_explained_and_the_secret_never_shown() {
    let server = FakeDynamo::start(|_| {
        Reply::error(
            400,
            "UnrecognizedClientException",
            &format!("The security token included in the request is invalid: {SECRET_KEY}"),
        )
    })
    .await;
    let error = DynamoClient::new(server.info(None, &[]))
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
    let server = FakeDynamo::start(|_| Reply::ok(json!({}))).await;
    let mut info = server.info(None, &[]);
    info.port = Some(port);
    let error = DynamoClient::new(info).await.err().unwrap();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::ConnectionError(_)), "{text}");
    assert!(!text.contains(SECRET_KEY), "{text}");
}

#[tokio::test]
async fn tables_pages_through_the_listing() {
    let server = FakeDynamo::start(|request| {
        if request.body["Limit"] == 1 {
            return Reply::ok(json!({"TableNames": ["a"]}));
        }
        match request.body["ExclusiveStartTableName"].as_str() {
            None => Reply::ok(
                json!({"TableNames": ["alpha", "beta"], "LastEvaluatedTableName": "beta"}),
            ),
            Some("beta") => Reply::ok(json!({"TableNames": ["gamma"]})),
            Some(other) => Reply::error(400, "ValidationException", other),
        }
    })
    .await;
    let client = server.client(None, &[]).await;
    let out = client.execute_limited("tables").await.unwrap();
    assert_eq!(out.result.columns, ["table"]);
    let names: Vec<String> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    assert_eq!(names, ["alpha", "beta", "gamma"]);
    assert!(!out.truncated);
    assert_eq!(server.of("ListTables").len(), 3, "the probe and two pages");

    let limited = client.execute_limited("tables --limit 2").await.unwrap();
    assert_eq!(limited.result.rows.len(), 2);
    assert!(limited.truncated);
    assert!(
        limited.warnings[0].contains("first 2 tables"),
        "{:?}",
        limited.warnings
    );

    let databases = client.list_databases().await.unwrap();
    assert_eq!(databases[0], ["Table"]);
    assert_eq!(databases.len(), 4);
}

#[tokio::test]
async fn describe_lists_keys_indexes_billing_and_size() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({})))).await;
    let client = server.client(Some("Orders"), &[]).await;
    let out = client.execute_limited("describe").await.unwrap();
    assert_eq!(out.result.columns, ["property", "value"]);
    let property = |name: &str| -> String {
        out.result
            .rows
            .iter()
            .find(|row| row[0].as_deref().is_some_and(|p| p.starts_with(name)))
            .and_then(|row| row[1].clone())
            .unwrap_or_else(|| panic!("no {name} in {:?}", out.result.rows))
    };
    assert_eq!(property("table"), "Orders");
    assert_eq!(property("status"), "ACTIVE");
    assert_eq!(property("keys"), "customer (partition), ts (sort)");
    assert_eq!(property("billing"), "on demand");
    assert_eq!(property("items"), "1234");
    assert_eq!(property("size"), "2.0 MiB");
    assert_eq!(
        property("global index by_status"),
        "status (partition); projects ALL; ACTIVE"
    );
    assert!(property("arn").contains("table/Orders"));
}

#[tokio::test]
async fn scan_pages_maps_items_and_reports_cost() {
    let server = FakeDynamo::start(with_orders(|request| {
        assert_eq!(request.operation, "Scan");
        assert_eq!(request.body["ReturnConsumedCapacity"], "TOTAL");
        match request.body.get("ExclusiveStartKey") {
            None => Reply::ok(json!({
                "Items": [order("c1", 1, "new"), order("c1", 2, "paid")],
                "LastEvaluatedKey": {"customer": {"S": "c1"}, "ts": {"N": "2"}},
                "ConsumedCapacity": {"TableName": "Orders", "CapacityUnits": 0.5}
            })),
            Some(_) => Reply::ok(json!({
                "Items": [order("c2", 3, "new")],
                "ConsumedCapacity": {"TableName": "Orders", "CapacityUnits": 0.5}
            })),
        }
    }))
    .await;
    let client = server.client(None, &[]).await;
    let out = client.execute_limited("scan Orders").await.unwrap();

    // the table's keys lead, the rest alphabetically
    assert_eq!(out.result.columns, ["customer", "ts", "status", "total"]);
    assert_eq!(out.result.rows.len(), 3);
    assert_eq!(text_of(&out, 0, 0), "c1");
    assert_eq!(text_of(&out, 2, 1), "3");
    assert_eq!(text_of(&out, 0, 3), "12.50", "numbers stay exact");
    assert!(!out.truncated);
    assert!(out.warnings[0].contains("scan reads items without a key condition"));
    assert!(out.warnings[0].contains("bounded to 1000 items"));
    assert_eq!(
        out.notices[0],
        "consumed 1 capacity units in 2 requests for 3 items"
    );
    let scans = server.of("Scan");
    assert_eq!(scans.len(), 2);
    assert_eq!(scans[0].body["TableName"], "Orders");
    assert_eq!(scans[0].body["Limit"], 1000);
    assert_eq!(scans[1].body["ExclusiveStartKey"]["customer"]["S"], "c1");
}

#[tokio::test]
async fn scan_limit_bounds_the_request_and_the_rows() {
    let server = FakeDynamo::start(with_orders(|request| {
        let asked = request.body["Limit"].as_u64().unwrap() as i64;
        let items: Vec<Value> = (0..asked).map(|i| order("c", i, "x")).collect();
        Reply::ok(json!({
            "Items": items,
            "LastEvaluatedKey": {"customer": {"S": "c"}, "ts": {"N": "9"}}
        }))
    }))
    .await;
    let client = server.client(Some("Orders"), &[]).await;
    let out = client.execute_limited("scan --limit 7").await.unwrap();
    assert_eq!(out.result.rows.len(), 7);
    assert!(out.truncated);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("stopped after 7 items"))
    );
    assert!(
        !out.warnings[0].contains("bounded to"),
        "the limit was chosen"
    );
    let scans = server.of("Scan");
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].body["Limit"], 7);
}

#[tokio::test]
async fn the_page_budget_bounds_a_sparse_table() {
    // every page returns one item and a continuation: only max_pages requests are made
    let server = FakeDynamo::start(with_orders(|_| {
        Reply::ok(json!({
            "Items": [order("c", 1, "x")],
            "LastEvaluatedKey": {"customer": {"S": "c"}, "ts": {"N": "1"}}
        }))
    }))
    .await;
    let client = server.client(Some("Orders"), &[("max_pages", "3")]).await;
    let out = client.execute_limited("scan").await.unwrap();
    assert_eq!(server.of("Scan").len(), 3);
    assert_eq!(out.result.rows.len(), 3);
    assert!(out.truncated);
}

#[tokio::test]
async fn max_rows_caps_every_command() {
    let server = FakeDynamo::start(with_orders(|request| {
        let asked = request.body["Limit"].as_u64().unwrap() as i64;
        let items: Vec<Value> = (0..asked).map(|i| order("c", i, "x")).collect();
        Reply::ok(json!({"Items": items, "NextToken": "more"}))
    }))
    .await;
    let client = server.client(Some("Orders"), &[("max_rows", "5")]).await;
    let out = client
        .execute_limited("scan Orders --limit 500")
        .await
        .unwrap();
    assert_eq!(out.result.rows.len(), 5);
    let out = client
        .execute_limited("SELECT * FROM \"Orders\"")
        .await
        .unwrap();
    assert_eq!(out.result.rows.len(), 5);
    assert!(out.truncated);
    assert_eq!(server.of("ExecuteStatement")[0].body["Limit"], 5);
}

#[tokio::test]
async fn get_converts_the_key_to_its_type_and_maps_the_item() {
    let server = FakeDynamo::start(with_orders(|request| {
        assert_eq!(request.operation, "GetItem");
        assert_eq!(request.body["TableName"], "Orders");
        Reply::ok(json!({
            "Item": {
                "customer": {"S": "c1"},
                "ts": {"N": "5"},
                "tags": {"SS": ["a", "b"]},
                "address": {"M": {"zip": {"S": "75001"}, "geo": {"L": [{"N": "1.5"}, {"N": "2"}]}}},
                "blob": {"B": "AP8="},
                "active": {"BOOL": true},
                "nothing": {"NULL": true}
            },
            "ConsumedCapacity": {"TableName": "Orders", "CapacityUnits": 0.5}
        }))
    }))
    .await;
    let client = server.client(None, &[]).await;
    let out = client
        .execute_limited("get Orders customer=c1 ts=5")
        .await
        .unwrap();
    let key = &server.of("GetItem")[0].body["Key"];
    assert_eq!(key["customer"], json!({"S": "c1"}));
    assert_eq!(key["ts"], json!({"N": "5"}), "ts is a number key");

    assert_eq!(out.result.rows.len(), 1);
    assert_eq!(&out.result.columns[..2], ["customer", "ts"]);
    assert_eq!(text_of(&out, 0, column_of(&out, "tags")), r#"["a","b"]"#);
    assert_eq!(
        text_of(&out, 0, column_of(&out, "address")),
        r#"{"geo":[1.5,2],"zip":"75001"}"#
    );
    assert_eq!(text_of(&out, 0, column_of(&out, "blob")), "AP8=");
    assert_eq!(text_of(&out, 0, column_of(&out, "active")), "true");
    assert_eq!(out.result.rows[0][column_of(&out, "nothing")], None);
    assert_eq!(
        out.notices[0],
        "consumed 0.5 capacity units in 1 request for 1 item"
    );
}

#[tokio::test]
async fn get_without_a_match_says_so() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({})))).await;
    let client = server.client(Some("Orders"), &[]).await;
    let out = client
        .execute_limited("get customer=c9 ts=1")
        .await
        .unwrap();
    assert!(out.result.rows.is_empty());
    assert_eq!(out.result.columns, ["customer", "ts"]);
    assert!(out.notices.iter().any(|n| n == "no item has that key"));
}

#[rstest]
#[case("get Orders customer=c1", "also has the sort key ts")]
#[case("get Orders ts=1", "give the partition key customer")]
#[case("get Orders customer=c1 ts=abc", "key ts is a number")]
#[case("get Orders customer=c1 other=1", "not a key of table Orders")]
#[tokio::test]
async fn get_refuses_a_key_that_does_not_fit_the_table(#[case] line: &str, #[case] expected: &str) {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({})))).await;
    let client = server.client(None, &[]).await;
    let error = client.execute_limited(line).await.unwrap_err().to_string();
    assert!(error.contains(expected), "{line}: {error}");
    assert!(server.of("GetItem").is_empty(), "nothing was read");
}

#[tokio::test]
async fn query_builds_the_key_condition_and_pages() {
    let server = FakeDynamo::start(with_orders(|request| {
        match request.body.get("ExclusiveStartKey") {
            None => Reply::ok(json!({
                "Items": [order("c1", 1, "new")],
                "LastEvaluatedKey": {"customer": {"S": "c1"}, "ts": {"N": "1"}}
            })),
            Some(_) => Reply::ok(json!({"Items": [order("c1", 2, "paid")]})),
        }
    }))
    .await;
    let client = server.client(None, &[]).await;
    let out = client
        .execute_limited("query Orders customer=c1 --desc")
        .await
        .unwrap();
    assert_eq!(out.result.rows.len(), 2);
    let queries = server.of("Query");
    assert_eq!(queries.len(), 2);
    let body = &queries[0].body;
    assert_eq!(body["KeyConditionExpression"], "#p = :p");
    assert_eq!(body["ExpressionAttributeNames"]["#p"], "customer");
    assert_eq!(body["ExpressionAttributeValues"][":p"], json!({"S": "c1"}));
    assert_eq!(body["ScanIndexForward"], false);
    assert!(body.get("IndexName").is_none());
    assert!(
        out.warnings.is_empty(),
        "a query is not a scan: {:?}",
        out.warnings
    );
}

#[tokio::test]
async fn query_supports_sort_key_prefix_and_equality_and_indexes() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({"Items": []})))).await;
    let client = server.client(Some("Orders"), &[]).await;

    client
        .execute_limited("query customer=c1 ts=7")
        .await
        .unwrap();
    let error = client
        .execute_limited("query customer=c1 ts^=17")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ts is a number"), "{error}");
    client
        .execute_limited("query status=paid --index by_status -n 3")
        .await
        .unwrap();
    let queries = server.of("Query");
    assert_eq!(
        queries[0].body["KeyConditionExpression"],
        "#p = :p AND #s = :s"
    );
    assert_eq!(
        queries[0].body["ExpressionAttributeValues"][":s"],
        json!({"N": "7"})
    );
    assert_eq!(queries[1].body["IndexName"], "by_status");
    assert_eq!(queries[1].body["ExpressionAttributeNames"]["#p"], "status");
    assert_eq!(queries[1].body["Limit"], 3);

    let error = client
        .execute_limited("query customer=c1 --index nope")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no index 'nope'") && error.contains("by_status"),
        "{error}"
    );
    let error = client
        .execute_limited("query ts=1")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("give the partition key customer"), "{error}");
}

#[tokio::test]
async fn query_begins_with_on_a_string_sort_key() {
    let server = FakeDynamo::start(|request| match request.operation.as_str() {
        "DescribeTable" => {
            let mut description = orders_description();
            description["Table"]["AttributeDefinitions"][1]["AttributeType"] = json!("S");
            Reply::ok(description)
        }
        _ => Reply::ok(json!({"Items": []})),
    })
    .await;
    let client = server.client(Some("Orders"), &[]).await;
    client
        .execute_limited("query customer=c1 ts^=2026-10")
        .await
        .unwrap();
    let body = &server.of("Query")[0].body;
    assert_eq!(
        body["KeyConditionExpression"],
        "#p = :p AND begins_with(#s, :s)"
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":s"],
        json!({"S": "2026-10"})
    );
}

#[tokio::test]
async fn partiql_runs_a_select_pages_and_leads_with_the_keys() {
    let server = FakeDynamo::start(with_orders(|request| {
        assert_eq!(request.operation, "ExecuteStatement");
        match request.body.get("NextToken").and_then(Value::as_str) {
            None => Reply::ok(json!({
                "Items": [order("c1", 1, "new")],
                "NextToken": "tok-1",
                "ConsumedCapacity": {"TableName": "Orders", "CapacityUnits": 1.0}
            })),
            Some("tok-1") => Reply::ok(json!({
                "Items": [order("c2", 2, "paid")],
                "ConsumedCapacity": {"TableName": "Orders", "CapacityUnits": 1.0}
            })),
            Some(other) => Reply::error(400, "ValidationException", other),
        }
    }))
    .await;
    let client = server.client(None, &[]).await;
    let sql = "SELECT * FROM \"Orders\" WHERE status = 'new';";
    let out = client.execute_limited(sql).await.unwrap();
    assert_eq!(out.result.columns, ["customer", "ts", "status", "total"]);
    assert_eq!(out.result.rows.len(), 2);
    assert_eq!(
        out.notices[0],
        "consumed 2 capacity units in 2 requests for 2 items"
    );
    let statements = server.of("ExecuteStatement");
    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[0].body["Statement"], "SELECT * FROM \"Orders\" WHERE status = 'new'",
        "the trailing ; is not sent"
    );
    assert_eq!(statements[0].body["Limit"], 1000);
    assert_eq!(statements[0].body["ReturnConsumedCapacity"], "TOTAL");
    assert_eq!(statements[1].body["NextToken"], "tok-1");

    // the structured result keeps NULL apart from empty
    let structured = client.execute_query_structured(sql).await.unwrap();
    assert_eq!(structured.columns[0], "customer");
}

#[tokio::test]
async fn consistent_reads_are_opt_in() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({"Items": []})))).await;
    let client = server
        .client(Some("Orders"), &[("consistent_read", "true")])
        .await;
    client.execute_limited("scan").await.unwrap();
    client.execute_limited("query customer=c").await.unwrap();
    client
        .execute_limited("SELECT * FROM \"Orders\"")
        .await
        .unwrap();
    for operation in ["Scan", "Query", "ExecuteStatement"] {
        assert_eq!(
            server.of(operation)[0].body["ConsistentRead"],
            true,
            "{operation}"
        );
    }
    let plain = server.client(Some("Orders"), &[]).await;
    plain.execute_limited("scan").await.unwrap();
    assert!(server.of("Scan")[1].body.get("ConsistentRead").is_none());
}

#[rstest]
#[case("INSERT INTO \"Orders\" VALUE {'customer': 'x'}")]
#[case("UPDATE \"Orders\" SET a = 1 WHERE customer = 'x'")]
#[case("DELETE FROM \"Orders\" WHERE customer = 'x'")]
#[case("SELECT * FROM \"Orders\"; DELETE FROM \"Orders\" WHERE customer = 'x'")]
#[case("put Orders customer=x")]
#[tokio::test]
async fn writes_are_refused_before_anything_is_sent(#[case] line: &str) {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({"Items": []})))).await;
    let client = server.client(Some("Orders"), &[]).await;
    let before = server.count();
    let error = client.execute_limited(line).await.unwrap_err().to_string();
    assert!(
        error.contains("only reads") || error.contains("one statement at a time"),
        "{error}"
    );
    assert!(client.test_query(line).await.is_err());
    assert_eq!(server.count(), before, "nothing reached the endpoint");
}

#[tokio::test]
async fn only_read_operations_are_ever_sent() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({"Items": []})))).await;
    let client = server.client(Some("Orders"), &[]).await;
    for line in [
        "tables",
        "describe",
        "scan --limit 2",
        "get customer=a ts=1",
        "query customer=a",
        "SELECT * FROM \"Orders\"",
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
        .get_table_details("Orders", None)
        .await
        .unwrap();
    const READS: [&str; 6] = [
        "ListTables",
        "DescribeTable",
        "Scan",
        "GetItem",
        "Query",
        "ExecuteStatement",
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

#[rstest]
#[case(
    "ProvisionedThroughputExceededException",
    "The level of configured provisioned throughput for the table was exceeded",
    "throttled"
)]
#[case(
    "ValidationException",
    "Statement wasn't well formed",
    "check the PartiQL"
)]
#[case(
    "AccessDeniedException",
    "User is not authorized to perform: dynamodb:Scan",
    "PartiQLSelect"
)]
#[case(
    "ResourceNotFoundException",
    "Requested resource not found",
    "no such table"
)]
#[tokio::test]
async fn service_errors_carry_the_code_a_hint_and_the_request_id(
    #[case] kind: &str,
    #[case] message: &str,
    #[case] hint: &str,
) {
    let (kind, message) = (kind.to_string(), message.to_string());
    let server = FakeDynamo::start(with_orders(move |_| Reply::error(400, &kind, &message))).await;
    let client = server.client(Some("Orders"), &[]).await;
    let error = client.execute_limited("scan").await.unwrap_err();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{text}");
    assert!(text.contains(hint), "{text}");
    assert!(text.contains("[request id REQ123]"), "{text}");
}

#[tokio::test]
async fn ctrl_c_interrupts_a_slow_request() {
    let server = FakeDynamo::start(with_orders(|_| {
        let mut reply = Reply::ok(json!({"Items": []}));
        reply.delay = Some(Duration::from_secs(30));
        reply
    }))
    .await;
    let client = server.client(Some("Orders"), &[]).await;
    let flag = Arc::new(AtomicBool::new(false));
    set_thread_interrupt_flag(Some(flag.clone()));
    let trigger = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.store(true, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let error = client.execute_limited("scan").await.unwrap_err();
    set_thread_interrupt_flag(None);
    assert!(error.to_string().contains("interrupted"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn metadata_names_the_keys_and_indexes() {
    let server = FakeDynamo::start(with_orders(|_| {
        Reply::ok(json!({"TableNames": ["Orders", "Users"]}))
    }))
    .await;
    let client = server.client(Some("Orders"), &[]).await;
    let provider = client.get_metadata_provider();

    assert_eq!(provider.get_schemas().await.unwrap(), ["us-east-1"]);
    assert_eq!(
        provider.get_tables(None).await.unwrap(),
        ["Orders", "Users"]
    );
    assert_eq!(
        provider.get_columns("Orders", None).await.unwrap(),
        ["customer", "ts", "status"]
    );
    let functions = provider.get_functions(None).await.unwrap();
    assert!(functions.contains(&"scan".to_string()) && functions.contains(&"SELECT".to_string()));
    assert!(!provider.supports_explain());
    assert_eq!(provider.default_schema().as_deref(), Some("Orders"));

    let details = provider.get_table_details("Orders", None).await.unwrap();
    assert_eq!(details.name, "Orders");
    let columns: Vec<_> = details
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.data_type.as_str()))
        .collect();
    assert_eq!(
        columns,
        [
            ("customer", "string (partition key)"),
            ("ts", "number (sort key)"),
            ("status", "string (partition key of by_status)"),
        ]
    );
    assert_eq!(details.indexes.len(), 2);
    assert!(details.indexes[0].is_primary);
    assert_eq!(
        details.indexes[0].definition,
        "customer (partition), ts (sort)"
    );
    assert_eq!(details.indexes[1].name, "by_status");
}

#[tokio::test]
async fn switching_tables_checks_the_new_one() {
    let server = FakeDynamo::start(|request| match request.body["TableName"].as_str() {
        Some("Orders") => Reply::ok(orders_description()),
        Some(_) => Reply::error(
            400,
            "ResourceNotFoundException",
            "Requested resource not found",
        ),
        None => Reply::ok(json!({"TableNames": []})),
    })
    .await;
    let mut client = server.client(None, &[]).await;
    client.connect_to_database("Orders").await.unwrap();
    assert_eq!(client.get_current_database(), "Orders");
    assert_eq!(
        client.get_connection_info().database.as_deref(),
        Some("Orders")
    );
    let error = client.connect_to_database("Gone").await.unwrap_err();
    assert!(
        error.to_string().contains("cannot open table Gone"),
        "{error}"
    );
    assert_eq!(
        client.get_current_database(),
        "Orders",
        "a failed switch keeps the table"
    );
    assert!(client.connect_to_database("a/b").await.is_err());
    assert!(client.connect_to_database("").await.is_err());
}

#[tokio::test]
async fn commands_without_a_table_say_how_to_pick_one() {
    let server = FakeDynamo::start(|_| Reply::ok(json!({"TableNames": []}))).await;
    let client = server.client(None, &[]).await;
    for line in ["scan", "describe", "get a=1", "query a=1"] {
        let error = client.execute_limited(line).await.unwrap_err().to_string();
        assert!(error.contains("no table selected"), "{line}: {error}");
    }
}

#[tokio::test]
async fn is_connected_and_server_info_never_show_the_secret() {
    let server = FakeDynamo::start(with_orders(|_| Reply::ok(json!({})))).await;
    let client = server.client(Some("Orders"), &[]).await;
    assert!(client.is_connected().await);
    let info = client.get_server_info().await.unwrap();
    assert!(
        info.server_type
            .contains("DynamoDB-compatible service at http://127.0.0.1"),
        "{}",
        info.server_type
    );
    assert_eq!(info.server_version, "us-east-1");
    assert_eq!(info.additional_info["credentials"], "static access key");
    assert_eq!(info.additional_info["table"], "Orders");
    assert!(!format!("{info:?}").contains(SECRET_KEY));
    assert!(client.explain_query("scan").await.is_err());
}

#[tokio::test]
async fn help_lists_every_command() {
    let server = FakeDynamo::start(|_| Reply::ok(json!({"TableNames": []}))).await;
    let client = server.client(None, &[]).await;
    let out = client.execute_limited("help").await.unwrap();
    let commands: Vec<String> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    for verb in dbcrust_core::dynamodb_command::command_names() {
        assert!(commands.iter().any(|c| c.starts_with(verb)), "{verb}");
    }
    assert!(commands.iter().any(|c| c.starts_with("SELECT")));
}

/// Live check against DynamoDB Local (or any endpoint): set `DYNAMODB_TEST_URL`
/// to a connection URL, e.g.
/// `dynamodb://AKIAFAKE:fakesecret@us-east-1?endpoint=http://127.0.0.1:8000`.
/// The test creates a table of its own, reads it back and deletes it.
#[tokio::test]
async fn live_endpoint_round_trip() {
    use aws_sdk_dynamodb::types::{
        AttributeDefinition, AttributeValue, BillingMode, KeySchemaElement, KeyType,
        ScalarAttributeType,
    };
    let Ok(url) = std::env::var("DYNAMODB_TEST_URL") else {
        eprintln!("DYNAMODB_TEST_URL not set; skipping the live DynamoDB test");
        return;
    };
    let table = format!("dbcrust_it_orders_{}", std::process::id());
    let info = ConnectionInfo::parse_url(&url).expect("DYNAMODB_TEST_URL parses");

    // seed with the SDK, through the same settings
    let settings = AwsSettings::from_connection_info(&info).expect("settings");
    let mut sdk = dbcrust_aws::load_sdk_config(&settings).await.expect("sdk");
    if sdk.region().is_none() {
        sdk = sdk
            .into_builder()
            .region(Region::new(FALLBACK_REGION))
            .build();
    }
    let admin = Client::from_conf(aws_sdk_dynamodb::config::Builder::from(&sdk).build());
    let attribute = |name: &str, kind: ScalarAttributeType| {
        AttributeDefinition::builder()
            .attribute_name(name)
            .attribute_type(kind)
            .build()
            .unwrap()
    };
    let key = |name: &str, kind: KeyType| {
        KeySchemaElement::builder()
            .attribute_name(name)
            .key_type(kind)
            .build()
            .unwrap()
    };
    admin
        .create_table()
        .table_name(&table)
        .billing_mode(BillingMode::PayPerRequest)
        .attribute_definitions(attribute("customer", ScalarAttributeType::S))
        .attribute_definitions(attribute("ts", ScalarAttributeType::N))
        .key_schema(key("customer", KeyType::Hash))
        .key_schema(key("ts", KeyType::Range))
        .send()
        .await
        .expect("create table");
    for ts in 1..=5 {
        admin
            .put_item()
            .table_name(&table)
            .item("customer", AttributeValue::S("c1".into()))
            .item("ts", AttributeValue::N(ts.to_string()))
            .item("note", AttributeValue::S(format!("order {ts}")))
            .item("tags", AttributeValue::Ss(vec!["a".into(), "b".into()]))
            .send()
            .await
            .expect("put item");
    }

    let client = DynamoClient::new(info).await.expect("connects");
    let outcome = async {
        let tables = client.execute_limited("tables").await?;
        assert!(
            tables
                .result
                .rows
                .iter()
                .any(|r| r[0].as_deref() == Some(&table))
        );
        let described = client.execute_limited(&format!("describe {table}")).await?;
        assert!(
            described
                .result
                .rows
                .iter()
                .any(|r| r[0].as_deref() == Some("keys"))
        );
        let scan = client
            .execute_limited(&format!("scan {table} --limit 3"))
            .await?;
        assert_eq!(scan.result.rows.len(), 3);
        assert_eq!(scan.result.columns[..2], ["customer", "ts"]);
        let get = client
            .execute_limited(&format!("get {table} customer=c1 ts=2"))
            .await?;
        assert_eq!(get.result.rows.len(), 1);
        let query = client
            .execute_limited(&format!("query {table} customer=c1 ts^=1"))
            .await;
        // begins_with is not valid on a number sort key: the service says so
        assert!(query.is_err());
        let query = client
            .execute_limited(&format!("query {table} customer=c1 --desc -n 2"))
            .await?;
        assert_eq!(query.result.rows.len(), 2);
        assert_eq!(text_of(&query, 0, 1), "5");
        let partiql = client
            .execute_limited(&format!("SELECT * FROM \"{table}\" WHERE customer = 'c1'"))
            .await?;
        assert_eq!(partiql.result.rows.len(), 5);
        assert!(
            client
                .execute_limited("DELETE FROM x WHERE a = 1")
                .await
                .is_err()
        );
        Ok::<(), DatabaseError>(())
    }
    .await;
    admin.delete_table().table_name(&table).send().await.ok();
    outcome.expect("live round trip");
}
