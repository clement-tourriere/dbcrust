use crate::database::{DatabaseType, DatabaseTypeExt};
use bollard::Docker;
use bollard::models::{ContainerInspectResponse, ContainerSummary};
use bollard::query_parameters::{InspectContainerOptions, ListContainersOptions};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DockerError {
    #[error("Docker connection failed: {0}")]
    ConnectionError(#[from] bollard::errors::Error),
    #[error("Container '{0}' not found")]
    ContainerNotFound(String),
    #[error("No database containers found")]
    NoDatabaseContainers,
    #[error("Container '{0}' is not running")]
    ContainerNotRunning(String),
    #[error("No exposed ports found for container '{0}'")]
    NoExposedPorts(String),
    #[error("Database type detection failed for container '{0}'")]
    DatabaseTypeDetectionFailed(String),
    #[error("Missing required environment variable: {0}")]
    MissingEnvironmentVariable(String),
}

#[derive(Debug, Clone)]
pub struct DockerContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub database_type: Option<DatabaseType>,
    pub host_port: Option<u16>,
    pub container_port: Option<u16>,
    pub ip_address: Option<String>,
    pub environment: HashMap<String, String>,
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct DockerDatabaseConnection {
    pub container_name: String,
    pub database_type: DatabaseType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database_name: Option<String>,
}

pub struct DockerClient {
    docker: Docker,
}

impl DockerClient {
    /// Create a new Docker client
    pub fn new() -> Result<Self, DockerError> {
        let docker = Docker::connect_with_defaults()?;
        Ok(DockerClient { docker })
    }

    /// Create a Docker client with custom socket path
    pub fn with_socket(_socket_path: &str) -> Result<Self, DockerError> {
        // TODO: Implement custom socket path support
        let docker = Docker::connect_with_http_defaults()?;
        Ok(DockerClient { docker })
    }

    /// List all running containers
    pub async fn list_containers(&self) -> Result<Vec<ContainerSummary>, DockerError> {
        let mut filters = HashMap::new();
        filters.insert("status".to_string(), vec!["running".to_string()]);

        let options = ListContainersOptions {
            all: false,
            filters: Some(filters),
            ..Default::default()
        };

        let containers = self.docker.list_containers(Some(options)).await?;
        Ok(containers)
    }

    /// List all database containers (running or not)
    pub async fn list_database_containers(&self) -> Result<Vec<DockerContainerInfo>, DockerError> {
        let options = ListContainersOptions {
            all: true,
            ..Default::default()
        };

        let containers = self.docker.list_containers(Some(options)).await?;
        let mut database_containers = Vec::new();

        for container in containers {
            if let Some(image) = &container.image {
                let database_type = Self::detect_database_type_from_image(image);
                if let Some(db_type) = database_type {
                    // Extract basic port information from container summary
                    let (host_port, container_port) =
                        self.extract_port_mapping_from_summary(&container, &db_type);

                    let container_info = DockerContainerInfo {
                        id: container.id.unwrap_or_default(),
                        name: container
                            .names
                            .unwrap_or_default()
                            .first()
                            .map(|n| n.trim_start_matches('/').to_string())
                            .unwrap_or_default(),
                        image: image.to_string(),
                        status: container.status.unwrap_or_default(),
                        database_type: Some(db_type),
                        host_port,
                        container_port,
                        ip_address: None, // Will be populated by inspect_container if needed
                        environment: HashMap::new(), // Will be populated by inspect_container if needed
                        labels: HashMap::new(), // Will be populated by inspect_container if needed
                    };
                    database_containers.push(container_info);
                }
            }
        }

        if database_containers.is_empty() {
            return Err(DockerError::NoDatabaseContainers);
        }

        // Sort containers: running containers first, then others
        database_containers.sort_by(|a, b| {
            let a_running = a.status.contains("running") || a.status.contains("Up");
            let b_running = b.status.contains("running") || b.status.contains("Up");

            match (a_running, b_running) {
                (true, false) => std::cmp::Ordering::Less, // a is running, b is not - a comes first
                (false, true) => std::cmp::Ordering::Greater, // b is running, a is not - b comes first
                _ => a.name.cmp(&b.name), // Both same status - sort by name alphabetically
            }
        });

        Ok(database_containers)
    }

    /// Get detailed information about a specific container
    pub async fn inspect_container(
        &self,
        container_id: &str,
    ) -> Result<DockerContainerInfo, DockerError> {
        let options = InspectContainerOptions {
            size: false,
            ..Default::default()
        };
        let container = self
            .docker
            .inspect_container(container_id, Some(options))
            .await?;

        let config = container
            .config
            .as_ref()
            .ok_or_else(|| DockerError::ContainerNotFound(container_id.to_string()))?;
        let state = container
            .state
            .as_ref()
            .ok_or_else(|| DockerError::ContainerNotFound(container_id.to_string()))?;

        // Check if container is running
        if !state.running.unwrap_or(false) {
            return Err(DockerError::ContainerNotRunning(container_id.to_string()));
        }

        let image = config.image.as_ref().unwrap_or(&String::new()).clone();
        let database_type = Self::detect_database_type_from_image(&image)
            .ok_or_else(|| DockerError::DatabaseTypeDetectionFailed(container_id.to_string()))?;

        // Extract environment variables
        let mut environment = HashMap::new();
        if let Some(env_vars) = &config.env {
            for env_var in env_vars {
                if let Some((key, value)) = env_var.split_once('=') {
                    environment.insert(key.to_string(), value.to_string());
                }
            }
        }

        // Extract labels
        let mut labels = HashMap::new();
        if let Some(container_labels) = &config.labels {
            for (key, value) in container_labels {
                labels.insert(key.clone(), value.clone());
            }
        }

        // Extract port mappings
        let (host_port, container_port) = self.extract_port_mapping(&container, &database_type)?;

        // Extract IP address from network settings
        let ip_address = self.extract_ip_address(&container);

        Ok(DockerContainerInfo {
            id: container.id.unwrap_or_default(),
            name: container
                .name
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string(),
            image,
            status: state
                .status
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            database_type: Some(database_type),
            host_port,
            container_port,
            ip_address,
            environment,
            labels,
        })
    }

    /// Extract IP address from container inspection
    fn extract_ip_address(&self, container: &ContainerInspectResponse) -> Option<String> {
        if let Some(network_settings) = &container.network_settings {
            // Try to get IP address from the default bridge network first
            if let Some(ip_address) = &network_settings.ip_address {
                if !ip_address.is_empty() {
                    return Some(ip_address.clone());
                }
            }

            // If no IP in default network, try to get from any network
            if let Some(networks) = &network_settings.networks {
                for network in networks.values() {
                    if let Some(ip_address) = &network.ip_address {
                        if !ip_address.is_empty() {
                            return Some(ip_address.clone());
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract port mapping from container summary (basic listing)
    fn extract_port_mapping_from_summary(
        &self,
        container: &ContainerSummary,
        database_type: &DatabaseType,
    ) -> (Option<u16>, Option<u16>) {
        let default_port = Self::get_default_port(database_type);

        if let Some(ports) = &container.ports {
            for port in ports {
                // Check if this port matches our database port
                if port.private_port == default_port {
                    // Found matching database port
                    if let Some(public_port) = port.public_port {
                        return (Some(public_port), Some(default_port));
                    }
                }
            }
        }

        (None, Some(default_port))
    }

    /// Extract port mapping from container inspection
    fn extract_port_mapping(
        &self,
        container: &ContainerInspectResponse,
        database_type: &DatabaseType,
    ) -> Result<(Option<u16>, Option<u16>), DockerError> {
        let default_port = Self::get_default_port(database_type);

        if let Some(network_settings) = &container.network_settings {
            if let Some(ports) = &network_settings.ports {
                // Look for the database port
                let port_key = format!("{default_port}/tcp");
                if let Some(port_bindings) = ports.get(&port_key) {
                    if let Some(port_binding) = port_bindings {
                        if let Some(binding) = port_binding.first() {
                            if let Some(host_port_str) = &binding.host_port {
                                if let Ok(host_port) = host_port_str.parse::<u16>() {
                                    return Ok((Some(host_port), Some(default_port)));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok((None, Some(default_port)))
    }

    /// Detect database type from Docker image name
    fn detect_database_type_from_image(image: &str) -> Option<DatabaseType> {
        let image_lower = image.to_lowercase();

        if image_lower.contains("postgres")
            || image_lower.contains("pgvector")
            || (image_lower.contains("pg")
                && (image_lower.contains(":") || image_lower.contains("/")))
        {
            Some(DatabaseType::PostgreSQL)
        } else if image_lower.contains("mysql") || image_lower.contains("mariadb") {
            Some(DatabaseType::MySQL)
        } else if image_lower.contains("sqlite") {
            Some(DatabaseType::SQLite)
        } else if image_lower.contains("clickhouse")
            || image_lower.contains("clickhouse-server")
            || image_lower.contains("yandex/clickhouse")
            || image_lower.contains("clickhouse/clickhouse")
        {
            Some(DatabaseType::ClickHouse)
        } else if image_lower.contains("mongo")
            || image_lower.contains("mongodb")
            || image_lower.contains("mongo:")
        {
            Some(DatabaseType::MongoDB)
        } else if image_lower.contains("elasticsearch")
            || image_lower.contains("elastic/elasticsearch")
            || image_lower.contains("docker.elastic.co/elasticsearch")
            || (image_lower.contains("elastic")
                && !image_lower.contains("kibana")  // Exclude Kibana containers
                && (image_lower.contains(":") || image_lower.contains("/")))
        {
            Some(DatabaseType::Elasticsearch)
        } else {
            None
        }
    }

    /// Get default port for database type
    fn get_default_port(database_type: &DatabaseType) -> u16 {
        database_type.default_port().unwrap_or(0)
    }

    /// Build database connection info from container info
    pub fn build_connection_info(
        &self,
        container_info: &DockerContainerInfo,
    ) -> Result<DockerDatabaseConnection, DockerError> {
        let database_type = container_info
            .database_type
            .clone()
            .ok_or_else(|| DockerError::DatabaseTypeDetectionFailed(container_info.name.clone()))?;

        // Try to get host port, or fall back to container IP (Linux) or OrbStack domain (macOS) if no port is exposed
        let (host, port) = if let Some(host_port) = container_info.host_port {
            ("localhost".to_string(), host_port)
        } else {
            // Check for OrbStack custom domain or compose project domain first
            if let Some((orbstack_host, orbstack_port)) =
                self.get_orbstack_custom_or_compose_domain(container_info, &database_type)?
            {
                (orbstack_host, orbstack_port)
            } else if let Some(container_ip) = &container_info.ip_address {
                // Fall back to container IP address (Linux or any system with IP)
                let default_port = Self::get_default_port(&database_type);
                (container_ip.clone(), default_port)
            } else if let Some((orbstack_host, orbstack_port)) =
                self.get_orbstack_automatic_domain(container_info, &database_type)?
            {
                // Fall back to OrbStack automatic domain (macOS)
                (orbstack_host, orbstack_port)
            } else {
                return Err(DockerError::NoExposedPorts(container_info.name.clone()));
            }
        };

        let (username, password, database_name) = if database_type.is_file_based() {
            // SQLite doesn't use network connections
            (None, None, None)
        } else {
            // Extract username from environment variables
            let username = database_type
                .docker_username_env_vars()
                .iter()
                .find_map(|var| container_info.environment.get(*var))
                .cloned()
                .unwrap_or_else(|| database_type.default_username().to_string());

            // Extract password from environment variables
            let password = database_type
                .docker_password_env_vars()
                .iter()
                .find_map(|var| container_info.environment.get(*var))
                .cloned();

            // Extract database name from environment variables
            let database_name = database_type
                .docker_database_env_vars()
                .iter()
                .find_map(|var| container_info.environment.get(*var))
                .cloned()
                .unwrap_or_else(|| username.clone());

            // Special handling for ClickHouse with CLICKHOUSE_SKIP_USER_SETUP=1
            let final_password = if database_type == DatabaseType::ClickHouse {
                // Check if user setup is skipped
                if let Some(skip_setup) =
                    container_info.environment.get("CLICKHOUSE_SKIP_USER_SETUP")
                {
                    if skip_setup == "1" {
                        // When user setup is skipped, default user is available without password
                        tracing::debug!(
                            "[DockerClient::build_connection_info] ClickHouse SKIP_USER_SETUP=1 detected, using default user without password"
                        );
                        None
                    } else {
                        password
                    }
                } else {
                    password
                }
            } else {
                password
            };

            (Some(username), final_password, Some(database_name))
        };

        Ok(DockerDatabaseConnection {
            container_name: container_info.name.clone(),
            database_type,
            host,
            port,
            username,
            password,
            database_name,
        })
    }

    /// Get OrbStack custom domain or compose project domain (high priority)
    pub fn get_orbstack_custom_or_compose_domain(
        &self,
        container_info: &DockerContainerInfo,
        database_type: &DatabaseType,
    ) -> Result<Option<(String, u16)>, DockerError> {
        // First check if we're running on OrbStack by looking for OrbStack-specific indicators
        if !self.is_orbstack_available() {
            return Ok(None);
        }

        let default_port = Self::get_default_port(database_type);

        // Check for custom domain label first
        if let Some(custom_domain) = self.get_custom_orbstack_domain(container_info) {
            return Ok(Some((custom_domain, default_port)));
        }

        // Check for compose project domain
        if let Some(compose_domain) = self.get_compose_orbstack_domain(container_info) {
            return Ok(Some((compose_domain, default_port)));
        }

        // No custom or compose domain found
        Ok(None)
    }

    /// Get OrbStack automatic domain for standalone containers (low priority)
    pub fn get_orbstack_automatic_domain(
        &self,
        container_info: &DockerContainerInfo,
        database_type: &DatabaseType,
    ) -> Result<Option<(String, u16)>, DockerError> {
        // First check if we're running on OrbStack by looking for OrbStack-specific indicators
        if !self.is_orbstack_available() {
            return Ok(None);
        }

        let default_port = Self::get_default_port(database_type);

        // For standalone containers, use container name domain
        let container_domain = format!("{}.orb.local", container_info.name);
        Ok(Some((container_domain, default_port)))
    }

    /// Get OrbStack domain for container if available (kept for backward compatibility)
    pub fn get_orbstack_domain(
        &self,
        container_info: &DockerContainerInfo,
        database_type: &DatabaseType,
    ) -> Result<Option<(String, u16)>, DockerError> {
        // Try custom/compose first
        if let Some(domain) =
            self.get_orbstack_custom_or_compose_domain(container_info, database_type)?
        {
            return Ok(Some(domain));
        }

        // Fall back to automatic domain
        self.get_orbstack_automatic_domain(container_info, database_type)
    }

    /// Check if OrbStack is available by looking for OrbStack-specific environment
    fn is_orbstack_available(&self) -> bool {
        // OrbStack typically runs on macOS and has specific characteristics
        // We can check for OrbStack's presence by attempting to resolve a known OrbStack domain
        // For now, we'll do a simple check - in production, you might want to be more sophisticated

        // Check if we can find any containers with OrbStack-style labels or networks
        // This is a heuristic approach
        std::env::consts::OS == "macos"
            || std::env::var("ORBSTACK_HOST").is_ok()
            || std::path::Path::new("/Applications/OrbStack.app").exists()
    }

    /// Get custom domain from container labels (dev.orbstack.domains label)
    fn get_custom_orbstack_domain(&self, container_info: &DockerContainerInfo) -> Option<String> {
        // Check for OrbStack custom domain label - the actual label is "dev.orbstack.domains"
        container_info.labels.get("dev.orbstack.domains").cloned()
    }

    /// Get compose project domain (service.project.orb.local)
    fn get_compose_orbstack_domain(&self, container_info: &DockerContainerInfo) -> Option<String> {
        // First check for compose project label
        if let Some(project_name) = container_info.labels.get("com.docker.compose.project") {
            if let Some(service_name) = container_info.labels.get("com.docker.compose.service") {
                // For compose projects, OrbStack uses service.project.orb.local format
                return Some(format!("{service_name}.{project_name}.orb.local"));
            } else {
                // Fallback to just project name
                return Some(format!("{project_name}.orb.local"));
            }
        }

        // Fallback: Check if this container is part of a compose project by name pattern
        // Compose containers typically have names like "project-service-1"
        if container_info.name.contains('-') {
            let parts: Vec<&str> = container_info.name.split('-').collect();
            if parts.len() >= 2 {
                // Take everything except the last part (which is usually the replica number)
                let project_service = parts[..parts.len() - 1].join("-");
                return Some(format!("{project_service}.orb.local"));
            }
        }
        None
    }

    /// Parse Docker URL format: docker://user:password@container_name/database
    pub fn parse_docker_url(
        url: &str,
    ) -> Option<(Option<String>, Option<String>, String, Option<String>)> {
        if !url.starts_with("docker://") {
            return None;
        }

        let url_without_prefix = &url["docker://".len()..];

        // Handle empty URL: docker://
        if url_without_prefix.is_empty() {
            return Some((None, None, String::new(), None));
        }

        // Parse user:password@container_name/database pattern
        let (credentials_part, container_and_db) =
            if let Some(at_pos) = url_without_prefix.find('@') {
                let (creds, rest) = url_without_prefix.split_at(at_pos);
                (Some(creds), &rest[1..]) // Skip the '@'
            } else {
                (None, url_without_prefix)
            };

        // Parse credentials
        let (username, password) = if let Some(creds) = credentials_part {
            if let Some(colon_pos) = creds.find(':') {
                let (user, pass) = creds.split_at(colon_pos);
                (Some(user.to_string()), Some(pass[1..].to_string())) // Skip the ':'
            } else {
                (Some(creds.to_string()), None)
            }
        } else {
            (None, None)
        };

        // Parse container_name/database
        let (container_name, database_name) = if let Some(slash_pos) = container_and_db.find('/') {
            let (container, db) = container_and_db.split_at(slash_pos);
            (container.to_string(), Some(db[1..].to_string())) // Skip the '/'
        } else {
            (container_and_db.to_string(), None)
        };

        Some((username, password, container_name, database_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_database_type_from_image() {
        assert_eq!(
            DockerClient::detect_database_type_from_image("postgres:13"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("pgvector/pgvector:pg16"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("pg:14"),
            Some(DatabaseType::PostgreSQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("mysql:8.0"),
            Some(DatabaseType::MySQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("mariadb:10.5"),
            Some(DatabaseType::MySQL)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("clickhouse:latest"),
            Some(DatabaseType::ClickHouse)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("clickhouse/clickhouse-server:23.8"),
            Some(DatabaseType::ClickHouse)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("yandex/clickhouse-server:latest"),
            Some(DatabaseType::ClickHouse)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("mongo:7.0"),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image(
                "mongodb/mongodb-community-server:latest"
            ),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("bitnami/mongodb:latest"),
            Some(DatabaseType::MongoDB)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("elasticsearch:8.15.0"),
            Some(DatabaseType::Elasticsearch)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("elastic/elasticsearch:8.15.0"),
            Some(DatabaseType::Elasticsearch)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image(
                "docker.elastic.co/elasticsearch/elasticsearch:8.15.0"
            ),
            Some(DatabaseType::Elasticsearch)
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("bitnami/elasticsearch:latest"),
            Some(DatabaseType::Elasticsearch)
        );
        // Test Kibana containers are NOT detected as Elasticsearch
        assert_eq!(
            DockerClient::detect_database_type_from_image("elastic/kibana:8.15.0"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("docker.elastic.co/kibana/kibana:8.15.0"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("kibana:8.15.0"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("bitnami/kibana:latest"),
            None
        );
        assert_eq!(
            DockerClient::detect_database_type_from_image("nginx:latest"),
            None
        );
    }

    #[test]
    fn test_parse_docker_url() {
        // Test empty URL
        assert_eq!(
            DockerClient::parse_docker_url("docker://"),
            Some((None, None, String::new(), None))
        );

        // Test container name only
        assert_eq!(
            DockerClient::parse_docker_url("docker://postgres-db"),
            Some((None, None, "postgres-db".to_string(), None))
        );

        // Test container name with database
        assert_eq!(
            DockerClient::parse_docker_url("docker://postgres-db/mydb"),
            Some((
                None,
                None,
                "postgres-db".to_string(),
                Some("mydb".to_string())
            ))
        );

        // Test with username
        assert_eq!(
            DockerClient::parse_docker_url("docker://user@postgres-db"),
            Some((
                Some("user".to_string()),
                None,
                "postgres-db".to_string(),
                None
            ))
        );

        // Test with username and password
        assert_eq!(
            DockerClient::parse_docker_url("docker://user:pass@postgres-db"),
            Some((
                Some("user".to_string()),
                Some("pass".to_string()),
                "postgres-db".to_string(),
                None
            ))
        );

        // Test full URL
        assert_eq!(
            DockerClient::parse_docker_url("docker://user:pass@postgres-db/mydb"),
            Some((
                Some("user".to_string()),
                Some("pass".to_string()),
                "postgres-db".to_string(),
                Some("mydb".to_string())
            ))
        );

        // Test invalid URL
        assert_eq!(
            DockerClient::parse_docker_url("postgres://user:pass@host/db"),
            None
        );
    }

    #[test]
    fn test_get_default_port() {
        assert_eq!(
            DockerClient::get_default_port(&DatabaseType::PostgreSQL),
            5432
        );
        assert_eq!(DockerClient::get_default_port(&DatabaseType::MySQL), 3306);
        assert_eq!(DockerClient::get_default_port(&DatabaseType::SQLite), 0);
    }

    #[test]
    fn test_orbstack_custom_domain() {
        let docker_client = DockerClient::new().unwrap();

        // Test custom domain detection
        let mut labels = HashMap::new();
        labels.insert(
            "dev.orbstack.domains".to_string(),
            "my-custom-db.local".to_string(),
        );

        let container_info = DockerContainerInfo {
            id: "test".to_string(),
            name: "test-container".to_string(),
            image: "postgres:13".to_string(),
            status: "running".to_string(),
            database_type: Some(DatabaseType::PostgreSQL),
            host_port: None,
            container_port: Some(5432),
            ip_address: None,
            environment: HashMap::new(),
            labels,
        };

        let custom_domain = docker_client.get_custom_orbstack_domain(&container_info);
        assert_eq!(custom_domain, Some("my-custom-db.local".to_string()));
    }

    #[test]
    fn test_orbstack_compose_domain() {
        let docker_client = DockerClient::new().unwrap();

        // Test compose project domain detection via labels
        let mut labels = HashMap::new();
        labels.insert(
            "com.docker.compose.project".to_string(),
            "myapp".to_string(),
        );
        labels.insert(
            "com.docker.compose.service".to_string(),
            "database".to_string(),
        );

        let container_info = DockerContainerInfo {
            id: "test".to_string(),
            name: "myapp-database-1".to_string(),
            image: "postgres:13".to_string(),
            status: "running".to_string(),
            database_type: Some(DatabaseType::PostgreSQL),
            host_port: None,
            container_port: Some(5432),
            ip_address: None,
            environment: HashMap::new(),
            labels,
        };

        let compose_domain = docker_client.get_compose_orbstack_domain(&container_info);
        assert_eq!(compose_domain, Some("database.myapp.orb.local".to_string()));

        // Test fallback to name-based detection
        let container_info_no_labels = DockerContainerInfo {
            id: "test".to_string(),
            name: "myproject-postgres-1".to_string(),
            image: "postgres:13".to_string(),
            status: "running".to_string(),
            database_type: Some(DatabaseType::PostgreSQL),
            host_port: None,
            container_port: Some(5432),
            ip_address: None,
            environment: HashMap::new(),
            labels: HashMap::new(),
        };

        let fallback_domain = docker_client.get_compose_orbstack_domain(&container_info_no_labels);
        assert_eq!(
            fallback_domain,
            Some("myproject-postgres.orb.local".to_string())
        );
    }

    #[test]
    fn test_linux_container_ip_fallback() {
        let docker_client = DockerClient::new().unwrap();

        // Test Linux container IP fallback when no ports are exposed
        let container_info = DockerContainerInfo {
            id: "test".to_string(),
            name: "postgrescontainer".to_string(),
            image: "postgres:13".to_string(),
            status: "running".to_string(),
            database_type: Some(DatabaseType::PostgreSQL),
            host_port: None,
            container_port: Some(5432),
            ip_address: Some("172.17.0.2".to_string()),
            environment: HashMap::new(),
            labels: HashMap::new(),
        };

        let connection_info = docker_client
            .build_connection_info(&container_info)
            .unwrap();
        assert_eq!(connection_info.host, "172.17.0.2");
        assert_eq!(connection_info.port, 5432);
    }
}
