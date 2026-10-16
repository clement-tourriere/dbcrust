use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

/// Replacement for a secret in redacted URLs and text.
pub const REDACTED: &str = "[REDACTED]";
/// Replacement for a secret value shown through `Debug`.
const MASK: &str = "***";

/// Substrings that mark an option/query key as holding a secret. Compared
/// against the lowercased key with `_`, `-` and `.` removed.
const SECRET_KEY_WORDS: [&str; 10] = [
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "accesskey",
    "credential",
    "authorization",
    // Mongo: carries e.g. AWS_SESSION_TOKEN:...
    "authmechanismproperties",
];
/// Keys that are secret only as a whole word (`authSource` is not).
const SECRET_KEY_EXACT: [&str; 2] = ["pass", "auth"];

/// True for option / query-string keys that hold a secret (`password`,
/// `sslpassword`, `api_key`, `token`, `password_command`…), case-insensitive.
pub fn is_secret_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect();
    SECRET_KEY_EXACT.contains(&normalized.as_str())
        || SECRET_KEY_WORDS
            .iter()
            .any(|word| normalized.contains(word))
}

/// Secret keys that must never reach storage. `password_command` is a
/// documented reference to a secret, persisted on purpose.
pub fn is_secret_option_key(key: &str) -> bool {
    is_secret_key(key) && !crate::options::is_password_command_option_key(key)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Replace secrets with [`REDACTED`] (logs, errors, display).
    Mask,
    /// Remove secrets entirely; password commands are kept (storage).
    Strip,
}

fn percent_decoded(text: &str) -> String {
    percent_encoding::percent_decode_str(text)
        .decode_utf8_lossy()
        .into_owned()
}

