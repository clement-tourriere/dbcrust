//! Application state shared by every view: live connections, their schema
//! caches, and the config store. One `Entity<AppState>` lives in a GPUI
//! global (see [`AppState::global`]).
//!
//! Views never talk to each other directly. They call methods here, and the
//! workspace reacts to the [`AppEvent`]s this entity emits (open a tab,
//! refresh the sidebar, …).

use dbcrust_gui_core::docker::{DockerFailure, DockerRoute};
use dbcrust_gui_core::launcher::{ConnectPhase, EnvTag, PhaseLog, TargetIdentity};
use dbcrust_gui_core::worker::{ConnectAttempt as BackendAttempt, ConnectCancel, ConnectFailure};
use dbcrust_gui_core::{
    ConfigStore, ConnectRequest, ConnectionHandle, ConnectionId, ConnectionSummary, GuiError,
    GuiResult, TableDetails, TableEntry,
};
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Schema of one connection, as shown in the explorer.
#[derive(Clone, Debug)]
pub enum SchemaState {
    Loading,
    Ready(Arc<Vec<TableEntry>>),
    Failed(String),
}

impl SchemaState {
    pub fn tables(&self) -> Option<&Arc<Vec<TableEntry>>> {
        match self {
            SchemaState::Ready(tables) => Some(tables),
            _ => None,
        }
    }
}

/// A connection the user opened.
pub struct LiveConnection {
    pub handle: ConnectionHandle,
    pub summary: ConnectionSummary,
    pub schema: SchemaState,
    /// `describe_table` results by the name they were requested with.
    pub details: HashMap<String, Arc<TableDetails>>,
    /// Stable per-connection accent slot (identity marker color), assigned
    /// at connect time. Map it to a color with `widgets::connection_accent`.
    pub accent: usize,
    /// What this connection actually opens (server or file, database, user,
    /// options, Docker daemon): the Connections page reuses it only for an
    /// entry that opens exactly the same.
    pub identity: Option<TargetIdentity>,
    /// The environment tag the connection was opened with (what the user
    /// picked on the form, or the entry's own): shown even when no list
    /// entry can be matched to the connection.
    pub environment: Option<EnvTag>,
}

impl LiveConnection {
    pub fn id(&self) -> ConnectionId {
        self.summary.id
    }
}

/// Things views ask the workspace to do, and state changes they react to.
#[derive(Clone, Debug)]
pub enum AppEvent {
    /// A connect started (`is_connecting()` is now true).
    ConnectStarted,
    Connected(ConnectionId),
    ConnectFailed(GuiError),
    Disconnected(ConnectionId),
    ActiveConnectionChanged(Option<ConnectionId>),
    /// Schema list or a cached table description changed.
    SchemaChanged(ConnectionId),
    /// Sessions, recent connections or named queries changed on disk.
    StorageChanged,
    /// Open a query tab (optionally running it right away).
    OpenQuery {
        connection: ConnectionId,
        title: Option<String>,
        sql: String,
        run: bool,
    },
    /// Open the table inspector tab.
    OpenTable {
        connection: ConnectionId,
        table: TableEntry,
    },
    OpenSettings,
    OpenHome,
    /// A connect or test attempt of the Connections page changed (started,
    /// moved to another phase, failed, finished).
    AttemptChanged(AttemptId),
    /// Bring a live connection's workspace forward: its most recent query
    /// tab, or a new one.
    OpenWorkspace(ConnectionId),
}

/// One connection attempt started from the Connections page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttemptId(u64);

impl AttemptId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptKind {
    /// Open the connection and show its workspace.
    Connect,
    /// The same setup as Connect, then close: no recent entry, no tab, no
    /// schema.
    Test,
}

/// What a finished test learned.
#[derive(Clone, Debug, PartialEq)]
pub struct TestOutcome {
    /// What the server said about itself.
    pub server: String,
    /// For a Docker container: how the test reached it.
    pub route: Option<DockerRoute>,
    pub elapsed: Duration,
}

