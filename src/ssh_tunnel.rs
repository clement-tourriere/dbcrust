use crate::config::SSHTunnelConfig;
use rand::{Rng, rng};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, info};

#[derive(Error, Debug, Clone)]
pub enum SSHTunnelError {
    #[error("SSH authentication error: {0}")]
    AuthError(String),

    #[error("Failed to bind to local port: {0}")]
    BindError(String),

    #[error("SSH connection error: {0}")]
    ConnectionError(String),

    #[error("SSH tunnel establishment timeout: {0}")]
    TimeoutError(String),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Failed to find available port: {0}")]
    PortError(String),

    #[error("SSH configuration error: {0}")]
    ConfigError(String),

    #[error("SSH command execution error: {0}")]
    SshCommandError(String),

    #[error("SSH command failed with non-zero exit status: {0}")]
    SshCommandFailed(String),
}

impl From<io::Error> for SSHTunnelError {
    fn from(err: io::Error) -> Self {
        SSHTunnelError::IoError(err.to_string())
    }
}

/// A struct that represents an SSH tunnel
#[derive(Clone)]
pub struct SSHTunnel {
    local_port: u16,
    remote_host: String,
    remote_port: u16,
    ssh_user: String,
    ssh_key: Option<PathBuf>,
    ssh_host: String,
    ssh_port: u16,
    tunnel_process: Arc<Mutex<Option<tokio::process::Child>>>,
}

/// Shared type for the SSH tunnel
pub type SharedSSHTunnel = Arc<Mutex<Option<SSHTunnel>>>;

impl SSHTunnel {
    /// Create a new empty SSH tunnel
    pub fn new() -> Option<Self> {
        Some(SSHTunnel::default())
    }

    /// Create a new SSH tunnel using the provided configuration and start it.
    pub async fn establish(
        &mut self,
        conn_config: &SSHTunnelConfig,
        target_service_host: &str,
        target_service_port: u16,
    ) -> Result<u16, SSHTunnelError> {
        self.ssh_host = conn_config.ssh_host.clone();
        self.ssh_port = conn_config.ssh_port;
        self.ssh_user = conn_config
            .ssh_username
            .clone()
            .ok_or_else(|| SSHTunnelError::ConfigError("SSH username is required".to_string()))?;
        self.ssh_key = conn_config.ssh_key_path.clone().map(PathBuf::from);
        self.remote_host = target_service_host.to_string();
        self.remote_port = target_service_port;

        let local_port = self.find_available_port().await?;
        self.local_port = local_port;

        let mut cmd = Command::new("ssh");
        cmd.arg(format!(
            "-L{}:{}:{}",
            self.local_port, self.remote_host, self.remote_port
        ));
        cmd.arg("-N");
        cmd.arg("-o");
        cmd.arg("ExitOnForwardFailure=yes");
        cmd.arg("-o");
        cmd.arg("BatchMode=yes");
        cmd.arg("-o");
        cmd.arg("ConnectTimeout=3"); // Reduced from 5 to 3 seconds
        cmd.arg("-o");
        cmd.arg("ServerAliveInterval=10");
        cmd.arg("-o");
        cmd.arg("ServerAliveCountMax=2");
        cmd.arg("-o");
        cmd.arg("StrictHostKeyChecking=accept-new");
        cmd.arg("-o");
        cmd.arg("PasswordAuthentication=no");
        cmd.arg("-o");
        cmd.arg("LogLevel=ERROR");

        if let Some(key_path) = &self.ssh_key {
            cmd.arg("-i");
            cmd.arg(key_path);
        }

        cmd.arg("-p");
        cmd.arg(self.ssh_port.to_string());

        cmd.arg(format!("{}@{}", self.ssh_user, self.ssh_host));

        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        // Create a user-friendly representation of the SSH command for debug output
        let ssh_command_str = format!(
            "ssh -L{}:{}:{} -N -o ExitOnForwardFailure=yes -o BatchMode=yes -o ConnectTimeout=3 -o ServerAliveInterval=10 -o ServerAliveCountMax=2 -o StrictHostKeyChecking=accept-new -o PasswordAuthentication=no -o LogLevel=ERROR {}{}@{} -p {}",
            self.local_port,
            self.remote_host,
            self.remote_port,
            if self.ssh_key.is_some() {
                format!("-i {} ", self.ssh_key.as_ref().unwrap().display())
            } else {
                String::new()
            },
            self.ssh_user,
            self.ssh_host,
            self.ssh_port
        );

        // Log that we're initiating the SSH tunnel
        info!(
            "Initiating SSH tunnel to {}:{} via {}@{}...",
            self.remote_host, self.remote_port, self.ssh_user, self.ssh_host
        );

        debug!(
            "Executing SSH command: {}",
            crate::password_sanitizer::sanitize_ssh_command(&ssh_command_str)
        );

        let child = cmd.spawn().map_err(|e| {
            SSHTunnelError::SshCommandError(format!("Failed to spawn ssh command: {e}"))
        })?;

        let child_id = child
            .id()
            .map_or_else(|| "[unknown_pid]".to_string(), |id| id.to_string());
        debug!(
            "SSH process {} spawned. Waiting for tunnel setup...",
            child_id
        );

        let mut process_guard = self.tunnel_process.lock().unwrap();
        *process_guard = Some(child);
        drop(process_guard);

        let total_establishment_timeout = Duration::from_secs(8); // Reduced from 12
        let tcp_check_interval = Duration::from_millis(500); // Check more frequently
        let individual_tcp_connect_timeout = Duration::from_millis(500); // Faster timeout
        let start_time = tokio::time::Instant::now();
        let local_addr = format!("127.0.0.1:{}", self.local_port);

        // Give SSH a moment to establish before first check
        tokio::time::sleep(Duration::from_millis(200)).await;

        loop {
            // First check if SSH process has already exited (failed)
            if let Ok(mut guard) = self.tunnel_process.lock() {
                if let Some(child) = guard.as_mut() {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            // SSH process has exited - this is an error
                            let mut stderr_output = String::new();
                            if let Some(mut stderr) = child.stderr.take() {
                                let _ = stderr.read_to_string(&mut stderr_output).await;
                            }
                            return Err(SSHTunnelError::SshCommandFailed(format!(
                                "SSH process exited with status: {}. Error: {}",
                                status,
                                stderr_output.trim()
                            )));
                        }
                        Ok(None) => {
                            // Process is still running, continue
                        }
                        Err(e) => {
                            debug!("Error checking SSH process status: {e}");
                        }
                    }
                }
            }

