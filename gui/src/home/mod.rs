//! The Connections page (the first tab).
//!
//! Two panes. The left one is the launcher's navigation: a search, the
//! All / Saved / Recent / Docker filters, one virtualized list and a fixed
//! "Connect with Vault…" action. The right one shows the selected item's
//! details, or the New / Edit connection form; both end in a sticky footer
//! with the actions.
//!
//! - `list.rs`: the left pane
//! - `details.rs`: the right pane for a selected item
//! - `form.rs`: the New / Edit form
//! - `attempts.rs`: connect and test attempts (progress, failures, details)
//! - `docker.rs`: container discovery and its loading / empty / error states
//! - `vault.rs`: the Vault wizard
//!
//! The pure model (what the list contains, selection rules, Docker facts,
//! persisted environment tags) lives in `dbcrust_gui_core::launcher`.

mod attempts;
mod details;
mod docker;
mod form;
mod list;
mod ui;
mod vault;

use crate::actions::{ExplainStatement, RunStatement};
use crate::app_state::{AppEvent, AppState, AttemptKind, AttemptSpec};
use dbcrust_gui_core::docker::DockerDaemon;
use dbcrust_gui_core::gui_state::GuiState;
use dbcrust_gui_core::launcher::{
    self, ConnectionMeta, DockerListState, Entry, EnvTag, Filter, LauncherState, Listing, Source,
    Sources, TargetIdentity, session_key,
};
use dbcrust_gui_core::store::{RecentEntry, SessionEntry};
use dbcrust_gui_core::{ConfigStore, ConnectRequest, ConnectionId};
use form::{ConnectionForm, FormEvent, FormMode, Submission};
use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::ButtonVariant,
    h_flex,
    input::{DeleteToBeginningOfLine, InputEvent, InputState},
    notification::Notification,
};
use gpui_kit::*;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Window width below which the list narrows from 320 to 280.
const NARROW_WINDOW: f32 = 1100.;
/// How long a selection must rest before it is written to disk.
const SELECTION_SETTLE: Duration = Duration::from_millis(600);

/// What the right pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Details,
    Form,
}

