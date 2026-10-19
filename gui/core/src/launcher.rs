//! The Connections page's pure model: which items the list shows (saved
//! sessions, recent connections, Docker containers), how a search and a
//! filter narrow them, how they are identified, what the details pane says
//! about a Docker container, and the display metadata (environment label and
//! colour, read-only preference) the page keeps per connection in
//! `gui_state.toml`.
//!
//! No GPUI here: the view renders what [`build_listing`] returns.

use crate::docker::{DockerContainer, DockerDaemon, ReachHint};
use crate::store::{RecentEntry, SessionEntry};
use crate::types::{ConnectRequest, ConnectTarget};
use crate::url_builder::{self as urls, Target};
use dbcrust_app::config::SavedSession;
use dbcrust_core::database::{DatabaseType, DatabaseTypeExt};
use dbcrust_docker::docker::PINNED_DAEMON_OPTION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

// ── Display metadata (persisted) ───────────────────────────────────────

/// The colour of an environment tag. Always shown next to its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvColor {
    Green,
    Blue,
    Amber,
    Red,
    Purple,
    Gray,
}

impl EnvColor {
    pub const ALL: [EnvColor; 6] = [
        EnvColor::Green,
        EnvColor::Blue,
        EnvColor::Amber,
        EnvColor::Red,
        EnvColor::Purple,
        EnvColor::Gray,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EnvColor::Green => "Green",
            EnvColor::Blue => "Blue",
            EnvColor::Amber => "Amber",
            EnvColor::Red => "Red",
            EnvColor::Purple => "Purple",
            EnvColor::Gray => "Gray",
        }
    }
}

/// The environments the form offers: label and the colour it gets.
pub const ENVIRONMENT_PRESETS: [(&str, EnvColor); 4] = [
    ("Local", EnvColor::Green),
    ("Development", EnvColor::Blue),
    ("Staging", EnvColor::Amber),
    ("Production", EnvColor::Red),
];

/// The colour a preset label implies (`Gray` for anything else).
pub fn preset_color(label: &str) -> EnvColor {
    ENVIRONMENT_PRESETS
        .iter()
        .find(|(preset, _)| preset.eq_ignore_ascii_case(label.trim()))
        .map_or(EnvColor::Gray, |(_, color)| *color)
}

/// An environment label with its colour. The label is never empty: a colour
/// without text is not an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvTag {
    pub label: String,
    pub color: EnvColor,
}

const MAX_ENVIRONMENT_LEN: usize = 24;
const MAX_META_ENTRIES: usize = 1_000;

/// What the Connections page remembers about one connection beyond what
/// DBCrust's own storage holds. Never a secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionMeta {
    /// Identity of the connection ([`session_key`], [`recent_key`], …).
    pub key: String,
    /// Environment label (`Production`); empty for none.
    pub environment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<EnvColor>,
    /// Open this connection read-only unless asked otherwise.
    pub read_only: bool,
}

impl ConnectionMeta {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            ..Self::default()
        }
    }

    pub fn env_tag(&self) -> Option<EnvTag> {
        let label = self.environment.trim();
        (!label.is_empty()).then(|| EnvTag {
            label: label.to_string(),
            color: self.color.unwrap_or_else(|| preset_color(label)),
        })
    }

    pub fn set_env(&mut self, tag: Option<EnvTag>) {
        match tag {
            Some(tag) => {
                self.environment = tag.label;
                self.color = Some(tag.color);
            }
            None => {
                self.environment.clear();
                self.color = None;
            }
        }
    }

    fn is_default(&self) -> bool {
        self.environment.trim().is_empty() && !self.read_only
    }
}

/// The part of `gui_state.toml` that belongs to the Connections page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherState {
    /// Identity of the item selected when the page was last used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_selected: Option<String>,
    pub entries: Vec<ConnectionMeta>,
}

impl LauncherState {
    pub fn meta(&self, key: &str) -> Option<&ConnectionMeta> {
        self.entries.iter().find(|meta| meta.key == key)
    }

    /// Store `meta`, or forget the key when it holds nothing worth keeping.
    pub fn set_meta(&mut self, meta: ConnectionMeta) {
        self.entries.retain(|existing| existing.key != meta.key);
        if !meta.is_default() {
            self.entries.push(meta);
        }
    }

    pub fn forget(&mut self, key: &str) {
        self.entries.retain(|meta| meta.key != key);
    }

    /// A renamed session keeps its metadata.
    pub fn rename(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        if let Some(mut meta) = self.entries.iter().find(|meta| meta.key == from).cloned() {
            self.entries.retain(|existing| existing.key != from);
            meta.key = to.to_string();
            self.set_meta(meta);
        }
    }

    /// Drop what a hand-edited file could break.
    pub fn sanitized(mut self) -> Self {
        self.last_selected = self.last_selected.filter(|key| !key.trim().is_empty());
        self.entries.retain(|meta| !meta.key.trim().is_empty());
        for meta in &mut self.entries {
            meta.environment = meta
                .environment
                .trim()
                .chars()
                .take(MAX_ENVIRONMENT_LEN)
                .collect();
            if meta.environment.is_empty() {
                meta.color = None;
            }
        }
        self.entries.retain(|meta| !meta.is_default());
        self.entries.truncate(MAX_META_ENTRIES);
        self
    }
}

// ── Identities ─────────────────────────────────────────────────────────

/// Stable identity of a saved session.
pub fn session_key(name: &str) -> String {
    format!("session:{name}")
}

/// Stable identity of a recent connection (its sanitized URL).
pub fn recent_key(url: &str) -> String {
    format!("recent:{}", url.split(" # ").next().unwrap_or(url).trim())
}

/// Stable identity of a Docker container, by name (containers are rebuilt
/// with new ids; their names are what users and `docker://` URLs use).
pub fn docker_key(container: &str) -> String {
    format!("docker:{container}")
}

/// Prefix of the keys under which the form's attempts are filed.
pub const FORM_KEY_PREFIX: &str = "form:";

/// The key of the attempts of one form state: the subject (a new connection,
/// or the edit of one item: its `generation`) and the connection fields as
/// they are now (`revision`). Either changing leaves earlier results behind,
/// so a result never shows on a form, or for fields, it does not belong to.
pub fn form_key(generation: u64, revision: u64) -> String {
    format!("{FORM_KEY_PREFIX}{generation}.{revision}")
}