            if start_time.elapsed() >= total_establishment_timeout {
                eprintln!(
                    "Total timeout of {total_establishment_timeout:?} reached for SSH tunnel establishment to {local_addr}."
                );
                // Attempt to grab stderr before failing
                let mut stderr_output = String::new();
                if let Ok(mut guard) = self.tunnel_process.lock() {
                    if let Some(child_check) = guard.as_mut() {
                        if let Some(mut stderr) = child_check.stderr.take() {
                            // Use a short timeout for this final stderr read attempt
                            match tokio::time::timeout(
                                Duration::from_secs(1),
                                stderr.read_to_string(&mut stderr_output),
                            )
                            .await
                            {
                                Ok(Ok(_)) => { /* Successfully read stderr */ }
                                Ok(Err(io_err)) => {
                                    eprintln!("IO error reading SSH stderr after timeout: {io_err}")
                                }
                                Err(_) => eprintln!(
                                    "Timeout reading SSH stderr after overall establishment timeout."
                                ),
                            }
                        }
                        match child_check.try_wait() {
                            Ok(Some(status)) => eprintln!(
                                "SSH process {child_id} exited with status {status} during final error handling."
                            ),
                            Ok(None) => eprintln!(
                                "SSH process {child_id} still running during final error handling."
                            ),
                            Err(e) => eprintln!(
                                "Error checking SSH process status during final error handling: {e}"
                            ),
                        }
                    }
                }
                return Err(SSHTunnelError::TimeoutError(format!(
                    "Failed to establish SSH tunnel to {} within {:?}. Last SSH stderr: {}",
                    local_addr,
                    total_establishment_timeout,
                    stderr_output.trim()
                )));
            }

            debug!(
                "Attempting TCP connection to {} to verify tunnel (try {}s / {}s total)...",
                local_addr,
                start_time.elapsed().as_secs(),
                total_establishment_timeout.as_secs()
            );

