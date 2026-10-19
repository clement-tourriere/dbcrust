//! One worker per live connection.
//!
//! Each worker is an OS thread running a current-thread tokio runtime +
//! `LocalSet` that owns its `Database` for the connection's whole life:
//! DBCrust futures are `!Send`, and sqlx pools must stay on the runtime that
//! created them. The runtime keeps running while idle (it awaits the command
//! queue), so pool housekeeping is never starved, and the `Database` is
//! dropped on that runtime when the connection closes.
//!
//! Commands are processed one at a time (one active operation per
//! connection). Cancellation is out-of-band: every request gets a fresh
//! cancel token, installed as the worker thread's interrupt flag while it
//! runs (see `dbcrust_core::database::set_thread_interrupt_flag`). PostgreSQL
//! polls it to cancel server-side; other engines stop waiting for the
//! statement. A request cancelled while still queued never starts, and a
//! late cancel can never hit the next request (its token is different).

use crate::connect::{self, ConnectError, ConnectMode, Opened};
use crate::docker::{DockerFailure, DockerRoute};
use crate::error::{GuiError, GuiResult};
use crate::ids::{ConnectionId, RequestId};
use crate::schema;
use crate::store::ConfigStore;
use crate::types::{
    ConnectRequest, ConnectionSummary, ExplainRaw, ResultSet, TableDetails, TableEntry,
};
use dbcrust_app::db::Database;
use futures_util::FutureExt;
use std::collections::HashSet;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc, oneshot, watch};

pub use crate::connect::ConnectPhase;

type Reply<T> = oneshot::Sender<GuiResult<T>>;

enum Command {
    Execute {
        request: RequestId,
        sql: String,
        reply: Reply<ResultSet>,
    },
    Explain {
        request: RequestId,
        sql: String,
        analyze: bool,
        reply: Reply<ExplainRaw>,
    },
    ListTables {
        reply: Reply<Vec<TableEntry>>,
    },
    DescribeTable {
        table: String,
        reply: Reply<TableDetails>,
    },
    ListDatabases {
        reply: Reply<Vec<String>>,
    },
    SiblingInfo {
        database: String,
        reply: Reply<dbcrust_core::database::ConnectionInfo>,
    },
    SetReadOnly {
        enabled: bool,
        reply: Reply<()>,
    },
    SaveSession {
        name: String,
        reply: Reply<()>,
    },
    Close,
}

impl Command {
    /// Answer the command with `error` without running it.
    fn fail(self, error: GuiError) {
        match self {
            Command::Execute { reply, .. } => drop(reply.send(Err(error))),
            Command::Explain { reply, .. } => drop(reply.send(Err(error))),
            Command::ListTables { reply } => drop(reply.send(Err(error))),
            Command::DescribeTable { reply, .. } => drop(reply.send(Err(error))),
            Command::ListDatabases { reply } => drop(reply.send(Err(error))),
            Command::SiblingInfo { reply, .. } => drop(reply.send(Err(error))),
            Command::SetReadOnly { reply, .. } => drop(reply.send(Err(error))),
            Command::SaveSession { reply, .. } => drop(reply.send(Err(error))),
            Command::Close => {}
        }
    }
}

/// Shared between the UI-side handle and the worker thread.
struct Control {
    cancellation: Mutex<Cancellation>,
    /// Commands queued or running.
    pending: AtomicUsize,
    /// `close()` was called: queued commands are refused, new ones rejected.
    closing: AtomicBool,
    /// How a Docker connection reached its container (set once connected).
    route: Mutex<Option<DockerRoute>>,
    /// The Docker daemon a container connection was made through (its pin).
    daemon_pin: Mutex<Option<String>>,
}

/// Guarded by one mutex so "is it running?" and "remember it for later"
/// cannot interleave with the worker starting the request.
#[derive(Default)]
struct Cancellation {
    /// The running request and its cancel token.
    running: Option<(RequestId, Arc<AtomicBool>)>,
    /// Requests cancelled before they started.
    cancelled_before_start: HashSet<RequestId>,
}

impl Control {
    fn new() -> Self {
        Self {
            cancellation: Mutex::new(Cancellation::default()),
            pending: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            route: Mutex::new(None),
            daemon_pin: Mutex::new(None),
        }
    }

    fn cancellation(&self) -> MutexGuard<'_, Cancellation> {
        self.cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// UI-side handle to a live connection. Cheap to clone; all clones talk to
/// the same worker. Dropping every clone closes the connection.
#[derive(Clone)]
pub struct ConnectionHandle {
    id: ConnectionId,
    tx: mpsc::UnboundedSender<Command>,
    control: Arc<Control>,
}

impl std::fmt::Debug for ConnectionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionHandle")
            .field("id", &self.id)
            .finish()
    }
}

/// Identifies one connect attempt. Results, phases and failures of an
/// attempt carry it, so the UI can ignore what belongs to an abandoned one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectAttemptId(u64);

impl ConnectAttemptId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl std::fmt::Display for ConnectAttemptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "attempt-{}", self.0)
    }
}

/// Cancels a connect attempt: whatever it is doing (probing, opening a
/// relay, authenticating) is dropped, and the worker thread ends. Cheap to
/// clone; cancelling twice or after the attempt finished is harmless.
#[derive(Clone, Default)]
pub struct ConnectCancel {
    notify: Arc<Notify>,
}

impl ConnectCancel {
    pub fn cancel(&self) {
        // `notify_one` keeps a permit, so a cancel that arrives before the
        // worker starts waiting is not lost
        self.notify.notify_one();
    }

    async fn cancelled(&self) {
        self.notify.notified().await;
    }
}

impl std::fmt::Debug for ConnectCancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectCancel")
    }
}

