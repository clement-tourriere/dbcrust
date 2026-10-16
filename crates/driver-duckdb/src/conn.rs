//! The connection layer: open the database from a [`ConnectionInfo`] on a
//! dedicated thread and run statements there with the process-wide timeout
//! and interrupt flag honoured.
//!
//! The `duckdb` crate is synchronous and its `Connection` is not `Sync`, so one
//! OS thread owns the connection for the whole session (`SET`, temp tables,
//! `USE` and open transactions persist like in the `duckdb` shell) and the async
//! side talks to it over a channel. A statement that outlives its deadline (or
//! Ctrl-C) is stopped with DuckDB's own `interrupt`, which the engine honours
//! at its next checkpoint; the session stays usable afterwards.

use crate::value::append_rows;
use dbcrust_core::database::{
    ConnectionInfo, DatabaseError, connection_read_only, connection_timeout_secs, interrupt_flag,
    query_timeout,
};
use dbcrust_core::password_sanitizer::redact_text;
use duckdb::{AccessMode, Config, Connection, InterruptHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{debug, warn};

/// Connection option: rows kept from one result set (0 = unlimited).
pub(crate) const OPT_MAX_ROWS: &str = "max_rows";
pub(crate) const DEFAULT_MAX_ROWS: usize = 100_000;
/// How often a running statement re-checks the interrupt flag and deadline.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long an interrupted statement gets to unwind before the caller moves on.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
/// The bound on a metadata query (it is not subject to the user's timeout).
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);

/// The in-memory database spelled as a path.
pub(crate) const MEMORY_PATH: &str = ":memory:";

/// Option value (case-insensitive key lookup).
pub(crate) fn option<'a>(info: &'a ConnectionInfo, key: &str) -> Option<&'a str> {
    info.options
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
}

fn flag(info: &ConnectionInfo, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        option(info, key).is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        })
    })
}

/// Whether `path` names the in-memory database.
pub(crate) fn is_memory_path(path: Option<&str>) -> bool {
    path.is_none_or(|path| {
        let path = path.trim();
        path.is_empty() || path == MEMORY_PATH || path == "/:memory:"
    })
}

/// An identifier-like extension name (`ducklake`, `httpfs`): the only text that
/// ever reaches `INSTALL`/`LOAD`.
pub(crate) fn valid_extension_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A DuckDB size such as `2GB`, `512MiB` or `80%`.
fn valid_memory_limit(value: &str) -> bool {
    let value = value.trim();
    let digits = value
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%' || c == ' ')
        .trim();
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
        && value[digits.len()..]
            .trim()
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '%')
}

/// A single-quoted SQL string literal.
pub(crate) fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Everything the worker needs to open the database, validated up front so a
/// bad URL fails before a thread exists.
#[derive(Debug, Clone)]
pub(crate) struct OpenSpec {
    /// `None` = in-memory.
    pub path: Option<String>,
    /// `access_mode=READ_ONLY` for the file.
    pub read_only_file: bool,
    /// The full hardened profile: read-only file, no external access, locked
    /// configuration, no extension install or autoload.
    pub hardened: bool,
    pub memory_limit: Option<String>,
    pub threads: Option<i64>,
    pub allowed_directories: Vec<String>,
    pub install_extensions: bool,
    pub extensions: Vec<String>,
    /// Statements run once after opening (the DuckLake attach), before any
    /// configuration lock; never present on a hardened connection.
    pub setup: Vec<String>,
}