fn is_scheme(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

fn secret_pair(key: &str, mode: Mode) -> bool {
    let key = percent_decoded(key);
    match mode {
        Mode::Mask => is_secret_key(&key),
        Mode::Strip => is_secret_option_key(&key),
    }
}

/// Scrub a `k=v&k=v` (or `;`-separated) query string / fragment.
fn scrub_pairs(pairs: &str, mode: Mode) -> String {
    if !pairs.contains('=') {
        return pairs.to_string();
    }
    let mut out = String::with_capacity(pairs.len());
    let mut kept_any = false;
    let mut separator = "";
    let mut rest = pairs;
    loop {
        let end = rest.find(['&', ';']).unwrap_or(rest.len());
        let pair = &rest[..end];
        let next_separator = rest.get(end..end + 1).unwrap_or("");
        match pair.split_once('=') {
            Some((key, value)) if secret_pair(key, mode) => match mode {
                Mode::Mask if !value.is_empty() => {
                    out.push_str(separator);
                    out.push_str(&format!("{key}={REDACTED}"));
                    kept_any = true;
                }
                Mode::Mask => {
                    out.push_str(separator);
                    out.push_str(pair);
                    kept_any = true;
                }
                Mode::Strip => {}
            },
            _ => {
                out.push_str(separator);
                out.push_str(pair);
                kept_any = true;
            }
        }
        if next_separator.is_empty() {
            break;
        }
        // Stripping re-joins what is left with `&`
        separator = match mode {
            Mode::Mask => next_separator,
            Mode::Strip if kept_any => "&",
            Mode::Strip => "",
        };
        rest = &rest[end + 1..];
    }
    out
}

fn scrub_url(url: &str, mode: Mode) -> String {
    // Our special Docker format: `<url> # Docker: <container>`
    if let Some(comment_pos) = url.find(" # Docker: ") {
        let (main_url, comment) = url.split_at(comment_pos);
        return format!("{}{comment}", scrub_url(main_url, mode));
    }

    let (prefix, rest, has_scheme) = match url.find("://") {
        Some(at) if is_scheme(&url[..at]) => (&url[..at + 3], &url[at + 3..], true),
        _ => ("", url, false),
    };

    let (mut head, mut tail) = match rest.find(['?', '#']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    // File URLs (`sqlite:///a:b@c.db`) have a path, not an authority
    let file_like = rest.starts_with('/')
        || [
            "sqlite", "file", "csv", "parquet", "json", "jsonl", "ndjson",
        ]
        .iter()
        .any(|scheme| prefix.trim_end_matches("://").eq_ignore_ascii_case(scheme));
    let mut at = head.rfind('@').filter(|_| !file_like);
    if at.is_none() && has_scheme && !file_like {
        // `scheme://user:p?ss@host`: a `?`/`#` inside the password moved the
        // cut too early. A non-numeric "port" in the authority gives it away.
        let authority = head.split('/').next().unwrap_or("");
        let looks_broken = authority.rsplit_once(':').is_some_and(|(_, port)| {
            !port.chars().all(|c| c.is_ascii_digit()) && !port.contains(']')
        });
        if looks_broken && let Some(last_at) = rest.rfind('@') {
            head = rest;
            tail = "";
            at = Some(last_at);
        }
    }

    let mut out = String::with_capacity(url.len());
    out.push_str(prefix);
    match at {
        Some(at) if has_scheme || !head[..at].chars().any(char::is_whitespace) => {
            let (userinfo, host) = head.split_at(at);
            match userinfo.split_once(':') {
                Some((user, password)) if !password.is_empty() => {
                    out.push_str(user);
                    if mode == Mode::Mask {
                        out.push(':');
                        out.push_str(REDACTED);
                    }
                }
                _ => out.push_str(userinfo),
            }
            out.push_str(host);
        }
        _ => out.push_str(head),
    }

    if let Some(query) = tail.strip_prefix('?') {
        let (query, fragment) = match query.split_once('#') {
            Some((query, fragment)) => (query, Some(fragment)),
            None => (query, None),
        };
        let query = scrub_pairs(query, mode);
        if !query.is_empty() {
            out.push('?');
            out.push_str(&query);
        }
        if let Some(fragment) = fragment {
            out.push('#');
            out.push_str(&scrub_pairs(fragment, mode));
        }
    } else if let Some(fragment) = tail.strip_prefix('#') {
        out.push('#');
        out.push_str(&scrub_pairs(fragment, mode));
    }

    if has_scheme {
        out
    } else {
        // libpq/ADO style `host=h password=secret` strings
        scrub_key_values(&out)
    }
}

static KEY_VALUE_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(^|[?&;,\s#'"(])((?:[\w.\-]*(?:password|passwd|pwd|secret|token|api[_\-.]?key|access[_\-.]?key|credential|authorization|authmechanismproperties)[\w.\-]*)|pass|auth)(\s*=\s*)('[^']*'|"[^"]*"|[^\s&;#]+)"#,
    )
    .expect("valid secret key=value regex")
});
static JSON_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)("[\w.\-]*(?:password|passwd|secret|token|api[_\-.]?key|credential)[\w.\-]*"\s*:\s*)"(?:[^"\\]|\\.)*""#,
    )
    .expect("valid secret json regex")
});
static URL_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)[a-z][a-z0-9+.\-]*://[^\s"'<>`]+"#).expect("valid url regex")
});
static USERINFO_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Za-z_][\w.\-]*):([^\s@/:'\x22]+)@([\w.\-]+|\[[0-9A-Fa-f:.]+\])")
        .expect("valid userinfo regex")
});
/// Well-known credential shapes: OpenAI-style keys, GitHub tokens, Vault
/// tokens and JWTs.
static TOKEN_SHAPES_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_\-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|hv[sbr]\.[A-Za-z0-9_\-]{16,}|eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,})",
    )
    .expect("valid token shape regex")
});
static BEARER_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=\-]{8,}").expect("valid bearer regex")
});

fn scrub_key_values(text: &str) -> String {
    KEY_VALUE_SECRET
        .replace_all(text, format!("${{1}}${{2}}${{3}}{REDACTED}").as_str())
        .into_owned()
}

