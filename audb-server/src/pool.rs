use anyhow::{anyhow, Result};
use audb_core::tools::{ssh::SshClient, types::Device};
use russh::client::Handle;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::connection::{ConnectionState, DeviceConnection};

/// Types of operations that can be requested
enum DeviceOperation {
    /// Execute a shell command
    Command {
        command: String,
        as_root: bool,
    },
    /// Upload a file via SFTP
    Upload {
        local_path: std::path::PathBuf,
        remote_path: std::path::PathBuf,
    },
    /// Download a file via SFTP
    Download {
        remote_path: std::path::PathBuf,
        local_path: std::path::PathBuf,
    },
    /// Ensure a script is present on the device
    EnsureScript {
        script_name: String,
        remote_path: String,
        content: String,
    },
}

/// Result of a device operation
enum OperationResult {
    /// Command output lines
    Lines(Vec<String>),
    /// Upload success
    UploadOk,
    /// Download success
    DownloadOk,
    /// Script ensured
    ScriptOk,
}

/// Command request for a device
struct DeviceCommandRequest {
    operation: DeviceOperation,
    response_tx: oneshot::Sender<Result<OperationResult>>,
}

/// Connection pool managing SSH sessions to multiple devices
pub struct ConnectionPool {
    connections: Arc<Mutex<HashMap<String, DeviceConnection>>>,
    command_queues: Arc<Mutex<HashMap<String, mpsc::Sender<DeviceCommandRequest>>>>,
}

impl ConnectionPool {
    /// Create a new empty connection pool
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            command_queues: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Add a device to the pool and start its command processor
    pub async fn add_device(&self, device: Device) {
        let host = device.host.clone();

        // Add to connections map
        {
            let mut connections = self.connections.lock().await;
            connections.insert(host.clone(), DeviceConnection::new(device.clone()));
        }

        // Create command queue for this device
        let (tx, rx) = mpsc::channel::<DeviceCommandRequest>(100);

        {
            let mut queues = self.command_queues.lock().await;
            queues.insert(host.clone(), tx);
        }

        // Spawn command processor task for this device
        let connections = Arc::clone(&self.connections);
        tokio::spawn(async move {
            device_command_processor(host, device, rx, connections).await;
        });
    }

    /// Execute a command on a device (queued execution)
    pub async fn execute_command(
        &self,
        host: &str,
        command: &str,
        as_root: bool,
    ) -> Result<Vec<String>> {
        let result = self
            .send_operation(
                host,
                DeviceOperation::Command {
                    command: command.to_string(),
                    as_root,
                },
            )
            .await?;

        match result {
            OperationResult::Lines(lines) => Ok(lines),
            _ => Err(anyhow!("Unexpected operation result")),
        }
    }

    /// Upload a file to a device
    pub async fn upload_file(
        &self,
        host: &str,
        local_path: &Path,
        remote_path: &Path,
    ) -> Result<()> {
        let result = self
            .send_operation(
                host,
                DeviceOperation::Upload {
                    local_path: local_path.to_path_buf(),
                    remote_path: remote_path.to_path_buf(),
                },
            )
            .await?;

        match result {
            OperationResult::UploadOk => Ok(()),
            _ => Err(anyhow!("Unexpected operation result")),
        }
    }

    /// Download a file from a device
    pub async fn download_file(
        &self,
        host: &str,
        remote_path: &Path,
        local_path: &Path,
    ) -> Result<()> {
        let result = self
            .send_operation(
                host,
                DeviceOperation::Download {
                    remote_path: remote_path.to_path_buf(),
                    local_path: local_path.to_path_buf(),
                },
            )
            .await?;

        match result {
            OperationResult::DownloadOk => Ok(()),
            _ => Err(anyhow!("Unexpected operation result")),
        }
    }

    /// Ensure a script is present on the device
    pub async fn ensure_script(
        &self,
        host: &str,
        script_name: &str,
        remote_path: &str,
        content: &str,
    ) -> Result<()> {
        let result = self
            .send_operation(
                host,
                DeviceOperation::EnsureScript {
                    script_name: script_name.to_string(),
                    remote_path: remote_path.to_string(),
                    content: content.to_string(),
                },
            )
            .await?;

        match result {
            OperationResult::ScriptOk => Ok(()),
            _ => Err(anyhow!("Unexpected operation result")),
        }
    }

    /// Send an operation to a device's command queue
    async fn send_operation(
        &self,
        host: &str,
        operation: DeviceOperation,
    ) -> Result<OperationResult> {
        // Get the command queue for this device
        let tx = {
            let queues = self.command_queues.lock().await;
            queues
                .get(host)
                .cloned()
                .ok_or_else(|| anyhow!("Device {} not found", host))?
        };

        // Create oneshot channel for response
        let (response_tx, response_rx) = oneshot::channel();

        // Send operation to device queue
        let request = DeviceCommandRequest {
            operation,
            response_tx,
        };

        tx.send(request)
            .await
            .map_err(|_| anyhow!("Device {} command queue closed", host))?;

        // Wait for response
        response_rx
            .await
            .map_err(|_| anyhow!("Device {} command processor died", host))?
    }