impl OpenSpec {
    pub(crate) fn from_info(info: &ConnectionInfo) -> Result<Self, DatabaseError> {
        let invalid = |message: String| DatabaseError::InvalidUrl(message);
        let hardened = connection_read_only(&info.options);
        let path = info
            .file_path
            .as_deref()
            .filter(|path| !is_memory_path(Some(path)))
            .map(str::to_string);
        let read_only_file = hardened
            || flag(info, &["read_only", "readonly"])
            || option(info, "access_mode")
                .is_some_and(|mode| mode.trim().eq_ignore_ascii_case("read_only"));
        if let Some(mode) = option(info, "access_mode") {
            if !matches!(
                mode.trim().to_ascii_lowercase().as_str(),
                "read_only" | "read_write" | "automatic"
            ) {
                return Err(invalid(format!(
                    "unknown access_mode '{mode}' (use read_only, read_write or automatic)"
                )));
            }
        }
        if path.is_none() && read_only_file {
            return Err(DatabaseError::ConnectionError(if hardened {
                "read-only mode needs a DuckDB database file: an in-memory database cannot be \
                 opened read-only (use duckdb:///path/to/file.duckdb)"
                    .to_string()
            } else {
                "an in-memory DuckDB database cannot be opened read-only".to_string()
            }));
        }

        let memory_limit = option(info, "memory_limit")
            .or_else(|| option(info, "max_memory"))
            .map(str::to_string);
        if let Some(limit) = &memory_limit {
            if !valid_memory_limit(limit) {
                return Err(invalid(format!(
                    "invalid memory_limit '{limit}' (use a size such as 2GB or 512MiB)"
                )));
            }
        }
        let threads = match option(info, "threads") {
            Some(value) => Some(
                value
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .filter(|threads| *threads > 0)
                    .ok_or_else(|| {
                        invalid(format!("invalid threads '{value}' (use a positive number)"))
                    })?,
            ),
            None => None,
        };

        let allowed_directories: Vec<String> = option(info, "allowed_directories")
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|dir| !dir.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let install_extensions = flag(info, &["install_extensions"]);
        let mut extensions: Vec<String> = Vec::new();
        if let Some(list) = option(info, "extensions") {
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                if !valid_extension_name(name) {
                    return Err(invalid(format!("invalid extension name '{name}'")));
                }
                extensions.push(name.to_ascii_lowercase());
            }
        }

        let mut setup = Vec::new();
        if let Some(catalog) = option(info, "ducklake") {
            if hardened {
                return Err(DatabaseError::ConnectionError(
                    "a DuckLake catalog cannot be attached in read-only mode: it needs the \
                     `ducklake` extension, which a hardened connection never installs or loads"
                        .to_string(),
                ));
            }
            setup = crate::ducklake::attach_statements(info, catalog)?;
            let needed = ["ducklake"]
                .into_iter()
                .chain(crate::ducklake::catalog_extension(catalog));
            for name in needed {
                if !extensions.iter().any(|loaded| loaded == name) {
                    extensions.push(name.to_string());
                }
            }
        }
        if hardened && !extensions.is_empty() {
            return Err(DatabaseError::ConnectionError(
                "extensions cannot be loaded in read-only mode: a hardened DuckDB connection \
                 never installs or loads extensions"
                    .to_string(),
            ));
        }

        Ok(Self {
            path,
            read_only_file,
            hardened,
            memory_limit,
            threads,
            allowed_directories,
            install_extensions,
            extensions,
            setup,
        })
    }

    /// The DuckDB configuration for this spec.
    ///
    /// The hardened profile is all-or-nothing and applied here, at open time,
    /// where DuckDB accepts it: a read-only file, external access off (no
    /// `COPY … TO`, no `read_csv('/etc/passwd')`, no `ATTACH`, no
    /// `INSTALL`/`LOAD`), and the configuration locked so no statement can
    /// turn any of that back on.
    fn config(&self) -> Result<Config, DatabaseError> {
        let bad = |error: duckdb::Error| {
            DatabaseError::ConnectionError(format!("DuckDB configuration: {error}"))
        };
        let mut config = Config::default();
        if self.read_only_file {
            config = config.access_mode(AccessMode::ReadOnly).map_err(bad)?;
        }
        if let Some(limit) = &self.memory_limit {
            config = config.max_memory(limit).map_err(bad)?;
        }
        if let Some(threads) = self.threads {
            config = config.threads(threads).map_err(bad)?;
        }
        if self.hardened {
            config = config
                .with("autoinstall_known_extensions", "false")
                .map_err(bad)?
                .with("autoload_known_extensions", "false")
                .map_err(bad)?
                .with("allow_community_extensions", "false")
                .map_err(bad)?
                .with("allow_unsigned_extensions", "false")
                .map_err(bad)?;
            // `allowed_directories` is a runtime-only setting: with folders to
            // allow, the two switches below are thrown right after opening
            // (see `lock_down`), before the worker answers anything
            if self.allowed_directories.is_empty() {
                config = config
                    .enable_external_access(false)
                    .map_err(bad)?
                    .with("lock_configuration", "true")
                    .map_err(bad)?;
            }
        } else {
            // Extensions come from the local extension directory unless the
            // user opted in to downloads (`install_extensions=true`)
            config = config
                .with(
                    "autoinstall_known_extensions",
                    if self.install_extensions {
                        "true"
                    } else {
                        "false"
                    },
                )
                .map_err(bad)?;
        }
        Ok(config)
    }
}