/// Redact a connection URL / DSN for logs, errors and display: the userinfo
/// password and the values of secret-looking query keys (`password`, `token`,
/// `api_key`, `password_command`…) become `[REDACTED]`; everything else —
/// including non-secret options — is kept. Works on any scheme, IPv6 hosts,
/// percent-encoded userinfo and malformed input (string based, never
/// re-serialised), and is idempotent.
pub fn redact_url(url: &str) -> String {
    scrub_url(url, Mode::Mask)
}

/// Remove every credential from a URL before it is stored: userinfo
/// passwords and secret-looking query pairs are dropped (`u:p@h` → `u@h`).
/// `password_command` options are kept — they reference a secret, they are
/// not one.
pub fn strip_url_secrets(url: &str) -> String {
    scrub_url(url, Mode::Strip)
}

/// Redact URL-shaped substrings, `key=value` secrets, JSON secret fields,
/// `user:pass@host` pairs and bearer tokens inside arbitrary text (driver and
/// HTTP error messages).
pub fn redact_text(text: &str) -> String {
    let text = URL_IN_TEXT.replace_all(text, |caps: &regex::Captures| redact_url(&caps[0]));
    let text = scrub_key_values(&text);
    let text = JSON_SECRET.replace_all(&text, format!("${{1}}\"{REDACTED}\"").as_str());
    let text = USERINFO_IN_TEXT.replace_all(&text, format!("${{1}}:{REDACTED}@${{3}}").as_str());
    let text = BEARER_IN_TEXT.replace_all(&text, format!("${{1}} {REDACTED}").as_str());
    TOKEN_SHAPES_IN_TEXT
        .replace_all(&text, REDACTED)
        .into_owned()
}

/// `redact_text` for any displayable error.
pub fn redact_error(error: &dyn std::fmt::Display) -> String {
    redact_text(&error.to_string())
}

/// A password for `Debug` output: `Some("***")` or `None`.
pub fn mask_secret(secret: &Option<String>) -> Option<&'static str> {
    secret.as_ref().map(|_| MASK)
}

/// Options for `Debug` output: secret values masked, URL-shaped values
/// scrubbed, sorted for stable output.
pub fn redact_options<'a>(
    options: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> BTreeMap<&'a str, String> {
    options
        .into_iter()
        .map(|(key, value)| {
            let value = if is_secret_key(key) {
                MASK.to_string()
            } else {
                redact_text(value)
            };
            (key.as_str(), value)
        })
        .collect()
}

/// Drop secret options and scrub URL-shaped values before storage.
pub fn scrub_options_for_storage(options: &mut HashMap<String, String>) {
    options.retain(|key, _| !is_secret_option_key(key));
    for value in options.values_mut() {
        if value.contains("://") {
            *value = strip_url_secrets(value);
        }
    }
}

/// Sanitize a connection URL by removing the password (and other secrets,
/// see [`redact_url`]).
pub fn sanitize_connection_url(url: &str) -> String {
    redact_url(url)
}

/// Sanitize an SSH tunnel string by removing the password
pub fn sanitize_ssh_tunnel_string(tunnel: &str) -> String {
    // Format: [user[:password]@]ssh_host[:ssh_port]
    if tunnel.contains('@') && tunnel.contains(':') {
        let parts: Vec<&str> = tunnel.split('@').collect();
        if parts.len() >= 2 {
            let user_pass = parts[0];
            let rest = parts[1..].join("@");
            if user_pass.contains(':') {
                let user_parts: Vec<&str> = user_pass.split(':').collect();
                if user_parts.len() >= 2 {
                    return format!("{}:[REDACTED]@{}", user_parts[0], rest);
                }
            }
        }
    }
    tunnel.to_string()
}

/// Sanitize SSH command strings that might contain passwords or sensitive information
#[allow(dead_code)]
pub fn sanitize_ssh_command(command: &str) -> String {
    // Look for patterns like -o PasswordAuthentication=yes password or similar
    // This is a basic implementation - SSH commands can be complex
    let mut sanitized = command.to_string();

    // Replace any obvious password patterns
    if sanitized.contains("password") || sanitized.contains("Password") {
        // This is a simple approach - in practice SSH passwords are usually not in command line
        // but could be in environment variables or other mechanisms
        sanitized = "[SSH_COMMAND_WITH_POTENTIAL_CREDENTIALS_REDACTED]".to_string();
    }

    sanitized
}