// ── What a connection opens ────────────────────────────────────────────

/// The effective target of a connection: what it actually opens, whatever it
/// is called. An open connection stands for a list entry only when their
/// identities are equal: the same server or file, database, user and
/// non-secret options, and for a container the same Docker daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetIdentity {
    scheme: String,
    host: String,
    port: Option<u16>,
    user: String,
    database: String,
    path: String,
    options: BTreeMap<String, String>,
    /// The Docker daemon a container connection is pinned to.
    daemon: Option<String>,
}

fn kept_options<'a>(
    options: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> BTreeMap<String, String> {
    options
        .into_iter()
        .filter(|(key, _)| {
            !urls::looks_secret(key)
                && !key.starts_with("vault_")
                && key.as_str() != PINNED_DAEMON_OPTION
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

impl TargetIdentity {
    fn empty(scheme: &str) -> Self {
        Self {
            scheme: scheme.to_string(),
            host: String::new(),
            port: None,
            user: String::new(),
            database: String::new(),
            path: String::new(),
            options: BTreeMap::new(),
            daemon: None,
        }
    }

    /// What `target` opens. `daemon` is the pin of a container connection.
    pub fn from_target(target: &Target, daemon: Option<&str>) -> Self {
        match target {
            Target::Database(spec) => {
                let mut identity = Self::empty(spec.engine.url_scheme());
                if spec.engine.is_file_based() {
                    identity.path = spec.path.trim().to_string();
                } else {
                    identity.host = spec.host.trim().to_lowercase();
                    identity.port = spec.port.or_else(|| spec.engine.default_port());
                    identity.user = spec.user.trim().to_string();
                    identity.database = spec.database.trim().to_string();
                }
                identity.options =
                    kept_options(spec.options.iter().map(|(key, value)| (key, value)));
                identity
            }
            Target::Docker(spec) => {
                let mut identity = Self::empty("docker");
                identity.host = spec.container.trim().to_string();
                identity.user = spec.user.trim().to_string();
                identity.database = spec.database.trim().to_string();
                identity.options =
                    kept_options(spec.options.iter().map(|(key, value)| (key, value)));
                if spec.use_tls {
                    identity.options.insert("ssl".into(), "true".into());
                }
                identity.daemon = daemon.map(str::to_string);
                identity
            }
            Target::Vault(spec) => {
                let mut identity = Self::empty("vault");
                identity.host = spec.mount.trim().to_string();
                identity.user = spec.role.trim().to_string();
                identity.database = spec.database.trim().to_string();
                identity
            }
        }
    }

    /// What a connection URL (a recent entry, a live connection's sanitized
    /// URL) opens. `None` for something that is no connection URL.
    pub fn from_url(url: &str) -> Option<Self> {
        let url = url.split(" # ").next().unwrap_or(url);
        urls::parse_target(url).map(|target| Self::from_target(&target, None))
    }

    /// What a saved session opens when it is connected to.
    pub fn from_session(session: &SavedSession) -> Self {
        let option = |key: &str| session.options.get(key).filter(|value| !value.is_empty());
        if let Some(container) = session.host.strip_prefix("DOCKER:") {
            let mut identity = Self::empty("docker");
            identity.host = container.to_string();
            identity.user = session.user.clone();
            identity.database = session.dbname.clone();
            identity.options = kept_options(&session.options);
            return identity;
        }
        if let (Some(mount), Some(database), Some(role)) = (
            option("vault_mount"),
            option("vault_database"),
            session.options.get("vault_role"),
        ) {
            let mut identity = Self::empty("vault");
            identity.host = mount.clone();
            identity.user = role.clone();
            identity.database = database.clone();
            return identity;
        }
        let engine = &session.database_type;
        let mut identity = Self::empty(engine.url_scheme());
        if engine.is_file_based() {
            identity.path = session.file_path.clone().unwrap_or_default();
        } else {
            identity.host = session.host.trim().to_lowercase();
            identity.port = Some(session.port)
                .filter(|port| *port != 0)
                .or_else(|| engine.default_port());
            identity.user = session.user.clone();
            identity.database = session.dbname.clone();
        }
        identity.options = kept_options(&session.options);
        identity
    }

    /// A container listed by Docker: no explicit user, database or options
    /// (its environment supplies them), on the daemon that listed it.
    pub fn container(name: &str, daemon: Option<&str>) -> Self {
        let mut identity = Self::empty("docker");
        identity.host = name.to_string();
        identity.daemon = daemon.map(str::to_string);
        identity
    }

    /// A container identity whose daemon is not known yet gets `pin`. Docker
    /// sessions and recents carry no daemon of their own: they open on the
    /// context that is current when they are used, which the caller resolves
    /// ([`current_docker_pin`]) or, when it cannot, leaves unknown (such an
    /// identity then matches no connection).
    pub fn with_docker_daemon(mut self, pin: Option<&str>) -> Self {
        if self.is_docker() && self.daemon.is_none() {
            self.daemon = pin.map(str::to_string);
        }
        self
    }

    /// The daemon a container connection really went through is the truth
    /// about it: it replaces whatever the request assumed (a retry after the
    /// Docker context changed asked for one daemon and got another). Without
    /// a pin (not a container connection) nothing changes.
    pub fn with_actual_docker_daemon(mut self, actual: Option<&str>) -> Self {
        if let Some(actual) = actual.filter(|_| self.is_docker()) {
            self.daemon = Some(actual.to_string());
        }
        self
    }

    pub fn is_docker(&self) -> bool {
        self.scheme == "docker"
    }
}

/// The daemon the current Docker context selects, as a pin comparable with
/// [`daemon_pin`]. Reads the Docker configuration files (no daemon, no
/// probing); `None` when the endpoint cannot be resolved.
pub fn current_docker_pin() -> Option<String> {
    dbcrust_docker::docker::DockerEndpoint::from_environment()
        .ok()
        .map(|endpoint| endpoint.pinned_value())
}

/// The Docker daemon a request is pinned to (see
/// [`DockerDaemon::connect_request`]); `None` for a plain request, which
/// uses whatever context is current.
pub fn daemon_pin_of(request: &ConnectRequest) -> Option<String> {
    match &request.target {
        ConnectTarget::Info(info) => info.options.get(PINNED_DAEMON_OPTION).cloned(),
        _ => None,
    }
}

/// The pin of `daemon`, comparable with [`daemon_pin_of`] of a request.
pub fn daemon_pin(daemon: &DockerDaemon) -> Option<String> {
    daemon_pin_of(&daemon.connect_request("pin"))
}

// ── The list ───────────────────────────────────────────────────────────

/// The list's filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Saved,
    Recent,
    Docker,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Filter::All, Filter::Saved, Filter::Recent, Filter::Docker];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Saved => "Saved",
            Filter::Recent => "Recent",
            Filter::Docker => "Docker",
        }
    }

    fn includes(self, section: Section) -> bool {
        matches!(
            (self, section),
            (Filter::All, _)
                | (Filter::Saved, Section::Saved)
                | (Filter::Recent, Section::Recent)
                | (Filter::Docker, Section::Docker)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Saved,
    Recent,
    Docker,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Saved => "Saved",
            Section::Recent => "Recent",
            Section::Docker => "Docker",
        }
    }
}