/// One statement's result, already as text cells.
#[derive(Debug, Default)]
pub(crate) struct Batch {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

struct Request {
    sql: String,
    params: Vec<String>,
    max_rows: usize,
    reply: oneshot::Sender<Result<Batch, String>>,
}

enum Job {
    Run(Request),
    Shutdown,
}

/// The session: a worker thread owning the connection, and the handle that
/// interrupts whatever it is running.
pub(crate) struct Session {
    jobs: StdMutex<Option<Sender<Job>>>,
    thread: StdMutex<Option<std::thread::JoinHandle<()>>>,
    /// Signalled by the worker once its connection is dropped
    finished: StdMutex<Option<Receiver<()>>>,
    interrupt: Arc<InterruptHandle>,
    info: StdMutex<ConnectionInfo>,
    spec: OpenSpec,
}

/// Why a statement stopped before it finished.
enum Stop {
    Interrupted,
    TimedOut(Duration),
}

impl Session {
    /// Open the database (on the worker thread) and run the setup statements.
    pub(crate) async fn open(info: &ConnectionInfo) -> Result<Self, DatabaseError> {
        let spec = OpenSpec::from_info(info)?;
        debug!(
            "[DuckDB] opening {} (read-only file: {}, hardened: {})",
            spec.path.as_deref().unwrap_or(MEMORY_PATH),
            spec.read_only_file,
            spec.hardened
        );
        let (jobs, inbox) = channel::<Job>();
        let (finished_tx, finished_rx) = channel::<()>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<Arc<InterruptHandle>, String>>();
        let worker_spec = spec.clone();
        let thread = std::thread::Builder::new()
            .name("dbcrust-duckdb".to_string())
            .spawn(move || worker(worker_spec, inbox, ready_tx, finished_tx))
            .map_err(|e| {
                DatabaseError::ConnectionError(format!("could not start the DuckDB thread: {e}"))
            })?;
        let interrupt = match ready_rx.await {
            Ok(Ok(handle)) => handle,
            Ok(Err(message)) => return Err(DatabaseError::ConnectionError(redact_text(&message))),
            Err(_) => {
                return Err(DatabaseError::ConnectionError(
                    "the DuckDB thread stopped while opening the database".to_string(),
                ));
            }
        };
        Ok(Self {
            jobs: StdMutex::new(Some(jobs)),
            thread: StdMutex::new(Some(thread)),
            finished: StdMutex::new(Some(finished_rx)),
            interrupt,
            info: StdMutex::new(info.clone()),
            spec,
        })
    }

    pub(crate) fn spec(&self) -> &OpenSpec {
        &self.spec
    }

    pub(crate) fn info(&self) -> ConnectionInfo {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn set_database(&self, database: &str) {
        self.info.lock().unwrap_or_else(|e| e.into_inner()).database = Some(database.to_string());
    }

    /// The row cap of one result set (`max_rows`, 0 = none).
    pub(crate) fn max_rows(&self) -> usize {
        let info = self.info();
        option(&info, OPT_MAX_ROWS)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_ROWS)
    }