/// A connect attempt that succeeded.
#[derive(Debug)]
pub struct Connected {
    pub attempt: ConnectAttemptId,
    /// The live connection; `None` for a test ([`ConnectionHandle::start_test`]),
    /// which closes the connection again.
    pub handle: Option<ConnectionHandle>,
    pub summary: ConnectionSummary,
    /// For Docker targets: the route that worked (direct address, OrbStack
    /// domain, or through Docker with the relay helper).
    pub route: Option<DockerRoute>,
}

/// A connect attempt that failed.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectFailure {
    pub attempt: ConnectAttemptId,
    /// The message to show; [`GuiError::Cancelled`] when the attempt was
    /// cancelled.
    pub error: GuiError,
    /// For a Docker container that no route reaches: the typed details
    /// (attempted routes, compose example) of the message.
    pub docker: Option<DockerFailure>,
}

pub type ConnectOutcome =
    Pin<Box<dyn Future<Output = Result<Connected, ConnectFailure>> + Send + 'static>>;

/// A running connect attempt: its identity, live progress, a way to cancel
/// it, and its eventual outcome. Destructure it; the pieces are independent.
///
/// Dropping `outcome` without awaiting it abandons the attempt (it is
/// cancelled and its thread ends).
pub struct ConnectAttempt {
    pub id: ConnectAttemptId,
    pub started: Instant,
    /// The current step. `phases.changed().await` wakes on every change;
    /// `*phases.borrow()` reads the current one. Closes when the attempt
    /// ends.
    pub phases: watch::Receiver<ConnectPhase>,
    pub cancel: ConnectCancel,
    pub outcome: ConnectOutcome,
}

/// What a worker thread needs to run one attempt.
struct ConnectJob {
    attempt: ConnectAttemptId,
    request: ConnectRequest,
    store: ConfigStore,
    mode: ConnectMode,
    cancel: ConnectCancel,
    phases: watch::Sender<ConnectPhase>,
    ready: oneshot::Sender<Result<(ConnectionSummary, Option<DockerRoute>), ConnectFailure>>,
}

impl ConnectionHandle {
    /// Start a worker and connect. Resolves once the connection is open (or
    /// failed); the worker thread exits on failure. Use
    /// [`ConnectionHandle::start_connect`] to follow the phases or cancel.
    pub fn connect(
        request: ConnectRequest,
        store: ConfigStore,
    ) -> impl Future<Output = GuiResult<(ConnectionHandle, ConnectionSummary)>> + Send + 'static
    {
        let outcome = Self::start_connect(request, store).outcome;
        async move {
            match outcome.await {
                Ok(connected) => connected
                    .handle
                    .map(|handle| (handle, connected.summary))
                    .ok_or(GuiError::Disconnected),
                Err(failure) => Err(failure.error),
            }
        }
    }

    /// Start connecting, with live phases and cancellation.
    pub fn start_connect(request: ConnectRequest, store: ConfigStore) -> ConnectAttempt {
        Self::start(request, store, ConnectMode::Open)
    }

    /// Test a connection: the same resolution, authentication, TLS and
    /// read-only setup as [`ConnectionHandle::start_connect`], then close.
    /// Nothing is recorded (no recent entry), no schema is loaded, and the
    /// outcome's `handle` is `None`.
    pub fn start_test(request: ConnectRequest, store: ConfigStore) -> ConnectAttempt {
        Self::start(request, store, ConnectMode::Test)
    }