/// Why Docker could not be listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerProblemKind {
    /// No daemon answers at the selected endpoint.
    Unavailable,
    /// The daemon is there but this user may not talk to it.
    PermissionDenied,
    /// The selected context uses a transport DBCrust does not speak.
    Unsupported,
    Other,
}

impl DockerProblemKind {
    pub fn title(self) -> &'static str {
        match self {
            DockerProblemKind::Unavailable => "Docker isn't running",
            DockerProblemKind::PermissionDenied => "Docker refused access",
            DockerProblemKind::Unsupported => "This Docker context isn't supported",
            DockerProblemKind::Other => "Couldn't list containers",
        }
    }

    /// What to do about it, naming the context when it is known.
    pub fn advice(self, context: Option<&str>) -> String {
        let named = context.map_or_else(String::new, |context| format!(" “{context}”"));
        match self {
            DockerProblemKind::Unavailable => format!(
                "Start Docker Desktop, OrbStack or Colima, then retry. DBCrust asked the Docker \
                 context{named}."
            ),
            DockerProblemKind::PermissionDenied => format!(
                "Your user can't use the Docker socket of the context{named}. Add it to the \
                 docker group or fix the socket's permissions, then retry."
            ),
            DockerProblemKind::Unsupported => format!(
                "The context{named} reaches Docker over a transport DBCrust can't use. Select \
                 another context (docker context use …), then retry."
            ),
            DockerProblemKind::Other => {
                format!("Docker answered with an error for the context{named}.")
            }
        }
    }
}

/// Sort a Docker error message into what the page can explain.
pub fn classify_docker_problem(message: &str) -> DockerProblemKind {
    let lower = message.to_lowercase();
    if lower.contains("permission denied") || lower.contains("access denied") {
        DockerProblemKind::PermissionDenied
    } else if lower.contains("unsupported") || lower.contains("not supported") {
        DockerProblemKind::Unsupported
    } else if lower.contains("no such file")
        || lower.contains("connection refused")
        || lower.contains("cannot connect")
        || lower.contains("can't connect")
        || lower.contains("not running")
        || lower.contains("socket")
        || lower.contains("daemon")
        || lower.contains("timed out")
    {
        DockerProblemKind::Unavailable
    } else {
        DockerProblemKind::Other
    }
}

/// What the Docker section knows.
#[derive(Debug, Clone, PartialEq)]
pub enum DockerListState {
    Loading,
    Ready {
        containers: Vec<DockerContainer>,
        /// The Docker context (daemon) the list came from.
        context: Option<String>,
    },
    Problem {
        kind: DockerProblemKind,
        message: String,
        context: Option<String>,
    },
}

/// What a row stands for, as an index into the lists given to
/// [`build_listing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Session(usize),
    Recent(usize),
    Docker(usize),
}

/// One selectable item of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: String,
    pub source: Source,
    pub title: String,
    pub subtitle: String,
    /// Right-aligned quiet text (age of a recent connection).
    pub trailing: String,
    pub db_type: Option<DatabaseType>,
}

/// Placeholder in the list while Docker loads, or why it has nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum DockerNote {
    /// `Docker` section is loading: draw skeleton rows.
    Loading,
    /// No database containers in the context.
    Empty { context: Option<String> },
    Problem {
        kind: DockerProblemKind,
        context: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ListRow {
    Header {
        section: Section,
        count: usize,
    },
    /// An index into [`Listing::entries`].
    Entry(usize),
    /// A compact explanation (inside the All filter) of a Docker section
    /// without items.
    Note(DockerNote),
}

/// Everything the left pane renders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub rows: Vec<ListRow>,
    /// Set when the Docker filter is active and Docker has no items: the
    /// pane draws a full-height explanation instead of rows.
    pub docker_block: Option<DockerNote>,
    /// Matches per section, ignoring the filter chip (for the chip counts).
    pub saved_count: usize,
    pub recent_count: usize,
    pub docker_count: usize,
}

impl Listing {
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    pub fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.key == key)
    }

    /// The row index of an entry key.
    pub fn row_of(&self, key: &str) -> Option<usize> {
        let ix = self.position(key)?;
        self.rows
            .iter()
            .position(|row| matches!(row, ListRow::Entry(entry) if *entry == ix))
    }

    /// The key after moving `delta` entries from `current` (clamped; the
    /// first entry when nothing is selected).
    pub fn step(&self, current: Option<&str>, delta: isize) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let at = current.and_then(|key| self.position(key));
        let next = match at {
            Some(ix) => (ix as isize + delta).clamp(0, self.entries.len() as isize - 1) as usize,
            None if delta < 0 => self.entries.len() - 1,
            None => 0,
        };
        Some(self.entries[next].key.clone())
    }
}

/// What the list is built from.
pub struct Sources<'a> {
    pub sessions: &'a [SessionEntry],
    pub recents: &'a [RecentEntry],
    pub docker: &'a DockerListState,
}

