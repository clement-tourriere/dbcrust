//! Which Docker daemon to talk to: the same choice the `docker` CLI makes.
//!
//! Precedence (an explicit selection in the application comes first, via
//! [`DockerEndpoint::explicit`]): `DOCKER_CONTEXT`, `DOCKER_HOST`, the current
//! context from `~/.docker/config.json`, then the platform default. A context
//! or host DBCrust cannot speak (`ssh://`, TLS) is reported as an error rather
//! than silently talking to another daemon.

use crate::docker::DockerError;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where the endpoint came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointSource {
    /// Chosen explicitly by the application (a socket path, a setting).
    Explicit,
    /// The daemon a live connection or a discovery result came from
    /// ([`DockerEndpoint::pinned_value`]), so follow-up connections reach the
    /// same daemon even if the current docker context changed meanwhile.
    Pinned,
    /// A docker context (`DOCKER_CONTEXT` or `currentContext`).
    Context(String),
    /// `DOCKER_HOST`.
    DockerHostEnv,
    /// Nothing configured: bollard's platform default.
    Default,
}

/// The daemon DBCrust talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerEndpoint {
    /// `unix://…`, `npipe://…` or `tcp://…`; `None` means bollard's defaults.
    pub host: Option<String>,
    pub source: EndpointSource,
}