    fn start(request: ConnectRequest, store: ConfigStore, mode: ConnectMode) -> ConnectAttempt {
        let attempt = ConnectAttemptId::next();
        let id = ConnectionId::next();
        let (tx, rx) = mpsc::unbounded_channel();
        let control = Arc::new(Control::new());
        let (ready_tx, ready_rx) = oneshot::channel();
        let (phases_tx, phases_rx) = watch::channel(ConnectPhase::Starting);
        let cancel = ConnectCancel::default();
        let job = ConnectJob {
            attempt,
            request,
            store,
            mode,
            cancel: cancel.clone(),
            phases: phases_tx,
            ready: ready_tx,
        };
        let worker_control = control.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("{}-{id}", crate::rt::THREAD_PREFIX))
            .spawn(move || run_worker(id, job, worker_control, rx));
        let handle = ConnectionHandle { id, tx, control };
        let outcome = async move {
            let fail = |error: GuiError| ConnectFailure {
                attempt,
                error,
                docker: None,
            };
            spawned.map_err(|e| {
                fail(GuiError::Connection(format!(
                    "Could not start a connection thread: {e}"
                )))
            })?;
            match ready_rx.await {
                Ok(Ok((summary, route))) => Ok(Connected {
                    attempt,
                    handle: (mode == ConnectMode::Open).then_some(handle),
                    summary,
                    route,
                }),
                Ok(Err(failure)) => Err(failure),
                Err(_) => Err(fail(GuiError::Disconnected)),
            }
        };
        ConnectAttempt {
            id: attempt,
            started: Instant::now(),
            phases: phases_rx,
            cancel,
            outcome: Box::pin(outcome),
        }
    }

    /// How a Docker connection reached its container, once connected.
    pub fn route(&self) -> Option<DockerRoute> {
        self.control
            .route
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// For a Docker connection: the daemon it was made through (the pin
    /// [`crate::docker::DockerDaemon`] requests carry). `None` for anything
    /// else.
    pub fn docker_daemon_pin(&self) -> Option<String> {
        self.control
            .daemon_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn id(&self) -> ConnectionId {
        self.id
    }

    /// Run one statement. The result's `auto_limit` tells whether DBCrust
    /// appended its default LIMIT.
    pub fn execute(
        &self,
        request: RequestId,
        sql: impl Into<String>,
    ) -> impl Future<Output = GuiResult<ResultSet>> + Send + 'static {
        let sql = sql.into();
        self.call(move |reply| Command::Execute {
            request,
            sql,
            reply,
        })
    }

    /// EXPLAIN a statement. `analyze` runs `EXPLAIN ANALYZE` (PostgreSQL
    /// only, and only for statements the read-only guard accepts — it
    /// executes the statement).
    pub fn explain(
        &self,
        request: RequestId,
        sql: impl Into<String>,
        analyze: bool,
    ) -> impl Future<Output = GuiResult<ExplainRaw>> + Send + 'static {
        let sql = sql.into();
        self.call(move |reply| Command::Explain {
            request,
            sql,
            analyze,
            reply,
        })
    }

    /// Tables, views, collections, … of the current database.
    pub fn list_tables(&self) -> impl Future<Output = GuiResult<Vec<TableEntry>>> + Send + 'static {
        self.call(|reply| Command::ListTables { reply })
    }

    /// Columns, indexes, constraints and references of one table
    /// (`schema.table` or `table`).
    pub fn describe_table(
        &self,
        table: impl Into<String>,
    ) -> impl Future<Output = GuiResult<TableDetails>> + Send + 'static {
        let table = table.into();
        self.call(move |reply| Command::DescribeTable { table, reply })
    }

    /// Databases on the server (engines with several databases).
    pub fn list_databases(&self) -> impl Future<Output = GuiResult<Vec<String>>> + Send + 'static {
        self.call(|reply| Command::ListDatabases { reply })
    }

    /// Open `database` on the same server as a new, independent connection
    /// (same credentials and read-only mode). Switching in place would
    /// bypass the connect-time hardening, so it is deliberately not offered.
    pub fn open_database(
        &self,
        database: impl Into<String>,
        read_only: bool,
        store: ConfigStore,
    ) -> impl Future<Output = GuiResult<(ConnectionHandle, ConnectionSummary)>> + Send + 'static
    {
        let database = database.into();
        let info = self.call(move |reply| Command::SiblingInfo { database, reply });
        async move {
            let info = info.await?;
            ConnectionHandle::connect(ConnectRequest::info(info).read_only(read_only), store).await
        }
    }

    /// Toggle DBCrust's write-statement guard on this connection. This does
    /// not re-harden already-open server connections; see
    /// [`ConnectRequest::read_only`] for that.
    pub fn set_read_only(
        &self,
        enabled: bool,
    ) -> impl Future<Output = GuiResult<()>> + Send + 'static {
        self.call(move |reply| Command::SetReadOnly { enabled, reply })
    }

    /// Save this connection as a named session (`sessions.toml`).
    pub fn save_session(
        &self,
        name: impl Into<String>,
    ) -> impl Future<Output = GuiResult<()>> + Send + 'static {
        let name = name.into();
        self.call(move |reply| Command::SaveSession { name, reply })
    }

    /// Cancel `request`: signal it if running, or make sure it never starts
    /// if still queued. Has no effect on any other request.
    pub fn cancel(&self, request: RequestId) {
        let mut cancellation = self.control.cancellation();
        match &cancellation.running {
            Some((running, token)) if *running == request => token.store(true, Ordering::SeqCst),
            _ if self.pending_operations() > 0 => {
                cancellation.cancelled_before_start.insert(request);
            }
            _ => {}
        }
    }

    /// Operations queued or running on this connection.
    pub fn pending_operations(&self) -> usize {
        self.control.pending.load(Ordering::SeqCst)
    }

    /// Close the connection now: the running statement is cancelled,
    /// queued operations are answered with [`GuiError::Disconnected`]
    /// without running, and the `Database` is dropped on its worker.
    pub fn close(&self) {
        self.control.closing.store(true, Ordering::SeqCst);
        if let Some((_, token)) = &self.control.cancellation().running {
            token.store(true, Ordering::SeqCst);
        }
        let _ = self.tx.send(Command::Close);
    }

    fn call<T: Send + 'static>(
        &self,
        make: impl FnOnce(Reply<T>) -> Command,
    ) -> impl Future<Output = GuiResult<T>> + Send + 'static {
        let (reply, response) = oneshot::channel();
        if self.control.closing.load(Ordering::SeqCst) {
            return futures_util::future::Either::Left(std::future::ready(Err(
                GuiError::Disconnected,
            )));
        }
        self.control.pending.fetch_add(1, Ordering::SeqCst);
        let sent = self.tx.send(make(reply)).is_ok();
        if !sent {
            self.control.pending.fetch_sub(1, Ordering::SeqCst);
        }
        futures_util::future::Either::Right(async move {
            if !sent {
                return Err(GuiError::Disconnected);
            }
            response.await.unwrap_or(Err(GuiError::Disconnected))
        })
    }
}

