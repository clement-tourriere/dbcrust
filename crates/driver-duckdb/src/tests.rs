//! Real tests: DuckDB is embedded, so every test opens a genuine engine on a
//! temporary file or in memory. Nothing here needs a server or the network.

use super::*;
use dbcrust_core::database::{
    OPT_HARDEN_READ_ONLY, OPT_HARDEN_TIMEOUT_SECS, set_thread_interrupt_flag,
};
use rstest::rstest;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn info(path: Option<&std::path::Path>, options: &[(&str, &str)]) -> ConnectionInfo {
    ConnectionInfo {
        database_type: DatabaseType::DuckDB,
        file_path: Some(
            path.map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ":memory:".to_string()),
        ),
        options: options
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<HashMap<_, _>>(),
        ..Default::default()
    }
}

async fn memory() -> DuckDbClient {
    DuckDbClient::new(info(None, &[])).await.unwrap()
}

/// A database file with a small schema, closed so it can be reopened.
async fn seeded_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("seed.duckdb");
    let mut db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    for sql in [
        "CREATE TABLE people(id INTEGER PRIMARY KEY, name VARCHAR NOT NULL, age INTEGER CHECK (age >= 0))",
        "CREATE TABLE pets(id INTEGER PRIMARY KEY, owner INTEGER REFERENCES people(id), kind VARCHAR)",
        "CREATE VIEW adults AS SELECT * FROM people WHERE age >= 18",
        "CREATE INDEX people_name ON people(name)",
        "INSERT INTO people VALUES (1, 'Ada', 36), (2, 'Bob', 12)",
        "INSERT INTO pets VALUES (1, 1, 'cat')",
    ] {
        db.execute_query(sql).await.unwrap();
    }
    db.close().await.unwrap();
    path
}

fn hardened() -> [(&'static str, &'static str); 1] {
    [(OPT_HARDEN_READ_ONLY, "1")]
}

fn first_cell(result: &StructuredQueryResult) -> Option<&str> {
    result.rows.first()?.first()?.as_deref()
}

// ----- queries and values ---------------------------------------------------

#[tokio::test]
async fn a_query_returns_columns_and_rows() {
    let db = memory().await;
    let result = db
        .execute_query_structured("SELECT 1 AS one, 'two' AS two")
        .await
        .unwrap();
    assert_eq!(result.columns, vec!["one", "two"]);
    assert_eq!(
        result.rows,
        vec![vec![Some("1".to_string()), Some("two".to_string())]]
    );
}

#[tokio::test]
async fn the_legacy_shape_has_a_header_row_and_renders_null_as_null() {
    let db = memory().await;
    let rows = db.execute_query("SELECT NULL AS n, '' AS e").await.unwrap();
    assert_eq!(rows, vec![vec!["n", "e"], vec!["NULL", ""]]);
}

#[tokio::test]
async fn null_and_the_empty_string_stay_distinct() {
    let db = memory().await;
    let result = db
        .execute_query_structured("SELECT NULL::VARCHAR AS n, '' AS e")
        .await
        .unwrap();
    assert_eq!(result.rows, vec![vec![None, Some(String::new())]]);
}

#[tokio::test]
async fn an_empty_statement_is_an_empty_result() {
    let db = memory().await;
    assert_eq!(
        db.execute_query("   ").await.unwrap(),
        Vec::<Vec<String>>::new()
    );
}

#[tokio::test]
async fn a_query_error_is_reported_with_the_engine_message() {
    let db = memory().await;
    let error = db.execute_query("SELECT * FROM nowhere").await.unwrap_err();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{error:?}");
    assert!(error.to_string().contains("nowhere"), "{error}");
}

#[tokio::test]
async fn a_multi_statement_string_runs_all_and_returns_the_last_result() {
    let db = memory().await;
    let result = db
        .execute_query_structured(
            "CREATE TABLE t(i INT); INSERT INTO t VALUES (1), (2); SELECT count(*) FROM t",
        )
        .await
        .unwrap();
    assert_eq!(first_cell(&result), Some("2"));
}

