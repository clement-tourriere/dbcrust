use crate::config::SSHTunnelConfig;
use crate::database::{
    ConnectionInfo, DatabaseClient, DatabaseType, DatabaseTypeExt, create_database_client,
};
use crate::pgpass;

use inquire::MultiSelect;
use std::collections::HashMap;
use std::error::Error as StdError;
use tracing::{debug, info};

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

/// Column filtering metadata to track when results are filtered
#[derive(Debug, Clone)]
pub struct ColumnFilteringInfo {
    pub total_columns: usize,
    pub displayed_columns: usize,
    pub filtered_column_names: Vec<String>,
}

impl ColumnFilteringInfo {
    pub fn new(
        total_columns: usize,
        displayed_columns: usize,
        filtered_column_names: Vec<String>,
    ) -> Self {
        Self {
            total_columns,
            displayed_columns,
            filtered_column_names,
        }
    }

    pub fn is_filtered(&self) -> bool {
        self.displayed_columns < self.total_columns
    }
}

/// Query results with optional column filtering information
#[derive(Debug)]
pub struct QueryResultsWithInfo {
    pub data: Vec<Vec<String>>,
    pub column_info: Option<ColumnFilteringInfo>,
}

pub struct Database {
    // Database abstraction layer client
    database_client: Option<Box<dyn DatabaseClient>>,

    // Connection info override for special cases like Vault connections
    connection_info_override: Option<crate::database::ConnectionInfo>,

    // SSH tunnel management
    ssh_tunnel: Option<crate::ssh_tunnel::SSHTunnel>,

    // Application settings and state
    expanded_display: bool,
    default_limit: usize,
    autocomplete_enabled: bool,
    explain_mode: bool,
    column_select_mode: bool,
    banner_enabled: bool,
    column_selection_threshold: usize,
    column_selection_default_all: bool,
    column_views: HashMap<String, Vec<String>>, // Map of column view name -> selected columns
    last_view_key: Option<String>,
    last_json_plan: Option<String>, // Store the last EXPLAIN JSON plan for copying
}