fn run_worker(
    id: ConnectionId,
    job: ConnectJob,
    control: Arc<Control>,
    mut rx: mpsc::UnboundedReceiver<Command>,
) {
    let ConnectJob {
        attempt,
        request,
        store,
        mode,
        cancel,
        phases,
        mut ready,
    } = job;
    let failure = |error: ConnectError| ConnectFailure {
        attempt,
        error: error.error,
        docker: error.docker.map(|failure| *failure),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = ready.send(Err(failure(
                GuiError::Connection(format!("Could not start a runtime: {e}")).into(),
            )));
            return;
        }
    };
    // Until a request installs its own token, nothing can cancel this thread
    dbcrust_core::database::set_thread_interrupt_flag(Some(Arc::new(AtomicBool::new(false))));
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let report = |phase: ConnectPhase| {
            phases.send_replace(phase);
        };
        // Cancelling, or the UI dropping the outcome, drops the connect
        // future: probes, a freshly opened relay and a half-done handshake
        // all stop here, and the thread ends.
        let opened = {
            let open =
                AssertUnwindSafe(connect::open(id, &request, &store, mode, &report)).catch_unwind();
            tokio::pin!(open);
            tokio::select! {
                result = &mut open => Some(result),
                _ = cancel.cancelled() => None,
                _ = ready.closed() => return,
            }
        };
        let opened = match opened {
            None => {
                let _ = ready.send(Err(ConnectFailure {
                    attempt,
                    error: GuiError::Cancelled { abandoned: false },
                    docker: None,
                }));
                return;
            }
            Some(result) => result.unwrap_or_else(|_| {
                Err(GuiError::Connection("Connecting crashed unexpectedly".into()).into())
            }),
        };
        let opened = match opened {
            Ok(opened) => opened,
            Err(e) => {
                let _ = ready.send(Err(failure(e)));
                return;
            }
        };
        if mode == ConnectMode::Test {
            // Same setup as a real connection, then close it (on this
            // runtime, so a relay is torn down before the thread exits)
            let Opened {
                database,
                summary,
                route,
                ..
            } = opened;
            drop(database);
            let _ = ready.send(Ok((summary, route)));
            return;
        }
        if let Ok(mut route) = control.route.lock() {
            *route = opened.route.clone();
        }
        if let Ok(mut pin) = control.daemon_pin.lock() {
            *pin = opened.database.docker_daemon_pin().map(str::to_string);
        }
        if ready
            .send(Ok((opened.summary.clone(), opened.route.clone())))
            .is_err()
        {
            return; // the UI gave up waiting
        }
        let mut worker = Worker {
            database: opened.database,
            summary: opened.summary,
            default_limit: opened.default_limit,
            store,
            control,
        };
        while let Some(command) = rx.recv().await {
            if matches!(command, Command::Close) {
                break;
            }
            if worker.control.closing.load(Ordering::SeqCst) {
                command.fail(GuiError::Disconnected);
                worker.control.pending.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let crashed = AssertUnwindSafe(worker.handle(command))
                .catch_unwind()
                .await
                .is_err();
            // A cancelled request's token must not leak into the next
            // command (metadata commands do not install their own)
            dbcrust_core::database::set_thread_interrupt_flag(Some(Arc::new(AtomicBool::new(
                false,
            ))));
            {
                let mut cancellation = worker.control.cancellation();
                cancellation.running = None;
                if worker.control.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                    // Queue drained: forget cancels of requests that already
                    // finished
                    cancellation.cancelled_before_start.clear();
                }
            }
            if crashed {
                tracing::error!("connection {id}: an operation panicked; closing the connection");
                break;
            }
        }
        // `worker` (and its Database) drops here, on its own runtime
    });
}

struct Worker {
    database: Database,
    summary: ConnectionSummary,
    default_limit: usize,
    store: ConfigStore,
    control: Arc<Control>,
}

impl Worker {
    async fn handle(&mut self, command: Command) {
        match command {
            Command::Execute {
                request,
                sql,
                reply,
            } => {
                let result = self.execute(request, sql).await;
                let _ = reply.send(result);
            }
            Command::Explain {
                request,
                sql,
                analyze,
                reply,
            } => {
                let result = self.explain(request, sql, analyze).await;
                let _ = reply.send(result);
            }
            Command::ListTables { reply } => {
                let result = match self.database.list_tables().await {
                    Ok(rows) => {
                        let mut tables = schema::parse_table_list(&self.summary.db_type, rows);
                        self.mark_views(&mut tables).await;
                        Ok(tables)
                    }
                    Err(e) => Err(GuiError::Query(format!("Could not list tables: {e}"))),
                };
                let _ = reply.send(result);
            }
            Command::DescribeTable { table, reply } => {
                let result = self
                    .database
                    .get_table_details(&table)
                    .await
                    .map_err(|e| GuiError::Query(format!("Could not describe {table}: {e}")));
                let _ = reply.send(result);
            }
            Command::ListDatabases { reply } => {
                let result = self
                    .database
                    .list_databases()
                    .await
                    .map(schema::parse_database_list)
                    .map_err(|e| GuiError::Query(format!("Could not list databases: {e}")));
                let _ = reply.send(result);
            }
            Command::SiblingInfo { database, reply } => {
                let result = self.sibling_info(&database);
                let _ = reply.send(result);
            }
            Command::SetReadOnly { enabled, reply } => {
                self.database.set_read_only(enabled);
                let _ = reply.send(Ok(()));
            }
            Command::SaveSession { name, reply } => {
                let result = self.save_session(&name);
                let _ = reply.send(result);
            }
            Command::Close => {}
        }
    }

    /// Start `request` with a fresh cancel token, installed as this
    /// thread's interrupt flag. Fails if it was cancelled while queued.
    fn begin(&self, request: RequestId) -> GuiResult<Arc<AtomicBool>> {
        let mut cancellation = self.control.cancellation();
        // Checked under the cancellation lock: `close()` sets `closing` and
        // then takes this lock, so either we see the flag here, or `close()`
        // sees this request as running and cancels it
        if self.control.closing.load(Ordering::SeqCst) {
            return Err(GuiError::Disconnected);
        }
        if cancellation.cancelled_before_start.remove(&request) {
            return Err(GuiError::Cancelled { abandoned: false });
        }
        let token = Arc::new(AtomicBool::new(false));
        dbcrust_core::database::set_thread_interrupt_flag(Some(token.clone()));
        cancellation.running = Some((request, token.clone()));
        Ok(token)
    }