#[rstest]
#[case::boolean("true", "true")]
#[case::tinyint("(-5)::TINYINT", "-5")]
#[case::bigint("9223372036854775807::BIGINT", "9223372036854775807")]
#[case::ubigint("18446744073709551615::UBIGINT", "18446744073709551615")]
#[case::hugeint(
    "170141183460469231731687303715884105::HUGEINT",
    "170141183460469231731687303715884105"
)]
#[case::hugeint_negative(
    "(-170141183460469231731687303715884105)::HUGEINT",
    "-170141183460469231731687303715884105"
)]
#[case::double("1.5::DOUBLE", "1.5")]
#[case::nan("'NaN'::DOUBLE", "NaN")]
#[case::decimal_keeps_its_scale("1.50::DECIMAL(10,2)", "1.50")]
#[case::decimal_is_exact(
    "12345678901234567890.123456789::DECIMAL(38,9)",
    "12345678901234567890.123456789"
)]
#[case::varchar("'héllo'", "héllo")]
#[case::date("DATE '2024-02-29'", "2024-02-29")]
#[case::date_infinity("'infinity'::DATE", "infinity")]
#[case::date_negative_infinity("'-infinity'::DATE", "-infinity")]
#[case::time("TIME '03:04:05'", "03:04:05")]
#[case::time_fraction("TIME '03:04:05.5'", "03:04:05.5")]
#[case::timestamp("TIMESTAMP '2024-01-02 03:04:05'", "2024-01-02 03:04:05")]
#[case::timestamp_micros("TIMESTAMP '2024-01-02 03:04:05.123456'", "2024-01-02 03:04:05.123456")]
#[case::timestamp_ms("TIMESTAMP_MS '2024-01-02 03:04:05.123'", "2024-01-02 03:04:05.123")]
#[case::timestamp_s("TIMESTAMP_S '2024-01-02 03:04:05'", "2024-01-02 03:04:05")]
#[case::timestamp_ns(
    "TIMESTAMP_NS '2024-01-02 03:04:05.123456789'",
    "2024-01-02 03:04:05.123456789"
)]
#[case::timestamp_infinity("'infinity'::TIMESTAMP", "infinity")]
#[case::timestamp_negative_infinity("'-infinity'::TIMESTAMP", "-infinity")]
#[case::timestamp_far_future("TIMESTAMP '9999-12-31 23:59:59.5'", "9999-12-31 23:59:59.5")]
#[case::timestamp_far_past("TIMESTAMP '0001-01-01 00:00:00'", "0001-01-01 00:00:00")]
#[case::timestamp_before_1970("TIMESTAMP '1960-06-15 12:00:00.25'", "1960-06-15 12:00:00.25")]
#[case::timestamptz_far_future("TIMESTAMPTZ '9999-12-31 23:59:59+00'", "9999-12-31 23:59:59+00")]
#[case::date_far_future("DATE '9999-12-31'", "9999-12-31")]
#[case::date_before_1970("DATE '1969-12-31'", "1969-12-31")]
#[case::timestamptz("TIMESTAMPTZ '2024-01-02 03:04:05.5+00'", "2024-01-02 03:04:05.5+00")]
#[case::timestamptz_is_shown_in_utc(
    "TIMESTAMPTZ '2024-01-02 03:04:05+02'",
    "2024-01-02 01:04:05+00"
)]
#[case::interval(
    "INTERVAL '1 year 2 months 3 days 4 hours 5 minutes 6.5 seconds'",
    "1 year 2 months 3 days 04:05:06.5"
)]
#[case::interval_singular("INTERVAL '1 day'", "1 day")]
#[case::interval_time_only("INTERVAL 90 MINUTES", "01:30:00")]
#[case::blob_printable("'abc'::BLOB", "abc")]
#[case::blob_escapes("'\\xAA\\x00z'::BLOB", "\\xAA\\x00z")]
#[case::uuid(
    "UUID '00000000-0000-0000-0000-000000000001'",
    "00000000-0000-0000-0000-000000000001"
)]
#[case::enum_value("'b'::ENUM('a', 'b')", "b")]
#[case::list("[1, 2, 3]", "[1,2,3]")]
#[case::list_with_null_and_strings("['a', NULL, 'b\"c']", "[\"a\",null,\"b\\\"c\"]")]
#[case::empty_list("[]::INTEGER[]", "[]")]
#[case::fixed_size_array("[1, 2]::INTEGER[2]", "[1,2]")]
#[case::struct_value("{'a': 1, 'b': [1.5, NULL]}", "{\"a\":1,\"b\":[1.5,null]}")]
#[case::struct_decimal_stays_exact("{'d': 1.50::DECIMAL(10,2)}", "{\"d\":1.50}")]
#[case::struct_date_is_a_string("{'d': DATE '2024-01-02'}", "{\"d\":\"2024-01-02\"}")]
#[case::map_value("MAP {'k': 1, 'j': 2}", "{\"k\":1,\"j\":2}")]
#[case::nested("[{'x': [1, 2]}, {'x': []}]", "[{\"x\":[1,2]},{\"x\":[]}]")]
#[case::union_value("union_value(num := 3)", "3")]
#[tokio::test]
async fn values_are_rendered_the_way_duckdb_prints_them(
    #[case] expression: &str,
    #[case] expected: &str,
) {
    let db = memory().await;
    let result = db
        .execute_query_structured(&format!("SELECT {expression}"))
        .await
        .unwrap();
    assert_eq!(first_cell(&result), Some(expected), "{expression}");
}

#[tokio::test]
async fn special_float_values_in_json_are_strings() {
    let db = memory().await;
    let result = db
        .execute_query_structured("SELECT [1.5, 'NaN'::DOUBLE, 'inf'::DOUBLE]")
        .await
        .unwrap();
    assert_eq!(first_cell(&result), Some("[1.5,\"NaN\",\"inf\"]"));
}

