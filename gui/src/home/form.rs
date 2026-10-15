//! The New / Edit connection form.
//!
//! One searchable engine picker (Servers / Files), real default values, a
//! Name, Host + Port and User + Password rows, an encryption summary with a
//! disclosure, read-only, an optional environment, and a footer with the
//! sanitized target and Test / Save / Connect. A URL is an input method:
//! "Paste connection URL…" fills the fields and keeps the options they have
//! no control for.

use super::attempts;
use super::ui::{
    docker_badge, footer_bar, group_label, mono, scroll_body, shortcut_hint, vault_badge,
};
use super::vault;
use crate::app_state::AppState;
use crate::widgets::{engine_badge, env_swatch};
use dbcrust_gui_core::dbcrust::config::SavedSession;
use dbcrust_gui_core::docker::{DockerContainer, DockerDaemon};
use dbcrust_gui_core::launcher::{
    self, ConnectionMeta, ENVIRONMENT_PRESETS, EnvColor, EnvTag, FORM_KEY_PREFIX, TargetIdentity,
    form_key,
};
use dbcrust_gui_core::url_builder::{
    self as urls, ConnectionSpec, DockerSpec, PASSWORD_MASK, SSL_MODES, Target, TlsSetting,
    VaultSpec,
};
use dbcrust_gui_core::{
    ConfigStore, ConnectRequest, ConnectTarget, DatabaseType, DatabaseTypeExt, GuiError, GuiResult,
    dbcrust::database::ConnectionInfo,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Selectable as _, Sizable as _,
    StyledExt as _,
    button::{Button, ButtonVariants as _},
    form::{Form, field, v_form},
    h_flex,
    input::{Input, InputContentType, InputEvent, InputState},
    searchable_list::{SearchableGroup, SearchableListItem, SearchableVec},
    select::{Select, SelectEvent, SelectState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// Whether the form is making a new connection or editing a saved one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormMode {
    New,
    Edit { session: String },
}

/// What the form hands to the page when the user acts.
#[derive(Clone, Debug)]
pub struct Submission {
    /// The URL of the connection, password included (what Save parses).
    pub url: String,
    /// The request that connects (or tests) exactly this. A listed container
    /// is requested through the daemon that listed it, with the edited user,
    /// database and options applied.
    pub request: ConnectRequest,
    /// What the request opens: kept with the live connection.
    pub identity: TargetIdentity,
    /// How a recent entry of this connection is stored (no password).
    pub recent_url: String,
    /// The form subject this submission belongs to (see
    /// [`ConnectionForm::generation`]).
    pub generation: u64,
    /// The key its attempts are filed under.
    pub attempt_key: String,
    /// How many edits the form had when this was taken.
    pub revision: u64,
    /// What to call it in messages (the name, or what would be saved).
    pub subject: String,
    /// The name to save under.
    pub name: String,
    pub read_only: bool,
    pub env: Option<EnvTag>,
    pub mode: FormMode,
}

pub enum FormEvent {
    Connect(Submission),
    Test(Submission),
    Save(Submission),
}

impl EventEmitter<FormEvent> for ConnectionForm {}

// ── Engine picker ──────────────────────────────────────────────────────

#[derive(Clone)]
struct EngineItem {
    engine: DatabaseType,
}

impl SearchableListItem for EngineItem {
    type Value = DatabaseType;

    fn title(&self) -> SharedString {
        self.engine.display_name().into()
    }

    fn display_title(&self) -> Option<AnyElement> {
        None
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let hint: SharedString = match self.engine.default_port() {
            Some(port) => format!(":{port}").into(),
            None => "file".into(),
        };
        h_flex()
            .w_full()
            .gap_2p5()
            .items_center()
            .child(engine_badge(&self.engine, 22., cx))
            .child(div().flex_1().child(self.engine.display_name()))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
    }

    fn value(&self) -> &Self::Value {
        &self.engine
    }

    fn matches(&self, query: &str) -> bool {
        let haystack = format!(
            "{} {}",
            self.engine.display_name(),
            urls::engine_aliases(&self.engine)
        )
        .to_lowercase();
        let query = query.trim().to_lowercase();
        haystack.contains(&query)
    }
}

type EngineList = SearchableVec<SearchableGroup<EngineItem>>;

fn engine_list() -> EngineList {
    let (files, servers): (Vec<_>, Vec<_>) = DatabaseType::supported_types()
        .into_iter()
        .partition(|db| db.is_file_based());
    let group = |title: &'static str, engines: Vec<DatabaseType>| {
        SearchableGroup::new(title).items(engines.into_iter().map(|engine| EngineItem { engine }))
    };
    SearchableVec::new(vec![group("Servers", servers), group("Files", files)])
}

/// Where the engine sits in the grouped list (for the initial selection).
fn engine_index(engine: &DatabaseType) -> IndexPath {
    let (files, servers): (Vec<_>, Vec<_>) = DatabaseType::supported_types()
        .into_iter()
        .partition(|db| db.is_file_based());
    match servers.iter().position(|db| db == engine) {
        Some(row) => IndexPath::default().section(0).row(row),
        None => IndexPath::default()
            .section(1)
            .row(files.iter().position(|db| db == engine).unwrap_or(0)),
    }
}

/// First entry of the SSL select: write no `sslmode` at all.
const SSL_DEFAULT: &str = "Default";

fn ssl_choices() -> Vec<&'static str> {
    std::iter::once(SSL_DEFAULT).chain(SSL_MODES).collect()
}

/// What the form connects to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Engine(DatabaseType),
    Docker,
    Vault,
}

/// A field a message can point at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Name,
    Host,
    Port,
    Path,
    Container,
}

#[derive(Clone, Debug)]
pub(super) struct FieldError {
    pub(super) field: Field,
    pub(super) message: SharedString,
}