/// Does every word of `query` occur in `haystack`? (Both lowercase.)
pub fn matches_query(haystack: &str, query: &str) -> bool {
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

fn session_entry(index: usize, session: &SessionEntry) -> (Entry, String) {
    let entry = Entry {
        key: session_key(&session.name),
        source: Source::Session(index),
        title: session.name.clone(),
        subtitle: session.target.clone(),
        trailing: String::new(),
        db_type: Some(session.db_type.clone()),
    };
    let haystack = format!(
        "{} {} {} {} {}",
        session.name,
        session.target,
        session.user,
        session.database,
        session.db_type.display_name()
    )
    .to_lowercase();
    (entry, haystack)
}

/// Name a recent connection the way a user would: the database, or the file.
pub fn recent_title(recent: &RecentEntry) -> String {
    let url = recent.url.split(" # ").next().unwrap_or(&recent.url);
    match urls::parse_target(url) {
        Some(target) => urls::suggest_session_name(&target),
        None => recent.display_name.clone(),
    }
}

/// Where a recent connection points, without repeating its title: the host
/// of a server (`user@host:port`), the folder of a file, the kind of source.
pub fn recent_endpoint(recent: &RecentEntry) -> String {
    let url = recent.url.split(" # ").next().unwrap_or(&recent.url);
    match urls::parse_target(url) {
        Some(Target::Database(spec)) if spec.engine.is_file_based() => {
            let path = spec.path.trim_end_matches('/');
            match path.rsplit_once('/') {
                Some(("", _)) => "/".to_string(),
                Some((folder, _)) => folder.to_string(),
                None => path.to_string(),
            }
        }
        Some(Target::Database(spec)) => {
            let mut text = String::new();
            if !spec.user.is_empty() {
                text.push_str(&spec.user);
                text.push('@');
            }
            text.push_str(&spec.host);
            if let Some(port) = spec.port {
                text.push(':');
                text.push_str(&port.to_string());
            }
            text
        }
        Some(Target::Docker(spec)) if !spec.user.is_empty() => {
            format!("Docker container · {}", spec.user)
        }
        Some(Target::Docker(_)) => "Docker container".to_string(),
        Some(Target::Vault(spec)) => format!("Vault · {}", spec.mount),
        None => recent.display_name.clone(),
    }
}

fn recent_entry(index: usize, recent: &RecentEntry, now_secs: i64) -> (Entry, String) {
    let age = urls::relative_age((now_secs - recent.timestamp.timestamp()).max(0));
    let entry = Entry {
        key: recent_key(&recent.url),
        source: Source::Recent(index),
        title: recent_title(recent),
        subtitle: recent_endpoint(recent),
        trailing: age,
        db_type: Some(recent.db_type.clone()),
    };
    let haystack = format!(
        "{} {} {} {}",
        entry.title,
        recent.display_name,
        recent.url,
        recent.db_type.display_name()
    )
    .to_lowercase();
    (entry, haystack)
}

/// How a Docker container is reachable, without probing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publish {
    /// A host port forwards to the database port.
    Published { host: String, port: u16 },
    /// Running, but the database port is not published: DBCrust picks the
    /// route (container network, OrbStack domain, or Docker forwarding).
    NotPublished,
    /// Stopped containers publish nothing.
    Stopped,
}

pub fn publish_state(container: &DockerContainer) -> Publish {
    if !container.running {
        return Publish::Stopped;
    }
    match container.reach_hint() {
        ReachHint::Published { port } => Publish::Published {
            host: "127.0.0.1".to_string(),
            port,
        },
        ReachHint::NotPublished => Publish::NotPublished,
    }
}

impl Publish {
    /// The one-line status of the details pane.
    pub fn status_line(&self) -> String {
        match self {
            Publish::Published { host, port } => format!("Port published {host}:{port}"),
            Publish::NotPublished => "Port not published · Automatic".to_string(),
            Publish::Stopped => "Stopped · nothing is published".to_string(),
        }
    }

    /// The short form of a list row.
    pub fn short(&self) -> String {
        match self {
            Publish::Published { host, port } => format!("{host}:{port}"),
            Publish::NotPublished => "not published".to_string(),
            Publish::Stopped => "stopped".to_string(),
        }
    }
}

fn docker_entry(index: usize, container: &DockerContainer) -> (Entry, String) {
    let entry = Entry {
        key: docker_key(&container.name),
        source: Source::Docker(index),
        title: container.name.clone(),
        subtitle: match publish_state(container) {
            Publish::Stopped => container.image.clone(),
            publish => format!("{} · {}", container.image, publish.short()),
        },
        trailing: if container.running {
            String::new()
        } else {
            "Stopped".to_string()
        },
        db_type: container.db_type.clone(),
    };
    let haystack = format!(
        "{} {} {} docker",
        container.name,
        container.image,
        container
            .db_type
            .as_ref()
            .map_or("", |db| db.display_name())
    )
    .to_lowercase();
    (entry, haystack)
}

/// Build the list for `filter` and `query`. Recent connections that a saved
/// session or a listed container already stands for are left out.
pub fn build_listing(filter: Filter, query: &str, sources: &Sources<'_>, now_secs: i64) -> Listing {
    let query = query.trim();
    let mut listing = Listing::default();

    let mut saved = Vec::new();
    for (ix, session) in sources.sessions.iter().enumerate() {
        let (entry, haystack) = session_entry(ix, session);
        if matches_query(&haystack, query) {
            saved.push(entry);
        }
    }

    let containers: &[DockerContainer] = match sources.docker {
        DockerListState::Ready { containers, .. } => containers,
        _ => &[],
    };
    let mut recent = Vec::new();
    for (ix, recent_entry_source) in sources.recents.iter().enumerate() {
        let duplicates_session = sources.sessions.iter().any(|session| {
            session.db_type == recent_entry_source.db_type
                && session.target == recent_entry_source.display_name
        });
        let duplicates_container = matches!(
            urls::parse_target(&recent_entry_source.url),
            Some(Target::Docker(spec)) if containers.iter().any(|c| c.name == spec.container)
        );
        if duplicates_session || duplicates_container {
            continue;
        }
        let (entry, haystack) = recent_entry(ix, recent_entry_source, now_secs);
        if matches_query(&haystack, query) {
            recent.push(entry);
        }
    }

    let mut docker = Vec::new();
    for (ix, container) in containers.iter().enumerate() {
        let (entry, haystack) = docker_entry(ix, container);
        if matches_query(&haystack, query) {
            docker.push(entry);
        }
    }

    listing.saved_count = saved.len();
    listing.recent_count = recent.len();
    listing.docker_count = docker.len();

    let show_headers = filter == Filter::All;
    let push_section = |listing: &mut Listing, section: Section, entries: Vec<Entry>| {
        if entries.is_empty() || !filter.includes(section) {
            return;
        }
        if show_headers {
            listing.rows.push(ListRow::Header {
                section,
                count: entries.len(),
            });
        }
        for entry in entries {
            listing.entries.push(entry);
            listing.rows.push(ListRow::Entry(listing.entries.len() - 1));
        }
    };
    push_section(&mut listing, Section::Saved, saved);
    push_section(&mut listing, Section::Recent, recent);
    push_section(&mut listing, Section::Docker, docker);

    // Docker without items: say why, compactly in All, in full in Docker
    let note = match sources.docker {
        DockerListState::Loading if query.is_empty() => Some(DockerNote::Loading),
        DockerListState::Ready {
            containers,
            context,
        } if containers.is_empty() && query.is_empty() => Some(DockerNote::Empty {
            context: context.clone(),
        }),
        DockerListState::Problem { kind, context, .. } => Some(DockerNote::Problem {
            kind: *kind,
            context: context.clone(),
        }),
        _ => None,
    };
    if let Some(note) = note {
        match filter {
            Filter::Docker => listing.docker_block = Some(note),
            Filter::All => {
                listing.rows.push(ListRow::Header {
                    section: Section::Docker,
                    count: 0,
                });
                listing.rows.push(ListRow::Note(note));
            }
            _ => {}
        }
    }
    listing
}

