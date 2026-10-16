//! The S3 client against an in-process HTTP server that speaks just enough of the
//! S3 REST API: the real SDK, the ring HTTP client and the request signing run
//! for real, nothing leaves the machine and no AWS account is needed.
//! A live run against MinIO is gated on `S3_TEST_URL`.

use super::*;
use dbcrust_core::database::{DatabaseClient, set_thread_interrupt_flag};
use rstest::rstest;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const ACCESS_KEY: &str = "AKIATESTKEY0123456AB";
const SECRET_KEY: &str = "test-secret-key-value/CANARY+12345";

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
}

impl Request {
    fn param(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then(|| percent_decode(value))
        })
    }

    fn has_param(&self, name: &str) -> bool {
        self.query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(name))
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Default)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: Option<Duration>,
}

impl Reply {
    fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            ..Default::default()
        }
    }

    fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            ..Default::default()
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn error(status: u16, code: &str, message: &str) -> Self {
        Self::status(
            status,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code>\
                 <Message>{message}</Message><RequestId>REQ123</RequestId></Error>"
            ),
        )
        .header("content-type", "application/xml")
        .header("x-amz-request-id", "REQ123")
    }
}

struct FakeS3 {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeS3 {
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
                        // one request head per iteration (keep-alive)
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
                        let target = parts.next().unwrap_or_default();
                        let (path, query) = target.split_once('?').unwrap_or((target, ""));
                        let headers = lines
                            .filter_map(|line| line.split_once(':'))
                            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                            .collect();
                        let request = Request {
                            method: method.clone(),
                            // the SDK addresses a bucket as `/b/`
                            path: match percent_decode(path) {
                                p if p.len() > 1 => p.trim_end_matches('/').to_string(),
                                p => p,
                            },
                            query: query.to_string(),
                            headers,
                        };
                        log.lock().unwrap().push(request.clone());
                        let reply = handler(&request);
                        if let Some(delay) = reply.delay {
                            tokio::time::sleep(delay).await;
                        }
                        let reason = match reply.status {
                            200 => "OK",
                            206 => "Partial Content",
                            301 => "Moved Permanently",
                            403 => "Forbidden",
                            404 => "Not Found",
                            416 => "Range Not Satisfiable",
                            _ => "Status",
                        };
                        let mut out = format!(
                            "HTTP/1.1 {} {reason}\r\nconnection: keep-alive\r\n",
                            reply.status
                        );
                        let mut has_length = false;
                        for (name, value) in &reply.headers {
                            has_length |= name.eq_ignore_ascii_case("content-length");
                            out.push_str(&format!("{name}: {value}\r\n"));
                        }
                        if !has_length {
                            out.push_str(&format!("content-length: {}\r\n", reply.body.len()));
                        }
                        out.push_str("\r\n");
                        let mut bytes = out.into_bytes();
                        if method != "HEAD" {
                            bytes.extend_from_slice(&reply.body);
                        }
                        if stream.write_all(&bytes).await.is_err() {
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

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn info(&self, bucket: Option<&str>, options: &[(&str, &str)]) -> ConnectionInfo {
        let mut info = ConnectionInfo {
            database_type: DatabaseType::S3,
            host: Some("127.0.0.1".to_string()),
            port: Some(self.addr.port()),
            use_tls: false,
            username: Some(ACCESS_KEY.to_string()),
            password: Some(SECRET_KEY.to_string()),
            database: bucket.map(str::to_string),
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

    async fn client(&self, bucket: Option<&str>, options: &[(&str, &str)]) -> S3Client {
        S3Client::new(self.info(bucket, options)).await.unwrap()
    }
}

fn list_xml(prefixes: &[&str], objects: &[(&str, i64)], next: Option<&str>) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Name>b</Name><Prefix></Prefix>",
    );
    xml.push_str(&format!(
        "<KeyCount>{}</KeyCount><MaxKeys>1000</MaxKeys>",
        prefixes.len() + objects.len()
    ));
    xml.push_str(&format!("<IsTruncated>{}</IsTruncated>", next.is_some()));
    if let Some(token) = next {
        xml.push_str(&format!(
            "<NextContinuationToken>{token}</NextContinuationToken>"
        ));
    }
    for (key, size) in objects {
        xml.push_str(&format!(
            "<Contents><Key>{key}</Key><LastModified>2026-01-02T03:04:05.000Z</LastModified>\
             <ETag>&quot;etag-{size}&quot;</ETag><Size>{size}</Size><StorageClass>STANDARD</StorageClass></Contents>"
        ));
    }
    for prefix in prefixes {
        xml.push_str(&format!(
            "<CommonPrefixes><Prefix>{prefix}</Prefix></CommonPrefixes>"
        ));
    }
    xml.push_str("</ListBucketResult>");
    xml
}

/// HEAD /bucket → 200; everything else is the test's business.
fn with_bucket(
    handler: impl Fn(&Request) -> Reply + Send + Sync + 'static,
) -> impl Fn(&Request) -> Reply + Send + Sync + 'static {
    move |request| {
        if request.method == "HEAD" && request.path == "/b" {
            return Reply::ok("");
        }
        handler(request)
    }
}

fn text_of(result: &LimitedResult, row: usize, column: usize) -> String {
    result.result.rows[row][column].clone().unwrap_or_default()
}

#[tokio::test]
async fn connecting_checks_the_bucket_path_style_and_signs_requests() {
    let server = FakeS3::start(|_| Reply::ok("")).await;
    let client = server.client(Some("b"), &[]).await;
    assert_eq!(client.get_current_database(), "b");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "HEAD");
    // path-style addressing: the bucket is in the path, the host stays the endpoint
    assert_eq!(requests[0].path, "/b");
    let authorization = requests[0].header("authorization").expect("signed");
    assert!(
        authorization.starts_with(&format!("AWS4-HMAC-SHA256 Credential={ACCESS_KEY}/")),
        "{authorization}"
    );
    // the secret key is never on the wire
    for request in &requests {
        for value in request.headers.values() {
            assert!(!value.contains(SECRET_KEY), "{value}");
        }
    }
}