#[tokio::test]
async fn a_large_result_streams_in_chunks_and_keeps_order() {
    let db = memory().await;
    let result = db
        .execute_query_structured("SELECT i FROM range(10000) t(i)")
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 10_000);
    assert_eq!(result.rows[0][0].as_deref(), Some("0"));
    assert_eq!(result.rows[9_999][0].as_deref(), Some("9999"));
}

#[tokio::test]
async fn a_command_answers_ok_and_a_dml_its_row_count() {
    let db = memory().await;
    let created = db
        .execute_query_structured("CREATE TABLE t(i INT)")
        .await
        .unwrap();
    assert_eq!(created.columns, vec!["Status"]);
    assert_eq!(first_cell(&created), Some("OK"));
    let inserted = db
        .execute_query_structured("INSERT INTO t VALUES (1), (2), (3)")
        .await
        .unwrap();
    assert_eq!(inserted.columns, vec!["Count"]);
    assert_eq!(first_cell(&inserted), Some("3"));
}

#[tokio::test]
async fn session_state_persists_across_statements() {
    let db = memory().await;
    db.execute_query("CREATE TEMP TABLE scratch AS SELECT 42 AS answer")
        .await
        .unwrap();
    db.execute_query("SET threads = 1").await.unwrap();
    let result = db
        .execute_query_structured("SELECT answer, current_setting('threads') FROM scratch")
        .await
        .unwrap();
    assert_eq!(result.rows[0][0].as_deref(), Some("42"));
    assert_eq!(result.rows[0][1].as_deref(), Some("1"));
}

#[tokio::test]
async fn parquet_csv_and_json_files_are_queried_natively() {
    let dir = tempfile::tempdir().unwrap();
    let parquet = dir.path().join("p.parquet");
    let csv = dir.path().join("c.csv");
    let json = dir.path().join("j.json");
    let db = memory().await;
    db.execute_query(&format!(
        "COPY (SELECT 1 AS a, 'x' AS b) TO '{}' (FORMAT parquet)",
        parquet.display()
    ))
    .await
    .unwrap();
    db.execute_query(&format!(
        "COPY (SELECT 2 AS a, 'y' AS b) TO '{}' (FORMAT csv, HEADER)",
        csv.display()
    ))
    .await
    .unwrap();
    db.execute_query(&format!(
        "COPY (SELECT 3 AS a, 'z' AS b) TO '{}' (FORMAT json)",
        json.display()
    ))
    .await
    .unwrap();
    for (path, expected) in [(&parquet, "1"), (&csv, "2"), (&json, "3")] {
        let result = db
            .execute_query_structured(&format!("SELECT a FROM '{}'", path.display()))
            .await
            .unwrap();
        assert_eq!(first_cell(&result), Some(expected), "{}", path.display());
    }
}

// ----- row cap, timeout, interrupt -------------------------------------------

#[tokio::test]
async fn max_rows_cuts_the_result_at_fetch_time() {
    let db = DuckDbClient::new(info(None, &[("max_rows", "5")]))
        .await
        .unwrap();
    let result = db
        .execute_query_structured("SELECT i FROM range(1000000000) t(i)")
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 5);
    // The session is fine afterwards (the cut statement was dropped, not left running)
    let next = db.execute_query_structured("SELECT 1").await.unwrap();
    assert_eq!(first_cell(&next), Some("1"));
}

#[tokio::test]
async fn max_rows_zero_means_unlimited() {
    let db = DuckDbClient::new(info(None, &[("max_rows", "0")]))
        .await
        .unwrap();
    let result = db
        .execute_query_structured("SELECT i FROM range(150000) t(i)")
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 150_000);
}

#[tokio::test]
async fn a_result_under_the_default_cap_is_not_cut() {
    let db = memory().await;
    let result = db
        .execute_query_structured("SELECT i FROM range(100000) t(i)")
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 100_000);
}

const ENDLESS: &str = "SELECT count(*) FROM range(100000000000) a, range(100000000000) b";