            match tokio::time::timeout(
                individual_tcp_connect_timeout,
                tokio::net::TcpStream::connect(&local_addr),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    // Connected successfully
                    drop(stream);
                    debug!(
                        "TCP check successful! SSH tunnel ready on {} -> {}:{} (via {}@{}:{})",
                        local_addr,
                        self.remote_host,
                        self.remote_port,
                        self.ssh_user,
                        self.ssh_host,
                        self.ssh_port
                    );
                    return Ok(self.local_port);
                }
                Ok(Err(e)) => {
                    // TCP connect failed within individual_tcp_connect_timeout
                    debug!(
                        "TCP check to {} failed: {}. Retrying in {}s... ({}/{}s elapsed)",
                        local_addr,
                        e,
                        tcp_check_interval.as_secs(),
                        start_time.elapsed().as_secs(),
                        total_establishment_timeout.as_secs()
                    );
                }
                Err(_) => {
                    // tokio::time::timeout returned an error (individual_tcp_connect_timeout exceeded)
                    debug!(
                        "TCP check to {} timed out. Retrying in {}s... ({}/{}s elapsed)",
                        local_addr,
                        tcp_check_interval.as_secs(),
                        start_time.elapsed().as_secs(),
                        total_establishment_timeout.as_secs()
                    );
                }
            }
            tokio::time::sleep(tcp_check_interval).await;
        }
    }

    /// Find an available local port to use for the tunnel
    async fn find_available_port(&mut self) -> Result<u16, SSHTunnelError> {
        if self.local_port != 0 {
            match TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                self.local_port,
            ))
            .await
            {
                Ok(listener) => {
                    drop(listener);
                    return Ok(self.local_port);
                }
                Err(e) => {
                    return Err(SSHTunnelError::BindError(format!(
                        "Specified local port {} is not available: {}",
                        self.local_port, e
                    )));
                }
            }
        }

        let mut rng = rng();
        for _ in 0..100 {
            let port = rng.random_range(10000_u16..60000_u16);
            if let Ok(listener) = TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                port,
            ))
            .await
            {
                drop(listener);
                self.local_port = port;
                return Ok(port);
            }
        }
        Err(SSHTunnelError::PortError(
            "No available local ports found after 100 attempts".to_string(),
        ))
    }

    /// Check if the SSH tunnel process is currently active.
    pub fn is_active(&self) -> bool {
        if let Ok(mut guard) = self.tunnel_process.lock() {
            if let Some(child) = guard.as_mut() {
                // Try to get exit status without waiting. If it's None, process is running.
                // If it's Some, process has exited.
                match child.try_wait() {
                    Ok(Some(_status)) => false, // Process has exited
                    Ok(None) => true,           // Process is still running
                    Err(_) => false, // Error checking status, assume not active for safety
                }
            } else {
                false // No child process stored
            }
        } else {
            false // Could not acquire lock, assume not active
        }
    }

    /// Stop the SSH tunnel
    pub async fn stop(&self) -> Result<(), SSHTunnelError> {
        if let Ok(mut guard) = self.tunnel_process.lock() {
            if let Some(mut child) = guard.take() {
                debug!("Stopping SSH tunnel process (PID: {:?})...", child.id());
                match child.kill().await {
                    Ok(_) => {
                        debug!("SSH tunnel process killed successfully");
                        match timeout(Duration::from_secs(5), child.wait()).await {
                            Ok(Ok(status)) => {
                                debug!("SSH tunnel process exited with status: {}", status)
                            }
                            Ok(Err(e)) => {
                                eprintln!("Error waiting for SSH tunnel process to exit: {e}")
                            }
                            Err(_) => eprintln!(
                                "Timeout waiting for SSH tunnel process to exit after kill."
                            ),
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to kill SSH tunnel process: {e}. It might have already exited."
                        );
                        return Err(SSHTunnelError::SshCommandError(format!(
                            "Failed to kill SSH tunnel process: {e}"
                        )));
                    }
                }
            } else {
                debug!("No active SSH tunnel process to stop.");
            }
        } else {
            eprintln!("Failed to acquire lock for stopping tunnel process.");
        }
        Ok(())
    }
}

impl Default for SSHTunnel {
    fn default() -> Self {
        SSHTunnel {
            local_port: 0,
            remote_host: String::new(),
            remote_port: 0,
            ssh_user: String::new(),
            ssh_key: None,
            ssh_host: String::new(),
            ssh_port: 22,
            tunnel_process: Arc::new(Mutex::new(None)),
        }
    }
}