/// Which entry stays selected after the list changed: the current one while
/// it is listed, else the one remembered from the last session once it
/// shows up, else the first.
pub fn resolve_selection(
    listing: &Listing,
    current: Option<&str>,
    restore: Option<&str>,
) -> Option<String> {
    [current, restore]
        .into_iter()
        .flatten()
        .find(|key| listing.entry(key).is_some())
        .map(str::to_string)
        .or_else(|| listing.entries.first().map(|entry| entry.key.clone()))
}

/// No saved connection, no recent one, no container: the page opens the
/// New connection form.
pub fn is_first_run(sessions: &[SessionEntry], recents: &[RecentEntry]) -> bool {
    sessions.is_empty() && recents.is_empty()
}

// ── Connecting ─────────────────────────────────────────────────────────

pub use crate::worker::ConnectPhase;

/// What the page calls a phase while an attempt runs.
pub fn phase_label(phase: ConnectPhase) -> &'static str {
    match phase {
        ConnectPhase::Starting => "Preparing",
        other => other.label(),
    }
}

/// The phases an attempt went through, with when each began (as an offset
/// from the start of the attempt).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseLog {
    steps: Vec<(ConnectPhase, Duration)>,
}

/// One line of the progress list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseStep {
    pub phase: ConnectPhase,
    /// How long it ran (so far, for the current phase).
    pub took: Duration,
    pub done: bool,
}

impl PhaseLog {
    /// Note that `phase` began `at` into the attempt. Repeating the current
    /// phase changes nothing.
    pub fn record(&mut self, phase: ConnectPhase, at: Duration) {
        if self.current() != Some(phase) {
            self.steps.push((phase, at));
        }
    }

    pub fn current(&self) -> Option<ConnectPhase> {
        self.steps.last().map(|(phase, _)| *phase)
    }

    /// The phases reached so far (never the ones still ahead), `now` being
    /// the time since the attempt began.
    pub fn steps(&self, now: Duration) -> Vec<PhaseStep> {
        self.steps
            .iter()
            .enumerate()
            .map(|(ix, (phase, began))| {
                let ended = self.steps.get(ix + 1).map(|(_, next)| *next);
                PhaseStep {
                    phase: *phase,
                    took: ended.unwrap_or(now).saturating_sub(*began),
                    done: ended.is_some(),
                }
            })
            .collect()
    }
}

/// `0.8 s`, `12 s`, `1 m 05 s`.
pub fn elapsed_label(elapsed: Duration) -> String {
    let millis = elapsed.as_millis();
    if millis < 10_000 {
        format!("{:.1} s", millis as f64 / 1000.0)
    } else if millis < 60_000 {
        format!("{} s", elapsed.as_secs())
    } else {
        let seconds = elapsed.as_secs();
        format!("{} m {:02} s", seconds / 60, seconds % 60)
    }
}