#[tokio::test(flavor = "multi_thread")]
async fn a_statement_past_the_deadline_is_interrupted_and_the_session_survives() {
    let db = DuckDbClient::new(info(None, &[(OPT_HARDEN_TIMEOUT_SECS, "1")]))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let error = db.execute_query(ENDLESS).await.unwrap_err();
    assert!(error.to_string().contains("timed out after 1s"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    let after = db.execute_query_structured("SELECT 'alive'").await.unwrap();
    assert_eq!(first_cell(&after), Some("alive"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_c_interrupts_a_running_statement() {
    let db = memory().await;
    let flag = Arc::new(AtomicBool::new(false));
    set_thread_interrupt_flag(Some(flag.clone()));
    let trigger = flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        trigger.store(true, Ordering::Relaxed);
    });
    let error = db.execute_query(ENDLESS).await.unwrap_err();
    assert!(error.to_string().contains("interrupted"), "{error}");

    flag.store(false, Ordering::Relaxed);
    let after = db.execute_query_structured("SELECT 'alive'").await.unwrap();
    assert_eq!(first_cell(&after), Some("alive"));
    set_thread_interrupt_flag(None);
}

// ----- files and persistence -------------------------------------------------

#[tokio::test]
async fn a_database_file_persists_across_connections() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    let result = db
        .execute_query_structured("SELECT name FROM people ORDER BY id")
        .await
        .unwrap();
    assert_eq!(
        result.rows,
        vec![vec![Some("Ada".to_string())], vec![Some("Bob".to_string())]]
    );
}

#[tokio::test]
async fn opening_a_directory_or_garbage_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let error = DuckDbClient::new(info(Some(dir.path()), &[]))
        .await
        .err()
        .expect("a directory is not a database");
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );

    let garbage = dir.path().join("garbage.duckdb");
    std::fs::write(&garbage, b"this is not a duckdb file at all, just text").unwrap();
    let error = DuckDbClient::new(info(Some(&garbage), &[]))
        .await
        .err()
        .expect("garbage is not a database");
    assert!(
        matches!(error, DatabaseError::ConnectionError(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn the_server_info_names_the_engine_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    let server = db.get_server_info().await.unwrap();
    assert_eq!(server.server_type, "DuckDB");
    assert_eq!(server.version_major, Some(1));
    assert_eq!(
        server.additional_info.get("database_file"),
        Some(&path.to_string_lossy().into_owned())
    );
    assert_eq!(
        server
            .additional_info
            .get("access_mode")
            .map(String::as_str),
        Some("read_write")
    );
}

// ----- metadata ----------------------------------------------------------------

#[tokio::test]
async fn schemas_tables_and_columns_come_from_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    let meta = db.get_metadata_provider();
    assert_eq!(meta.get_schemas().await.unwrap(), vec!["main"]);
    assert_eq!(
        meta.get_tables(None).await.unwrap(),
        vec!["adults", "people", "pets"]
    );
    assert_eq!(
        meta.get_tables(Some("main")).await.unwrap(),
        vec!["adults", "people", "pets"]
    );
    assert_eq!(
        meta.get_columns("people", None).await.unwrap(),
        vec!["id", "name", "age"]
    );
    assert!(meta.get_tables(Some("nope")).await.unwrap().is_empty());
    assert!(
        meta.get_functions(None)
            .await
            .unwrap()
            .contains(&"read_csv".to_string())
    );
    assert_eq!(meta.default_schema().as_deref(), Some("main"));
    assert!(meta.supports_explain());
}

#[tokio::test]
async fn table_details_cover_keys_checks_indexes_and_references() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    let meta = db.get_metadata_provider();

    let people = meta.get_table_details("people", None).await.unwrap();
    assert_eq!(people.schema, "main");
    assert_eq!(people.full_name, "main.people");
    let name = people.columns.iter().find(|c| c.name == "name").unwrap();
    assert_eq!(name.data_type, "VARCHAR");
    assert!(!name.nullable);
    let age = people.columns.iter().find(|c| c.name == "age").unwrap();
    assert!(age.nullable);
    let primary = people.indexes.iter().find(|i| i.is_primary).unwrap();
    assert_eq!(primary.definition, "(id)");
    assert!(primary.is_unique);
    let explicit = people
        .indexes
        .iter()
        .find(|i| i.name == "people_name")
        .unwrap();
    assert_eq!(explicit.definition, "(name)");
    assert!(!explicit.is_unique);
    assert_eq!(people.check_constraints.len(), 1);
    assert!(people.check_constraints[0].definition.contains("age"));
    assert_eq!(people.referenced_by.len(), 1);
    assert_eq!(people.referenced_by[0].table, "pets");
    assert_eq!(
        people.referenced_by[0].definition,
        "FOREIGN KEY (owner) REFERENCES people(id)"
    );

    let pets = meta.get_table_details("pets", None).await.unwrap();
    assert_eq!(pets.foreign_keys.len(), 1);
    assert_eq!(
        pets.foreign_keys[0].definition,
        "FOREIGN KEY (owner) REFERENCES people(id)"
    );

    // Views have columns too
    let adults = meta.get_table_details("adults", None).await.unwrap();
    assert_eq!(adults.columns.len(), 3);

    let missing = meta.get_table_details("ghost", None).await.unwrap_err();
    assert!(missing.to_string().contains("does not exist"), "{missing}");
}

#[tokio::test]
async fn metadata_names_travel_as_parameters_not_sql() {
    let db = memory().await;
    db.execute_query("CREATE TABLE \"it's\"(\"a'b\" INT)")
        .await
        .unwrap();
    let meta = db.get_metadata_provider();
    assert_eq!(meta.get_tables(None).await.unwrap(), vec!["it's"]);
    assert_eq!(meta.get_columns("it's", None).await.unwrap(), vec!["a'b"]);
    // An injection attempt is just a table that does not exist
    assert!(
        meta.get_columns("x' OR '1'='1", None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn databases_are_listed_and_switched_with_use() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let other = dir.path().join("other.duckdb");
    let mut db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    assert_eq!(db.get_current_database(), "seed");

    db.execute_query(&format!("ATTACH '{}' AS other", other.display()))
        .await
        .unwrap();
    db.execute_query("CREATE TABLE other.marker(i INT)")
        .await
        .unwrap();

    let listing = db.list_databases().await.unwrap();
    assert_eq!(listing[0], vec!["Name", "Path", "Type", "Kind", "Current"]);
    let seed = listing.iter().find(|row| row[0] == "seed").unwrap();
    assert_eq!(seed[4], "*");
    assert!(
        listing
            .iter()
            .any(|row| row[0] == "other" && row[4].is_empty())
    );

    db.connect_to_database("other").await.unwrap();
    assert_eq!(db.get_current_database(), "other");
    assert_eq!(db.get_connection_info().database.as_deref(), Some("other"));
    assert_eq!(
        db.get_metadata_provider().get_tables(None).await.unwrap(),
        vec!["marker"]
    );
    assert!(db.connect_to_database("missing").await.is_err());
    assert_eq!(db.get_current_database(), "other");

    // A USE typed as a statement is picked up too
    db.execute_query("USE seed").await.unwrap();
    assert_eq!(db.get_current_database(), "seed");
}

// ----- EXPLAIN -----------------------------------------------------------------

#[tokio::test]
async fn explain_returns_the_plan_without_running_the_statement() {
    let db = memory().await;
    db.execute_query("CREATE TABLE t(i INT)").await.unwrap();
    let raw = db
        .explain_query_raw("INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    assert_eq!(raw[0], vec!["explain_key", "explain_value"]);
    assert_eq!(raw[1][0], "physical_plan");
    let count = db
        .execute_query_structured("SELECT count(*) FROM t")
        .await
        .unwrap();
    assert_eq!(first_cell(&count), Some("0"));

    let lines = db.explain_query("SELECT * FROM t;").await.unwrap();
    assert_eq!(lines[0], vec!["QUERY PLAN"]);
    assert!(lines.len() > 3);
    assert!(
        lines
            .iter()
            .any(|line| line[0].contains("Table: memory.main.t"))
    );
}

#[tokio::test]
async fn a_typed_explain_prints_one_plan_line_per_row() {
    let db = memory().await;
    db.execute_query("CREATE TABLE t(i INT)").await.unwrap();
    let typed = db
        .execute_query_structured("explain select * from t")
        .await
        .unwrap();
    assert_eq!(typed.columns, vec!["QUERY PLAN"]);
    assert!(typed.rows.len() > 3, "{typed:?}");
    assert!(
        typed
            .rows
            .iter()
            .all(|row| !row[0].as_deref().unwrap_or_default().contains('\n'))
    );
    // The explain methods keep the raw (key, value) answer
    let raw = db.explain_query_raw("select * from t").await.unwrap();
    assert_eq!(raw[0], vec!["explain_key", "explain_value"]);
    assert_eq!(raw.len(), 2);
    // Anything else is untouched, even a query that looks alike
    let other = db
        .execute_query_structured("SELECT 'physical_plan' AS explain_key, 'x' AS explain_value")
        .await
        .unwrap();
    assert_eq!(other.columns, vec!["explain_key", "explain_value"]);
}

#[tokio::test]
async fn explain_analyze_runs_the_statement_and_reports_timings() {
    let db = memory().await;
    let analyzed = db
        .explain_analyze_query_raw("SELECT count(*) FROM range(1000)")
        .await
        .unwrap();
    assert_eq!(analyzed[0], vec!["explain_key", "explain_value"]);
    assert!(analyzed[1][1].to_lowercase().contains("total time"));
}

#[tokio::test]
async fn test_query_validates_without_executing() {
    let db = memory().await;
    db.execute_query("CREATE TABLE t(i INT)").await.unwrap();
    db.test_query("INSERT INTO t VALUES (1)").await.unwrap();
    let count = db
        .execute_query_structured("SELECT count(*) FROM t")
        .await
        .unwrap();
    assert_eq!(first_cell(&count), Some("0"));
    let error = db.test_query("SELECT * FROM nowhere").await.unwrap_err();
    assert!(
        error.to_string().contains("Query validation failed"),
        "{error}"
    );
}

// ----- read-only: enforced by the engine ---------------------------------------

/// Run through the session directly, bypassing the textual guard, so these
/// tests prove what the engine itself refuses.
async fn engine(db: &DuckDbClient, sql: &str) -> Result<(), String> {
    db.session
        .run(sql)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tokio::test]
async fn read_only_opens_the_file_read_only_and_the_engine_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let db = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();

    // Reads work
    let people = db
        .execute_query_structured("SELECT count(*) FROM people")
        .await
        .unwrap();
    assert_eq!(first_cell(&people), Some("2"));
    let setting = db
        .execute_query_structured("SELECT current_setting('access_mode')")
        .await
        .unwrap();
    assert_eq!(first_cell(&setting), Some("read_only"));

    // Every write is refused by the engine, not by a textual guard
    for sql in [
        "INSERT INTO people VALUES (3, 'Eve', 20)",
        "UPDATE people SET age = 1",
        "DELETE FROM people",
        "CREATE TABLE z(i INT)",
        "DROP TABLE pets",
        "ALTER TABLE people ADD COLUMN x INT",
        "TRUNCATE people",
    ] {
        let refused = engine(&db, sql).await;
        assert!(refused.is_err(), "{sql} must be refused, got {refused:?}");
    }
    let still = db
        .execute_query_structured("SELECT count(*) FROM people")
        .await
        .unwrap();
    assert_eq!(first_cell(&still), Some("2"));

    // DuckDB's temporary catalog is in memory, never in the file
    engine(&db, "CREATE TEMP TABLE scratch AS SELECT 1 AS i")
        .await
        .unwrap();
    drop(db);
    let reopened = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();
    let tables = reopened
        .get_metadata_provider()
        .get_tables(None)
        .await
        .unwrap();
    assert_eq!(tables, vec!["adults", "people", "pets"]);
}

#[tokio::test]
async fn read_only_turns_external_access_off_and_locks_the_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let outside = dir.path().join("leak.csv");
    let db = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();

    let attempts = [
        format!("COPY (SELECT 1) TO '{}'", outside.display()),
        format!("COPY people TO '{}' (FORMAT csv)", outside.display()),
        "SELECT * FROM read_csv('/etc/hosts')".to_string(),
        "SELECT * FROM '/etc/hosts.csv'".to_string(),
        "SELECT * FROM glob('/etc/*')".to_string(),
        "SELECT * FROM read_text('/etc/hosts')".to_string(),
        format!(
            "ATTACH '{}' AS other",
            dir.path().join("other.duckdb").display()
        ),
        "INSTALL httpfs".to_string(),
        "LOAD httpfs".to_string(),
        format!("EXPORT DATABASE '{}'", dir.path().join("export").display()),
        "SET enable_external_access = true".to_string(),
        "SET access_mode = 'read_write'".to_string(),
        "SET lock_configuration = false".to_string(),
        "SET autoinstall_known_extensions = true".to_string(),
        "SET allow_community_extensions = true".to_string(),
        "SET allowed_directories = ['/']".to_string(),
        "SET threads = 99".to_string(),
    ];
    for sql in &attempts {
        let refused = engine(&db, sql).await;
        assert!(refused.is_err(), "{sql} must be refused, got {refused:?}");
    }
    assert!(
        !outside.exists(),
        "nothing was written outside the database"
    );
    assert!(!dir.path().join("export").exists());

    // The locks themselves are visible as settings
    let external = db
        .execute_query_structured("SELECT current_setting('enable_external_access')")
        .await
        .unwrap();
    assert_eq!(first_cell(&external), Some("false"));
    let locked = db
        .execute_query_structured("SELECT current_setting('lock_configuration')")
        .await
        .unwrap();
    assert_eq!(first_cell(&locked), Some("true"));
}

#[tokio::test]
async fn allowed_directories_let_a_read_only_connection_read_one_folder() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let data = tempfile::tempdir().unwrap();
    let secret = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("ok.csv"), "a\n1\n").unwrap();
    std::fs::write(secret.path().join("no.csv"), "a\n2\n").unwrap();
    let allowed = data.path().canonicalize().unwrap();
    let db = DuckDbClient::new(info(
        Some(&path),
        &[
            (OPT_HARDEN_READ_ONLY, "1"),
            ("allowed_directories", allowed.to_str().unwrap()),
        ],
    ))
    .await
    .unwrap();
    let ok = engine(
        &db,
        &format!(
            "SELECT * FROM read_csv('{}')",
            allowed.join("ok.csv").display()
        ),
    )
    .await;
    assert!(ok.is_ok(), "{ok:?}");
    let denied = engine(
        &db,
        &format!(
            "SELECT * FROM read_csv('{}')",
            secret
                .path()
                .canonicalize()
                .unwrap()
                .join("no.csv")
                .display()
        ),
    )
    .await;
    assert!(denied.is_err(), "{denied:?}");
    // The folder list itself is locked
    let widen = engine(&db, "SET allowed_directories = ['/']").await;
    assert!(widen.is_err(), "{widen:?}");
    let external = engine(&db, "SET enable_external_access = true").await;
    assert!(external.is_err(), "{external:?}");
    // Reading anywhere else, and the ATTACH/INSTALL family, stay refused
    for sql in [
        "SELECT * FROM read_csv('/etc/hosts')",
        "INSTALL httpfs",
        "ATTACH '/tmp/other.duckdb' AS other",
    ] {
        assert!(engine(&db, sql).await.is_err(), "{sql}");
    }
}

