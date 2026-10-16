//! Building and parsing connection URLs for the connect form.
//!
//! Pure functions, no I/O: the form turns its fields into a URL
//! ([`build_url`]) and a URL back into fields ([`parse_target`]). Every value
//! is percent-encoded the way `dbcrust::database::ConnectionInfo::parse_url`
//! decodes it, so a password like `p@ss:/?#%` or a database called `a/b` survives
//! the round trip (the tests below check this against DBCrust's own parser).
//!
//! Beyond the URL itself the module holds the small, testable rules of the
//! connect page: which TLS control an engine has, how a bare file path
//! becomes a `scheme:///…` URL, what to call a saved session, and how old a
//! recent connection is.

use dbcrust::database::{DatabaseType, DatabaseTypeExt};

/// Shown instead of a password wherever a URL is displayed.
pub const PASSWORD_MASK: &str = "••••••••";

/// PostgreSQL `sslmode` values offered by the form (DBCrust also accepts
/// `allow`).
pub const SSL_MODES: [&str; 5] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

/// How an engine's TLS is configured in the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsSetting {
    /// No TLS option (file engines).
    None,
    /// PostgreSQL: `sslmode=<one of SSL_MODES>`.
    SslMode,
    /// A boolean query option (`ssl=true` / `tls=true`).
    Flag(&'static str),
}

/// The TLS control `engine` offers. The option names are the ones DBCrust
/// reads: `ssl` for Elasticsearch (it only looks at that key) and ClickHouse
/// (its error hints say `?ssl=true`), `tls` for the rest.
pub fn tls_setting(engine: &DatabaseType) -> TlsSetting {
    match engine {
        DatabaseType::PostgreSQL | DatabaseType::Redshift => TlsSetting::SslMode,
        DatabaseType::MySQL
        | DatabaseType::MongoDB
        | DatabaseType::WhiteDragon
        | DatabaseType::Redis
        | DatabaseType::InfluxDB => TlsSetting::Flag("tls"),
        DatabaseType::ClickHouse | DatabaseType::Elasticsearch => TlsSetting::Flag("ssl"),
        // On unless `encrypt=false`: see `ConnectionSpec::tls_enabled`
        DatabaseType::SqlServer => TlsSetting::Flag("encrypt"),
        // On unless `tls=false`: see `ConnectionSpec::tls_enabled`
        DatabaseType::LibSql => TlsSetting::Flag("tls"),
        _ => TlsSetting::None,
    }
}

/// Engines whose clients read a database name from the URL path.
pub fn uses_database_name(engine: &DatabaseType) -> bool {
    !engine.is_file_based()
        && !matches!(
            engine,
            DatabaseType::Elasticsearch | DatabaseType::WhiteDragon
        )
}

/// A connection described field by field.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionSpec {
    pub engine: DatabaseType,
    /// A non-canonical scheme to keep when rebuilding (`mongodb+srv`,
    /// `postgresql`, …). `None` uses the engine's own scheme.
    pub scheme: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    pub password: String,
    pub database: String,
    /// File engines: the file (or glob) path.
    pub path: String,
    /// Query options in the order they appear in the URL.
    pub options: Vec<(String, String)>,
}

/// Option pairs for `Debug`: secret-looking values masked.
fn masked_options(options: &[(String, String)]) -> Vec<(&str, String)> {
    options
        .iter()
        .map(|(key, value)| {
            let value = if dbcrust::password_sanitizer::is_secret_key(key) {
                "***".to_string()
            } else {
                dbcrust::password_sanitizer::redact_text(value)
            };
            (key.as_str(), value)
        })
        .collect()
}

// `Debug` never shows the password or secret-looking option values.
impl std::fmt::Debug for ConnectionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionSpec")
            .field("engine", &self.engine)
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .field("database", &self.database)
            .field("path", &self.path)
            .field("options", &masked_options(&self.options))
            .finish()
    }
}

impl ConnectionSpec {
    pub fn new(engine: DatabaseType) -> Self {
        Self {
            engine,
            scheme: None,
            host: String::new(),
            port: None,
            user: String::new(),
            password: String::new(),
            database: String::new(),
            path: String::new(),
            options: Vec::new(),
        }
    }

    /// Value of a query option.
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Set (`Some`) or remove (`None`) a query option, keeping the others'
    /// order.
    pub fn set_option(&mut self, key: &str, value: Option<&str>) {
        match (self.options.iter().position(|(k, _)| k == key), value) {
            (Some(ix), Some(value)) => self.options[ix].1 = value.to_string(),
            (Some(ix), None) => {
                self.options.remove(ix);
            }
            (None, Some(value)) => self.options.push((key.to_string(), value.to_string())),
            (None, None) => {}
        }
    }

    /// TLS requested through any of the options DBCrust understands
    /// (`ssl`/`tls`/`secure` true, or a strict `sslmode`).
    pub fn tls_enabled(&self) -> bool {
        if self.engine == DatabaseType::SqlServer {
            // SQL Server encrypts unless the URL says `encrypt=false`
            return !self.options.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("encrypt")
                    && matches!(
                        value.to_ascii_lowercase().as_str(),
                        "false" | "0" | "no" | "off" | "optional"
                    )
            });
        }
        if self.engine == DatabaseType::LibSql {
            // libSQL speaks HTTPS unless the URL says `tls=false`
            return !self.options.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("tls")
                    && matches!(
                        value.to_ascii_lowercase().as_str(),
                        "false" | "0" | "no" | "off"
                    )
            });
        }
        if self.engine == DatabaseType::Redshift {
            // Redshift requires TLS unless `sslmode` explicitly says less
            return !self.options.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("sslmode")
                    && matches!(
                        value.to_ascii_lowercase().as_str(),
                        "disable" | "allow" | "prefer"
                    )
            });
        }
        self.options.iter().any(|(key, value)| {
            let value = value.to_ascii_lowercase();
            match key.to_ascii_lowercase().as_str() {
                "ssl" | "tls" | "secure" => matches!(value.as_str(), "true" | "1" | "yes" | "on"),
                "sslmode" => matches!(value.as_str(), "require" | "verify-ca" | "verify-full"),
                _ => false,
            }
        })
    }

    /// Turn the engine's boolean TLS option on or off (no-op for engines
    /// without one). Clears the aliases so the URL never says both.
    pub fn set_tls(&mut self, enabled: bool) {
        if self.engine == DatabaseType::SqlServer {
            // Encrypted is the default, so only "off" is written (and
            // `encrypt=strict` stays strict while encryption stays on)
            let strict = enabled
                && self.options.iter().any(|(k, v)| {
                    k.eq_ignore_ascii_case("encrypt") && v.eq_ignore_ascii_case("strict")
                });
            self.options
                .retain(|(k, _)| !k.eq_ignore_ascii_case("encrypt"));
            if !enabled {
                self.options
                    .push(("encrypt".to_string(), "false".to_string()));
            } else if strict {
                self.options
                    .push(("encrypt".to_string(), "strict".to_string()));
            }
            return;
        }
        if self.engine == DatabaseType::LibSql {
            // TLS is the default, so only "off" is written
            self.options.retain(|(k, _)| !k.eq_ignore_ascii_case("tls"));
            if !enabled {
                self.options.push(("tls".to_string(), "false".to_string()));
            }
            return;
        }
        if let TlsSetting::Flag(key) = tls_setting(&self.engine) {
            self.options.retain(|(k, _)| {
                !matches!(k.to_ascii_lowercase().as_str(), "ssl" | "tls" | "secure")
            });
            if enabled {
                self.options.push((key.to_string(), "true".to_string()));
            }
        }
    }

    /// File engines: expand `~` and anchor a relative path (the GUI's own
    /// working directory is rarely the user's). Pure: callers pass the
    /// directories.
    pub fn resolve_path(&mut self, home: Option<&str>, cwd: Option<&str>) {
        if self.engine.is_file_based() {
            self.path = resolve_file_path(&self.path, home, cwd);
        }
    }
}

/// A Docker connection
/// (`docker://[user[:password]@]container[/database][?options]`).
/// DBCrust does not percent-decode the userinfo, container and database, so
/// those parts are used verbatim; option values are percent-encoded.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DockerSpec {
    pub user: String,
    pub password: String,
    pub container: String,
    pub database: String,
    /// Connection options (`sslmode=verify-full`, …) in URL order. `ssl=true`
    /// is not here: it is [`DockerSpec::use_tls`].
    pub options: Vec<(String, String)>,
    pub use_tls: bool,
}

impl std::fmt::Debug for DockerSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerSpec")
            .field("user", &self.user)
            .field(
                "password",
                &if self.password.is_empty() { "" } else { "***" },
            )
            .field("container", &self.container)
            .field("database", &self.database)
            .field("options", &masked_options(&self.options))
            .field("use_tls", &self.use_tls)
            .finish()
    }
}

impl DockerSpec {
    /// The same connection as `dbcrust`'s canonical Docker target (the parser
    /// and renderer of `docker://` URLs).
    pub fn to_target(&self) -> dbcrust::docker::DockerTarget {
        let some = |text: &str| Some(text.trim().to_string()).filter(|text| !text.is_empty());
        dbcrust::docker::DockerTarget {
            user: some(&self.user),
            password: Some(self.password.clone()).filter(|password| !password.is_empty()),
            container: self.container.trim().to_string(),
            database: some(&self.database),
            options: self.options.iter().cloned().collect(),
            use_tls: self.use_tls,
        }
    }

    pub fn from_target(target: dbcrust::docker::DockerTarget) -> Self {
        Self {
            user: target.user.unwrap_or_default(),
            password: target.password.unwrap_or_default(),
            container: target.container,
            database: target.database.unwrap_or_default(),
            options: target.options.into_iter().collect(),
            use_tls: target.use_tls,
        }
    }
}