    /// The deadline of one statement: the shorter of the CLI query timeout
    /// and the broker-style hardening timeout.
    fn deadline(&self) -> Option<Duration> {
        let hardened = connection_timeout_secs(&self.info().options).map(Duration::from_secs);
        match (query_timeout(), hardened) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn send(&self, job: Job) -> Result<(), DatabaseError> {
        let guard = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .and_then(|jobs| jobs.send(job).ok())
            .ok_or_else(|| DatabaseError::ConnectionError("the DuckDB session is closed".into()))
    }

    /// Run `sql` as the user's statement: row cap, timeout and Ctrl-C apply.
    pub(crate) async fn run(&self, sql: &str) -> Result<Batch, DatabaseError> {
        let deadline = self.deadline();
        let max_rows = self.max_rows();
        self.dispatch(sql, Vec::new(), max_rows, deadline, true)
            .await
    }

    /// Run a metadata query with `?` parameters: bounded by a fixed deadline
    /// rather than the user's timeout, and not cut by Ctrl-C.
    pub(crate) async fn query_params(
        &self,
        sql: &str,
        params: &[&str],
    ) -> Result<Vec<Vec<Option<String>>>, DatabaseError> {
        let owned = params.iter().map(|p| (*p).to_string()).collect();
        self.dispatch(sql, owned, 0, Some(METADATA_TIMEOUT), false)
            .await
            .map(|batch| batch.rows)
            .map_err(|error| match error {
                DatabaseError::QueryError(message) => DatabaseError::MetadataError(message),
                other => other,
            })
    }

    async fn dispatch(
        &self,
        sql: &str,
        params: Vec<String>,
        max_rows: usize,
        deadline: Option<Duration>,
        interruptible: bool,
    ) -> Result<Batch, DatabaseError> {
        let (reply, mut answer) = oneshot::channel();
        self.send(Job::Run(Request {
            sql: sql.to_string(),
            params,
            max_rows,
            reply,
        }))?;

        let interrupt = interrupt_flag();
        let started = tokio::time::Instant::now();
        let mut stop: Option<Stop> = None;
        let outcome = loop {
            tokio::select! {
                done = &mut answer => break Some(done),
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    if interruptible && interrupt.load(Ordering::Relaxed) {
                        stop = Some(Stop::Interrupted);
                        break None;
                    }
                    if let Some(limit) = deadline {
                        if started.elapsed() >= limit {
                            stop = Some(Stop::TimedOut(limit));
                            break None;
                        }
                    }
                }
            }
        };

        match (outcome, stop) {
            (Some(Ok(result)), _) => result.map_err(|m| query_error(&m)),
            (Some(Err(_)), _) => Err(DatabaseError::QueryError(
                "the DuckDB thread stopped unexpectedly".to_string(),
            )),
            (None, Some(reason)) => {
                self.interrupt.interrupt();
                // The engine unwinds at its next checkpoint; wait for it so the
                // next statement does not queue behind a still-running one
                let _ = tokio::time::timeout(INTERRUPT_GRACE, &mut answer).await;
                Err(DatabaseError::QueryError(match reason {
                    Stop::Interrupted => "query interrupted".to_string(),
                    Stop::TimedOut(limit) => format!(
                        "query timed out after {}s and was cancelled",
                        limit.as_secs()
                    ),
                }))
            }
            (None, None) => Err(DatabaseError::QueryError("query aborted".to_string())),
        }
    }

    /// Ask the worker to stop; the connection closes when its thread returns.
    fn shutdown(&self) {
        let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(jobs) = jobs {
            let _ = jobs.send(Job::Shutdown);
        }
    }