/// Raw inputs of the resolution (explicit so it can be tested).
#[derive(Debug, Clone, Default)]
pub struct EndpointInputs {
    pub docker_host: Option<String>,
    pub docker_context: Option<String>,
    /// `DOCKER_TLS_VERIFY` is set: a `tcp://` host is expected to be TLS.
    pub tls_verify: bool,
    /// `$DOCKER_CONFIG` or `~/.docker`.
    pub config_dir: Option<PathBuf>,
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn endpoint_error(message: String) -> DockerError {
    DockerError::Endpoint(message)
}

impl DockerEndpoint {
    /// Resolve from the process environment and `~/.docker`.
    pub fn from_environment() -> Result<Self, DockerError> {
        Self::resolve(&EndpointInputs {
            docker_host: non_empty_env("DOCKER_HOST"),
            docker_context: non_empty_env("DOCKER_CONTEXT"),
            tls_verify: non_empty_env("DOCKER_TLS_VERIFY").is_some(),
            config_dir: non_empty_env("DOCKER_CONFIG")
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|home| home.join(".docker"))),
        })
    }

    /// `DOCKER_CONTEXT` → `DOCKER_HOST` → current context → default.
    pub fn resolve(inputs: &EndpointInputs) -> Result<Self, DockerError> {
        let config_dir = inputs.config_dir.as_deref();
        let by_env = || -> Result<Option<Self>, DockerError> {
            inputs
                .docker_host
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(|host| {
                    check_host(host, inputs.tls_verify, "DOCKER_HOST")?;
                    Ok(Self {
                        host: Some(host.to_string()),
                        source: EndpointSource::DockerHostEnv,
                    })
                })
                .transpose()
        };

        // DOCKER_CONTEXT=default means "no context": fall through to
        // DOCKER_HOST / the default socket, as the docker CLI does.
        if let Some(context) = inputs
            .docker_context
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            if context == "default" {
                return Ok(by_env()?.unwrap_or(Self::default_endpoint()));
            }
            return Self::from_context(config_dir, context, inputs.tls_verify);
        }
        if let Some(endpoint) = by_env()? {
            return Ok(endpoint);
        }
        if let Some(context) = config_dir
            .and_then(current_context)
            .filter(|c| c != "default")
        {
            return Self::from_context(config_dir, &context, inputs.tls_verify);
        }
        Ok(Self::default_endpoint())
    }

    fn default_endpoint() -> Self {
        Self {
            host: None,
            source: EndpointSource::Default,
        }
    }

    fn from_context(
        config_dir: Option<&Path>,
        context: &str,
        tls_verify: bool,
    ) -> Result<Self, DockerError> {
        let config_dir = config_dir.ok_or_else(|| {
            endpoint_error(format!(
                "Docker context '{context}' is selected but there is no Docker config directory to read it from."
            ))
        })?;
        let host = context_host(config_dir, context).ok_or_else(|| {
            endpoint_error(format!(
                "Docker context '{context}' is selected but it isn't defined in {}. \
                 Pick another with `docker context use`.",
                config_dir.join("contexts/meta").display()
            ))
        })?;
        check_host(&host, tls_verify, &format!("Docker context '{context}'"))?;
        if has_tls_material(config_dir, context) {
            return Err(endpoint_error(format!(
                "Docker context '{context}' uses TLS client certificates, which are not supported by DBCrust yet. \
                 Pick another context with `docker context use` or set DOCKER_HOST to a local socket."
            )));
        }
        Ok(Self {
            host: Some(host),
            source: EndpointSource::Context(context.to_string()),
        })
    }

    /// An endpoint chosen by the application, e.g. a bare socket path or
    /// `unix://…`.
    pub fn explicit(host: &str) -> Result<Self, DockerError> {
        let host = host.trim();
        let host = if host.contains("://") {
            host.to_string()
        } else {
            format!("unix://{host}")
        };
        check_host(&host, false, "The Docker endpoint")?;
        Ok(Self {
            host: Some(host),
            source: EndpointSource::Explicit,
        })
    }

    /// A stable identity of this daemon, to carry along with a connection or
    /// a discovery result and hand to [`DockerEndpoint::from_pinned`] later.
    /// Always a concrete endpoint (the platform default is spelled out).
    pub fn pinned_value(&self) -> String {
        self.host
            .clone()
            .unwrap_or_else(|| DEFAULT_DOCKER_HOST.to_string())
    }

    /// The daemon identified by [`DockerEndpoint::pinned_value`]. No
    /// environment or context is consulted.
    pub fn from_pinned(value: &str) -> Result<Self, DockerError> {
        let endpoint = Self::explicit(value)?;
        Ok(Self {
            source: EndpointSource::Pinned,
            ..endpoint
        })
    }

    /// Whether the daemon runs on this machine (so loopback publishing and,
    /// on Linux, container addresses apply).
    pub fn is_local(&self) -> bool {
        self.remote_host().is_none()
    }

    /// Host name of a daemon reached over TCP that is not this machine.
    pub fn remote_host(&self) -> Option<String> {
        let host = self.host.as_deref()?;
        let rest = host
            .strip_prefix("tcp://")
            .or_else(|| host.strip_prefix("http://"))?;
        let authority = rest.split('/').next().unwrap_or(rest);
        let name = if let Some(stripped) = authority.strip_prefix('[') {
            stripped.split(']').next().unwrap_or(stripped)
        } else {
            authority.rsplit_once(':').map_or(authority, |(h, _)| h)
        };
        let is_loopback =
            name.eq_ignore_ascii_case("localhost") || name == "::1" || name.starts_with("127.");
        (!is_loopback && !name.is_empty()).then(|| name.to_string())
    }

    /// OrbStack's daemon socket lives under `~/.orbstack/`.
    pub fn mentions_orbstack(&self) -> bool {
        self.host
            .as_deref()
            .is_some_and(|host| host.contains("/.orbstack/"))
    }

    /// The docker context name, when the endpoint came from one.
    pub fn context_name(&self) -> Option<&str> {
        match &self.source {
            EndpointSource::Context(name) => Some(name),
            _ => None,
        }
    }

    /// Short label for UIs and messages: the context name, `DOCKER_HOST`, or
    /// the default socket.
    pub fn label(&self) -> String {
        match &self.source {
            EndpointSource::Context(name) => name.clone(),
            EndpointSource::DockerHostEnv => "DOCKER_HOST".to_string(),
            EndpointSource::Explicit | EndpointSource::Pinned | EndpointSource::Default => self
                .host
                .clone()
                .unwrap_or_else(|| "the default Docker socket".to_string()),
        }
    }

    /// Longer description for diagnostics, e.g.
    /// `context 'orbstack' (unix:///Users/me/.orbstack/run/docker.sock)`.
    pub fn describe(&self) -> String {
        let host = self.host.as_deref().unwrap_or("the default Docker socket");
        match &self.source {
            EndpointSource::DockerHostEnv => format!("DOCKER_HOST ({host})"),
            EndpointSource::Context(name) => format!("context '{name}' ({host})"),
            EndpointSource::Pinned => format!("the daemon at {host}"),
            EndpointSource::Explicit | EndpointSource::Default => host.to_string(),
        }
    }
}

/// Where bollard looks without any configuration.
#[cfg(unix)]
const DEFAULT_DOCKER_HOST: &str = "unix:///var/run/docker.sock";
#[cfg(windows)]
const DEFAULT_DOCKER_HOST: &str = "npipe:////./pipe/docker_engine";