#[tokio::test]
async fn the_textual_guard_also_runs_on_a_read_only_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let db = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();
    let refused = db.execute_query("DELETE FROM people").await.unwrap_err();
    assert!(refused.to_string().contains("read-only mode:"), "{refused}");
    let refused = db
        .execute_query("SELECT * FROM read_csv('/etc/hosts')")
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("read-only mode:"), "{refused}");
    // Reads, DESCRIBE, SHOW, EXPLAIN and PRAGMA table_info go through
    for sql in [
        "SELECT * FROM adults",
        "DESCRIBE people",
        "SHOW TABLES",
        "EXPLAIN SELECT * FROM people",
        "PRAGMA table_info('people')",
        "FROM people SELECT name",
    ] {
        db.execute_query(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let explain = db.explain_query("DELETE FROM people").await.unwrap_err();
    assert!(explain.to_string().contains("read-only mode:"), "{explain}");
}

#[tokio::test]
async fn read_only_refuses_a_memory_database_a_missing_file_and_extensions() {
    let error = DuckDbClient::new(info(None, &hardened()))
        .await
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("needs a DuckDB database file"),
        "{error}"
    );

    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.duckdb");
    assert!(
        DuckDbClient::new(info(Some(&missing), &hardened()))
            .await
            .is_err()
    );
    assert!(!missing.exists(), "a read-only open never creates the file");

    let path = seeded_file(&dir).await;
    let error = DuckDbClient::new(info(
        Some(&path),
        &[(OPT_HARDEN_READ_ONLY, "1"), ("extensions", "httpfs")],
    ))
    .await
    .err()
    .unwrap();
    assert!(
        error.to_string().contains("never installs or loads"),
        "{error}"
    );
    let error = DuckDbClient::new(info(
        Some(&path),
        &[
            (OPT_HARDEN_READ_ONLY, "1"),
            ("ducklake", "sqlite:/tmp/x.sqlite"),
        ],
    ))
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("DuckLake"), "{error}");
}