/// General sanitization function for any text that might contain passwords in various formats.
/// This uses regex patterns to match common password patterns and redact them.
#[allow(dead_code)]
pub fn sanitize_text_for_logging(text: &str) -> String {
    let mut sanitized = text.to_string();

    // Look for PostgreSQL connection URLs first (more specific)
    if sanitized.contains("postgres://") || sanitized.contains("postgresql://") {
        let re = Regex::new(r"postgres(?:ql)?://([^:]+):([^@]+)@([^/\s]+)").unwrap();
        sanitized = re
            .replace_all(&sanitized, "postgres://$1:[REDACTED]@$3")
            .to_string();
    } else {
        // Only look for simple connection strings if no PostgreSQL URL was found
        let re = Regex::new(r"([a-zA-Z0-9_]+):([^@\s]+)@([a-zA-Z0-9.-]+)").unwrap();
        sanitized = re.replace_all(&sanitized, "$1:[REDACTED]@$3").to_string();
    }

    // Look for password= patterns (this can run regardless)
    let re = Regex::new(r"password=([^\s&]+)").unwrap();
    sanitized = re
        .replace_all(&sanitized, "password=[REDACTED]")
        .to_string();

    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("mysql://u:secret@h:3306/db", "mysql://u:[REDACTED]@h:3306/db")]
    #[case(
        "clickhouse://default:secret@h:8123/db",
        "clickhouse://default:[REDACTED]@h:8123/db"
    )]
    #[case("mongodb://u:secret@h:27017/db", "mongodb://u:[REDACTED]@h:27017/db")]
    #[case("docker://u:secret@pg/db", "docker://u:[REDACTED]@pg/db")]
    #[case("mysql://u@h:3306/db", "mysql://u@h:3306/db")]
    #[case("u:secret@h:5432/db", "u:[REDACTED]@h:5432/db")]
    // Unparsable (bad port): still redacted
    #[case("mysql://u:secret@host:bad/db", "mysql://u:[REDACTED]@host:bad/db")]
    #[case("mysql://u:p@ss:w0rd@host:bad/db", "mysql://u:[REDACTED]@host:bad/db")]
    fn test_sanitize_keeps_the_scheme_of_every_engine(#[case] url: &str, #[case] expected: &str) {
        assert_eq!(sanitize_connection_url(url), expected);
    }

    #[test]
    fn test_sanitize_docker_connection_url() {
        // Test Docker URL with comment and password
        let docker_url = "postgres://user:password@host:5432/db # Docker: container-name";
        let sanitized = sanitize_connection_url(docker_url);
        assert_eq!(
            sanitized,
            "postgres://user:[REDACTED]@host:5432/db # Docker: container-name"
        );

        // Test Docker URL without password
        let docker_url_no_pass = "postgres://user@host:5432/db # Docker: container-name";
        let sanitized_no_pass = sanitize_connection_url(docker_url_no_pass);
        assert_eq!(
            sanitized_no_pass,
            "postgres://user@host:5432/db # Docker: container-name"
        );
    }

    #[test]
    fn test_sanitize_connection_url_with_password() {
        let url = "postgres://user:secret123@localhost:5432/mydb";
        let sanitized = sanitize_connection_url(url);
        // Should now contain [REDACTED] without URL encoding
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("secret123"));
        assert!(!sanitized.contains("%5BREDACTED%5D")); // Should not be URL encoded
        assert!(sanitized.contains("user"));
        assert!(sanitized.contains("localhost"));
    }

    #[test]
    fn test_sanitize_connection_url_without_password() {
        let url = "postgres://user@localhost:5432/mydb";
        let sanitized = sanitize_connection_url(url);
        assert_eq!(sanitized, url);
    }

    #[test]
    fn test_sanitize_connection_url_simple_format() {
        let url = "user:password@host:5432/db";
        let sanitized = sanitize_connection_url(url);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("password"));
        assert!(sanitized.contains("user"));
    }

    #[test]
    fn test_sanitize_ssh_tunnel_string_with_password() {
        let tunnel = "user:secret@jumphost.com:22";
        let sanitized = sanitize_ssh_tunnel_string(tunnel);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("user"));
        assert!(sanitized.contains("jumphost.com"));
    }

    #[test]
    fn test_sanitize_ssh_tunnel_string_without_password() {
        let tunnel = "user@jumphost.com:22";
        let sanitized = sanitize_ssh_tunnel_string(tunnel);
        assert_eq!(sanitized, tunnel);
    }

    #[test]
    fn test_sanitize_ssh_command() {
        let command = "ssh -L 5432:localhost:5432 user@host";
        let sanitized = sanitize_ssh_command(command);
        assert_eq!(sanitized, command); // No password in this command

        let command_with_password = "ssh -o PasswordAuthentication=yes user@host";
        let sanitized_with_password = sanitize_ssh_command(command_with_password);
        assert!(
            sanitized_with_password.contains("[SSH_COMMAND_WITH_POTENTIAL_CREDENTIALS_REDACTED]")
        );
    }

    #[test]
    fn test_sanitize_text_for_logging() {
        // Test PostgreSQL URL sanitization
        let text = "Error connecting to postgres://user:secret@localhost:5432/db";
        let sanitized = sanitize_text_for_logging(text);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("secret"));
        assert!(sanitized.contains("user"));

        // Test simple connection string sanitization
        let text = "Connection failed for user:password@host.com";
        let sanitized = sanitize_text_for_logging(text);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("password"));
        assert!(sanitized.contains("user"));

        // Test password= pattern sanitization
        let text = "Connection string: host=localhost password=secret123 user=test";
        let sanitized = sanitize_text_for_logging(text);
        assert!(sanitized.contains("password=[REDACTED]"));
        assert!(!sanitized.contains("secret123"));
        assert!(sanitized.contains("user=test"));

        // Test text without passwords
        let text = "Normal log message without sensitive data";
        let sanitized = sanitize_text_for_logging(text);
        assert_eq!(sanitized, text);
    }

    const CANARY: &str = "Cnry-7f3a9";

    #[rstest]
    // userinfo, every scheme
    #[case("postgres://u:Cnry-7f3a9@h:5432/db")]
    #[case("postgresql://u:Cnry-7f3a9@h/db?sslmode=require")]
    #[case("mysql://u:Cnry-7f3a9@h:3306/db")]
    #[case("mongodb://u:Cnry-7f3a9@h1:27017,h2:27017/db?replicaSet=rs0")]
    #[case("mongodb+srv://u:Cnry-7f3a9@cluster0.example.net/db?retryWrites=true")]
    #[case("clickhouse://default:Cnry-7f3a9@h:8123/db")]
    #[case("docker://u:Cnry-7f3a9@container/db")]
    #[case("docker://u:Cnry-7f3a9@container/db?ssl=true")]
    #[case("vault://role:Cnry-7f3a9@database/mydb")]
    #[case("white-dragon://u:Cnry-7f3a9@h:7700")]
    // percent-encoded and unusual passwords
    #[case("postgres://u:Cnry%2D7f3a9@h/db")]
    #[case("postgres://u:Cnry-7f3a9%40x@h/db")]
    #[case("postgres://u:Cnry-7f3a9@x@h/db")]
    #[case("postgres://u:Cnry-7f3a9/x@h/db")]
    #[case("postgres://u:Cnry-7f3a9?x@h/db")]
    // IPv6
    #[case("postgres://u:Cnry-7f3a9@[::1]:5432/db")]
    #[case("postgres://u:Cnry-7f3a9@[2001:db8::1]/db")]
    // query-param secrets, any case / key
    #[case("postgres://u@h/db?password=Cnry-7f3a9")]
    #[case("postgres://u@h/db?PASSWORD=Cnry-7f3a9")]
    #[case("postgres://u@h/db?sslmode=require&pass=Cnry-7f3a9&application_name=app")]
    #[case("postgres://u@h/db?pwd=Cnry-7f3a9")]
    #[case("postgres://u@h/db?passwd=Cnry-7f3a9")]
    #[case("clickhouse://u@h/db?token=Cnry-7f3a9")]
    #[case("clickhouse://u@h/db?Secret=Cnry-7f3a9")]
    #[case("elasticsearch://h:9200?api_key=Cnry-7f3a9")]
    #[case("elasticsearch://h:9200?apikey=Cnry-7f3a9")]
    #[case("elasticsearch://h:9200?API-KEY=Cnry-7f3a9")]
    #[case("s3://h/b?access_key=Cnry-7f3a9&secret_key=Cnry-7f3a9")]
    #[case("postgres://u@h/db?sslpassword=Cnry-7f3a9")]
    #[case("postgres://u@h/db?auth=Cnry-7f3a9")]
    #[case("postgres://u@h/db?Authorization=Cnry-7f3a9")]
    #[case("postgres://u@h/db?credentials=Cnry-7f3a9")]
    #[case("postgres://u@h/db?password_command=Cnry-7f3a9")]
    #[case("postgres://u@h/db?password_cmd=Cnry-7f3a9")]
    #[case("postgres://u@h/db?pass%77ord=x&password=Cnry-7f3a9")]
    #[case("postgres://u@h/db?password=Cnry-7f3a9#frag")]
    #[case("postgres://u@h/db#password=Cnry-7f3a9")]
    #[case("postgres://u@h/db?a=1;password=Cnry-7f3a9")]
    #[case("mongodb://u@h/db?authMechanismProperties=AWS_SESSION_TOKEN:Cnry-7f3a9")]
    // both userinfo and query
    #[case("postgres://u:Cnry-7f3a9@h/db?password=Cnry-7f3a9&token=Cnry-7f3a9")]
    // schemeless / DSN / malformed
    #[case("u:Cnry-7f3a9@h:5432/db")]
    #[case("host=h user=u password=Cnry-7f3a9 dbname=db")]
    #[case("host=h password='Cnry-7f3a9' dbname=db")]
    #[case("Server=h;User Id=u;Password=Cnry-7f3a9;Database=db")]
    #[case("postgres://u:Cnry-7f3a9@h:bad/db")]
    #[case("postgres://u:Cnry-7f3a9@")]
    #[case("postgres://u:Cnry-7f3a9@h:5432/db # Docker: pg")]
    fn test_redact_url_never_leaks_the_canary(#[case] url: &str) {
        for redacted in [redact_url(url), sanitize_connection_url(url)] {
            assert!(!redacted.contains(CANARY), "{url} -> {redacted}");
            assert!(
                !redacted.contains(&CANARY.replace('-', "%2D")),
                "{url} -> {redacted}"
            );
            assert_eq!(redact_url(&redacted), redacted, "not idempotent: {url}");
        }
        // The text variant must agree
        let text = redact_text(&format!("error connecting to {url}: refused"));
        assert!(!text.contains(CANARY), "{url} -> {text}");
    }

    #[rstest]
    #[case(
        "postgres://u:Cnry-7f3a9@h:5432/db?sslmode=require&application_name=my%20app",
        "postgres://u:[REDACTED]@h:5432/db?sslmode=require&application_name=my%20app"
    )]
    #[case(
        "postgres://u@h/db?sslmode=require&password=Cnry-7f3a9&connect_timeout=5",
        "postgres://u@h/db?sslmode=require&password=[REDACTED]&connect_timeout=5"
    )]
    #[case(
        "mongodb+srv://u:Cnry-7f3a9@c.example.net/db?authSource=admin&retryWrites=true",
        "mongodb+srv://u:[REDACTED]@c.example.net/db?authSource=admin&retryWrites=true"
    )]
    #[case(
        "postgres://u:Cnry-7f3a9@[::1]:5432/db",
        "postgres://u:[REDACTED]@[::1]:5432/db"
    )]
    #[case("docker://u:Cnry-7f3a9@pg/db", "docker://u:[REDACTED]@pg/db")]
    #[case("vault://role@database/mydb", "vault://role@database/mydb")]
    #[case("postgres://u@h/db#section", "postgres://u@h/db#section")]
    #[case("postgres://u@h/db?email=a@b.c", "postgres://u@h/db?email=a@b.c")]
    #[case("postgres://u@h:5432/db", "postgres://u@h:5432/db")]
    #[case("sqlite:///tmp/a.db", "sqlite:///tmp/a.db")]
    #[case("/tmp/a.db", "/tmp/a.db")]
    #[case("host=h dbname=db", "host=h dbname=db")]
    #[case("postgres://u@h/db?password=", "postgres://u@h/db?password=")]
    #[case("postgres://u:@h/db", "postgres://u:@h/db")]
    #[case(
        "host=h user=u password=Cnry-7f3a9 dbname=db",
        "host=h user=u password=[REDACTED] dbname=db"
    )]
    #[case(
        "postgres://u:Cnry-7f3a9@h:5432/db # Docker: pg",
        "postgres://u:[REDACTED]@h:5432/db # Docker: pg"
    )]
    fn test_redact_url_keeps_everything_else(#[case] url: &str, #[case] expected: &str) {
        assert_eq!(redact_url(url), expected);
    }

    #[rstest]
    #[case(
        "postgres://u:Cnry-7f3a9@h/db?sslmode=require&password=Cnry-7f3a9",
        "postgres://u@h/db?sslmode=require"
    )]
    #[case("postgres://u@h/db?token=Cnry-7f3a9", "postgres://u@h/db")]
    #[case(
        "clickhouse://u@h/db?secure=true&password_command=echo%20hi",
        "clickhouse://u@h/db?secure=true&password_command=echo%20hi"
    )]
    #[case("postgres://u@h/db", "postgres://u@h/db")]
    fn test_strip_url_secrets(#[case] url: &str, #[case] expected: &str) {
        assert_eq!(strip_url_secrets(url), expected);
    }

    #[rstest]
    #[case("password", true)]
    #[case("SSLPassword", true)]
    #[case("pass", true)]
    #[case("api-key", true)]
    #[case("Secret_Key", true)]
    #[case("authorization", true)]
    #[case("password_command", true)]
    #[case("sslmode", false)]
    #[case("application_name", false)]
    #[case("authSource", false)]
    #[case("passenger", false)]
    fn test_is_secret_key(#[case] key: &str, #[case] expected: bool) {
        assert_eq!(is_secret_key(key), expected);
    }

    #[test]
    fn test_redact_text_scrubs_urls_inside_error_messages() {
        // Built at runtime so secret scanners don't flag the test source
        let bearer = "tok".repeat(6);
        let message = format!(
            "error connecting to \"postgres://u:{CANARY}@db.internal:5432/app?sslmode=require\" \
             (caused by: password={CANARY}; also user:{CANARY}@host.example; \
             Authorization: Bearer {bearer}; {{\"password\": \"{CANARY}\"}})"
        );
        let redacted = redact_text(&message);
        assert!(!redacted.contains(CANARY), "{redacted}");
        assert!(!redacted.contains(&bearer), "{redacted}");
        assert!(redacted.contains("db.internal:5432/app?sslmode=require"));
        assert_eq!(
            redact_text("plain failure: timeout"),
            "plain failure: timeout"
        );
        assert_eq!(redact_text(&redacted), redacted);
    }

    #[test]
    fn test_redact_options_masks_secret_values() {
        let options: HashMap<String, String> = [
            ("sslmode", "require"),
            ("password", CANARY),
            ("endpoint", &format!("https://u:{CANARY}@h/x")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let shown = format!("{:?}", redact_options(&options));
        assert!(!shown.contains(CANARY), "{shown}");
        assert!(shown.contains("require"));
    }
}