/// Why an attempt failed, in the words the page shows.
#[derive(Clone, Debug, PartialEq)]
pub struct AttemptFailure {
    /// The engine's message, without the "Connection failed:" prefix.
    pub message: String,
    /// A Docker container that no route reached: the routes tried and the
    /// compose line that publishes the port.
    pub docker: Option<DockerFailure>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AttemptState {
    Running,
    /// A test passed.
    Tested(TestOutcome),
    Failed(AttemptFailure),
}

/// A connect or test attempt, tied to the target the user chose.
pub struct ConnectAttempt {
    pub id: AttemptId,
    pub kind: AttemptKind,
    /// The Connections page's identity of the target (a list entry key or
    /// the form's).
    pub target: String,
    /// What to call the target in messages.
    pub subject: String,
    /// Kept so Retry runs exactly what failed.
    pub request: ConnectRequest,
    /// What the request opens (see [`AttemptSpec::identity`]).
    pub identity: Option<TargetIdentity>,
    /// See [`AttemptSpec::environment`].
    pub environment: Option<EnvTag>,
    pub started: Instant,
    pub phases: PhaseLog,
    pub state: AttemptState,
    /// Stops the backend's work (probing, forwarding, the handshake).
    cancel: ConnectCancel,
    /// Following the backend; dropping them abandons the attempt.
    _tasks: Vec<Task<()>>,
}

impl ConnectAttempt {
    pub fn is_running(&self) -> bool {
        self.state == AttemptState::Running
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

/// What to start.
pub struct AttemptSpec {
    pub kind: AttemptKind,
    pub target: String,
    pub subject: String,
    pub request: ConnectRequest,
    /// What the request opens, when the page knows (it is kept with the live
    /// connection); otherwise it is derived from the connection's URL.
    pub identity: Option<TargetIdentity>,
    /// The environment tag to keep with the live connection.
    pub environment: Option<EnvTag>,
}

pub struct AppState {
    store: ConfigStore,
    connections: Vec<LiveConnection>,
    active: Option<ConnectionId>,
    connecting: usize,
    next_accent: usize,
    attempts: Vec<ConnectAttempt>,
    /// Environment tags the Connections page knows, by what they open.
    environments: Vec<(TargetIdentity, EnvTag)>,
}

struct GlobalAppState(Entity<AppState>);

impl Global for GlobalAppState {}

impl EventEmitter<AppEvent> for AppState {}

impl AppState {
    /// Create the global state. Call once at startup, after `gpui_kit::init`.
    pub fn init(store: ConfigStore, cx: &mut App) -> Entity<AppState> {
        let state = cx.new(|_| AppState {
            store,
            connections: Vec::new(),
            active: None,
            connecting: 0,
            next_accent: 0,
            attempts: Vec::new(),
            environments: Vec::new(),
        });
        cx.set_global(GlobalAppState(state.clone()));
        state
    }

    pub fn global(cx: &App) -> Entity<AppState> {
        cx.global::<GlobalAppState>().0.clone()
    }

    pub fn store(&self) -> &ConfigStore {
        &self.store
    }

    pub fn connections(&self) -> &[LiveConnection] {
        &self.connections
    }

    pub fn connection(&self, id: ConnectionId) -> Option<&LiveConnection> {
        self.connections.iter().find(|c| c.id() == id)
    }

    pub fn handle(&self, id: ConnectionId) -> Option<ConnectionHandle> {
        self.connection(id).map(|c| c.handle.clone())
    }

    pub fn active_id(&self) -> Option<ConnectionId> {
        self.active
    }

    pub fn active(&self) -> Option<&LiveConnection> {
        self.active.and_then(|id| self.connection(id))
    }

    pub fn is_connecting(&self) -> bool {
        self.connecting > 0
    }

    pub fn set_active(&mut self, id: Option<ConnectionId>, cx: &mut Context<Self>) {
        let id = id.filter(|id| self.connection(*id).is_some());
        if self.active != id {
            self.active = id;
            cx.emit(AppEvent::ActiveConnectionChanged(id));
            cx.notify();
        }
    }

    /// Open a connection. On success it becomes active, its schema starts
    /// loading, and `Connected` is emitted; on failure `ConnectFailed` is
    /// emitted (and returned).
    pub fn connect(
        &mut self,
        request: ConnectRequest,
        cx: &mut Context<Self>,
    ) -> Task<GuiResult<ConnectionId>> {
        let connecting = ConnectionHandle::connect(request, self.store.clone());
        self.track_connect(connecting, cx)
    }

    /// Open another database of `from`'s server as a new connection.
    pub fn open_database(
        &mut self,
        from: ConnectionId,
        database: String,
        cx: &mut Context<Self>,
    ) -> Task<GuiResult<ConnectionId>> {
        let Some(live) = self.connection(from) else {
            return Task::ready(Err(GuiError::Disconnected));
        };
        let connecting =
            live.handle
                .open_database(database, live.summary.read_only, self.store.clone());
        self.track_connect(connecting, cx)
    }

    fn track_connect(
        &mut self,
        connecting: impl std::future::Future<Output = GuiResult<(ConnectionHandle, ConnectionSummary)>>
        + 'static,
        cx: &mut Context<Self>,
    ) -> Task<GuiResult<ConnectionId>> {
        self.connecting += 1;
        cx.emit(AppEvent::ConnectStarted);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = connecting.await;
            this.update(cx, |this, cx| {
                this.connecting -= 1;
                let outcome = match result {
                    Ok((handle, summary)) => {
                        Ok(this.register_connection(handle, summary, None, None, cx))
                    }
                    Err(error) => {
                        cx.emit(AppEvent::ConnectFailed(error.clone()));
                        Err(error)
                    }
                };
                cx.notify();
                outcome
            })
            .unwrap_or(Err(GuiError::Disconnected))
        })
    }