    /// Get list of all devices
    pub async fn list_devices(&self) -> Vec<(String, ConnectionState)> {
        let connections = self.connections.lock().await;
        connections
            .iter()
            .map(|(host, conn)| (host.clone(), conn.state.clone()))
            .collect()
    }

    /// Get device connection info
    pub async fn get_device_info(&self, host: &str) -> Result<DeviceConnection> {
        let connections = self.connections.lock().await;
        connections
            .get(host)
            .cloned()
            .ok_or_else(|| anyhow!("Device {} not found", host))
    }

    /// Get device by host
    #[allow(dead_code)]
    pub async fn get_device(&self, host: &str) -> Result<Device> {
        let connections = self.connections.lock().await;
        connections
            .get(host)
            .map(|conn| conn.device.clone())
            .ok_or_else(|| anyhow!("Device {} not found", host))
    }
}

/// Reconnection backoff configuration
const INITIAL_BACKOFF_MS: u64 = 1000;
const MAX_BACKOFF_MS: u64 = 60000;
const BACKOFF_MULTIPLIER: u64 = 2;

/// Health check interval (60 seconds)
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Command processor for a single device
/// Ensures commands to the same device execute serially
/// Maintains a persistent SSH connection with auto-reconnect
async fn device_command_processor(
    host: String,
    device: Device,
    mut rx: mpsc::Receiver<DeviceCommandRequest>,
    connections: Arc<Mutex<HashMap<String, DeviceConnection>>>,
) {
    info!("Started command processor for device: {}", host);

    // Persistent SSH session - stored here, not in DeviceConnection
    // because Handle<SshClient> is not Clone
    let mut session: Option<Handle<SshClient>> = None;
    let mut connected_since: Option<Instant> = None;
    let mut last_health_check: Option<Instant> = None;
    let mut current_backoff_ms: u64 = INITIAL_BACKOFF_MS;

    // Track which scripts have been uploaded to avoid re-checking every time
    let mut uploaded_scripts: HashSet<String> = HashSet::new();

    while let Some(request) = rx.recv().await {
        debug!("Processing operation for {}", host);

        // Check if we need a health check (only if connected)
        if session.is_some() {
            if let Some(last_check) = last_health_check {
                if last_check.elapsed() > HEALTH_CHECK_INTERVAL {
                    debug!("Running health check for {}", host);
                    if let Some(ref mut sess) = session {
                        match SshClient::exec(sess, "echo 1") {
                            Ok(_) => {
                                debug!("Health check passed for {}", host);
                                last_health_check = Some(Instant::now());
                            }
                            Err(e) => {
                                warn!("Health check failed for {}: {}, will reconnect", host, e);
                                session = None;
                                connected_since = None;
                                uploaded_scripts.clear(); // Scripts may need re-upload after reconnect
                            }
                        }
                    }
                }
            }
        }

        // Try to establish connection if not connected
        if session.is_none() {
            let connect_result = establish_connection(&host, &device, &connections).await;
            match connect_result {
                Ok(sess) => {
                    session = Some(sess);
                    connected_since = Some(Instant::now());
                    last_health_check = Some(Instant::now());
                    current_backoff_ms = INITIAL_BACKOFF_MS; // Reset backoff on success
                    uploaded_scripts.clear(); // Clear script cache on new connection

                    // Update state to connected
                    let mut conns = connections.lock().await;
                    if let Some(conn) = conns.get_mut(&host) {
                        conn.state = ConnectionState::Connected {
                            since: connected_since.unwrap(),
                        };
                    }
                    info!("Established persistent SSH connection to {}", host);
                }
                Err(e) => {
                    warn!("Failed to connect to {}: {}", host, e);

                    // Update state to errored with next retry time
                    let next_retry = Instant::now() + Duration::from_millis(current_backoff_ms);
                    {
                        let mut conns = connections.lock().await;
                        if let Some(conn) = conns.get_mut(&host) {
                            conn.state = ConnectionState::Errored {
                                error: e.to_string(),
                                next_retry: Some(next_retry),
                            };
                            conn.stats.last_error = Some(e.to_string());
                        }
                    }

                    // Send error response
                    let _ = request.response_tx.send(Err(e));

                    // Apply backoff before next attempt
                    current_backoff_ms =
                        (current_backoff_ms * BACKOFF_MULTIPLIER).min(MAX_BACKOFF_MS);
                    continue;
                }
            }
        }

        // Execute the operation using persistent session
        let result = if let Some(ref mut sess) = session {
            execute_operation(sess, &device, request.operation, &mut uploaded_scripts).await
        } else {
            Err(anyhow!("No active session"))
        };

        // Handle result and potential reconnection
        match &result {
            Ok(_) => {
                // Update stats
                let mut conns = connections.lock().await;
                if let Some(conn) = conns.get_mut(&host) {
                    conn.stats.successful_commands += 1;
                    if let Some(since) = connected_since {
                        conn.state = ConnectionState::Connected { since };
                    }
                }
            }
            Err(e) => {
                let error_str = e.to_string();

                // Check if this is a connection error that requires reconnection
                if is_connection_error(&error_str) {
                    warn!(
                        "Connection error for {}: {}, marking for reconnection",
                        host, error_str
                    );
                    session = None;
                    connected_since = None;
                    last_health_check = None;
                    uploaded_scripts.clear();
                }

                // Update stats
                let mut conns = connections.lock().await;
                if let Some(conn) = conns.get_mut(&host) {
                    conn.stats.failed_commands += 1;
                    conn.stats.last_error = Some(error_str.clone());

                    if session.is_none() {
                        conn.state = ConnectionState::Disconnected;
                    }
                }
            }
        }

        // Send response back
        if request.response_tx.send(result).is_err() {
            warn!("Command response channel closed for {}", host);
        }
    }

    info!("Command processor stopped for device: {}", host);
}

