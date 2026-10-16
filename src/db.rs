use crate::config::SSHTunnelConfig;
use crate::database::{ConnectionInfo, DatabaseClient, DatabaseType, DatabaseTypeExt};
#[cfg(feature = "docker")]
use crate::docker::DockerPhase;
#[cfg(feature = "docker")]
use crate::docker_route::DockerRoute;
#[cfg(feature = "docker")]
use crate::docker_tunnel::DockerTunnel;
use crate::drivers::create_database_client;
use crate::pgpass;

use inquire::MultiSelect;
use std::collections::HashMap;
use std::error::Error as StdError;
#[cfg(feature = "docker")]
use std::sync::Arc;
use tracing::{debug, info};

// Metadata DTOs live in `crate::model`; re-exported so `crate::db::TableDetails`
// (and `dbcrust::db::TableDetails` for the GUI) keep resolving.
pub use crate::model::*;

#[derive(Debug)]
pub struct ColumnSelectionAborted;

impl std::fmt::Display for ColumnSelectionAborted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Column selection aborted by user")
    }
}

impl StdError for ColumnSelectionAborted {}

/// Connection pool statistics for monitoring and debugging
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub max_connections: u32,
    pub total_connections: u32,
    pub active_connections: u32,
    pub idle_connections: u32,
    pub acquire_timeout_seconds: u64,
}

/// Query results with optional column filtering information
#[derive(Debug)]
pub struct QueryResultsWithInfo {
    pub data: Vec<Vec<String>>,
    pub column_info: Option<ColumnFilteringInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendMode {
    Cli,
    Gui,
}

impl FrontendMode {
    fn allows_interactive_terminal_ui(self) -> bool {
        matches!(self, Self::Cli)
    }

    fn allows_stdout_status(self) -> bool {
        matches!(self, Self::Cli)
    }
}

/// A connected Docker container: the database, what to record for it, and
/// how it is reached.
#[cfg(feature = "docker")]
pub struct DockerConnection {
    pub database: Database,
    /// The resolved connection (stable: for a relayed connection the host is
    /// the container, not the relay's ephemeral local port).
    pub connection_info: ConnectionInfo,
    pub route: DockerRoute,
}

pub struct Database {
    // Database abstraction layer client
    database_client: Option<Box<dyn DatabaseClient>>,

    // Connection info override for special cases like Vault connections
    connection_info_override: Option<crate::database::ConnectionInfo>,

    // SSH tunnel management
    ssh_tunnel: Option<crate::ssh_tunnel::SSHTunnel>,

    // `docker exec` relay for Docker connections the host cannot route to.
    // Owned here so it lives (and dies) with the connection: dropping the
    // Database aborts the relay and every exec it started.
    #[cfg(feature = "docker")]
    docker_tunnel: Option<Arc<DockerTunnel>>,
    #[cfg(feature = "docker")]
    docker_route: Option<DockerRoute>,
    // The Docker daemon this connection was made through (see
    // `docker_daemon_pin`)
    #[cfg(feature = "docker")]
    docker_daemon: Option<String>,

    // Application settings and state
    expanded_display: bool,
    output_format: crate::cli::OutputFormat,
    read_only: bool,
    default_limit: usize,
    autocomplete_enabled: bool,
    explain_mode: bool,
    explain_tui_mode: bool, // TUI-based explain visualizer mode
    column_select_mode: bool,
    banner_enabled: bool,
    column_selection_threshold: usize,
    column_selection_default_all: bool,
    column_views: HashMap<String, Vec<String>>, // Map of column view name -> selected columns
    last_view_key: Option<String>,
    last_json_plan: Option<String>, // Store the last EXPLAIN JSON plan for copying
    frontend_mode: FrontendMode,
}

impl Database {
    /// Create a new Database instance from a database URL
    pub async fn from_url(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        Self::from_url_with_mode(
            url,
            default_limit,
            expanded_display_default,
            FrontendMode::Cli,
        )
        .await
    }

    pub async fn from_url_with_mode(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        frontend_mode: FrontendMode,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        // Docker targets don't need (and mustn't wait for) the config files
        if url.starts_with("docker://") {
            #[cfg(feature = "docker")]
            {
                let (database, _) = Self::from_docker_url_with_mode(
                    url,
                    default_limit,
                    expanded_display_default,
                    frontend_mode,
                )
                .await?;
                return Ok(database);
            }
            #[cfg(not(feature = "docker"))]
            return Err(Box::new(crate::features::FeatureNotCompiled::Docker));
        }
        let config = crate::config::Config::load();
        Self::from_url_with_config(
            url,
            &config,
            default_limit,
            expanded_display_default,
            frontend_mode,
        )
        .await
    }

    /// [`Database::from_url_with_mode`] with the settings supplied by the
    /// caller instead of loaded from the config files: constructing a
    /// `Database` then touches no shared state besides the process-wide
    /// read-only request.
    pub async fn from_url_with_config(
        url: &str,
        config: &crate::config::Config,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        frontend_mode: FrontendMode,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        debug!("[Database::from_url] Creating database from URL");
        let step_start = std::time::Instant::now();

        // Handle Docker URLs specially
        if url.starts_with("docker://") {
            #[cfg(feature = "docker")]
            {
                let (database, _) = Self::from_docker_url_with_mode(
                    url,
                    default_limit,
                    expanded_display_default,
                    frontend_mode,
                )
                .await?;
                return Ok(database);
            }
            #[cfg(not(feature = "docker"))]
            return Err(Box::new(crate::features::FeatureNotCompiled::Docker));
        }

        // Parse the connection info from URL
        let config_start = std::time::Instant::now();
        debug!("📋 Parsing connection URL...");
        let connection_info = ConnectionInfo::parse_url(url)?;
        debug!(
            "[Database::from_url] Parsed URL in {:?}",
            step_start.elapsed()
        );

        // For SQLite, we don't need SSH tunneling
        if connection_info.database_type.is_file_based() {
            return Self::from_connection_info_with_config(
                connection_info,
                default_limit,
                expanded_display_default,
                None,
                frontend_mode,
                config,
            )
            .await;
        }

        // For PostgreSQL/MySQL, check for SSH tunnel patterns
        debug!("🔍 Checking for SSH tunnel patterns...");
        let ssh_tunnel_config = if let Some(ref host) = connection_info.host {
            config.get_ssh_tunnel_for_host(host)
        } else {
            None
        };
        debug!(
            "[Database::from_url] Config check took {:?}",
            config_start.elapsed()
        );

        if ssh_tunnel_config.is_some() {
            info!(
                "✓ SSH tunnel pattern found for host: {:?}",
                connection_info.host
            );
            debug!(
                "[Database::from_url] SSH tunnel configuration found for host: {:?}",
                connection_info.host
            );
        } else {
            debug!(
                "⚠️  No SSH tunnel pattern found for host: {:?}",
                connection_info.host
            );
        }

        debug!("🔧 Creating database connection...");
        let conn_start = std::time::Instant::now();
        let result = Self::from_connection_info_with_config(
            connection_info,
            default_limit,
            expanded_display_default,
            ssh_tunnel_config,
            frontend_mode,
            config,
        )
        .await;
        debug!(
            "[Database::from_url] from_connection_info took {:?}",
            conn_start.elapsed()
        );
        result
    }
}

// Docker connection flow: needs the `docker` feature (`dbcrust-docker`). A
// build without it answers `docker://` URLs with
// `FeatureNotCompiled::Docker` (see `from_url_with_config`).
#[cfg(feature = "docker")]
impl Database {
    /// Create a new Database instance from a Docker URL
    pub async fn from_docker_url(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
    ) -> std::result::Result<(Self, Option<ConnectionInfo>), Box<dyn StdError>> {
        Self::from_docker_url_with_mode(
            url,
            default_limit,
            expanded_display_default,
            FrontendMode::Cli,
        )
        .await
    }

    pub async fn from_docker_url_with_mode(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        frontend_mode: FrontendMode,
    ) -> std::result::Result<(Self, Option<ConnectionInfo>), Box<dyn StdError>> {
        debug!("[Database::from_docker_url] Creating database from Docker URL");

        // Parse Docker URL (container, credentials, database, options)
        let mut template = crate::docker_target::DockerTarget::parse(url)
            .ok_or("Invalid Docker URL format")?
            .to_connection_info();

        // If the container name is empty, provide interactive selection
        let container_name = template
            .docker_container
            .clone()
            .ok_or("Docker container name not found in URL")?;
        if container_name.is_empty() {
            if !frontend_mode.allows_interactive_terminal_ui() {
                return Err("Interactive Docker container selection is not available in GUI mode. Provide an explicit container name.".into());
            }
            template.docker_container = Some(Self::select_docker_container().await?);
        }

        let connected = Self::connect_docker_container(
            &template,
            |_| {},
            |info| {
                Self::from_connection_info_with_mode(
                    info,
                    default_limit,
                    expanded_display_default,
                    None,
                    frontend_mode,
                )
            },
        )
        .await?;

        if connected.route.is_forwarded()
            && frontend_mode.allows_stdout_status()
            && !crate::database::one_shot_mode()
        {
            println!(
                "🐳 '{}' is not reachable from this machine directly: connected through a `docker exec` relay ({}).",
                template.docker_container.as_deref().unwrap_or_default(),
                connected.route.helper.as_deref().unwrap_or("relay"),
            );
        }
        Ok((connected.database, Some(connected.connection_info)))
    }