    /// A connection opened: it becomes active and its schema starts loading.
    fn register_connection(
        &mut self,
        handle: ConnectionHandle,
        summary: ConnectionSummary,
        identity: Option<TargetIdentity>,
        environment: Option<EnvTag>,
        cx: &mut Context<Self>,
    ) -> ConnectionId {
        let id = summary.id;
        let accent = self.next_accent;
        self.next_accent += 1;
        // A container connection records the daemon it really went through
        let identity = identity
            .or_else(|| TargetIdentity::from_url(&summary.display_url))
            // The daemon the connection really used is authoritative
            .map(|identity| {
                identity.with_actual_docker_daemon(handle.docker_daemon_pin().as_deref())
            });
        self.connections.push(LiveConnection {
            handle,
            summary,
            schema: SchemaState::Loading,
            details: HashMap::new(),
            accent,
            identity,
            environment,
        });
        self.active = Some(id);
        cx.emit(AppEvent::Connected(id));
        cx.emit(AppEvent::ActiveConnectionChanged(Some(id)));
        cx.emit(AppEvent::StorageChanged); // recent list grew
        self.refresh_schema(id, cx);
        id
    }

    // ── Environment tags ──────────────────────────────────────────────

    /// The Connections page tells which environment tag belongs to what.
    /// Replaces the previous bindings; observers are notified only when
    /// something changed.
    pub fn set_environments(
        &mut self,
        environments: Vec<(TargetIdentity, EnvTag)>,
        cx: &mut Context<Self>,
    ) {
        if self.environments != environments {
            self.environments = environments;
            cx.notify();
        }
    }

    /// The environment tag (`Production`, red) of a live connection, resolved
    /// from what it opens.
    pub fn environment_of(&self, id: ConnectionId) -> Option<EnvTag> {
        let live = self.connection(id)?;
        // The rule: the tag a connection was opened with is what it shows (the
        // form's explicit choice, or the entry's own). Only a connection opened
        // without one (the palette, the Vault wizard) looks up the page's
        // bindings by what it opens.
        live.environment.clone().or_else(|| {
            let identity = live.identity.as_ref()?;
            self.environments
                .iter()
                .find(|(bound, _)| bound == identity)
                .map(|(_, tag)| tag.clone())
        })
    }