/// Establish a new SSH connection to a device
async fn establish_connection(
    host: &str,
    device: &Device,
    connections: &Arc<Mutex<HashMap<String, DeviceConnection>>>,
) -> Result<Handle<SshClient>> {
    // Update state to connecting
    {
        let mut conns = connections.lock().await;
        if let Some(conn) = conns.get_mut(host) {
            conn.state = ConnectionState::Connecting {
                attempt: conn.stats.connect_attempts as u32 + 1,
                next_retry: Instant::now(),
            };
            conn.stats.connect_attempts += 1;
        }
    }

    // Establish SSH connection
    SshClient::connect(&device.host, device.port, &device.auth_path())
}

/// Execute an operation on an existing SSH session
async fn execute_operation(
    session: &mut Handle<SshClient>,
    device: &Device,
    operation: DeviceOperation,
    uploaded_scripts: &mut HashSet<String>,
) -> Result<OperationResult> {
    match operation {
        DeviceOperation::Command { command, as_root } => {
            let lines = if as_root {
                SshClient::exec_as_devel_su(session, &command, &device.root_password)?
            } else {
                SshClient::exec(session, &command)?
            };
            Ok(OperationResult::Lines(lines))
        }
        DeviceOperation::Upload {
            local_path,
            remote_path,
        } => {
            SshClient::upload(session, &local_path, &remote_path)?;
            Ok(OperationResult::UploadOk)
        }
        DeviceOperation::Download {
            remote_path,
            local_path,
        } => {
            SshClient::download(session, &remote_path, &local_path)?;
            Ok(OperationResult::DownloadOk)
        }
        DeviceOperation::EnsureScript {
            script_name,
            remote_path,
            content,
        } => {
            // Check if we've already uploaded this script in this session
            if uploaded_scripts.contains(&script_name) {
                return Ok(OperationResult::ScriptOk);
            }

            // Check if script exists with correct size
            let expected_size = content.len();
            let check_cmd = format!(
                "test -f {} && stat -c %s {} || echo 0",
                remote_path, remote_path
            );

            let result = SshClient::exec(session, &check_cmd)?;
            let current_size: usize = result.first().and_then(|s| s.parse().ok()).unwrap_or(0);

            if current_size != expected_size {
                // Upload needed
                let temp_file =
                    std::env::temp_dir().join(Path::new(&remote_path).file_name().unwrap());
                std::fs::write(&temp_file, &content)?;

                SshClient::upload(session, &temp_file, Path::new(&remote_path))?;

                // Make executable
                SshClient::exec(session, &format!("chmod +x {}", remote_path))?;

                // Cleanup local temp
                std::fs::remove_file(&temp_file).ok();

                debug!("Uploaded script {} to {}", script_name, remote_path);
            }

            // Mark as uploaded for this session
            uploaded_scripts.insert(script_name);

            Ok(OperationResult::ScriptOk)
        }
    }
}

/// Check if an error indicates a connection problem that requires reconnection
fn is_connection_error(error: &str) -> bool {
    let connection_error_patterns = [
        "connection",
        "disconnect",
        "timeout",
        "broken pipe",
        "reset by peer",
        "channel",
        "session",
        "eof",
        "closed",
    ];

    let error_lower = error.to_lowercase();
    connection_error_patterns
        .iter()
        .any(|pattern| error_lower.contains(pattern))
}

impl Default for ConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}