/// A Vault dynamic-credentials connection (`vault://role@mount/database`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VaultSpec {
    pub role: String,
    pub mount: String,
    pub database: String,
}

impl VaultSpec {
    /// Role and database are both mandatory to connect.
    pub fn is_complete(&self) -> bool {
        !self.role.is_empty() && !self.database.is_empty()
    }
}

/// Anything the connect form can describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Database(ConnectionSpec),
    Docker(DockerSpec),
    Vault(VaultSpec),
}

// ── Percent encoding ───────────────────────────────────────────────────

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn push_escaped(out: &mut String, byte: u8) {
    out.push('%');
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0F) as usize] as char);
}

/// Encode everything except RFC 3986 unreserved characters. Used for user,
/// password, database and query keys/values, where over-encoding is harmless
/// (DBCrust decodes) and under-encoding corrupts the URL.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            push_escaped(&mut out, byte);
        }
    }
    out
}

/// Encode a file path: keeps `/`, `:`, `*` (globs) and friends readable and
/// escapes only what would change the URL's structure or be decoded twice
/// (`%`, `?`, `#`, spaces, brackets, non-ASCII, …).
fn encode_file_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        let escape = !(0x21..0x7F).contains(&byte)
            || matches!(
                byte,
                b'"' | b'#'
                    | b'%'
                    | b'<'
                    | b'>'
                    | b'?'
                    | b'['
                    | b']'
                    | b'`'
                    | b'{'
                    | b'}'
                    | b'\\'
                    | b'^'
                    | b'|'
            );
        if escape {
            push_escaped(&mut out, byte);
        } else {
            out.push(byte as char);
        }
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_bytes(input: &str, plus_is_space: bool) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        match bytes[ix] {
            b'%' => {
                let digit = |offset: usize| bytes.get(ix + offset).copied().and_then(hex_value);
                if let (Some(high), Some(low)) = (digit(1), digit(2)) {
                    out.push(high << 4 | low);
                    ix += 3;
                } else {
                    out.push(b'%');
                    ix += 1;
                }
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                ix += 1;
            }
            byte => {
                out.push(byte);
                ix += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode `%XX` escapes (invalid escapes stay as typed).
pub fn percent_decode(input: &str) -> String {
    decode_bytes(input, false)
}

// ── Building ───────────────────────────────────────────────────────────

/// Build the connection URL for `spec`. Never fails: missing hosts become
/// `localhost`, and validation is [`validate`]'s job.
pub fn build_url(spec: &ConnectionSpec) -> String {
    render_url(spec, Secret::Reveal)
}

/// [`build_url`] for display: the password is replaced by [`PASSWORD_MASK`]
/// while the URL is being assembled from the fields, never by searching a
/// finished URL for it.
pub fn build_masked_url(spec: &ConnectionSpec) -> String {
    render_url(spec, Secret::Mask)
}

/// What to write where a password goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Secret {
    Reveal,
    Mask,
}

fn render_url(spec: &ConnectionSpec, secret: Secret) -> String {
    let scheme = spec
        .scheme
        .as_deref()
        .unwrap_or_else(|| spec.engine.url_scheme());
    let mut url = format!("{scheme}://");

    if spec.engine.is_file_based() {
        // `scheme:///abs/path`: the path's own leading slash is the third one
        url.push('/');
        url.push_str(&encode_file_path(spec.path.trim().trim_start_matches('/')));
    } else {
        let user = spec.user.trim();
        if !user.is_empty() || !spec.password.is_empty() {
            url.push_str(&percent_encode(user));
            if !spec.password.is_empty() {
                url.push(':');
                match secret {
                    Secret::Reveal => url.push_str(&percent_encode(&spec.password)),
                    Secret::Mask => url.push_str(PASSWORD_MASK),
                }
            }
            url.push('@');
        }
        let host = spec.host.trim();
        if host.is_empty() {
            url.push_str("localhost");
        } else if host.contains(':') && !host.starts_with('[') {
            url.push('[');
            url.push_str(host);
            url.push(']');
        } else {
            url.push_str(host);
        }
        if let Some(port) = spec.port {
            url.push(':');
            url.push_str(&port.to_string());
        }
        let database = spec.database.trim();
        if !database.is_empty() {
            url.push('/');
            url.push_str(&percent_encode(database));
        }
    }

    let query: Vec<String> = spec
        .options
        .iter()
        .filter(|(key, _)| !key.is_empty())
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect();
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    url
}

/// `docker://[user[:password]@]container[/database]`. The parts are written
/// verbatim because DBCrust does not decode these URLs; [`validate_docker`]
/// says which characters that rules out.
pub fn build_docker_url(spec: &DockerSpec) -> String {
    render_docker_url(spec, Secret::Reveal)
}

/// [`build_docker_url`] for display (see [`build_masked_url`]).
pub fn build_masked_docker_url(spec: &DockerSpec) -> String {
    render_docker_url(spec, Secret::Mask)
}

fn render_docker_url(spec: &DockerSpec, secret: Secret) -> String {
    let mut url = String::from("docker://");
    let user = spec.user.trim();
    if !user.is_empty() {
        url.push_str(user);
        if !spec.password.is_empty() {
            url.push(':');
            match secret {
                Secret::Reveal => url.push_str(&spec.password),
                Secret::Mask => url.push_str(PASSWORD_MASK),
            }
        }
        url.push('@');
    }
    url.push_str(spec.container.trim());
    let database = spec.database.trim();
    if !database.is_empty() {
        url.push('/');
        url.push_str(database);
    }
    let mut query: Vec<String> = spec
        .options
        .iter()
        .filter(|(key, _)| !key.is_empty())
        // What is shown never carries a secret-looking option
        .filter(|(key, _)| secret == Secret::Reveal || !looks_secret(key))
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect();
    if spec.use_tls {
        query.push("ssl=true".to_string());
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    url
}

/// Option names that hold a secret.
pub fn looks_secret(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["password", "passwd", "pwd", "secret", "token"]
        .iter()
        .any(|word| key.contains(word))
}

/// `vault://role@mount/database` (role and database may be missing).
pub fn build_vault_url(spec: &VaultSpec) -> String {
    let mount = if spec.mount.trim().is_empty() {
        "database"
    } else {
        spec.mount.trim()
    };
    let mut url = String::from("vault://");
    if !spec.role.trim().is_empty() {
        url.push_str(spec.role.trim());
        url.push('@');
    }
    url.push_str(mount);
    if !spec.database.trim().is_empty() {
        url.push('/');
        url.push_str(spec.database.trim());
    }
    url
}

pub fn build_target_url(target: &Target) -> String {
    match target {
        Target::Database(spec) => build_url(spec),
        Target::Docker(spec) => build_docker_url(spec),
        Target::Vault(spec) => build_vault_url(spec),
    }
}

/// The URL of `target` as it may be shown on screen: built from the fields
/// with the password replaced by [`PASSWORD_MASK`], so no password can reach
/// the display whatever characters it contains.
pub fn build_masked_target_url(target: &Target) -> String {
    match target {
        Target::Database(spec) => build_masked_url(spec),
        Target::Docker(spec) => build_masked_docker_url(spec),
        Target::Vault(spec) => build_vault_url(spec),
    }
}

// ── Parsing ────────────────────────────────────────────────────────────

struct UrlParts<'a> {
    scheme: &'a str,
    /// Everything after `://`, without fragment and query.
    rest: &'a str,
    query: &'a str,
}

fn split_url(url: &str) -> Option<UrlParts<'_>> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    Some(UrlParts {
        scheme,
        rest,
        query,
    })
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode_bytes(key, true), decode_bytes(value, true))
        })
        .collect()
}

fn engine_for_scheme(scheme: &str) -> Option<DatabaseType> {
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "docker" || scheme == "vault" {
        return None;
    }
    DatabaseType::from_scheme(&scheme).ok()
}

/// The engine a URL scheme (`postgres`, `mongodb+srv`, `wd`, …) selects.
pub fn engine_from_scheme(scheme: &str) -> Option<DatabaseType> {
    engine_for_scheme(scheme)
}

/// Parse a database URL (every scheme DBCrust accepts except `docker://` and
/// `vault://`, see [`parse_target`]). Mirrors `ConnectionInfo::parse_url`:
/// user, password and database are percent-decoded, the database is the first
/// path segment, and query pairs become options.
pub fn parse_url(url: &str) -> Option<ConnectionSpec> {
    let parts = split_url(url)?;
    let engine = engine_for_scheme(parts.scheme)?;
    let mut spec = ConnectionSpec::new(engine.clone());
    let scheme = parts.scheme.to_ascii_lowercase();
    if scheme != engine.url_scheme() {
        spec.scheme = Some(scheme);
    }
    spec.options = parse_query(parts.query);

    if engine.is_file_based() {
        // sqlite:///abs, sqlite:////abs, sqlite:///./rel, sqlite://./rel
        let raw = percent_decode(parts.rest);
        spec.path = if raw.starts_with("/./") || raw.starts_with("/../") || raw.starts_with("//") {
            raw[1..].to_string()
        } else {
            raw
        };
        return Some(spec);
    }

    let (authority, path) = match parts.rest.find('/') {
        Some(ix) => (&parts.rest[..ix], &parts.rest[ix..]),
        None => (parts.rest, ""),
    };
    let host_port = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => {
            let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
            spec.user = percent_decode(user);
            spec.password = percent_decode(password);
            host_port
        }
        None => authority,
    };
    let (host, port) = split_host_port(host_port);
    spec.host = host;
    spec.port = port;
    spec.database = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .map(percent_decode)
        .unwrap_or_default();
    Some(spec)
}