    // ── Attempts of the Connections page ──────────────────────────────

    pub fn attempts(&self) -> &[ConnectAttempt] {
        &self.attempts
    }

    pub fn attempt(&self, id: AttemptId) -> Option<&ConnectAttempt> {
        self.attempts.iter().find(|attempt| attempt.id == id)
    }

    /// The attempt filed under `target` (running, or the failure / test
    /// result it left).
    pub fn attempt_for(&self, target: &str) -> Option<&ConnectAttempt> {
        self.attempts
            .iter()
            .find(|attempt| attempt.target == target)
    }

    /// Start connecting (or testing). A target with an attempt already
    /// running gets that one back: no duplicate submission.
    pub fn start_attempt(&mut self, spec: AttemptSpec, cx: &mut Context<Self>) -> AttemptId {
        if let Some(running) = self
            .attempts
            .iter()
            .find(|attempt| attempt.target == spec.target && attempt.is_running())
        {
            return running.id;
        }
        // What the target's previous attempt left is replaced
        self.attempts
            .retain(|attempt| attempt.target != spec.target);

        let id = AttemptId::next();
        let kind = spec.kind;
        let backend = match kind {
            AttemptKind::Connect => {
                ConnectionHandle::start_connect(spec.request.clone(), self.store.clone())
            }
            AttemptKind::Test => {
                ConnectionHandle::start_test(spec.request.clone(), self.store.clone())
            }
        };
        let BackendAttempt {
            mut phases,
            cancel,
            outcome,
            ..
        } = backend;
        let followed = cx.spawn(async move |this, cx| {
            loop {
                let phase = *phases.borrow_and_update();
                let seen = this.update(cx, |this, cx| {
                    if let Some(attempt) = this.attempts.iter_mut().find(|a| a.id == id) {
                        let at = attempt.started.elapsed();
                        attempt.phases.record(phase, at);
                        cx.emit(AppEvent::AttemptChanged(id));
                        cx.notify();
                    }
                });
                if seen.is_err() || phases.changed().await.is_err() {
                    break;
                }
            }
        });
        let finished = cx.spawn(async move |this, cx| {
            let result = outcome.await;
            this.update(cx, |this, cx| this.finish_attempt(id, result, cx))
                .ok();
        });
        self.attempts.push(ConnectAttempt {
            id,
            kind,
            target: spec.target,
            subject: spec.subject,
            request: spec.request,
            identity: spec.identity,
            environment: spec.environment,
            started: Instant::now(),
            phases: PhaseLog::default(),
            state: AttemptState::Running,
            cancel,
            _tasks: vec![followed, finished],
        });
        if kind == AttemptKind::Connect {
            self.connecting += 1;
            cx.emit(AppEvent::ConnectStarted);
        }
        cx.emit(AppEvent::AttemptChanged(id));
        cx.notify();
        id
    }

    fn finish_attempt(
        &mut self,
        id: AttemptId,
        result: Result<dbcrust_gui_core::worker::Connected, ConnectFailure>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.attempts.iter().position(|attempt| attempt.id == id) else {
            // Cancelled meanwhile: the late result has nowhere to go
            if let Ok(connected) = result
                && let Some(handle) = connected.handle
            {
                handle.close();
            }
            return;
        };
        let (kind, started) = (self.attempts[ix].kind, self.attempts[ix].started);
        if kind == AttemptKind::Connect {
            self.connecting = self.connecting.saturating_sub(1);
        }
        match result {
            Ok(connected) => match connected.handle {
                Some(handle) => {
                    let attempt = self.attempts.remove(ix);
                    self.register_connection(
                        handle,
                        connected.summary,
                        attempt.identity,
                        attempt.environment,
                        cx,
                    );
                }
                None => {
                    self.attempts[ix].state = AttemptState::Tested(TestOutcome {
                        server: format!(
                            "{} · {}",
                            connected.summary.type_name(),
                            connected.summary.target()
                        ),
                        route: connected.route,
                        elapsed: started.elapsed(),
                    });
                }
            },
            Err(failure) if failure.error.is_cancelled() => {
                self.attempts.remove(ix);
            }
            Err(failure) => {
                let message = failure.error.to_string();
                let message = message
                    .strip_prefix("Connection failed: ")
                    .unwrap_or(&message)
                    .to_string();
                self.attempts[ix].state = AttemptState::Failed(AttemptFailure {
                    message,
                    docker: failure.docker,
                });
            }
        }
        cx.emit(AppEvent::AttemptChanged(id));
        cx.notify();
    }