impl Drop for SSHTunnel {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.tunnel_process.lock() {
            if let Some(mut child) = guard.take() {
                debug!(
                    "SSHTunnel dropped. Attempting to kill tunnel process (PID: {:?}).",
                    child.id()
                );
                if let Err(e) = child.start_kill() {
                    eprintln!("Error attempting to kill SSH tunnel process in drop: {e}");
                } else {
                    info!("SSH tunnel closed");
                    debug!("SSH tunnel process kill signal sent from drop.");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;

    #[fixture]
    fn basic_tunnel_config() -> SSHTunnelConfig {
        SSHTunnelConfig {
            enabled: true,
            ssh_host: "localhost".to_string(),
            ssh_port: 2222,
            ssh_username: Some("testuser".to_string()),
            ssh_password: None,
            ssh_key_path: None,
        }
    }

    #[rstest]
    fn test_ssh_tunnel_error_display() {
        let auth_err = SSHTunnelError::AuthError("auth failed".to_string());
        assert_eq!(
            auth_err.to_string(),
            "SSH authentication error: auth failed"
        );
        let cmd_err = SSHTunnelError::SshCommandError("cmd exec failed".to_string());
        assert_eq!(
            cmd_err.to_string(),
            "SSH command execution error: cmd exec failed"
        );
    }

    #[rstest]
    fn test_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let tunnel_err = SSHTunnelError::from(io_err);
        assert!(matches!(tunnel_err, SSHTunnelError::IoError(_)));
        assert_eq!(tunnel_err.to_string(), "IO error: file not found");
    }

    #[rstest]
    fn test_ssh_tunnel_default_values() {
        let tunnel = SSHTunnel::default();
        assert_eq!(tunnel.local_port, 0);
        assert_eq!(tunnel.remote_port, 0);
        assert!(tunnel.ssh_user.is_empty());
        assert!(tunnel.ssh_key.is_none());
        assert!(tunnel.tunnel_process.lock().unwrap().is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn test_find_available_port_dynamic() {
        let mut tunnel = SSHTunnel::default();
        let port = tunnel
            .find_available_port()
            .await
            .expect("Failed to find a dynamic port");
        assert_ne!(port, 0, "Dynamically found port should not be 0");
        assert_eq!(
            tunnel.local_port, port,
            "Tunnel should store the dynamically found port"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn test_find_available_port_specific_free() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let free_port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut tunnel = SSHTunnel::default();
        tunnel.local_port = free_port;
        let port = tunnel
            .find_available_port()
            .await
            .expect("Should use specified free port");
        assert_eq!(port, free_port);
    }

    #[rstest]
    #[tokio::test]
    async fn test_find_available_port_specific_taken() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let taken_port = listener.local_addr().unwrap().port();
        let mut tunnel = SSHTunnel::default();
        tunnel.local_port = taken_port;
        let result = tunnel.find_available_port().await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SSHTunnelError::BindError(_)));
        drop(listener);
    }

    #[rstest]
    #[tokio::test]
    #[ignore]
    async fn test_establish_and_stop_tunnel(mut basic_tunnel_config: SSHTunnelConfig) {
        basic_tunnel_config.ssh_host = "localhost".to_string();
        basic_tunnel_config.ssh_port = 2222;

        let mut tunnel = SSHTunnel::new().unwrap();

        let establish_result = tunnel
            .establish(&basic_tunnel_config, "target-db-host", 5432)
            .await;

        match establish_result {
            Ok(local_port) => {
                println!("Tunnel unexpectedly established on local port: {local_port}");
                assert_eq!(tunnel.ssh_host, basic_tunnel_config.ssh_host);
                assert_ne!(local_port, 0);

                tunnel.stop().await.unwrap_or_else(|e| {
                    eprintln!("Error stopping tunnel (might be ok if already stopped): {e}");
                });
                assert!(
                    tunnel.tunnel_process.lock().unwrap().is_none(),
                    "Tunnel process should be None after stop"
                );
            }
            Err(e) => {
                println!("Tunnel establishment failed as expected (or due to actual error): {e}");
                assert!(matches!(
                    e,
                    SSHTunnelError::SshCommandFailed(_)
                        | SSHTunnelError::SshCommandError(_)
                        | SSHTunnelError::TimeoutError(_)
                ));
            }
        }
        tunnel.stop().await.unwrap_or_else(|e| {
            eprintln!("Error stopping tunnel (idempotency check): {e}");
        });
        assert!(
            tunnel.tunnel_process.lock().unwrap().is_none(),
            "Tunnel process should be None after stop, even on failure"
        );
    }
}