pub struct HomeView {
    focus_handle: FocusHandle,
    /// Focus target of the list area: clicking a row lands here, so the
    /// arrow keys keep working without the search box.
    list_focus: FocusHandle,
    search: Entity<InputState>,
    form: Entity<ConnectionForm>,
    sessions: Vec<SessionEntry>,
    /// What each saved session opens, by name (from the saved fields).
    session_identities: HashMap<String, TargetIdentity>,
    recents: Vec<RecentEntry>,
    docker: DockerListState,
    /// The daemon `docker` was listed from: containers connect through it.
    docker_daemon: Option<DockerDaemon>,
    /// Its pin, comparable with the one of a live connection's request.
    docker_pin: Option<String>,
    /// The daemon the current Docker context selects: where a Docker session
    /// or recent opens. Resolved when connecting (and cached for display).
    current_docker_pin: Option<String>,
    /// How that is resolved (replaced in tests).
    resolve_docker_pin: fn() -> Option<String>,
    docker_in_flight: bool,
    filter: Filter,
    listing: Listing,
    list: ListState,
    selected: Option<String>,
    /// The item selected last time, until the user picks another or it
    /// shows up (Docker lists load late).
    pending_restore: Option<String>,
    pane: Pane,
    launcher: LauncherState,
    reloading: bool,
    persist_task: Option<Task<()>>,
    /// Repaints the page while an attempt runs (its elapsed time).
    ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl HomeView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx);
        let store = state.read(cx).store().clone();
        let launcher = cx
            .try_global::<crate::RestoredState>()
            .map(|restored| restored.0.launcher.clone())
            .unwrap_or_default();
        let sessions = store.sessions();
        let session_identities = session_identities(&store);
        let recents = store.recent_connections();
        let first_run = launcher::is_first_run(&sessions, &recents);

        let form = cx.new(|cx| ConnectionForm::new(window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search connections…"));
        let focus_handle = cx.focus_handle();

        let subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&state, |this, _, event: &AppEvent, cx| match event {
                AppEvent::StorageChanged => this.read_lists(cx),
                AppEvent::OpenHome => this.refresh(cx),
                AppEvent::Connected(_) | AppEvent::Disconnected(_) => cx.notify(),
                AppEvent::AttemptChanged(_) => {
                    this.sync_ticker(cx);
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.list.reset(0);
                    this.rebuild(cx);
                }
            }),
            cx.subscribe_in(&form, window, Self::on_form_event),
            // The workspace focuses the page as a whole; put the caret where
            // the user starts typing
            cx.on_focus(&focus_handle, window, |this, window, cx| {
                this.focus_primary(window, cx);
            }),
        ];

        let mut this = Self {
            focus_handle,
            list_focus: cx.focus_handle(),
            search,
            form,
            sessions,
            session_identities,
            recents,
            docker: DockerListState::Loading,
            docker_daemon: None,
            docker_pin: None,
            current_docker_pin: None,
            resolve_docker_pin: launcher::current_docker_pin,
            docker_in_flight: false,
            filter: Filter::All,
            listing: Listing::default(),
            list: ListState::new(0, ListAlignment::Top, px(240.)),
            selected: None,
            pending_restore: launcher.last_selected.clone(),
            pane: if first_run { Pane::Form } else { Pane::Details },
            launcher,
            reloading: false,
            persist_task: None,
            ticker: None,
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this.discover_docker(cx);
        this.focus_primary(window, cx);
        this
    }

    // ── Focus ─────────────────────────────────────────────────────────

    /// Where the caret goes when the page is shown: the form's first
    /// field while it is open (first run), the search otherwise.
    pub fn focus_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pane {
            Pane::Form => self.form.update(cx, |form, cx| form.focus(window, cx)),
            Pane::Details => self
                .search
                .update(cx, |search, cx| search.focus(window, cx)),
        }
    }

    /// The `NewConnection` action: open the form, caret in its first field.
    pub fn focus_new_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_form(window, cx);
    }

    /// Show the New connection form. A draft the user left is kept; an
    /// edit of a saved connection is replaced by a fresh form.
    pub fn open_new_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form.update(cx, |form, cx| {
            if !matches!(form.mode(), FormMode::New) {
                form.reset(window, cx);
            }
        });
        self.pane = Pane::Form;
        self.form.update(cx, |form, cx| form.focus(window, cx));
        cx.notify();
    }

    fn form_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.pane == Pane::Form && self.form.focus_handle(cx).contains_focused(window, cx)
    }

    // ── Data ──────────────────────────────────────────────────────────

    /// The page was shown again: pick up sessions and recents written by the
    /// CLI, and look for Docker containers anew.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload_from_disk(cx);
        self.discover_docker(cx);
    }

    /// Re-read the in-memory store (cheap).
    fn read_lists(&mut self, cx: &mut Context<Self>) {
        let store = AppState::global(cx).read(cx).store().clone();
        self.sessions = store.sessions();
        self.session_identities = session_identities(&store);
        self.recents = store.recent_connections();
        self.rebuild(cx);
    }

    /// The Docker daemon the list came from (it travels with the list).
    pub(super) fn set_docker_daemon(&mut self, daemon: Option<DockerDaemon>) {
        self.docker_pin = daemon.as_ref().and_then(launcher::daemon_pin);
        // The listing came from the current context: Docker sessions and recents
        // open there too (re-resolved when one is connected)
        if self.docker_pin.is_some() {
            self.current_docker_pin = self.docker_pin.clone();
        }
        self.docker_daemon = daemon;
    }

    /// Re-read the files (blocking I/O, so off the UI thread).
    fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        if self.reloading {
            return;
        }
        self.reloading = true;
        let store = AppState::global(cx).read(cx).store().clone();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move { store.reload() }).await;
            this.update(cx, |this, cx| {
                this.reloading = false;
                this.read_lists(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Recompute the list from the data, the filter and the search, keep the
    /// selection where it makes sense, and tell the list how many rows it
    /// has now.
    pub(super) fn rebuild(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string();
        let listing = launcher::build_listing(
            self.filter,
            &query,
            &Sources {
                sessions: &self.sessions,
                recents: &self.recents,
                docker: &self.docker,
            },
            unix_now(),
        );
        let selected = launcher::resolve_selection(
            &listing,
            self.selected.as_deref(),
            self.pending_restore.as_deref(),
        );
        if selected.as_deref() == self.pending_restore.as_deref() && selected.is_some() {
            self.pending_restore = None;
        }
        if selected != self.selected {
            self.selected = selected;
        }
        let old = self.list.item_count();
        self.list.splice(0..old, listing.rows.len());
        self.listing = listing;
        self.reveal_selection();
        self.publish_environments(cx);
        cx.notify();
    }

    /// Tell the app which environment tag belongs to what, so the toolbar
    /// and the explorer can show it next to a live connection.
    fn publish_environments(&self, cx: &mut Context<Self>) {
        let mut bindings: Vec<(TargetIdentity, EnvTag)> = Vec::new();
        let mut bind = |key: String, identity: Option<TargetIdentity>| {
            if let (Some(identity), Some(tag)) = (identity, self.env_tag(&key)) {
                bindings.push((identity, tag));
            }
        };
        for session in &self.sessions {
            bind(
                session_key(&session.name),
                self.session_identities.get(&session.name).cloned(),
            );
        }
        for recent in &self.recents {
            bind(
                launcher::recent_key(&recent.url),
                TargetIdentity::from_url(&recent.url),
            );
        }
        if let DockerListState::Ready { containers, .. } = &self.docker {
            for container in containers {
                bind(
                    launcher::docker_key(&container.name),
                    Some(TargetIdentity::container(
                        &container.name,
                        self.docker_pin.as_deref(),
                    )),
                );
            }
        }
        AppState::global(cx).update(cx, |state, cx| state.set_environments(bindings, cx));
    }

    fn reveal_selection(&self) {
        if let Some(row) = self
            .selected
            .as_deref()
            .and_then(|key| self.listing.row_of(key))
        {
            self.list.scroll_to_reveal_item(row);
        }
    }

    /// Open the New connection form with the engine picker focused
    /// (screenshots and tests).
    #[doc(hidden)]
    pub fn preview_engine_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_form(window, cx);
        self.form
            .update(cx, |form, cx| form.focus_engine_picker(window, cx));
    }

    /// Show a Docker state without asking a daemon (screenshots and tests).
    pub fn preview_docker(&mut self, state: DockerListState, cx: &mut Context<Self>) {
        self.docker = state;
        self.rebuild(cx);
    }

    /// Show only one kind of item.
    pub fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.list.reset(0);
            self.rebuild(cx);
        }
    }

    // ── Selection ─────────────────────────────────────────────────────

    pub(super) fn selected_entry(&self) -> Option<&Entry> {
        self.selected
            .as_deref()
            .and_then(|key| self.listing.entry(key))
    }

    /// Select an item by its identity (`launcher::session_key`, …).
    pub fn select(&mut self, key: &str, cx: &mut Context<Self>) {
        self.pending_restore = None;
        if self.selected.as_deref() != Some(key) {
            self.selected = Some(key.to_string());
            self.persist_selection(cx);
        }
        self.leave_form(cx);
        self.reveal_selection();
        cx.notify();
    }

    fn step_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(key) = self.listing.step(self.selected.as_deref(), delta) {
            self.select(&key, cx);
        }
    }

    /// Back from the form to the details of the selection. A draft of a new
    /// connection stays in the form; an edit is dropped.
    fn leave_form(&mut self, cx: &mut Context<Self>) {
        if self.pane == Pane::Form {
            self.pane = Pane::Details;
            self.form.update(cx, |form, _| form.discard_edit());
        }
    }

    /// Write the selection to `gui_state.toml` once it has settled.
    fn persist_selection(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        self.launcher.last_selected = Some(key.clone());
        self.persist_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(SELECTION_SETTLE).await;
            cx.background_spawn(async move {
                if let Err(error) = GuiState::update(|state| {
                    state.launcher.last_selected = Some(key);
                }) {
                    tracing::warn!("could not remember the selected connection: {error}");
                }
            })
            .await;
        }));
    }

    // ── Environment and access metadata ───────────────────────────────

    pub(super) fn meta(&self, key: &str) -> ConnectionMeta {
        self.launcher
            .meta(key)
            .cloned()
            .unwrap_or_else(|| ConnectionMeta::new(key))
    }

    pub(super) fn env_tag(&self, key: &str) -> Option<EnvTag> {
        self.launcher.meta(key).and_then(ConnectionMeta::env_tag)
    }

    /// Remember a connection's environment tag and read-only preference.
    pub(super) fn store_meta(&mut self, meta: ConnectionMeta, cx: &mut Context<Self>) {
        self.launcher.set_meta(meta);
        self.write_launcher(cx);
        self.publish_environments(cx);
        cx.notify();
    }

    /// "Open read-only" of an item, from its details.
    pub(super) fn set_read_only_preference(
        &mut self,
        key: &str,
        read_only: bool,
        cx: &mut Context<Self>,
    ) {
        let mut meta = self.meta(key);
        meta.read_only = read_only;
        self.store_meta(meta, cx);
    }

    fn write_launcher(&mut self, cx: &mut Context<Self>) {
        let launcher = self.launcher.clone();
        cx.background_spawn(async move {
            let result = GuiState::update(|state| {
                // The selection is written separately: keep what is on disk
                let selected = state.launcher.last_selected.take();
                state.launcher = launcher;
                if selected.is_some() {
                    state.launcher.last_selected = selected;
                }
            });
            if let Err(error) = result {
                tracing::warn!("could not save connection labels: {error}");
            }
        })
        .detach();
    }

    // ── Live connections ──────────────────────────────────────────────

    /// What `entry` opens when it is connected to: the saved fields of a
    /// session, the stored URL of a recent entry, a listed container on the
    /// daemon that listed it.
    pub(super) fn entry_identity(&self, entry: &Entry) -> Option<TargetIdentity> {
        match entry.source {
            Source::Session(ix) => self
                .sessions
                .get(ix)
                .and_then(|session| self.session_identities.get(&session.name))
                .cloned()
                .map(|identity| identity.with_docker_daemon(self.current_docker_pin.as_deref())),
            Source::Recent(ix) => self
                .recents
                .get(ix)
                .and_then(|recent| TargetIdentity::from_url(&recent.url))
                .map(|identity| identity.with_docker_daemon(self.current_docker_pin.as_deref())),
            Source::Docker(ix) => match &self.docker {
                DockerListState::Ready { containers, .. } => containers.get(ix).map(|container| {
                    TargetIdentity::container(&container.name, self.docker_pin.as_deref())
                }),
                _ => None,
            },
        }
    }

    /// How a connection of `key` opens: the caller's wish (⇧↵) or the item's
    /// own preference, and always read-only when Settings say so.
    pub(super) fn effective_read_only(&self, key: &str, requested: Option<bool>, cx: &App) -> bool {
        requested.unwrap_or_else(|| self.meta(key).read_only) || details::forced_read_only(cx)
    }

    /// The open connection that stands for `entry`, if any: one that opens
    /// exactly what the entry opens now (same database, user, options and,
    /// for a container, daemon). With `read_only` given, only one opened in
    /// that mode counts: a read-only connection is not the read-write one.
    pub(super) fn live_for(
        &self,
        entry: &Entry,
        read_only: Option<bool>,
        cx: &App,
    ) -> Option<ConnectionId> {
        let identity = self.entry_identity(entry)?;
        let state = AppState::global(cx);
        let state = state.read(cx);
        state
            .connections()
            .iter()
            .filter(|live| read_only.is_none_or(|wanted| live.summary.read_only == wanted))
            .find(|live| live.identity.as_ref() == Some(&identity))
            .map(|live| live.id())
    }

    // ── Connecting ────────────────────────────────────────────────────

    /// The request that opens `entry`.
    fn request_for(&self, entry: &Entry) -> Option<ConnectRequest> {
        match entry.source {
            Source::Session(ix) => self
                .sessions
                .get(ix)
                .map(|session| ConnectRequest::session(session.name.clone())),
            Source::Recent(ix) => self
                .recents
                .get(ix)
                .map(|recent| ConnectRequest::recent(recent.clone())),
            Source::Docker(ix) => match &self.docker {
                DockerListState::Ready { containers, .. } => containers
                    .get(ix)
                    .filter(|container| container.running)
                    .map(|container| match &self.docker_daemon {
                        // The daemon that listed it, not whichever is current now
                        Some(daemon) => daemon.connect_request(&container.name),
                        None => ConnectRequest::url(container.connection_url()),
                    }),
                _ => None,
            },
        }
    }

    /// Connect (or open the workspace of the connection that is already
    /// open). `read_only` overrides the item's own preference.
    pub(super) fn connect_key(
        &mut self,
        key: &str,
        read_only: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.listing.entry(key).cloned() else {
            return;
        };
        let read_only = self.effective_read_only(key, read_only, cx);
        // A Docker session opens on the context that is current right now:
        // only a connection on that very daemon is the one to reuse
        self.current_docker_pin = (self.resolve_docker_pin)();
        let state = AppState::global(cx);
        if let Some(live) = self.live_for(&entry, Some(read_only), cx) {
            state.update(cx, |state, cx| state.open_workspace(live, cx));
            return;
        }
        let Some(request) = self.request_for(&entry) else {
            return;
        };
        let request = request
            .read_only(read_only)
            .vault_addr(vault::remembered_addr());
        let identity = self.entry_identity(&entry);
        let environment = self.env_tag(key);
        state.update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Connect,
                    target: key.to_string(),
                    subject: entry.title.clone(),
                    request,
                    identity,
                    environment,
                },
                cx,
            );
        });
    }

    /// Test the selected item: the same setup as Connect, then close.
    pub(super) fn test_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(entry) = self.listing.entry(key).cloned() else {
            return;
        };
        let read_only = self.effective_read_only(key, None, cx);
        let Some(request) = self.request_for(&entry) else {
            return;
        };
        let request = request
            .read_only(read_only)
            .vault_addr(vault::remembered_addr());
        let identity = self.entry_identity(&entry);
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Test,
                    target: attempts::test_target(key),
                    subject: entry.title.clone(),
                    request,
                    identity,
                    environment: None,
                },
                cx,
            );
        });
    }

    /// While an attempt runs, repaint a few times a second so its elapsed
    /// time and phases stay live.
    fn sync_ticker(&mut self, cx: &mut Context<Self>) {
        let running = AppState::global(cx)
            .read(cx)
            .attempts()
            .iter()
            .any(|attempt| attempt.is_running());
        if running && self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    let keep_going = this
                        .update(cx, |this, cx| {
                            let running = AppState::global(cx)
                                .read(cx)
                                .attempts()
                                .iter()
                                .any(|attempt| attempt.is_running());
                            if running {
                                cx.notify();
                            } else {
                                this.ticker = None;
                            }
                            running
                        })
                        .unwrap_or(false);
                    if !keep_going {
                        break;
                    }
                }
            }));
        }
    }

    // ── Forms ─────────────────────────────────────────────────────────

    fn on_form_event(
        &mut self,
        _: &Entity<ConnectionForm>,
        event: &FormEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FormEvent::Connect(submission) => self.connect_submission(submission, cx),
            FormEvent::Test(submission) => self.test_submission(submission, cx),
            FormEvent::Save(submission) => self.save_submission(submission, window, cx),
        }
    }

    fn submission_request(submission: &Submission) -> ConnectRequest {
        submission
            .request
            .clone()
            .vault_addr(vault::remembered_addr())
            .read_only(submission.read_only)
    }

    fn connect_submission(&mut self, submission: &Submission, cx: &mut Context<Self>) {
        // The environment of an unsaved connection follows it into Recent
        if let Some(tag) = &submission.env {
            let key = launcher::recent_key(&submission.recent_url);
            let mut meta = self.meta(&key);
            meta.set_env(Some(tag.clone()));
            meta.read_only = submission.read_only;
            self.store_meta(meta, cx);
        }
        let request = Self::submission_request(submission);
        let subject = submission.subject.clone();
        let identity = Some(submission.identity.clone());
        // The tag the user picked goes with the live connection itself, whatever
        // entry (if any) it can be matched to later
        let environment = submission.env.clone();
        let target = submission.attempt_key.clone();
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Connect,
                    target,
                    subject,
                    request,
                    identity,
                    environment,
                },
                cx,
            );
        });
    }

    fn test_submission(&mut self, submission: &Submission, cx: &mut Context<Self>) {
        let request = Self::submission_request(submission);
        let subject = submission.subject.clone();
        let identity = Some(submission.identity.clone());
        let target = attempts::test_target(&submission.attempt_key);
        AppState::global(cx).update(cx, |state, cx| {
            state.start_attempt(
                AttemptSpec {
                    kind: AttemptKind::Test,
                    target,
                    subject,
                    request,
                    identity,
                    environment: None,
                },
                cx,
            );
        });
    }

    /// Persist a session without connecting.
    fn save_submission(
        &mut self,
        submission: &Submission,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = AppState::global(cx).read(cx).store().clone();
        let url = submission.url.clone();
        let name = submission.name.clone();
        let editing = match &submission.mode {
            FormMode::Edit { session } => Some(session.clone()),
            FormMode::New => None,
        };
        let mut meta = self.meta(&session_key(&name));
        meta.set_env(submission.env.clone());
        meta.read_only = submission.read_only;
        let renamed_from = editing.filter(|old| *old != name);

        let saving = {
            let name = name.clone();
            let renamed_from = renamed_from.clone();
            cx.background_spawn(
                async move { form::save_session(&store, &name, &url, renamed_from) },
            )
        };
        let generation = submission.generation;
        let revision = submission.revision;
        cx.spawn_in(window, async move |this, cx| {
            let result = saving.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    if let Some(old) = &renamed_from {
                        this.launcher.rename(&session_key(old), &session_key(&name));
                    }
                    this.store_meta(meta, cx);
                    AppState::global(cx).update(cx, |state, cx| state.storage_changed(cx));
                    // Only a form that still shows what was saved is finished by
                    // it; one that moved on (another item, a new draft) stays
                    let owns_form = this.form.update(cx, |form, cx| {
                        form.save_finished(generation, revision, Some(&name), cx)
                    });
                    if owns_form {
                        this.pane = Pane::Details;
                        this.filter = Filter::All;
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
                        this.rebuild(cx);
                        this.select(&session_key(&name), cx);
                    }
                    window.push_notification(Notification::success(format!("Saved “{name}”")), cx);
                }
                Err(error) => {
                    this.form.update(cx, |form, cx| {
                        form.save_finished(generation, revision, None, cx)
                    });
                    window.push_notification(
                        Notification::error(error.to_string()).title("Couldn’t save"),
                        cx,
                    )
                }
            })
            .ok();
        })
        .detach();
    }

    /// ⌘E: edit the selected item.
    pub fn edit_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        self.edit_entry(&entry, false, window, cx);
    }

    /// Fill the form from `entry` and show it. `copy` starts a new
    /// connection from a saved one instead of editing it.
    pub(super) fn edit_entry(
        &mut self,
        entry: &Entry,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let meta = self.meta(&entry.key);
        match entry.source {
            Source::Session(ix) => {
                let Some(session) = self.sessions.get(ix).cloned() else {
                    return;
                };
                let store = AppState::global(cx).read(cx).store().clone();
                let Some(saved) = store.read(|config| config.get_session(&session.name).cloned())
                else {
                    return;
                };
                self.form.update(cx, |form, cx| {
                    form.load_session(&session.name, &saved, &meta, copy, window, cx)
                });
            }
            Source::Recent(ix) => {
                let Some(recent) = self.recents.get(ix).cloned() else {
                    return;
                };
                let url = recent
                    .url
                    .split(" # ")
                    .next()
                    .unwrap_or(&recent.url)
                    .to_string();
                self.form.update(cx, |form, cx| {
                    form.load_url(&url, Some(entry.title.clone()), &meta, window, cx)
                });
            }
            Source::Docker(ix) => {
                let DockerListState::Ready { containers, .. } = &self.docker else {
                    return;
                };
                let Some(container) = containers.get(ix).cloned() else {
                    return;
                };
                let daemon = self.docker_daemon.clone();
                self.form.update(cx, |form, cx| {
                    form.load_docker(&container, daemon, &meta, window, cx)
                });
            }
        }
        self.pane = Pane::Form;
        self.form.update(cx, |form, cx| form.focus(window, cx));
        cx.notify();
    }

    // ── Removing ──────────────────────────────────────────────────────

    /// ⌘⌫: confirm, then remove the selected saved connection or recent
    /// entry. A Docker container is never removed from here.
    pub(super) fn confirm_remove_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.selected_entry().cloned() {
            self.confirm_remove(&entry, window, cx);
        }
    }

    /// Confirm, then remove `entry` (a saved connection or a recent entry).
    /// What is removed is fixed when the dialog opens: the session's name or
    /// the recent entry's identity, never a position in a list that may
    /// have changed by the time the user confirms.
    pub(super) fn confirm_remove(
        &mut self,
        entry: &Entry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (removal, title, description, ok_text) = match entry.source {
            Source::Session(ix) => {
                let Some(session) = self.sessions.get(ix) else {
                    return;
                };
                (
                    Removal::Session(session.name.clone()),
                    format!("Delete “{}”?", entry.title),
                    "The saved connection is removed. The database itself is untouched.",
                    "Delete",
                )
            }
            Source::Recent(ix) => {
                let Some(recent) = self.recents.get(ix) else {
                    return;
                };
                (
                    Removal::Recent(launcher::recent_key(&recent.url)),
                    format!("Remove “{}” from Recent?", entry.title),
                    "Only this entry of DBCrust's recent list goes away. The database, file or \
                     container is untouched.",
                    "Remove from Recent",
                )
            }
            Source::Docker(_) => return,
        };
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let removal = removal.clone();
            alert
                .confirm()
                .title(title.clone())
                .description(description)
                .ok_text(ok_text)
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| this.remove(removal.clone(), window, cx));
                    true
                })
        });
    }

    fn remove(&mut self, removal: Removal, window: &mut Window, cx: &mut Context<Self>) {
        let store = AppState::global(cx).read(cx).store().clone();
        let key = removal.key();
        let removing = cx.background_spawn(async move {
            match removal {
                Removal::Session(name) => store.delete_session(&name).map(|_| ()),
                Removal::Recent(key) => store.remove_recent(&key).map(|_| ()),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = removing.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.launcher.forget(&key);
                    this.write_launcher(cx);
                    AppState::global(cx).update(cx, |state, cx| state.storage_changed(cx));
                }
                Err(error) => window.push_notification(
                    Notification::error(error.to_string()).title("Couldn’t remove"),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    // ── Keyboard ──────────────────────────────────────────────────────

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        let in_form = self.form_has_focus(window, cx);

        if key == "escape" && !modifiers.modified() {
            if self.escape(window, cx) {
                cx.stop_propagation();
            }
            return;
        }
        // Arrows and Enter belong to the list while the search box or the list
        // has the focus; a focused button or form field keeps them
        let in_list = self.search.focus_handle(cx).is_focused(window)
            || self.list_focus.contains_focused(window, cx)
            || self.focus_handle.is_focused(window);
        if in_form || !in_list {
            return;
        }
        let handled = match (key, modifiers.platform || modifiers.control) {
            ("down", false) if !modifiers.alt => {
                self.step_selection(1, cx);
                true
            }
            ("up", false) if !modifiers.alt => {
                self.step_selection(-1, cx);
                true
            }
            ("enter", false) => {
                if let Some(key) = self.selected.clone() {
                    self.connect_key(&key, modifiers.shift.then_some(true), cx);
                }
                true
            }
            ("backspace" | "delete", true) | ("delete", false) if self.search_is_empty(cx) => {
                self.confirm_remove_selected(window, cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn search_is_empty(&self, cx: &App) -> bool {
        self.search.read(cx).value().is_empty()
    }

    /// Esc dismisses the topmost layer: a running attempt, then the form,
    /// then the search text. `true` when something was dismissed.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let state = AppState::global(cx);
        let running = {
            let state = state.read(cx);
            let key = match self.pane {
                Pane::Form => Some(self.form.read(cx).attempt_key()),
                Pane::Details => self.selected.clone(),
            };
            key.and_then(|key| {
                [Some(key.clone()), Some(attempts::test_target(&key))]
                    .into_iter()
                    .flatten()
                    .find_map(|target| {
                        state
                            .attempt_for(&target)
                            .filter(|attempt| attempt.is_running())
                            .map(|attempt| attempt.id)
                    })
            })
        };
        if let Some(id) = running {
            state.update(cx, |state, cx| state.cancel_attempt(id, cx));
            return true;
        }
        if self.pane == Pane::Form && self.form.update(cx, |form, cx| form.dismiss_layer(cx)) {
            return true;
        }
        if self.pane == Pane::Form && self.selected.is_some() {
            self.leave_form(cx);
            self.focus_primary(window, cx);
            cx.notify();
            return true;
        }
        if !self.search_is_empty(cx) {
            // Setting the value from code does not announce a change
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
            self.list.reset(0);
            self.rebuild(cx);
            return true;
        }
        false
    }

    /// ⌘↵: connect, from the list or the form.
    fn on_run_statement(&mut self, _: &RunStatement, window: &mut Window, cx: &mut Context<Self>) {
        match self.pane {
            Pane::Form => self.form.update(cx, |form, cx| form.connect(window, cx)),
            Pane::Details => {
                if let Some(key) = self.selected.clone() {
                    self.connect_key(&key, None, cx);
                }
            }
        }
    }

    /// ⌘E: edit the selected saved connection (the app's global "explain"
    /// shortcut means nothing on this page).
    fn on_explain_statement(
        &mut self,
        _: &ExplainStatement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane == Pane::Details {
            self.edit_selected(window, cx);
        }
    }

    /// ⌘⌫ in the search box deletes its text, as everywhere; with nothing to
    /// delete it removes the selected connection.
    fn on_delete_to_line_start(
        &mut self,
        _: &DeleteToBeginningOfLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search_focused = self.search.focus_handle(cx).is_focused(window);
        if search_focused && self.search_is_empty(cx) && self.pane == Pane::Details {
            self.confirm_remove_selected(window, cx);
            cx.stop_propagation();
        }
    }
}

/// What a confirmed removal deletes, fixed when the dialog opens.
#[derive(Clone, Debug)]
enum Removal {
    /// A saved session, by name.
    Session(String),
    /// A recent entry, by its identity (`launcher::recent_key`).
    Recent(String),
}

impl Removal {
    /// The identity under which its display metadata is kept.
    fn key(&self) -> String {
        match self {
            Removal::Session(name) => session_key(name),
            Removal::Recent(key) => key.clone(),
        }
    }
}

/// What each saved session opens, from its stored fields.
fn session_identities(store: &ConfigStore) -> HashMap<String, TargetIdentity> {
    store.read(|config| {
        config
            .list_sessions()
            .into_iter()
            .map(|(name, saved)| (name, TargetIdentity::from_session(&saved)))
            .collect()
    })
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

impl Focusable for HomeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = window.viewport_size().width < px(NARROW_WINDOW);
        let left = self.render_left(narrow, window, cx);
        let right = match self.pane {
            Pane::Form => self.form.clone().into_any_element(),
            Pane::Details => self.render_details(window, cx),
        };
        h_flex()
            .id("connections")
            .key_context("Connections")
            .track_focus(&self.focus_handle)
            .size_full()
            .min_w_0()
            .bg(cx.theme().background)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::on_run_statement))
            .on_action(cx.listener(Self::on_explain_statement))
            .capture_action(cx.listener(Self::on_delete_to_line_start))
            .child(left)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(right),
            )
    }
}

#[cfg(test)]
mod tests;