    async fn execute(&mut self, request: RequestId, sql: String) -> GuiResult<ResultSet> {
        let cancel = self.begin(request)?;
        let started = Instant::now();
        let auto_limit = (self.database.maybe_add_limit(&sql) != sql).then_some(self.default_limit);
        let server_cancel = self.summary.supports_server_cancel();

        let outcome = {
            let query = self.database.execute_query_structured(&sql);
            if server_cancel {
                Some(query.await)
            } else {
                stop_when_cancelled(query, &cancel).await
            }
        };
        let elapsed = started.elapsed();

        match outcome {
            None => Err(GuiError::Cancelled { abandoned: true }),
            Some(Err(_)) if cancel.load(Ordering::SeqCst) => {
                Err(GuiError::Cancelled { abandoned: false })
            }
            Some(Err(e)) => Err(classify_query_error(e.to_string())),
            Some(Ok(result)) => Ok(ResultSet {
                columns: result.columns,
                rows: result.rows,
                elapsed,
                auto_limit,
                sql,
            }),
        }
    }

    async fn explain(
        &mut self,
        request: RequestId,
        sql: String,
        analyze: bool,
    ) -> GuiResult<ExplainRaw> {
        let cancel = self.begin(request)?;
        let started = Instant::now();
        let db_type = self.summary.db_type.clone();

        let rows = if analyze {
            if !self.summary.supports_explain_analyze() {
                return Err(GuiError::Unsupported(format!(
                    "EXPLAIN ANALYZE is not available for {}",
                    self.summary.type_name()
                )));
            }
            // ANALYZE executes the statement: only for statements the
            // read-only guard accepts, whatever the connection mode. The
            // exact text that is classified is the text that runs: leading
            // comments are stripped (conservatively) from both.
            let statement = crate::statements::strip_leading_comments(&sql)
                .trim()
                .trim_end_matches(';')
                .to_string();
            dbcrust_core::safety::check_read_only(&statement, &db_type).map_err(|e| {
                GuiError::ReadOnly(format!(
                    "EXPLAIN ANALYZE runs the statement, so it is only offered for read-only statements. {e}"
                ))
            })?;
            // Raw JSON plan (the structured path would return DBCrust's
            // display summary of the json value)
            self.database
                .execute_explain_analyze_query_raw(&statement)
                .await
                .map_err(|e| e.to_string())
        } else {
            let server_cancel = self.summary.supports_server_cancel();
            let query = self.database.execute_explain_query_raw(&sql);
            let outcome = if server_cancel {
                Some(query.await)
            } else {
                stop_when_cancelled(query, &cancel).await
            };
            match outcome {
                None => return Err(GuiError::Cancelled { abandoned: true }),
                Some(result) => result.map_err(|e| e.to_string()),
            }
        };

        match rows {
            Err(_) if cancel.load(Ordering::SeqCst) => {
                Err(GuiError::Cancelled { abandoned: false })
            }
            Err(e) => Err(classify_query_error(e)),
            Ok(rows) => Ok(ExplainRaw {
                db_type,
                sql,
                rows,
                analyzed: analyze,
                elapsed: started.elapsed(),
            }),
        }
    }

    /// DBCrust's table listing reports every object as a table; ask the
    /// catalog which ones are views (best effort: a failing catalog query
    /// just leaves the kinds as they are). Runs through the client directly
    /// so the default LIMIT is not appended.
    async fn mark_views(&self, tables: &mut [TableEntry]) {
        let Some(client) = self.database.get_database_client() else {
            return;
        };
        for (kind, query) in schema::view_queries(&self.summary.db_type) {
            match client.execute_query_structured(query).await {
                Ok(result) => {
                    let rows: Vec<Vec<String>> = result
                        .rows
                        .into_iter()
                        .map(|row| row.into_iter().map(Option::unwrap_or_default).collect())
                        .collect();
                    schema::mark_kinds(&self.summary.db_type, tables, &kind, &rows);
                }
                Err(e) => tracing::debug!("view catalog query failed: {e}"),
            }
        }
    }

    /// Connection details for `database` on this server: credentials, TLS
    /// and other options are kept (a URL round-trip would drop some).
    /// Never leaves the backend except to open the sibling connection.
    fn sibling_info(&self, database: &str) -> GuiResult<dbcrust_core::database::ConnectionInfo> {
        if !self.summary.supports_database_switch() {
            return Err(GuiError::Unsupported(format!(
                "{} connections have a single database",
                self.summary.type_name()
            )));
        }
        let mut info = self
            .database
            .get_connection_info()
            .cloned()
            .ok_or_else(|| {
                GuiError::Unsupported("This connection cannot open other databases".into())
            })?;
        info.database = Some(database.to_string());
        // A container connection's sibling resolves the container again (its
        // own relay), but on the daemon the parent used: if the user
        // switched docker contexts meanwhile, the parent's credentials must
        // not go to a same-named container on another daemon.
        if let Some(pin) = self.database.docker_daemon_pin() {
            info.options.insert(
                dbcrust_docker::docker::PINNED_DAEMON_OPTION.to_string(),
                pin.to_string(),
            );
        }
        Ok(info)
    }

    fn save_session(&mut self, name: &str) -> GuiResult<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(GuiError::Storage("Session name cannot be empty".into()));
        }
        let mut info = self
            .database
            .get_connection_info()
            .cloned()
            .ok_or_else(|| {
                GuiError::Storage("This connection cannot be saved as a session".into())
            })?;
        // Routing information, not part of what a session is
        info.options
            .remove(dbcrust_docker::docker::PINNED_DAEMON_OPTION);
        self.store
            .update_sessions(|config| config.save_session_from_connection_info(name, &info))
            .map_err(|e| GuiError::Storage(format!("Could not save session \"{name}\": {e}")))?;
        self.summary.session_name = Some(name.to_string());
        Ok(())
    }
}

