//! Unit tests (no server needed) and the `MSSQL_URL`-gated integration suite.

use crate::conn::{blank_literals, build_config, describe_target, is_plain_dml};
use crate::explain_target;
use crate::statement_changes_database;
use crate::{MssqlClient, conn};
use dbcrust_core::database::{ConnectionInfo, DatabaseClient, DatabaseError, DatabaseType};
use rstest::rstest;

fn info(url: &str) -> ConnectionInfo {
    ConnectionInfo::parse_url(url).unwrap()
}

// ---- configuration ----------------------------------------------------------

#[rstest]
#[case("mssql://sa:pw@db.example/Sales", "db.example:1433")]
#[case("mssql://sa:pw@db.example:1444/Sales", "db.example:1444")]
#[case("mssql://sa:pw@[::1]:1433/master", "::1:1433")]
#[case("sqlserver://sa:pw@10.0.0.5", "10.0.0.5:1433")]
fn config_addresses(#[case] url: &str, #[case] addr: &str) {
    assert_eq!(build_config(&info(url)).unwrap().get_addr(), addr);
}

#[rstest]
#[case("mssql://sa:pw@h", "Required")]
#[case("mssql+tls://sa:pw@h", "Required")]
#[case("mssql://sa:pw@h?encrypt=true", "Required")]
#[case("mssql://sa:pw@h?encrypt=false", "Off")]
#[case("mssql://sa:pw@h?encrypt=strict", "Strict")]
fn encryption_levels(#[case] url: &str, #[case] level: &str) {
    let config = format!("{:?}", build_config(&info(url)).unwrap());
    assert!(
        config.contains(&format!("encryption: {level}")),
        "{url}: {config}"
    );
}

#[test]
fn certificates_are_verified_unless_explicitly_trusted() {
    let verified = format!("{:?}", build_config(&info("mssql://sa:pw@h")).unwrap());
    assert!(verified.contains("bypass: false"), "{verified}");
    let trusted = format!(
        "{:?}",
        build_config(&info("mssql://sa:pw@h?trustServerCertificate=true")).unwrap()
    );
    assert!(trusted.contains("bypass: true"), "{trusted}");
}

#[test]
fn a_pinned_ca_and_blind_trust_cannot_be_combined() {
    let mut conflicting = info("mssql://sa:pw@h?trustServerCertificate=true");
    conflicting
        .options
        .insert("ca_cert".to_string(), "/etc/ssl/ca.pem".to_string());
    assert!(matches!(
        build_config(&conflicting),
        Err(DatabaseError::InvalidUrl(_))
    ));
}

#[test]
fn a_pinned_ca_keeps_verification_on() {
    let mut pinned = info("mssql://sa:pw@h");
    pinned
        .options
        .insert("ca_cert".to_string(), "/etc/ssl/ca.pem".to_string());
    let config = format!("{:?}", build_config(&pinned).unwrap());
    assert!(config.contains("bypass: false"), "{config}");
    assert!(config.contains("/etc/ssl/ca.pem"), "{config}");
}

#[test]
fn named_instances_resolve_through_sql_browser() {
    let named = info("mssql://sa:pw@srv%5CSQLEXPRESS/Sales");
    let config = build_config(&named).unwrap();
    let debug = format!("{config:?}");
    assert!(
        debug.contains("instance_name: Some(\"SQLEXPRESS\")"),
        "{debug}"
    );
    assert_eq!(describe_target(&named), "srv\\SQLEXPRESS");
}

#[test]
fn application_name_and_intent_are_passed_on() {
    let config = format!(
        "{:?}",
        build_config(&info(
            "mssql://sa:pw@h?application_name=etl&intent=readonly"
        ))
        .unwrap()
    );
    assert!(
        config.contains("application_name: Some(\"etl\")"),
        "{config}"
    );
    assert!(config.contains("readonly: true"), "{config}");
    let default = format!("{:?}", build_config(&info("mssql://sa:pw@h")).unwrap());
    assert!(
        default.contains("application_name: Some(\"dbcrust\")"),
        "{default}"
    );
    assert!(default.contains("readonly: false"), "{default}");
}

#[test]
fn a_login_is_required() {
    let error = build_config(&info("mssql://h/db")).unwrap_err();
    assert!(error.to_string().contains("needs a login"), "{error}");
}