pub struct ConnectionForm {
    focus_handle: FocusHandle,
    mode: FormMode,
    /// Set when leaving an edit: the next time the form opens it starts over.
    needs_reset: bool,
    /// Loaded from a list item (an edit, a copy): not a draft of the user's
    /// own, so leaving it drops it.
    seeded: bool,
    /// Changes whenever the form is pointed at another subject (reset, a
    /// loaded item, a pasted URL). Attempts and saves are tagged with it, so
    /// a result never lands on a form it does not belong to.
    generation: u64,
    /// The generation whose Save is in flight.
    saving: Option<u64>,
    /// Counts every edit of the form (a field, the engine, a switch, the
    /// environment): a Save only finishes a form nobody has edited since.
    fields_revision: u64,
    /// Counts the edits that change what a connection would open. Attempts
    /// are filed under it, so a result belongs to the fields it was run for;
    /// unlike the generation it keeps the discovered container's daemon.
    connection_revision: u64,
    /// The Docker daemon a discovered container was listed from: requests
    /// for it go to that daemon, whatever context is current later.
    docker_daemon: Option<DockerDaemon>,
    pub(super) kind: Kind,
    engine_select: Entity<SelectState<EngineList>>,
    pub(super) name: Entity<InputState>,
    pub(super) host: Entity<InputState>,
    pub(super) port: Entity<InputState>,
    pub(super) user: Entity<InputState>,
    password: Entity<InputState>,
    pub(super) database: Entity<InputState>,
    path: Entity<InputState>,
    delimiter: Entity<InputState>,
    container: Entity<InputState>,
    pub(super) import: Entity<InputState>,
    ssl_mode: Entity<SelectState<Vec<&'static str>>>,
    pub(super) tls: bool,
    csv_header: bool,
    read_only: bool,
    pub(super) env: Option<EnvTag>,
    advanced_open: bool,
    import_open: bool,
    import_error: Option<SharedString>,
    /// Scheme and options of a parsed URL the fields cannot show; kept so
    /// the connection still carries them.
    scheme: Option<String>,
    pub(super) extra_options: Vec<(String, String)>,
    vault: Option<VaultSpec>,
    pub(super) error: Option<FieldError>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionForm {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |window: &mut Window, cx: &mut Context<Self>, placeholder: &'static str| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let name = input(window, cx, "Untitled connection");
        let host = input(window, cx, "Host name or IP address");
        let port = input(window, cx, "Port");
        let user = input(window, cx, "User");
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password")
        });
        let database = input(window, cx, "Database");
        let path = input(window, cx, "Path to the file");
        let delimiter = input(window, cx, ",");
        let container = input(window, cx, "Container name");
        let import = input(
            window,
            cx,
            "postgres://user@host:5432/database, a docker:// URL or a file path",
        );
        let ssl_mode =
            cx.new(|cx| SelectState::new(ssl_choices(), Some(IndexPath::default()), window, cx));
        let engine_select = cx.new(|cx| {
            SelectState::new(
                engine_list(),
                Some(engine_index(&DatabaseType::PostgreSQL)),
                window,
                cx,
            )
            .searchable(true)
        });

        let mut form = Self {
            focus_handle: cx.focus_handle(),
            mode: FormMode::New,
            needs_reset: false,
            seeded: false,
            generation: 1,
            saving: None,
            fields_revision: 0,
            connection_revision: 0,
            docker_daemon: None,
            kind: Kind::Engine(DatabaseType::PostgreSQL),
            engine_select,
            name,
            host,
            port,
            user,
            password,
            database,
            path,
            delimiter,
            container,
            import,
            ssl_mode,
            tls: false,
            csv_header: true,
            read_only: false,
            env: None,
            advanced_open: false,
            import_open: false,
            import_error: None,
            scheme: None,
            extra_options: Vec::new(),
            vault: None,
            error: None,
            _subscriptions: Vec::new(),
        };
        let inputs = [
            form.name.clone(),
            form.host.clone(),
            form.port.clone(),
            form.user.clone(),
            form.password.clone(),
            form.database.clone(),
            form.path.clone(),
            form.delimiter.clone(),
            form.container.clone(),
        ];
        for input in &inputs {
            // The name labels the connection; every other field is part of it
            let is_name = *input == form.name;
            form._subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        this.touch(!is_name, cx);
                        this.refresh_name_hint(window, cx);
                        cx.notify();
                    }
                },
            ));
        }
        form._subscriptions.push(cx.subscribe_in(
            &form.import,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.import_error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.apply_import(window, cx),
                _ => {}
            },
        ));
        form._subscriptions.push(cx.subscribe(
            &form.ssl_mode,
            |this, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.error = None;
                this.touch(true, cx);
                cx.notify();
            },
        ));
        form._subscriptions.push(cx.subscribe_in(
            &form.engine_select,
            window,
            |this, _, event: &SelectEvent<EngineList>, window, cx| {
                let SelectEvent::Confirm(Some(engine)) = event else {
                    return;
                };
                this.set_engine(engine.clone(), window, cx);
            },
        ));
        form.apply_defaults(&DatabaseType::PostgreSQL, None, window, cx);
        form
    }

    // ── Public surface ────────────────────────────────────────────────

    /// Put the caret on the engine picker (screenshots and tests open its
    /// menu from there).
    #[doc(hidden)]
    pub fn focus_engine_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.engine_select
            .update(cx, |select, cx| select.focus(window, cx));
    }

    pub fn mode(&self) -> &FormMode {
        &self.mode
    }

    /// Identifies what the form is about right now.
    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The key under which the attempts of this form's subject are filed.
    pub fn attempt_key(&self) -> String {
        form_key(self.generation, self.connection_revision)
    }

    /// The form is about something else now: results of earlier subjects no
    /// longer belong to it.
    fn new_subject(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.docker_daemon = None;
        self.touch(true, cx);
    }

    /// The user edited the form. `connection`: what it would open changed,
    /// so results of earlier attempts no longer describe it.
    pub(super) fn touch(&mut self, connection: bool, cx: &mut Context<Self>) {
        self.fields_revision += 1;
        if !connection {
            return;
        }
        self.connection_revision += 1;
        let key = self.attempt_key();
        let keep = [key.clone(), attempts::test_target(&key)];
        AppState::global(cx).update(cx, |state, cx| {
            state.retire_attempts(FORM_KEY_PREFIX, &keep, cx)
        });
    }

    /// Put the caret in the first field the user fills.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.needs_reset {
            self.reset(window, cx);
        }
        let target = match &self.kind {
            Kind::Engine(db) if db.is_file_based() => Some(&self.path),
            Kind::Engine(_) => Some(&self.host),
            Kind::Docker => Some(&self.container),
            Kind::Vault => None,
        };
        match target {
            Some(input) => input.update(cx, |input, cx| input.focus(window, cx)),
            None => window.focus(&self.focus_handle, cx),
        }
    }

    /// Start over with a blank PostgreSQL connection.
    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_subject(cx);
        self.mode = FormMode::New;
        self.needs_reset = false;
        self.seeded = false;
        self.kind = Kind::Engine(DatabaseType::PostgreSQL);
        self.scheme = None;
        self.extra_options.clear();
        self.vault = None;
        self.env = None;
        self.error = None;
        self.import_open = false;
        self.import_error = None;
        self.advanced_open = false;
        self.tls = false;
        self.csv_header = true;
        self.read_only = false;
        for input in [
            &self.name,
            &self.password,
            &self.path,
            &self.container,
            &self.import,
            &self.delimiter,
            &self.database,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.ssl_mode.update(cx, |select, cx| {
            select.set_selected_value(&SSL_DEFAULT, window, cx);
        });
        self.engine_select.update(cx, |select, cx| {
            select.set_selected_value(&DatabaseType::PostgreSQL, window, cx);
        });
        self.apply_defaults(&DatabaseType::PostgreSQL, None, window, cx);
        cx.notify();
    }

    /// Leaving the form while it shows an edit (or a copy of an item) drops
    /// it; a draft the user typed stays.
    pub fn discard_edit(&mut self) {
        if matches!(self.mode, FormMode::Edit { .. }) || self.seeded {
            self.needs_reset = true;
        }
    }

    /// The Save of `generation` ended. Returns whether the form still shows
    /// that subject: only then does a successful save finish the form (the
    /// next New connection starts blank); a form that moved on is left as it
    /// is.
    pub fn save_finished(
        &mut self,
        generation: u64,
        revision: u64,
        saved_as: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.saving == Some(generation) {
            self.saving = None;
        }
        let same_subject = self.generation == generation;
        // Only a form nobody edited since the Save started is finished by it
        let owns = same_subject && self.fields_revision == revision;
        if let Some(name) = saved_as
            && same_subject
        {
            // The saved connection now belongs to this form, edited or not:
            // the next Save updates it (and renames it) instead of colliding
            // with it as a "new" one
            self.mode = FormMode::Edit {
                session: name.to_string(),
            };
            self.seeded = true;
            if owns {
                self.needs_reset = true;
            }
        }
        cx.notify();
        owns
    }

    /// Esc: close the URL box if it is open. `true` when it was.
    pub fn dismiss_layer(&mut self, cx: &mut Context<Self>) -> bool {
        if self.import_open {
            self.import_open = false;
            self.import_error = None;
            cx.notify();
            return true;
        }
        false
    }

    /// Fill the form from a saved session.
    pub fn load_session(
        &mut self,
        name: &str,
        saved: &SavedSession,
        meta: &ConnectionMeta,
        copy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset(window, cx);
        self.seeded = true;
        self.mode = if copy {
            FormMode::New
        } else {
            FormMode::Edit {
                session: name.to_string(),
            }
        };
        let display_name = if copy {
            format!("{name} copy")
        } else {
            name.to_string()
        };
        self.set_text(&self.name.clone(), &display_name, window, cx);

        let option = |key: &str| saved.options.get(key).cloned();
        if let Some(container) = saved.host.strip_prefix("DOCKER:") {
            self.kind = Kind::Docker;
            self.set_text(&self.container.clone(), container, window, cx);
            self.set_text(&self.user.clone(), &saved.user, window, cx);
            self.set_text(&self.database.clone(), &saved.dbname, window, cx);
            // The options and TLS the session carries stay with it
            let mut options: Vec<(String, String)> = saved
                .options
                .iter()
                .filter(|(key, _)| !key.starts_with("vault_") && key.as_str() != "ssl")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            options.sort();
            self.extra_options = options;
            self.tls = saved
                .options
                .get("ssl")
                .is_some_and(|value| value.eq_ignore_ascii_case("true"));
            self.advanced_open = self.tls || !self.extra_options.is_empty();
        } else if let (Some(mount), Some(database), Some(role)) = (
            option("vault_mount"),
            option("vault_database"),
            option("vault_role"),
        ) {
            self.kind = Kind::Vault;
            self.vault = Some(VaultSpec {
                role,
                mount,
                database,
            });
        } else {
            let engine = saved.database_type.clone();
            self.kind = Kind::Engine(engine.clone());
            self.engine_select.update(cx, |select, cx| {
                select.set_selected_value(&engine, window, cx);
            });
            let mut spec = ConnectionSpec::new(engine.clone());
            if engine.is_file_based() {
                spec.path = saved.file_path.clone().unwrap_or_default();
            } else {
                spec.host = saved.host.clone();
                spec.port = Some(saved.port).filter(|port| *port != 0);
                spec.user = saved.user.clone();
                spec.database = saved.dbname.clone();
            }
            let mut options: Vec<(String, String)> = saved
                .options
                .iter()
                .filter(|(key, _)| !key.starts_with("vault_"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            options.sort();
            spec.options = options;
            self.show_spec(&spec, window, cx);
        }
        self.apply_meta(meta, window, cx);
    }

    /// Fill the form from a URL (a recent connection).
    pub fn load_url(
        &mut self,
        url: &str,
        name: Option<String>,
        meta: &ConnectionMeta,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset(window, cx);
        self.seeded = true;
        if let Some(target) = urls::parse_target(url) {
            self.show_target(target, window, cx);
        }
        if let Some(name) = name {
            self.set_text(&self.name.clone(), &name, window, cx);
        }
        self.apply_meta(meta, window, cx);
    }

    /// Fill the form from a Docker container.
    /// Fill the form from a Docker container found by discovery. `daemon` is
    /// the one that listed it: Test and Connect go to that very daemon. No
    /// user, password or database is filled in: the container's own
    /// environment supplies them unless the user types some.
    pub fn load_docker(
        &mut self,
        container: &DockerContainer,
        daemon: Option<DockerDaemon>,
        meta: &ConnectionMeta,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset(window, cx);
        self.seeded = true;
        self.docker_daemon = daemon;
        self.kind = Kind::Docker;
        // The blank form starts with PostgreSQL's login: not the container's
        for input in [
            &self.user.clone(),
            &self.password.clone(),
            &self.database.clone(),
        ] {
            self.set_text(input, "", window, cx);
        }
        self.set_text(&self.container.clone(), &container.name, window, cx);
        self.set_text(&self.name.clone(), &container.name, window, cx);
        self.apply_meta(meta, window, cx);
    }

    fn apply_meta(&mut self, meta: &ConnectionMeta, window: &mut Window, cx: &mut Context<Self>) {
        self.env = meta.env_tag();
        self.read_only = meta.read_only;
        self.refresh_name_hint(window, cx);
        cx.notify();
    }

    // ── Reading and writing fields ────────────────────────────────────

    pub(super) fn text(&self, input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().to_string()
    }

    pub(super) fn set_text(
        &self,
        input: &Entity<InputState>,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        input.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn selected_ssl_mode(&self, cx: &App) -> Option<&'static str> {
        self.ssl_mode
            .read(cx)
            .selected_value()
            .copied()
            .filter(|mode| *mode != SSL_DEFAULT)
    }

    /// Settings → "Open connections read-only": the backend makes every
    /// connection read-only whatever the switch says.
    fn forced_read_only(cx: &App) -> bool {
        super::details::forced_read_only(cx)
    }

    fn effective_read_only(&self, cx: &App) -> bool {
        self.read_only || Self::forced_read_only(cx)
    }

    /// The fields as a target. `strict` reports what is missing; otherwise
    /// invalid parts are skipped (for previews).
    fn target(&self, strict: bool, cx: &App) -> Result<Target, FieldError> {
        let fail = |field: Field, message: &str| FieldError {
            field,
            message: message.to_string().into(),
        };
        let (home, cwd) = environment_dirs();
        match &self.kind {
            Kind::Engine(db) => {
                let mut spec = ConnectionSpec::new(db.clone());
                spec.scheme = self.scheme.clone();
                spec.options = self.extra_options.clone();
                if db.is_file_based() {
                    spec.path = self.text(&self.path, cx);
                    spec.resolve_path(home.as_deref(), cwd.as_deref());
                    if *db == DatabaseType::CSV {
                        spec.set_option(
                            "header",
                            Some(if self.csv_header { "true" } else { "false" }),
                        );
                        let delimiter = self.text(&self.delimiter, cx);
                        let delimiter = if delimiter == "\\t" {
                            "\t".to_string()
                        } else {
                            delimiter
                        };
                        spec.set_option(
                            "delimiter",
                            Some(delimiter.as_str()).filter(|d| !d.is_empty() && *d != ","),
                        );
                    }
                    if strict && let Err(message) = urls::validate(&spec) {
                        return Err(fail(Field::Path, &message));
                    }
                } else {
                    spec.host = self.text(&self.host, cx).trim().to_string();
                    if strict && spec.host.is_empty() {
                        return Err(fail(Field::Host, "Enter a host name or IP address"));
                    }
                    spec.port = match urls::parse_port(&self.text(&self.port, cx)) {
                        Ok(port) => port,
                        Err(message) if strict => return Err(fail(Field::Port, &message)),
                        Err(_) => None,
                    };
                    spec.user = self.text(&self.user, cx).trim().to_string();
                    spec.password = self.text(&self.password, cx);
                    if urls::uses_database_name(db) {
                        spec.database = self.text(&self.database, cx).trim().to_string();
                    }
                    match urls::tls_setting(db) {
                        TlsSetting::SslMode => {
                            spec.set_option("sslmode", self.selected_ssl_mode(cx))
                        }
                        TlsSetting::Flag(_) => spec.set_tls(self.tls),
                        TlsSetting::None => {}
                    }
                }
                Ok(Target::Database(spec))
            }
            Kind::Docker => {
                let spec = DockerSpec {
                    user: self.text(&self.user, cx).trim().to_string(),
                    password: self.text(&self.password, cx),
                    container: self.text(&self.container, cx).trim().to_string(),
                    database: self.text(&self.database, cx).trim().to_string(),
                    options: self.extra_options.clone(),
                    use_tls: self.tls,
                };
                if strict {
                    if spec.container.is_empty() {
                        return Err(fail(Field::Container, "Enter the container's name"));
                    }
                    if let Err(message) = urls::validate_docker(&spec) {
                        return Err(fail(Field::Container, &message));
                    }
                }
                Ok(Target::Docker(spec))
            }
            Kind::Vault => match &self.vault {
                Some(spec) if spec.is_complete() => Ok(Target::Vault(spec.clone())),
                _ if strict => Err(fail(
                    Field::Name,
                    "Choose the Vault role and database first",
                )),
                other => Ok(Target::Vault(other.clone().unwrap_or_default())),
            },
        }
    }

    /// Show a parsed target in the fields.
    fn show_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.scheme = None;
        self.extra_options.clear();
        self.vault = None;
        match target {
            Target::Database(spec) => {
                let engine = spec.engine.clone();
                self.kind = Kind::Engine(engine.clone());
                self.engine_select.update(cx, |select, cx| {
                    select.set_selected_value(&engine, window, cx);
                });
                self.show_spec(&spec, window, cx);
            }
            Target::Docker(spec) => {
                self.kind = Kind::Docker;
                self.set_text(&self.container.clone(), &spec.container, window, cx);
                self.set_text(&self.user.clone(), &spec.user, window, cx);
                self.set_text(&self.database.clone(), &spec.database, window, cx);
                self.set_password(&spec.password, window, cx);
                // What the URL carries beyond the fields stays with the connection
                self.extra_options = spec.options.clone();
                self.tls = spec.use_tls;
                self.advanced_open = self.tls || !self.extra_options.is_empty();
            }
            Target::Vault(spec) => {
                self.kind = Kind::Vault;
                self.vault = Some(spec);
            }
        }
        self.refresh_name_hint(window, cx);
        cx.notify();
    }

    /// Show a connection spec in the fields of its engine.
    fn show_spec(&mut self, spec: &ConnectionSpec, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = urls::form_defaults(&spec.engine);
        self.scheme = spec.scheme.clone();
        if spec.engine.is_file_based() {
            self.set_text(&self.path.clone(), &spec.path, window, cx);
        } else {
            let host = if spec.host.is_empty() {
                defaults.host
            } else {
                &spec.host
            };
            self.set_text(&self.host.clone(), host, window, cx);
            let port = spec.port.or(defaults.port);
            self.set_text(
                &self.port.clone(),
                &port.map(|port| port.to_string()).unwrap_or_default(),
                window,
                cx,
            );
            self.set_text(&self.user.clone(), &spec.user, window, cx);
            self.set_text(&self.database.clone(), &spec.database, window, cx);
            self.set_password(&spec.password, window, cx);
        }
        self.apply_options(spec, window, cx);
    }

    fn set_password(&mut self, password: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A recent entry's sanitized URL carries a placeholder, not a secret
        let value = if password == PASSWORD_MASK || password == "[REDACTED]" {
            ""
        } else {
            password
        };
        self.set_text(&self.password.clone(), value, window, cx);
    }

    /// Route a parsed URL's options into the controls that own them; the
    /// rest stay in `extra_options`.
    fn apply_options(
        &mut self,
        spec: &ConnectionSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut rest = Vec::new();
        self.tls = false;
        let mut ssl_choice = SSL_DEFAULT;
        for (key, value) in &spec.options {
            match (&spec.engine, key.as_str()) {
                (DatabaseType::PostgreSQL, "sslmode") => {
                    match SSL_MODES.iter().find(|mode| **mode == value.as_str()) {
                        Some(mode) => ssl_choice = mode,
                        None => rest.push((key.clone(), value.clone())),
                    }
                }
                (DatabaseType::CSV, "header") => {
                    self.csv_header = value.eq_ignore_ascii_case("true")
                }
                (DatabaseType::CSV, "delimiter") => {
                    let shown = if value == "\t" { "\\t" } else { value.as_str() };
                    self.set_text(&self.delimiter.clone(), shown, window, cx);
                }
                (_, "ssl" | "tls" | "secure")
                    if matches!(urls::tls_setting(&spec.engine), TlsSetting::Flag(_)) =>
                {
                    self.tls = matches!(
                        value.to_ascii_lowercase().as_str(),
                        "true" | "1" | "yes" | "on"
                    );
                }
                _ => rest.push((key.clone(), value.clone())),
            }
        }
        self.extra_options = rest;
        self.ssl_mode.update(cx, |select, cx| {
            select.set_selected_value(&ssl_choice, window, cx);
        });
        // Show the settings the URL changed
        self.advanced_open = self.advanced_open
            || ssl_choice != SSL_DEFAULT
            || self.tls
            || !self.extra_options.is_empty();
    }

    // ── Engines and defaults ──────────────────────────────────────────

    /// Fill host, port and user with the engine's real defaults. A field the
    /// user has changed from the previous engine's default is left alone.
    fn apply_defaults(
        &mut self,
        engine: &DatabaseType,
        previous: Option<&DatabaseType>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let defaults = urls::form_defaults(engine);
        let old = previous.map(urls::form_defaults);
        let fill = |this: &Self,
                    input: &Entity<InputState>,
                    old_default: Option<String>,
                    new_default: String,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            let current = this.text(input, cx);
            let untouched = current.trim().is_empty()
                || old_default.is_some_and(|old_default| old_default == current.trim());
            if untouched {
                this.set_text(input, &new_default, window, cx);
            }
        };
        let port = |defaults: &urls::FormDefaults| {
            defaults
                .port
                .map(|port| port.to_string())
                .unwrap_or_default()
        };
        fill(
            self,
            &self.host.clone(),
            old.map(|old| old.host.to_string()),
            defaults.host.to_string(),
            window,
            cx,
        );
        fill(
            self,
            &self.port.clone(),
            old.as_ref().map(port),
            port(&defaults),
            window,
            cx,
        );
        fill(
            self,
            &self.user.clone(),
            old.map(|old| old.user.to_string()),
            defaults.user.to_string(),
            window,
            cx,
        );
        self.refresh_name_hint(window, cx);
    }

    pub(super) fn set_engine(
        &mut self,
        engine: DatabaseType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = match &self.kind {
            Kind::Engine(db) => Some(db.clone()),
            _ => None,
        };
        if previous.as_ref() == Some(&engine) {
            return;
        }
        self.kind = Kind::Engine(engine.clone());
        self.scheme = None;
        self.extra_options.clear();
        self.tls = false;
        self.error = None;
        self.touch(true, cx);
        self.ssl_mode.update(cx, |select, cx| {
            select.set_selected_value(&SSL_DEFAULT, window, cx);
        });
        self.apply_defaults(&engine, previous.as_ref(), window, cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// The Name field's hint is the name Save would use.
    fn refresh_name_hint(&self, window: &mut Window, cx: &mut Context<Self>) {
        let suggestion = self
            .target(false, cx)
            .ok()
            .map(|target| urls::suggest_session_name(&target))
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Untitled connection".to_string());
        self.name.update(cx, |input, cx| {
            input.set_placeholder(suggestion, window, cx)
        });
    }

    // ── URL import ────────────────────────────────────────────────────

    fn toggle_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.import_open = !self.import_open;
        self.import_error = None;
        if self.import_open {
            // A URL on the clipboard saves the paste
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let text = text.trim().to_string();
                if text.contains("://") && urls::parse_target(&text).is_some() {
                    self.set_text(&self.import.clone(), &text, window, cx);
                }
            }
            self.import.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn apply_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(&self.import, cx);
        let (home, cwd) = environment_dirs();
        match urls::import_target(&text, home.as_deref(), cwd.as_deref()) {
            Ok(target) => {
                // A pasted URL is a plain one: another subject, no pinned daemon
                self.new_subject(cx);
                self.show_target(target, window, cx);
                self.set_text(&self.import.clone(), "", window, cx);
                self.import_open = false;
                self.import_error = None;
                self.error = None;
                self.focus(window, cx);
            }
            Err(message) => self.import_error = Some(message.into()),
        }
        cx.notify();
    }

    // ── Acting ────────────────────────────────────────────────────────

    /// Everything the page needs to act on the form, or what is missing.
    pub(super) fn submission(&self, saving: bool, cx: &App) -> Result<Submission, FieldError> {
        let target = self.target(true, cx)?;
        let typed = self.text(&self.name, cx).trim().to_string();
        let name = if typed.is_empty() {
            urls::suggest_session_name(&target)
        } else {
            typed.clone()
        };
        if saving {
            let taken = AppState::global(cx)
                .read(cx)
                .store()
                .read(|config| config.get_session(&name).is_some());
            let own_name = matches!(&self.mode, FormMode::Edit { session } if *session == name);
            if taken && !own_name {
                return Err(FieldError {
                    field: Field::Name,
                    message: format!("A saved connection named “{name}” already exists").into(),
                });
            }
        }
        let pin = self.docker_daemon.as_ref().and_then(launcher::daemon_pin);
        let url = urls::build_target_url(&target);
        let (request, recent_url) = match &target {
            Target::Docker(spec) => (
                docker_request(self.docker_daemon.as_ref(), spec),
                spec.to_target().render(),
            ),
            _ => (
                ConnectRequest::url(url.clone()),
                dbcrust_gui_core::dbcrust::password_sanitizer::sanitize_connection_url(&url),
            ),
        };
        Ok(Submission {
            url,
            request,
            identity: TargetIdentity::from_target(&target, pin.as_deref()),
            recent_url,
            generation: self.generation,
            attempt_key: self.attempt_key(),
            revision: self.fields_revision,
            subject: if typed.is_empty() {
                urls::describe_target(&target)
            } else {
                typed
            },
            name,
            read_only: self.effective_read_only(cx),
            env: self.env.clone(),
            mode: self.mode.clone(),
        })
    }

    fn act(
        &mut self,
        saving: bool,
        event: impl FnOnce(Submission) -> FormEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if attempts::is_busy(&self.attempt_key(), cx) {
            return;
        }
        // One Save of this subject at a time
        if saving && self.saving == Some(self.generation) {
            return;
        }
        match self.submission(saving, cx) {
            Ok(submission) => {
                self.error = None;
                if saving {
                    self.saving = Some(submission.generation);
                }
                cx.emit(event(submission));
            }
            Err(error) => {
                let input = match error.field {
                    Field::Name => &self.name,
                    Field::Host => &self.host,
                    Field::Port => &self.port,
                    Field::Path => &self.path,
                    Field::Container => &self.container,
                };
                input.update(cx, |input, cx| input.focus(window, cx));
                self.error = Some(error);
            }
        }
        cx.notify();
    }

    pub fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A Vault target that is not complete yet needs its wizard first
        if self.kind == Kind::Vault && self.vault.as_ref().is_none_or(|spec| !spec.is_complete()) {
            self.open_vault(window, cx);
            return;
        }
        self.act(false, FormEvent::Connect, window, cx);
    }

    fn test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.act(false, FormEvent::Test, window, cx);
    }

    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.act(true, FormEvent::Save, window, cx);
    }

    fn open_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.vault.clone().unwrap_or_default();
        vault::open_wizard(spec, self.effective_read_only(cx), window, cx);
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                this.update_in(cx, |this, window, cx| this.choose_file(path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Use a picked file; a file of another format switches the engine
    /// (`.csv` picked while SQLite is selected, …).
    fn choose_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.to_string_lossy().into_owned();
        if let Some((engine, defaults)) = urls::detect_file_engine(&path) {
            if self.kind != Kind::Engine(engine.clone()) {
                self.engine_select.update(cx, |select, cx| {
                    select.set_selected_value(&engine, window, cx);
                });
                self.set_engine(engine.clone(), window, cx);
            }
            if engine == DatabaseType::CSV {
                let tab = defaults
                    .iter()
                    .any(|(key, value)| *key == "delimiter" && *value == "\t");
                self.set_text(
                    &self.delimiter.clone(),
                    if tab { "\\t" } else { "" },
                    window,
                    cx,
                );
            }
        }
        self.set_text(&self.path.clone(), &path, window, cx);
        self.touch(true, cx);
        self.error = None;
        cx.notify();
    }
}