/// Drive `future`, giving up (and dropping it) once `cancel` is set.
/// `None` means the caller stopped waiting.
async fn stop_when_cancelled<F: Future>(future: F, cancel: &AtomicBool) -> Option<F::Output> {
    let watch = async {
        loop {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        output = future => Some(output),
        _ = watch => None,
    }
}

fn classify_query_error(message: String) -> GuiError {
    let lower = message.to_lowercase();
    if lower.contains("read-only") || lower.contains("read only") {
        GuiError::ReadOnly(message)
    } else {
        GuiError::Query(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ConnectTarget;

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn test_store() -> ConfigStore {
        // Connections record recents from their worker thread: keep every
        // thread out of the real config directory, not only named ones
        crate::test_support::isolate();
        ConfigStore::from_config(dbcrust_app::config::Config::default())
    }

    fn sqlite_url(dir: &tempfile::TempDir) -> String {
        let path = dir.path().join("worker.db");
        std::fs::File::create(&path).unwrap();
        format!("sqlite://{}", path.display())
    }

    #[test]
    fn connect_execute_describe_and_close_on_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let request = ConnectRequest {
            target: ConnectTarget::Url(sqlite_url(&dir)),
            read_only: false,
            vault_addr: None,
        };
        block_on(async {
            let (handle, summary) = ConnectionHandle::connect(request, test_store())
                .await
                .unwrap();
            assert_eq!(
                summary.db_type,
                dbcrust_core::database::DatabaseType::SQLite
            );

            handle
                .execute(
                    RequestId::next(),
                    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)",
                )
                .await
                .unwrap();
            handle
                .execute(
                    RequestId::next(),
                    "INSERT INTO users (name) VALUES ('ada'), (NULL)",
                )
                .await
                .unwrap();
            let result = handle
                .execute(RequestId::next(), "SELECT id, name FROM users ORDER BY id")
                .await
                .unwrap();
            assert_eq!(result.columns, vec!["id", "name"]);
            assert_eq!(result.rows.len(), 2);
            assert_eq!(result.rows[1][1], None);
            assert!(result.auto_limit.is_some(), "default LIMIT is reported");

            handle
                .execute(
                    RequestId::next(),
                    "CREATE VIEW named_users AS SELECT name FROM users",
                )
                .await
                .unwrap();
            let tables = handle.list_tables().await.unwrap();
            assert!(
                tables
                    .iter()
                    .any(|t| t.name == "users" && t.kind == crate::types::TableKind::Table),
                "{tables:?}"
            );
            assert!(
                tables
                    .iter()
                    .any(|t| t.name == "named_users" && t.kind == crate::types::TableKind::View),
                "{tables:?}"
            );

            let details = handle.describe_table("users").await.unwrap();
            assert_eq!(details.columns.len(), 2);

            handle.close();
            let after = handle.execute(RequestId::next(), "SELECT 1").await;
            assert_eq!(after, Err(GuiError::Disconnected));
        });
    }

    #[test]
    fn close_cancels_running_work_and_refuses_queued_commands() {
        let dir = tempfile::tempdir().unwrap();
        let url = sqlite_url(&dir);
        block_on(async {
            let (handle, _) =
                ConnectionHandle::connect(ConnectRequest::url(url.clone()), test_store())
                    .await
                    .unwrap();
            handle
                .execute(RequestId::next(), "CREATE TABLE audit (note TEXT)")
                .await
                .unwrap();
            // A slow statement, then a write queued behind it
            let slow = handle.execute(
                RequestId::next(),
                "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 50000000) SELECT count(*) FROM n",
            );
            let queued_write = handle.execute(
                RequestId::next(),
                "INSERT INTO audit VALUES ('must not run')",
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
            handle.close();

            assert!(slow.await.is_err(), "the running statement is cancelled");
            assert_eq!(queued_write.await, Err(GuiError::Disconnected));
            assert_eq!(
                handle.execute(RequestId::next(), "SELECT 1").await,
                Err(GuiError::Disconnected),
                "new commands are refused immediately"
            );

            let (check, _) = ConnectionHandle::connect(ConnectRequest::url(url), test_store())
                .await
                .unwrap();
            let rows = check
                .execute(RequestId::next(), "SELECT count(*) FROM audit")
                .await
                .unwrap();
            assert_eq!(
                rows.rows[0][0].as_deref(),
                Some("0"),
                "queued write never ran"
            );
        });
    }

    #[test]
    fn read_only_connection_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let request = ConnectRequest {
            target: ConnectTarget::Url(sqlite_url(&dir)),
            read_only: true,
            vault_addr: None,
        };
        block_on(async {
            let (handle, summary) = ConnectionHandle::connect(request, test_store())
                .await
                .unwrap();
            assert!(summary.read_only);
            let error = handle
                .execute(RequestId::next(), "CREATE TABLE t (id INTEGER)")
                .await
                .unwrap_err();
            assert!(matches!(error, GuiError::ReadOnly(_)), "{error:?}");
        });
        assert!(
            !dbcrust_core::database::read_only_requested(),
            "global flag is reset"
        );
    }

    #[test]
    fn connect_failure_is_reported() {
        let request = ConnectRequest::url("postgres://nobody@127.0.0.1:1/nothing");
        let result = block_on(ConnectionHandle::connect(request, test_store()));
        assert!(matches!(result, Err(GuiError::Connection(_))));
    }

    fn idle_handle() -> ConnectionHandle {
        ConnectionHandle {
            id: ConnectionId::next(),
            tx: mpsc::unbounded_channel().0,
            control: Arc::new(Control::new()),
        }
    }

    #[test]
    fn cancel_only_targets_the_running_request() {
        let handle = idle_handle();
        let running = RequestId::next();
        let token = Arc::new(AtomicBool::new(false));
        handle.control.cancellation().running = Some((running, token.clone()));
        handle.control.pending.store(1, Ordering::SeqCst);

        handle.cancel(RequestId::next());
        assert!(
            !token.load(Ordering::SeqCst),
            "another request is untouched"
        );
        handle.cancel(running);
        assert!(token.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_before_start_prevents_the_request() {
        let handle = idle_handle();
        let queued = RequestId::next();
        handle.control.pending.store(2, Ordering::SeqCst);
        handle.cancel(queued);
        assert!(
            handle
                .control
                .cancellation()
                .cancelled_before_start
                .contains(&queued)
        );
        // Idle connection: nothing to remember
        let idle = idle_handle();
        idle.cancel(RequestId::next());
        assert!(
            idle.control
                .cancellation()
                .cancelled_before_start
                .is_empty()
        );
    }

    #[test]
    fn stop_when_cancelled_drops_the_future() {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let output = block_on(async move {
            let slow = tokio::time::sleep(Duration::from_secs(30));
            let trigger = async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                flag.store(true, Ordering::SeqCst);
            };
            let (output, _) = tokio::join!(stop_when_cancelled(slow, &cancel), trigger);
            output
        });
        assert!(output.is_none());
    }
    fn recent_count(store: &ConfigStore) -> usize {
        store.read(|config| config.get_recent_connections().len())
    }

    #[test]
    fn attempts_report_phases_and_a_test_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let url = sqlite_url(&dir);
        let store = test_store();
        block_on(async {
            let before = recent_count(&store);

            let tested =
                ConnectionHandle::start_test(ConnectRequest::url(url.clone()), store.clone());
            let ConnectAttempt {
                phases, outcome, ..
            } = tested;
            let connected = outcome.await.unwrap();
            assert!(connected.handle.is_none(), "a test keeps no connection");
            assert_eq!(
                connected.summary.db_type,
                dbcrust_core::database::DatabaseType::SQLite
            );
            assert!(connected.route.is_none());
            assert_eq!(*phases.borrow(), ConnectPhase::Authenticating);
            assert_eq!(recent_count(&store), before, "a test records no recent");

            let attempt =
                ConnectionHandle::start_connect(ConnectRequest::url(url.clone()), store.clone());
            let id = attempt.id;
            let connected = attempt.outcome.await.unwrap();
            assert_eq!(connected.attempt, id);
            let handle = connected.handle.expect("a live connection");
            assert!(handle.route().is_none());
            assert_eq!(recent_count(&store), before + 1);
            handle.close();
        });
    }

    #[test]
    fn a_failed_attempt_carries_its_id_and_the_error() {
        let attempt = ConnectionHandle::start_connect(
            ConnectRequest::url("sqlite:///definitely/not/a/dir/x.db"),
            test_store(),
        );
        let id = attempt.id;
        let failure = match block_on(attempt.outcome) {
            Ok(_) => panic!("must fail"),
            Err(failure) => failure,
        };
        assert_eq!(failure.attempt, id);
        assert!(matches!(failure.error, GuiError::Connection(_)));
        assert!(failure.docker.is_none());
    }

    #[test]
    fn cancelling_interrupts_a_handshake_that_never_finishes() {
        // A server that accepts and then says nothing.
        let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = silent.local_addr().unwrap().port();
        let _hold = std::thread::spawn(move || {
            let held = silent.accept();
            std::thread::sleep(Duration::from_secs(20));
            drop(held);
        });
        let attempt = ConnectionHandle::start_connect(
            ConnectRequest::url(format!("postgres://u:p@127.0.0.1:{port}/db")),
            test_store(),
        );
        let ConnectAttempt {
            mut phases,
            cancel,
            outcome,
            started,
            ..
        } = attempt;
        let failure = block_on(async move {
            // Wait until the handshake is underway, then cancel it.
            tokio::time::timeout(Duration::from_secs(5), async {
                while *phases.borrow_and_update() != ConnectPhase::Authenticating {
                    phases.changed().await.unwrap();
                }
            })
            .await
            .expect("reaches the handshake");
            cancel.cancel();
            match tokio::time::timeout(Duration::from_secs(5), outcome)
                .await
                .expect("cancel must end the attempt promptly")
            {
                Ok(_) => panic!("a silent server cannot be connected to"),
                Err(failure) => failure,
            }
        });
        assert_eq!(failure.error, GuiError::Cancelled { abandoned: false });
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_cancel_before_the_worker_waits_is_not_lost() {
        let cancel = ConnectCancel::default();
        cancel.cancel();
        cancel.cancel();
        block_on(async {
            tokio::time::timeout(Duration::from_secs(1), cancel.cancelled())
                .await
                .expect("the stored permit wakes the first waiter");
        });
    }

    /// Needs a running, unpublished postgres container:
    /// `DBCRUST_DOCKER_IT=1 DBCRUST_DOCKER_IT_CONTAINER=<name> cargo test -p dbcrust-gui-core docker_container_end_to_end`
    #[test]
    fn docker_container_end_to_end() {
        let Some(container) = std::env::var("DBCRUST_DOCKER_IT_CONTAINER")
            .ok()
            .filter(|_| std::env::var("DBCRUST_DOCKER_IT").is_ok_and(|v| v == "1"))
        else {
            eprintln!("skipping: set DBCRUST_DOCKER_IT=1 and DBCRUST_DOCKER_IT_CONTAINER");
            return;
        };
        let url = format!("docker://{container}");
        let store = test_store();
        block_on(async {
            // Test connection: same setup, then closed.
            let started = Instant::now();
            let tested = ConnectionHandle::start_test(ConnectRequest::url(&url), store.clone())
                .outcome
                .await
                .unwrap_or_else(|f| panic!("test failed: {}", f.error));
            assert!(tested.handle.is_none());
            let route = tested.route.expect("a docker route");
            eprintln!("test connect: {:?} via {}", started.elapsed(), route.detail);

            // A live connection, with the route and phases.
            let attempt = ConnectionHandle::start_connect(ConnectRequest::url(&url), store.clone());
            let phases = attempt.phases.clone();
            let connected = attempt
                .outcome
                .await
                .unwrap_or_else(|f| panic!("connect failed: {}", f.error));
            let parent = connected.handle.expect("handle");
            let route = parent.route().expect("route recorded on the handle");
            eprintln!("connected via {} ({:?})", route.detail, *phases.borrow());
            assert_eq!(
                connected.summary.host, container,
                "identity is the container"
            );
            assert_eq!(connected.summary.display_url, url);
            let result = parent
                .execute(RequestId::next(), "select 1 as one")
                .await
                .unwrap();
            assert_eq!(result.rows.len(), 1);

            // A sibling database opened after its parent is closed must
            // resolve the container again, not reuse the parent's relay.
            let sibling = parent.open_database("postgres", false, store.clone());
            // The sibling's details are fetched from the live parent...
            while parent.pending_operations() > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // ...and the parent (and its relay) is gone before it connects.
            // Meanwhile the user "switches docker context" to somewhere
            // else: the sibling still goes to the parent's daemon.
            parent.close();
            // SAFETY: opt-in integration test; nothing else reads the
            // environment concurrently in this scenario
            unsafe {
                std::env::set_var("DOCKER_CONTEXT", "dbcrust-it-context-that-does-not-exist")
            };
            tokio::time::sleep(Duration::from_millis(500)).await;
            let fresh = ConnectionHandle::start_connect(ConnectRequest::url(&url), store.clone())
                .outcome
                .await;
            let sibling = sibling.await;
            unsafe { std::env::remove_var("DOCKER_CONTEXT") };
            let failure = fresh.expect_err("a fresh connect follows the (broken) context");
            assert!(
                failure.error.to_string().contains("does-not-exist"),
                "{}",
                failure.error
            );
            let (sibling, summary) = sibling.unwrap_or_else(|e| panic!("sibling: {e}"));
            assert_eq!(summary.database, "postgres");
            assert_eq!(summary.host, container);
            let result = sibling
                .execute(RequestId::next(), "select 2 as two")
                .await
                .unwrap();
            assert_eq!(result.rows.len(), 1);
            sibling.close();

            // Cancelling mid-setup stops the work and leaves no relay behind.
            for _ in 0..3 {
                let attempt =
                    ConnectionHandle::start_connect(ConnectRequest::url(&url), store.clone());
                let ConnectAttempt {
                    mut phases,
                    cancel,
                    outcome,
                    ..
                } = attempt;
                tokio::time::timeout(Duration::from_secs(5), async {
                    while *phases.borrow_and_update() == ConnectPhase::Starting {
                        phases.changed().await.unwrap();
                    }
                })
                .await
                .expect("setup starts");
                cancel.cancel();
                match tokio::time::timeout(Duration::from_secs(5), outcome)
                    .await
                    .expect("cancelling ends the attempt promptly")
                {
                    Err(failure) => assert!(failure.error.is_cancelled(), "{}", failure.error),
                    Ok(connected) => drop(connected), // finished before the cancel
                }
            }
            let mut leftover = usize::MAX;
            for _ in 0..30 {
                leftover = relay_processes(&container);
                if leftover == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            assert_eq!(leftover, 0, "relay processes left in {container}");
        });
    }

    /// Relay programs (perl, nc, socat) running in a container, via the CLI.
    fn relay_processes(container: &str) -> usize {
        let output = std::process::Command::new("docker")
            .args([
                "exec",
                container,
                "sh",
                "-c",
                r#"for p in /proc/[0-9]*; do tr '\0' ' ' < $p/cmdline 2>/dev/null; echo; done | grep -cE '^(perl -e|nc [0-9]|socat - TCP)'"#,
            ])
            .output()
            .expect("docker exec");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(usize::MAX)
    }

    /// Destructive for the named container (it is restarted):
    /// `DBCRUST_DOCKER_IT=1 DBCRUST_DOCKER_IT_RESTART=<disposable container> cargo test -p dbcrust-gui-core docker_container_restart`
    #[test]
    fn docker_container_restart_surfaces_an_error_and_reconnecting_works() {
        let Some(container) = std::env::var("DBCRUST_DOCKER_IT_RESTART")
            .ok()
            .filter(|_| std::env::var("DBCRUST_DOCKER_IT").is_ok_and(|v| v == "1"))
        else {
            eprintln!("skipping: set DBCRUST_DOCKER_IT=1 and DBCRUST_DOCKER_IT_RESTART");
            return;
        };
        let url = format!("docker://{container}");
        let store = test_store();
        block_on(async {
            let connected =
                ConnectionHandle::start_connect(ConnectRequest::url(&url), store.clone())
                    .outcome
                    .await
                    .unwrap_or_else(|f| panic!("connect failed: {}", f.error));
            let handle = connected.handle.expect("handle");
            handle.execute(RequestId::next(), "select 1").await.unwrap();

            let status = std::process::Command::new("docker")
                .args(["restart", "-t", "1", &container])
                .status()
                .expect("docker restart");
            assert!(status.success());

            // The old connection is not replayed or papered over: a statement
            // either fails or runs on a fresh pooled socket, within seconds.
            let started = Instant::now();
            let mut outcomes = Vec::new();
            for _ in 0..4 {
                let result = tokio::time::timeout(
                    Duration::from_secs(20),
                    handle.execute(RequestId::next(), "select 1"),
                )
                .await
                .expect("a statement after a restart must not hang");
                outcomes.push(result.is_ok());
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            eprintln!("after restart: {outcomes:?} in {:?}", started.elapsed());
            handle.close();

            // A fresh connect resolves the container again.
            let reconnected = ConnectionHandle::start_connect(ConnectRequest::url(&url), store)
                .outcome
                .await
                .unwrap_or_else(|f| panic!("reconnect failed: {}", f.error));
            let handle = reconnected.handle.expect("handle");
            handle.execute(RequestId::next(), "select 1").await.unwrap();
            handle.close();
        });
    }
}