#[tokio::test]
async fn anonymous_access_sends_unsigned_requests() {
    let server = FakeS3::start(with_bucket(|_| {
        Reply::ok(list_xml(&[], &[("a.txt", 1)], None))
    }))
    .await;
    let mut info = server.info(Some("b"), &[("anonymous", "true")]);
    info.username = None;
    info.password = None;
    let client = S3Client::new(info).await.unwrap();
    client.execute_limited("ls").await.unwrap();
    for request in server.requests() {
        assert!(request.header("authorization").is_none(), "{request:?}");
    }
}

#[tokio::test]
async fn a_denied_head_bucket_does_not_block_the_connection() {
    let server = FakeS3::start(|request| {
        if request.method == "HEAD" {
            Reply::status(403, "")
        } else {
            Reply::ok(list_xml(&[], &[("only.txt", 3)], None))
        }
    })
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("ls").await.unwrap();
    assert_eq!(text_of(&out, 0, 0), "only.txt");
}

#[tokio::test]
async fn an_unknown_bucket_fails_the_connection_with_the_s3_code() {
    let server = FakeS3::start(|_| Reply::status(404, "")).await;
    let error = S3Client::new(server.info(Some("nope"), &[]))
        .await
        .err()
        .unwrap();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::ConnectionError(_)), "{text}");
    assert!(text.contains("cannot open s3://nope"), "{text}");
}

#[tokio::test]
async fn bad_credentials_are_explained_and_the_secret_never_shown() {
    let server = FakeS3::start(|_| {
        Reply::error(
            403,
            "SignatureDoesNotMatch",
            &format!("The request signature does not match: {SECRET_KEY}"),
        )
    })
    .await;
    // no bucket: the preflight is a ListBuckets GET, whose error carries a document
    let error = S3Client::new(server.info(None, &[])).await.err().unwrap();
    let text = error.to_string();
    assert!(text.contains("SignatureDoesNotMatch"), "{text}");
    assert!(text.contains("secret access key is wrong"), "{text}");
    assert!(!text.contains(SECRET_KEY), "{text}");
    assert!(!format!("{error:?}").contains(SECRET_KEY));
}

#[tokio::test]
async fn a_denied_head_says_so_without_inventing_a_cause() {
    // HEAD answers have no body: only the status is known
    let server = FakeS3::start(|_| Reply::status(404, "")).await;
    let error = S3Client::new(server.info(Some("b"), &[]))
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("NotFound"), "{error}");
}

#[tokio::test]
async fn a_wrong_region_names_the_right_one() {
    let server = FakeS3::start(|_| {
        Reply::error(301, "PermanentRedirect", "The bucket you are attempting to access must be addressed using the specified endpoint")
            .header("x-amz-bucket-region", "eu-west-3")
    })
    .await;
    let error = S3Client::new(server.info(Some("b"), &[]))
        .await
        .err()
        .unwrap();
    let text = error.to_string();
    assert!(text.contains("?region=eu-west-3"), "{text}");
}

#[tokio::test]
async fn an_unreachable_endpoint_is_a_connection_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let server = FakeS3::start(|_| Reply::ok("")).await;
    let mut info = server.info(Some("b"), &[]);
    info.port = Some(port);
    let error = S3Client::new(info).await.err().unwrap();
    let text = error.to_string();
    assert!(matches!(error, DatabaseError::ConnectionError(_)), "{text}");
    assert!(!text.contains(SECRET_KEY), "{text}");
}