    /// Show an attempt in a given state without a backend (screenshots and
    /// tests): `phases` are (phase, start offset) pairs and `elapsed` how long
    /// ago it started.
    #[doc(hidden)]
    pub fn preview_attempt(
        &mut self,
        spec: AttemptSpec,
        state: AttemptState,
        phases: &[(ConnectPhase, Duration)],
        elapsed: Duration,
        cx: &mut Context<Self>,
    ) {
        self.attempts
            .retain(|attempt| attempt.target != spec.target);
        let mut log = PhaseLog::default();
        for (phase, at) in phases {
            log.record(*phase, *at);
        }
        let id = AttemptId::next();
        self.attempts.push(ConnectAttempt {
            id,
            kind: spec.kind,
            target: spec.target,
            subject: spec.subject,
            request: spec.request,
            identity: spec.identity,
            environment: spec.environment,
            started: Instant::now()
                .checked_sub(elapsed)
                .unwrap_or_else(Instant::now),
            phases: log,
            state,
            cancel: ConnectCancel::default(),
            _tasks: Vec::new(),
        });
        cx.emit(AppEvent::AttemptChanged(id));
        cx.notify();
    }

    /// Stop an attempt that is running: the backend drops whatever it was
    /// doing (probes, a forwarding relay, a half-done handshake). Nothing is
    /// left behind on the page.
    pub fn cancel_attempt(&mut self, id: AttemptId, cx: &mut Context<Self>) {
        let Some(ix) = self.attempts.iter().position(|attempt| attempt.id == id) else {
            return;
        };
        let attempt = self.attempts.remove(ix);
        if attempt.is_running() {
            attempt.cancel.cancel();
            if attempt.kind == AttemptKind::Connect {
                self.connecting = self.connecting.saturating_sub(1);
            }
        }
        drop(attempt);
        cx.emit(AppEvent::AttemptChanged(id));
        cx.notify();
    }

    /// Retire what a form left behind when it moved on (another subject, or
    /// edited fields): every attempt whose target starts with `prefix` and is
    /// not in `keep`. Finished ones are dropped; running ones are cancelled,
    /// so nothing the form can no longer show (or cancel) can open a
    /// connection later.
    pub fn retire_attempts(&mut self, prefix: &str, keep: &[String], cx: &mut Context<Self>) {
        let retired: Vec<AttemptId> = self
            .attempts
            .iter()
            .filter(|attempt| attempt.target.starts_with(prefix) && !keep.contains(&attempt.target))
            .map(|attempt| attempt.id)
            .collect();
        for id in retired {
            self.cancel_attempt(id, cx);
        }
    }

    /// Forget a finished attempt (its failure or test result).
    pub fn dismiss_attempt(&mut self, id: AttemptId, cx: &mut Context<Self>) {
        self.cancel_attempt(id, cx);
    }

    /// Close a connection. Tabs bound to it stay open (showing it as closed)
    /// so no draft is lost.
    pub fn disconnect(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        let Some(index) = self.connections.iter().position(|c| c.id() == id) else {
            return;
        };
        let live = self.connections.remove(index);
        live.handle.close();
        if self.active == Some(id) {
            self.active = self.connections.last().map(|c| c.id());
            cx.emit(AppEvent::ActiveConnectionChanged(self.active));
        }
        cx.emit(AppEvent::Disconnected(id));
        cx.notify();
    }