    /// Connect to a database container by the first route that works (see
    /// [`crate::docker::DockerClient::connect_via_routes`]).
    ///
    /// `template` is the parsed `docker://` target: the container name plus
    /// any credentials/database/options given explicitly (they win over the
    /// container's environment). `progress` is told each [`DockerPhase`];
    /// `build` constructs the `Database` from the resolved address — it is
    /// where callers serialise construction and set up read-only mode, so
    /// the Docker work around it never waits on anything. The whole thing,
    /// container lookup included, runs under one deadline. The returned
    /// `Database` owns the relay (if any): dropping it tears the relay down,
    /// and so does dropping this future.
    ///
    /// Which daemon: the current one, unless `template.options` carries
    /// [`crate::docker_target::PINNED_DAEMON_OPTION`] (as the siblings of an open
    /// Docker connection do: see [`Database::docker_daemon_pin`]), in which
    /// case that very daemon is used, whatever context is current by now.
    pub async fn connect_docker_container<P, B, Fut>(
        template: &ConnectionInfo,
        progress: P,
        mut build: B,
    ) -> std::result::Result<DockerConnection, Box<dyn StdError>>
    where
        P: FnMut(DockerPhase),
        B: FnMut(ConnectionInfo) -> Fut,
        Fut: std::future::Future<Output = std::result::Result<Self, Box<dyn StdError>>>,
    {
        let container_name = template
            .docker_container
            .as_deref()
            .filter(|name| !name.is_empty())
            .ok_or("Docker container name not found in URL")?;

        // The pin is routing information, not a database option
        let mut template = template.clone();
        let pinned = template
            .options
            .remove(crate::docker_target::PINNED_DAEMON_OPTION);
        let docker_client = match pinned {
            Some(pin) => crate::docker::DockerClient::with_endpoint(
                crate::docker::DockerEndpoint::from_pinned(&pin)?,
            )?,
            None => crate::docker::DockerClient::new()?,
        };

        let connected = docker_client
            .connect_container(container_name, progress, |docker_connection| {
                let resolved = Self::resolve_docker_connection_info(&template, docker_connection);
                let attempt = build(resolved.clone());
                async move { attempt.await.map(|database| (database, resolved)) }
            })
            .await?;

        let (mut database, resolved_info) = connected.value;
        debug!(
            "[Database::from_docker_url] Resolved Docker connection: {}@{}:{}/{} ({})",
            resolved_info.username.as_deref().unwrap_or(""),
            resolved_info.host.as_deref().unwrap_or(""),
            resolved_info.port.unwrap_or(0),
            resolved_info.database.as_deref().unwrap_or(""),
            connected.route.detail
        );
        // What recents/history record must not change with every connection:
        // a relay's ephemeral local port is not part of the container's identity.
        let connection_info = if connected.route.is_forwarded() {
            ConnectionInfo {
                host: Some(connected.container_name.clone()),
                port: Some(connected.database_port),
                ..resolved_info
            }
        } else {
            resolved_info
        };
        database.docker_tunnel = connected.tunnel;
        database.docker_route = Some(connected.route.clone());
        database.docker_daemon = Some(docker_client.endpoint().pinned_value());
        Ok(DockerConnection {
            database,
            connection_info,
            route: connected.route,
        })
    }

    /// For a Docker connection: the identity of the daemon it was made
    /// through. Put it under [`crate::docker_target::PINNED_DAEMON_OPTION`] in the
    /// options of a follow-up request (another database of the same
    /// container) so it reaches the same daemon even if the user switched
    /// docker contexts meanwhile.
    pub fn docker_daemon_pin(&self) -> Option<&str> {
        self.docker_daemon.as_deref()
    }

    /// Merge the `docker://` target with the address and credentials found
    /// for the container: explicit values from the URL win.
    fn resolve_docker_connection_info(
        template: &ConnectionInfo,
        docker_connection: crate::docker::DockerDatabaseConnection,
    ) -> ConnectionInfo {
        ConnectionInfo {
            database_type: docker_connection.database_type,
            host: Some(docker_connection.host),
            port: Some(docker_connection.port),
            username: template
                .username
                .clone()
                .filter(|u| !u.is_empty())
                .or(docker_connection.username),
            password: template
                .password
                .clone()
                .filter(|p| !p.is_empty())
                .or(docker_connection.password),
            database: template
                .database
                .clone()
                .or(docker_connection.database_name),
            options: template.options.clone(),
            docker_container: template.docker_container.clone(),
            use_tls: template.use_tls,
            ..Default::default()
        }
    }

    /// The route of a Docker connection: direct, or through a `docker exec`
    /// relay owned by this `Database`.
    pub fn docker_route(&self) -> Option<&DockerRoute> {
        self.docker_route.as_ref()
    }

    /// Interactive Docker container selection
    async fn select_docker_container() -> std::result::Result<String, Box<dyn StdError>> {
        println!("🐳 Discovering Docker database containers...");

        // Create Docker client
        let docker_client = crate::docker::DockerClient::new()
            .map_err(|e| format!("Failed to create Docker client: {e}"))?;

        // List all database containers
        let containers = docker_client
            .list_database_containers()
            .await
            .map_err(|e| format!("Failed to list Docker containers: {e}"))?;

        if containers.is_empty() {
            return Err(
                "No database containers found. Make sure you have database containers running."
                    .into(),
            );
        }

        // Separate running and stopped containers
        let running_containers: Vec<_> = containers
            .iter()
            .filter(|c| c.status.contains("running") || c.status.contains("Up"))
            .collect();
        let stopped_containers: Vec<_> = containers
            .iter()
            .filter(|c| !(c.status.contains("running") || c.status.contains("Up")))
            .collect();

        // Show summary of stopped containers if any exist
        if !stopped_containers.is_empty() {
            println!(
                "ℹ️  Found {} stopped database container(s):",
                stopped_containers.len()
            );
            for container in &stopped_containers {
                let db_type = container
                    .database_type
                    .as_ref()
                    .map(|dt| format!("{dt}"))
                    .unwrap_or("Unknown".to_string());
                println!(
                    "   🔴 {} ({}) - {}",
                    container.name, db_type, container.status
                );
            }
            println!();
        }

        // Check if we have any running containers
        if running_containers.is_empty() {
            return Err(
                "No running database containers found. Please start a database container first."
                    .into(),
            );
        }

        // Create selection options only for running containers
        let mut options = Vec::new();
        for container in &running_containers {
            let db_type = container
                .database_type
                .as_ref()
                .map(|dt| format!("{dt}"))
                .unwrap_or("Unknown".to_string());

            let port_info = if let Some(port) = container.host_port {
                format!(" | Port: {port}")
            } else {
                " (no exposed port)".to_string()
            };

            let option = format!(
                "🟢 {} ({}) - {}{}",
                container.name, db_type, container.status, port_info
            );
            options.push(option);
        }

        // Show interactive selection and get the index
        let selected_index = inquire::Select::new("Select a database container:", options.clone())
            .prompt()
            .map_err(|e| format!("Selection cancelled: {e}"))?;

        // Find the index of the selected option
        let container_index = options
            .iter()
            .position(|option| option == &selected_index)
            .ok_or("Invalid selection")?;

        // Get the selected container by index (from running containers only)
        let selected_container = running_containers[container_index];

        println!(
            "📦 Selected container: {} ({})",
            selected_container.name,
            selected_container
                .database_type
                .as_ref()
                .map(|dt| format!("{dt}"))
                .unwrap_or("Unknown".to_string())
        );

        Ok(selected_container.name.clone())
    }

    /// Create a new Database instance from a Docker URL and return connection info for tracking
    pub async fn from_docker_url_with_tracking(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
    ) -> std::result::Result<(Self, Option<ConnectionInfo>), Box<dyn StdError>> {
        Self::from_docker_url_with_tracking_mode(
            url,
            default_limit,
            expanded_display_default,
            FrontendMode::Cli,
        )
        .await
    }

    pub async fn from_docker_url_with_tracking_mode(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        frontend_mode: FrontendMode,
    ) -> std::result::Result<(Self, Option<ConnectionInfo>), Box<dyn StdError>> {
        Self::from_docker_url_with_mode(url, default_limit, expanded_display_default, frontend_mode)
            .await
    }
}

impl Database {
    /// Create a new Database instance from ConnectionInfo
    pub async fn from_connection_info(
        connection_info: ConnectionInfo,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        ssh_tunnel_config: Option<SSHTunnelConfig>,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        Self::from_connection_info_with_mode(
            connection_info,
            default_limit,
            expanded_display_default,
            ssh_tunnel_config,
            FrontendMode::Cli,
        )
        .await
    }

    pub async fn from_connection_info_with_mode(
        connection_info: ConnectionInfo,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        ssh_tunnel_config: Option<SSHTunnelConfig>,
        frontend_mode: FrontendMode,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        let config = crate::config::Config::load();
        Self::from_connection_info_with_config(
            connection_info,
            default_limit,
            expanded_display_default,
            ssh_tunnel_config,
            frontend_mode,
            &config,
        )
        .await
    }

    /// [`Database::from_connection_info_with_mode`] with the settings
    /// supplied by the caller (no config files are read).
    pub async fn from_connection_info_with_config(
        connection_info: ConnectionInfo,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        ssh_tunnel_config: Option<SSHTunnelConfig>,
        frontend_mode: FrontendMode,
        config: &crate::config::Config,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        debug!("[Database::from_connection_info] Creating database from connection info");

        let original_connection_info = connection_info.clone();

        // For SSH tunnel scenarios, we need to create a modified connection info
        let (final_connection_info, ssh_tunnel) = if let Some(ref tunnel_config) = ssh_tunnel_config
        {
            if tunnel_config.enabled {
                // Establish SSH tunnel
                let mut ssh_tunnel =
                    crate::ssh_tunnel::SSHTunnel::new().ok_or("Failed to create SSH tunnel")?;

                let original_host = connection_info
                    .host
                    .as_ref()
                    .ok_or("Host is required for SSH tunnel")?;
                let original_port = connection_info
                    .port
                    .or_else(|| connection_info.default_port())
                    .ok_or("Port is required for SSH tunnel")?;

                let local_port = ssh_tunnel
                    .establish(tunnel_config, original_host, original_port)
                    .await
                    .map_err(|e| format!("Failed to establish SSH tunnel: {e}"))?;

                // Create modified connection info to use the local tunnel port
                let mut modified_connection_info = connection_info.clone();
                modified_connection_info.host = Some("127.0.0.1".to_string());
                modified_connection_info.port = Some(local_port);

                (modified_connection_info, Some(ssh_tunnel))
            } else {
                (connection_info, None)
            }
        } else {
            (connection_info, None)
        };

        // Create database client using the new abstraction layer
        debug!("[Database::from_connection_info] Creating database client");
        let database_client = create_database_client(final_connection_info)
            .await
            .map_err(|e| format!("Failed to create database client: {e}"))?;
        let connection_info_override = ssh_tunnel.as_ref().map(|_| original_connection_info);

        let db = Self {
            database_client: Some(database_client),
            connection_info_override,
            ssh_tunnel,
            #[cfg(feature = "docker")]
            docker_tunnel: None,
            #[cfg(feature = "docker")]
            docker_route: None,
            #[cfg(feature = "docker")]
            docker_daemon: None,
            expanded_display: expanded_display_default.unwrap_or(false),
            output_format: crate::cli::OutputFormat::Table,
            // Every construction path (direct URL, docker, vault, session)
            // picks up --read-only from the process-wide request
            read_only: crate::database::read_only_requested(),
            default_limit: default_limit.unwrap_or(100),
            autocomplete_enabled: config.autocomplete_enabled,
            explain_mode: config.explain_mode_default,
            explain_tui_mode: false,
            column_select_mode: false,
            banner_enabled: config.show_banner,
            column_selection_threshold: config.column_selection_threshold,
            column_selection_default_all: config.column_selection_default_all,
            column_views: HashMap::new(),
            last_view_key: None,
            last_json_plan: None,
            frontend_mode,
        };

        // Validate the connection before returning
        debug!("[Database::from_connection_info] Validating connection");
        db.validate_connection().await?;

        // Display server info if enabled in config and supported by this
        // frontend — never in one-shot mode, where stdout must stay parseable.
        if config.show_server_info
            && frontend_mode.allows_stdout_status()
            && !crate::database::one_shot_mode()
        {
            db.display_server_info().await;
        }

        Ok(db)
    }