/// Reject transports bollard (as built here) cannot speak.
fn check_host(host: &str, tls_verify: bool, what: &str) -> Result<(), DockerError> {
    if host.starts_with("unix://") || host.starts_with("npipe://") {
        return Ok(());
    }
    if host.starts_with("tcp://") || host.starts_with("http://") {
        if tls_verify {
            return Err(endpoint_error(format!(
                "{what} points at {host} with DOCKER_TLS_VERIFY set; TLS-secured Docker endpoints are not supported by DBCrust yet."
            )));
        }
        return Ok(());
    }
    let scheme = host.split("://").next().unwrap_or(host);
    Err(endpoint_error(format!(
        "{what} uses {host}: {scheme}:// is not supported by DBCrust \
         (supported: unix sockets, named pipes and plain tcp). \
         Select another context with `docker context use` or set DOCKER_HOST."
    )))
}

fn current_context(config_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_dir.join("config.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("currentContext")?
        .as_str()
        .map(str::to_string)
        .filter(|c| !c.trim().is_empty())
}

/// `contexts/meta/<sha256(name)>` is where the docker CLI stores a context.
pub(crate) fn context_dir_name(context: &str) -> String {
    hex::encode(Sha256::digest(context.as_bytes()))
}

fn context_host(config_dir: &Path, context: &str) -> Option<String> {
    let meta = config_dir
        .join("contexts/meta")
        .join(context_dir_name(context))
        .join("meta.json");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(meta).ok()?).ok()?;
    json.get("Endpoints")?
        .get("docker")?
        .get("Host")?
        .as_str()
        .map(str::to_string)
        .filter(|h| !h.is_empty())
}

/// Contexts with client certificates need TLS, which this build lacks.
fn has_tls_material(config_dir: &Path, context: &str) -> bool {
    config_dir
        .join("contexts/tls")
        .join(context_dir_name(context))
        .join("docker")
        .is_dir()
}