#[tokio::test]
async fn the_access_mode_option_opens_the_file_read_only_without_the_full_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    for options in [[("access_mode", "read_only")], [("read_only", "true")]] {
        let db = DuckDbClient::new(info(Some(&path), &options))
            .await
            .unwrap();
        let write = engine(&db, "INSERT INTO people VALUES (9, 'Zed', 1)").await;
        assert!(write.is_err(), "{options:?}: {write:?}");
        // Not hardened: files may still be read, the configuration is open
        engine(&db, "SET threads = 2").await.unwrap();
        let info = db.get_server_info().await.unwrap();
        assert_eq!(
            info.additional_info.get("access_mode").map(String::as_str),
            Some("read_only")
        );
        assert!(!info.additional_info.contains_key("external_access"));
    }
    let db = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();
    let info = db.get_server_info().await.unwrap();
    assert_eq!(
        info.additional_info
            .get("external_access")
            .map(String::as_str),
        Some("disabled")
    );
}

#[tokio::test]
async fn two_read_only_connections_can_share_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = seeded_file(&dir).await;
    let first = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();
    let second = DuckDbClient::new(info(Some(&path), &hardened()))
        .await
        .unwrap();
    for db in [&first, &second] {
        let people = db
            .execute_query_structured("SELECT count(*) FROM people")
            .await
            .unwrap();
        assert_eq!(first_cell(&people), Some("2"));
    }
}