impl Database {
    /// Create a new Database instance from a database URL
    pub async fn from_url(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        debug!("[Database::from_url] Creating database from URL");
        let step_start = std::time::Instant::now();

        // Handle Docker URLs specially
        if url.starts_with("docker://") {
            let (database, _) =
                Self::from_docker_url(url, default_limit, expanded_display_default).await?;
            return Ok(database);
        }

        // Parse the connection info from URL
        let config_start = std::time::Instant::now();
        let config = crate::config::Config::load();
        debug!("📋 Parsing connection URL...");
        let connection_info = ConnectionInfo::parse_url(url)?;
        debug!(
            "[Database::from_url] Parsed URL in {:?}",
            step_start.elapsed()
        );

        // For SQLite, we don't need SSH tunneling
        if connection_info.database_type.is_file_based() {
            return Self::from_connection_info(
                connection_info,
                default_limit,
                expanded_display_default,
                None,
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
            // SSH tunnel info should always be shown (even in quiet mode)
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
        let result = Self::from_connection_info(
            connection_info,
            default_limit,
            expanded_display_default,
            ssh_tunnel_config,
        )
        .await;
        debug!(
            "[Database::from_url] from_connection_info took {:?}",
            conn_start.elapsed()
        );
        result
    }

    /// Create a new Database instance from a Docker URL
    pub async fn from_docker_url(
        url: &str,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
    ) -> std::result::Result<(Self, Option<ConnectionInfo>), Box<dyn StdError>> {
        debug!("[Database::from_docker_url] Creating database from Docker URL");

        // Parse Docker URL
        let connection_info = ConnectionInfo::parse_url(url)?;

        // Get the container name from the connection info
        let container_name = connection_info
            .docker_container
            .as_ref()
            .ok_or("Docker container name not found in URL")?;

        // If container name is empty, provide interactive selection
        if container_name.is_empty() {
            let selected_container = Self::select_docker_container().await?;

            // Create Docker client
            let docker_client = crate::docker::DockerClient::new()
                .map_err(|e| format!("Failed to create Docker client: {e}"))?;

            let container_info = docker_client
                .inspect_container(&selected_container)
                .await
                .map_err(|e| {
                    format!(
                        "Failed to inspect selected Docker container '{selected_container}': {e}"
                    )
                })?;

            // Build database connection info from container
            let docker_connection = docker_client
                .build_connection_info(&container_info)
                .map_err(|e| format!("Failed to build connection info: {e}"))?;

            // Create a new ConnectionInfo with the resolved Docker information
            let resolved_connection_info = ConnectionInfo {
                database_type: docker_connection.database_type,
                host: Some(docker_connection.host),
                port: Some(docker_connection.port),
                username: connection_info
                    .username
                    .filter(|u| !u.is_empty())
                    .or(docker_connection.username),
                password: connection_info
                    .password
                    .filter(|p| !p.is_empty())
                    .or(docker_connection.password),
                database: connection_info.database.or(docker_connection.database_name),
                file_path: None,
                options: connection_info.options,
                docker_container: Some(selected_container.clone()),
            };

            let database = Self::from_connection_info(
                resolved_connection_info.clone(),
                default_limit,
                expanded_display_default,
                None,
            )
            .await?;
            return Ok((database, Some(resolved_connection_info)));
        }

        // Create Docker client and inspect the container
        let docker_client = crate::docker::DockerClient::new()
            .map_err(|e| format!("Failed to create Docker client: {e}"))?;

        let container_info = docker_client
            .inspect_container(container_name)
            .await
            .map_err(|e| format!("Failed to inspect Docker container '{container_name}': {e}"))?;

        // Build database connection info from container
        let docker_connection = docker_client
            .build_connection_info(&container_info)
            .map_err(|e| format!("Failed to build connection info: {e}"))?;

        // Create a new ConnectionInfo with the resolved Docker information
        let resolved_connection_info = ConnectionInfo {
            database_type: docker_connection.database_type,
            host: Some(docker_connection.host),
            port: Some(docker_connection.port),
            username: connection_info
                .username
                .filter(|u| !u.is_empty())
                .or(docker_connection.username),
            password: connection_info
                .password
                .filter(|p| !p.is_empty())
                .or(docker_connection.password),
            database: connection_info.database.or(docker_connection.database_name),
            file_path: None,
            options: connection_info.options,
            docker_container: Some(container_name.clone()),
        };

        debug!(
            "[Database::from_docker_url] Resolved Docker connection: {}@{}:{}/{}",
            resolved_connection_info.username.as_deref().unwrap_or(""),
            resolved_connection_info.host.as_deref().unwrap_or(""),
            resolved_connection_info.port.unwrap_or(0),
            resolved_connection_info.database.as_deref().unwrap_or("")
        );

        // Create database connection using the resolved info
        let database = Self::from_connection_info(
            resolved_connection_info.clone(),
            default_limit,
            expanded_display_default,
            None,
        )
        .await?;
        Ok((database, Some(resolved_connection_info)))
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
        Self::from_docker_url(url, default_limit, expanded_display_default).await
    }

    /// Create a new Database instance from ConnectionInfo
    pub async fn from_connection_info(
        connection_info: ConnectionInfo,
        default_limit: Option<usize>,
        expanded_display_default: Option<bool>,
        ssh_tunnel_config: Option<SSHTunnelConfig>,
    ) -> std::result::Result<Self, Box<dyn StdError>> {
        debug!("[Database::from_connection_info] Creating database from connection info");

        let config = crate::config::Config::load();

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

        let db = Self {
            database_client: Some(database_client),
            connection_info_override: None,
            ssh_tunnel,
            expanded_display: expanded_display_default.unwrap_or(false),
            default_limit: default_limit.unwrap_or(100),
            autocomplete_enabled: config.autocomplete_enabled,
            explain_mode: config.explain_mode_default,
            column_select_mode: false,
            banner_enabled: config.show_banner,
            column_selection_threshold: config.column_selection_threshold,
            column_selection_default_all: config.column_selection_default_all,
            column_views: HashMap::new(),
            last_view_key: None,
            last_json_plan: None,
        };

        // Validate the connection before returning
        debug!("[Database::from_connection_info] Validating connection");
        db.validate_connection().await?;

        // Display server info if enabled in config
        if config.show_server_info {
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
        _ssl_mode: Option<sqlx::postgres::PgSslMode>,
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
            file_path: None,
            options: HashMap::new(),
            docker_container: None,
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
            return Err("No database client available".into());
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
            return Err("No database client available".into());
        }
    }

    /// List users (database-specific implementation)
    pub async fn list_users(&mut self) -> std::result::Result<Vec<Vec<String>>, Box<dyn StdError>> {
        debug!("[Database::list_users] Listing database users");

        if let Some(ref database_client) = self.database_client {
            debug!("Using database abstraction layer for list_users");

            let connection_info = database_client.get_connection_info();

            if connection_info.database_type.is_file_based() {
                // SQLite doesn't have users concept
                Ok(vec![
                    vec!["Note".to_string()],
                    vec!["SQLite is file-based and doesn't have user accounts".to_string()],
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

            if connection_info.database_type.is_file_based() {
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
                        Some("text") => format!("{{ \"{}\": \"text\" }}", field),
                        Some("hash") => format!("{{ \"{}\": \"hashed\" }}", field),
                        Some("desc") => format!("{{ \"{}\": -1 }}", field),
                        _ => format!("{{ \"{}\": 1 }}", field), // default ascending
                    };

                    let query = format!(
                        "db.runCommand({{ createIndexes: \"{}\", indexes: [{{ key: {}, name: \"{}_{}_idx\" }}] }})",
                        collection, index_spec, collection, field
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
                        "db.runCommand({{ dropIndexes: \"{}\", index: \"{}\" }})",
                        collection, index_name
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
                    let projection_str = projection.map(|p| format!(", {}", p)).unwrap_or_default();
                    let limit_str = limit.map(|l| format!(", limit: {}", l)).unwrap_or_default();

                    let query = format!(
                        "db.{}.find({}{}){}",
                        collection, filter_str, projection_str, limit_str
                    );

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
                    let query = format!("db.{}.aggregate({})", collection, pipeline);

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
                    let filter = format!(r#"{{"$text": {{"$search": "{}"}}}}"#, search_term);
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

    pub fn get_pool(&self) -> Option<&sqlx::PgPool> {
        // Legacy method - pool management is now handled by database clients
        None
    }

    // Pool management methods removed - now handled by database clients

    pub fn get_database_client(&self) -> Option<&Box<dyn DatabaseClient>> {
        self.database_client.as_ref()
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

            // Add table/collection rows
            for table in tables {
                let conn_info = database_client.get_connection_info();
                let schema_name =
                    if conn_info.database_type == crate::database::DatabaseType::MongoDB {
                        "".to_string() // MongoDB doesn't have schemas
                    } else {
                        database_client
                            .get_metadata_provider()
                            .default_schema()
                            .unwrap_or_else(|| "main".to_string())
                    };

                let object_type =
                    if conn_info.database_type == crate::database::DatabaseType::MongoDB {
                        "collection".to_string()
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

    pub async fn execute_query_with_interrupt_and_info(
        &mut self,
        query: &str,
        interrupt_flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::result::Result<QueryResultsWithInfo, Box<dyn StdError>> {
        // Check if we should EXPLAIN this query (applies to all database types)
        if self.explain_mode && is_query_explainable(query) {
            debug!("EXPLAIN mode is enabled, executing EXPLAIN query");
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
            return self.apply_column_selection_if_needed_with_info(results, interrupt_flag);
        } else {
            return Err("No database client available".into());
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
        let should_apply =
            self.column_select_mode || self.should_auto_enable_column_selection(column_count);

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
                    eprintln!("Column selection error: {e}");
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
            return Err("No database client available".into());
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
            return Err("No database client available".into());
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
            return Ok(results);
        } else {
            return Err("No database client available".into());
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
        // Add limit to query if default_limit is set and query doesn't already have LIMIT
        let query_lower = query.to_lowercase();
        if self.default_limit > 0
            && !query_lower.contains("limit")
            && (query_lower.contains("select") && !query_lower.contains("count("))
        {
            // Check if query ends with semicolon and insert LIMIT before it
            let query_trimmed = query.trim_end();
            if query_trimmed.ends_with(';') {
                let without_semicolon = query_trimmed.trim_end_matches(';');
                format!("{} LIMIT {};", without_semicolon, self.default_limit)
            } else {
                format!("{} LIMIT {}", query, self.default_limit)
            }
        } else {
            query.to_string()
        }
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

    pub fn is_explain_mode(&self) -> bool {
        self.explain_mode
    }

    pub fn toggle_explain_mode(&mut self) -> bool {
        self.explain_mode = !self.explain_mode;
        self.explain_mode
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
            match database_client
                .get_metadata_provider()
                .get_table_details(table_name, None)
                .await
            {
                Ok(table_details) => return Ok(table_details),
                Err(e) => {
                    debug!("Error using database client for get_table_details: {e}");
                    return Err(Box::new(e));
                }
            }
        } else {
            return Err("No database client available".into());
        }
    }

    pub fn new_for_test() -> Self {
        let config = crate::config::Config::load();
        Self {
            database_client: None, // No database client in test mode
            connection_info_override: None,
            ssh_tunnel: None, // No SSH tunnel in test mode
            expanded_display: false,
            default_limit: 100,
            autocomplete_enabled: config.autocomplete_enabled,
            explain_mode: false,
            column_select_mode: false,
            banner_enabled: config.show_banner,
            column_selection_threshold: config.column_selection_threshold,
            column_selection_default_all: config.column_selection_default_all,
            column_views: HashMap::new(),
            last_view_key: None,
            last_json_plan: None,
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
            return Ok(names);
        } else {
            return Err("No database client available".into());
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
                    return Ok(tables);
                }
                Err(e) => {
                    debug!(
                        "Error using database client for get_tables_and_views: {}",
                        e
                    );
                    return Err(Box::new(e));
                }
            }
        } else {
            return Err("No database client available".into());
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
                    return Ok(schemas);
                }
                Err(e) => {
                    debug!("Error using database client for get_schemas: {e}");
                    return Err(Box::new(e));
                }
            }
        } else {
            return Err("No database client available".into());
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
            return Ok(functions);
        } else {
            return Err("No database client available".into());
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
            return Ok(columns);
        } else {
            return Err("No database client available".into());
        }
    }

    /// Simplified column getter for the new completion system
    pub async fn get_columns(
        &mut self,
        table_name: &str,
    ) -> std::result::Result<Vec<String>, Box<dyn StdError>> {
        // Try to parse schema from table name if it contains a dot
        let (schema, table) = if table_name.contains('.') {
            let parts: Vec<&str> = table_name.splitn(2, '.').collect();
            (Some(parts[0]), parts[1])
        } else {
            (None, table_name)
        };

        self.get_columns_for_table(table, schema).await
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

#[derive(Debug)]
pub struct TableDetails {
    pub name: String,
    pub schema: String,
    #[allow(dead_code)]
    pub full_name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub check_constraints: Vec<CheckConstraintInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub referenced_by: Vec<ReferencedByInfo>,
    /// Nested field details for struct/complex types (column_name -> field descriptions)
    pub nested_field_details: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub collation: String,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub enum_values: Option<Vec<String>>, // For enum types, contains the possible values
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub index_type: String,
    pub is_primary: bool,
    pub is_unique: bool,
    pub predicate: Option<String>,
    pub definition: String,
    #[allow(dead_code)]
    pub constraint_def: Option<String>,
}

#[derive(Debug)]
pub struct ForeignKeyInfo {
    pub name: String,
    pub definition: String,
}

#[derive(Debug)]
pub struct CheckConstraintInfo {
    pub name: String,
    pub definition: String,
}

#[derive(Debug)]
pub struct ReferencedByInfo {
    pub schema: String,
    pub table: String,
    pub constraint_name: String,
    pub definition: String,
}

// Helper function to determine if a query can be explained
fn is_query_explainable(query: &str) -> bool {
    let query = query.trim().to_lowercase();

    // Only try to EXPLAIN statements that make sense
    // Only SELECT and WITH queries should be explainable
    query.starts_with("select") || query.starts_with("with")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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