#[test]
fn the_password_never_reaches_debug_or_errors() {
    let secret = "Cnry-7f3a9";
    let with_password = info(&format!("mssql://sa:{secret}@db.example/Sales"));
    let config = format!("{:?}", build_config(&with_password).unwrap());
    assert!(!config.contains(secret), "{config}");
    assert!(!format!("{with_password:?}").contains(secret));
    assert!(!describe_target(&with_password).contains(secret));
    let wrong_option = format!("mssql://sa:{secret}@db.example/Sales?intent=readonly");
    assert!(!format!("{:?}", build_config(&info(&wrong_option)).unwrap()).contains(secret));
}

// ---- statement sniffing -------------------------------------------------------

#[rstest]
#[case("INSERT INTO t VALUES (1)", true)]
#[case("  update t set a = 1 where b = 2;", true)]
#[case("DELETE FROM t", true)]
#[case("MERGE t USING s ON 1 = 1 WHEN MATCHED THEN DELETE", true)]
#[case("INSERT INTO t OUTPUT inserted.* VALUES (1)", false)]
#[case("INSERT INTO t SELECT * FROM s", false)]
#[case("UPDATE t SET a = 1; SELECT * FROM t", false)]
#[case("UPDATE t SET a = 1; UPDATE t SET a = 2", false)]
#[case("SELECT 1", false)]
#[case("EXEC sp_who", false)]
#[case("-- comment\nDELETE FROM t", true)]
#[case("/* x */ DELETE FROM t", true)]
#[case("UPDATE t SET note = 'a; SELECT 1 OUTPUT' WHERE id = 1", true)]
#[case("UPDATE [output] SET [select] = 1", true)]
#[case("", false)]
fn plain_dml_is_sniffed_without_being_fooled_by_literals(#[case] sql: &str, #[case] plain: bool) {
    assert_eq!(is_plain_dml(sql), plain, "{sql}");
}

#[test]
fn blanking_keeps_offsets_and_hides_literals() {
    let sql = "SELECT 'a;b', [x]]y], \"q\" /* c /* d */ e */ -- tail\nFROM t";
    let blanked = blank_literals(sql);
    assert_eq!(blanked.chars().count(), sql.chars().count());
    assert!(!blanked.contains(';') && !blanked.contains("tail") && !blanked.contains('x'));
    assert!(blanked.contains("FROM t"));
}

#[rstest]
#[case("EXPLAIN SELECT 1", Some("SELECT 1"))]
#[case("explain   select * from t", Some("select * from t"))]
#[case("SELECT 1", None)]
#[case("EXPLAINED SELECT 1", None)]
fn explain_prefix(#[case] sql: &str, #[case] expected: Option<&str>) {
    assert_eq!(explain_target(sql).unwrap(), expected);
}

#[test]
fn explain_analyze_and_empty_explain_are_refused() {
    assert!(matches!(
        explain_target("EXPLAIN ANALYZE SELECT 1"),
        Err(DatabaseError::FeatureNotSupported { .. })
    ));
    assert!(matches!(
        explain_target("EXPLAIN"),
        Err(DatabaseError::QueryError(_))
    ));
}

#[rstest]
#[case("USE Sales", true)]
#[case("use [Sales];", true)]
#[case("SELECT 1", false)]
#[case("USERS", false)]
fn use_is_noticed(#[case] sql: &str, #[case] expected: bool) {
    assert_eq!(statement_changes_database(sql), expected);
}

// ---- errors ----------------------------------------------------------------

#[test]
fn connection_errors_name_the_target_and_hint_at_certificates() {
    let error = tiberius::error::Error::Tls("invalid peer certificate: UnknownIssuer".to_string());
    let text = conn::connection_error_for_tests("db.example:1433", &error).to_string();
    assert!(text.contains("db.example:1433"));
    assert!(text.contains("trustServerCertificate=true"));
    assert!(text.contains("ca_cert"));
}

#[test]
fn old_self_signed_certificates_get_their_own_explanation() {
    let error = tiberius::error::Error::Tls(
        "the server's certificate was rejected during the TLS handshake: Other(OtherError(UnsupportedCertVersion)). \
         The SQL Server presented a certificate rustls considers too old. To connect anyway you can: (1) trust a \
         specific CA certificate with `Config::trust_cert_ca(path)`"
            .to_string(),
    );
    let text = conn::connection_error_for_tests("db:1433", &error).to_string();
    assert!(text.contains("X.509"), "{text}");
    assert!(text.contains("trustServerCertificate=true"));
    assert!(!text.contains("Config::"), "{text}");
}