/// Whether `docker info` describes OrbStack's daemon.
pub fn is_orbstack_daemon(operating_system: Option<&str>, name: Option<&str>) -> bool {
    [operating_system, name]
        .into_iter()
        .flatten()
        .any(|text| text.to_ascii_lowercase().contains("orbstack"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Write a docker config dir with the given contexts and `currentContext`.
    fn docker_config(current: Option<&str>, contexts: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(current) = current {
            std::fs::write(
                dir.path().join("config.json"),
                format!(r#"{{"auths":{{}},"currentContext":"{current}"}}"#),
            )
            .unwrap();
        }
        for (name, host) in contexts {
            let meta = dir
                .path()
                .join("contexts/meta")
                .join(context_dir_name(name));
            std::fs::create_dir_all(&meta).unwrap();
            std::fs::write(
                meta.join("meta.json"),
                format!(
                    r#"{{"Name":"{name}","Metadata":{{}},"Endpoints":{{"docker":{{"Host":"{host}","SkipTLSVerify":false}}}}}}"#
                ),
            )
            .unwrap();
        }
        dir
    }

    fn resolve(
        docker_host: Option<&str>,
        docker_context: Option<&str>,
        dir: &tempfile::TempDir,
    ) -> Result<DockerEndpoint, DockerError> {
        DockerEndpoint::resolve(&EndpointInputs {
            docker_host: docker_host.map(str::to_string),
            docker_context: docker_context.map(str::to_string),
            tls_verify: false,
            config_dir: Some(dir.path().to_path_buf()),
        })
    }

    #[test]
    fn context_directory_is_the_sha256_of_its_name() {
        // Matches the directory `docker context create orbstack` makes.
        assert_eq!(
            context_dir_name("orbstack"),
            "2d89b732b01a00a2d1675ed3cee9fd0f965daadf90603c989dd3afd4569c6896"
        );
    }

    #[test]
    fn current_context_endpoint_is_used_without_env() {
        let dir = docker_config(
            Some("orbstack"),
            &[("orbstack", "unix:///Users/me/.orbstack/run/docker.sock")],
        );
        let endpoint = resolve(None, None, &dir).unwrap();
        assert_eq!(
            endpoint.host.as_deref(),
            Some("unix:///Users/me/.orbstack/run/docker.sock")
        );
        assert_eq!(endpoint.source, EndpointSource::Context("orbstack".into()));
        assert_eq!(endpoint.context_name(), Some("orbstack"));
        assert!(endpoint.mentions_orbstack());
        assert!(endpoint.is_local());
    }

    #[test]
    fn docker_context_env_beats_docker_host_and_current_context() {
        let dir = docker_config(
            Some("orbstack"),
            &[
                ("orbstack", "unix:///Users/me/.orbstack/run/docker.sock"),
                ("colima", "unix:///Users/me/.colima/default/docker.sock"),
            ],
        );
        let endpoint = resolve(Some("unix:///tmp/other.sock"), Some("colima"), &dir).unwrap();
        assert_eq!(
            endpoint.host.as_deref(),
            Some("unix:///Users/me/.colima/default/docker.sock")
        );
        assert!(!endpoint.mentions_orbstack());
    }

    #[test]
    fn docker_host_beats_the_current_context() {
        let dir = docker_config(
            Some("orbstack"),
            &[("orbstack", "unix:///Users/me/.orbstack/run/docker.sock")],
        );
        let endpoint = resolve(Some("unix:///tmp/other.sock"), None, &dir).unwrap();
        assert_eq!(endpoint.host.as_deref(), Some("unix:///tmp/other.sock"));
        assert_eq!(endpoint.source, EndpointSource::DockerHostEnv);
    }

    #[test]
    fn the_default_context_defers_to_docker_host() {
        let dir = docker_config(Some("orbstack"), &[]);
        let endpoint = resolve(Some("unix:///tmp/other.sock"), Some("default"), &dir).unwrap();
        assert_eq!(endpoint.source, EndpointSource::DockerHostEnv);
        assert_eq!(
            resolve(None, Some("default"), &dir).unwrap().source,
            EndpointSource::Default
        );
    }

    #[rstest]
    // no config at all
    #[case(None, &[])]
    // the built-in context has no meta file
    #[case(Some("default"), &[])]
    fn falls_back_to_the_platform_default(
        #[case] current: Option<&str>,
        #[case] contexts: &[(&str, &str)],
    ) {
        let dir = docker_config(current, contexts);
        let endpoint = resolve(None, None, &dir).unwrap();
        assert_eq!(endpoint.host, None);
        assert_eq!(endpoint.source, EndpointSource::Default);
    }

    #[rstest]
    // current context without a meta file
    #[case(Some("ghost"), &[], "isn't defined")]
    // ssh daemons need a transport DBCrust does not have
    #[case(Some("prod"), &[("prod", "ssh://me@203.0.113.9")], "ssh://")]
    #[case(Some("sec"), &[("sec", "https://10.0.0.5:2376")], "https://")]
    fn unsupported_contexts_are_errors_not_silent_fallbacks(
        #[case] current: Option<&str>,
        #[case] contexts: &[(&str, &str)],
        #[case] needle: &str,
    ) {
        let dir = docker_config(current, contexts);
        let error = resolve(None, None, &dir).unwrap_err();
        assert!(matches!(error, DockerError::Endpoint(_)));
        assert!(error.to_string().contains(needle), "{error}");
    }

    #[test]
    fn plain_tcp_contexts_are_fine() {
        let dir = docker_config(Some("lab"), &[("lab", "tcp://10.0.0.5:2375")]);
        let endpoint = resolve(None, None, &dir).unwrap();
        assert_eq!(endpoint.host.as_deref(), Some("tcp://10.0.0.5:2375"));
        assert_eq!(endpoint.remote_host().as_deref(), Some("10.0.0.5"));
    }

    #[test]
    fn contexts_with_tls_material_are_errors() {
        let dir = docker_config(Some("secure"), &[("secure", "tcp://10.0.0.5:2376")]);
        let tls = dir
            .path()
            .join("contexts/tls")
            .join(context_dir_name("secure"))
            .join("docker");
        std::fs::create_dir_all(&tls).unwrap();
        let error = resolve(None, None, &dir).unwrap_err();
        assert!(error.to_string().contains("TLS"), "{error}");
    }

    #[test]
    fn tls_verified_tcp_hosts_are_errors() {
        let error = DockerEndpoint::resolve(&EndpointInputs {
            docker_host: Some("tcp://10.0.0.5:2376".into()),
            tls_verify: true,
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("DOCKER_TLS_VERIFY"), "{error}");
    }

    #[test]
    fn unreadable_config_means_no_current_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{ not json").unwrap();
        assert_eq!(
            resolve(None, None, &dir).unwrap().source,
            EndpointSource::Default
        );
    }

    #[rstest]
    #[case(None, true, None)]
    #[case(Some("unix:///var/run/docker.sock"), true, None)]
    #[case(Some("npipe:////./pipe/docker_engine"), true, None)]
    #[case(Some("tcp://localhost:2375"), true, None)]
    #[case(Some("tcp://127.0.0.1:2375"), true, None)]
    #[case(Some("tcp://[::1]:2375"), true, None)]
    #[case(Some("tcp://10.0.0.5:2375"), false, Some("10.0.0.5"))]
    #[case(Some("http://docker.lab:2375"), false, Some("docker.lab"))]
    #[case(Some("tcp://[fd00::5]:2375"), false, Some("fd00::5"))]
    fn locality_of_the_daemon(
        #[case] host: Option<&str>,
        #[case] local: bool,
        #[case] remote: Option<&str>,
    ) {
        let endpoint = DockerEndpoint {
            host: host.map(str::to_string),
            source: EndpointSource::DockerHostEnv,
        };
        assert_eq!(endpoint.is_local(), local);
        assert_eq!(endpoint.remote_host().as_deref(), remote);
    }

    #[test]
    fn a_pinned_daemon_ignores_the_environment_and_current_context() {
        let dir = docker_config(
            Some("orbstack"),
            &[
                ("orbstack", "unix:///Users/me/.orbstack/run/docker.sock"),
                ("colima", "unix:///Users/me/.colima/default/docker.sock"),
            ],
        );
        // Pin while orbstack is current...
        let pinned = resolve(None, None, &dir).unwrap().pinned_value();
        assert_eq!(pinned, "unix:///Users/me/.orbstack/run/docker.sock");
        // ...then the user switches context: the pin still names orbstack.
        let switched = resolve(None, Some("colima"), &dir).unwrap();
        assert_ne!(switched.pinned_value(), pinned);
        let again = DockerEndpoint::from_pinned(&pinned).unwrap();
        assert_eq!(again.host.as_deref(), Some(pinned.as_str()));
        assert_eq!(again.source, EndpointSource::Pinned);
        assert!(again.mentions_orbstack());
        assert_eq!(again.describe(), format!("the daemon at {pinned}"));
    }

    #[test]
    fn the_default_daemon_is_pinned_as_a_concrete_endpoint() {
        let default = DockerEndpoint {
            host: None,
            source: EndpointSource::Default,
        };
        let pinned = default.pinned_value();
        assert!(pinned.contains("://"), "{pinned}");
        assert!(DockerEndpoint::from_pinned(&pinned).is_ok());
        // Nothing unsupported can be smuggled in through a pin.
        assert!(DockerEndpoint::from_pinned("ssh://me@host").is_err());
    }

    #[test]
    fn explicit_endpoints_accept_bare_socket_paths() {
        let endpoint = DockerEndpoint::explicit("/run/docker.sock").unwrap();
        assert_eq!(endpoint.host.as_deref(), Some("unix:///run/docker.sock"));
        assert_eq!(endpoint.source, EndpointSource::Explicit);
        assert!(DockerEndpoint::explicit("ssh://me@host").is_err());
    }

    #[test]
    fn descriptions_name_the_source() {
        let endpoint = DockerEndpoint {
            host: Some("unix:///x.sock".into()),
            source: EndpointSource::Context("orbstack".into()),
        };
        assert_eq!(endpoint.describe(), "context 'orbstack' (unix:///x.sock)");
        assert_eq!(endpoint.label(), "orbstack");
        let env = DockerEndpoint {
            host: Some("tcp://h:1".into()),
            source: EndpointSource::DockerHostEnv,
        };
        assert_eq!(env.label(), "DOCKER_HOST");
        assert_eq!(env.describe(), "DOCKER_HOST (tcp://h:1)");
    }

    #[rstest]
    #[case(Some("OrbStack"), Some("orbstack"), true)]
    #[case(Some("Docker Desktop"), Some("docker-desktop"), false)]
    #[case(Some("Ubuntu 24.04"), Some("colima"), false)]
    #[case(None, Some("my-orbstack-vm"), true)]
    #[case(None, None, false)]
    fn orbstack_is_detected_from_the_daemon(
        #[case] os: Option<&str>,
        #[case] name: Option<&str>,
        #[case] expected: bool,
    ) {
        assert_eq!(is_orbstack_daemon(os, name), expected);
    }
}