fn split_host_port(host_port: &str) -> (String, Option<u16>) {
    if let Some(stripped) = host_port.strip_prefix('[') {
        // [::1]:5432
        if let Some((host, rest)) = stripped.split_once(']') {
            let port = rest.strip_prefix(':').and_then(|p| p.parse().ok());
            return (host.to_string(), port);
        }
    }
    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (host.to_string(), Some(port)),
            Err(_) => (host_port.to_string(), None),
        },
        _ => (host_port.to_string(), None),
    }
}

/// Parse `docker://[user[:password]@]container[/database][?options]` with
/// `dbcrust`'s canonical parser: options and `ssl=true` are kept.
pub fn parse_docker_url(url: &str) -> Option<DockerSpec> {
    let url = url.trim();
    // The scheme is case-insensitive here, as everywhere else on the page
    let canonical;
    let url = match url.get(..9) {
        Some(scheme) if scheme.eq_ignore_ascii_case("docker://") => {
            canonical = format!("docker://{}", &url[9..]);
            canonical.as_str()
        }
        _ => url,
    };
    dbcrust::docker::DockerTarget::parse(url).map(DockerSpec::from_target)
}

/// Parse `vault://[role@][mount][/database]`.
pub fn parse_vault_url(url: &str) -> Option<VaultSpec> {
    let (role, mount, database) = dbcrust::vault_client::parse_vault_url(url.trim())?;
    Some(VaultSpec {
        role: role.unwrap_or_default(),
        mount,
        database: database.unwrap_or_default(),
    })
}

/// Classify and parse any URL the connect form can show as fields.
pub fn parse_target(url: &str) -> Option<Target> {
    let url = url.trim();
    let lower = url
        .get(..9)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if lower.starts_with("docker://") {
        parse_docker_url(url).map(Target::Docker)
    } else if lower.starts_with("vault://") {
        parse_vault_url(url).map(Target::Vault)
    } else {
        parse_url(url).map(Target::Database)
    }
}

// ── Passwords in displayed URLs ────────────────────────────────────────
//
// A displayed URL is built from the fields with the password replaced
// ([`build_masked_target_url`]); these helpers only deal with the mask in a
// URL the user may edit and then connect with.

/// Locate the userinfo's password inside `url`, as a byte range.
fn password_range(url: &str) -> Option<std::ops::Range<usize>> {
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let at = authority.rfind('@')?;
    let colon = authority[..at].find(':')?;
    Some(scheme_end + colon + 1..scheme_end + at)
}

fn is_docker_url(url: &str) -> bool {
    url.get(..9)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("docker://"))
}

/// Undo the display mask: a URL whose password is the mask gets `password`
/// back (percent-encoded; verbatim for `docker://`, which DBCrust does not
/// decode; removed when empty). Any other URL is returned unchanged, so a
/// password typed over the mask wins.
pub fn unmask_password(url: &str, password: &str) -> String {
    match password_range(url) {
        Some(range) if &url[range.clone()] == PASSWORD_MASK => {
            if password.is_empty() {
                // drop ":" + mask
                format!("{}{}", &url[..range.start - 1], &url[range.end..])
            } else {
                let password = if is_docker_url(url) {
                    password.to_string()
                } else {
                    percent_encode(password)
                };
                format!("{}{}{}", &url[..range.start], password, &url[range.end..])
            }
        }
        _ => url.to_string(),
    }
}

/// True when the displayed URL still carries the mask (its password is the
/// stored one, not something newly typed).
pub fn has_masked_password(url: &str) -> bool {
    password_range(url).is_some_and(|range| &url[range] == PASSWORD_MASK)
}

/// Can `password` stand in a masked `docker://` URL once it is unmasked?
/// `Ok` for every other URL.
pub fn check_unmasked_password(url: &str, password: &str) -> Result<(), String> {
    if is_docker_url(url) && has_masked_password(url) {
        validate_docker_password(password)
    } else {
        Ok(())
    }
}

// ── Bare paths → URLs ──────────────────────────────────────────────────

/// Engine (and default options) for a data file, judged by extension the way
/// the CLI does (compression suffixes ignored).
pub fn detect_file_engine(path: &str) -> Option<(DatabaseType, Vec<(&'static str, &'static str)>)> {
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    let mut name = file_name.as_str();
    for suffix in [".gz", ".gzip", ".bz2", ".xz", ".zst"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped;
            break;
        }
    }
    let ends = |extensions: &[&str]| extensions.iter().any(|ext| name.ends_with(ext));
    if ends(&[".parquet", ".parq"]) {
        Some((DatabaseType::Parquet, vec![]))
    } else if ends(&[".csv"]) {
        Some((DatabaseType::CSV, vec![]))
    } else if ends(&[".tsv", ".tab"]) {
        Some((DatabaseType::CSV, vec![("delimiter", "\t")]))
    } else if ends(&[".json", ".jsonl", ".ndjson"]) {
        Some((DatabaseType::JSON, vec![]))
    } else if ends(&[".sqlite", ".sqlite3", ".db", ".db3"]) {
        Some((DatabaseType::SQLite, vec![]))
    } else if ends(&[".duckdb", ".ddb"]) {
        Some((DatabaseType::DuckDB, vec![]))
    } else {
        None
    }
}

/// `~` → home, relative → anchored at `cwd`.
pub fn resolve_file_path(path: &str, home: Option<&str>, cwd: Option<&str>) -> String {
    let path = path.trim();
    let expanded = match (path.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", home.trim_end_matches('/'))
        }
        _ => path.to_string(),
    };
    if expanded.is_empty() || expanded.starts_with('/') {
        return expanded;
    }
    match cwd {
        Some(cwd) => {
            let relative = expanded.strip_prefix("./").unwrap_or(&expanded);
            format!("{}/{relative}", cwd.trim_end_matches('/'))
        }
        None => expanded,
    }
}

/// Turn what the user typed in the URL box into a URL DBCrust connects to:
/// URLs pass through; a bare data-file path becomes `scheme:///abs/path`
/// (like the CLI); anything else is read as `user:pass@host/db` shorthand for
/// PostgreSQL.
pub fn normalize_target(input: &str, home: Option<&str>, cwd: Option<&str>) -> String {
    let input = input.trim();
    if input.is_empty() || input.contains("://") {
        return input.to_string();
    }
    let (path, query) = input.split_once('?').unwrap_or((input, ""));
    if let Some((engine, defaults)) = detect_file_engine(path) {
        let mut spec = ConnectionSpec::new(engine);
        spec.path = resolve_file_path(path, home, cwd);
        spec.options = parse_query(query);
        for (key, value) in defaults {
            if spec.option(key).is_none() {
                spec.options.push((key.to_string(), value.to_string()));
            }
        }
        return build_url(&spec);
    }
    format!("postgres://{input}")
}

// ── Validation ─────────────────────────────────────────────────────────

/// Parse the port field: empty means "engine default".
pub fn parse_port(text: &str) -> Result<Option<u16>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<u16>() {
        Ok(0) | Err(_) => Err("Port must be a number between 1 and 65535".to_string()),
        Ok(port) => Ok(Some(port)),
    }
}

/// What is missing before `spec` can connect.
pub fn validate(spec: &ConnectionSpec) -> Result<(), String> {
    if spec.engine.is_file_based() && spec.path.trim().is_empty() {
        return Err(format!("Choose a {} file", spec.engine.display_name()));
    }
    Ok(())
}

/// `docker://` URLs are cut up by plain character searches and never
/// percent-decoded (`DockerClient::parse_docker_url`), after the `url` crate
/// has accepted them (`ConnectionInfo::parse_url`). Escaping cannot help, so
/// the characters that would change how the URL is split are refused instead
/// of silently sending a different user, password or container.
const DOCKER_USER_FORBIDDEN: [char; 5] = [':', '@', '/', '?', '#'];
const DOCKER_PASSWORD_FORBIDDEN: [char; 4] = ['@', '/', '?', '#'];
const DOCKER_CONTAINER_FORBIDDEN: [char; 5] = [':', '@', '/', '?', '#'];

fn forbidden_in(value: &str, forbidden: &[char]) -> Option<char> {
    value.chars().find(|c| forbidden.contains(c))
}