#[test]
fn connection_errors_redact_secrets_in_server_text() {
    let error = tiberius::error::Error::Protocol(
        "bad packet for mssql://sa:Cnry-7f3a9@db.example/x".into(),
    );
    let text = conn::connection_error_for_tests("db.example:1433", &error).to_string();
    assert!(!text.contains("Cnry-7f3a9"), "{text}");
}

#[test]
fn only_server_errors_leave_the_session_usable() {
    let io = tiberius::error::Error::Io {
        kind: tiberius::error::IoErrorKind::ConnectionReset,
        message: "reset".to_string(),
    };
    assert!(matches!(
        conn::query_error(&io),
        DatabaseError::ConnectionError(_)
    ));
}

// ---- integration (MSSQL_URL) --------------------------------------------------

/// `MSSQL_URL`, e.g. `mssql://sa:Passw0rd!x@localhost:1433/master?trustServerCertificate=true`.
fn live_url() -> Option<String> {
    std::env::var("MSSQL_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

async fn live() -> Option<MssqlClient> {
    let url = live_url()?;
    Some(
        MssqlClient::new(ConnectionInfo::parse_url(&url).unwrap())
            .await
            .expect("MSSQL_URL must be reachable"),
    )
}

fn cell(
    result: &dbcrust_core::database::StructuredQueryResult,
    row: usize,
    col: usize,
) -> Option<&str> {
    result.rows[row][col].as_deref()
}

#[tokio::test]
async fn live_types_nulls_and_empty_strings() {
    let Some(client) = live().await else { return };
    let result = client
        .execute_query_structured(
            "SELECT CAST(1 AS tinyint) AS a, CAST(-2 AS smallint) AS b, CAST(3 AS int) AS c, \
             CAST(4 AS bigint) AS d, CAST(1 AS bit) AS e, CAST(12.3450 AS decimal(10,4)) AS f, \
             CAST(1.5 AS float) AS g, CAST(2.5 AS real) AS h, CAST(12.5 AS money) AS i, \
             N'héllo' AS j, CAST('' AS nvarchar(10)) AS k, CAST(NULL AS int) AS l, \
             CAST('2024-02-29' AS date) AS m, CAST('13:14:15.123' AS time(3)) AS n, \
             CAST('2024-02-29 13:14:15.1234567' AS datetime2(7)) AS o, \
             CAST('2024-02-29 13:14:15 +05:30' AS datetimeoffset(0)) AS p, \
             CAST('2024-02-29 13:14:15.003' AS datetime) AS q, \
             CAST('2024-02-29 13:14' AS smalldatetime) AS r, \
             CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS uniqueidentifier) AS s, \
             CAST(0x01AB AS varbinary(10)) AS t, CAST('<a/>' AS xml) AS u",
        )
        .await
        .unwrap();
    let expected = [
        Some("1"),
        Some("-2"),
        Some("3"),
        Some("4"),
        Some("1"),
        Some("12.3450"),
        Some("1.5"),
        Some("2.5"),
        Some("12.5000"),
        Some("héllo"),
        Some(""),
        None,
        Some("2024-02-29"),
        Some("13:14:15.123"),
        Some("2024-02-29 13:14:15.1234567"),
        Some("2024-02-29 13:14:15 +05:30"),
        Some("2024-02-29 13:14:15.003"),
        Some("2024-02-29 13:14:00"),
        Some("6F9619FF-8B86-D011-B42D-00C04FC964FF"),
        Some("0x01AB"),
        Some("<a/>"),
    ];
    assert_eq!(result.rows.len(), 1);
    for (index, want) in expected.iter().enumerate() {
        assert_eq!(
            cell(&result, 0, index),
            *want,
            "column {}",
            result.columns[index]
        );
    }
}

#[tokio::test]
async fn live_first_result_set_wins_and_the_session_persists() {
    let Some(client) = live().await else { return };
    let result = client
        .execute_query_structured("SELECT 1 AS first_set; SELECT 2 AS second_set")
        .await
        .unwrap();
    assert_eq!(result.columns, vec!["first_set"]);
    // temp tables live as long as the session
    client
        .execute_query("CREATE TABLE #dbcrust_t (id int)")
        .await
        .unwrap();
    client
        .execute_query("INSERT INTO #dbcrust_t VALUES (1), (2)")
        .await
        .unwrap();
    let count = client
        .execute_query_structured("SELECT COUNT(*) FROM #dbcrust_t")
        .await
        .unwrap();
    assert_eq!(cell(&count, 0, 0), Some("2"));
}

#[tokio::test]
async fn live_dml_reports_rows_affected() {
    let Some(client) = live().await else { return };
    client
        .execute_query("CREATE TABLE #dbcrust_dml (id int)")
        .await
        .unwrap();
    let inserted = client
        .execute_query_structured("INSERT INTO #dbcrust_dml VALUES (1), (2), (3)")
        .await
        .unwrap();
    assert_eq!(inserted.columns, vec!["Rows affected"]);
    assert_eq!(cell(&inserted, 0, 0), Some("3"));
}

#[tokio::test]
async fn live_sql_errors_keep_the_session() {
    let Some(client) = live().await else { return };
    let error = client
        .execute_query("SELECT * FROM dbcrust_no_such_table")
        .await
        .unwrap_err();
    assert!(matches!(error, DatabaseError::QueryError(_)), "{error:?}");
    assert!(error.to_string().contains("dbcrust_no_such_table"));
    assert_eq!(
        client.execute_query("SELECT 1 AS ok").await.unwrap()[1][0],
        "1"
    );
}

#[tokio::test]
async fn live_timeout_cancels_and_the_session_survives() {
    let Some(client) = live().await else { return };
    dbcrust_core::database::set_query_timeout_seconds(1);
    let error = client.execute_query("WAITFOR DELAY '00:00:20'").await;
    dbcrust_core::database::set_query_timeout_seconds(30);
    let error = error.unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    assert_eq!(
        client.execute_query("SELECT 1 AS ok").await.unwrap()[1][0],
        "1"
    );
}

#[tokio::test]
async fn live_row_cap_stops_the_read() {
    let Some(client) = live().await else { return };
    let mut capped = ConnectionInfo::parse_url(&live_url().unwrap()).unwrap();
    capped
        .options
        .insert("max_rows".to_string(), "5".to_string());
    let client2 = MssqlClient::new(capped).await.unwrap();
    let result = client2
        .execute_query_structured("SELECT TOP (1000) n = ROW_NUMBER() OVER (ORDER BY (SELECT 1)) FROM sys.all_objects a CROSS JOIN sys.all_objects b")
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 5);
    // the cancelled session is usable again
    assert_eq!(client2.execute_query("SELECT 1").await.unwrap()[1][0], "1");
    drop(client);
}

#[tokio::test]
async fn live_metadata_round_trip() {
    let Some(client) = live().await else { return };
    client
        .execute_query("CREATE TABLE dbo.dbcrust_parent (id int NOT NULL PRIMARY KEY, name nvarchar(50) NOT NULL DEFAULT (N'x'))")
        .await
        .ok();
    client
        .execute_query("CREATE TABLE dbo.dbcrust_child (id int IDENTITY(1,1) PRIMARY KEY, parent_id int NOT NULL CONSTRAINT fk_dbcrust_child FOREIGN KEY REFERENCES dbo.dbcrust_parent(id), qty decimal(10,2) NULL CHECK (qty >= 0))")
        .await
        .ok();
    let metadata = client.get_metadata_provider();
    assert!(
        metadata
            .get_schemas()
            .await
            .unwrap()
            .contains(&"dbo".to_string())
    );
    let tables = metadata.get_tables(None).await.unwrap();
    assert!(tables.contains(&"dbcrust_child".to_string()), "{tables:?}");
    let columns = metadata.get_columns("dbcrust_child", None).await.unwrap();
    assert_eq!(columns, vec!["id", "parent_id", "qty"]);
    let details = metadata
        .get_table_details("dbcrust_child", None)
        .await
        .unwrap();
    assert_eq!(
        details.columns[0].default_value.as_deref(),
        Some("IDENTITY(1,1)")
    );
    assert_eq!(details.columns[2].data_type, "decimal(10,2)");
    assert!(details.columns[2].nullable);
    assert!(details.indexes.iter().any(|i| i.is_primary));
    assert_eq!(details.foreign_keys.len(), 1);
    assert!(
        details.foreign_keys[0]
            .definition
            .contains("REFERENCES [dbo].[dbcrust_parent]")
    );
    assert_eq!(details.check_constraints.len(), 1);
    let parent = metadata
        .get_table_details("dbcrust_parent", Some("dbo"))
        .await
        .unwrap();
    assert_eq!(parent.columns[1].default_value.as_deref(), Some("N'x'"));
    assert_eq!(parent.referenced_by.len(), 1);
    for cleanup in [
        "DROP TABLE dbo.dbcrust_child",
        "DROP TABLE dbo.dbcrust_parent",
    ] {
        client.execute_query(cleanup).await.ok();
    }
}

#[tokio::test]
async fn live_explain_returns_the_plan_without_running_the_statement() {
    let Some(client) = live().await else { return };
    client
        .execute_query("CREATE TABLE #dbcrust_plan (id int)")
        .await
        .unwrap();
    let plan = client
        .explain_query("INSERT INTO #dbcrust_plan VALUES (1)")
        .await
        .unwrap();
    assert!(plan[1][0].contains("ShowPlanXML"), "{plan:?}");
    let count = client
        .execute_query_structured("SELECT COUNT(*) FROM #dbcrust_plan")
        .await
        .unwrap();
    assert_eq!(cell(&count, 0, 0), Some("0"));
    // the session left SHOWPLAN mode
    assert_eq!(
        client.execute_query("SELECT 7 AS n").await.unwrap()[1][0],
        "7"
    );
    let shorthand = client.execute_query("EXPLAIN SELECT 1").await.unwrap();
    assert!(shorthand[1][0].contains("ShowPlanXML"));
}

#[tokio::test]
async fn live_server_info_databases_and_use() {
    let Some(mut client) = live().await else {
        return;
    };
    let server = client.get_server_info().await.unwrap();
    assert_eq!(server.server_type, "SQL Server");
    assert!(server.version_major.is_some());
    let databases = client.list_databases().await.unwrap();
    assert!(databases.iter().any(|row| row[0] == "master"));
    client.connect_to_database("tempdb").await.unwrap();
    assert_eq!(client.get_current_database(), "tempdb");
    client.execute_query("USE master").await.unwrap();
    assert_eq!(client.get_current_database(), "master");
    assert!(client.is_connected().await);
    client.test_query("SELECT 1").await.unwrap();
    assert!(client.test_query("SELECT FROM WHERE").await.is_err());
    assert_eq!(client.execute_query("SELECT 5").await.unwrap()[1][0], "5");
    client.close().await.unwrap();
}

#[tokio::test]
async fn live_read_only_connections_refuse_writes_before_sending() {
    let Some(_client) = live().await else { return };
    let mut hardened = ConnectionInfo::parse_url(&live_url().unwrap()).unwrap();
    hardened.options.insert(
        dbcrust_core::database::OPT_HARDEN_READ_ONLY.to_string(),
        "1".to_string(),
    );
    assert_eq!(hardened.database_type, DatabaseType::SqlServer);
    let client = MssqlClient::new(hardened).await.unwrap();
    let error = client
        .execute_query("CREATE TABLE dbo.dbcrust_should_not_exist (a int)")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("read-only mode"), "{error}");
    assert_eq!(client.execute_query("SELECT 1").await.unwrap()[1][0], "1");
}