// ----- configuration validation --------------------------------------------------

#[rstest]
#[case(&[("memory_limit", "lots")], "invalid memory_limit")]
#[case(&[("threads", "0")], "invalid threads")]
#[case(&[("threads", "many")], "invalid threads")]
#[case(&[("access_mode", "sideways")], "unknown access_mode")]
#[case(&[("extensions", "ht tp;fs")], "invalid extension name")]
#[tokio::test]
async fn bad_options_are_rejected_before_anything_opens(
    #[case] options: &[(&str, &str)],
    #[case] expected: &str,
) {
    let error = DuckDbClient::new(info(None, options)).await.err().unwrap();
    assert!(matches!(error, DatabaseError::InvalidUrl(_)), "{error:?}");
    assert!(error.to_string().contains(expected), "{error}");
}

#[tokio::test]
async fn memory_limit_and_threads_are_applied() {
    let db = DuckDbClient::new(info(None, &[("memory_limit", "256MB"), ("threads", "1")]))
        .await
        .unwrap();
    let threads = db
        .execute_query_structured("SELECT current_setting('threads')")
        .await
        .unwrap();
    assert_eq!(first_cell(&threads), Some("1"));
    let limit = db
        .execute_query_structured("SELECT current_setting('memory_limit')")
        .await
        .unwrap();
    assert!(
        first_cell(&limit).is_some_and(|v| v.contains("MiB") || v.contains("MB")),
        "{limit:?}"
    );
}