#[tokio::test]
async fn ls_pages_through_a_listing_and_maps_the_rows() {
    let server = FakeS3::start(with_bucket(|request| {
        match request.param("continuation-token").as_deref() {
            None => Reply::ok(list_xml(&["logs/"], &[("a.txt", 12)], Some("tok-1"))),
            Some("tok-1") => Reply::ok(list_xml(&[], &[("b.txt", 34), ("c d.txt", 5)], None)),
            Some(other) => Reply::error(400, "InvalidArgument", other),
        }
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("ls").await.unwrap();

    assert_eq!(
        out.result.columns,
        [
            "key",
            "type",
            "size",
            "last_modified",
            "storage_class",
            "etag"
        ]
    );
    let keys: Vec<_> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    assert_eq!(keys, ["logs/", "a.txt", "b.txt", "c d.txt"]);
    assert_eq!(text_of(&out, 0, 1), "dir");
    assert_eq!(out.result.rows[0][2], None, "a prefix has no size");
    assert_eq!(text_of(&out, 1, 1), "object");
    assert_eq!(text_of(&out, 1, 2), "12");
    assert_eq!(text_of(&out, 1, 3), "2026-01-02T03:04:05Z");
    assert_eq!(text_of(&out, 1, 4), "STANDARD");
    assert_eq!(text_of(&out, 1, 5), "etag-12", "the etag is unquoted");
    assert!(!out.truncated);

    let listings: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.has_param("list-type"))
        .collect();
    assert_eq!(listings.len(), 2, "two pages");
    assert_eq!(listings[0].path, "/b");
    assert_eq!(listings[0].param("list-type").as_deref(), Some("2"));
    assert_eq!(listings[0].param("delimiter").as_deref(), Some("/"));
    assert_eq!(listings[0].param("max-keys").as_deref(), Some("1000"));
    assert!(!listings[0].has_param("prefix"));
    assert_eq!(
        listings[1].param("continuation-token").as_deref(),
        Some("tok-1")
    );
}

#[tokio::test]
async fn ls_uses_the_url_prefix_the_argument_and_recursion() {
    let server = FakeS3::start(with_bucket(|_| Reply::ok(list_xml(&[], &[], None)))).await;
    let mut info = server.info(Some("b"), &[]);
    info.file_path = Some("logs/2026/".to_string());
    let client = S3Client::new(info).await.unwrap();

    client.execute_limited("ls").await.unwrap();
    client.execute_limited("ls other/ -r").await.unwrap();
    client.execute_limited("ls /leading/slash/").await.unwrap();
    let listings: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.has_param("list-type"))
        .collect();
    assert_eq!(listings[0].param("prefix").as_deref(), Some("logs/2026/"));
    assert_eq!(listings[0].param("delimiter").as_deref(), Some("/"));
    assert_eq!(listings[1].param("prefix").as_deref(), Some("other/"));
    assert!(
        !listings[1].has_param("delimiter"),
        "recursive has no delimiter"
    );
    assert_eq!(
        listings[2].param("prefix").as_deref(),
        Some("leading/slash/")
    );
}

#[tokio::test]
async fn the_row_limit_shrinks_the_page_and_reports_the_cut() {
    let server = FakeS3::start(with_bucket(|request| {
        let asked: usize = request.param("max-keys").unwrap().parse().unwrap();
        let objects: Vec<(String, i64)> = (0..asked).map(|i| (format!("k{i:03}"), 1)).collect();
        let refs: Vec<(&str, i64)> = objects.iter().map(|(k, s)| (k.as_str(), *s)).collect();
        Reply::ok(list_xml(&[], &refs, Some("more")))
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("ls -n 7").await.unwrap();
    assert_eq!(out.result.rows.len(), 7);
    assert!(out.truncated);
    assert!(
        out.notices[0].contains("listing stopped at 7 entries"),
        "{:?}",
        out.notices
    );
    let listings: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.has_param("list-type"))
        .collect();
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].param("max-keys").as_deref(), Some("7"));
}

#[tokio::test]
async fn the_page_budget_bounds_the_requests() {
    let server = FakeS3::start(with_bucket(|request| {
        let page: usize = request
            .param("continuation-token")
            .map_or(0, |t| t.trim_start_matches('p').parse().unwrap());
        Reply::ok(list_xml(
            &[],
            &[(&format!("p{page}.txt"), 1)],
            Some(&format!("p{}", page + 1)),
        ))
    }))
    .await;
    let client = server
        .client(Some("b"), &[("max_pages", "3"), ("max_rows", "100")])
        .await;
    let out = client.execute_limited("ls").await.unwrap();
    let listings = server
        .requests()
        .into_iter()
        .filter(|r| r.has_param("list-type"))
        .count();
    assert_eq!(listings, 3, "exactly max_pages requests");
    assert_eq!(out.result.rows.len(), 3);
    assert!(out.truncated);
}

#[tokio::test]
async fn max_rows_caps_even_an_explicit_limit() {
    let server = FakeS3::start(with_bucket(|request| {
        let asked: usize = request.param("max-keys").unwrap().parse().unwrap();
        let objects: Vec<(String, i64)> = (0..asked).map(|i| (format!("k{i}"), 1)).collect();
        let refs: Vec<(&str, i64)> = objects.iter().map(|(k, s)| (k.as_str(), *s)).collect();
        Reply::ok(list_xml(&[], &refs, None))
    }))
    .await;
    let client = server.client(Some("b"), &[("max_rows", "5")]).await;
    let out = client.execute_limited("ls -n 500").await.unwrap();
    assert_eq!(out.result.rows.len(), 5);
}

fn object_reply(body: &[u8], range: Option<&str>, total: usize) -> Reply {
    let mut reply = Reply::status(if range.is_some() { 206 } else { 200 }, body.to_vec())
        .header("content-type", "application/octet-stream")
        .header("etag", "\"abc\"");
    if let Some(range) = range {
        reply = reply.header("content-range", &format!("bytes {range}/{total}"));
    }
    reply
}

#[tokio::test]
async fn get_without_a_range_reads_the_first_chunk_as_text_lines() {
    let server = FakeS3::start(with_bucket(|request| {
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/b/dir/file.txt");
        object_reply(b"first\nsecond\nthird", Some("0-17"), 100_000)
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("get dir/file.txt").await.unwrap();

    assert_eq!(out.result.columns, ["line", "text"]);
    assert_eq!(out.result.rows.len(), 3);
    assert_eq!(text_of(&out, 1, 1), "second");
    assert!(out.truncated, "100000 bytes exist, 18 were read");
    assert!(
        out.notices[0].contains("shown from byte 0"),
        "{:?}",
        out.notices
    );
    // always bounded: the first 64 KiB, never an unbounded GetObject
    let get = server
        .requests()
        .into_iter()
        .find(|r| r.method == "GET")
        .unwrap();
    assert_eq!(get.header("range"), Some("bytes=0-65535"));
}

#[rstest]
#[case("get k bytes=100-199", "bytes=100-199")]
#[case("get k bytes=-5", "bytes=-5")]
#[case("get k --bytes 10", "bytes=0-9")]
#[case("get k bytes=7-", "bytes=7-1048582")]
#[tokio::test]
async fn get_sends_the_requested_range_clamped_to_the_cap(
    #[case] command: &str,
    #[case] expected: &str,
) {
    let server = FakeS3::start(with_bucket(|_| object_reply(b"xyz", Some("0-2"), 3))).await;
    let client = server.client(Some("b"), &[]).await;
    client.execute_limited(command).await.unwrap();
    let get = server
        .requests()
        .into_iter()
        .find(|r| r.method == "GET")
        .unwrap();
    assert_eq!(get.header("range"), Some(expected));
}

#[tokio::test]
async fn a_range_over_the_cap_is_refused_before_any_request() {
    let server = FakeS3::start(with_bucket(|_| Reply::ok(""))).await;
    let client = server.client(Some("b"), &[("max_get_bytes", "1000")]).await;
    let before = server.count();
    for command in [
        "get k --bytes 1001",
        "get k bytes=0-1000",
        "get k bytes=-5000",
    ] {
        let error = client.execute_limited(command).await.unwrap_err();
        let text = error.to_string();
        assert!(text.contains("capped at"), "{command}: {text}");
        assert!(text.contains("max_get_bytes"), "{text}");
    }
    assert_eq!(server.count(), before, "nothing was requested");
}

#[tokio::test]
async fn a_server_that_ignores_the_range_never_gets_more_than_the_cap_read() {
    let big = vec![b'a'; 200_000];
    let server = FakeS3::start(with_bucket(move |_| Reply::ok(big.clone()))).await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("get k --bytes 100").await.unwrap();
    let shown: usize = out
        .result
        .rows
        .iter()
        .map(|row| row[1].as_ref().unwrap().len())
        .sum();
    assert_eq!(shown, 100, "reading stopped at the requested 100 bytes");
    assert!(out.truncated);
}

#[tokio::test]
async fn binary_content_is_a_hex_dump_and_base64_on_request() {
    let server = FakeS3::start(with_bucket(|_| {
        object_reply(&[0x89, b'P', b'N', b'G', 0, 1, 2, 3], Some("0-7"), 8)
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("get img.png").await.unwrap();
    assert_eq!(out.result.columns, ["offset", "hex", "ascii"]);
    assert_eq!(text_of(&out, 0, 1), "89 50 4e 47 00 01 02 03");
    assert_eq!(text_of(&out, 0, 2), ".PNG....");
    assert!(out.notices[0].contains("binary"), "{:?}", out.notices);
    assert!(!out.truncated, "the whole 8-byte object was shown");

    let b64 = client
        .execute_limited("get img.png --base64")
        .await
        .unwrap();
    assert_eq!(b64.result.columns, ["offset", "base64"]);
    assert_eq!(text_of(&b64, 0, 1), "iVBORwABAgM=");
}

#[tokio::test]
async fn a_ranged_binary_read_reports_its_offset() {
    let server = FakeS3::start(with_bucket(|_| {
        object_reply(&[0xff; 4], Some("4096-4099"), 10_000)
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client
        .execute_limited("get k bytes=4096-4099")
        .await
        .unwrap();
    assert_eq!(text_of(&out, 0, 0), "00001000");
    assert!(out.truncated);
}

#[tokio::test]
async fn a_character_cut_by_the_range_is_not_binary() {
    let server = FakeS3::start(with_bucket(|_| {
        object_reply(&"héllo".as_bytes()[..2], Some("0-1"), 6)
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("get k --bytes 2").await.unwrap();
    assert_eq!(out.result.columns, ["line", "text"]);
    assert_eq!(text_of(&out, 0, 1), "h");
}

#[tokio::test]
async fn an_empty_object_is_reported_not_an_error() {
    let server = FakeS3::start(with_bucket(|_| {
        Reply::error(
            416,
            "InvalidRange",
            "The requested range is not satisfiable",
        )
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("get empty.txt").await.unwrap();
    assert!(out.result.rows.is_empty());
    assert!(out.notices[0].contains("is empty"), "{:?}", out.notices);
}

#[tokio::test]
async fn a_missing_key_is_explained() {
    let server = FakeS3::start(with_bucket(|_| {
        Reply::error(404, "NoSuchKey", "The specified key does not exist.")
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let error = client.execute_limited("get nope.txt").await.unwrap_err();
    let text = error.to_string();
    assert!(text.contains("NoSuchKey"), "{text}");
    assert!(text.contains("`ls` shows what is there"), "{text}");
    assert!(text.contains("request id REQ123"), "{text}");
}

#[tokio::test]
async fn head_shows_the_object_attributes_and_user_metadata() {
    let server = FakeS3::start(|request| {
        if request.method == "HEAD" && request.path == "/b/data/report.csv" {
            Reply::status(200, "")
                .header("content-length", "2048")
                .header("content-type", "text/csv")
                .header("etag", "\"deadbeef\"")
                .header("last-modified", "Sat, 02 Jan 2027 03:04:05 GMT")
                .header("x-amz-storage-class", "STANDARD_IA")
                .header("x-amz-server-side-encryption", "AES256")
                .header("x-amz-version-id", "v1")
                .header("x-amz-meta-owner", "data-team")
        } else {
            Reply::ok("")
        }
    })
    .await;
    let client = server.client(Some("b"), &[]).await;
    let out = client
        .execute_limited("head data/report.csv")
        .await
        .unwrap();
    assert_eq!(out.result.columns, ["attribute", "value"]);
    let attributes: HashMap<String, String> = out
        .result
        .rows
        .iter()
        .map(|row| (row[0].clone().unwrap(), row[1].clone().unwrap()))
        .collect();
    assert_eq!(attributes["bucket"], "b");
    assert_eq!(attributes["key"], "data/report.csv");
    assert_eq!(attributes["size"], "2048");
    assert_eq!(attributes["content_type"], "text/csv");
    assert_eq!(attributes["etag"], "deadbeef");
    assert_eq!(attributes["storage_class"], "STANDARD_IA");
    assert_eq!(attributes["server_side_encryption"], "AES256");
    assert_eq!(attributes["version_id"], "v1");
    assert_eq!(attributes["metadata.owner"], "data-team");
    assert_eq!(attributes["last_modified"], "2027-01-02T03:04:05Z");
}

#[tokio::test]
async fn buckets_lists_the_account() {
    let server = FakeS3::start(|request| {
        if request.path == "/" {
            Reply::ok(
                "<?xml version=\"1.0\"?><ListAllMyBucketsResult><Owner><ID>o</ID></Owner><Buckets>\
                 <Bucket><Name>alpha</Name><CreationDate>2025-05-06T07:08:09.000Z</CreationDate></Bucket>\
                 <Bucket><Name>beta</Name><CreationDate>2026-01-01T00:00:00.000Z</CreationDate></Bucket>\
                 </Buckets></ListAllMyBucketsResult>",
            )
        } else {
            Reply::ok("")
        }
    })
    .await;
    let client = server.client(None, &[]).await;
    let out = client.execute_limited("buckets").await.unwrap();
    assert_eq!(out.result.columns, ["bucket", "created", "region"]);
    assert_eq!(text_of(&out, 0, 0), "alpha");
    assert_eq!(text_of(&out, 0, 1), "2025-05-06T07:08:09Z");
    assert_eq!(text_of(&out, 1, 0), "beta");

    let rows = client.list_databases().await.unwrap();
    assert_eq!(rows[0], ["Bucket", "Created", "Region"]);
    assert_eq!(rows[1][0], "alpha");
    let schemas = client.get_metadata_provider().get_schemas().await.unwrap();
    assert_eq!(schemas, ["alpha", "beta"]);
}

#[tokio::test]
async fn without_a_bucket_object_commands_explain_what_to_do() {
    let server =
        FakeS3::start(|_| Reply::ok("<ListAllMyBucketsResult><Buckets/></ListAllMyBucketsResult>"))
            .await;
    let client = server.client(None, &[]).await;
    for command in ["ls", "head k", "get k", "cd x"] {
        let error = client.execute_limited(command).await.unwrap_err();
        assert!(
            error.to_string().contains("no bucket selected"),
            "{command}: {error}"
        );
    }
}

#[tokio::test]
async fn cd_pwd_and_the_default_prefix_follow_each_other() {
    let server = FakeS3::start(with_bucket(|_| Reply::ok(list_xml(&[], &[], None)))).await;
    let client = server.client(Some("b"), &[]).await;
    assert_eq!(
        text_of(&client.execute_limited("pwd").await.unwrap(), 0, 0),
        "s3://b"
    );
    assert_eq!(
        text_of(&client.execute_limited("cd logs/2026").await.unwrap(), 0, 0),
        "s3://b/logs/2026/"
    );
    client.execute_limited("ls").await.unwrap();
    assert_eq!(
        text_of(&client.execute_limited("cd ..").await.unwrap(), 0, 0),
        "s3://b/logs/"
    );
    assert_eq!(
        text_of(&client.execute_limited("cd /").await.unwrap(), 0, 0),
        "s3://b"
    );
    let listing = server
        .requests()
        .into_iter()
        .rfind(|r| r.has_param("list-type"))
        .unwrap();
    assert_eq!(listing.param("prefix").as_deref(), Some("logs/2026/"));
}

#[tokio::test]
async fn help_lists_every_verb() {
    let server = FakeS3::start(|_| Reply::ok("")).await;
    let client = server.client(Some("b"), &[]).await;
    let out = client.execute_limited("help").await.unwrap();
    let verbs: Vec<_> = (0..out.result.rows.len())
        .map(|i| text_of(&out, i, 0))
        .collect();
    assert_eq!(verbs, dbcrust_core::s3_command::command_names());
}

#[rstest]
#[case("put a.txt")]
#[case("rm a.txt")]
#[case("DELETE FROM t")]
#[case("")]
#[tokio::test]
async fn nothing_that_writes_can_run(#[case] command: &str) {
    let server = FakeS3::start(|_| Reply::ok("")).await;
    let client = server.client(Some("b"), &[]).await;
    let before = server.count();
    let error = client.execute_query(command).await.unwrap_err();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{error}");
    assert_eq!(server.count(), before);
    assert!(client.test_query(command).await.is_err());
    // and the read-only guard agrees with the parser on every line
    assert!(dbcrust_core::safety::check_read_only(command, &DatabaseType::S3).is_err());
}

#[tokio::test]
async fn only_reads_reach_the_wire() {
    let server = FakeS3::start(with_bucket(|request| match request.method.as_str() {
        "GET" if request.has_param("list-type") => Reply::ok(list_xml(&["p/"], &[("a", 1)], None)),
        "GET" if request.path == "/" => {
            Reply::ok("<ListAllMyBucketsResult><Buckets/></ListAllMyBucketsResult>")
        }
        "GET" => object_reply(b"x", Some("0-0"), 1),
        _ => Reply::ok(""),
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    for command in [
        "ls",
        "ls -r",
        "head a",
        "get a",
        "get a bytes=0-0",
        "buckets",
        "pwd",
        "cd p",
        "help",
    ] {
        let _ = client.execute_limited(command).await;
    }
    for request in server.requests() {
        assert!(
            matches!(request.method.as_str(), "GET" | "HEAD"),
            "a write verb reached the server: {request:?}"
        );
    }
}

#[tokio::test]
async fn metadata_browses_the_current_prefix() {
    let server = FakeS3::start(with_bucket(|request| {
        if request.has_param("list-type") {
            Reply::ok(list_xml(&["logs/", "tmp/"], &[("readme.md", 10)], None))
        } else if request.method == "HEAD" {
            Reply::status(200, "")
                .header("content-length", "10")
                .header("content-type", "text/markdown")
        } else {
            Reply::ok("")
        }
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let provider = client.get_metadata_provider();

    let tables = provider.get_tables(None).await.unwrap();
    assert_eq!(tables, ["logs/", "tmp/", "readme.md"]);
    // naming the connected bucket as the schema lists the same place; after a
    // `cd` both follow the new prefix, and another bucket is listed at its root
    client.execute_limited("cd logs").await.unwrap();
    provider.get_tables(Some("b")).await.unwrap();
    provider.get_tables(None).await.unwrap();
    provider.get_tables(Some("elsewhere")).await.unwrap();
    let prefixes: Vec<(String, Option<String>)> = server
        .requests()
        .into_iter()
        .filter(|r| r.has_param("list-type"))
        .map(|r| (r.path.clone(), r.param("prefix")))
        .collect();
    assert_eq!(
        prefixes[1..],
        [
            ("/b".to_string(), Some("logs/".to_string())),
            ("/b".to_string(), Some("logs/".to_string())),
            ("/elsewhere".to_string(), None),
        ]
    );
    client.execute_limited("cd /").await.unwrap();
    let kinds = provider.get_table_kinds(&tables).await.unwrap();
    assert_eq!(kinds["logs/"], "dir");
    assert_eq!(kinds["readme.md"], "object");

    let columns = provider.get_columns("logs/", None).await.unwrap();
    assert_eq!(columns[0], "key");
    let attributes = provider.get_columns("readme.md", None).await.unwrap();
    assert!(
        attributes.contains(&"content_type".to_string()),
        "{attributes:?}"
    );

    let details = provider.get_table_details("readme.md", None).await.unwrap();
    assert_eq!(details.schema, "b");
    assert!(
        details
            .columns
            .iter()
            .any(|c| c.name == "content_type" && c.data_type == "text/markdown")
    );
    let dir = provider.get_table_details("logs/", None).await.unwrap();
    assert_eq!(dir.columns[0].name, "type");
    assert_eq!(dir.columns[0].data_type, "prefix");

    assert_eq!(
        provider.get_functions(None).await.unwrap(),
        dbcrust_core::s3_command::command_names()
    );
    assert_eq!(provider.default_schema().as_deref(), Some("b"));
    assert!(!provider.supports_explain());
}

#[tokio::test]
async fn schemas_fall_back_to_the_connected_bucket_when_listing_is_denied() {
    let server = FakeS3::start(|request| {
        if request.path == "/" {
            Reply::error(403, "AccessDenied", "Access Denied")
        } else {
            Reply::ok("")
        }
    })
    .await;
    let client = server.client(Some("b"), &[]).await;
    assert_eq!(
        client.get_metadata_provider().get_schemas().await.unwrap(),
        ["b"]
    );
}

#[tokio::test]
async fn switching_buckets_checks_the_new_one() {
    let server = FakeS3::start(|request| {
        if request.method == "HEAD" && matches!(request.path.as_str(), "/other" | "/b") {
            Reply::ok("")
        } else if request.method == "HEAD" {
            Reply::status(404, "")
        } else {
            Reply::ok(list_xml(&[], &[], None))
        }
    })
    .await;
    let mut client = server.client(Some("b"), &[]).await;
    client.connect_to_database("other").await.unwrap();
    assert_eq!(client.get_current_database(), "other");
    assert_eq!(
        client.get_connection_info().database.as_deref(),
        Some("other")
    );
    assert!(client.connect_to_database("missing").await.is_err());
    assert_eq!(
        client.get_current_database(),
        "other",
        "a failed switch changes nothing"
    );
    assert!(client.connect_to_database("bad/name").await.is_err());
    assert!(client.connect_to_database("").await.is_err());
    client.execute_limited("ls").await.unwrap();
    let listing = server
        .requests()
        .into_iter()
        .rfind(|r| r.has_param("list-type"))
        .unwrap();
    assert_eq!(listing.path, "/other");
}

#[tokio::test]
async fn ctrl_c_abandons_a_slow_request() {
    let server = FakeS3::start(with_bucket(|request| {
        if request.has_param("list-type") {
            Reply {
                delay: Some(Duration::from_secs(60)),
                ..Reply::ok(list_xml(&[], &[], None))
            }
        } else {
            Reply::ok("")
        }
    }))
    .await;
    let client = server.client(Some("b"), &[]).await;
    let flag = Arc::new(AtomicBool::new(false));
    set_thread_interrupt_flag(Some(flag.clone()));
    let trigger = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.store(true, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let error = client.execute_limited("ls").await.unwrap_err();
    set_thread_interrupt_flag(None);
    assert!(error.to_string().contains("interrupted"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn is_connected_reflects_the_endpoint() {
    let server = FakeS3::start(|_| Reply::ok("")).await;
    let client = server.client(Some("b"), &[]).await;
    assert!(client.is_connected().await);
}

#[tokio::test]
async fn server_info_names_the_endpoint_and_never_the_secret() {
    let server = FakeS3::start(|_| Reply::ok("")).await;
    let client = server.client(Some("b"), &[]).await;
    let info = client.get_server_info().await.unwrap();
    assert!(
        info.server_type
            .contains("S3-compatible store at http://127.0.0.1"),
        "{}",
        info.server_type
    );
    assert_eq!(info.server_version, "us-east-1");
    assert_eq!(info.additional_info["region"], "us-east-1");
    assert_eq!(info.additional_info["credentials"], "static access key");
    assert!(!format!("{info:?}").contains(SECRET_KEY));
}

#[test]
fn options_are_clamped() {
    let mut info = ConnectionInfo::default();
    let limits = limits_from(&info);
    assert_eq!(limits.max_rows, DEFAULT_MAX_ROWS);
    assert_eq!(limits.max_get_bytes, DEFAULT_MAX_GET_BYTES);
    assert_eq!(limits.default_get_bytes, DEFAULT_GET_BYTES);
    info.options
        .insert("max_get_bytes".into(), "999999999999".into());
    info.options.insert("max_rows".into(), "999999999".into());
    info.options.insert("get_bytes".into(), "0".into());
    let limits = limits_from(&info);
    assert_eq!(limits.max_get_bytes, MAX_MAX_GET_BYTES);
    assert_eq!(limits.max_rows, MAX_MAX_ROWS);
    assert_eq!(
        limits.default_get_bytes, DEFAULT_GET_BYTES,
        "0 is not a size"
    );
    // the default chunk never exceeds the cap
    info.options.insert("max_get_bytes".into(), "1000".into());
    info.options.insert("get_bytes".into(), "5000".into());
    let limits = limits_from(&info);
    assert_eq!(limits.default_get_bytes, 1000);
}

mod range_plans {
    use crate::ops::{RangePlan, plan_range};
    use dbcrust_core::s3_command::ByteRange;
    use rstest::rstest;

    const DEFAULT: u64 = 100;
    const MAX: u64 = 1000;

    #[rstest]
    #[case(ByteRange::Default, "bytes=0-99", 100, Some(0))]
    #[case(ByteRange::First(10), "bytes=0-9", 10, Some(0))]
    #[case(ByteRange::First(1000), "bytes=0-999", 1000, Some(0))]
    #[case(ByteRange::From { start: 5, end: Some(14) }, "bytes=5-14", 10, Some(5))]
    #[case(ByteRange::From { start: 5, end: None }, "bytes=5-1004", 1000, Some(5))]
    #[case(ByteRange::Suffix(7), "bytes=-7", 7, None)]
    fn plans(
        #[case] range: ByteRange,
        #[case] header: &str,
        #[case] limit: u64,
        #[case] start: Option<u64>,
    ) {
        assert_eq!(
            plan_range(range, DEFAULT, MAX).unwrap(),
            RangePlan {
                header: header.to_string(),
                limit,
                start
            }
        );
    }

    #[rstest]
    #[case(ByteRange::First(1001))]
    #[case(ByteRange::From { start: 0, end: Some(1000) })]
    #[case(ByteRange::Suffix(1001))]
    fn over_the_cap_is_refused(#[case] range: ByteRange) {
        assert!(
            plan_range(range, DEFAULT, MAX)
                .unwrap_err()
                .contains("capped")
        );
    }

    #[test]
    fn the_default_chunk_never_exceeds_the_cap() {
        assert_eq!(plan_range(ByteRange::Default, 5000, 50).unwrap().limit, 50);
    }

    #[test]
    fn open_ranges_near_u64_max_do_not_overflow() {
        let plan = plan_range(
            ByteRange::From {
                start: u64::MAX - 5,
                end: None,
            },
            DEFAULT,
            MAX,
        )
        .unwrap();
        assert_eq!(plan.limit, MAX);
    }
}

mod prefixes {
    use crate::ops::{normalize_prefix, parent_prefix};
    use rstest::rstest;

    #[rstest]
    #[case("a/b/", "a/")]
    #[case("a/", "")]
    #[case("", "")]
    #[case("a/b/c", "a/b/")]
    fn parents(#[case] prefix: &str, #[case] expected: &str) {
        assert_eq!(parent_prefix(prefix), expected);
    }

    #[rstest]
    #[case("a", "a/")]
    #[case("/a/b", "a/b/")]
    #[case("a/b/", "a/b/")]
    #[case("/", "")]
    #[case("", "")]
    fn normalized(#[case] path: &str, #[case] expected: &str) {
        assert_eq!(normalize_prefix(path), expected);
    }
}

/// Live check against a real endpoint (MinIO, LocalStack, R2…): set
/// `S3_TEST_URL` to a connection URL whose bucket holds at least one object
/// (e.g. `s3://AKIA:secret@bucket?endpoint=http://127.0.0.1:9000`), and
/// optionally `S3_TEST_KEY` to the key of a text object to read.
#[tokio::test]
async fn live_endpoint_round_trip() {
    let Ok(url) = std::env::var("S3_TEST_URL") else {
        eprintln!("S3_TEST_URL not set; skipping the live S3 test");
        return;
    };
    let info = ConnectionInfo::parse_url(&url).expect("S3_TEST_URL parses");
    let client = S3Client::new(info).await.expect("connects");
    let listing = client.execute_limited("ls -r -n 10").await.expect("ls");
    assert!(
        !listing.result.rows.is_empty(),
        "the bucket should hold an object"
    );
    let key = std::env::var("S3_TEST_KEY")
        .ok()
        .or_else(|| {
            listing
                .result
                .rows
                .iter()
                .find(|row| row[1].as_deref() == Some("object"))
                .and_then(|row| row[0].clone())
        })
        .expect("an object to read");
    let head = client
        .execute_limited(&format!("head '{key}'"))
        .await
        .expect("head");
    assert!(
        head.result
            .rows
            .iter()
            .any(|row| row[0].as_deref() == Some("size"))
    );
    let get = client
        .execute_limited(&format!("get '{key}' --bytes 64"))
        .await
        .expect("get");
    assert!(!get.result.rows.is_empty() || get.notices.iter().any(|n| n.contains("empty")));
    let buckets = client.execute_limited("buckets").await;
    assert!(buckets.is_ok() || buckets.unwrap_err().to_string().contains("AccessDenied"));
    assert!(client.execute_limited("rm x").await.is_err());
}