fn quoted_list(forbidden: &[char]) -> String {
    forbidden
        .iter()
        .map(|c| format!("“{c}”"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A Docker password DBCrust can read back from the URL.
pub fn validate_docker_password(password: &str) -> Result<(), String> {
    match forbidden_in(password, &DOCKER_PASSWORD_FORBIDDEN) {
        Some(_) => Err(format!(
            "A Docker password can’t contain {} (docker:// URLs are not percent-decoded). \
             Use the container’s published host and port with a regular URL instead.",
            quoted_list(&DOCKER_PASSWORD_FORBIDDEN)
        )),
        None => Ok(()),
    }
}

/// What cannot be written into a `docker://` URL.
pub fn validate_docker(spec: &DockerSpec) -> Result<(), String> {
    let user = spec.user.trim();
    if forbidden_in(user, &DOCKER_USER_FORBIDDEN).is_some() {
        return Err(format!(
            "A Docker user can’t contain {} (docker:// URLs are not percent-decoded).",
            quoted_list(&DOCKER_USER_FORBIDDEN)
        ));
    }
    if !user.is_empty() {
        validate_docker_password(&spec.password)?;
    }
    let container = spec.container.trim();
    if forbidden_in(container, &DOCKER_CONTAINER_FORBIDDEN).is_some()
        || container.chars().any(char::is_whitespace)
    {
        return Err(
            "A container name can’t contain spaces, “:”, “@”, “/”, “?” or “#”.".to_string(),
        );
    }
    if spec.database.trim().contains('@') {
        return Err("A Docker database name can’t contain “@”.".to_string());
    }
    Ok(())
}

// ── Names and ages ─────────────────────────────────────────────────────

fn is_local_host(host: &str) -> bool {
    matches!(host, "" | "localhost" | "127.0.0.1" | "::1" | "unknown")
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// A reasonable default session name for what is about to be saved.
pub fn suggest_session_name(target: &Target) -> String {
    match target {
        Target::Database(spec) if spec.engine.is_file_based() => {
            file_name(spec.path.trim()).to_string()
        }
        Target::Database(spec) => {
            let host = spec.host.trim();
            match (spec.database.trim(), is_local_host(host)) {
                ("", true) => spec.engine.display_name().to_string(),
                ("", false) => host.to_string(),
                (database, true) => database.to_string(),
                (database, false) => format!("{database}@{host}"),
            }
        }
        Target::Docker(spec) => match spec.database.trim() {
            "" => spec.container.trim().to_string(),
            database => format!("{database}@{}", spec.container.trim()),
        },
        Target::Vault(spec) => match spec.database.trim() {
            "" => format!("vault {}", spec.mount.trim()),
            database => format!("{database} (vault)"),
        },
    }
}

/// One-line, password-free description of a target for previews:
/// `user@host:5432/db`, a file path, `docker: container/db`, or the vault URL.
pub fn describe_target(target: &Target) -> String {
    match target {
        Target::Database(spec) if spec.engine.is_file_based() => spec.path.clone(),
        Target::Database(spec) => {
            let mut text = String::new();
            if !spec.user.is_empty() {
                text.push_str(&spec.user);
                text.push('@');
            }
            text.push_str(if spec.host.is_empty() {
                "localhost"
            } else {
                &spec.host
            });
            if let Some(port) = spec.port {
                text.push(':');
                text.push_str(&port.to_string());
            }
            if !spec.database.is_empty() {
                text.push('/');
                text.push_str(&spec.database);
            }
            text
        }
        Target::Docker(spec) => match spec.database.as_str() {
            "" => format!("docker: {}", spec.container),
            database => format!("docker: {}/{database}", spec.container),
        },
        Target::Vault(spec) => build_vault_url(spec),
    }
}

/// "just now", "5 min ago", "2 h ago", "3 d ago", "2 mo ago", "1 y ago".
pub fn relative_age(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    match seconds {
        s if s < 45 => "just now".to_string(),
        s if s < HOUR => format!("{} min ago", (s + 30) / MINUTE),
        s if s < DAY => format!("{} h ago", s / HOUR),
        s if s < 2 * WEEK => format!("{} d ago", s / DAY),
        s if s < 2 * MONTH => format!("{} w ago", s / WEEK),
        s if s < YEAR => format!("{} mo ago", s / MONTH),
        s => format!("{} y ago", s / YEAR),
    }
}

/// Do two URLs point at the same server, database and user? Passwords,
/// options and cosmetic differences (`postgres`/`postgresql`, trailing
/// slashes, DBCrust's `# Docker:` suffix) are ignored. Used to mark recent
/// connections that are currently open.
pub fn same_target(a: &str, b: &str) -> bool {
    fn clean(url: &str) -> &str {
        let url = url.split(" # ").next().unwrap_or(url);
        url.trim().trim_end_matches('/')
    }
    let (a, b) = (clean(a), clean(b));
    if a == b {
        return true;
    }
    match (parse_target(a), parse_target(b)) {
        (Some(Target::Database(a)), Some(Target::Database(b))) => {
            a.engine == b.engine
                && if a.engine.is_file_based() {
                    a.path == b.path
                } else {
                    let port =
                        |spec: &ConnectionSpec| spec.port.or_else(|| spec.engine.default_port());
                    a.host.eq_ignore_ascii_case(&b.host)
                        && port(&a) == port(&b)
                        && a.user == b.user
                        && a.database == b.database
                }
        }
        (Some(Target::Docker(a)), Some(Target::Docker(b))) => {
            a.container == b.container && a.database == b.database
        }
        (Some(Target::Vault(a)), Some(Target::Vault(b))) => a == b,
        _ => false,
    }
}

// ── Form defaults and imports ──────────────────────────────────────────

/// What the connect form starts with for an engine: real values, not
/// placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormDefaults {
    /// `localhost` for servers, empty for files.
    pub host: &'static str,
    pub port: Option<u16>,
    pub user: &'static str,
    /// What the database field means when left empty; `None` when the
    /// engine has no such field.
    pub database_hint: Option<&'static str>,
    /// Example path for the file field.
    pub path_hint: &'static str,
}

pub fn form_defaults(engine: &DatabaseType) -> FormDefaults {
    let file = engine.is_file_based();
    FormDefaults {
        host: if file { "" } else { "localhost" },
        port: engine.default_port(),
        user: engine.default_username(),
        database_hint: match engine {
            _ if !uses_database_name(engine) => None,
            DatabaseType::PostgreSQL => Some("Optional. Empty connects to “postgres”."),
            DatabaseType::Redis => {
                Some("Optional. The logical database number (0–15). Empty is 0.")
            }
            DatabaseType::SqlServer => {
                Some("Optional. Empty opens the login’s default database (usually “master”).")
            }
            DatabaseType::Redshift => {
                Some("Optional. Empty opens the cluster’s default database (“dev”).")
            }
            DatabaseType::LibSql => {
                Some("Optional. The sqld namespace (x-namespace); empty for Turso Cloud.")
            }
            DatabaseType::InfluxDB => Some(
                "The database (v1, v3) or bucket (v2) to open. The password field is the API token for v2 and v3.",
            ),
            _ => Some("Optional. The database to open first."),
        },
        path_hint: match engine {
            DatabaseType::SQLite => "/path/to/database.db",
            DatabaseType::DuckDB => "/path/to/database.duckdb",
            DatabaseType::Parquet => "/path/to/data.parquet (globs work: /data/*.parquet)",
            DatabaseType::CSV => "/path/to/data.csv",
            _ => "/path/to/data.json",
        },
    }
}

/// What the secret field of the form holds for `engine`.
pub fn secret_label(engine: &DatabaseType) -> &'static str {
    match engine {
        DatabaseType::LibSql => "Auth token",
        _ => "Password",
    }
}

/// Extra words a search in the engine picker matches besides the name.
pub fn engine_aliases(engine: &DatabaseType) -> &'static str {
    match engine {
        DatabaseType::PostgreSQL => "pg postgres postgresql psql",
        DatabaseType::MySQL => "mariadb",
        DatabaseType::SQLite => "sqlite3 db file",
        DatabaseType::ClickHouse => "ch clickhouse",
        DatabaseType::MongoDB => "mongo nosql",
        DatabaseType::Elasticsearch => "es elastic opensearch",
        DatabaseType::WhiteDragon => "wd search",
        DatabaseType::Redis => "valkey keydb dragonfly cache kv",
        DatabaseType::SqlServer => "mssql sqlserver azure sql tsql tds microsoft",
        DatabaseType::Redshift => "aws amazon warehouse spectrum serverless",
        DatabaseType::LibSql => "turso sqld hrana edge sqlite remote",
        DatabaseType::InfluxDB => "influx tsdb timeseries time series flux influxql telegraf",
        DatabaseType::Parquet => "pq columnar file",
        DatabaseType::CSV => "tsv file",
        DatabaseType::JSON => "jsonl ndjson file",
        DatabaseType::DuckDB => "ddb file",
    }
}

/// Read what the user pasted into "Paste connection URL…": a URL of any
/// scheme DBCrust accepts, the path of a data file, or `user:pass@host/db`
/// shorthand. The error says what is accepted.
pub fn import_target(input: &str, home: Option<&str>, cwd: Option<&str>) -> Result<Target, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Paste a connection URL or the path of a data file".to_string());
    }
    let path = input.split_once('?').map_or(input, |(path, _)| path);
    let recognisable =
        input.contains("://") || input.contains('@') || detect_file_engine(path).is_some();
    let normalized = normalize_target(input, home, cwd);
    match parse_target(&normalized).filter(|_| recognisable) {
        Some(target) => Ok(target),
        None => Err(
            "That isn't a connection URL DBCrust recognises. Try postgres://, mysql://, \
             sqlite://, mongodb://, clickhouse://, docker:// or vault://, or the path of a \
             CSV, Parquet, JSON or SQLite file."
                .to_string(),
        ),
    }
}

/// "2 more options (application_name, connect_timeout)" for options a URL
/// carried that the form has no control for; keys only, values may be
/// secrets. `None` when there are none.
pub fn describe_kept_options(options: &[(String, String)]) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    let keys: Vec<&str> = options.iter().map(|(key, _)| key.as_str()).collect();
    let shown = keys.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    let more = keys.len().saturating_sub(3);
    let noun = if keys.len() == 1 { "option" } else { "options" };
    Some(if more > 0 {
        format!(
            "{} {noun} kept from the URL ({shown} and {more} more)",
            keys.len()
        )
    } else {
        format!("{} {noun} kept from the URL ({shown})", keys.len())
    })
}