#[tokio::test]
async fn extensions_are_never_downloaded_without_the_opt_in() {
    let db = memory().await;
    let setting = db
        .execute_query_structured("SELECT current_setting('autoinstall_known_extensions')")
        .await
        .unwrap();
    assert_eq!(first_cell(&setting), Some("false"));
    let opted_in = DuckDbClient::new(info(None, &[("install_extensions", "true")]))
        .await
        .unwrap();
    let setting = opted_in
        .execute_query_structured("SELECT current_setting('autoinstall_known_extensions')")
        .await
        .unwrap();
    assert_eq!(first_cell(&setting), Some("true"));
}

#[tokio::test]
async fn ducklake_is_attached_through_the_extension_or_fails_with_a_hint() {
    // Offline and without a local copy of the extension, loading fails: the
    // message says how to fetch it
    let dir = tempfile::tempdir().unwrap();
    let meta = dir.path().join("meta.ducklake");
    let result = DuckDbClient::new(info(
        None,
        &[
            ("ducklake", meta.to_str().unwrap()),
            ("data_path", dir.path().join("data").to_str().unwrap()),
        ],
    ))
    .await;
    match result {
        // The extension happens to be installed here
        Ok(db) => assert_eq!(db.get_current_database(), "lake"),
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains("ducklake"), "{message}");
            assert!(message.contains("install_extensions=true"), "{message}");
        }
    }
}

/// Live DuckLake (downloads the extension): `DUCKLAKE_TEST=1`.
#[tokio::test]
async fn ducklake_round_trip() {
    if std::env::var_os("DUCKLAKE_TEST").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let meta = dir.path().join("meta.ducklake");
    let data = dir.path().join("data");
    let db = DuckDbClient::new(info(
        None,
        &[
            ("ducklake", meta.to_str().unwrap()),
            ("data_path", data.to_str().unwrap()),
            ("install_extensions", "true"),
        ],
    ))
    .await
    .unwrap();
    assert_eq!(db.get_current_database(), "lake");
    db.execute_query("CREATE TABLE events(id INT, what VARCHAR)")
        .await
        .unwrap();
    db.execute_query("INSERT INTO events VALUES (1, 'created'), (2, 'updated')")
        .await
        .unwrap();
    let count = db
        .execute_query_structured("SELECT count(*) FROM events")
        .await
        .unwrap();
    assert_eq!(first_cell(&count), Some("2"));
    assert_eq!(
        db.get_metadata_provider().get_tables(None).await.unwrap(),
        vec!["events"]
    );
    // The data lives as Parquet under data_path
    let parquet = std::fs::read_dir(&data).unwrap().flatten().any(|entry| {
        entry.path().is_dir() || entry.path().extension().is_some_and(|e| e == "parquet")
    });
    assert!(parquet, "no data files under {}", data.display());
    // Time travel is plain SQL
    let snapshots = db
        .execute_query_structured("SELECT count(*) FROM lake.snapshots()")
        .await
        .unwrap();
    assert!(first_cell(&snapshots).is_some());
}

// ----- lifecycle --------------------------------------------------------------------

#[tokio::test]
async fn close_releases_the_file_and_later_statements_fail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("life.duckdb");
    let mut db = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    assert!(db.is_connected().await);
    db.close().await.unwrap();
    assert!(!db.is_connected().await);
    let error = db.execute_query("SELECT 1").await.unwrap_err();
    assert!(error.to_string().contains("closed"), "{error}");
    // The lock is gone: another connection can write
    let again = DuckDbClient::new(info(Some(&path), &[])).await.unwrap();
    again.execute_query("CREATE TABLE t(i INT)").await.unwrap();
}

#[tokio::test]
async fn the_factory_in_the_registry_style_builds_a_client() {
    let client = DuckDbClient::new(info(None, &[])).await.unwrap();
    assert_eq!(
        client.get_connection_info().database_type,
        DatabaseType::DuckDB
    );
    let boxed: Box<dyn DatabaseClient> = Box::new(client);
    assert_eq!(boxed.get_current_database(), "memory");
}

#[test]
fn driver_errors_say_duckdb_and_redact_secrets() {
    let error = DatabaseError::driver(
        DatabaseType::DuckDB,
        std::io::Error::other("connect failed for postgres://user:hunter2@db/x"),
    );
    let text = error.to_string();
    assert!(text.starts_with("DuckDB error: "), "{text}");
    assert!(!text.contains("hunter2"), "{text}");
}