#[tokio::test]
async fn live_unverified_certificates_are_refused_with_a_hint() {
    let Some(url) = live_url() else { return };
    let mut strict = ConnectionInfo::parse_url(&url).unwrap();
    strict.options.remove("trustServerCertificate");
    // A development server with a self-signed certificate must be refused,
    // naming the two ways forward; a server with a valid chain just connects
    if let Err(error) = MssqlClient::new(strict).await {
        let text = error.to_string();
        assert!(text.contains("trustServerCertificate=true"), "{text}");
        // pinning a CA, or why pinning cannot help (SQL Server's own v1 certificate)
        assert!(text.contains("ca_cert") || text.contains("X.509"), "{text}");
    }
}

#[tokio::test]
async fn live_bad_passwords_are_reported_without_echoing_them() {
    let Some(url) = live_url() else { return };
    let mut wrong = ConnectionInfo::parse_url(&url).unwrap();
    wrong.password = Some("Wr0ng-Canary-9f2".to_string());
    let Err(error) = MssqlClient::new(wrong).await else {
        panic!("a wrong password must not connect");
    };
    let text = error.to_string();
    assert!(text.to_ascii_lowercase().contains("login failed"), "{text}");
    assert!(!text.contains("Wr0ng-Canary-9f2"), "{text}");
}

#[tokio::test]
async fn live_unencrypted_login_only_mode_connects() {
    let Some(url) = live_url() else { return };
    let mut plain = ConnectionInfo::parse_url(&url).unwrap();
    plain
        .options
        .insert("encrypt".to_string(), "false".to_string());
    plain.use_tls = false;
    let client = MssqlClient::new(plain).await.unwrap();
    assert_eq!(client.execute_query("SELECT 1").await.unwrap()[1][0], "1");
}