    pub async fn new(
        host: &str,
        port: u16,
        user: &str,
        password_param: &str,
        dbname: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        ssh_tunnel_config: Option<SSHTunnelConfig>,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        // Legacy method - convert parameters to ConnectionInfo and use from_connection_info
        let password = if password_param.is_empty() {
            pgpass::lookup_password(host, port, dbname, user)
        } else {
            Some(password_param.to_string())
        };

        let connection_info = ConnectionInfo {
            database_type: DatabaseType::PostgreSQL,
            host: Some(host.to_string()),
            port: Some(port),
            username: Some(user.to_string()),
            password,
            database: Some(dbname.to_string()),
            ..Default::default()
        };

        Self::from_connection_info(
            connection_info,
            default_limit,
            expanded_display_default,
            ssh_tunnel_config,
        )
        .await
    }

    /// Prefetch metadata asynchronously to warm up autocompletion cache
    pub async fn connect_to_db(
        &mut self,
        dbname: &str,
    ) -> std::result::Result<(), Box<dyn StdError>> {
        // Use new database abstraction layer
        if let Some(ref mut database_client) = self.database_client {
            debug!("Using database abstraction layer for connect_to_db");
            return database_client
                .connect_to_database(dbname)
                .await
                .map_err(|e| e.into());
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn list_databases(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        // Use new database abstraction layer
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for list_databases");
            return database_client.list_databases().await.map_err(|e| e.into());
        } else {
            Err("No database client available".into())
        }
    }

    /// List users (database-specific implementation)
    pub async fn list_users(&mut self) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::list_users] Listing database users");

        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for list_users");

            let connection_info = database_client.get_connection_info();

            if connection_info.database_type.is_file_based() {
                // File-based engines have no users concept
                Ok(vec![
                    vec!["Note".to_string()],
                    vec![format!(
                        "{} is file-based and doesn't have user accounts",
                        connection_info.database_type
                    )],
                    vec!["Access control is handled at the file system level".to_string()],
                ])
            } else {
                match connection_info.database_type {
                    crate::database::DatabaseType::MySQL => self
                        .execute_query(
                            "SELECT User, Host, account_locked FROM mysql.user ORDER BY User",
                        )
                        .await
                        .map_err(|e| format!("Error listing MySQL users: {e}").into()),
                    crate::database::DatabaseType::PostgreSQL => self
                        .execute_query(
                            "SELECT usename, usesuper, usecreatedb FROM pg_user ORDER BY usename",
                        )
                        .await
                        .map_err(|e| format!("Error listing PostgreSQL users: {e}").into()),
                    crate::database::DatabaseType::SqlServer => self
                        .execute_query(
                            "SELECT name, type_desc AS type, is_disabled \
                             FROM sys.server_principals \
                             WHERE type IN ('S', 'U', 'G') AND name NOT LIKE '##%' \
                             ORDER BY name",
                        )
                        .await
                        .map_err(|e| format!("Error listing SQL Server logins: {e}").into()),
                    _ => Ok(vec![
                        vec!["Error".to_string()],
                        vec!["Unsupported database type".to_string()],
                    ]),
                }
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// List indexes (primarily for SQLite)
    pub async fn list_indexes(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::list_indexes] Listing database indexes");

        // Try using the new database abstraction layer first
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for list_indexes");

            let connection_info = database_client.get_connection_info();

            if connection_info.database_type == crate::database::DatabaseType::DuckDB {
                let query = r#"
                    SELECT
                        index_name AS "Index Name",
                        table_name AS "Table",
                        CASE WHEN is_unique THEN 'UNIQUE' ELSE 'NON-UNIQUE' END AS "Type"
                    FROM duckdb_indexes()
                    WHERE database_name = current_database()
                    ORDER BY table_name, index_name
                "#;
                return self
                    .execute_query(query)
                    .await
                    .map_err(|e| format!("Error listing DuckDB indexes: {e}").into());
            } else if connection_info.database_type.is_file_based() {
                let query = r#"
                    SELECT
                        name as 'Index Name',
                        tbl_name as 'Table',
                        CASE
                            WHEN "unique" = 1 THEN 'UNIQUE'
                            ELSE 'NON-UNIQUE'
                        END as 'Type'
                    FROM sqlite_master
                    WHERE type = 'index'
                      AND name NOT LIKE 'sqlite_%'
                    ORDER BY tbl_name, name
                "#;
                match self.execute_query(query).await {
                    Ok(results) => return Ok(results),
                    Err(e) => return Err(format!("Error listing SQLite indexes: {e}").into()),
                }
            } else {
                match connection_info.database_type {
                    crate::database::DatabaseType::MySQL => {
                        return Ok(vec![
                            vec!["Note".to_string()],
                            vec!["Use MySQL's SHOW INDEX FROM <table> command".to_string()],
                            vec!["Or query INFORMATION_SCHEMA.STATISTICS".to_string()],
                        ]);
                    }
                    crate::database::DatabaseType::PostgreSQL => {
                        return Ok(vec![
                            vec!["Note".to_string()],
                            vec!["Use PostgreSQL's \\di command or".to_string()],
                            vec!["Query pg_indexes system view".to_string()],
                        ]);
                    }
                    crate::database::DatabaseType::SqlServer => {
                        return self
                            .execute_query(
                                "SELECT s.name AS [Schema], t.name AS [Table], i.name AS [Index], \
                                 i.type_desc AS [Type], \
                                 CASE WHEN i.is_primary_key = 1 THEN 'PRIMARY KEY' \
                                      WHEN i.is_unique = 1 THEN 'UNIQUE' ELSE 'NON-UNIQUE' END AS [Kind] \
                                 FROM sys.indexes AS i \
                                 JOIN sys.tables AS t ON t.object_id = i.object_id \
                                 JOIN sys.schemas AS s ON s.schema_id = t.schema_id \
                                 WHERE i.type > 0 AND t.is_ms_shipped = 0 \
                                 ORDER BY s.name, t.name, i.name",
                            )
                            .await
                            .map_err(|e| format!("Error listing SQL Server indexes: {e}").into());
                    }
                    _ => {
                        return Ok(vec![
                            vec!["Error".to_string()],
                            vec!["Unsupported database type".to_string()],
                        ]);
                    }
                }
            }
        }

        // Default response for when database abstraction layer is not available
        Ok(vec![
            vec!["Note".to_string()],
            vec!["Index listing not available for this database type".to_string()],
        ])
    }

    /// List pragmas (SQLite-specific)
    pub async fn list_pragmas(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::list_pragmas] Listing database pragmas");