    pub fn disconnect_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<_> = self.connections.iter().map(|c| c.id()).collect();
        for id in ids {
            self.disconnect(id, cx);
        }
    }

    /// Reload the table list (and drop cached table descriptions).
    pub fn refresh_schema(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        let Some(live) = self.connections.iter_mut().find(|c| c.id() == id) else {
            return;
        };
        live.schema = SchemaState::Loading;
        live.details.clear();
        let listing = live.handle.list_tables();
        cx.emit(AppEvent::SchemaChanged(id));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = listing.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(live) = this.connections.iter_mut().find(|c| c.id() == id) {
                    live.schema = match result {
                        Ok(tables) => SchemaState::Ready(Arc::new(tables)),
                        Err(error) => SchemaState::Failed(error.to_string()),
                    };
                    cx.emit(AppEvent::SchemaChanged(id));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Describe a table, cached per connection until the next schema
    /// refresh.
    pub fn table_details(
        &mut self,
        id: ConnectionId,
        table: String,
        cx: &mut Context<Self>,
    ) -> Task<GuiResult<Arc<TableDetails>>> {
        let Some(live) = self.connection(id) else {
            return Task::ready(Err(GuiError::Disconnected));
        };
        if let Some(details) = live.details.get(&table) {
            return Task::ready(Ok(details.clone()));
        }
        let describing = live.handle.describe_table(table.clone());
        cx.spawn(async move |this, cx| {
            let details = Arc::new(describing.await?);
            let _ = this.update(cx, |this, cx| {
                if let Some(live) = this.connections.iter_mut().find(|c| c.id() == id) {
                    live.details.insert(table, details.clone());
                    cx.emit(AppEvent::SchemaChanged(id));
                }
            });
            Ok(details)
        })
    }

    /// Toggle the write-statement guard of a live connection.
    pub fn set_read_only_guard(&mut self, id: ConnectionId, enabled: bool, cx: &mut Context<Self>) {
        let Some(live) = self.connections.iter_mut().find(|c| c.id() == id) else {
            return;
        };
        live.summary.read_only = enabled;
        let setting = live.handle.set_read_only(enabled);
        cx.background_spawn(setting).detach();
        cx.notify();
    }

    /// Save a live connection as a named session.
    pub fn save_session(
        &mut self,
        id: ConnectionId,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<GuiResult<()>> {
        let Some(live) = self.connection(id) else {
            return Task::ready(Err(GuiError::Disconnected));
        };
        let saving = live.handle.save_session(name.clone());
        cx.spawn(async move |this, cx| {
            saving.await?;
            let _ = this.update(cx, |this, cx| {
                if let Some(live) = this.connections.iter_mut().find(|c| c.id() == id) {
                    live.summary.session_name = Some(name.clone());
                    live.summary.label = name;
                }
                cx.emit(AppEvent::StorageChanged);
                cx.notify();
            });
            Ok(())
        })
    }

    /// Ask the workspace to open a query tab.
    pub fn open_query(
        &mut self,
        connection: ConnectionId,
        title: Option<String>,
        sql: String,
        run: bool,
        cx: &mut Context<Self>,
    ) {
        cx.emit(AppEvent::OpenQuery {
            connection,
            title,
            sql,
            run,
        });
    }

    /// Ask the workspace to open the table inspector.
    pub fn open_table(
        &mut self,
        connection: ConnectionId,
        table: TableEntry,
        cx: &mut Context<Self>,
    ) {
        cx.emit(AppEvent::OpenTable { connection, table });
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::OpenSettings);
    }

    pub fn open_home(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::OpenHome);
    }

    /// Show the workspace of a connection that is already open.
    pub fn open_workspace(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        if self.connection(id).is_some() {
            self.set_active(Some(id), cx);
            cx.emit(AppEvent::OpenWorkspace(id));
        }
    }

    /// Sessions / recents / named queries changed: views re-read the store.
    pub fn storage_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(AppEvent::StorageChanged);
        cx.notify();
    }
}