    /// Stop the worker and wait until the database is closed (its file lock
    /// released, its WAL checkpointed). Blocks: call it from a blocking
    /// context.
    pub(crate) fn close(&self) {
        self.shutdown();
        let thread = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(thread) = thread {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A statement still running when the session is dropped (quitting
        // during a long query) is stopped first
        self.interrupt.interrupt();
        self.shutdown();
        // The engine must be gone before the process is: an extension
        // (DuckLake…) torn down by process exit while the worker still holds the
        // database can crash it. Wait for the connection to close, but not
        // forever (a big WAL checkpoint runs on this thread's behalf)
        let finished = self
            .finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(finished) = finished {
            let _ = finished.recv_timeout(Duration::from_secs(5));
        }
    }
}

fn query_error(message: &str) -> DatabaseError {
    DatabaseError::QueryError(redact_text(message))
}

fn worker(
    spec: OpenSpec,
    inbox: Receiver<Job>,
    ready: oneshot::Sender<Result<Arc<InterruptHandle>, String>>,
    finished: Sender<()>,
) {
    let connection = match open_connection(&spec) {
        Ok(connection) => connection,
        Err(message) => {
            let _ = ready.send(Err(message));
            let _ = finished.send(());
            return;
        }
    };
    let _ = ready.send(Ok(connection.interrupt_handle()));

    while let Ok(job) = inbox.recv() {
        match job {
            Job::Run(request) => {
                let result = catch_unwind(AssertUnwindSafe(|| execute(&connection, &request)))
                    .unwrap_or_else(|_| Err("the DuckDB engine failed unexpectedly".to_string()));
                let _ = request.reply.send(result);
            }
            Job::Shutdown => break,
        }
    }
    // Close the database (WAL checkpoint, extensions unloaded) *before* telling
    // anyone the session is over
    drop(connection);
    let _ = finished.send(());
    debug!("[DuckDB] session thread finished");
}

fn open_connection(spec: &OpenSpec) -> Result<Connection, String> {
    let config = spec.config().map_err(|error| error.to_string())?;
    let connection = match &spec.path {
        Some(path) => Connection::open_with_flags(path, config),
        None => Connection::open_in_memory_with_flags(config),
    }
    .map_err(|error| format!("could not open the DuckDB database: {error}"))?;

    if spec.hardened && !spec.allowed_directories.is_empty() {
        lock_down(&connection, &spec.allowed_directories)?;
    }

    for name in &spec.extensions {
        let install = if spec.install_extensions {
            format!("INSTALL {name}; ")
        } else {
            String::new()
        };
        connection
            .execute_batch(&format!("{install}LOAD {name};"))
            .map_err(|error| {
                format!(
                    "could not load the DuckDB extension '{name}': {error}{}",
                    if spec.install_extensions {
                        ""
                    } else {
                        " (add install_extensions=true to download it, which needs network access)"
                    }
                )
            })?;
    }
    for statement in &spec.setup {
        connection
            .execute_batch(statement)
            .map_err(|error| format!("DuckDB setup statement failed: {error}"))?;
    }
    Ok(connection)
}

/// The hardened profile for a connection that may read some folders: allow
/// them, switch external access off, lock the configuration. Nothing else runs
/// on the connection before this returns. DuckDB lets a connection read *and
/// write* inside an allowed folder; that is why the option is for the owner of
/// the command line and is never accepted for broker aliases.
fn lock_down(connection: &Connection, directories: &[String]) -> Result<(), String> {
    let list = directories
        .iter()
        .map(|dir| sql_literal(dir))
        .collect::<Vec<_>>()
        .join(", ");
    connection
        .execute_batch(&format!(
            "SET allowed_directories = [{list}]; \
             SET enable_external_access = false; \
             SET lock_configuration = true;"
        ))
        .map_err(|error| format!("could not lock down the DuckDB connection: {error}"))
}

/// Run one request on the worker: stream the result chunk by chunk and stop at
/// the row cap, so an enormous `SELECT` never materialises in DBCrust.
fn execute(connection: &Connection, request: &Request) -> Result<Batch, String> {
    let mut statement = connection
        .prepare(&request.sql)
        .map_err(|error| error.to_string())?;
    let schema = {
        let stream = statement
            .stream_arrow(duckdb::params_from_iter(request.params.iter()))
            .map_err(|error| error.to_string())?;
        stream.get_schema()
    };
    let mut batch = Batch {
        columns: schema
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect(),
        ..Batch::default()
    };
    while let Some(chunk) = statement.step().map_err(|error| error.to_string())? {
        let record = duckdb::arrow::record_batch::RecordBatch::from(&chunk);
        append_rows(&record, &mut batch.rows);
        if request.max_rows > 0 && batch.rows.len() > request.max_rows {
            batch.rows.truncate(request.max_rows);
            warn!(
                "DuckDB result cut at {} rows (`max_rows` option; 0 = unlimited)",
                request.max_rows
            );
            break;
        }
    }
    Ok(batch)
}