/// One line on how `spec` treats encryption; `None` for engines without
/// TLS settings.
pub fn tls_summary(spec: &ConnectionSpec) -> Option<String> {
    match tls_setting(&spec.engine) {
        TlsSetting::None => None,
        TlsSetting::SslMode => Some(
            match spec.option("sslmode") {
                None if spec.engine == DatabaseType::Redshift => {
                    "Default · TLS required, certificate not checked"
                }
                None => "Default · uses TLS when the server offers it",
                Some("disable") => "Off · never encrypted",
                Some("allow") => "Allowed · plain first, TLS if the server insists",
                Some("prefer") => "Preferred · TLS when the server offers it",
                Some("require") => "Required · certificate not checked",
                Some("verify-ca") => "Verified · certificate authority checked",
                Some("verify-full") => "Verified · certificate and host name checked",
                Some(_) => "Custom sslmode",
            }
            .to_string(),
        ),
        TlsSetting::Flag(_) => Some(
            if spec.tls_enabled() {
                "On · encrypted with TLS"
            } else {
                "Off · not encrypted"
            }
            .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbcrust::database::ConnectionInfo;
    use rstest::rstest;

    /// The connect form lists every `DatabaseType`; the desktop app depends on
    /// `dbcrust` with the `bundled` preset so that each of them can connect.
    #[test]
    fn the_desktop_build_compiles_every_engine() {
        for engine in DatabaseType::supported_types() {
            assert!(
                dbcrust::drivers::is_compiled(&engine),
                "{engine} is offered by the connect form but not compiled in"
            );
        }
    }

    fn network(engine: DatabaseType, user: &str, password: &str, database: &str) -> ConnectionSpec {
        let mut spec = ConnectionSpec::new(engine);
        spec.host = "db.example.com".into();
        spec.port = Some(6543);
        spec.user = user.into();
        spec.password = password.into();
        spec.database = database.into();
        spec
    }

    #[test]
    fn builds_a_plain_postgres_url() {
        let mut spec = network(DatabaseType::PostgreSQL, "app", "secret", "shop");
        spec.set_option("sslmode", Some("require"));
        assert_eq!(
            build_url(&spec),
            "postgres://app:secret@db.example.com:6543/shop?sslmode=require"
        );
    }

    #[test]
    fn defaults_the_host_and_omits_empty_parts() {
        let spec = ConnectionSpec::new(DatabaseType::MySQL);
        assert_eq!(build_url(&spec), "mysql://localhost");
        let mut spec = ConnectionSpec::new(DatabaseType::Elasticsearch);
        spec.host = "search.local".into();
        spec.port = Some(9200);
        assert_eq!(build_url(&spec), "elasticsearch://search.local:9200");
    }

    #[test]
    fn user_without_password_and_password_without_user() {
        let mut spec = network(DatabaseType::PostgreSQL, "app", "", "");
        assert_eq!(build_url(&spec), "postgres://app@db.example.com:6543");
        spec.user.clear();
        spec.password = "pw".into();
        assert_eq!(build_url(&spec), "postgres://:pw@db.example.com:6543");
    }

    #[rstest]
    #[case("p@ss:/?#%word")]
    #[case("plain")]
    #[case("päss wörd ✓")]
    #[case("a+b=c&d")]
    #[case("%41%")]
    #[case("[brackets]{braces}")]
    #[case("日本語のパスワード")]
    fn credentials_round_trip(#[case] secret: &str) {
        for engine in [
            DatabaseType::PostgreSQL,
            DatabaseType::MySQL,
            DatabaseType::ClickHouse,
            DatabaseType::MongoDB,
        ] {
            let spec = network(engine, secret, secret, secret);
            let url = build_url(&spec);
            assert_eq!(parse_url(&url), Some(spec.clone()), "{url}");
            // DBCrust's own parser must agree with ours
            let info = ConnectionInfo::parse_url(&url).expect("dbcrust parses the URL");
            assert_eq!(info.username.as_deref(), Some(secret), "{url}");
            assert_eq!(info.password.as_deref(), Some(secret), "{url}");
            assert_eq!(info.database.as_deref(), Some(secret), "{url}");
            assert_eq!(info.host.as_deref(), Some("db.example.com"), "{url}");
            assert_eq!(info.port, Some(6543), "{url}");
        }
    }

    #[rstest]
    #[case(DatabaseType::PostgreSQL)]
    #[case(DatabaseType::MySQL)]
    #[case(DatabaseType::ClickHouse)]
    #[case(DatabaseType::MongoDB)]
    #[case(DatabaseType::Elasticsearch)]
    #[case(DatabaseType::WhiteDragon)]
    #[case(DatabaseType::Redis)]
    #[case(DatabaseType::SqlServer)]
    #[case(DatabaseType::Redshift)]
    #[case(DatabaseType::LibSql)]
    #[case(DatabaseType::InfluxDB)]
    fn network_engines_round_trip(#[case] engine: DatabaseType) {
        let mut spec = network(engine, "app", "s3cret", "shop");
        spec.set_tls(true);
        spec.set_option("sslmode", Some("verify-full"));
        let url = build_url(&spec);
        assert!(url.starts_with(&format!("{}://", spec.engine.url_scheme())));
        assert_eq!(parse_url(&url), Some(spec));
    }

    #[test]
    fn ipv6_hosts_are_bracketed() {
        let mut spec = ConnectionSpec::new(DatabaseType::PostgreSQL);
        spec.host = "::1".into();
        spec.port = Some(5432);
        assert_eq!(build_url(&spec), "postgres://[::1]:5432");
        assert_eq!(
            parse_url("postgres://[::1]:5432").map(|s| (s.host, s.port)),
            Some(("::1".into(), Some(5432)))
        );
        let info = ConnectionInfo::parse_url(&build_url(&spec)).expect("parses");
        assert_eq!(info.port, Some(5432));
    }

    #[test]
    fn parses_what_dbcrust_documents() {
        let spec = parse_url("postgresql://user:pass%40word@localhost:5432/my%20db?sslmode=require&application_name=a+b")
            .expect("parses");
        assert_eq!(spec.engine, DatabaseType::PostgreSQL);
        assert_eq!(spec.scheme.as_deref(), Some("postgresql"));
        assert_eq!(spec.user, "user");
        assert_eq!(spec.password, "pass@word");
        assert_eq!(spec.database, "my db");
        assert_eq!(spec.option("sslmode"), Some("require"));
        assert_eq!(spec.option("application_name"), Some("a b"));
        assert!(spec.tls_enabled());
        // the non-canonical scheme survives a rebuild
        assert!(build_url(&spec).starts_with("postgresql://"));
    }

    #[test]
    fn mongodb_srv_keeps_its_scheme() {
        let spec = parse_url("mongodb+srv://u:p@cluster0.example.net/app").expect("parses");
        assert_eq!(spec.engine, DatabaseType::MongoDB);
        assert!(build_url(&spec).starts_with("mongodb+srv://"));
    }

    #[test]
    fn unknown_and_special_schemes_are_not_databases() {
        assert_eq!(parse_url("ftp://host/file"), None);
        assert_eq!(parse_url("docker://pg/app"), None);
        assert_eq!(parse_url("vault://ro@database/app"), None);
        assert_eq!(parse_url("not a url"), None);
    }

    #[rstest]
    #[case(
        DatabaseType::SQLite,
        "/Users/me/data/app.db",
        "sqlite:///Users/me/data/app.db"
    )]
    #[case(DatabaseType::Parquet, "/data/*.parquet", "parquet:///data/*.parquet")]
    #[case(DatabaseType::CSV, "/tmp/a b/c#d.csv", "csv:///tmp/a%20b/c%23d.csv")]
    #[case(
        DatabaseType::JSON,
        "/tmp/100%/é.json",
        "json:///tmp/100%25/%C3%A9.json"
    )]
    #[case(
        DatabaseType::DuckDB,
        "/tmp/what?.duckdb",
        "duckdb:///tmp/what%3F.duckdb"
    )]
    fn file_urls(#[case] engine: DatabaseType, #[case] path: &str, #[case] expected: &str) {
        let mut spec = ConnectionSpec::new(engine);
        spec.path = path.into();
        let url = build_url(&spec);
        assert_eq!(url, expected);
        assert_eq!(parse_url(&url).map(|s| s.path), Some(path.to_string()));
        let info = ConnectionInfo::parse_url(&url).expect("dbcrust parses the URL");
        assert_eq!(info.file_path.as_deref(), Some(path), "{url}");
    }

    #[test]
    fn csv_options_round_trip() {
        let mut spec = ConnectionSpec::new(DatabaseType::CSV);
        spec.path = "/tmp/data.csv".into();
        spec.set_option("header", Some("true"));
        spec.set_option("delimiter", Some("\t"));
        let url = build_url(&spec);
        assert_eq!(url, "csv:///tmp/data.csv?header=true&delimiter=%09");
        assert_eq!(parse_url(&url), Some(spec));
        let info = ConnectionInfo::parse_url(&url).expect("parses");
        assert_eq!(
            info.options.get("delimiter").map(String::as_str),
            Some("\t")
        );
    }

    #[rstest]
    #[case("sqlite:///./rel/app.db", "./rel/app.db")]
    #[case("sqlite:////abs/app.db", "/abs/app.db")]
    #[case("sqlite://./rel/app.db", "./rel/app.db")]
    fn parses_file_path_spellings(#[case] url: &str, #[case] path: &str) {
        assert_eq!(parse_url(url).map(|s| s.path), Some(path.to_string()));
    }

    #[test]
    fn tls_helpers_follow_the_engine() {
        let mut pg = ConnectionSpec::new(DatabaseType::PostgreSQL);
        assert_eq!(tls_setting(&pg.engine), TlsSetting::SslMode);
        pg.set_option("sslmode", Some("prefer"));
        assert!(!pg.tls_enabled());
        pg.set_option("sslmode", Some("require"));
        assert!(pg.tls_enabled());

        let mut es = ConnectionSpec::new(DatabaseType::Elasticsearch);
        es.set_tls(true);
        assert_eq!(es.option("ssl"), Some("true"));
        assert!(es.tls_enabled());
        es.set_tls(false);
        assert!(es.options.is_empty());

        let mut my = ConnectionSpec::new(DatabaseType::MySQL);
        my.set_option("secure", Some("1"));
        assert!(my.tls_enabled());
        my.set_tls(true);
        assert_eq!(my.options, vec![("tls".to_string(), "true".to_string())]);

        let sqlite = ConnectionSpec::new(DatabaseType::SQLite);
        assert_eq!(tls_setting(&sqlite.engine), TlsSetting::None);
    }

    #[test]
    fn dbcrust_agrees_about_tls() {
        for (engine, setting) in [
            (DatabaseType::MySQL, true),
            (DatabaseType::ClickHouse, true),
            (DatabaseType::Elasticsearch, true),
            (DatabaseType::MongoDB, true),
            (DatabaseType::WhiteDragon, true),
            (DatabaseType::Redis, true),
            (DatabaseType::SqlServer, true),
            (DatabaseType::InfluxDB, true),
        ] {
            let mut spec = network(engine, "u", "p", "d");
            spec.set_tls(setting);
            let info = ConnectionInfo::parse_url(&build_url(&spec)).expect("parses");
            assert!(info.use_tls, "{:?}", spec.engine);
        }
    }

    #[test]
    fn sql_server_encrypts_unless_told_not_to() {
        let mut spec = network(DatabaseType::SqlServer, "sa", "p", "Sales");
        assert!(spec.tls_enabled(), "the default is encrypted");
        assert!(!build_url(&spec).contains("encrypt"));
        spec.set_tls(false);
        assert!(!spec.tls_enabled());
        let url = build_url(&spec);
        assert!(url.contains("encrypt=false"), "{url}");
        assert!(!ConnectionInfo::parse_url(&url).expect("parses").use_tls);
        spec.set_tls(true);
        assert!(spec.tls_enabled());
        assert!(!build_url(&spec).contains("encrypt"));
        // strict stays strict while encryption stays on
        spec.set_option("encrypt", Some("strict"));
        spec.set_tls(true);
        assert!(build_url(&spec).contains("encrypt=strict"));
        assert!(spec.tls_enabled());
    }

    #[test]
    fn libsql_speaks_https_unless_told_otherwise() {
        let mut spec = network(DatabaseType::LibSql, "", "tok", "");
        spec.host = "db-org.turso.io".to_string();
        assert!(spec.tls_enabled(), "TLS is the default");
        assert!(!build_url(&spec).contains("tls="));
        assert!(
            ConnectionInfo::parse_url(&build_url(&spec))
                .expect("parses")
                .use_tls
        );
        spec.set_tls(false);
        assert!(!spec.tls_enabled());
        let url = build_url(&spec);
        assert!(url.contains("tls=false"), "{url}");
        let info = ConnectionInfo::parse_url(&url).expect("plain HTTP is confirmed by tls=false");
        assert!(!info.use_tls);
        spec.set_tls(true);
        assert!(spec.tls_enabled());
        assert!(!build_url(&spec).contains("tls="));
        assert_eq!(secret_label(&DatabaseType::LibSql), "Auth token");
        assert_eq!(secret_label(&DatabaseType::PostgreSQL), "Password");
    }

    #[test]
    fn redshift_requires_tls_unless_told_otherwise() {
        let mut spec = network(DatabaseType::Redshift, "awsuser", "p", "dev");
        assert!(spec.tls_enabled(), "TLS is the default");
        assert_eq!(tls_setting(&spec.engine), TlsSetting::SslMode);
        assert_eq!(
            tls_summary(&spec).as_deref(),
            Some("Default · TLS required, certificate not checked")
        );
        assert!(
            ConnectionInfo::parse_url(&build_url(&spec))
                .expect("parses")
                .use_tls
        );
        spec.set_option("sslmode", Some("disable"));
        assert!(!spec.tls_enabled());
        let url = build_url(&spec);
        assert!(url.contains("sslmode=disable"), "{url}");
        assert!(!ConnectionInfo::parse_url(&url).expect("parses").use_tls);
        spec.set_option("sslmode", Some("verify-full"));
        assert!(spec.tls_enabled());
    }

    #[test]
    fn set_option_keeps_order() {
        let mut spec = ConnectionSpec::new(DatabaseType::PostgreSQL);
        spec.set_option("a", Some("1"));
        spec.set_option("b", Some("2"));
        spec.set_option("a", Some("3"));
        assert_eq!(
            spec.options,
            vec![("a".into(), "3".into()), ("b".into(), "2".into())]
        );
        spec.set_option("a", None);
        assert_eq!(spec.options, vec![("b".into(), "2".into())]);
    }

    #[test]
    fn docker_urls_round_trip() {
        let spec = DockerSpec {
            user: "postgres".into(),
            password: "pw".into(),
            container: "pg-dev".into(),
            database: "shop".into(),
            ..Default::default()
        };
        let url = build_docker_url(&spec);
        assert_eq!(url, "docker://postgres:pw@pg-dev/shop");
        assert_eq!(parse_docker_url(&url), Some(spec));
        assert_eq!(
            build_docker_url(&DockerSpec {
                container: "pg".into(),
                ..Default::default()
            }),
            "docker://pg"
        );
        assert!(matches!(
            parse_target("docker://pg/app"),
            Some(Target::Docker(_))
        ));
    }

    #[test]
    fn vault_urls_round_trip() {
        let spec = VaultSpec {
            role: "ro".into(),
            mount: "database".into(),
            database: "app".into(),
        };
        let url = build_vault_url(&spec);
        assert_eq!(url, "vault://ro@database/app");
        assert_eq!(parse_vault_url(&url), Some(spec.clone()));
        assert!(spec.is_complete());
        let partial = parse_vault_url("vault://secrets").expect("parses");
        assert_eq!(
            partial,
            VaultSpec {
                role: String::new(),
                mount: "secrets".into(),
                database: String::new()
            }
        );
        assert!(!partial.is_complete());
        assert_eq!(build_vault_url(&VaultSpec::default()), "vault://database");
        assert!(matches!(
            parse_target("vault://ro@m/app"),
            Some(Target::Vault(_))
        ));
    }

    #[test]
    fn masking_hides_only_the_password() {
        let mut spec = network(DatabaseType::PostgreSQL, "app", "s3cret", "shop");
        spec.set_option("sslmode", Some("require"));
        let url = build_url(&spec);
        let masked = build_masked_url(&spec);
        assert_eq!(
            masked,
            format!("postgres://app:{PASSWORD_MASK}@db.example.com:6543/shop?sslmode=require")
        );
        assert!(has_masked_password(&masked));
        assert!(!has_masked_password(&url));
        assert_eq!(unmask_password(&masked, "s3cret"), url);
        // encoded on the way back
        assert_eq!(
            unmask_password(&masked, "p@ss"),
            "postgres://app:p%40ss@db.example.com:6543/shop?sslmode=require"
        );
        // no stored password: the colon goes too
        assert_eq!(
            unmask_password(&masked, ""),
            "postgres://app@db.example.com:6543/shop?sslmode=require"
        );
        // a newly typed password is kept
        assert_eq!(unmask_password(&url, "other"), url);
        // nothing to mask
        spec.password.clear();
        assert_eq!(build_masked_url(&spec), build_url(&spec));
        let sqlite = {
            let mut spec = ConnectionSpec::new(DatabaseType::SQLite);
            spec.path = "/tmp/a.db".into();
            spec
        };
        assert_eq!(build_masked_url(&sqlite), "sqlite:///tmp/a.db");
        // the mask never leaks into a parsed value when unmasked first
        let spec = parse_url(&unmask_password(&masked, "s3cret")).expect("parses");
        assert_eq!(spec.password, "s3cret");
    }

    #[test]
    fn docker_passwords_come_back_verbatim() {
        let spec = DockerSpec {
            user: "postgres".into(),
            password: "p%41:ss".into(),
            container: "pg".into(),
            database: "shop".into(),
            ..Default::default()
        };
        let masked = build_masked_docker_url(&spec);
        assert_eq!(masked, format!("docker://postgres:{PASSWORD_MASK}@pg/shop"));
        // not percent-encoded: DBCrust does not decode docker:// URLs
        assert_eq!(
            unmask_password(&masked, &spec.password),
            build_docker_url(&spec)
        );
        assert_eq!(check_unmasked_password(&masked, "p%41:ss"), Ok(()));
        assert!(check_unmasked_password(&masked, "a/b").is_err());
        // other schemes are not held to the Docker rules
        let pg = build_masked_url(&network(DatabaseType::PostgreSQL, "u", "a/b", "d"));
        assert_eq!(check_unmasked_password(&pg, "a/b"), Ok(()));
    }

    /// Distinctive enough that it can only come from the password.
    const MARK: &str = "Qz9";

    fn passwords() -> Vec<String> {
        [
            "/", "@", ":", "#", "?", "%", "%40", "%2F", "[]", " ", "é", "+&=", "\\",
        ]
        .iter()
        .map(|special| format!("{MARK}{special}{MARK}x"))
        .chain([format!("{MARK}abc/def"), format!("{MARK}@{MARK}@")])
        .collect()
    }

    fn assert_no_secret(preview: &str, password: &str, what: &str) {
        for needle in [MARK, password, percent_encode(password).as_str()] {
            assert!(
                !preview.contains(needle),
                "{what}: {preview:?} contains {needle:?}"
            );
        }
        assert!(preview.contains(PASSWORD_MASK), "{what}: {preview:?}");
    }

    #[test]
    fn previews_never_contain_the_password_for_any_engine() {
        for password in passwords() {
            for engine in DatabaseType::supported_types() {
                if engine.is_file_based() {
                    // no userinfo at all
                    let mut spec = ConnectionSpec::new(engine.clone());
                    spec.path = "/tmp/data.file".into();
                    spec.user = "app".into();
                    spec.password = password.clone();
                    let preview = build_masked_target_url(&Target::Database(spec));
                    assert!(!preview.contains(MARK), "{engine:?}: {preview:?}");
                    continue;
                }
                for (user, database) in [("app", "shop"), ("", ""), ("a/b", "c@d")] {
                    let spec = network(engine.clone(), user, &password, database);
                    let preview = build_masked_target_url(&Target::Database(spec));
                    assert_no_secret(&preview, &password, &format!("{engine:?} {user:?}"));
                }
            }
            for (user, container, database) in [
                ("pg", "pg-dev", "shop"),
                ("pg", "pg-dev", ""),
                ("u", "c", "a/b"),
            ] {
                let spec = DockerSpec {
                    user: user.into(),
                    password: password.clone(),
                    container: container.into(),
                    database: database.into(),
                    ..Default::default()
                };
                let preview = build_masked_target_url(&Target::Docker(spec));
                assert_no_secret(&preview, &password, "docker");
            }
            // Vault never carries a password
            let vault = Target::Vault(VaultSpec {
                role: "ro".into(),
                mount: "database".into(),
                database: "app".into(),
            });
            assert!(!build_masked_target_url(&vault).contains(MARK));
        }
    }

    #[rstest]
    #[case("pw/ab")]
    #[case("pw@ab")]
    #[case("pw:ab")]
    #[case("pw#ab")]
    #[case("pw%ab")]
    #[case("pw%40ab")]
    fn a_masked_preview_survives_the_trip_through_the_url_box(#[case] password: &str) {
        for engine in [
            DatabaseType::PostgreSQL,
            DatabaseType::MySQL,
            DatabaseType::ClickHouse,
            DatabaseType::MongoDB,
            DatabaseType::Elasticsearch,
            DatabaseType::WhiteDragon,
            DatabaseType::Redis,
            DatabaseType::SqlServer,
            DatabaseType::Redshift,
            DatabaseType::LibSql,
            DatabaseType::InfluxDB,
        ] {
            let spec = network(engine, "app", password, "shop");
            let masked = build_masked_url(&spec);
            assert!(has_masked_password(&masked), "{masked}");
            assert_eq!(unmask_password(&masked, password), build_url(&spec));
        }
    }

    #[rstest]
    #[case("plain", true)]
    #[case("with space", true)]
    #[case("co:lon", true)]
    #[case("per%cent", true)]
    #[case("per%40cent", true)]
    #[case("é✓", true)]
    #[case("sl/ash", false)]
    #[case("a@t", false)]
    #[case("que?ry", false)]
    #[case("ha#sh", false)]
    fn docker_passwords_are_validated_against_dbcrusts_parser(
        #[case] password: &str,
        #[case] accepted: bool,
    ) {
        let spec = DockerSpec {
            user: "app".into(),
            password: password.into(),
            container: "pg".into(),
            database: "shop".into(),
            ..Default::default()
        };
        assert_eq!(validate_docker(&spec).is_ok(), accepted, "{password}");
        assert_eq!(validate_docker_password(password).is_ok(), accepted);

        // What passes must come out of DBCrust's own parsers unchanged
        let url = build_docker_url(&spec);
        let parsed = ConnectionInfo::parse_url(&url);
        if accepted {
            let info = parsed.expect("dbcrust parses the URL");
            assert_eq!(info.password.as_deref(), Some(password), "{url}");
            assert_eq!(info.username.as_deref(), Some("app"), "{url}");
            assert_eq!(info.docker_container.as_deref(), Some("pg"), "{url}");
            assert_eq!(info.database.as_deref(), Some("shop"), "{url}");
            assert_eq!(parse_docker_url(&url), Some(spec));
        } else {
            // What is refused would be misread (or rejected) by DBCrust
            let misread = match parsed {
                Err(_) => true,
                Ok(info) => {
                    info.password.as_deref() != Some(password)
                        || info.docker_container.as_deref() != Some("pg")
                        || info.database.as_deref() != Some("shop")
                }
            };
            assert!(misread, "{url} is read correctly; the rule is too strict");
        }
    }

    #[rstest]
    #[case("pg-dev", "app", "shop", true)]
    #[case("pg_dev.1", "app", "a/b", true)]
    #[case("", "", "", true)]
    #[case("pg dev", "app", "shop", false)]
    #[case("pg@dev", "app", "shop", false)]
    #[case("pg:5432", "app", "shop", false)]
    #[case("pg/dev", "app", "shop", false)]
    #[case("pg", "a:b", "shop", false)]
    #[case("pg", "a@b", "shop", false)]
    #[case("pg", "a/b", "shop", false)]
    #[case("pg", "app", "sh@p", false)]
    fn docker_parts_are_validated(
        #[case] container: &str,
        #[case] user: &str,
        #[case] database: &str,
        #[case] accepted: bool,
    ) {
        let spec = DockerSpec {
            user: user.into(),
            password: "pw".into(),
            container: container.into(),
            database: database.into(),
            ..Default::default()
        };
        assert_eq!(validate_docker(&spec).is_ok(), accepted);
        if accepted && !container.is_empty() {
            let url = build_docker_url(&spec);
            let info = ConnectionInfo::parse_url(&url).expect("dbcrust parses the URL");
            assert_eq!(info.docker_container.as_deref(), Some(container), "{url}");
            assert_eq!(
                parse_docker_url(&url).map(|s| s.database),
                Some(database.into())
            );
        }
    }

    #[test]
    fn a_docker_password_without_a_user_is_not_written() {
        let spec = DockerSpec {
            password: "a/b".into(),
            container: "pg".into(),
            ..Default::default()
        };
        assert_eq!(build_docker_url(&spec), "docker://pg");
        assert_eq!(validate_docker(&spec), Ok(()));
    }

    #[rstest]
    #[case("/data/x.parquet", "parquet:///data/x.parquet")]
    #[case("/data/x.PARQ", "parquet:///data/x.PARQ")]
    #[case("/data/x.csv.gz", "csv:///data/x.csv.gz")]
    #[case("/data/x.tsv", "csv:///data/x.tsv?delimiter=%09")]
    #[case(
        "/data/x.tsv?header=false",
        "csv:///data/x.tsv?header=false&delimiter=%09"
    )]
    #[case("/data/x.ndjson", "json:///data/x.ndjson")]
    #[case("/data/my app.db", "sqlite:///data/my%20app.db")]
    #[case("/data/x.duckdb", "duckdb:///data/x.duckdb")]
    #[case("~/x.csv", "csv:///home/me/x.csv")]
    #[case("rel/x.csv", "csv:///work/rel/x.csv")]
    #[case("./x.csv", "csv:///work/x.csv")]
    #[case("postgres://u@h/db", "postgres://u@h/db")]
    #[case("docker://pg/app", "docker://pg/app")]
    #[case("u:p@h:5432/db", "postgres://u:p@h:5432/db")]
    #[case("  ", "")]
    fn normalizes_what_the_user_typed(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            normalize_target(input, Some("/home/me"), Some("/work")),
            expected
        );
    }

    #[test]
    fn resolve_path_leaves_non_file_engines_alone() {
        let mut spec = ConnectionSpec::new(DatabaseType::PostgreSQL);
        spec.path = "x".into();
        spec.resolve_path(Some("/h"), Some("/c"));
        assert_eq!(spec.path, "x");
        let mut spec = ConnectionSpec::new(DatabaseType::SQLite);
        spec.path = "~".into();
        spec.resolve_path(Some("/h/"), Some("/c"));
        assert_eq!(spec.path, "/h");
    }

    #[rstest]
    #[case("", Ok(None))]
    #[case("  5432 ", Ok(Some(5432)))]
    #[case("65535", Ok(Some(65535)))]
    #[case("0", Err(()))]
    #[case("65536", Err(()))]
    #[case("-1", Err(()))]
    #[case("abc", Err(()))]
    fn ports(#[case] text: &str, #[case] expected: Result<Option<u16>, ()>) {
        assert_eq!(parse_port(text).map_err(|_| ()), expected);
    }

    #[test]
    fn validation_requires_a_file_for_file_engines() {
        let mut spec = ConnectionSpec::new(DatabaseType::Parquet);
        assert_eq!(validate(&spec), Err("Choose a Parquet file".to_string()));
        spec.path = "/x.parquet".into();
        assert_eq!(validate(&spec), Ok(()));
        assert_eq!(
            validate(&ConnectionSpec::new(DatabaseType::PostgreSQL)),
            Ok(())
        );
    }

    #[test]
    fn session_names() {
        let mut spec = network(DatabaseType::PostgreSQL, "u", "", "shop");
        assert_eq!(
            suggest_session_name(&Target::Database(spec.clone())),
            "shop@db.example.com"
        );
        spec.host = "localhost".into();
        assert_eq!(
            suggest_session_name(&Target::Database(spec.clone())),
            "shop"
        );
        spec.database.clear();
        assert_eq!(
            suggest_session_name(&Target::Database(spec.clone())),
            "PostgreSQL"
        );
        spec.host = "db.internal".into();
        assert_eq!(suggest_session_name(&Target::Database(spec)), "db.internal");

        let mut file = ConnectionSpec::new(DatabaseType::SQLite);
        file.path = "/tmp/data/shop.db".into();
        assert_eq!(suggest_session_name(&Target::Database(file)), "shop.db");
        assert_eq!(
            suggest_session_name(&Target::Docker(DockerSpec {
                container: "pg".into(),
                database: "app".into(),
                ..Default::default()
            })),
            "app@pg"
        );
        assert_eq!(
            suggest_session_name(&Target::Vault(VaultSpec {
                role: "ro".into(),
                mount: "database".into(),
                database: "app".into()
            })),
            "app (vault)"
        );
    }

    #[rstest]
    #[case("postgres://app:pw@db:5432/shop", "app@db:5432/shop")]
    #[case("mysql://db", "db")]
    #[case("sqlite:///tmp/a.db", "/tmp/a.db")]
    #[case("docker://pg/app", "docker: pg/app")]
    #[case("docker://pg", "docker: pg")]
    #[case("vault://ro@database/app", "vault://ro@database/app")]
    fn describes_targets_without_passwords(#[case] url: &str, #[case] expected: &str) {
        let target = parse_target(url).expect("parses");
        assert_eq!(describe_target(&target), expected);
    }

    #[rstest]
    #[case(-5, "just now")]
    #[case(0, "just now")]
    #[case(44, "just now")]
    #[case(60, "1 min ago")]
    #[case(5 * 60, "5 min ago")]
    #[case(59 * 60, "59 min ago")]
    #[case(3600, "1 h ago")]
    #[case(2 * 3600 + 100, "2 h ago")]
    #[case(23 * 3600, "23 h ago")]
    #[case(86_400, "1 d ago")]
    #[case(13 * 86_400, "13 d ago")]
    #[case(14 * 86_400, "2 w ago")]
    #[case(59 * 86_400, "8 w ago")]
    #[case(60 * 86_400, "2 mo ago")]
    #[case(364 * 86_400, "12 mo ago")]
    #[case(800 * 86_400, "2 y ago")]
    fn ages(#[case] seconds: i64, #[case] expected: &str) {
        assert_eq!(relative_age(seconds), expected);
    }

    #[rstest]
    #[case("postgres://u:[REDACTED]@h:5432/db", "postgresql://u@h/db", true)]
    #[case("postgres://u@h/db/", "postgres://u@h:5432/db", true)]
    #[case("postgres://u@h/db", "postgres://u@h/other", false)]
    #[case("postgres://u@h/db", "mysql://u@h/db", false)]
    #[case("postgres://u@h/db", "postgres://v@h/db", false)]
    #[case("docker://pg/app # Docker: pg", "docker://pg/app", true)]
    #[case("docker://pg/app", "docker://pg2/app", false)]
    #[case("sqlite:///tmp/a.db", "sqlite:///tmp/a.db", true)]
    #[case("sqlite:///tmp/a.db", "sqlite:///tmp/b.db", false)]
    #[case("vault://ro@database/app", "vault://ro@database/app", true)]
    #[case("vault://ro@database/app", "vault://rw@database/app", false)]
    fn same_targets(#[case] a: &str, #[case] b: &str, #[case] expected: bool) {
        assert_eq!(same_target(a, b), expected);
    }

    #[test]
    fn percent_codec_is_symmetric() {
        let samples = [
            "",
            "plain",
            "a b",
            "é✓日本",
            "%",
            "%4",
            "%zz",
            "/?#[]@:",
            "~-._",
        ];
        for sample in samples {
            assert_eq!(
                percent_decode(&percent_encode(sample)),
                sample,
                "{sample:?}"
            );
        }
        assert_eq!(percent_encode("~-._abcXYZ019"), "~-._abcXYZ019");
        assert_eq!(percent_decode("a%2fb%2Fc%zz%4"), "a/b/c%zz%4");
    }

    #[test]
    fn debug_output_never_contains_the_password() {
        const CANARY: &str = "Cnry-7f3a9";
        let mut spec = network(DatabaseType::PostgreSQL, "app", CANARY, "shop");
        spec.set_option("sslmode", Some("require"));
        spec.set_option("sslpassword", Some(CANARY));
        let docker = DockerSpec {
            user: "app".into(),
            password: CANARY.into(),
            container: "pg".into(),
            options: vec![("token".into(), CANARY.into())],
            ..Default::default()
        };
        let shown = format!(
            "{spec:?} {spec:#?} {docker:?} {:?}",
            Target::Docker(docker.clone())
        );
        assert!(!shown.contains(CANARY), "{shown}");
        assert!(shown.contains("sslmode"), "{shown}");
        assert!(shown.contains("***"), "{shown}");

        let request = crate::types::ConnectRequest::url(format!("postgres://app:{CANARY}@h/db"));
        let shown = format!("{request:?}");
        assert!(!shown.contains(CANARY), "{shown}");
        assert!(shown.contains("[REDACTED]"), "{shown}");
        let error = crate::error::GuiError::Connection(format!(
            "connection to postgres://app:{CANARY}@h/db?password={CANARY} refused"
        ));
        assert!(!error.to_string().contains(CANARY), "{error}");
    }
}

