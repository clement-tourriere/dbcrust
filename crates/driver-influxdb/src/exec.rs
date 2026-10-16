//! Sending a query: one request per statement, shaped by the API generation
//! and the language, streamed into a table under the row and byte budget.

use crate::conn::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_ROWS, Http, OPT_MAX_BYTES, OPT_MAX_ROWS, Sink, error_message,
    limit_option, status_error, stream_body,
};
use crate::csv::CsvTableSink;
use crate::flux::FluxSink;
use crate::influxql::InfluxQlSink;
use crate::table::Table;
use dbcrust_core::database::{ConnectionInfo, DatabaseError, InfluxLanguage, influxdb_version};
use dbcrust_core::safety::check_influxdb_read_only;
use reqwest::{Method, Response, StatusCode};
use std::sync::{Mutex, RwLock};
use std::time::Instant;

/// Rows per JSON document of a chunked InfluxQL reply.
const CHUNK_SIZE: usize = 5_000;

/// Everything needed to talk to one InfluxDB server.
pub(crate) struct Engine {
    pub(crate) http: Http,
    pub(crate) version: u8,
    pub(crate) language: InfluxLanguage,
    pub(crate) org: Option<String>,
    rp: Option<String>,
    epoch: Option<String>,
    /// Flux duration the schema functions look back (`schema_window`).
    pub(crate) schema_window: String,
    pub(crate) max_rows: usize,
    pub(crate) max_bytes: usize,
    database: RwLock<Option<String>>,
    server_version: Mutex<String>,
}

fn option<'a>(info: &'a ConnectionInfo, key: &str) -> Option<&'a str> {
    info.options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

impl Engine {
    pub(crate) fn new(info: &ConnectionInfo) -> Result<Self, DatabaseError> {
        let version = influxdb_version(&info.options);
        let language = InfluxLanguage::from_options(&info.options);
        let org = option(info, "org").map(str::to_string);
        if version == 2 && org.is_none() {
            return Err(DatabaseError::InvalidUrl(
                "InfluxDB v2 needs the organization: add ?org=<name> to the URL".to_string(),
            ));
        }
        Ok(Self {
            http: Http::new(info)?,
            version,
            language,
            org,
            rp: option(info, "rp").map(str::to_string),
            epoch: option(info, "epoch").map(str::to_string),
            schema_window: option(info, "schema_window").unwrap_or("-30d").to_string(),
            max_rows: limit_option(info, OPT_MAX_ROWS, DEFAULT_MAX_ROWS),
            max_bytes: limit_option(info, OPT_MAX_BYTES, DEFAULT_MAX_BYTES),
            database: RwLock::new(info.database.clone()),
            server_version: Mutex::new(String::new()),
        })
    }

    pub(crate) fn database(&self) -> Option<String> {
        self.database.read().expect("database lock").clone()
    }

    pub(crate) fn set_database(&self, name: &str) {
        *self.database.write().expect("database lock") = Some(name.to_string());
    }

    pub(crate) fn server_version(&self) -> String {
        self.server_version.lock().expect("version lock").clone()
    }

    fn remember_version(&self, version: &str) {
        *self.server_version.lock().expect("version lock") = version.to_string();
    }

    /// Run `text` in the connection's language against the current database.
    pub(crate) async fn run_user(&self, text: &str) -> Result<Table, DatabaseError> {
        self.run(self.language, text, self.database().as_deref())
            .await
    }

    pub(crate) async fn run(
        &self,
        language: InfluxLanguage,
        text: &str,
        database: Option<&str>,
    ) -> Result<Table, DatabaseError> {
        let started = Instant::now();
        match (self.version, language) {
            (2, InfluxLanguage::Flux) => self.run_flux(text, started).await,
            (3, _) => self.run_v3(text, database, language, started).await,
            _ => self.run_influxql(text, database, started).await,
        }
    }

