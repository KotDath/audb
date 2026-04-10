use anyhow::{anyhow, Result};
use audb_core::tools::{ssh::SshClient, types::Device};
use russh::client::Handle;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};

use crate::connection::{ConnectionState, DeviceConnection};

/// Types of operations that can be requested
enum DeviceOperation {
    /// Execute a shell command
    Command { command: String, as_root: bool },
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
}

/// Result of a device operation
enum OperationResult {
    /// Command output lines
    Lines(Vec<String>),
    /// Upload success
    UploadOk,
    /// Download success
    DownloadOk,
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
        let device_id = device.id.clone();

        // Add to connections map
        {
            let mut connections = self.connections.lock().await;
            connections.insert(device_id.clone(), DeviceConnection::new(device.clone()));
        }

        // Create command queue for this device
        let (tx, rx) = mpsc::channel::<DeviceCommandRequest>(100);

        {
            let mut queues = self.command_queues.lock().await;
            queues.insert(device_id.clone(), tx);
        }

        // Spawn command processor task for this device
        let connections = Arc::clone(&self.connections);
        tokio::spawn(async move {
            device_command_processor(device_id, device, rx, connections).await;
        });
    }

    pub async fn ensure_device(&self, device: Device) {
        let exists = {
            let mut connections = self.connections.lock().await;
            if let Some(conn) = connections.get_mut(&device.id) {
                conn.device = device.clone();
                true
            } else {
                false
            }
        };
        if !exists {
            self.add_device(device).await;
        }
    }

    /// Execute a command on a device (queued execution)
    pub async fn execute_command(
        &self,
        device_id: &str,
        command: &str,
        as_root: bool,
    ) -> Result<Vec<String>> {
        let result = self
            .send_operation(
                device_id,
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
        device_id: &str,
        local_path: &Path,
        remote_path: &Path,
    ) -> Result<()> {
        let result = self
            .send_operation(
                device_id,
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
        device_id: &str,
        remote_path: &Path,
        local_path: &Path,
    ) -> Result<()> {
        let result = self
            .send_operation(
                device_id,
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

    /// Send an operation to a device's command queue
    async fn send_operation(
        &self,
        device_id: &str,
        operation: DeviceOperation,
    ) -> Result<OperationResult> {
        // Get the command queue for this device
        let tx = {
            let queues = self.command_queues.lock().await;
            queues
                .get(device_id)
                .cloned()
                .ok_or_else(|| anyhow!("Device {} not found", device_id))?
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
            .map_err(|_| anyhow!("Device {} command queue closed", device_id))?;

        // Wait for response
        response_rx
            .await
            .map_err(|_| anyhow!("Device {} command processor died", device_id))?
    }

    /// Get list of all devices
    #[allow(dead_code)]
    pub async fn list_devices(&self) -> Vec<(String, ConnectionState)> {
        let connections = self.connections.lock().await;
        connections
            .iter()
            .map(|(device_id, conn)| (device_id.clone(), conn.state.clone()))
            .collect()
    }

    /// Get device connection info
    pub async fn get_device_info(&self, device_id: &str) -> Result<DeviceConnection> {
        let connections = self.connections.lock().await;
        connections
            .get(device_id)
            .cloned()
            .ok_or_else(|| anyhow!("Device {} not found", device_id))
    }

    pub async fn reset_device(&self, device_id: &str) -> Result<()> {
        let mut connections = self.connections.lock().await;
        let conn = connections
            .get_mut(device_id)
            .ok_or_else(|| anyhow!("Device {} not found", device_id))?;
        conn.state = ConnectionState::Disconnected;
        conn.stats.last_error = None;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn get_device(&self, device_id: &str) -> Result<Device> {
        let connections = self.connections.lock().await;
        connections
            .get(device_id)
            .map(|conn| conn.device.clone())
            .ok_or_else(|| anyhow!("Device {} not found", device_id))
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
    device_id: String,
    initial_device: Device,
    mut rx: mpsc::Receiver<DeviceCommandRequest>,
    connections: Arc<Mutex<HashMap<String, DeviceConnection>>>,
) {
    info!("Started command processor for device: {}", device_id);

    // Persistent SSH session - stored here, not in DeviceConnection
    // because Handle<SshClient> is not Clone
    let mut session: Option<Handle<SshClient>> = None;
    let mut connected_since: Option<Instant> = None;
    let mut last_health_check: Option<Instant> = None;
    let mut current_backoff_ms: u64 = INITIAL_BACKOFF_MS;

    while let Some(request) = rx.recv().await {
        debug!("Processing operation for {}", device_id);
        let device = current_device(&connections, &device_id)
            .await
            .unwrap_or_else(|| initial_device.clone());

        // Check if we need a health check (only if connected)
        if session.is_some() {
            if let Some(last_check) = last_health_check {
                if last_check.elapsed() > HEALTH_CHECK_INTERVAL {
                    debug!("Running health check for {}", device_id);
                    if let Some(ref mut sess) = session {
                        match SshClient::exec(sess, "echo 1") {
                            Ok(_) => {
                                debug!("Health check passed for {}", device_id);
                                last_health_check = Some(Instant::now());
                            }
                            Err(e) => {
                                warn!(
                                    "Health check failed for {}: {}, will reconnect",
                                    device_id, e
                                );
                                session = None;
                                connected_since = None;
                            }
                        }
                    }
                }
            }
        }

        // Try to establish connection if not connected
        if session.is_none() {
            let connect_result = establish_connection(&device_id, &device, &connections).await;
            match connect_result {
                Ok(sess) => {
                    session = Some(sess);
                    connected_since = Some(Instant::now());
                    last_health_check = Some(Instant::now());
                    current_backoff_ms = INITIAL_BACKOFF_MS; // Reset backoff on success

                    // Update state to connected
                    let mut conns = connections.lock().await;
                    if let Some(conn) = conns.get_mut(&device_id) {
                        conn.state = ConnectionState::Connected {
                            since: connected_since.unwrap(),
                        };
                    }
                    info!("Established persistent SSH connection to {}", device_id);
                }
                Err(e) => {
                    warn!("Failed to connect to {}: {}", device_id, e);

                    // Update state to errored with next retry time
                    let next_retry = Instant::now() + Duration::from_millis(current_backoff_ms);
                    {
                        let mut conns = connections.lock().await;
                        if let Some(conn) = conns.get_mut(&device_id) {
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
            execute_operation(sess, &device, request.operation).await
        } else {
            Err(anyhow!("No active session"))
        };

        // Handle result - keep connection alive, only update stats
        match &result {
            Ok(_) => {
                // Update stats
                let mut conns = connections.lock().await;
                if let Some(conn) = conns.get_mut(&device_id) {
                    conn.stats.successful_commands += 1;
                    if let Some(since) = connected_since {
                        conn.state = ConnectionState::Connected { since };
                    }
                }
            }
            Err(e) => {
                let error_str = e.to_string();

                // Update stats but don't disconnect - let health check handle real disconnections
                let mut conns = connections.lock().await;
                if let Some(conn) = conns.get_mut(&device_id) {
                    conn.stats.failed_commands += 1;
                    conn.stats.last_error = Some(error_str.clone());
                    // Keep state as connected - health check will detect real disconnections
                }
            }
        }

        // Send response back
        if request.response_tx.send(result).is_err() {
            warn!("Command response channel closed for {}", device_id);
        }
    }

    info!("Command processor stopped for device: {}", device_id);
}

/// Establish a new SSH connection to a device
async fn establish_connection(
    device_id: &str,
    device: &Device,
    connections: &Arc<Mutex<HashMap<String, DeviceConnection>>>,
) -> Result<Handle<SshClient>> {
    // Update state to connecting
    {
        let mut conns = connections.lock().await;
        if let Some(conn) = conns.get_mut(device_id) {
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

async fn current_device(
    connections: &Arc<Mutex<HashMap<String, DeviceConnection>>>,
    device_id: &str,
) -> Option<Device> {
    let connections = connections.lock().await;
    connections.get(device_id).map(|conn| conn.device.clone())
}

/// Execute an operation on an existing SSH session
async fn execute_operation(
    session: &mut Handle<SshClient>,
    device: &Device,
    operation: DeviceOperation,
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
    }
}

impl Default for ConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}