#[cfg(test)]
mod connect_form_tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(DatabaseType::PostgreSQL, "localhost", Some(5432), "postgres", true)]
    #[case(DatabaseType::MySQL, "localhost", Some(3306), "root", true)]
    #[case(DatabaseType::Elasticsearch, "localhost", Some(9200), "elastic", false)]
    #[case(DatabaseType::SQLite, "", None, "", false)]
    fn defaults_are_real_values(
        #[case] engine: DatabaseType,
        #[case] host: &str,
        #[case] port: Option<u16>,
        #[case] user: &str,
        #[case] has_database: bool,
    ) {
        let defaults = form_defaults(&engine);
        assert_eq!(defaults.host, host);
        assert_eq!(defaults.port, port);
        assert_eq!(defaults.user, user);
        assert_eq!(defaults.database_hint.is_some(), has_database);
    }

    #[test]
    fn every_engine_has_a_path_hint_and_aliases() {
        for engine in DatabaseType::supported_types() {
            assert!(!form_defaults(&engine).path_hint.is_empty());
            assert!(!engine_aliases(&engine).is_empty(), "{engine}");
        }
    }

    #[test]
    fn imports_urls_files_and_shorthand() {
        let url = import_target("postgres://app:pw@db:6543/shop?sslmode=require", None, None);
        let Ok(Target::Database(spec)) = url else {
            panic!("expected a database URL");
        };
        assert_eq!((spec.host.as_str(), spec.port), ("db", Some(6543)));
        assert_eq!(spec.option("sslmode"), Some("require"));

        let file = import_target("~/data/events.parquet", Some("/home/me"), None);
        let Ok(Target::Database(spec)) = file else {
            panic!("expected a file");
        };
        assert_eq!(spec.engine, DatabaseType::Parquet);
        assert_eq!(spec.path, "/home/me/data/events.parquet");

        assert!(matches!(
            import_target("app:pw@db/shop", None, None),
            Ok(Target::Database(spec)) if spec.engine == DatabaseType::PostgreSQL
        ));
        assert!(matches!(
            import_target("docker://pg/app", None, None),
            Ok(Target::Docker(_))
        ));
    }

    #[rstest]
    #[case("")]
    #[case("hello")]
    #[case("ftp://example.com")]
    fn refuses_what_is_not_a_connection(#[case] input: &str) {
        assert!(import_target(input, None, None).is_err());
    }

    #[test]
    fn kept_options_name_keys_never_values() {
        assert_eq!(describe_kept_options(&[]), None);
        let one = describe_kept_options(&[("application_name".into(), "secret-value".into())]);
        assert_eq!(
            one.as_deref(),
            Some("1 option kept from the URL (application_name)")
        );
        let many: Vec<(String, String)> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|key| ((*key).to_string(), "v".to_string()))
            .collect();
        assert_eq!(
            describe_kept_options(&many).as_deref(),
            Some("5 options kept from the URL (a, b, c and 2 more)")
        );
    }

    #[test]
    fn docker_urls_keep_their_options_and_tls() {
        let spec = parse_docker_url(
            "docker://app@pg/shop?sslmode=verify-full&ssl=true&application_name=dbcrust",
        )
        .expect("docker url");
        assert_eq!(spec.user, "app");
        assert_eq!(spec.container, "pg");
        assert_eq!(spec.database, "shop");
        assert!(spec.use_tls, "ssl=true is the TLS flag");
        assert_eq!(
            spec.options,
            vec![
                ("application_name".to_string(), "dbcrust".to_string()),
                ("sslmode".to_string(), "verify-full".to_string())
            ]
        );
        // The round trip keeps everything but the order of the query
        let rebuilt = build_docker_url(&spec);
        assert_eq!(parse_docker_url(&rebuilt), Some(spec.clone()));
        assert!(rebuilt.contains("sslmode=verify-full") && rebuilt.contains("ssl=true"));
        // The page's import path finds them too
        assert!(matches!(
            parse_target("docker://app@pg/shop?sslmode=verify-full"),
            Some(Target::Docker(spec)) if spec.options.len() == 1
        ));
        // And the canonical target (what connecting uses) carries them
        let target = spec.to_target();
        assert_eq!(
            target.options.get("sslmode").map(String::as_str),
            Some("verify-full")
        );
        assert!(target.use_tls);
        assert_eq!(DockerSpec::from_target(target), spec);
    }

    #[test]
    fn displayed_docker_urls_hide_secrets_and_mixed_case_schemes_parse() {
        let spec = DockerSpec {
            user: "app".into(),
            password: "hunter2".into(),
            container: "pg".into(),
            options: vec![
                ("sslmode".into(), "require".into()),
                ("password_command".into(), "pass show db".into()),
            ],
            ..Default::default()
        };
        let masked = build_masked_docker_url(&spec);
        assert!(
            !masked.contains("hunter2") && !masked.contains("password_command"),
            "{masked}"
        );
        assert!(masked.contains(PASSWORD_MASK) && masked.contains("sslmode=require"));
        // The connect URL keeps what the connection needs
        assert!(build_docker_url(&spec).contains("password_command"));
        assert!(parse_docker_url("DOCKER://pg/shop").is_some());
    }

    #[test]
    fn tls_is_summarised_per_engine() {
        let mut pg = ConnectionSpec::new(DatabaseType::PostgreSQL);
        assert!(tls_summary(&pg).is_some_and(|text| text.starts_with("Default")));
        pg.set_option("sslmode", Some("verify-full"));
        assert!(tls_summary(&pg).is_some_and(|text| text.contains("host name")));

        let mut my = ConnectionSpec::new(DatabaseType::MySQL);
        assert!(tls_summary(&my).is_some_and(|text| text.starts_with("Off")));
        my.set_tls(true);
        assert!(tls_summary(&my).is_some_and(|text| text.starts_with("On")));

        assert_eq!(
            tls_summary(&ConnectionSpec::new(DatabaseType::SQLite)),
            None
        );
    }
}