fn environment_dirs() -> (Option<String>, Option<String>) {
    let home = std::env::var("HOME").ok().filter(|home| !home.is_empty());
    let cwd = std::env::current_dir()
        .ok()
        .map(|dir| dir.to_string_lossy().into_owned());
    (home, cwd)
}

/// The request that connects to a container with the form's user, password,
/// database and options. Through `daemon` (the one that listed the container)
/// when there is one.
fn docker_request(daemon: Option<&DockerDaemon>, spec: &DockerSpec) -> ConnectRequest {
    let target = spec.to_target();
    let Some(daemon) = daemon else {
        return ConnectRequest::info(target.to_connection_info());
    };
    let mut request = daemon.connect_request(&target.container);
    if let ConnectTarget::Info(info) = &mut request.target {
        info.username = target.user.clone();
        info.password = target.password.clone();
        info.database = target.database.clone();
        // The pin travels in the options: keep it, add the user's
        info.options.extend(target.options.clone());
        info.use_tls = target.use_tls;
    }
    request
}

/// Save a session without connecting: parse the URL the way DBCrust does,
/// write it to `sessions.toml`, and drop the old name of a renamed one.
/// Passwords are not part of a saved session.
pub(super) fn save_session(
    store: &ConfigStore,
    name: &str,
    url: &str,
    renamed_from: Option<String>,
) -> GuiResult<()> {
    let info = match dbcrust_gui_core::dbcrust::docker::DockerTarget::parse(url) {
        // A container keeps its user, database, options and TLS, but no password
        Some(target) => {
            let mut info = target.to_connection_info();
            if target.use_tls {
                info.options.insert("ssl".to_string(), "true".to_string());
            }
            info
        }
        None => ConnectionInfo::parse_url(url)
            .map_err(|e| GuiError::Storage(format!("Could not read this connection: {e}")))?,
    };
    store
        .update_sessions(|config| -> Result<(), Box<dyn std::error::Error>> {
            config.save_session_from_connection_info(name, &info)?;
            if let Some(old) = renamed_from.as_deref() {
                config.delete_session(old)?;
            }
            Ok(())
        })
        .map_err(|e| GuiError::Storage(format!("Could not save “{name}”: {e}")))
}