/// The `ports:` snippet that publishes a database port on localhost only,
/// for a compose file. The host port is left to Docker.
pub fn compose_ports_snippet(container_port: u16) -> String {
    format!("ports: [\"127.0.0.1::{container_port}\"]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use rstest::rstest;

    fn session(name: &str, target: &str, db_type: DatabaseType) -> SessionEntry {
        SessionEntry {
            name: name.into(),
            target: target.into(),
            db_type,
            user: "u".into(),
            database: "db".into(),
        }
    }

    fn recent(url: &str, display: &str, db_type: DatabaseType, minutes_ago: i64) -> RecentEntry {
        let timestamp: DateTime<Utc> = Utc::now() - chrono::Duration::minutes(minutes_ago);
        RecentEntry {
            display_name: display.into(),
            url: url.into(),
            db_type: db_type.clone(),
            timestamp,
            connection: dbcrust_app::config::RecentConnection {
                connection_url: url.into(),
                display_name: display.into(),
                timestamp,
                database_type: db_type,
                success: true,
                options: Default::default(),
            },
        }
    }

    fn container(name: &str, running: bool, host_port: Option<u16>) -> DockerContainer {
        DockerContainer {
            id: format!("{name}-id"),
            name: name.into(),
            image: "postgres:17".into(),
            status: if running {
                "Up".into()
            } else {
                "Exited".into()
            },
            db_type: Some(DatabaseType::PostgreSQL),
            host_port,
            container_port: Some(5432),
            running,
        }
    }

    fn docker_ready(containers: Vec<DockerContainer>) -> DockerListState {
        DockerListState::Ready {
            containers,
            context: Some("orbstack".into()),
        }
    }

    fn now() -> i64 {
        Utc::now().timestamp()
    }

    #[test]
    fn environment_tags_need_text_and_default_their_colour_from_the_preset() {
        let mut meta = ConnectionMeta::new("session:prod");
        assert_eq!(meta.env_tag(), None);
        meta.environment = "Production".into();
        assert_eq!(
            meta.env_tag(),
            Some(EnvTag {
                label: "Production".into(),
                color: EnvColor::Red
            })
        );
        meta.color = Some(EnvColor::Purple);
        assert_eq!(meta.env_tag().map(|tag| tag.color), Some(EnvColor::Purple));
        meta.set_env(None);
        assert_eq!(meta.env_tag(), None);
        assert_eq!(meta.color, None);
        assert_eq!(preset_color("custom"), EnvColor::Gray);
        assert_eq!(preset_color(" staging "), EnvColor::Amber);
    }

    #[test]
    fn launcher_state_keeps_meaningful_metadata_only() {
        let mut state = LauncherState::default();
        let mut meta = ConnectionMeta::new("session:a");
        meta.environment = "Staging".into();
        state.set_meta(meta.clone());
        assert_eq!(state.meta("session:a"), Some(&meta));
        // Overwriting replaces; an empty record forgets
        meta.read_only = true;
        state.set_meta(meta.clone());
        assert_eq!(state.entries.len(), 1);
        state.set_meta(ConnectionMeta::new("session:a"));
        assert!(state.entries.is_empty());

        // A renamed session keeps its tag
        state.set_meta(meta);
        state.rename("session:a", "session:b");
        assert!(state.meta("session:a").is_none());
        assert_eq!(
            state
                .meta("session:b")
                .map(|meta| meta.environment.as_str()),
            Some("Staging")
        );
    }

    #[test]
    fn launcher_state_is_sanitized() {
        let state = LauncherState {
            last_selected: Some("  ".into()),
            entries: vec![
                ConnectionMeta::new(""),
                ConnectionMeta {
                    key: "session:x".into(),
                    environment: "  A very long environment label indeed ".into(),
                    color: Some(EnvColor::Blue),
                    read_only: false,
                },
                ConnectionMeta {
                    key: "session:y".into(),
                    environment: "".into(),
                    color: Some(EnvColor::Blue),
                    read_only: true,
                },
                ConnectionMeta::new("session:nothing"),
            ],
        }
        .sanitized();
        assert_eq!(state.last_selected, None);
        assert_eq!(state.entries.len(), 2);
        assert_eq!(state.entries[0].environment.chars().count(), 24);
        assert_eq!(state.entries[1].color, None, "no label, no colour");
    }

    #[test]
    fn form_attempts_are_filed_per_generation() {
        assert_ne!(form_key(1, 0), form_key(2, 0));
        assert_ne!(
            form_key(1, 0),
            form_key(1, 1),
            "edited fields are another state"
        );
        assert!(form_key(7, 3).starts_with(FORM_KEY_PREFIX));
    }

    #[test]
    fn identities_ignore_docker_suffixes_of_recents() {
        assert_eq!(session_key("prod"), "session:prod");
        assert_eq!(
            recent_key("docker://pg # Docker: pg"),
            recent_key("docker://pg")
        );
        assert_eq!(docker_key("pg"), "docker:pg");
    }

    #[test]
    fn a_session_that_a_recent_repeats_is_listed_once() {
        let sessions = [session(
            "prod",
            "analyst@warehouse:5432/analytics",
            DatabaseType::PostgreSQL,
        )];
        let recents = [
            recent(
                "postgres://analyst@warehouse:5432/analytics",
                "analyst@warehouse:5432/analytics",
                DatabaseType::PostgreSQL,
                5,
            ),
            recent("sqlite:///tmp/shop.db", "shop.db", DatabaseType::SQLite, 60),
        ];
        let docker = DockerListState::Loading;
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &sessions,
                recents: &recents,
                docker: &docker,
            },
            now(),
        );
        let titles: Vec<_> = listing.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["prod", "shop.db"]);
        assert_eq!(listing.recent_count, 1);
    }

    #[test]
    fn all_shows_headers_and_a_docker_note_while_loading() {
        let sessions = [session("a", "u@h:5432/d", DatabaseType::PostgreSQL)];
        let docker = DockerListState::Loading;
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &sessions,
                recents: &[],
                docker: &docker,
            },
            now(),
        );
        assert!(matches!(
            listing.rows.as_slice(),
            [
                ListRow::Header {
                    section: Section::Saved,
                    count: 1
                },
                ListRow::Entry(0),
                ListRow::Header {
                    section: Section::Docker,
                    count: 0
                },
                ListRow::Note(DockerNote::Loading)
            ]
        ));
        assert_eq!(listing.docker_block, None);
    }

    #[test]
    fn the_docker_filter_explains_an_empty_or_failing_daemon_in_full() {
        let empty = docker_ready(vec![]);
        let listing = build_listing(
            Filter::Docker,
            "",
            &Sources {
                sessions: &[],
                recents: &[],
                docker: &empty,
            },
            now(),
        );
        assert!(listing.rows.is_empty());
        assert_eq!(
            listing.docker_block,
            Some(DockerNote::Empty {
                context: Some("orbstack".into())
            })
        );

        let broken = DockerListState::Problem {
            kind: DockerProblemKind::PermissionDenied,
            message: "permission denied".into(),
            context: None,
        };
        let listing = build_listing(
            Filter::Docker,
            "",
            &Sources {
                sessions: &[],
                recents: &[],
                docker: &broken,
            },
            now(),
        );
        assert!(matches!(
            listing.docker_block,
            Some(DockerNote::Problem {
                kind: DockerProblemKind::PermissionDenied,
                ..
            })
        ));
    }

    #[test]
    fn searching_hides_notes_and_matches_every_word() {
        let sessions = [
            session(
                "prod-analytics",
                "a@warehouse:5432/x",
                DatabaseType::PostgreSQL,
            ),
            session(
                "staging-api",
                "api@staging:3306/orders",
                DatabaseType::MySQL,
            ),
        ];
        let docker = docker_ready(vec![]);
        let sources = Sources {
            sessions: &sessions,
            recents: &[],
            docker: &docker,
        };
        let listing = build_listing(Filter::All, "mysql stag", &sources, now());
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].title, "staging-api");
        assert!(
            !listing
                .rows
                .iter()
                .any(|row| matches!(row, ListRow::Note(_))),
            "a search has no empty-Docker note"
        );
        let none = build_listing(Filter::All, "zzz", &sources, now());
        assert!(none.entries.is_empty());
    }

    #[test]
    fn a_recent_docker_connection_is_hidden_while_its_container_is_listed() {
        let recents = [recent(
            "docker://shop-postgres",
            "shop-postgres (Docker: shop-postgres)",
            DatabaseType::PostgreSQL,
            3,
        )];
        let ready = docker_ready(vec![container("shop-postgres", true, Some(5432))]);
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &[],
                recents: &recents,
                docker: &ready,
            },
            now(),
        );
        assert_eq!(listing.recent_count, 0);
        assert_eq!(listing.docker_count, 1);
        // Docker unavailable: the recent is the only way back in
        let down = DockerListState::Problem {
            kind: DockerProblemKind::Unavailable,
            message: "no daemon".into(),
            context: None,
        };
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &[],
                recents: &recents,
                docker: &down,
            },
            now(),
        );
        assert_eq!(listing.recent_count, 1);
    }

    #[test]
    fn stepping_clamps_and_starts_from_the_edges() {
        let sessions = [
            session("a", "t", DatabaseType::PostgreSQL),
            session("b", "t", DatabaseType::PostgreSQL),
            session("c", "t", DatabaseType::PostgreSQL),
        ];
        let docker = docker_ready(vec![container("pg", true, None)]);
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &sessions,
                recents: &[],
                docker: &docker,
            },
            now(),
        );
        assert_eq!(listing.step(None, 1).as_deref(), Some("session:a"));
        assert_eq!(listing.step(None, -1).as_deref(), Some("docker:pg"));
        assert_eq!(
            listing.step(Some("session:c"), 1).as_deref(),
            Some("docker:pg")
        );
        assert_eq!(
            listing.step(Some("docker:pg"), 1).as_deref(),
            Some("docker:pg")
        );
        assert_eq!(
            listing.step(Some("session:a"), -1).as_deref(),
            Some("session:a")
        );
        assert_eq!(listing.row_of("session:b"), Some(2));
        assert_eq!(
            Listing::default().step(None, 1),
            None,
            "nothing to select in an empty list"
        );
    }

    #[rstest]
    #[case(true, Some(5433), "Port published 127.0.0.1:5433", "127.0.0.1:5433")]
    #[case(true, None, "Port not published · Automatic", "not published")]
    #[case(false, None, "Stopped · nothing is published", "stopped")]
    #[case(false, Some(5433), "Stopped · nothing is published", "stopped")]
    fn publishing_is_described_without_probing(
        #[case] running: bool,
        #[case] host_port: Option<u16>,
        #[case] line: &str,
        #[case] short: &str,
    ) {
        let state = publish_state(&container("pg", running, host_port));
        assert_eq!(state.status_line(), line);
        assert_eq!(state.short(), short);
    }

    #[rstest]
    #[case(
        "dial unix /var/run/docker.sock: permission denied",
        DockerProblemKind::PermissionDenied
    )]
    #[case(
        "Cannot connect to the Docker daemon at unix:///x. Is it running?",
        DockerProblemKind::Unavailable
    )]
    #[case(
        "No such file or directory (os error 2)",
        DockerProblemKind::Unavailable
    )]
    #[case(
        "unsupported Docker host scheme ssh://",
        DockerProblemKind::Unsupported
    )]
    #[case("internal server error", DockerProblemKind::Other)]
    fn docker_problems_are_classified(#[case] message: &str, #[case] kind: DockerProblemKind) {
        assert_eq!(classify_docker_problem(message), kind);
    }

    #[test]
    fn docker_advice_names_the_context() {
        let text = DockerProblemKind::Unavailable.advice(Some("orbstack"));
        assert!(text.contains("“orbstack”"), "{text}");
        let text = DockerProblemKind::Unavailable.advice(None);
        assert!(!text.contains("“"), "{text}");
    }

    #[rstest]
    #[case(0, "0.0 s")]
    #[case(850, "0.8 s")]
    #[case(9_990, "10.0 s")]
    #[case(12_400, "12 s")]
    #[case(65_000, "1 m 05 s")]
    fn elapsed_labels(#[case] millis: u64, #[case] expected: &str) {
        assert_eq!(elapsed_label(Duration::from_millis(millis)), expected);
    }

    #[test]
    fn the_phase_log_lists_only_the_phases_reached() {
        let mut log = PhaseLog::default();
        assert_eq!(log.current(), None);
        assert!(log.steps(Duration::from_secs(1)).is_empty());
        log.record(ConnectPhase::ResolvingDocker, Duration::ZERO);
        log.record(ConnectPhase::ResolvingDocker, Duration::from_millis(50));
        log.record(
            ConnectPhase::CheckingDirectAccess,
            Duration::from_millis(300),
        );
        assert_eq!(log.current(), Some(ConnectPhase::CheckingDirectAccess));
        let steps = log.steps(Duration::from_millis(1_300));
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0],
            PhaseStep {
                phase: ConnectPhase::ResolvingDocker,
                took: Duration::from_millis(300),
                done: true
            }
        );
        assert_eq!(steps[1].took, Duration::from_secs(1));
        assert!(!steps[1].done);
    }

    #[test]
    fn compose_snippet_publishes_on_loopback_only() {
        assert_eq!(compose_ports_snippet(5432), "ports: [\"127.0.0.1::5432\"]");
    }

    #[test]
    fn selection_survives_discovery_and_falls_back_in_order() {
        let sessions = [
            session("a", "t", DatabaseType::PostgreSQL),
            session("b", "t", DatabaseType::PostgreSQL),
        ];
        let loading = DockerListState::Loading;
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &sessions,
                recents: &[],
                docker: &loading,
            },
            now(),
        );
        // The current selection wins over the remembered one
        assert_eq!(
            resolve_selection(&listing, Some("session:b"), Some("session:a")).as_deref(),
            Some("session:b")
        );
        // A remembered item that is not there yet (Docker still loading)
        // leaves the first item selected...
        assert_eq!(
            resolve_selection(&listing, None, Some("docker:pg")).as_deref(),
            Some("session:a")
        );
        // ...and takes over once it is listed and nothing else was chosen
        let ready = docker_ready(vec![container("pg", true, None)]);
        let listing = build_listing(
            Filter::All,
            "",
            &Sources {
                sessions: &sessions,
                recents: &[],
                docker: &ready,
            },
            now(),
        );
        assert_eq!(
            resolve_selection(&listing, None, Some("docker:pg")).as_deref(),
            Some("docker:pg")
        );
        assert_eq!(
            resolve_selection(&Listing::default(), Some("x"), None),
            None
        );
    }

    fn saved(host: &str, port: u16, db: &str, options: &[(&str, &str)]) -> SavedSession {
        SavedSession {
            host: host.into(),
            port,
            user: "app".into(),
            dbname: db.into(),
            ssh_tunnel: None,
            database_type: DatabaseType::PostgreSQL,
            file_path: None,
            options: options
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn identities_tell_databases_users_and_options_apart() {
        let base = TargetIdentity::from_session(&saved("DB.example.com", 5432, "shop", &[]));
        // A recent entry of the same server opens the same thing
        assert_eq!(
            Some(base.clone()),
            TargetIdentity::from_url("postgres://app@db.example.com:5432/shop")
        );
        // The default port is the port
        assert_eq!(
            Some(base.clone()),
            TargetIdentity::from_url("postgresql://app@db.example.com/shop")
        );
        // Another database, user or option is another target
        for other in [
            "postgres://app@db.example.com:5432/billing",
            "postgres://root@db.example.com:5432/shop",
            "postgres://app@db.example.com:5432/shop?sslmode=require",
            "postgres://app@db.example.com:6432/shop",
        ] {
            assert_ne!(
                Some(base.clone()),
                TargetIdentity::from_url(other),
                "{other}"
            );
        }
        // A secret never decides identity
        assert_eq!(
            TargetIdentity::from_session(&saved(
                "h",
                5432,
                "d",
                &[("password_command", "pass show h")]
            )),
            TargetIdentity::from_session(&saved("h", 5432, "d", &[]))
        );
        // An edited session is another target than the connection it had
        assert_ne!(
            base,
            TargetIdentity::from_session(&saved("db.example.com", 5432, "billing", &[]))
        );
    }

    #[test]
    fn container_identities_include_the_daemon_and_overrides() {
        let on_orbstack = TargetIdentity::container("pg", Some("context:orbstack"));
        assert_eq!(
            on_orbstack,
            TargetIdentity::container("pg", Some("context:orbstack"))
        );
        assert_ne!(
            on_orbstack,
            TargetIdentity::container("pg", Some("context:desktop"))
        );
        assert_ne!(
            on_orbstack,
            TargetIdentity::container("pg", None),
            "an unpinned one"
        );
        // Typing a user, a database or options makes it another connection
        let with_database = urls::parse_target("docker://pg/shop")
            .map(|target| TargetIdentity::from_target(&target, Some("context:orbstack")));
        assert_ne!(Some(on_orbstack.clone()), with_database);
        let with_tls = urls::parse_target("docker://pg?sslmode=verify-full")
            .map(|target| TargetIdentity::from_target(&target, Some("context:orbstack")));
        assert_ne!(Some(on_orbstack.clone()), with_tls);
        // The same URL on the same daemon is the same
        let plain = urls::parse_target("docker://pg")
            .map(|target| TargetIdentity::from_target(&target, Some("context:orbstack")));
        assert_eq!(Some(on_orbstack), plain);
        // A saved Docker session
        let session = saved("DOCKER:pg", 0, "shop", &[]);
        assert_eq!(
            Some(TargetIdentity::from_session(&session)),
            TargetIdentity::from_url("docker://app@pg/shop")
        );
    }

    #[test]
    fn unpinned_container_identities_take_the_current_daemon_only_when_known() {
        let session = saved("DOCKER:pg", 0, "shop", &[]);
        let on_a = TargetIdentity::from_session(&session).with_docker_daemon(Some("unix:///a"));
        let on_b = TargetIdentity::from_session(&session).with_docker_daemon(Some("unix:///b"));
        let unknown = TargetIdentity::from_session(&session).with_docker_daemon(None);
        assert_ne!(
            on_a, on_b,
            "the same session on another daemon is another connection"
        );
        assert_ne!(on_a, unknown, "an unresolved daemon is never confirmed");
        // A pinned identity keeps its own daemon; servers have none
        let pinned = TargetIdentity::container("pg", Some("unix:///a"));
        assert_eq!(pinned.clone().with_docker_daemon(Some("unix:///b")), pinned);
        let server = TargetIdentity::from_session(&saved("h", 5432, "d", &[]));
        assert_eq!(server.clone().with_docker_daemon(Some("unix:///a")), server);
    }

    #[test]
    fn the_daemon_a_connection_really_used_replaces_the_requested_one() {
        // A Docker session failed on A, the context moved to B, Retry worked
        let requested = TargetIdentity::container("pg", Some("unix:///a.sock"));
        let recorded = requested
            .clone()
            .with_actual_docker_daemon(Some("unix:///b.sock"));
        assert_eq!(
            recorded,
            TargetIdentity::container("pg", Some("unix:///b.sock"))
        );
        assert_ne!(
            recorded, requested,
            "switching back to A must not reuse B's connection"
        );
        // No pin (not a container connection): untouched
        assert_eq!(requested.clone().with_actual_docker_daemon(None), requested);
        let server = TargetIdentity::from_session(&saved("h", 5432, "d", &[]));
        assert_eq!(
            server
                .clone()
                .with_actual_docker_daemon(Some("unix:///b.sock")),
            server
        );
    }

    #[test]
    fn daemon_pins_come_from_requests() {
        assert_eq!(daemon_pin_of(&ConnectRequest::url("docker://pg")), None);
        assert_eq!(daemon_pin_of(&ConnectRequest::session("prod")), None);
        let mut info =
            dbcrust_core::database::ConnectionInfo::parse_url("docker://pg").expect("info");
        info.options
            .insert(PINNED_DAEMON_OPTION.to_string(), "context:orbstack".into());
        assert_eq!(
            daemon_pin_of(&ConnectRequest::info(info)).as_deref(),
            Some("context:orbstack")
        );
    }

    #[test]
    fn first_run_means_nothing_saved_or_recent() {
        assert!(is_first_run(&[], &[]));
        assert!(!is_first_run(
            &[session("a", "t", DatabaseType::SQLite)],
            &[]
        ));
    }

    #[test]
    fn launcher_state_round_trips_through_toml() {
        let mut state = LauncherState {
            last_selected: Some("session:prod".into()),
            entries: Vec::new(),
        };
        let mut meta = ConnectionMeta::new("session:prod");
        meta.environment = "Production".into();
        meta.color = Some(EnvColor::Red);
        meta.read_only = true;
        state.set_meta(meta);
        let text = toml::to_string_pretty(&state).expect("serialize");
        let back: LauncherState = toml::from_str(&text).expect("parse");
        assert_eq!(back, state);
        // An older file without the table
        let empty: LauncherState = toml::from_str("").expect("empty");
        assert_eq!(empty, LauncherState::default());
    }
}