        // Try using the new database abstraction layer first
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for list_pragmas");

            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::SQLite => {
                    // Get common SQLite pragma values
                    let mut results = Vec::new();
                    results.push(vec!["Pragma".to_string(), "Value".to_string()]);

                    let pragmas = vec![
                        "PRAGMA journal_mode",
                        "PRAGMA synchronous",
                        "PRAGMA cache_size",
                        "PRAGMA foreign_keys",
                        "PRAGMA auto_vacuum",
                        "PRAGMA encoding",
                        "PRAGMA page_size",
                        "PRAGMA temp_store",
                        "PRAGMA locking_mode",
                        "PRAGMA wal_autocheckpoint",
                    ];

                    for pragma in pragmas {
                        match self.execute_query(pragma).await {
                            Ok(pragma_results) => {
                                if pragma_results.len() > 1 && !pragma_results[1].is_empty() {
                                    let pragma_name = pragma.replace("PRAGMA ", "");
                                    let value = &pragma_results[1][0];
                                    results.push(vec![pragma_name, value.clone()]);
                                }
                            }
                            Err(_) => {
                                // Skip pragmas that fail
                            }
                        }
                    }

                    return Ok(results);
                }
                _ => {
                    return Ok(vec![
                        vec!["Note".to_string()],
                        vec!["Pragmas are specific to SQLite".to_string()],
                        vec!["This command is only available for SQLite databases".to_string()],
                    ]);
                }
            }
        }

        // Default response for when database abstraction layer is not available
        Err("No database client available".into())
    }

    /// List MongoDB collections
    pub async fn list_collections(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::list_collections] Listing MongoDB collections");

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Use MongoDB-specific query to list collections
                    let query = r#"
                        SELECT
                            name as "Collection Name",
                            type as "Type",
                            options as "Options"
                        FROM (
                            SELECT
                                name,
                                'collection' as type,
                                '{}' as options
                            FROM system.namespaces
                            WHERE name NOT LIKE 'system.%'
                        ) collections
                        ORDER BY name
                    "#;

                    self.execute_query(query)
                        .await
                        .map_err(|e| format!("Error listing MongoDB collections: {e}").into())
                }
                _ => Ok(vec![
                    vec!["Error".to_string()],
                    vec!["This command is only available for MongoDB databases".to_string()],
                ]),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Describe MongoDB collection
    pub async fn describe_collection(
        &mut self,
        collection_name: &str,
    ) -> std::result::Result<crate::db::TableDetails, Box<dyn StdError>> {
        debug!(
            "[Database::describe_collection] Describing MongoDB collection: {}",
            collection_name
        );

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Get collection metadata from MongoDB
                    database_client
                        .get_metadata_provider()
                        .get_table_details(collection_name, None)
                        .await
                        .map_err(|e| format!("Error describing MongoDB collection: {e}").into())
                }
                _ => Err("This command is only available for MongoDB databases".into()),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// List MongoDB indexes
    pub async fn list_mongo_indexes(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::list_mongo_indexes] Listing MongoDB indexes");

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Query MongoDB system.indexes collection
                    let query = r#"
                        SELECT
                            name as "Index Name",
                            ns as "Namespace",
                            key as "Keys"
                        FROM system.indexes
                        WHERE name NOT LIKE '_id_'
                        ORDER BY ns, name
                    "#;

                    self.execute_query(query)
                        .await
                        .map_err(|e| format!("Error listing MongoDB indexes: {e}").into())
                }
                _ => Ok(vec![
                    vec!["Error".to_string()],
                    vec!["This command is only available for MongoDB databases".to_string()],
                ]),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Create MongoDB index
    pub async fn create_mongo_index(
        &mut self,
        collection: &str,
        field: &str,
        index_type: Option<&str>,
    ) -> std::result::Result<(), Box<dyn StdError>> {
        debug!(
            "[Database::create_mongo_index] Creating MongoDB index on {}.{}",
            collection, field
        );

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Create index using MongoDB command
                    let index_spec = match index_type {
                        Some("text") => format!("{{ \"{field}\": \"text\" }}"),
                        Some("hash") => format!("{{ \"{field}\": \"hashed\" }}"),
                        Some("desc") => format!("{{ \"{field}\": -1 }}"),
                        _ => format!("{{ \"{field}\": 1 }}"), // default ascending
                    };

                    let query = format!(
                        "db.runCommand({{ createIndexes: \"{collection}\", indexes: [{{ key: {index_spec}, name: \"{collection}_{field}_idx\" }}] }})"
                    );

                    self.execute_query(&query)
                        .await
                        .map_err(|e| format!("Error creating MongoDB index: {e}"))?;

                    Ok(())
                }
                _ => Err("This command is only available for MongoDB databases".into()),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Drop MongoDB index
    pub async fn drop_mongo_index(
        &mut self,
        collection: &str,
        index_name: &str,
    ) -> std::result::Result<(), Box<dyn StdError>> {
        debug!(
            "[Database::drop_mongo_index] Dropping MongoDB index {} from {}",
            index_name, collection
        );

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Drop index using MongoDB command
                    let query = format!(
                        "db.runCommand({{ dropIndexes: \"{collection}\", index: \"{index_name}\" }})"
                    );

                    self.execute_query(&query)
                        .await
                        .map_err(|e| format!("Error dropping MongoDB index: {e}"))?;

                    Ok(())
                }
                _ => Err("This command is only available for MongoDB databases".into()),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Get MongoDB database statistics
    pub async fn mongo_stats(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::mongo_stats] Getting MongoDB database statistics");

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Get database statistics using MongoDB command
                    let query = "db.runCommand({ dbStats: 1 })";

                    self.execute_query(query)
                        .await
                        .map_err(|e| format!("Error getting MongoDB stats: {e}").into())
                }
                _ => Ok(vec![
                    vec!["Error".to_string()],
                    vec!["This command is only available for MongoDB databases".to_string()],
                ]),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Execute MongoDB find query
    pub async fn mongo_find(
        &mut self,
        collection: &str,
        filter: Option<&str>,
        projection: Option<&str>,
        limit: Option<i64>,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!(
            "[Database::mongo_find] Executing MongoDB find on collection: {}",
            collection
        );

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Build MongoDB find query
                    let filter_str = filter.unwrap_or("{}");
                    let projection_str = projection.map(|p| format!(", {p}")).unwrap_or_default();
                    let limit_str = limit.map(|l| format!(", limit: {l}")).unwrap_or_default();

                    let query =
                        format!("db.{collection}.find({filter_str}{projection_str}){limit_str}");

                    self.execute_query(&query)
                        .await
                        .map_err(|e| format!("Error executing MongoDB find: {e}").into())
                }
                _ => Ok(vec![
                    vec!["Error".to_string()],
                    vec!["This command is only available for MongoDB databases".to_string()],
                ]),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Execute MongoDB aggregation pipeline
    pub async fn mongo_aggregate(
        &mut self,
        collection: &str,
        pipeline: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!(
            "[Database::mongo_aggregate] Executing MongoDB aggregation on collection: {}",
            collection
        );

        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();

            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Execute aggregation pipeline
                    let query = format!("db.{collection}.aggregate({pipeline})");

                    self.execute_query(&query)
                        .await
                        .map_err(|e| format!("Error executing MongoDB aggregation: {e}").into())
                }
                _ => Ok(vec![
                    vec!["Error".to_string()],
                    vec!["This command is only available for MongoDB databases".to_string()],
                ]),
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Execute MongoDB text search
    pub async fn mongo_text_search(
        &mut self,
        collection: &str,
        search_term: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!(
            "[Database::mongo_text_search] Executing text search on collection: {}",
            collection
        );
        if let Some(ref database_client) = self.database_client {
            let connection_info = database_client.get_connection_info();
            match connection_info.database_type {
                crate::database::DatabaseType::MongoDB => {
                    // Execute MongoDB text search using $text operator
                    let filter = format!(r#"{{"$text": {{"$search": "{search_term}"}}}}"#);
                    self.mongo_find(collection, Some(&filter), None, Some(10))
                        .await
                        .map_err(|e| format!("Error executing text search: {e}").into())
                }
                _ => Ok(vec![
                    vec!["Error".to_string()],
                    vec!["This command is only available for MongoDB databases".to_string()],
                ]),
            }
        } else {
            Err("No database client available".into())
        }
    }

    pub fn get_current_db(&self) -> String {
        if let Some(ref client) = self.database_client {
            client.get_current_database()
        } else {
            "unknown".to_string()
        }
    }

    pub fn get_username(&self) -> String {
        if let Some(ref client) = self.database_client {
            client
                .get_connection_info()
                .username
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        }
    }

    pub fn get_host(&self) -> String {
        if let Some(ref client) = self.database_client {
            client
                .get_connection_info()
                .host
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        }
    }

    pub fn get_port(&self) -> u16 {
        if let Some(ref client) = self.database_client {
            client.get_connection_info().port.unwrap_or(0)
        } else {
            0
        }
    }

    pub fn get_database_client(&self) -> Option<&dyn DatabaseClient> {
        self.database_client.as_deref()
    }

    pub async fn is_connected(&self) -> bool {
        if let Some(ref client) = self.database_client {
            client.is_connected().await
        } else {
            false
        }
    }

    /// Check if we have a valid database connection
    /// This is used by completion system to determine if metadata queries are possible
    pub fn has_database_connection(&self) -> bool {
        self.database_client.is_some()
    }

    /// Check if this is a test database instance (used by completion system for mock data)
    /// This is separate from has_database_connection to cleanly separate test vs production logic
    pub fn is_test_instance(&self) -> bool {
        // Test instances have no database client (created with new_for_test)
        self.database_client.is_none()
    }

    /// Get the current connection information
    /// The `--read-only` statement guard for this connection: InfluxDB is judged
    /// in the language its connection speaks (InfluxQL, Flux or SQL), every
    /// other engine by its engine-wide classifier.
    fn check_read_only_query(
        &self,
        query: &str,
    ) -> std::result::Result<(), crate::safety::ReadOnlyViolation> {
        let database_type = self.get_database_type();
        if database_type == crate::database::DatabaseType::InfluxDB
            && let Some(info) = self.get_connection_info()
        {
            return crate::safety::check_influxdb_read_only(
                query,
                crate::database::InfluxLanguage::from_options(&info.options),
            );
        }
        crate::safety::check_read_only(query, &database_type)
    }

    pub fn get_connection_info(&self) -> Option<&crate::database::ConnectionInfo> {
        // Check override first (for Vault connections), then fall back to database client
        if let Some(ref override_info) = self.connection_info_override {
            Some(override_info)
        } else {
            self.database_client
                .as_ref()
                .map(|client| client.get_connection_info())
        }
    }

    /// Get the current database type
    pub fn get_database_type(&self) -> DatabaseType {
        self.get_connection_info()
            .map(|info| info.database_type.clone())
            .unwrap_or(DatabaseType::PostgreSQL)
    }

    /// Set or override the connection information for this database
    /// This is useful for cases like Vault connections where the connection info
    /// needs to be set after database creation
    pub fn set_connection_info_override(
        &mut self,
        connection_info: crate::database::ConnectionInfo,
    ) {
        self.connection_info_override = Some(connection_info);
    }

    pub async fn list_tables(
        &mut self,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        // Use the new database abstraction layer
        if let Some(ref database_client) = self.database_client {
            let tables = database_client
                .get_metadata_provider()
                .get_tables(None)
                .await?;

            let mut results = Vec::new();
            // Add header row
            results.push(vec![
                "Schema".to_string(),
                "Name".to_string(),
                "Type".to_string(),
                "Owner".to_string(),
            ]);

            // Add table/collection rows. Providers may qualify objects outside
            // the default schema (for example `analytics.orders` in PostgreSQL).
            // Engines whose objects are not all tables (Redis key types)
            // name each one's kind; an engine that says nothing keeps the
            // "table"/"collection" labels below
            let kinds = database_client
                .get_metadata_provider()
                .get_table_kinds(&tables)
                .await
                .unwrap_or_default();

            for table_reference in tables {
                let conn_info = database_client.get_connection_info();
                let (explicit_schema, table) = if matches!(
                    conn_info.database_type,
                    crate::database::DatabaseType::PostgreSQL
                        | crate::database::DatabaseType::SqlServer
                        | crate::database::DatabaseType::Redshift
                ) {
                    metadata_table_parts(&table_reference, &conn_info.database_type)
                } else if matches!(
                    conn_info.database_type,
                    crate::database::DatabaseType::Redis
                        | crate::database::DatabaseType::S3
                        | crate::database::DatabaseType::DynamoDB
                        | crate::database::DatabaseType::CloudWatchLogs
                ) {
                    // A key name is not an identifier: keep quotes and brackets
                    (None, table_reference.clone())
                } else {
                    (None, unquote_metadata_identifier(&table_reference))
                };
                let schema_name =
                    if conn_info.database_type == crate::database::DatabaseType::MongoDB {
                        "".to_string() // MongoDB doesn't have schemas
                    } else {
                        explicit_schema.unwrap_or_else(|| {
                            database_client
                                .get_metadata_provider()
                                .default_schema()
                                .unwrap_or_else(|| "main".to_string())
                        })
                    };

                let object_type = if let Some(kind) = kinds.get(&table_reference) {
                    kind.clone()
                } else if conn_info.database_type == crate::database::DatabaseType::MongoDB {
                    "collection".to_string()
                } else if matches!(
                    conn_info.database_type,
                    crate::database::DatabaseType::Redis | crate::database::DatabaseType::S3
                ) {
                    "key".to_string()
                } else {
                    "table".to_string()
                };

                results.push(vec![schema_name, table, object_type, self.get_username()]);
            }

            Ok(results)
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn execute_query(
        &mut self,
        query: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        self.execute_query_with_interrupt(
            query,
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
    }

    /// Execute a query preserving per-cell NULL-ness (for structured
    /// consumers like the GUI). Shares the read-only guard, explain mode,
    /// and auto-LIMIT with the display path, but never runs interactive
    /// column selection.
    pub async fn execute_query_structured(
        &mut self,
        query: &str,
    ) -> std::result::Result<crate::database::StructuredQueryResult, Box<dyn StdError>> {
        if self.read_only {
            self.check_read_only_query(query)?;
        }

        if self.explain_mode && is_query_explainable(query) {
            let results = self.execute_explain_query(query).await?;
            return Ok(crate::database::StructuredQueryResult::from_display_rows(
                results,
            ));
        }

        if let Some(ref database_client) = self.database_client {
            let query_with_limit = self.maybe_add_limit(query);
            Ok(database_client
                .execute_query_structured(&query_with_limit)
                .await?)
        } else {
            Err("No database client available".into())
        }
    }

    /// Test query execution without side effects (for validating named queries before saving)
    pub async fn test_query_execution(
        &mut self,
        query: &str,
    ) -> std::result::Result<(), Box<dyn StdError>> {
        if let Some(ref database_client) = self.database_client {
            // For file-based databases (SQLite), we can't use transactions in the same way
            // so we'll just do a basic validation query execution
            database_client
                .test_query(query)
                .await
                .map_err(|e| e.into())
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn execute_query_with_info(
        &mut self,
        query: &str,
    ) -> std::result::Result<QueryResultsWithInfo, Box<dyn StdError>> {
        self.execute_query_with_interrupt_and_info(
            query,
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await
    }

    pub async fn execute_query_with_info_no_column_selection(
        &mut self,
        query: &str,
    ) -> std::result::Result<QueryResultsWithInfo, Box<dyn StdError>> {
        // Temporarily disable column selection
        let original_cs_mode = self.column_select_mode;
        let original_threshold = self.column_selection_threshold;

        self.column_select_mode = false;
        self.column_selection_threshold = usize::MAX; // Effectively disable auto-triggering

        let result = self
            .execute_query_with_interrupt_and_info(
                query,
                &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .await;

        // Restore original settings
        self.column_select_mode = original_cs_mode;
        self.column_selection_threshold = original_threshold;

        result
    }

    pub async fn execute_query_with_interrupt(
        &mut self,
        query: &str,
        interrupt_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        // Use the new method that returns metadata and extract just the data
        match self
            .execute_query_with_interrupt_and_info(query, interrupt_flag)
            .await
        {
            Ok(results_with_info) => Ok(results_with_info.data),
            Err(e) => Err(e),
        }
    }

    /// Like [`Database::execute_query_with_interrupt`], but with interactive
    /// column selection fully disabled (both the explicit mode and the
    /// auto-enable threshold) while preserving the interrupt flag. For
    /// non-interactive callers like the AI agent, which must never raise an
    /// `inquire` prompt mid-query (it would hang a background thread, and is wrong
    /// during an autonomous investigation in the REPL too).
    pub async fn execute_query_with_interrupt_no_column_selection(
        &mut self,
        query: &str,
        interrupt_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        let original_cs_mode = self.column_select_mode;
        let original_threshold = self.column_selection_threshold;
        self.column_select_mode = false;
        self.column_selection_threshold = usize::MAX; // disable auto-triggering

        let result = self
            .execute_query_with_interrupt(query, interrupt_flag)
            .await;

        self.column_select_mode = original_cs_mode;
        self.column_selection_threshold = original_threshold;
        result
    }

    pub async fn execute_query_with_interrupt_and_info(
        &mut self,
        query: &str,
        interrupt_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::result::Result<QueryResultsWithInfo, Box<dyn StdError>> {
        // --read-only guard: this is the single choke point every execution
        // path funnels through (-c/-f/stdin, REPL, named queries, AI run_sql,
        // Python API)
        if self.read_only {
            self.check_read_only_query(query)?;
        }

        // Check if we should EXPLAIN this query (applies to all database types)
        if self.explain_mode && is_query_explainable(query) {
            debug!("EXPLAIN mode is enabled, executing EXPLAIN query");

            // Check if TUI mode is enabled (PostgreSQL only for now)
            #[cfg(feature = "explain-tui")]
            if self.explain_tui_mode && self.get_database_type() == DatabaseType::PostgreSQL {
                debug!("TUI mode is enabled, launching visualizer");
                return self.execute_explain_with_tui(query).await;
            }

            let results = self.execute_explain_query(query).await?;
            return Ok(QueryResultsWithInfo {
                data: results,
                column_info: None,
            });
        }

        // Use new database abstraction layer
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for execute_query");
            let query_with_limit = self.maybe_add_limit(query);
            debug!("[database_client] Original query: {}", query);
            debug!("[database_client] Query with limit: {}", query_with_limit);
            let results = database_client.execute_query(&query_with_limit).await?;
            self.apply_column_selection_if_needed_with_info(results, interrupt_flag)
        } else {
            Err("No database client available".into())
        }
    }

    fn apply_column_selection_if_needed_with_info(
        &mut self,
        results: Vec<Vec<String>>,
        interrupt_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::result::Result<QueryResultsWithInfo, Box<dyn StdError>> {
        if results.is_empty() {
            return Ok(QueryResultsWithInfo {
                data: results,
                column_info: None,
            });
        }

        let column_count = results[0].len();

        // Check if we should apply column selection
        let should_apply = self.frontend_mode.allows_interactive_terminal_ui()
            && (self.column_select_mode || self.should_auto_enable_column_selection(column_count));

        if should_apply {
            debug!(
                "Applying column selection: cs_mode={}, columns={}, threshold={}",
                self.column_select_mode, column_count, self.column_selection_threshold
            );
            match self.interactive_column_selection_with_info(&results, interrupt_flag) {
                Ok(results_with_info) => Ok(results_with_info),
                Err(e) if e.to_string().contains("Column selection aborted") => {
                    // Re-throw the abort error to propagate it up
                    Err(e)
                }
                Err(e) => {
                    // For other errors, log and return original results
                    debug!("Column selection error: {e}");
                    Ok(QueryResultsWithInfo {
                        data: results,
                        column_info: None,
                    })
                }
            }
        } else {
            Ok(QueryResultsWithInfo {
                data: results,
                column_info: None,
            })
        }
    }

    /// Execute an EXPLAIN query and display in the TUI visualizer
    #[cfg(feature = "explain-tui")]
    async fn execute_explain_with_tui(
        &mut self,
        query: &str,
    ) -> std::result::Result<QueryResultsWithInfo, Box<dyn StdError>> {
        if let Some(ref database_client) = self.database_client {
            debug!("Executing EXPLAIN with TUI visualizer");

            // Get the raw JSON plan
            let raw_results = database_client.explain_query_raw(query).await?;

            // Parse the JSON from results
            if raw_results.len() > 1 && !raw_results[1].is_empty() {
                let json_str = &raw_results[1][0];

                // Store for \ecopy
                self.last_json_plan = Some(json_str.clone());

                // Parse the JSON
                let plan_json: serde_json::Value = serde_json::from_str(json_str)
                    .map_err(|e| format!("Failed to parse EXPLAIN JSON: {e}"))?;

                // Run the TUI visualizer
                match crate::explain_tui::run_explain_tui(&plan_json) {
                    Ok(_) => {
                        // TUI exited normally, return empty result to indicate success
                        Ok(QueryResultsWithInfo {
                            data: vec![vec!["Plan visualizer closed.".to_string()]],
                            column_info: None,
                        })
                    }
                    Err(e) => {
                        // TUI failed, fall back to formatted output
                        eprintln!("TUI error: {e}. Falling back to text output.");
                        let results = database_client.explain_query(query).await?;
                        Ok(QueryResultsWithInfo {
                            data: results,
                            column_info: None,
                        })
                    }
                }
            } else {
                Err("No EXPLAIN output received".into())
            }
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn execute_explain_query(
        &mut self,
        query: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        // Use new database abstraction layer for EXPLAIN queries
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for execute_explain_query");

            // First get the raw JSON plan for \ecopy
            match database_client.explain_query_raw(query).await {
                Ok(raw_results) => {
                    // Store the raw JSON plan for \ecopy command
                    if raw_results.len() > 1 && !raw_results[1].is_empty() {
                        let json_plan = &raw_results[1][0]; // First data row, first column contains JSON
                        self.last_json_plan = Some(json_plan.clone());
                        debug!(
                            "Stored JSON plan for \\ecopy ({} characters)",
                            json_plan.len()
                        );
                    } else {
                        debug!("No JSON plan data found in raw results");
                    }
                }
                Err(e) => {
                    debug!("Failed to get raw JSON plan: {e}");
                    // Continue with formatted query even if raw JSON fails
                }
            }

            return database_client
                .explain_query(query)
                .await
                .map_err(|e| e.into());
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn execute_explain_query_raw(
        &mut self,
        query: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        // Use new database abstraction layer for raw EXPLAIN queries
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for execute_explain_query_raw");
            return database_client
                .explain_query_raw(query)
                .await
                .map_err(|e| e.into());
        } else {
            Err("No database client available".into())
        }
    }

    /// `EXPLAIN ANALYZE` returning the engine's raw plan (PostgreSQL JSON).
    /// Executes the statement, so it honours the read-only guard and refuses
    /// statements the guard classifies as writes even when the guard is off.
    pub async fn execute_explain_analyze_query_raw(
        &mut self,
        query: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        self.check_read_only_query(query)?;
        match self.database_client {
            Some(ref database_client) => database_client
                .explain_analyze_query_raw(query)
                .await
                .map_err(|e| e.into()),
            None => Err("No database client available".into()),
        }
    }

    pub async fn execute_explain_query_formatted(
        &mut self,
        query: &str,
    ) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        // Use new database abstraction layer for formatted EXPLAIN queries
        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for execute_explain_query_formatted");

            // First get the raw JSON plan for \ecopy
            match database_client.explain_query_raw(query).await {
                Ok(raw_results) => {
                    // Store the raw JSON plan for \ecopy command
                    if raw_results.len() > 1 && !raw_results[1].is_empty() {
                        let json_plan = &raw_results[1][0]; // First data row, first column contains JSON
                        self.last_json_plan = Some(json_plan.clone());
                        debug!(
                            "Stored JSON plan for \\ecopy ({} characters)",
                            json_plan.len()
                        );
                    } else {
                        debug!("No JSON plan data found in raw results");
                    }
                }
                Err(e) => {
                    debug!("Failed to get raw JSON plan: {e}");
                    // Continue with formatted query even if raw JSON fails
                }
            }

            // Then get the formatted output for display
            let results = database_client.explain_query(query).await?;
            Ok(results)
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn validate_connection(&self) -> std::result::Result<(), Box<dyn StdError>> {
        if let Some(ref database_client) = self.database_client {
            // Use the database client's connection validation
            if database_client.is_connected().await {
                Ok(())
            } else {
                Err("Database connection is not active".into())
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Display server information to the user (pgcli-style)
    pub async fn display_server_info(&self) {
        if !self.frontend_mode.allows_stdout_status() {
            return;
        }

        if let Some(ref database_client) = self.database_client {
            match database_client.get_server_info().await {
                Ok(server_info) => {
                    // Display server info in pgcli style
                    println!(
                        "Server: {} {}",
                        server_info.server_type, server_info.server_version
                    );
                    println!("Version: {}", server_info.client_version);

                    // Optionally show additional database-specific info in debug mode
                    if !server_info.additional_info.is_empty() {
                        debug!(
                            "[Database::display_server_info] Additional server info: {:?}",
                            server_info.additional_info
                        );
                    }
                }
                Err(e) => {
                    debug!(
                        "[Database::display_server_info] Failed to get server info: {}",
                        e
                    );
                    // Don't show error to user since this is not critical - just log it
                }
            }
        }
    }

    pub fn maybe_add_limit(&self, query: &str) -> String {
        add_engine_default_limit(&self.get_database_type(), query, self.default_limit)
    }

    pub fn is_autocomplete(&self) -> bool {
        self.autocomplete_enabled
    }

    pub fn set_autocomplete(&mut self, enabled: bool) {
        self.autocomplete_enabled = enabled;
    }

    pub fn is_expanded_display(&self) -> bool {
        self.expanded_display
    }

    pub fn toggle_expanded_display(&mut self) -> bool {
        self.expanded_display = !self.expanded_display;
        self.expanded_display
    }

    pub fn set_expanded_display(&mut self, enabled: bool) {
        self.expanded_display = enabled;
    }

    pub fn output_format(&self) -> crate::cli::OutputFormat {
        self.output_format
    }

    pub fn set_output_format(&mut self, format: crate::cli::OutputFormat) {
        self.output_format = format;
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, enabled: bool) {
        self.read_only = enabled;
    }

    pub fn is_explain_mode(&self) -> bool {
        self.explain_mode
    }

    pub fn toggle_explain_mode(&mut self) -> bool {
        self.explain_mode = !self.explain_mode;
        self.explain_mode
    }

    /// Check if TUI explain visualizer mode is enabled
    pub fn is_explain_tui_mode(&self) -> bool {
        self.explain_tui_mode
    }

    /// Toggle TUI explain visualizer mode
    pub fn toggle_explain_tui_mode(&mut self) -> bool {
        self.explain_tui_mode = !self.explain_tui_mode;
        // If enabling TUI mode, also enable explain mode
        if self.explain_tui_mode {
            self.explain_mode = true;
        }
        self.explain_tui_mode
    }

    /// Test network connectivity to a host:port combination with timeout
    pub async fn test_network_connectivity(
        host: &str,
        port: u16,
        timeout_secs: u64,
    ) -> std::result::Result<(), Box<dyn StdError>> {
        debug!(
            "[Database::test_network_connectivity] Testing connection to {}:{}",
            host, port
        );

        // Test DNS resolution first
        match tokio::net::lookup_host(format!("{host}:{port}")).await {
            Ok(mut addresses) => {
                if addresses.next().is_none() {
                    return Err(
                        format!("DNS resolution failed: no addresses found for {host}").into(),
                    );
                }
                debug!(
                    "[Database::test_network_connectivity] DNS resolution successful for {}",
                    host
                );
            }
            Err(e) => {
                return Err(format!("DNS resolution failed for {host}: {e}").into());
            }
        }

        // Test TCP connectivity with timeout
        let timeout = std::time::Duration::from_secs(timeout_secs);
        match tokio::time::timeout(
            timeout,
            tokio::net::TcpStream::connect(format!("{host}:{port}")),
        )
        .await
        {
            Ok(Ok(_)) => {
                debug!(
                    "[Database::test_network_connectivity] TCP connection successful to {}:{}",
                    host, port
                );
                Ok(())
            }
            Ok(Err(e)) => Err(format!("TCP connection failed to {host}:{port}: {e}").into()),
            Err(_) => Err(format!(
                "Connection timeout to {host}:{port} after {timeout_secs} seconds"
            )
            .into()),
        }
    }

    pub async fn get_table_details(
        &mut self,
        table_name: &str,
    ) -> std::result::Result<TableDetails, Box<dyn StdError>> {
        // Use the new database abstraction layer if available
        if let Some(ref database_client) = self.database_client {
            let database_type = &database_client.get_connection_info().database_type;
            let provider = database_client.get_metadata_provider();
            let (schema, table) = metadata_table_parts(table_name, database_type);
            let mut result = provider.get_table_details(&table, schema.as_deref()).await;

            // A dot inside a bare table name parses as `schema.table` above.
            // When that lookup fails (SQLite errors on the phantom schema) or
            // comes back empty (PostgreSQL returns zero columns), retry the
            // name verbatim in the default schema.
            if schema.is_some() && !matches!(result, Ok(ref details) if !details.columns.is_empty())
            {
                let verbatim = unquote_metadata_identifier(table_name);
                if let Ok(details) = provider.get_table_details(&verbatim, None).await {
                    if !details.columns.is_empty() {
                        result = Ok(details);
                    }
                }
            }

            match result {
                Ok(table_details) => Ok(table_details),
                Err(e) => {
                    debug!("Error using database client for get_table_details: {e}");
                    Err(Box::new(e))
                }
            }
        } else {
            Err("No database client available".into())
        }
    }

    /// Like [`Database::get_table_details`], but for an explicit schema (e.g. the
    /// agent's `describe_table` tool resolving `analytics.orders`). `None` schema
    /// uses the backend default (PostgreSQL: `public` / search_path).
    pub async fn get_table_details_in_schema(
        &mut self,
        table_name: &str,
        schema: Option<&str>,
    ) -> std::result::Result<TableDetails, Box<dyn StdError>> {
        if let Some(ref database_client) = self.database_client {
            database_client
                .get_metadata_provider()
                .get_table_details(table_name, schema)
                .await
                .map_err(|e| Box::new(e) as Box<dyn StdError>)
        } else {
            Err("No database client available".into())
        }
    }

    /// Fetch details for many tables concurrently, preserving input order.
    ///
    /// `get_table_details` is 5-6 catalog round-trips per table; doing them
    /// sequentially for a whole schema is painfully slow over an SSH tunnel.
    /// The connection pool allows several concurrent queries, so fan the
    /// per-table fetches out (bounded) instead. A table whose details fail to
    /// load comes back as `None` so callers can degrade gracefully rather than
    /// aborting the whole context build.
    pub async fn get_table_details_bulk(
        &self,
        table_names: &[String],
    ) -> Vec<(String, Option<TableDetails>)> {
        use futures_util::stream::StreamExt;

        let Some(ref database_client) = self.database_client else {
            return table_names
                .iter()
                .map(|name| (name.clone(), None))
                .collect();
        };

        let provider = database_client.get_metadata_provider();
        let database_type = &database_client.get_connection_info().database_type;
        // Bounded concurrency: stay within the pool's connection budget.
        const MAX_CONCURRENT: usize = 8;
        let mut indexed: Vec<(usize, String, Option<TableDetails>)> =
            futures_util::stream::iter(table_names.iter().enumerate())
                .map(|(idx, name)| async move {
                    // Accept schema-qualified and quoted names while preserving
                    // dots that are intrinsic to schemaless backend objects.
                    let (schema, table) = metadata_table_parts(name, database_type);
                    let mut details = provider
                        .get_table_details(&table, schema.as_deref())
                        .await
                        .ok();
                    // Dotted bare names parse as schema.table; retry verbatim
                    // when that lookup finds nothing.
                    if schema.is_some() && details.as_ref().is_none_or(|d| d.columns.is_empty()) {
                        if let Ok(fallback) = provider
                            .get_table_details(&unquote_metadata_identifier(name), None)
                            .await
                        {
                            if !fallback.columns.is_empty() {
                                details = Some(fallback);
                            }
                        }
                    }
                    (idx, name.clone(), details)
                })
                .buffer_unordered(MAX_CONCURRENT)
                .collect()
                .await;

        // buffer_unordered yields in completion order; restore input order so the
        // generated DDL is deterministic across runs.
        indexed.sort_by_key(|(idx, _, _)| *idx);
        indexed
            .into_iter()
            .map(|(_, name, details)| (name, details))
            .collect()
    }

    pub fn new_for_test() -> Self {
        let config = crate::config::Config::load();
        Self {
            database_client: None, // No database client in test mode
            connection_info_override: None,
            ssh_tunnel: None, // No SSH tunnel in test mode
            #[cfg(feature = "docker")]
            docker_tunnel: None,
            #[cfg(feature = "docker")]
            docker_route: None,
            #[cfg(feature = "docker")]
            docker_daemon: None,
            expanded_display: false,
            output_format: crate::cli::OutputFormat::Table,
            read_only: false,
            default_limit: 100,
            autocomplete_enabled: config.autocomplete_enabled,
            explain_mode: false,
            explain_tui_mode: false,
            column_select_mode: false,
            banner_enabled: config.show_banner,
            column_selection_threshold: config.column_selection_threshold,
            column_selection_default_all: config.column_selection_default_all,
            column_views: HashMap::new(),
            last_view_key: None,
            last_json_plan: None,
            frontend_mode: FrontendMode::Cli,
        }
    }

    pub async fn list_database_names(
        &mut self,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        // Use new database abstraction layer
        if let Some(ref database_client) = self.database_client {
            let databases = database_client.list_databases().await?;
            // Extract just the database names (first column)
            let names: Vec<String> = databases
                .into_iter()
                .skip(1)
                .map(|row| row[0].clone())
                .collect();
            Ok(names)
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn get_tables_and_views(
        &mut self,
        schema_filter: Option<&str>,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        let start_time = std::time::Instant::now();
        debug!(
            "[get_tables_and_views] Starting query for schema_filter: {:?}",
            schema_filter
        );

        // Use the new database abstraction layer if available
        if let Some(ref database_client) = self.database_client {
            debug!("[get_tables_and_views] Using new database abstraction layer");
            match database_client
                .get_metadata_provider()
                .get_tables(schema_filter)
                .await
            {
                Ok(tables) => {
                    let duration = start_time.elapsed();
                    debug!(
                        "[get_tables_and_views] Database abstraction layer returned {} tables in {:?}",
                        tables.len(),
                        duration
                    );
                    Ok(tables)
                }
                Err(e) => {
                    debug!(
                        "Error using database client for get_tables_and_views: {}",
                        e
                    );
                    Err(Box::new(e))
                }
            }
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn get_schemas(&mut self) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        let start_time = std::time::Instant::now();
        debug!("[get_schemas] Starting query");

        // Use the new database abstraction layer if available
        if let Some(ref database_client) = self.database_client {
            debug!("[get_schemas] Using new database abstraction layer");
            match database_client.get_metadata_provider().get_schemas().await {
                Ok(schemas) => {
                    let duration = start_time.elapsed();
                    debug!(
                        "[get_schemas] Database abstraction layer returned {} schemas in {:?}",
                        schemas.len(),
                        duration
                    );
                    Ok(schemas)
                }
                Err(e) => {
                    debug!("Error using database client for get_schemas: {e}");
                    Err(Box::new(e))
                }
            }
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn get_functions(
        &mut self,
        schema_filter: Option<&str>,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        // Use new database abstraction layer
        if let Some(ref database_client) = self.database_client {
            let functions = database_client
                .get_metadata_provider()
                .get_functions(schema_filter)
                .await?;
            Ok(functions)
        } else {
            Err("No database client available".into())
        }
    }

    pub async fn get_columns_for_table(
        &mut self,
        table_name: &str,
        schema: Option<&str>,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        // Use new database abstraction layer
        if let Some(ref database_client) = self.database_client {
            let columns = database_client
                .get_metadata_provider()
                .get_columns(table_name, schema)
                .await?;
            Ok(columns)
        } else {
            Err("No database client available".into())
        }
    }

    /// Simplified column getter for the new completion system
    pub async fn get_columns(
        &mut self,
        table_name: &str,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        let database_type = self.get_database_type();
        let (schema, table) = metadata_table_parts(table_name, &database_type);
        let primary = self.get_columns_for_table(&table, schema.as_deref()).await;
        match primary {
            Ok(columns) if !columns.is_empty() => Ok(columns),
            primary_result if schema.is_some() => {
                // A dot inside a bare table name parses as `schema.table`;
                // retry the name verbatim in the default schema.
                let verbatim = unquote_metadata_identifier(table_name);
                match self.get_columns_for_table(&verbatim, None).await {
                    Ok(columns) if !columns.is_empty() => Ok(columns),
                    _ => primary_result,
                }
            }
            primary_result => primary_result,
        }
    }

    pub fn is_column_select_mode(&self) -> bool {
        self.column_select_mode
    }

    pub fn toggle_column_select_mode(&mut self) -> bool {
        self.column_select_mode = !self.column_select_mode;
        self.column_select_mode
    }

    pub fn is_banner_enabled(&self) -> bool {
        self.banner_enabled
    }

    pub fn toggle_banner_enabled(&mut self) -> bool {
        self.banner_enabled = !self.banner_enabled;
        self.banner_enabled
    }

    pub fn save_column_view(&mut self, view_name: &str, columns: Vec<String>) {
        self.column_views.insert(view_name.to_string(), columns);
    }

    pub fn get_column_view(&self, view_name: &str) -> Option<&Vec<String>> {
        self.column_views.get(view_name)
    }

    pub fn generate_column_view_key(&self, headers: &[String]) -> String {
        headers.join(":")
    }

    // Note: interactive_column_selection method implementation is below

    // Utility methods for column view management

    pub fn get_last_json_plan(&self) -> Option<String> {
        self.last_json_plan.clone()
    }

    pub fn clear_column_views(&mut self) {
        self.column_views.clear();
        self.last_view_key = None;
    }

    pub fn reset_column_view(&mut self) {
        if let Some(last_view_key) = &self.last_view_key {
            self.column_views.remove(last_view_key);
        }
        self.last_view_key = None;
    }

    pub fn set_column_selection_threshold(&mut self, threshold: usize) {
        self.column_selection_threshold = threshold;
    }

    pub fn should_auto_enable_column_selection(&self, column_count: usize) -> bool {
        // Auto-enable column selection mode if there are more columns than the threshold
        column_count > self.column_selection_threshold
    }

    pub fn interactive_column_selection_with_info(
        &mut self,
        data: &[Vec<String>],
        interrupt_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<QueryResultsWithInfo, Box<dyn StdError>> {
        if data.is_empty() {
            return Ok(QueryResultsWithInfo {
                data: data.to_vec(),
                column_info: None,
            });
        }

        // Get the headers from the first row
        let headers = &data[0];
        let view_key = self.generate_column_view_key(headers);

        // Check if we have a saved view for this column set
        let selected_columns = if let Some(saved_columns) = self.get_column_view(&view_key) {
            saved_columns.clone()
        } else {
            // Create options for the multi-select
            let options: Vec<String> = headers.clone();

            // Build the prompt
            let prompt_message = format!(
                "Select columns to display ({} columns available):",
                headers.len()
            );

            // Create the multi-select
            let mut selector = MultiSelect::new(&prompt_message, options.clone())
                .with_help_message("Use arrow keys to navigate, Space to select/deselect, Enter to confirm, Ctrl-C to cancel")
                .with_vim_mode(false);

            // Pre-select columns based on configuration
            let all_indices: Vec<usize> = if self.column_selection_default_all {
                // Pre-select all columns (opt-out behavior)
                (0..headers.len()).collect()
            } else {
                // Start with no columns selected (opt-in behavior)
                Vec::new()
            };

            if !all_indices.is_empty() {
                selector = selector.with_default(&all_indices);
            }

            // Handle the selection with interrupt support
            let selection_result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| selector.prompt()));

            match selection_result {
                Ok(Ok(selected)) => {
                    if selected.is_empty() {
                        // If no columns selected, show all columns
                        headers.clone()
                    } else {
                        selected
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    // User cancelled with Ctrl-C or error occurred
                    if interrupt_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(Box::new(ColumnSelectionAborted));
                    }
                    // For other errors, return all columns
                    headers.clone()
                }
            }
        };

        // Save the selection for future use
        self.save_column_view(&view_key, selected_columns.clone());
        self.last_view_key = Some(view_key);

        // Get column indices for selected columns
        let column_indices: Vec<usize> = selected_columns
            .iter()
            .filter_map(|col| headers.iter().position(|h| h == col))
            .collect();

        if column_indices.is_empty() {
            // If no valid columns found, return all data
            return Ok(QueryResultsWithInfo {
                data: data.to_vec(),
                column_info: None,
            });
        }

        // Filter the data to include only selected columns
        let filtered_data: Vec<Vec<String>> = data
            .iter()
            .map(|row| {
                column_indices
                    .iter()
                    .map(|&idx| row.get(idx).cloned().unwrap_or_default())
                    .collect()
            })
            .collect();

        // Create column info for selected columns
        let column_info = Some(ColumnFilteringInfo {
            total_columns: headers.len(),
            displayed_columns: selected_columns.len(),
            filtered_column_names: selected_columns.clone(),
        });

        Ok(QueryResultsWithInfo {
            data: filtered_data,
            column_info,
        })
    }
}

// Helper function to determine if a query can be explained
fn is_query_explainable(query: &str) -> bool {
    let query = query.trim().to_lowercase();

    // Only try to EXPLAIN statements that make sense
    // Only SELECT and WITH queries should be explainable
    query.starts_with("select") || query.starts_with("with")
}

/// First SQL keyword of a statement, lowercased, skipping leading whitespace
/// and `--`/`/* */` comments.
fn leading_sql_keyword(query: &str) -> Option<String> {
    let mut rest = query;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map(|(_, tail)| tail).unwrap_or("");
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map(|(_, tail)| tail)?;
        } else {
            break;
        }
    }
    let keyword: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if keyword.is_empty() {
        None
    } else {
        Some(keyword.to_lowercase())
    }
}

/// The auto-LIMIT of an engine's dialect.
fn add_engine_default_limit(
    db_type: &crate::database::DatabaseType,
    query: &str,
    default_limit: usize,
) -> String {
    match db_type {
        crate::database::DatabaseType::SqlServer => add_default_top(query, default_limit),
        crate::database::DatabaseType::InfluxDB => add_default_limit_influxql(query, default_limit),
        // PartiQL for DynamoDB has no LIMIT clause (the service rejects it): the
        // driver bounds every read itself (`max_rows`, `max_pages`)
        crate::database::DatabaseType::DynamoDB => query.to_string(),
        // Logs Insights queries are not SQL: they carry their own `limit` command
        // and the driver passes `max_rows` as the query's result limit
        crate::database::DatabaseType::CloudWatchLogs => query.to_string(),
        _ => add_default_limit(query, default_limit),
    }
}

/// Append `LIMIT n` only to statements whose leading keyword is SELECT.
/// Anything that merely *contains* a SELECT (INSERT … SELECT, CREATE TABLE AS,
/// DELETE/UPDATE with subqueries) must stay untouched: a trailing LIMIT there
/// silently truncates writes or produces syntax errors.
fn add_default_limit(query: &str, default_limit: usize) -> String {
    if default_limit == 0 || leading_sql_keyword(query).as_deref() != Some("select") {
        return query.to_string();
    }
    let query_lower = query.to_lowercase();
    let has_limit_token = query_lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|token| token == "limit");
    if has_limit_token || query_lower.contains("count(") {
        return query.to_string();
    }
    let query_trimmed = query.trim_end();
    if query_trimmed.ends_with(';') {
        let without_semicolon = query_trimmed.trim_end_matches(';');
        format!("{} LIMIT {};", without_semicolon, default_limit)
    } else {
        format!("{} LIMIT {}", query, default_limit)
    }
}

/// InfluxQL (and v3 SQL) take a trailing `LIMIT`, but `LIMIT` must precede
/// `OFFSET` / `SLIMIT` / `SOFFSET` / `tz()` in InfluxQL: a query that already
/// has one of those, or writes with `INTO`, is left alone (the driver's
/// `max_rows` still caps what is read). Flux programs never start with
/// `SELECT`, so they are never touched.
fn add_default_limit_influxql(query: &str, default_limit: usize) -> String {
    let lower = query.to_lowercase();
    let has_clause = lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|token| {
            matches!(
                token,
                "offset" | "slimit" | "soffset" | "tz" | "into" | "fetch"
            )
        });
    if has_clause {
        return query.to_string();
    }
    add_default_limit(query, default_limit)
}

/// T-SQL has no `LIMIT`: cap a plain single `SELECT` with `TOP (n)` right after
/// `SELECT [DISTINCT|ALL]`. Anything that already bounds itself (TOP, FETCH,
/// OFFSET) or where a leading TOP would not bound the whole statement (UNION,
/// INTERSECT, EXCEPT, INTO, FOR XML/JSON, several statements) is left alone —
/// the driver's `max_rows` still caps what is read.
fn add_default_top(query: &str, default_limit: usize) -> String {
    const UNTOUCHED: [&str; 8] = [
        "top",
        "fetch",
        "offset",
        "union",
        "intersect",
        "except",
        "into",
        "for",
    ];
    let trimmed = query.trim_start();
    let starts_with_select = trimmed
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("select"))
        && trimmed[6..]
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '(' || c == '*');
    if default_limit == 0 || !starts_with_select {
        return query.to_string();
    }
    let lower = query.to_lowercase();
    let words = || {
        lower
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| !word.is_empty())
    };
    if words().any(|word| UNTOUCHED.contains(&word))
        || lower.contains("count(")
        || query.trim_end().trim_end_matches(';').contains(';')
    {
        return query.to_string();
    }
    let select_end = query.len() - trimmed.len() + 6;
    let rest = &query[select_end..];
    let after_space = rest.trim_start();
    let next_word: String = after_space
        .chars()
        .take_while(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    let insert_at = if next_word == "distinct" || next_word == "all" {
        select_end + (rest.len() - after_space.len()) + next_word.len()
    } else {
        select_end
    };
    format!(
        "{} TOP ({default_limit}){}",
        &query[..insert_at],
        &query[insert_at..]
    )
}

/// The catalog lookups `\ddl` (and the AI assistant, through its own
/// adapter) use to list tables across schemas.
#[async_trait::async_trait(?Send)]
impl crate::ddl::TableCatalog for Database {
    async fn catalog_schemas(&mut self) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        self.get_schemas().await
    }

    async fn catalog_tables(
        &mut self,
        schema_filter: Option<&str>,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        self.get_tables_and_views(schema_filter).await
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if let Some(_ssh_tunnel) = &mut self.ssh_tunnel {
            // The SSH tunnel will be automatically cleaned up when dropped
            // due to its internal Drop implementation
            debug!("Cleaning up SSH tunnel on Database drop");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // -- docker:// pinning (the connect flow lives here, the daemon code in
    // `dbcrust-docker`) ---------------------------------------------------

    // -- builds without an add-on or a connector ---------------------------

    #[cfg(not(feature = "docker"))]
    #[tokio::test]
    async fn docker_urls_say_docker_is_not_compiled() {
        let error = Database::from_url("docker://some-container/app", None, None)
            .await
            .err()
            .expect("docker:// cannot connect without the feature");
        assert_eq!(
            error.downcast_ref::<crate::features::FeatureNotCompiled>(),
            Some(&crate::features::FeatureNotCompiled::Docker)
        );
        assert!(error.to_string().contains("`docker` cargo feature"));
    }

    #[cfg(not(feature = "mongodb"))]
    #[tokio::test]
    async fn a_disabled_engine_says_which_feature_provides_it() {
        let error = Database::from_url("mongodb://localhost:27017/app", None, None)
            .await
            .err()
            .expect("MongoDB cannot connect without the feature");
        let text = error.to_string();
        assert!(
            text.contains("MongoDB support is not compiled into this build"),
            "{text}"
        );
        assert!(text.contains("`mongodb` cargo feature"), "{text}");
    }

    #[cfg(not(feature = "files"))]
    #[tokio::test]
    async fn file_formats_name_the_files_feature() {
        let error = Database::from_url("csv:///tmp/does-not-matter.csv", None, None)
            .await
            .err()
            .expect("CSV cannot connect without the feature");
        assert!(
            error.to_string().contains("`files` cargo feature"),
            "{error}"
        );
    }

    #[cfg(feature = "docker")]
    #[tokio::test]
    async fn a_pinned_daemon_is_used_whatever_the_current_context_is() {
        // The current daemon (whatever it is here) is not consulted: the
        // pin names a socket that doesn't exist, and that is what fails.
        let mut template = crate::docker_target::DockerTarget::parse("docker://some-container")
            .unwrap()
            .to_connection_info();
        template.options.insert(
            crate::docker_target::PINNED_DAEMON_OPTION.to_string(),
            "unix:///nonexistent/dbcrust-pinned.sock".to_string(),
        );
        let error = match Database::connect_docker_container(
            &template,
            |_| {},
            |_| async { Err::<Database, _>("unreachable".into()) },
        )
        .await
        {
            Ok(_) => panic!("a missing daemon cannot connect"),
            Err(e) => e,
        };
        let text = error.to_string();
        assert!(text.contains("dbcrust-pinned.sock"), "{text}");
        assert!(matches!(
            error.downcast_ref::<crate::docker::DockerError>(),
            Some(crate::docker::DockerError::DaemonUnavailable { .. })
        ));
    }

    #[cfg(feature = "docker")]
    #[test]
    fn a_pin_that_cannot_be_used_is_an_error_not_a_fallback() {
        let mut template = crate::docker_target::DockerTarget::parse("docker://c")
            .unwrap()
            .to_connection_info();
        template.options.insert(
            crate::docker_target::PINNED_DAEMON_OPTION.to_string(),
            "ssh://me@host".to_string(),
        );
        let error = futures_util::FutureExt::now_or_never(Database::connect_docker_container(
            &template,
            |_| {},
            |_| async { Err::<Database, _>("unreachable".into()) },
        ))
        .expect("fails before any await")
        .err()
        .expect("error");
        assert!(matches!(
            error.downcast_ref::<crate::docker::DockerError>(),
            Some(crate::docker::DockerError::Endpoint(_))
        ));
    }

    #[rstest]
    fn test_is_query_explainable() {
        // Simple select statement - should be explainable
        assert!(is_query_explainable("SELECT * FROM users"));
        assert!(is_query_explainable("select * from users"));
        assert!(is_query_explainable("WITH t AS (SELECT 1) SELECT * FROM t"));

        // DDL and other commands - should not be explainable
        assert!(!is_query_explainable("CREATE TABLE users (id INT)"));
        assert!(!is_query_explainable("INSERT INTO users VALUES (1)"));
        assert!(!is_query_explainable("UPDATE users SET name='john'"));
        assert!(!is_query_explainable("DELETE FROM users"));
        assert!(!is_query_explainable("DROP TABLE users"));
        assert!(!is_query_explainable("TRUNCATE users"));
        assert!(!is_query_explainable("BEGIN"));
        assert!(!is_query_explainable("COMMIT"));
        assert!(!is_query_explainable("ROLLBACK"));
        assert!(!is_query_explainable("-- comment only"));
    }

    #[rstest]
    fn test_leading_sql_keyword() {
        assert_eq!(leading_sql_keyword("SELECT 1"), Some("select".into()));
        assert_eq!(
            leading_sql_keyword("  -- comment\n  select 1"),
            Some("select".into())
        );
        assert_eq!(
            leading_sql_keyword("/* block */ INSERT INTO t VALUES (1)"),
            Some("insert".into())
        );
        assert_eq!(leading_sql_keyword("-- only a comment"), None);
        assert_eq!(leading_sql_keyword("/* unterminated"), None);
        assert_eq!(leading_sql_keyword(""), None);
    }

    #[rstest]
    #[case("SELECT * FROM t", "SELECT TOP (100) * FROM t")]
    #[case(
        "select a, b from t where x = 1;",
        "select TOP (100) a, b from t where x = 1;"
    )]
    #[case("  SELECT DISTINCT a FROM t", "  SELECT DISTINCT TOP (100) a FROM t")]
    #[case("SELECT ALL a FROM t", "SELECT ALL TOP (100) a FROM t")]
    #[case("SELECT\n  a\nFROM t", "SELECT TOP (100)\n  a\nFROM t")]
    #[case("SELECT * FROM t -- note", "SELECT TOP (100) * FROM t -- note")]
    fn test_add_default_top_caps_plain_selects(#[case] query: &str, #[case] expected: &str) {
        assert_eq!(add_default_top(query, 100), expected);
    }

    #[rstest]
    #[case("SELECT TOP 5 * FROM t")]
    #[case("SELECT * FROM t ORDER BY a OFFSET 5 ROWS FETCH NEXT 5 ROWS ONLY")]
    #[case("SELECT a FROM t UNION SELECT a FROM u")]
    #[case("SELECT * INTO #copy FROM t")]
    #[case("SELECT * FROM t FOR JSON PATH")]
    #[case("SELECT COUNT(*) FROM t")]
    #[case("SELECT 1; SELECT 2")]
    #[case("WITH c AS (SELECT 1 AS n) SELECT n FROM c")]
    #[case("INSERT INTO t SELECT * FROM s")]
    #[case("SELECTED")]
    #[case("-- c\nSELECT * FROM t")]
    fn test_add_default_top_leaves_everything_else_alone(#[case] query: &str) {
        assert_eq!(add_default_top(query, 100), query);
        assert_eq!(add_default_top("SELECT * FROM t", 0), "SELECT * FROM t");
    }

    #[rstest]
    #[case("SELECT * FROM cpu", "SELECT * FROM cpu LIMIT 100")]
    #[case(
        "SELECT mean(usage) FROM cpu WHERE time > now() - 1h GROUP BY time(5m);",
        "SELECT mean(usage) FROM cpu WHERE time > now() - 1h GROUP BY time(5m) LIMIT 100;"
    )]
    #[case("SELECT * FROM cpu LIMIT 5", "SELECT * FROM cpu LIMIT 5")]
    #[case(
        "SELECT * FROM cpu LIMIT 5 OFFSET 5",
        "SELECT * FROM cpu LIMIT 5 OFFSET 5"
    )]
    #[case(
        "SELECT * FROM cpu GROUP BY * SLIMIT 2",
        "SELECT * FROM cpu GROUP BY * SLIMIT 2"
    )]
    #[case(
        "SELECT * FROM cpu tz('Europe/Paris')",
        "SELECT * FROM cpu tz('Europe/Paris')"
    )]
    #[case("SELECT * INTO copy FROM cpu", "SELECT * INTO copy FROM cpu")]
    #[case("SHOW MEASUREMENTS", "SHOW MEASUREMENTS")]
    #[case(
        "from(bucket: \"b\") |> range(start: -1h)",
        "from(bucket: \"b\") |> range(start: -1h)"
    )]
    fn test_influxql_default_limit_leaves_clauses_alone(
        #[case] query: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(add_default_limit_influxql(query, 100), expected);
    }

    #[test]
    fn test_dynamodb_partiql_gets_no_auto_limit() {
        use crate::database::DatabaseType;
        let sql = "SELECT * FROM \"Orders\" WHERE customer = 'a'";
        assert_eq!(
            add_engine_default_limit(&DatabaseType::DynamoDB, sql, 100),
            sql
        );
        let insights = "fields @message | limit 5";
        assert_eq!(
            add_engine_default_limit(&DatabaseType::CloudWatchLogs, insights, 100),
            insights
        );
        assert_eq!(
            add_engine_default_limit(&DatabaseType::PostgreSQL, "SELECT 1", 100),
            "SELECT 1 LIMIT 100"
        );
    }

    #[rstest]
    fn test_add_default_limit_only_touches_plain_selects() {
        // Plain SELECTs get the limit
        assert_eq!(
            add_default_limit("SELECT * FROM users", 100),
            "SELECT * FROM users LIMIT 100"
        );
        assert_eq!(
            add_default_limit("SELECT * FROM users;", 100),
            "SELECT * FROM users LIMIT 100;"
        );
        // Word-boundary check: identifiers containing "limit" don't suppress it
        assert_eq!(
            add_default_limit("SELECT rate_limit FROM quotas", 100),
            "SELECT rate_limit FROM quotas LIMIT 100"
        );

        // Existing LIMIT, count(), or disabled limit leave the query alone
        assert_eq!(
            add_default_limit("SELECT * FROM users LIMIT 5", 100),
            "SELECT * FROM users LIMIT 5"
        );
        assert_eq!(
            add_default_limit("SELECT count(*) FROM users", 100),
            "SELECT count(*) FROM users"
        );
        assert_eq!(
            add_default_limit("SELECT * FROM users", 0),
            "SELECT * FROM users"
        );

        // DML/DDL containing a SELECT must never be rewritten
        for q in [
            "INSERT INTO archive SELECT * FROM big_table",
            "CREATE TABLE copy AS SELECT * FROM src",
            "DELETE FROM t WHERE id IN (SELECT id FROM dead)",
            "UPDATE t SET v = (SELECT max(v) FROM s)",
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
            "EXPLAIN SELECT * FROM users",
        ] {
            assert_eq!(add_default_limit(q, 100), q, "must not rewrite: {q}");
        }
    }
}