impl Focusable for ConnectionForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Rendering ─────────────────────────────────────────────────────────

impl ConnectionForm {
    fn form(&self) -> Form {
        v_form().columns(6).label_text_size(rems(0.75))
    }

    fn field_error(&self, field: Field, cx: &App) -> Option<AnyElement> {
        self.error
            .as_ref()
            .filter(|error| error.field == field)
            .map(|error| {
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(error.message.clone())
                    .into_any_element()
            })
    }

    /// An input with the message of its field underneath.
    fn with_error(&self, field: Field, input: impl IntoElement, cx: &App) -> AnyElement {
        v_flex()
            .w_full()
            .child(input)
            .children(self.field_error(field, cx))
            .into_any_element()
    }

    fn render_title(&self, cx: &Context<Self>) -> impl IntoElement {
        let (title, subtitle) = match &self.mode {
            FormMode::New => (
                "New connection".to_string(),
                "Connect once, or save it to keep it in the list.",
            ),
            FormMode::Edit { session } => (
                format!("Edit “{session}”"),
                "Changes apply the next time you connect.",
            ),
        };
        h_flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(22.))
                            .line_height(relative(1.2))
                            .font_semibold()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(subtitle),
                    ),
            )
            .child(
                Button::new("paste-url")
                    .outline()
                    .icon(IconName::ClipboardPaste)
                    .label("Paste connection URL…")
                    .selected(self.import_open)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_import(window, cx))),
            )
    }

    fn render_import(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.import_open {
            return None;
        }
        let theme = cx.theme();
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .p_3()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted.opacity(0.35))
                .child(div().text_xs().text_color(theme.muted_foreground).child(
                    "Paste a URL (postgres://, mysql://, sqlite://, mongodb://, \
                             docker://, vault://…) or the path of a data file. The fields \
                             below are filled in; options they have no control for are kept.",
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.import).cleanable(true)),
                        )
                        .child(
                            Button::new("apply-import")
                                .primary()
                                .label("Fill form")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.apply_import(window, cx)
                                    }),
                                ),
                        ),
                )
                .children(
                    self.import_error
                        .clone()
                        .map(|message| div().text_xs().text_color(theme.danger).child(message)),
                )
                .into_any_element(),
        )
    }

    /// A read-only look-alike of the engine picker for Docker and Vault
    /// targets, which are sources, not engines.
    fn render_source(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (badge, label) = match self.kind {
            Kind::Docker => (docker_badge(22., cx), "Docker container"),
            _ => (vault_badge(22., cx), "Vault dynamic credentials"),
        };
        h_flex()
            .w_full()
            .h(px(32.))
            .px_2p5()
            .gap_2p5()
            .items_center()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.3))
            .text_sm()
            .child(badge)
            .child(div().flex_1().child(label))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Source"),
            )
            .into_any_element()
    }

    fn render_fields(&self, cx: &Context<Self>) -> AnyElement {
        let suggestion = self
            .target(false, cx)
            .ok()
            .map(|target| urls::suggest_session_name(&target))
            .filter(|name| !name.trim().is_empty());
        let name = self.with_error(Field::Name, Input::new(&self.name).cleanable(true), cx);
        let engine: AnyElement = match &self.kind {
            Kind::Engine(_) => Select::new(&self.engine_select)
                .search_placeholder("Search engines…")
                .menu_max_h(px(500.))
                .placeholder("Choose an engine")
                .into_any_element(),
            _ => self.render_source(cx),
        };
        let mut form =
            self.form()
                .child(field().label("Name").col_span(6).child(name).description(
                    match suggestion {
                        Some(suggestion) => {
                            format!("Shown in the list. Empty saves it as “{suggestion}”.")
                        }
                        None => "Shown in the list when you save it.".to_string(),
                    },
                ))
                .child(field().label("Engine").col_span(6).child(engine));

        match &self.kind {
            Kind::Engine(db) if db.is_file_based() => {
                let browse = Button::new("browse")
                    .ghost()
                    .xsmall()
                    .label("Browse…")
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx)));
                let defaults = urls::form_defaults(db);
                form = form.child(
                    field()
                        .label(format!("{} file", db.display_name()))
                        .col_span(6)
                        .child(self.with_error(
                            Field::Path,
                            Input::new(&self.path).suffix(browse),
                            cx,
                        ))
                        .description(format!("For example {}", defaults.path_hint)),
                );
                if *db == DatabaseType::CSV {
                    form = form
                        .child(
                            field()
                                .label("Delimiter")
                                .col_span(2)
                                .child(Input::new(&self.delimiter))
                                .description("Empty means a comma; \\t is a tab"),
                        )
                        .child(
                            field().label("Header").col_span(4).child(
                                h_flex().h(px(32.)).child(
                                    Switch::new("csv-header")
                                        .checked(self.csv_header)
                                        .label("First row holds the column names")
                                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                            this.csv_header = *checked;
                                            this.touch(true, cx);
                                            cx.notify();
                                        })),
                                ),
                            ),
                        );
                }
            }
            Kind::Engine(db) => {
                let defaults = urls::form_defaults(db);
                form = form
                    .child(field().label("Host").col_span(4).child(self.with_error(
                        Field::Host,
                        Input::new(&self.host),
                        cx,
                    )))
                    .child(field().label("Port").col_span(2).child(self.with_error(
                        Field::Port,
                        Input::new(&self.port),
                        cx,
                    )))
                    .child(
                        field()
                            .label("User")
                            .col_span(3)
                            .child(Input::new(&self.user)),
                    )
                    .child(
                        field().label("Password").col_span(3).child(
                            Input::new(&self.password)
                                .mask_toggle()
                                .content_type(InputContentType::Password),
                        ),
                    );
                if let Some(hint) = defaults.database_hint {
                    form = form.child(
                        field()
                            .label("Database")
                            .col_span(6)
                            .child(Input::new(&self.database))
                            .description(hint),
                    );
                }
            }
            Kind::Docker => {
                form = form
                    .child(
                        field()
                            .label("Container")
                            .col_span(6)
                            .child(self.with_error(
                                Field::Container,
                                Input::new(&self.container),
                                cx,
                            )),
                    )
                    .child(
                        field()
                            .label("User")
                            .col_span(3)
                            .child(Input::new(&self.user)),
                    )
                    .child(
                        field().label("Password").col_span(3).child(
                            Input::new(&self.password)
                                .mask_toggle()
                                .content_type(InputContentType::Password),
                        ),
                    )
                    .child(
                        field()
                            .label("Database")
                            .col_span(6)
                            .child(Input::new(&self.database))
                            .description(
                                "User, password and database come from the container's \
                                 environment unless you set them here.",
                            ),
                    );
            }
            Kind::Vault => {
                let summary = self
                    .vault
                    .as_ref()
                    .filter(|spec| spec.is_complete())
                    .map(urls::build_vault_url);
                form = form.child(
                    field().label("Role").col_span(6).child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div().flex_1().min_w_0().child(match summary {
                                    Some(url) => mono(url, cx).into_any_element(),
                                    None => div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Not chosen yet")
                                        .into_any_element(),
                                }),
                            )
                            .child(
                                Button::new("open-vault")
                                    .outline()
                                    .label("Choose…")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_vault(window, cx)
                                    })),
                            ),
                    ),
                );
            }
        }
        form.into_any_element()
    }

    /// Encryption summary, its disclosure, and the options kept from a URL.
    fn render_encryption(&self, cx: &Context<Self>) -> Option<AnyElement> {
        // The TLS control of the kind, and what its settings say
        let (tls_control, summary) = match &self.kind {
            Kind::Engine(db) => {
                let Ok(Target::Database(spec)) = self.target(false, cx) else {
                    return None;
                };
                (urls::tls_setting(db), urls::tls_summary(&spec))
            }
            Kind::Docker => (
                TlsSetting::Flag("ssl"),
                Some(
                    if self.tls {
                        "On · encrypted with TLS"
                    } else {
                        "Default · the container's engine decides"
                    }
                    .to_string(),
                ),
            ),
            Kind::Vault => return None,
        };
        let kept = urls::describe_kept_options(&self.extra_options);
        if summary.is_none() && kept.is_none() {
            return None;
        }
        let theme = cx.theme();
        let open = self.advanced_open;
        let controls = match tls_control {
            TlsSetting::SslMode => Some(
                field()
                    .label("SSL mode")
                    .col_span(6)
                    .child(Select::new(&self.ssl_mode).placeholder(SSL_DEFAULT)),
            ),
            TlsSetting::Flag(_) => Some(
                field().label("Encryption").col_span(6).child(
                    h_flex().h(px(32.)).child(
                        Switch::new("tls")
                            .checked(self.tls)
                            .label("Use TLS")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.tls = *checked;
                                this.touch(true, cx);
                                cx.notify();
                            })),
                    ),
                ),
            ),
            TlsSetting::None => None,
        };
        Some(
            v_flex()
                .w_full()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .id("encryption-toggle")
                        .px_3()
                        .min_h(px(44.))
                        .gap_3()
                        .items_center()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.advanced_open = !this.advanced_open;
                            cx.notify();
                        }))
                        .child(
                            Icon::new(IconName::ShieldCheck)
                                .size(px(16.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().font_medium().child("Encryption"))
                                .children(summary.map(|summary| {
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(summary)
                                })),
                        )
                        .child(
                            Icon::new(if open {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(14.))
                            .text_color(theme.muted_foreground),
                        ),
                )
                .when(open, |this| {
                    this.child(
                        v_flex()
                            .px_3()
                            .pb_3()
                            .gap_2()
                            .children(controls.map(|control| self.form().child(control)))
                            .children(kept.map(|kept| {
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(kept)
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_environment(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (selected_bg, hover_bg) = (theme.secondary, theme.list_hover);
        let current = self.env.as_ref().map(|tag| tag.label.clone());
        let pill = |label: &'static str, color: Option<EnvColor>| {
            let selected = match (&current, color) {
                (None, None) => true,
                (Some(current), Some(_)) => current.eq_ignore_ascii_case(label),
                _ => false,
            };
            let tag = color.map(|color| EnvTag {
                label: label.to_string(),
                color,
            });
            h_flex()
                .id(SharedString::from(format!("env-{label}")))
                .h(px(28.))
                .px_2p5()
                .gap_1p5()
                .items_center()
                .rounded(px(7.))
                .border_1()
                .border_color(if selected {
                    theme.ring.opacity(0.6)
                } else {
                    theme.border
                })
                .text_sm()
                .cursor_pointer()
                .when(selected, |this| this.bg(selected_bg).font_medium())
                .when(!selected, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(move |style| style.bg(hover_bg))
                })
                .children(
                    color.map(|color| div().size(px(8.)).rounded_full().bg(env_swatch(color, cx))),
                )
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.env = tag.clone();
                    this.touch(false, cx);
                    cx.notify();
                }))
        };
        v_flex()
            .w_full()
            .gap_2()
            .child(group_label("Environment", cx))
            .child(
                h_flex()
                    .gap_1p5()
                    .flex_wrap()
                    .child(pill("None", None))
                    .children(
                        ENVIRONMENT_PRESETS
                            .iter()
                            .map(|(label, color)| pill(label, Some(*color))),
                    ),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Shown next to the connection everywhere, so production never looks like local.",
            ))
            .into_any_element()
    }

    fn render_read_only(&self, cx: &Context<Self>) -> AnyElement {
        let forced = Self::forced_read_only(cx);
        v_flex()
            .gap_1()
            .child(
                Switch::new("read-only")
                    .checked(self.effective_read_only(cx))
                    .disabled(forced)
                    .label("Read-only")
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.read_only = *checked;
                        this.touch(true, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if forced {
                        "Settings make every connection read-only."
                    } else {
                        "Blocks writes, and opens the connection read-only where the engine \
                         supports it."
                    }),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let busy = attempts::is_busy(&self.attempt_key(), cx);
        let saving = self.saving == Some(self.generation);
        let target = self
            .target(false, cx)
            .ok()
            .filter(|target| match target {
                Target::Docker(spec) => !spec.container.is_empty(),
                Target::Vault(spec) => spec.is_complete(),
                Target::Database(spec) => spec.engine.is_file_based() || !spec.host.is_empty(),
            })
            .map(|target| urls::build_masked_target_url(&target));
        let left = match target {
            Some(text) => h_flex()
                .gap_2()
                .items_center()
                .min_w_0()
                .child(
                    Icon::new(IconName::Link)
                        .size(px(13.))
                        .text_color(theme.muted_foreground)
                        .flex_none(),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(mono(text, cx).text_color(theme.muted_foreground)),
                )
                .into_any_element(),
            None => div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("The target shows here once the form is filled in.")
                .into_any_element(),
        };
        let editing = matches!(self.mode, FormMode::Edit { .. });
        let vault_pending =
            self.kind == Kind::Vault && self.vault.as_ref().is_none_or(|spec| !spec.is_complete());
        let can_save = self.kind != Kind::Vault;
        let connect_label = if vault_pending {
            "Choose Vault role…"
        } else {
            "Connect"
        };
        let right = h_flex()
            .gap_2()
            .child(
                Button::new("form-test")
                    .outline()
                    .label("Test")
                    .disabled(busy || vault_pending)
                    .tooltip("Connect and close again: nothing is opened or recorded")
                    .on_click(cx.listener(|this, _, window, cx| this.test(window, cx))),
            )
            .child(
                Button::new("form-save")
                    .outline()
                    .label(if editing { "Save changes" } else { "Save" })
                    .disabled(busy || saving || !can_save)
                    .loading(saving)
                    .tooltip(if can_save {
                        "Keep it in Saved without connecting"
                    } else {
                        "Connect first, then save the session from the workspace"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
            .child(
                Button::new("form-connect")
                    .primary()
                    .label(connect_label)
                    .disabled(busy)
                    .loading(busy)
                    .tooltip(format!("{}↵", super::ui::modifier_label()))
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
            );
        footer_bar(left, right, cx)
    }
}

impl Render for ConnectionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Both the connect and the test of this subject (and nothing else)
        let edit: Option<attempts::EditHandler> = None;
        let panels = attempts::attempt_panels(&self.attempt_key(), edit, cx);

        let body = v_flex()
            .w_full()
            .gap_5()
            .child(self.render_title(cx))
            .children(self.render_import(cx))
            .child(self.render_fields(cx))
            .children(self.render_encryption(cx))
            .child(self.render_environment(cx))
            .child(self.render_read_only(cx))
            .children(panels)
            .child(
                h_flex()
                    .gap_4()
                    .child(shortcut_hint(
                        &[&format!("{}↵", super::ui::modifier_label())],
                        "Connect",
                        cx,
                    ))
                    .child(shortcut_hint(&["Esc"], "Close the form", cx)),
            );

        v_flex()
            .id("connection-form")
            .key_context("ConnectionForm")
            .track_focus(&self.focus_handle)
            .size_full()
            .child(scroll_body("form-body", body))
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::searchable_list::SearchableListDelegate;
    // `gpui_kit::*` (via `super::*`) exports GPUI's `test` macro
    use std::prelude::v1::test;

    #[test]
    fn the_picker_groups_servers_before_files_and_lists_every_engine_once() {
        let list = engine_list();
        let mut seen = Vec::new();
        for section in 0..2 {
            let items = SearchableListDelegate::items_count(&list, section);
            for row in 0..items {
                let item = SearchableListDelegate::item(
                    &list,
                    IndexPath::default().section(section).row(row),
                )
                .expect("item");
                assert_eq!(
                    item.engine.is_file_based(),
                    section == 1,
                    "{} sits in the wrong group",
                    item.engine
                );
                seen.push(item.engine.clone());
            }
        }
        assert_eq!(seen.len(), DatabaseType::supported_types().len());
        for engine in DatabaseType::supported_types() {
            assert_eq!(seen.iter().filter(|seen| **seen == engine).count(), 1);
        }
    }

    #[test]
    fn engine_indexes_point_at_their_engine() {
        let list = engine_list();
        for engine in DatabaseType::supported_types() {
            let ix = engine_index(&engine);
            let item = SearchableListDelegate::item(&list, ix).expect("item");
            assert_eq!(item.engine, engine);
        }
    }

    #[test]
    fn the_picker_searches_names_and_aliases() {
        let postgres = EngineItem {
            engine: DatabaseType::PostgreSQL,
        };
        assert!(postgres.matches("postg"));
        assert!(postgres.matches("pg"));
        assert!(!postgres.matches("sqlite"));
        let duck = EngineItem {
            engine: DatabaseType::DuckDB,
        };
        assert!(duck.matches("duck"));
    }

    #[test]
    fn ssl_choices_start_with_the_default() {
        let choices = ssl_choices();
        assert_eq!(choices[0], SSL_DEFAULT);
        assert_eq!(&choices[1..], SSL_MODES);
    }

    #[test]
    fn a_session_is_saved_without_connecting() {
        let store = ConfigStore::from_config(dbcrust_gui_core::dbcrust::config::Config::default());
        // The config directory of tests is isolated by DBCrust itself
        let result = save_session(
            &store,
            "analytics",
            "postgres://analyst@warehouse.internal:5432/analytics?sslmode=require",
            None,
        );
        if let Err(error) = result {
            panic!("save failed: {error}");
        }
        let sessions = store.sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "analytics");
        assert_eq!(sessions[0].user, "analyst");
        assert!(sessions[0].target.contains("warehouse.internal"));
        // Renaming replaces the old entry
        save_session(
            &store,
            "analytics-ro",
            "postgres://analyst@warehouse.internal:5432/analytics",
            Some("analytics".into()),
        )
        .expect("rename");
        let names: Vec<_> = store.sessions().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["analytics-ro"]);
    }

    #[test]
    fn saving_refuses_what_dbcrust_cannot_parse() {
        let store = ConfigStore::from_config(dbcrust_gui_core::dbcrust::config::Config::default());
        assert!(save_session(&store, "x", "not a url", None).is_err());
        assert!(store.sessions().is_empty());
    }
}