    async fn read_table(
        &self,
        response: Response,
        sink: &mut dyn Sink,
        started: Instant,
    ) -> Result<bool, DatabaseError> {
        let end = stream_body(response, sink, self.max_bytes, started).await?;
        Ok(end.sink_stopped || end.byte_limit_hit)
    }

    /// Fail on a non-success status with the server's own message.
    async fn checked(
        &self,
        response: Response,
        started: Instant,
    ) -> Result<Response, DatabaseError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let message = error_message(response, started).await;
        Err(status_error(status, &message, false))
    }

    /// v1 (and v2's 1.x-compatible) `/query`: reads go as `GET`, the method the
    /// API documents for `SELECT` and `SHOW`; anything else (only reachable
    /// when `--read-only` is off) is sent as `POST`. InfluxDB 1.x does **not**
    /// enforce this split: a write sent as `GET` still runs, with only a
    /// "deprecated use … in a read only context" warning in the reply (which
    /// [`InfluxQlSink`] turns into a notice). The statement classifier and a
    /// read-only user are the guards, not the verb.
    async fn run_influxql(
        &self,
        text: &str,
        database: Option<&str>,
        started: Instant,
    ) -> Result<Table, DatabaseError> {
        let is_read = check_influxdb_read_only(text, InfluxLanguage::InfluxQl).is_ok();
        let mut params: Vec<(&str, String)> = vec![
            ("chunked", "true".to_string()),
            (
                "chunk_size",
                CHUNK_SIZE.min(self.max_rows).max(1).to_string(),
            ),
        ];
        if let Some(database) = database {
            params.push(("db", database.to_string()));
        }
        if let Some(rp) = &self.rp {
            params.push(("rp", rp.clone()));
        }
        if let Some(epoch) = &self.epoch {
            params.push(("epoch", epoch.clone()));
        }
        let request = if is_read {
            params.push(("q", text.to_string()));
            self.http.request(Method::GET, "/query").query(&params)
        } else {
            self.http
                .request(Method::POST, "/query")
                .query(&params)
                .form(&[("q", text)])
        };
        let response = self.http.send(request, started).await?;
        let response = self.checked(response, started).await?;
        let mut sink = InfluxQlSink::new(self.max_rows);
        let cut = self.read_table(response, &mut sink, started).await?;
        let mut table = sink.into_table();
        table.truncated |= cut;
        Ok(table)
    }

    /// v2 `/api/v2/query` (Flux) with annotated CSV back.
    async fn run_flux(&self, text: &str, started: Instant) -> Result<Table, DatabaseError> {
        let org = self.org.as_deref().ok_or_else(|| {
            DatabaseError::QueryError(
                "InfluxDB v2 needs the organization: add ?org=<name> to the URL".to_string(),
            )
        })?;
        let body = serde_json::json!({
            "query": text,
            "type": "flux",
            "dialect": {
                "annotations": ["datatype", "group", "default"],
                "header": true,
                "delimiter": ",",
                "commentPrefix": "#",
                "dateTimeFormat": "RFC3339",
            },
        })
        .to_string();
        let request = self
            .http
            .request(Method::POST, "/api/v2/query")
            .query(&[("org", org)])
            .header("Accept", "application/csv")
            .header("Content-Type", "application/json")
            .body(body);
        let response = self.http.send(request, started).await?;
        let response = self.checked(response, started).await?;
        let mut sink = FluxSink::new(self.max_rows);
        let cut = self.read_table(response, &mut sink, started).await?;
        let mut table = sink.into_table();
        table.truncated |= cut;
        Ok(table)
    }

    /// v3 `query_sql` / `query_influxql`, CSV back (the order of the columns is
    /// kept, which JSON objects would lose).
    async fn run_v3(
        &self,
        text: &str,
        database: Option<&str>,
        language: InfluxLanguage,
        started: Instant,
    ) -> Result<Table, DatabaseError> {
        let database = database.ok_or_else(|| {
            DatabaseError::QueryError(
                "InfluxDB v3 needs a database: connect to one (/<database> in the URL, or \\c)"
                    .to_string(),
            )
        })?;
        let path = if language == InfluxLanguage::InfluxQl {
            "/api/v3/query_influxql"
        } else {
            "/api/v3/query_sql"
        };
        let body = serde_json::json!({"db": database, "q": text, "format": "csv"}).to_string();
        let request = self
            .http
            .request(Method::POST, path)
            .header("Content-Type", "application/json")
            .body(body);
        let response = self.http.send(request, started).await?;
        let response = self.checked(response, started).await?;
        let mut sink = CsvTableSink::new(self.max_rows);
        let cut = self.read_table(response, &mut sink, started).await?;
        let mut table = sink.into_table();
        table.truncated |= cut;
        Ok(table)
    }

    /// Reach the server, learn its version and prove the credentials work.
    pub(crate) async fn connect_check(&self) -> Result<(), DatabaseError> {
        let target = self.http.base().to_string();
        match self.version {
            3 => {
                let response = self.http.probe("/ping").await?;
                let body = response.text().await.unwrap_or_default();
                if let Some(version) = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.get("version")
                            .and_then(|v| v.as_str().map(str::to_string))
                    })
                {
                    self.remember_version(&version);
                }
                // /ping needs no token: an authenticated read proves it
                match self.database() {
                    Some(database) => {
                        match self
                            .run(InfluxLanguage::Sql, "SELECT 1", Some(&database))
                            .await
                        {
                            Ok(_) => {}
                            Err(DatabaseError::ConnectionError(message)) => {
                                return Err(DatabaseError::ConnectionError(message));
                            }
                            Err(error) => {
                                return Err(DatabaseError::ConnectionError(format!(
                                    "InfluxDB at {target}: database '{database}' is not usable: {error}"
                                )));
                            }
                        }
                    }
                    None => {
                        let response = self
                            .http
                            .probe("/api/v3/configure/database?format=json")
                            .await?;
                        let status = response.status();
                        if status == StatusCode::UNAUTHORIZED {
                            let message = error_message(response, Instant::now()).await;
                            return Err(status_error(status, &message, true));
                        }
                    }
                }
            }
            2 => {
                let response = self.http.probe("/health").await?;
                let status = response.status();
                if !status.is_success() {
                    let message = error_message(response, Instant::now()).await;
                    return Err(status_error(status, &message, true));
                }
                let body = response.text().await.unwrap_or_default();
                if let Some(version) = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.get("version")
                            .and_then(|v| v.as_str().map(str::to_string))
                    })
                {
                    self.remember_version(&version);
                }
                let response = self.http.probe("/api/v2/me").await?;
                if response.status() == StatusCode::UNAUTHORIZED {
                    let message = error_message(response, Instant::now()).await;
                    return Err(status_error(StatusCode::UNAUTHORIZED, &message, true));
                }
            }
            _ => {
                let response = self.http.probe("/ping").await?;
                let status = response.status();
                if !status.is_success() {
                    let message = error_message(response, Instant::now()).await;
                    return Err(status_error(status, &message, true));
                }
                if let Some(version) = response
                    .headers()
                    .get("x-influxdb-version")
                    .and_then(|v| v.to_str().ok())
                {
                    self.remember_version(version);
                }
                // /ping needs no login: an authenticated read proves it
                let response = self
                    .http
                    .request(Method::GET, "/query")
                    .query(&[("q", "SHOW DATABASES")])
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await
                    .map_err(|error| crate::conn::transport_error(&target, error))?;
                if response.status() == StatusCode::UNAUTHORIZED {
                    let message = error_message(response, Instant::now()).await;
                    return Err(status_error(StatusCode::UNAUTHORIZED, &message, true));
                }
            }
        }
        Ok(())
    }

    /// Whether the server answers its ping.
    pub(crate) async fn ping(&self) -> bool {
        let path = if self.version == 2 {
            "/health"
        } else {
            "/ping"
        };
        matches!(self.http.probe(path).await, Ok(r) if r.status().is_success())
    }
}
